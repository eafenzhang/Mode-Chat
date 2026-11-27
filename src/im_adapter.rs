//! IM 通道外挂适配器（stdio JSONL 桥 + 在线更新）。
//!
//! - 适配器包：`{data_root}/v2/im-adapters/manifest.json` + `<channel>/adapter.mjs`；
//!   manifest 里 `version` 是包整体版本，`package`/`sha256` 供在线更新用；
//! - 进程桥：每个 Bot 一个 `node adapter.mjs` 子进程，启动时写一条
//!   `{"op":"start",...}`（凭据从 TokenVault 读出后放进 config），后续事件走
//!   stdout JSONL：status/message/log；
//! - 回复：`BotReplySink` 把文本（或媒体路径）封装成 `{"op":"reply",...}` 写 stdin，
//!   `context_token` 是适配器自己的回复凭据（入站时透传），应用不理解其内容；
//! - 更新：GET 远端 manifest（版本比较）→ 下载 zip → sha256 → `tar -xf` 解压
//!   到临时目录 → 原子 swap → 重启适配器；失败回滚旧目录；
//! - 内置兜底：`adapters/` 目录通过 `include_str!` 编进应用，数据目录缺失时
//!   先复制一份（保证全新安装开箱可用）；manifest 里没有的通道继续走内置
//!   Rust 协议（微信轮询 / 企微 WS / 钉钉 Stream）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

use crate::im_service::now_ms;

pub const DEFAULT_SOURCE_URL: &str =
    "https://raw.githubusercontent.com/eafenzhang/Mode-Chat/main/adapters/manifest.json";

const BUILTIN_MANIFEST: &str = include_str!("../adapters/manifest.json");
const BUILTIN_DINGTALK: &str = include_str!("../adapters/dingtalk/adapter.mjs");
const BUILTIN_FEISHU: &str = include_str!("../adapters/feishu/adapter.mjs");
const BUILTIN_WECOM: &str = include_str!("../adapters/wecom/adapter.mjs");

/// 适配器数据目录：`{data_root}/v2/im-adapters`。
pub fn adapters_dir() -> PathBuf {
    crate::data::data_root().join("v2").join("im-adapters")
}

fn manifest_path() -> PathBuf {
    adapters_dir().join("manifest.json")
}

/// 远端源 URL（prefs 覆盖默认仓库地址）。
pub fn source_url() -> String {
    prefs_read_string("im_adapter_source_url")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SOURCE_URL.to_string())
}

/// 源 URL 可配置：设置 UI 化之前的预留入口（README 已承诺）。
#[allow(dead_code)]
pub fn set_source_url(value: &str) -> Result<(), String> {
    prefs_write_string("im_adapter_source_url", value.trim())
}

/// prefs.json 字符串键读写（与 ui_kit 偏好同一文件，缺键返回 None）。
fn prefs_path() -> PathBuf {
    crate::instance::state_dir().join("prefs.json")
}

