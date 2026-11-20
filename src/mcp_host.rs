//! 轻量 MCP 宿主（stdio）：连接器 → MCP 服务器 → pi-rust 工具。
//!
//! - 只读 `connectors.json` + `secrets.json`，与设置页同一份数据；
//! - 连接器命令（`npx …`）走系统 Node；系统没有 Node 时按需下载官方
//!   Windows zip 到 `{data_root}/runtime/node`；
//! - 每个工作区一个共享宿主，工具名 `mcp__<server>__<tool>`，审批矩阵按
//!   `mcp__` 前缀统一走人工确认。

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rpi_agent::{AgentTool, AgentToolResult, TextContentOrImage};
use rpi_ai::types::Tool;
use rpi_ai::Schema;
use rpi_harness::types::HarnessTool;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

const PROTOCOL_VERSION: &str = "2024-11-05";
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

struct McpServer {
    child: Mutex<Child>,
    stdin: tokio::sync::Mutex<ChildStdin>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Value>>>,
    next_id: AtomicU64,
}

struct McpToolDef {
    server: String,
    name: String,
    description: String,
    schema: Value,
}

pub struct McpHost {
    servers: HashMap<String, Arc<McpServer>>,
    tools: Vec<McpToolDef>,
}

impl Drop for McpServer {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.start_kill();
        }
    }
}

fn data_root() -> PathBuf {
    crate::data::data_root()
}

fn runtime_node_dir() -> PathBuf {
    data_root().join("runtime").join("node")
}

fn node_exe_name() -> &'static str {
    if cfg!(windows) { "node.exe" } else { "node" }
}

/// 系统 Node 或已下载的 Node 是否可用。
pub fn node_available() -> bool {
    let system = std::process::Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if system {
        return true;
    }
    runtime_node_dir()
        .join(node_exe_name())
        .is_file()
}

fn path_with_runtime_node() -> Option<String> {
    let dir = runtime_node_dir();
    if !dir.is_dir() {
        return None;
    }
    let mut paths = vec![dir.to_string_lossy().into_owned()];
    if let Ok(existing) = std::env::var("PATH") {
        paths.push(existing);
    }
    Some(paths.join(if cfg!(windows) { ";" } else { ":" }))
}

/// 系统没有 Node 时下载官方 LTS zip 到数据根（只跑 MCP，不跑 Mode Agent）。
pub async fn ensure_node() -> Result<(), String> {
    if node_available() {
        return Ok(());
    }
    let dir = runtime_node_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("创建 Node 目录失败：{error}"))?;
    crate::tls::ensure();
    let index_bytes = reqwest::get("https://nodejs.org/dist/index.json")
        .await
        .map_err(|error| format!("获取 Node 版本清单失败：{error}"))?
        .bytes()
        .await
        .map_err(|error| format!("读取 Node 版本清单失败：{error}"))?;
    let index: Value = serde_json::from_slice(&index_bytes)
        .map_err(|error| format!("解析 Node 版本清单失败：{error}"))?;
    let release = index
        .as_array()
        .and_then(|items| {
            items.iter().find(|item| {
                item.get("lts").is_some_and(|lts| lts != &Value::Bool(false))
                    && item
                        .get("files")
                        .and_then(|files| files.as_array())
                        .is_some_and(|files| {
                            files.iter().any(|file| file == "win-x64-zip")
                        })
            })
        })
        .ok_or_else(|| "Node 清单里没有 Windows x64 的 LTS 包".to_string())?;
    let version = release
        .get("version")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "Node 版本号缺失".to_string())?;
    let zip_name = format!("node-{version}-win-x64.zip");
    let url = format!("https://nodejs.org/dist/{version}/{zip_name}");
    let zip_path = dir.join(&zip_name);
    let bytes = reqwest::get(&url)
        .await
        .map_err(|error| format!("下载 Node 失败：{error}"))?
        .bytes()
        .await
        .map_err(|error| format!("读取 Node 安装包失败：{error}"))?;
    std::fs::write(&zip_path, &bytes)
        .map_err(|error| format!("写入 Node 安装包失败：{error}"))?;
    let status = std::process::Command::new("tar")
        .arg("-xf")
        .arg(&zip_path)
        .arg("-C")
        .arg(&dir)
        .status()
        .map_err(|error| format!("解压 Node 失败：{error}"))?;
    if !status.success() {
        return Err("解压 Node 安装包失败（tar 返回非零）".to_string());
    }
    let extracted = dir.join(format!("node-{version}-win-x64"));
    if extracted.is_dir() {
        for entry in std::fs::read_dir(&extracted)
            .map_err(|error| format!("读取 Node 目录失败：{error}"))?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            let target = dir.join(entry.file_name());
            if target.exists() {
                continue;
            }
            std::fs::rename(entry.path(), target)
                .map_err(|error| format!("整理 Node 目录失败：{error}"))?;
        }
        let _ = std::fs::remove_dir_all(&extracted);
    }
    let _ = std::fs::remove_file(&zip_path);
    if !node_available() {
        return Err("Node 安装后仍不可用".to_string());
    }
    Ok(())
}

