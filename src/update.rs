//! 更新：探测 + 下载校验 + 自替换（P4-3 / L5）。
//!
//! 清单是 Mode-Chat 的 `gpui-latest.json`（`platforms.windows-x86_64`）
//! （兼容 `windows-x64`）下读 `url` / `sha256` / `size`。没有 sha256 的清单只做探测，
//! 不提供自更新（fail closed）。
//!
//! 替换方式：下载 zip → SHA-256 校验 → 解压出新 exe → 启动一个 PowerShell 等待本进程
//! 退出后 `备份 → 覆盖 → 启动`；启动失败回滚备份。开发构建（路径含 `target`）默认拒绝
//! 自替换，`BOT_PILOT_ALLOW_SELF_UPDATE=1` 可覆盖（仅测试）。
//!
//! 为什么仍用 curl.exe：Windows 10 自带 `System32\curl.exe`，不引 HTTP/TLS 依赖；
//! 解压用 Windows 自带 `tar.exe`。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// 更新源清单端点：GPUI 专用清单（与壳的 Tauri `latest.json` 分开，避免互相覆盖）。
pub const MANIFEST_ENDPOINT: &str =
    "https://github.com/eafenzhang/Mode-Chat/releases/latest/download/gpui-latest.json";

/// 发布页（人工下载入口）。
pub const RELEASES_PAGE: &str = "https://github.com/eafenzhang/Mode-Chat/releases/latest";

/// 可下载的更新目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub size: Option<u64>,
}

/// 探测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// 已探测：当前版本 / 最新版本 / 是否有更新 / 可自更新目标。
    Done {
        current: String,
        latest: String,
        newer: bool,
        target: Option<Target>,
    },
    /// 发布源还没有任何版本（404）：不是网络/解析错误，UI 用中性提示。
    NoRelease,
    Failed(String),
}

/// 当前版本：优先构建期注入的 `MODE_BOT_VERSION`（随产品打包时由 CI 写入），
/// 否则退回本 crate 的版本号（试点独立运行时）。
pub fn current_version() -> String {
    option_env!("MODE_BOT_VERSION")
        .map(str::to_string)
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

/// 清单地址：`BOT_PILOT_UPDATE_MANIFEST` 可覆盖（本机 `file://` 或内网镜像）。
pub fn manifest_url() -> String {
    std::env::var("BOT_PILOT_UPDATE_MANIFEST")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| MANIFEST_ENDPOINT.to_string())
}

/// 拉清单并比对。网络/解析失败一律返回 `Check::Failed`。
pub fn check(timeout: Duration) -> Check {
    match fetch_manifest(timeout) {
        Ok(manifest) => {
            let current = current_version();
            let newer = match (version_tuple(&current), version_tuple(&manifest.version)) {
                (Some(current), Some(latest)) => latest > current,
                _ => false,
            };
            Check::Done {
                current,
                latest: manifest.version,
                newer,
                target: manifest.target,
            }
        }
        Err(error) if error == NO_RELEASE => Check::NoRelease,
        Err(error) => Check::Failed(error),
    }
}

const NO_RELEASE: &str = "__no_release__";