fn prefs_read_string(key: &str) -> Option<String> {
    let bytes = std::fs::read(prefs_path()).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

#[allow(dead_code)]
fn prefs_write_string(key: &str, value: &str) -> Result<(), String> {
    let path = prefs_path();
    let mut root: serde_json::Value = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !root.is_object() {
        root = serde_json::json!({});
    }
    root[key] = serde_json::json!(value);
    let _ = atomic_file::write_json_atomic(&path, &root, &atomic_file::WriteOptions::default());
    Ok(())
}

/// 解析 node 可执行文件：系统 PATH 里的 node 优先，回落数据目录的便携版。
fn resolve_node() -> Result<PathBuf, String> {
    if crate::mcp_host::node_available() {
        return Ok(PathBuf::from("node"));
    }
    let bundled = crate::mcp_host::runtime_node_dir_public()
        .join(if cfg!(windows) { "node.exe" } else { "node" });
    if bundled.is_file() {
        return Ok(bundled);
    }
    Err("Node 运行时不可用：适配器需要 Node（首次使用连接器/适配器时会自动下载官方便携版）".to_string())
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AdapterManifest {
    pub version: u64,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub adapters: HashMap<String, AdapterEntry>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AdapterEntry {
    pub entry: String,
    #[serde(default)]
    pub providers: Option<Vec<String>>,
}

impl AdapterManifest {
    /// 某个 provider 是否有适配器（`providers` 列出归属通道；空列表按通道名匹配）。
    pub fn covers(&self, provider: &str) -> bool {
        self.entry_for(provider).is_some()
    }

    pub fn entry_for(&self, provider: &str) -> Option<String> {
        let owned = provider.to_string();
        self.adapters
            .iter()
            .find(|(name, entry)| {
                let providers = entry
                    .providers
                    .clone()
                    .unwrap_or_else(|| vec![name.to_string()]);
                providers.iter().any(|value| value.eq_ignore_ascii_case(&owned))
            })
            .map(|(_, entry)| entry.entry.clone())
    }
}

/// 读本地 manifest；数据目录没有时用内置包铺一份（幂等）。
pub fn load_manifest() -> Result<AdapterManifest, String> {
    ensure_builtin_installed()?;
    let bytes = std::fs::read(manifest_path())
        .map_err(|error| format!("读取适配器 manifest 失败：{error}"))?;
    serde_json::from_slice::<AdapterManifest>(&bytes)
        .map_err(|error| format!("适配器 manifest 解析失败：{error}"))
}

fn ensure_builtin_installed() -> Result<(), String> {
    let path = manifest_path();
    if path.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(adapters_dir())
        .map_err(|error| format!("创建适配器目录失败：{error}"))?;
    std::fs::write(&path, BUILTIN_MANIFEST)
        .map_err(|error| format!("写入内置 manifest 失败：{error}"))?;
    for (relative, content) in [
        ("dingtalk/adapter.mjs", BUILTIN_DINGTALK),
        ("feishu/adapter.mjs", BUILTIN_FEISHU),
        ("wecom/adapter.mjs", BUILTIN_WECOM),
    ] {
        let target = adapters_dir().join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建适配器子目录失败：{error}"))?;
        }
        std::fs::write(&target, content)
            .map_err(|error| format!("写入内置适配器失败：{error}"))?;
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// 在线检查：`Ok(Some(remote))` = 有新版本；`Ok(None)` = 已是最新。
pub async fn check_update() -> Result<Option<(AdapterManifest, String)>, String> {
    let local = load_manifest()?.version;
    let remote = fetch_manifest(source_url().as_str()).await?;
    if remote.version > local {
        let package = remote
            .package
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "远端 manifest 缺少 package（下载地址）".to_string())?;
        Ok(Some((remote, package)))
    } else {
        Ok(None)
    }
}

async fn fetch_manifest(url: &str) -> Result<AdapterManifest, String> {
    let response = reqwest::get(url)
        .await
        .map_err(|error| format!("拉取适配器清单失败：{error}"))?
        .error_for_status()
        .map_err(|error| format!("拉取适配器清单失败：{error}"))?
        .text()
        .await
        .map_err(|error| format!("读取适配器清单失败：{error}"))?;
    serde_json::from_str::<AdapterManifest>(&response)
        .map_err(|error| format!("远端 manifest 解析失败：{error}"))
}

/// 在线更新：下载 → sha256 → 解压 → 验证 → 原子 swap。返回新版本号。
pub async fn update() -> Result<u64, String> {
    let Some((remote, package_url)) = check_update().await? else {
        return Ok(load_manifest()?.version);
    };
    let bytes = reqwest::get(&package_url)
        .await
        .map_err(|error| format!("下载适配器包失败：{error}"))?
        .bytes()
        .await
        .map_err(|error| format!("读取适配器包失败：{error}"))?;
    if let Some(expected) = remote.sha256.as_deref().filter(|value| !value.is_empty()) {
        let actual = sha256_hex(&bytes);
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(format!(
                "适配器包校验失败：sha256 不匹配（期望 {expected}，实际 {actual}）"
            ));
        }
    }
    let staging = adapters_dir().join(format!(".update-{}", now_ms()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|error| format!("创建临时更新目录失败：{error}"))?;
    let zip_path = staging.join("adapters.zip");
    std::fs::write(&zip_path, &bytes)
        .map_err(|error| format!("写入适配器包失败：{error}"))?;
    extract_zip(&zip_path, &staging)?;
    let _ = std::fs::remove_file(&zip_path);
    // 解压内容必须自带合法 manifest（version 不得低于远端清单）。
    let staged_manifest = staging.join("manifest.json");
    let staged: AdapterManifest = serde_json::from_slice(
        &std::fs::read(&staged_manifest)
            .map_err(|error| format!("解压后缺 manifest.json：{error}"))?,
    )
    .map_err(|error| format!("解压后 manifest 解析失败：{error}"))?;
    if staged.version < remote.version {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("解压包版本低于远端清单，已回滚。".to_string());
    }
    // 原子 swap：旧目录 → .bak，staging → 正式目录，成功后删 .bak。
    let target = adapters_dir();
    let backup = adapters_dir().join(".backup");
    let _ = std::fs::remove_dir_all(&backup);
    if target.exists() {
        std::fs::rename(&target, &backup)
            .map_err(|error| format!("备份旧适配器目录失败：{error}"))?;
    }
    if std::fs::rename(&staging, &target).is_err() {
        if backup.exists() {
            let _ = std::fs::rename(&backup, &target);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err("适配器目录替换失败，已回滚旧包。".to_string());
    }
    let _ = std::fs::remove_dir_all(&backup);
    Ok(remote.version)
}

/// Windows 10+ 自带 bsdtar 能解 zip（与 mcp_host 的 Node 下载同一手段）。
fn extract_zip(zip_path: &Path, dest: &Path) -> Result<(), String> {
    let mut command = std::process::Command::new("tar");
    command.arg("-xf").arg(zip_path).arg("-C").arg(dest);
    crate::process_console::no_console(&mut command);
    let status = command
        .status()
        .map_err(|error| format!("解压适配器包失败：{error}"))?;
    if !status.success() {
        return Err(format!("解压适配器包失败（tar 退出码 {status}）"));
    }
    Ok(())
}

// ---------- 进程桥 ----------

/// reply 共享句柄：spawn 后 clone 给 BotReplySink。
pub type AdapterShared = Arc<tokio::sync::Mutex<tokio::process::ChildStdin>>;

/// 一个适配器子进程的运行句柄。
pub struct AdapterProcess {
    pub child: Child,
    pub stdin: AdapterShared,
    pub token: CancellationToken,
}

/// 启动适配器子进程并写 `start` 配置；事件回调在独立任务里跑。
/// 返回（进程句柄，事件接收任务）。`on_message`/`on_status` 由运行时层给。
pub async fn spawn_adapter(
    entry: &str,
    bot_id: &str,
    config_json: serde_json::Value,
    on_message: impl Fn(serde_json::Value) + Send + Sync + 'static,
    on_status: impl Fn(&str, Option<String>) + Send + Sync + 'static,
    on_log: impl Fn(&str) + Send + Sync + 'static,
) -> Result<AdapterProcess, String> {
    let node = resolve_node()?;
    let script = adapters_dir().join(entry);
    if !script.exists() {
        return Err(format!("适配器入口不存在：{}", script.display()));
    }
    let mut child = Command::new(node)
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("启动适配器进程失败：{error}"))?;
    let stdin: AdapterShared = Arc::new(tokio::sync::Mutex::new(
        child
            .stdin
            .take()
            .ok_or_else(|| "适配器 stdin 不可用".to_string())?,
    ));
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "适配器 stdout 不可用".to_string())?;
    let token = CancellationToken::new();
    let start_line = serde_json::json!({
        "op": "start",
        "bot_id": bot_id,
        "config": config_json,
    });
    {
        let mut writer = stdin.lock().await;
        writer
            .write_all(format!("{start_line}\n").as_bytes())
            .await
            .map_err(|error| format!("写适配器启动配置失败：{error}"))?;
        writer.flush().await.map_err(|error| format!("{error}"))?;
    }
    let token_for_reader = token.clone();
    let on_message = Arc::new(on_message);
    let on_status = Arc::new(on_status);
    let on_log = Arc::new(on_log);
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            tokio::select! {
                _ = token_for_reader.cancelled() => break,
                read = reader.read_line(&mut line) => {
                    match read {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let trimmed = line.trim().to_string();
                            line.clear();
                            if trimmed.is_empty() {
                                continue;
                            }
                            let Ok(value) = serde_json::from_str::<serde_json::Value>(&trimmed) else {
                                continue;
                            };
                            match value.get("event").and_then(serde_json::Value::as_str) {
                                Some("message") => on_message(value),
                                Some("status") => on_status(
                                    value.get("status").and_then(serde_json::Value::as_str).unwrap_or("reconnecting"),
                                    value.get("detail").and_then(serde_json::Value::as_str).map(str::to_string),
                                ),
                                Some("log") => on_log(
                                    value.get("line").and_then(serde_json::Value::as_str).unwrap_or_default(),
                                ),
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    });
    Ok(AdapterProcess {
        child,
        stdin,
        token,
    })
}

impl AdapterProcess {

    pub async fn shutdown(mut self) {
        self.token.cancel();
        let _ = self.child.kill().await;
    }
}


/// 把通道凭据整理成适配器 config（TokenVault 读取在调用方完成）。
pub struct AdapterCredentialSource;

impl AdapterCredentialSource {
    pub fn config_for(&self, provider: &str, bot: &bots_service::BotConfig, secret: Option<String>) -> serde_json::Value {
        match provider {
            "dingtalk" => serde_json::json!({
                "clientId": bot.dingtalk_client_id.clone().unwrap_or_default(),
                "clientSecret": secret.unwrap_or_default(),
            }),
            "feishu" | "lark" => serde_json::json!({
                "appId": bot.feishu_app_id.clone().unwrap_or_default(),
                "appSecret": secret.unwrap_or_default(),
            }),
            "wecom" => serde_json::json!({
                "botId": bot.wecom_bot_id.clone().unwrap_or_default(),
                "secret": secret.unwrap_or_default(),
            }),
            _ => serde_json::json!({}),
        }
    }
}
