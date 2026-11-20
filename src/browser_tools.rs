//! 纯 Rust 浏览器控制（Edge/Chrome DevTools Protocol）。
//!
//! 首次调用时拉起本机 Edge/Chrome（独立 profile + 随机调试端口），通过 CDP
//! 提供导航、快照、截图、点击、输入与任意 JS 求值；不依赖 Node/Playwright。

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use rpi_agent::{AgentTool, AgentToolResult, TextContentOrImage};
use rpi_ai::types::{ImageContent, Tool};
use rpi_ai::Schema;
use rpi_harness::types::HarnessTool;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

static SESSION: OnceLock<tokio::sync::Mutex<Option<Arc<Browser>>>> = OnceLock::new();

async fn session_slot() -> &'static tokio::sync::Mutex<Option<Arc<Browser>>> {
    SESSION.get_or_init(|| tokio::sync::Mutex::new(None))
}

struct Browser {
    child: Mutex<tokio::process::Child>,
    write: tokio::sync::Mutex<futures::stream::SplitSink<Ws, Message>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    next_id: AtomicU64,
}

impl Drop for Browser {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.start_kill();
        }
    }
}

fn data_root() -> PathBuf {
    crate::data::data_root()
}

fn find_browser() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        candidates.push(PathBuf::from(&program_files).join("Microsoft/Edge/Application/msedge.exe"));
        candidates.push(PathBuf::from(&program_files).join("Google/Chrome/Application/chrome.exe"));
    }
    if let Ok(program_files) = std::env::var("ProgramFiles(x86)") {
        candidates.push(PathBuf::from(&program_files).join("Microsoft/Edge/Application/msedge.exe"));
        candidates.push(PathBuf::from(&program_files).join("Google/Chrome/Application/chrome.exe"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(&local).join("Google/Chrome/Application/chrome.exe"));
    }
    candidates.into_iter().find(|path| path.is_file())
}