async fn send_request(server: &McpServer, method: &str, params: Value) -> Result<Value, String> {
    let id = server.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let (tx, rx) = oneshot::channel();
    server
        .pending
        .lock()
        .expect("mcp pending poisoned")
        .insert(id, tx);
    let payload = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
    {
        let mut stdin = server.stdin.lock().await;
        let line = serde_json::to_string(&payload).map_err(|error| error.to_string())?;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| format!("MCP 写入失败：{error}"))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|error| format!("MCP 写入失败：{error}"))?;
        stdin.flush().await.map_err(|error| error.to_string())?;
    }
    let response = tokio::time::timeout(CALL_TIMEOUT, rx)
        .await
        .map_err(|_| format!("MCP {method} 超时"))?
        .map_err(|_| format!("MCP {method} 通道关闭"))?;
    if let Some(error) = response.get("error") {
        return Err(format!(
            "MCP {method} 失败：{}",
            error.get("message").and_then(|value| value.as_str()).unwrap_or("unknown")
        ));
    }
    Ok(response.get("result").cloned().unwrap_or(Value::Null))
}

async fn send_notification(server: &McpServer, method: &str, params: Value) {
    let payload = json!({"jsonrpc":"2.0","method":method,"params":params});
    if let Ok(line) = serde_json::to_string(&payload)
        && let Ok(mut stdin) = server.stdin.try_lock()
    {
        let _ = stdin.write_all(line.as_bytes()).await;
        let _ = stdin.write_all(b"\n").await;
        let _ = stdin.flush().await;
    }
}

fn spawn_server(
    name: &str,
    command: &str,
    args: &[String],
    env: &std::collections::BTreeMap<String, String>,
    node_path: Option<&str>,
) -> Result<Arc<McpServer>, String> {
    let mut cmd = if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(command).args(args);
        cmd
    } else {
        let mut cmd = Command::new(command);
        cmd.args(args);
        cmd
    };
    if let Some(path) = node_path {
        cmd.env("PATH", path);
    }
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|error| format!("启动 MCP 服务器 {name} 失败（{command}）：{error}"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("MCP 服务器 {name} 没有 stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("MCP 服务器 {name} 没有 stdout"))?;
    let server = Arc::new(McpServer {
        child: Mutex::new(child),
        stdin: tokio::sync::Mutex::new(stdin),
        pending: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(0),
    });
    let weak = Arc::downgrade(&server);
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(id) = value.get("id").and_then(|value| value.as_u64()) else {
                continue;
            };
            let Some(server) = weak.upgrade() else {
                break;
            };
            let sender = server
                .pending
                .lock()
                .expect("mcp pending poisoned")
                .remove(&id);
            if let Some(sender) = sender {
                let _ = sender.send(value);
            }
        }
    });
    Ok(server)
}

impl McpHost {
    pub async fn empty() -> Arc<Self> {
        Arc::new(Self {
            servers: HashMap::new(),
            tools: Vec::new(),
        })
    }