/// 用系统 `curl.exe` 取清单并解析版本/平台目标。
fn fetch_manifest(timeout: Duration) -> Result<Manifest, String> {
    let seconds = timeout.as_secs().max(1).to_string();
    let url = manifest_url();
    let mut curl = Command::new("curl");
    curl.args([
        "--silent",
        "--show-error",
        "--location",
        "--max-time",
        &seconds,
        "--header",
        "Accept: application/json",
        "--write-out",
        "|HTTP:%{http_code}",
        &url,
    ])
    .stdin(Stdio::null())
    .stderr(Stdio::piped());
    // GUI 发布包没有控制台：控制台子进程不加这个标志会弹黑框。
    crate::process_console::no_console(&mut curl);
    let output = curl
        .output()
        .map_err(|error| format!("调用 curl 失败：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        return Err(if detail.is_empty() {
            format!("网络请求失败（curl 退出码 {:?}）", output.status.code())
        } else {
            format!("网络请求失败：{detail}")
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let (body, status) = stdout
        .rsplit_once("|HTTP:")
        .map_or((stdout.as_ref(), ""), |(body, status)| (body, status));
    if status.trim() == "404" {
        return Err(NO_RELEASE.to_string());
    }
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| format!("清单不是合法 JSON：{error}"))?;
    parse_manifest(&value)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Manifest {
    version: String,
    target: Option<Target>,
}

/// 解析清单；平台目标缺少/非法 sha256 时只保留版本（探测可，自更新不可）。
fn parse_manifest(value: &serde_json::Value) -> Result<Manifest, String> {
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .ok_or_else(|| "清单缺少 version 字段".to_string())?
        .to_string();
    let platform = value
        .get("platforms")
        .and_then(|platforms| {
            platforms
                .get("windows-x86_64")
                .or_else(|| platforms.get("windows-x64"))
        });
    let target = platform.and_then(|platform| {
        let url = platform
            .get("url")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|url| !url.is_empty())?;
        let sha256 = platform
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|sha| is_sha256_hex(sha))?;
        let size = platform.get("size").and_then(serde_json::Value::as_u64);
        Some(Target {
            version: version.clone(),
            url: url.to_string(),
            sha256: sha256.to_ascii_lowercase(),
            size,
        })
    });
    Ok(Manifest { version, target })
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

/// 开发构建默认拒绝自替换（避免覆盖 `target/debug` 或 CI 产物）。
pub fn can_self_update(current_exe: &Path) -> Result<(), String> {
    if std::env::var("BOT_PILOT_ALLOW_SELF_UPDATE").ok().as_deref() == Some("1") {
        return Ok(());
    }
    let lower = current_exe
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if lower.contains("/target/") {
        return Err("开发构建（target 目录）不执行自替换；请用安装包测试更新".to_string());
    }
    Ok(())
}

/// 下载后的暂存结果。
#[derive(Debug, Clone)]
pub struct StagedUpdate {
    pub new_exe: PathBuf,
    pub work_dir: PathBuf,
    /// 更新包里带的运行时目录（agent/node/config），替换 exe 后一并覆盖。
    pub runtime_dirs: Vec<String>,
}

/// 下载 zip、校验大小/SHA-256、解压并定位新 exe。
pub fn download_and_stage(target: &Target) -> Result<StagedUpdate, String> {
    let work_dir = std::env::temp_dir().join(format!(
        "modebot-gpui-update-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&work_dir)
        .map_err(|error| format!("创建更新目录失败：{error}"))?;
    let zip_path = work_dir.join("update.zip");
    let seconds = "600";
    let mut curl = Command::new("curl");
    curl.args([
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--max-time",
        seconds,
        "--output",
    ])
    .arg(&zip_path)
    .arg(&target.url)
    .stdin(Stdio::null())
    .stderr(Stdio::piped());
    crate::process_console::no_console(&mut curl);
    let output = curl
        .output()
        .map_err(|error| format!("调用 curl 失败：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.trim().is_empty() {
            format!("curl 退出码 {:?}", output.status.code())
        } else {
            stderr.trim().to_string()
        };
        return Err(format!("下载更新失败：{detail}"));
    }
    let bytes = std::fs::metadata(&zip_path)
        .map_err(|error| format!("读取更新包失败：{error}"))?
        .len();
    if let Some(expected) = target.size
        && bytes != expected
    {
        return Err(format!(
            "更新包大小不符：期望 {expected} 字节，实际 {bytes} 字节"
        ));
    }
    let actual = sha256_file(&zip_path)?;
    if actual != target.sha256 {
        return Err(format!(
            "更新包 SHA-256 不符：期望 {}，实际 {actual}",
            target.sha256
        ));
    }
    let mut extract_cmd = Command::new("tar");
    extract_cmd
        .args(["-xf"])
        .arg(&zip_path)
        .arg("-C")
        .arg(&work_dir)
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    crate::process_console::no_console(&mut extract_cmd);
    let extract = extract_cmd
        .output()
        .map_err(|error| format!("调用 tar 失败：{error}"))?;
    if !extract.status.success() {
        return Err(format!(
            "解压更新包失败：{}",
            String::from_utf8_lossy(&extract.stderr).trim()
        ));
    }
    // 包内 exe 名跟随产品名（ModeChat.exe），不再是 Cargo 的 bin 名。
    // 两者都接受：老安装包是 bot-gpui-pilot.exe，新包是 ModeChat.exe。
    let new_exe = ["ModeChat.exe", "bot-gpui-pilot.exe"]
        .iter()
        .map(|name| work_dir.join(name))
        .find(|path| path.is_file())
        .ok_or_else(|| {
            "更新包缺少 ModeChat.exe（也没有 bot-gpui-pilot.exe）".to_string()
        })?;
    let runtime_dirs = ["agent", "node", "config"]
        .iter()
        .filter(|name| work_dir.join(name).is_dir())
        .map(|name| (*name).to_string())
        .collect();
    Ok(StagedUpdate {
        new_exe,
        work_dir,
        runtime_dirs,
    })
}

/// 计算文件 SHA-256（小写十六进制，流式读，避免整包进内存）。
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|error| format!("打开文件失败：{error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        use std::io::Read as _;
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("读取文件失败：{error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// 启动替换脚本：等待本进程退出 → 备份 → 覆盖 → 启动；启动失败回滚。
///
/// 调用方在成功后应立即退出进程，让脚本继续。
pub fn spawn_swap_and_restart(current_exe: &Path, staged: &StagedUpdate) -> Result<(), String> {
    let backup = backup_path(current_exe);
    let script_path = staged.work_dir.join("apply-update.ps1");
    let script = swap_script(
        std::process::id(),
        current_exe,
        &staged.new_exe,
        &backup,
        &staged.work_dir,
        &staged.runtime_dirs,
    );
    std::fs::write(&script_path, script)
        .map_err(|error| format!("写入更新脚本失败：{error}"))?;
    let log_path = staged.work_dir.join("apply-update.out.log");
    let log = std::fs::File::create(&log_path).map_err(|error| format!("创建更新日志失败：{error}"))?;
    let log_err = log
        .try_clone()
        .map_err(|error| format!("创建更新日志句柄失败：{error}"))?;
    let mut command = Command::new("powershell");
    command
        .args([
            "-NoProfile",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    // 更新脚本必须无窗口运行（发布包是 GUI 子系统，多一个黑框会挡住替换流程）。
    crate::process_console::no_console(&mut command);
    command
        .spawn()
        .map_err(|error| format!("启动更新脚本失败：{error}"))?;
    Ok(())
}

fn backup_path(current_exe: &Path) -> PathBuf {
    let mut name = current_exe
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| "ModeChat.exe".into());
    name.push(".bak");
    current_exe.with_file_name(name)
}

fn swap_script(
    pid: u32,
    current: &Path,
    new_exe: &Path,
    backup: &Path,
    staged_dir: &Path,
    runtime_dirs: &[String],
) -> String {
    let quote = |path: &Path| path.to_string_lossy().replace('\'', "''");
    let current = quote(current);
    let new_exe = quote(new_exe);
    let backup = quote(backup);
    let log_path = quote(&staged_dir.join("apply-update.log"));
    let staged_dir = quote(staged_dir);
    let runtime_list = runtime_dirs
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ");
    r#"$ErrorActionPreference = 'Stop'
$log = '__LOG__'
"start __PID__" | Out-File -FilePath $log -Append -Encoding utf8
try { Wait-Process -Id __PID__ -Timeout 60 } catch {}
$runtimeOk = $false
for ($attempt = 1; $attempt -le 20; $attempt++) {
  try {
    $runtimeDirs = @(__RUNTIMES__)
    foreach ($name in $runtimeDirs) {
      $src = Join-Path '__STAGED__' $name
      if (Test-Path -LiteralPath $src) {
        $dst = Join-Path (Split-Path -Parent '__CURRENT__') $name
        if (Test-Path -LiteralPath $dst) { Remove-Item -LiteralPath $dst -Recurse -Force }
        Copy-Item -LiteralPath $src -Destination $dst -Recurse -Force
      }
    }
    $runtimeOk = $true
    break
  } catch {
    "runtime copy attempt $attempt failed: $_" | Out-File -FilePath $log -Append -Encoding utf8
    Start-Sleep -Milliseconds 500
  }
}
if (-not $runtimeOk) {
  "runtime copy failed; restarting old version" | Out-File -FilePath $log -Append -Encoding utf8
  Start-Process -FilePath '__CURRENT__' -WorkingDirectory (Split-Path -Parent '__CURRENT__')
  exit 1
}
try {
  Copy-Item -LiteralPath '__CURRENT__' -Destination '__BACKUP__' -Force
  Copy-Item -LiteralPath '__NEW__' -Destination '__CURRENT__' -Force
} catch {
  "exe replace failed: $_" | Out-File -FilePath $log -Append -Encoding utf8
  Start-Process -FilePath '__CURRENT__' -WorkingDirectory (Split-Path -Parent '__CURRENT__')
  exit 1
}
try {
  Start-Process -FilePath '__CURRENT__' -WorkingDirectory (Split-Path -Parent '__CURRENT__')
  "started updated version" | Out-File -FilePath $log -Append -Encoding utf8
} catch {
  "start failed, rolling back: $_" | Out-File -FilePath $log -Append -Encoding utf8
  Copy-Item -LiteralPath '__BACKUP__' -Destination '__CURRENT__' -Force
  Start-Process -FilePath '__CURRENT__' -WorkingDirectory (Split-Path -Parent '__CURRENT__')
}
Start-Sleep -Seconds 20
Remove-Item -LiteralPath '__BACKUP__' -Force -ErrorAction SilentlyContinue
# 保留 apply-update*.log 便于排障，清掉大文件（zip + 解压出的运行时）。
Remove-Item -LiteralPath (Join-Path '__STAGED__' 'update.zip') -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath (Join-Path '__STAGED__' 'ModeChat.exe') -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath (Join-Path '__STAGED__' 'bot-gpui-pilot.exe') -Force -ErrorAction SilentlyContinue
foreach ($name in @('agent', 'node', 'config')) {{
  Remove-Item -LiteralPath (Join-Path '__STAGED__' $name) -Recurse -Force -ErrorAction SilentlyContinue
}}
"#
    .replace("__PID__", &pid.to_string())
    .replace("__LOG__", &log_path)
    .replace("__RUNTIMES__", &runtime_list)
    .replace("__STAGED__", &staged_dir)
    .replace("__CURRENT__", &current)
    .replace("__BACKUP__", &backup)
    .replace("__NEW__", &new_exe)
}

/// `1.2.3` → `(1,2,3)`；允许前缀 `v`，多余段丢弃。
fn version_tuple(raw: &str) -> Option<(u32, u32, u32)> {
    let trimmed = raw.trim().trim_start_matches(['v', 'V']);
    let mut parts = trimmed.split(['.', '-', '+']).filter(|part| !part.is_empty());
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u32>().unwrap_or(0);
    let patch = parts.next().unwrap_or("0").parse::<u32>().unwrap_or(0);
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_tuple_parses_common_shapes() {
        assert_eq!(version_tuple("1.2.3"), Some((1, 2, 3)));
        assert_eq!(version_tuple("v0.0.42"), Some((0, 0, 42)));
        assert_eq!(version_tuple("2.1"), Some((2, 1, 0)));
        assert_eq!(version_tuple("3"), Some((3, 0, 0)));
        assert_eq!(version_tuple("1.2.3-beta.1"), Some((1, 2, 3)));
        assert_eq!(version_tuple("abc"), None);
        assert_eq!(version_tuple(""), None);
    }

    #[test]
    fn newer_comparison_is_ordered() {
        let pair = |a: &str, b: &str| version_tuple(a) > version_tuple(b);
        assert!(pair("0.0.42", "0.0.41"));
        assert!(!pair("0.0.41", "0.0.42"));
        assert!(!pair("1.0.0", "1.0.0"));
        assert!(pair("0.1.0", "0.0.99"));
    }

    #[test]
    fn current_version_is_non_empty() {
        assert!(!current_version().trim().is_empty());
    }

    #[test]
    fn manifest_endpoint_is_gpui_specific() {
        // 与 Tauri 的 latest.json 分开，否则会读到壳的清单（没有 GPUI 包的 sha256）。
        assert_eq!(
            MANIFEST_ENDPOINT,
            "https://github.com/eafenzhang/Mode-Chat/releases/latest/download/gpui-latest.json"
        );
    }

    #[test]
    fn manifest_parses_target() {
        let value = serde_json::json!({
            "version": "0.0.43",
            "platforms": {
                "windows-x86_64": {
                    "url": "https://example.test/app.zip",
                    "sha256": "a".repeat(64),
                    "size": 123
                }
            }
        });
        let manifest = parse_manifest(&value).expect("manifest");
        assert_eq!(manifest.version, "0.0.43");
        let target = manifest.target.expect("target");
        assert_eq!(target.url, "https://example.test/app.zip");
        assert_eq!(target.size, Some(123));
    }

    #[test]
    fn manifest_without_sha256_is_probe_only() {
        let value = serde_json::json!({
            "version": "0.0.43",
            "platforms": { "windows-x64": { "url": "https://example.test/app.zip" } }
        });
        let manifest = parse_manifest(&value).expect("manifest");
        assert!(manifest.target.is_none());
    }

    #[test]
    fn manifest_without_version_is_error() {
        let value = serde_json::json!({ "platforms": {} });
        assert!(parse_manifest(&value).is_err());
    }

    #[test]
    fn dev_build_is_refused_unless_overridden() {
        let dev = Path::new("D:/Mode Bot/Mode/apps/bot-gpui-pilot/target/debug/bot-gpui-pilot.exe");
        assert!(can_self_update(dev).is_err());
        let installed = Path::new("C:/Users/me/AppData/Local/Programs/ModeBot-GPUI/bot-gpui-pilot.exe");
        assert!(can_self_update(installed).is_ok());
    }

    #[test]
    fn sha256_matches_known_content() {
        let dir = std::env::temp_dir().join(format!("modebot-update-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("payload");
        std::fs::write(&file, b"abc").expect("write");
        assert_eq!(
            sha256_file(&file).expect("hash"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn swap_script_waits_and_rolls_back() {
        let script = swap_script(
            42,
            Path::new("C:\\App\\bot.exe"),
            Path::new("C:\\Temp\\new.exe"),
            Path::new("C:\\App\\bot.exe.bak"),
            Path::new("C:\\Temp\\stage"),
            &["agent".to_string(), "node".to_string()],
        );
        assert!(script.contains("Wait-Process -Id 42"));
        assert!(script.contains("Copy-Item -LiteralPath 'C:\\App\\bot.exe'"));
        assert!(script.contains("Start-Process"));
        assert!(script.contains("bot.exe.bak"));
        assert!(script.contains("$runtimeDirs = @('agent', 'node')"));
        assert!(script.contains("Join-Path 'C:\\Temp\\stage' $name"));
    }

    #[cfg(windows)]
    #[test]
    fn downloads_verifies_and_extracts_zip() {
        let dir = std::env::temp_dir().join(format!("modebot-update-zip-{}", uuid::Uuid::new_v4()));
        let payload_dir = dir.join("payload");
        std::fs::create_dir_all(&payload_dir).expect("dir");
        std::fs::write(payload_dir.join("bot-gpui-pilot.exe"), b"fake-exe").expect("exe");
        std::fs::create_dir_all(payload_dir.join("agent").join("glm")).expect("agent dir");
        std::fs::write(payload_dir.join("agent").join("glm").join("mode.cjs"), b"fake-agent")
            .expect("agent");
        let zip = dir.join("update.zip");
        let status = std::process::Command::new("tar")
            .args(["-a", "-cf"])
            .arg(&zip)
            .arg("-C")
            .arg(&payload_dir)
            .arg("bot-gpui-pilot.exe")
            .arg("agent")
            .status()
            .expect("tar");
        assert!(status.success(), "tar 创建 zip 失败");
        let sha256 = sha256_file(&zip).expect("hash");
        let size = std::fs::metadata(&zip).expect("meta").len();
        let url = format!(
            "file:///{}",
            zip.display().to_string().replace('\\', "/")
        );
        let target = Target {
            version: "0.0.99".to_string(),
            url,
            sha256,
            size: Some(size),
        };
        let staged = download_and_stage(&target).expect("stage");
        assert!(staged.new_exe.is_file());
        assert_eq!(
            std::fs::read(&staged.new_exe).expect("read"),
            b"fake-exe"
        );
        assert_eq!(staged.runtime_dirs, vec!["agent".to_string()]);
        assert!(staged.work_dir.join("agent").join("glm").join("mode.cjs").is_file());
        let _ = std::fs::remove_dir_all(&staged.work_dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    /// 回归：发布包里的 exe 叫 `ModeChat.exe`（产品名），不是 Cargo 的 bin 名。
    /// 早期只认 `bot-gpui-pilot.exe`，导致线上更新一直报「更新包缺少 …」。
    #[cfg(windows)]
    #[test]
    fn accepts_product_exe_name_in_package() {
        let dir = std::env::temp_dir().join(format!("modebot-update-name-{}", uuid::Uuid::new_v4()));
        let payload = dir.join("payload");
        std::fs::create_dir_all(&payload).expect("dir");
        std::fs::write(payload.join("ModeChat.exe"), b"fake-exe").expect("exe");
        let zip = dir.join("update.zip");
        let status = std::process::Command::new("tar")
            .args(["-a", "-cf"])
            .arg(&zip)
            .arg("-C")
            .arg(&payload)
            .arg("ModeChat.exe")
            .status()
            .expect("tar");
        assert!(status.success(), "tar 创建 zip 失败");
        let sha256 = sha256_file(&zip).expect("hash");
        let size = std::fs::metadata(&zip).expect("meta").len();
        let target = Target {
            version: "0.0.98".to_string(),
            url: format!("file:///{}", zip.display().to_string().replace('\\', "/")),
            sha256,
            size: Some(size),
        };
        let staged = download_and_stage(&target).expect("ModeChat.exe 应被接受");
        assert!(staged.new_exe.is_file());
        assert_eq!(
            staged.new_exe.file_name().unwrap().to_string_lossy(),
            "ModeChat.exe"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_mismatched_sha256() {
        let dir = std::env::temp_dir().join(format!("modebot-update-bad-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("bot-gpui-pilot.exe"), b"fake-exe").expect("exe");
        let zip = dir.join("update.zip");
        let status = std::process::Command::new("tar")
            .args(["-a", "-cf"])
            .arg(&zip)
            .arg("-C")
            .arg(&dir)
            .arg("bot-gpui-pilot.exe")
            .status()
            .expect("tar");
        assert!(status.success(), "tar 创建 zip 失败");
        let target = Target {
            version: "0.0.99".to_string(),
            url: format!("file:///{}", zip.display().to_string().replace('\\', "/")),
            sha256: "0".repeat(64),
            size: None,
        };
        let error = download_and_stage(&target).expect_err("sha 不符应失败");
        assert!(error.contains("SHA-256"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