async fn wait_devtools_port(profile: &std::path::Path) -> Result<u16, String> {
    let port_file = profile.join("DevToolsActivePort");
    for _ in 0..75 {
        if let Ok(text) = std::fs::read_to_string(&port_file)
            && let Some(line) = text.lines().next()
            && let Ok(port) = line.trim().parse::<u16>()
            && port > 0
        {
            return Ok(port);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err("浏览器调试端口未就绪（Edge/Chrome 启动失败？）".to_string())
}

async fn page_ws_url(port: u16) -> Result<String, String> {
    crate::tls::ensure();
    let url = format!("http://127.0.0.1:{port}/json/list");
    for _ in 0..75 {
        if let Ok(response) = reqwest::get(&url).await
            && let Ok(bytes) = response.bytes().await
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            && let Some(items) = value.as_array()
            && let Some(ws) = items
                .iter()
                .find(|item| item.get("type").and_then(Value::as_str) == Some("page"))
                .and_then(|target| target.get("webSocketDebuggerUrl"))
                .and_then(Value::as_str)
        {
            return Ok(ws.to_string());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // 没有 page 目标时主动新建一个（Edge 某些启动参数只给扩展后台页）。
    let new_url = format!("http://127.0.0.1:{port}/json/new?url=about:blank");
    if let Ok(client) = reqwest::Client::builder().build()
        && let Ok(response) = client.put(&new_url).send().await
        && let Ok(bytes) = response.bytes().await
        && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        && let Some(ws) = value.get("webSocketDebuggerUrl").and_then(Value::as_str)
    {
        return Ok(ws.to_string());
    }
    Err("没有找到可用的浏览器页面目标".to_string())
}

impl Browser {
    async fn launch() -> Result<Arc<Self>, String> {
        let exe = find_browser().ok_or_else(|| {
            "未找到 Edge 或 Chrome（浏览器控制需要本机安装其一）".to_string()
        })?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        let profile = data_root()
            .join("runtime")
            .join("browser-profiles")
            .join(format!("{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&profile)
            .map_err(|error| format!("创建浏览器 profile 失败：{error}"))?;
        let child = tokio::process::Command::new(&exe)
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-background-networking")
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("启动浏览器失败：{error}"))?;
        let port = wait_devtools_port(&profile).await?;
        let ws_url = page_ws_url(port).await?;
        let (ws, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|error| format!("连接浏览器调试端口失败：{error}"))?;
        let (write, mut read) = ws.split();
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_for_reader = Arc::clone(&pending);
        tokio::spawn(async move {
            while let Some(Ok(message)) = read.next().await {
                let Message::Text(text) = message else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                let Some(id) = value.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                if let Some(sender) = pending_for_reader
                    .lock()
                    .expect("cdp pending poisoned")
                    .remove(&id)
                {
                    let _ = sender.send(value);
                }
            }
        });
        let browser = Arc::new(Self {
            child: Mutex::new(child),
            write: tokio::sync::Mutex::new(write),
            pending,
            next_id: AtomicU64::new(0),
        });
        let _ = browser
            .send("Page.enable", json!({}))
            .await
            .map_err(|error| format!("浏览器 Page.enable 失败：{error}"))?;
        let _ = browser.send("Runtime.enable", json!({})).await;
        eprintln!("[browser] CDP ready port={port}");
        Ok(browser)
    }

    async fn send(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("cdp pending poisoned")
            .insert(id, tx);
        let payload = json!({"id": id, "method": method, "params": params});
        {
            let mut write = self.write.lock().await;
            write
                .send(Message::Text(payload.to_string()))
                .await
                .map_err(|error| format!("浏览器发送失败：{error}"))?;
        }
        let response = tokio::time::timeout(Duration::from_secs(30), rx)
            .await
            .map_err(|_| format!("浏览器 {method} 超时"))?
            .map_err(|_| "浏览器连接已关闭".to_string())?;
        if let Some(error) = response.get("error") {
            return Err(format!(
                "浏览器 {method} 失败：{}",
                error.get("message").and_then(Value::as_str).unwrap_or("unknown")
            ));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn evaluate(&self, expression: &str) -> Result<Value, String> {
        let result = self
            .send(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                    "userGesture": true,
                }),
            )
            .await?;
        if let Some(exception) = result.get("exceptionDetails") {
            return Err(format!(
                "页面脚本异常：{}",
                exception
                    .pointer("/exception/description")
                    .and_then(Value::as_str)
                    .or_else(|| exception.get("text").and_then(Value::as_str))
                    .unwrap_or("unknown")
            ));
        }
        Ok(result
            .pointer("/result/value")
            .cloned()
            .unwrap_or(Value::Null))
    }

    async fn navigate(&self, url: &str) -> Result<String, String> {
        let result = self.send("Page.navigate", json!({"url": url})).await?;
        if let Some(error) = result.get("errorText").and_then(Value::as_str) {
            return Err(format!("打开页面失败：{error}"));
        }
        // 等首帧 + 正文可读，最长 15s。
        for _ in 0..75 {
            let ready = self.evaluate("document.readyState").await.unwrap_or(Value::Null);
            let has_text = self
                .evaluate("!!(document.body && document.body.innerText && document.body.innerText.trim().length > 0)")
                .await
                .unwrap_or(Value::Bool(false));
            if (ready.as_str() == Some("complete") || ready.as_str() == Some("interactive"))
                && has_text.as_bool() == Some(true)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let title = self.evaluate("document.title").await.unwrap_or(Value::Null);
        Ok(format!(
            "已打开 {url}（{}）",
            title.as_str().unwrap_or("无标题")
        ))
    }
}

async fn get_session() -> Result<Arc<Browser>, String> {
    let slot = session_slot().await;
    let mut guard = slot.lock().await;
    if let Some(browser) = guard.as_ref() {
        return Ok(Arc::clone(browser));
    }
    let browser = Browser::launch().await?;
    *guard = Some(Arc::clone(&browser));
    Ok(browser)
}

/// 设置页显示：本机浏览器路径。
pub fn browser_path() -> Option<String> {
    find_browser().map(|path| path.display().to_string())
}

/// 设置页「打开浏览器」：启动并返回一句状态。
pub async fn launch_for_ui() -> Result<String, String> {
    get_session().await?;
    Ok("浏览器已启动（独立 profile）".to_string())
}

/// 设置页「关闭浏览器」。
pub async fn close_for_ui() -> Result<(), String> {
    close_session().await
}

/// 设置页「页面截图」：返回 PNG 字节。
pub async fn screenshot_png() -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    let browser = get_session().await?;
    let result = browser
        .send("Page.captureScreenshot", json!({"format": "png"}))
        .await?;
    let data = result
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| "截图没有返回数据".to_string())?;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|error| format!("截图解码失败：{error}"))
}

async fn close_session() -> Result<(), String> {
    let slot = session_slot().await;
    let mut guard = slot.lock().await;
    if let Some(browser) = guard.take() {
        drop(browser);
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Action {
    Open,
    Snapshot,
    Screenshot,
    Click,
    Type,
    Eval,
    Close,
}

struct BrowserTool {
    tool: Tool,
    action: Action,
}

fn define(name: &str, description: &str, parameters: Value, action: Action) -> HarnessTool {
    HarnessTool::new(Arc::new(BrowserTool {
        tool: Tool {
            name: name.to_string(),
            description: description.to_string(),
            parameters: Schema(parameters),
            constrained_sampling: None,
        },
        action,
    }))
}

/// 浏览器工具集（纯 Rust CDP）。
pub fn tools() -> Vec<HarnessTool> {
    vec![
        define(
            "browser_open",
            "用本机 Edge/Chrome 打开一个 URL 并等待页面加载完成。",
            json!({
                "type": "object",
                "properties": {"url": {"type": "string"}},
                "required": ["url"]
            }),
            Action::Open,
        ),
        define(
            "browser_snapshot",
            "读取当前页面的可见文本（body.innerText，截断到 12000 字符）。",
            json!({"type": "object", "properties": {}}),
            Action::Snapshot,
        ),
        define(
            "browser_screenshot",
            "截取当前页面（PNG，返回图片给模型）。",
            json!({"type": "object", "properties": {}}),
            Action::Screenshot,
        ),
        define(
            "browser_click",
            "点击 CSS 选择器匹配的第一个元素（或 text=可见文本）。",
            json!({
                "type": "object",
                "properties": {"selector": {"type": "string"}},
                "required": ["selector"]
            }),
            Action::Click,
        ),
        define(
            "browser_type",
            "向 CSS 选择器匹配的输入框/可编辑元素输入文本（触发 input/change）。",
            json!({
                "type": "object",
                "properties": {
                    "selector": {"type": "string"},
                    "text": {"type": "string"}
                },
                "required": ["selector", "text"]
            }),
            Action::Type,
        ),
        define(
            "browser_eval",
            "在当前页面执行一段 JavaScript 并返回结果（JSON 值）。",
            json!({
                "type": "object",
                "properties": {"expression": {"type": "string"}},
                "required": ["expression"]
            }),
            Action::Eval,
        ),
        define(
            "browser_close",
            "关闭由浏览器控制拉起的 Edge/Chrome 实例。",
            json!({"type": "object", "properties": {}}),
            Action::Close,
        ),
    ]
}

fn text_result(text: String) -> AgentToolResult {
    AgentToolResult {
        content: vec![TextContentOrImage::Text(rpi_ai::TextContent {
            kind: rpi_ai::types::TextContentType,
            text,
            text_signature: None,
        })],
        details: Value::Null,
        usage: None,
        added_tool_names: Vec::new(),
        terminate: false,
    }
}

#[async_trait]
impl AgentTool for BrowserTool {
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
        if matches!(self.action, Action::Screenshot) {
            return self.screenshot().await.map_err(rpi_agent::AgentError::Tool);
        }
        let text = self
            .run_text(params)
            .await
            .map_err(rpi_agent::AgentError::Tool)?;
        Ok(text_result(text))
    }
}

impl BrowserTool {
    async fn screenshot(&self) -> Result<AgentToolResult, String> {
        let browser = get_session().await?;
        let result = browser
            .send("Page.captureScreenshot", json!({"format": "png"}))
            .await?;
        let data = result
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| "截图没有返回数据".to_string())?;
        let image: ImageContent = serde_json::from_value(json!({
            "type": "image",
            "data": data,
            "mimeType": "image/png"
        }))
        .map_err(|error| format!("截图编码失败：{error}"))?;
        Ok(AgentToolResult {
            content: vec![TextContentOrImage::Image(image)],
            details: Value::Null,
            usage: None,
            added_tool_names: Vec::new(),
            terminate: false,
        })
    }

    async fn run_text(&self, params: Value) -> Result<String, String> {
        if matches!(self.action, Action::Close) {
            close_session().await?;
            return Ok("浏览器已关闭".to_string());
        }
        let browser = get_session().await?;
        match self.action {
            Action::Open => {
                let url = params
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "缺少 url".to_string())?;
                browser.navigate(url).await
            }
            Action::Snapshot => {
                let text = browser
                    .evaluate("document.body ? document.body.innerText : ''")
                    .await?;
                let mut text = text.as_str().unwrap_or("").to_string();
                if text.chars().count() > 12_000 {
                    text = format!("{}…", text.chars().take(12_000).collect::<String>());
                }
                if text.trim().is_empty() {
                    return Ok("页面没有可见文本".to_string());
                }
                Ok(text)
            }
            Action::Click => {
                let selector = params
                    .get("selector")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "缺少 selector".to_string())?;
                let script = format!(
                    r#"(() => {{
                        const sel = {sel};
                        let el = sel.startsWith("text=")
                          ? [...document.querySelectorAll('a,button,[role=button],input,textarea')].find(n => (n.innerText||n.value||'').includes(sel.slice(5)))
                          : document.querySelector(sel);
                        if (!el) return "not found";
                        el.scrollIntoView({{block:"center"}});
                        el.click();
                        return "clicked";
                    }})()"#,
                    sel = serde_json::to_string(selector).unwrap_or_default()
                );
                let result = browser.evaluate(&script).await?;
                Ok(result.as_str().unwrap_or("clicked").to_string())
            }
            Action::Type => {
                let selector = params
                    .get("selector")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "缺少 selector".to_string())?;
                let text = params
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "缺少 text".to_string())?;
                let script = format!(
                    r#"(() => {{
                        const el = document.querySelector({sel});
                        if (!el) return "not found";
                        el.focus();
                        const text = {text};
                        if (el.isContentEditable) {{
                            el.textContent = text;
                        }} else {{
                            const proto = el instanceof HTMLTextAreaElement
                              ? HTMLTextAreaElement.prototype
                              : HTMLInputElement.prototype;
                            const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
                            if (setter) setter.call(el, text); else el.value = text;
                        }}
                        el.dispatchEvent(new Event("input", {{bubbles:true}}));
                        el.dispatchEvent(new Event("change", {{bubbles:true}}));
                        return "typed";
                    }})()"#,
                    sel = serde_json::to_string(selector).unwrap_or_default(),
                    text = serde_json::to_string(text).unwrap_or_default()
                );
                let result = browser.evaluate(&script).await?;
                Ok(result.as_str().unwrap_or("typed").to_string())
            }
            Action::Screenshot => Err("截图在 execute 中单独处理".to_string()),
            Action::Eval => {
                let expression = params
                    .get("expression")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "缺少 expression".to_string())?;
                let value = browser.evaluate(expression).await?;
                Ok(serde_json::to_string_pretty(&value).unwrap_or_default())
            }
            Action::Close => Ok("浏览器已关闭".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "拉起本机 Edge/Chrome"]
    async fn browser_navigates_example() {
        let browser = Browser::launch().await.expect("launch browser");
        let text = browser.navigate("https://example.com").await.expect("navigate");
        assert!(text.contains("example"));
        let snapshot = browser
            .evaluate("document.body.innerText")
            .await
            .expect("snapshot");
        assert!(!snapshot.as_str().unwrap_or("").trim().is_empty());
        let _ = browser.send("Page.captureScreenshot", json!({"format": "png"})).await;
        close_session().await.expect("close");
    }
}