    /// 读取工作区启用连接器并连接；未启用任何连接器时返回空宿主。
    pub async fn connect(workspace: &str) -> Result<Arc<Self>, String> {
        let root = data_root();
        let state = connectors_service::read_state(&root.join("v2").join("connectors.json"));
        if state.enabled.is_empty() {
            return Ok(Self::empty().await);
        }
        ensure_node().await?;
        let secrets = crate::secrets::values(&root).unwrap_or_default();
        let specs = connectors_service::resolve_mcp_servers(&state, &secrets, Some(workspace))?;
        let node_path = path_with_runtime_node();
        let mut servers = HashMap::new();
        let mut tools = Vec::new();
        for spec in specs {
            let server = match spawn_server(
                &spec.name,
                &spec.command,
                &spec.args,
                &spec.env,
                node_path.as_deref(),
            ) {
                Ok(server) => server,
                Err(error) => {
                    eprintln!("[mcp] {error}");
                    continue;
                }
            };
            let init = send_request(
                &server,
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "mode-chat", "version": env!("CARGO_PKG_VERSION")},
                }),
            )
            .await;
            if let Err(error) = init {
                eprintln!("[mcp] {} 初始化失败：{error}", spec.name);
                continue;
            }
            send_notification(&server, "notifications/initialized", json!({})).await;
            let listed = match send_request(&server, "tools/list", json!({})).await {
                Ok(result) => result,
                Err(error) => {
                    eprintln!("[mcp] {} tools/list 失败：{error}", spec.name);
                    continue;
                }
            };
            for tool in listed
                .get("tools")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
            {
                let Some(name) = tool.get("name").and_then(|value| value.as_str()) else {
                    continue;
                };
                tools.push(McpToolDef {
                    server: spec.name.clone(),
                    name: name.to_string(),
                    description: tool
                        .get("description")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string(),
                    schema: tool
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type":"object"})),
                });
            }
            servers.insert(spec.name.clone(), server);
        }
        eprintln!(
            "[mcp] workspace={workspace} servers={} tools={}",
            servers.len(),
            tools.len()
        );
        Ok(Arc::new(Self {
            servers,
            tools,
        }))
    }

    async fn call(&self, server_name: &str, tool: &str, arguments: Value) -> Result<String, String> {
        let server = self
            .servers
            .get(server_name)
            .ok_or_else(|| format!("MCP 服务器未连接：{server_name}"))?;
        let result = send_request(
            server,
            "tools/call",
            json!({"name": tool, "arguments": arguments}),
        )
        .await?;
        let mut out = String::new();
        for item in result
            .get("content")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
        {
            match item.get("type").and_then(|value| value.as_str()) {
                Some("text") => {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(item.get("text").and_then(|value| value.as_str()).unwrap_or(""));
                }
                Some("image") => {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str("[image]");
                }
                Some("resource") => {
                    if let Some(text) = item
                        .pointer("/resource/text")
                        .and_then(|value| value.as_str())
                    {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(text);
                    }
                }
                _ => {}
            }
        }
        if out.trim().is_empty() {
            out = serde_json::to_string(&result).unwrap_or_default();
        }
        Ok(out)
    }

    /// 设置页「测试连接」：只连一个连接器，返回工具数量。
    pub async fn test_connector(id: &str, workspace: &str) -> Result<String, String> {
        let root = data_root();
        let state = connectors_service::ConnectorState {
            version: 1,
            enabled: vec![id.to_string()],
        };
        let secrets = crate::secrets::values(&root).unwrap_or_default();
        let specs = connectors_service::resolve_mcp_servers(&state, &secrets, Some(workspace))?;
        let spec = specs
            .into_iter()
            .next()
            .ok_or_else(|| format!("未知连接器：{id}"))?;
        ensure_node().await?;
        let node_path = path_with_runtime_node();
        let server = spawn_server(
            &spec.name,
            &spec.command,
            &spec.args,
            &spec.env,
            node_path.as_deref(),
        )?;
        send_request(
            &server,
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "mode-chat", "version": env!("CARGO_PKG_VERSION")},
            }),
        )
        .await?;
        send_notification(&server, "notifications/initialized", json!({})).await;
        let listed = send_request(&server, "tools/list", json!({})).await?;
        let count = listed
            .get("tools")
            .and_then(|value| value.as_array())
            .map(Vec::len)
            .unwrap_or(0);
        Ok(format!("{} 连接成功，{count} 个工具", spec.name))
    }

    /// 把 MCP 工具包装成 pi-rust 的 HarnessTool。
    pub fn harness_tools(self: &Arc<Self>) -> Vec<HarnessTool> {
        self.tools
            .iter()
            .map(|def| {
                let tool = Tool {
                    name: format!("mcp__{}__{}", def.server, def.name),
                    description: def.description.clone(),
                    parameters: Schema(def.schema.clone()),
                    constrained_sampling: None,
                };
                HarnessTool::new(Arc::new(McpTool {
                    host: Arc::clone(self),
                    def: McpToolDef {
                        server: def.server.clone(),
                        name: def.name.clone(),
                        description: def.description.clone(),
                        schema: def.schema.clone(),
                    },
                    tool,
                }))
            })
            .collect()
    }
}

struct McpTool {
    host: Arc<McpHost>,
    def: McpToolDef,
    tool: Tool,
}

#[async_trait]
impl AgentTool for McpTool {
    fn schema(&self) -> &Tool {
        &self.tool
    }

    fn label(&self) -> &str {
        &self.tool.name
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: CancellationToken,
        _on_update: Arc<dyn Fn(rpi_agent::ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, rpi_agent::AgentError> {
        let text = self
            .host
            .call(&self.def.server, &self.def.name, params)
            .await
            .map_err(|error| rpi_agent::AgentError::Tool(error.to_string()))?;
        Ok(AgentToolResult {
            content: vec![TextContentOrImage::Text(rpi_ai::TextContent {
                kind: rpi_ai::types::TextContentType,
                text,
                text_signature: None,
            })],
            details: Value::Null,
            usage: None,
            added_tool_names: Vec::new(),
            terminate: false,
        })
    }
}

/// 工作区 → 共享 MCP 宿主（连接器/密钥变更后调用 `refresh_all` 重建）。
static HOSTS: std::sync::OnceLock<Mutex<HashMap<String, Arc<McpHost>>>> =
    std::sync::OnceLock::new();

fn hosts() -> &'static Mutex<HashMap<String, Arc<McpHost>>> {
    HOSTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub async fn for_workspace(workspace: &str) -> Arc<McpHost> {
    if let Some(host) = hosts()
        .lock()
        .expect("mcp hosts poisoned")
        .get(workspace)
        .cloned()
    {
        return host;
    }
    let host = match McpHost::connect(workspace).await {
        Ok(host) => host,
        Err(error) => {
            eprintln!("[mcp] 连接器初始化失败：{error}");
            McpHost::empty().await
        }
    };
    hosts()
        .lock()
        .expect("mcp hosts poisoned")
        .insert(workspace.to_string(), Arc::clone(&host));
    host
}

/// 连接器/密钥写盘后调用：丢弃缓存，下一次建会话重新连接。
pub fn refresh_all() {
    hosts().lock().expect("mcp hosts poisoned").clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER_JS: &str = r#"
import readline from "node:readline";
const rl = readline.createInterface({ input: process.stdin });
const send = (id, result) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id, result }) + "\n");
rl.on("line", (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.method === "initialize") {
    send(msg.id, { protocolVersion: "2024-11-05", capabilities: { tools: {} }, serverInfo: { name: "test", version: "0" } });
  } else if (msg.method === "tools/list") {
    send(msg.id, { tools: [{ name: "echo", description: "echo", inputSchema: { type: "object" } }] });
  } else if (msg.method === "tools/call") {
    send(msg.id, { content: [{ type: "text", text: "pong" }] });
  } else if (msg.id !== undefined) {
    send(msg.id, {});
  }
});
"#;

    #[tokio::test]
    async fn mcp_roundtrip_with_system_node() {
        if !node_available() {
            eprintln!("skip: node not installed");
            return;
        }
        let dir = std::env::temp_dir().join(format!("mode-chat-mcp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("server.mjs");
        std::fs::write(&script, SERVER_JS).unwrap();
        let server = spawn_server(
            "test",
            "node",
            &[script.to_string_lossy().into_owned()],
            &std::collections::BTreeMap::new(),
            None,
        )
        .expect("spawn");
        send_request(
            &server,
            "initialize",
            json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
        )
        .await
        .expect("initialize");
        let listed = send_request(&server, "tools/list", json!({}))
            .await
            .expect("tools/list");
        assert_eq!(listed["tools"][0]["name"], "echo");
        let called = send_request(
            &server,
            "tools/call",
            json!({"name": "echo", "arguments": {}}),
        )
        .await
        .expect("tools/call");
        assert_eq!(called["content"][0]["text"], "pong");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
