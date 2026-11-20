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

    /// 等首帧 + 正文可读，最长 15s。
    async fn wait_ready(&self) {
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
    }

    async fn navigate(&self, url: &str) -> Result<String, String> {
        let result = self.send("Page.navigate", json!({"url": url})).await?;
        if let Some(error) = result.get("errorText").and_then(Value::as_str) {
            return Err(format!("打开页面失败：{error}"));
        }
        self.wait_ready().await;
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
    Back,
    Forward,
    Reload,
    Wait,
    Hover,
    Select,
    Press,
    Scroll,
    Links,
    Html,
    Upload,
    Info,
    Elements,
}

struct BrowserTool {
    tool: Tool,
    action: Action,
    /// 会话身份；执行时按设置页「使用范围」再查一次。
    scope: String,
}

fn define(
    scope: &str,
    name: &str,
    description: &str,
    parameters: Value,
    action: Action,
) -> HarnessTool {
    HarnessTool::new(Arc::new(BrowserTool {
        tool: Tool {
            name: name.to_string(),
            description: description.to_string(),
            parameters: Schema(parameters),
            constrained_sampling: None,
        },
        action,
        scope: scope.to_string(),
    }))
}

/// 浏览器工具集（纯 Rust CDP）；`identity` 用于按 Bot 判断使用范围。
pub fn tools(identity: &str) -> Vec<HarnessTool> {
    let scope = identity;
    vec![
        define(
            scope,
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
            scope,
            "browser_snapshot",
            "读取当前页面的可见文本（可传 selector，截断到 12000 字符）。",
            json!({
                "type": "object",
                "properties": {"selector": {"type": "string"}}
            }),
            Action::Snapshot,
        ),
        define(
            scope,
            "browser_screenshot",
            "截取当前页面（PNG，返回图片给模型；full_page=true 截整页）。",
            json!({
                "type": "object",
                "properties": {"full_page": {"type": "boolean"}}
            }),
            Action::Screenshot,
        ),
        define(
            scope,
            "browser_click",
            "点击元素（真实鼠标事件）：selector 支持 CSS 或 text=可见文本；button=left/right/middle，count=1-2。",
            json!({
                "type": "object",
                "properties": {
                    "selector": {"type": "string"},
                    "button": {"type": "string"},
                    "count": {"type": "integer"}
                },
                "required": ["selector"]
            }),
            Action::Click,
        ),
        define(
            scope,
            "browser_type",
            "向输入框/可编辑元素写入文本（默认先全选替换，append=true 追加）。",
            json!({
                "type": "object",
                "properties": {
                    "selector": {"type": "string"},
                    "text": {"type": "string"},
                    "append": {"type": "boolean"}
                },
                "required": ["selector", "text"]
            }),
            Action::Type,
        ),
        define(
            scope,
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
            scope,
            "browser_back",
            "浏览器后退一页并等待加载。",
            json!({"type": "object", "properties": {}}),
            Action::Back,
        ),
        define(
            scope,
            "browser_forward",
            "浏览器前进一页并等待加载。",
            json!({"type": "object", "properties": {}}),
            Action::Forward,
        ),
        define(
            scope,
            "browser_reload",
            "刷新当前页面并等待加载。",
            json!({"type": "object", "properties": {}}),
            Action::Reload,
        ),
        define(
            scope,
            "browser_wait",
            "等待 CSS 选择器出现或页面文本包含指定内容（默认 5 秒）。",
            json!({
                "type": "object",
                "properties": {
                    "selector": {"type": "string"},
                    "text": {"type": "string"},
                    "timeout_ms": {"type": "integer"}
                }
            }),
            Action::Wait,
        ),
        define(
            scope,
            "browser_hover",
            "把鼠标悬停到 CSS 选择器/text= 元素上（触发鼠标事件）。",
            json!({
                "type": "object",
                "properties": {"selector": {"type": "string"}},
                "required": ["selector"]
            }),
            Action::Hover,
        ),
        define(
            scope,
            "browser_select",
            "选择 <select> 选项：按 value 或可见文本匹配。",
            json!({
                "type": "object",
                "properties": {
                    "selector": {"type": "string"},
                    "value": {"type": "string"}
                },
                "required": ["selector", "value"]
            }),
            Action::Select,
        ),
        define(
            scope,
            "browser_press",
            "向当前焦点发送按键，例如 enter、tab、esc、ctrl+a、arrowdown。",
            json!({
                "type": "object",
                "properties": {"keys": {"type": "string"}},
                "required": ["keys"]
            }),
            Action::Press,
        ),
        define(
            scope,
            "browser_scroll",
            "在页面内滚动：delta_y 像素（正数向下）或 pages 页数。",
            json!({
                "type": "object",
                "properties": {
                    "delta_y": {"type": "integer"},
                    "pages": {"type": "number"}
                }
            }),
            Action::Scroll,
        ),
        define(
            scope,
            "browser_links",
            "列出当前页面可见链接（文本 + href，最多 100 条）。",
            json!({"type": "object", "properties": {}}),
            Action::Links,
        ),
        define(
            scope,
            "browser_html",
            "读取页面或指定元素的 HTML（截断到 24000 字符）。",
            json!({
                "type": "object",
                "properties": {"selector": {"type": "string"}}
            }),
            Action::Html,
        ),
        define(
            scope,
            "browser_upload",
            "给 file input 选择本机文件（CSS 选择器 + files 路径数组）。",
            json!({
                "type": "object",
                "properties": {
                    "selector": {"type": "string"},
                    "files": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["selector", "files"]
            }),
            Action::Upload,
        ),
        define(
            scope,
            "browser_info",
            "读取当前页面 URL、标题与加载状态。",
            json!({"type": "object", "properties": {}}),
            Action::Info,
        ),
        define(
            scope,
            "browser_elements",
            "列出页面可交互元素（链接/按钮/输入框/下拉框，含文本与占位符，最多 80 条）。",
            json!({"type": "object", "properties": {}}),
            Action::Elements,
        ),
        define(
            scope,
            "browser_close",
            "关闭由浏览器控制拉起的 Edge/Chrome 实例。",
            json!({"type": "object", "properties": {}}),
            Action::Close,
        ),
    ]
}

/// 找到元素并滚动到视口中央，返回页面坐标（CSS 像素）。
async fn element_point(browser: &Browser, selector: &str) -> Result<Option<(f64, f64)>, String> {
    let script = format!(
        r#"(() => {{
            const sel = {sel};
            let el = sel.startsWith("text=")
              ? [...document.querySelectorAll('a,button,[role=button],input,textarea')].find(n => (n.innerText||n.value||'').includes(sel.slice(5)))
              : document.querySelector(sel);
            if (!el) return null;
            el.scrollIntoView({{block:"center", inline:"center"}});
            const rect = el.getBoundingClientRect();
            if (rect.width <= 0 || rect.height <= 0) return null;
            return [rect.left + rect.width / 2, rect.top + rect.height / 2];
        }})()"#,
        sel = serde_json::to_string(selector).unwrap_or_default()
    );
    let point = browser.evaluate(&script).await?;
    let Some(items) = point.as_array() else {
        return Ok(None);
    };
    let x = items.first().and_then(Value::as_f64);
    let y = items.get(1).and_then(Value::as_f64);
    Ok(match (x, y) {
        (Some(x), Some(y)) => Some((x, y)),
        _ => None,
    })
}

/// 解析 ctrl+shift+a 之类的组合键：返回 CDP modifiers 与主键名。
fn parse_key_chord(keys: &str) -> Result<(u32, String), String> {
    let parts: Vec<&str> = keys
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    let Some(key) = parts.last() else {
        return Err("keys 为空".to_string());
    };
    let mut modifiers = 0u32;
    for modifier in &parts[..parts.len().saturating_sub(1)] {
        match modifier.to_ascii_lowercase().as_str() {
            "alt" => modifiers |= 1,
            "ctrl" | "control" => modifiers |= 2,
            "meta" | "win" => modifiers |= 4,
            "shift" => modifiers |= 8,
            other => return Err(format!("不支持修饰键：{other}")),
        }
    }
    Ok((modifiers, (*key).to_string()))
}

fn key_code(key: &str) -> Result<(&'static str, u32), String> {
    let code = match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => ("Enter", 13),
        "tab" => ("Tab", 9),
        "esc" | "escape" => ("Escape", 27),
        "space" => ("Space", 32),
        "backspace" => ("Backspace", 8),
        "delete" | "del" => ("Delete", 46),
        "arrowup" | "up" => ("ArrowUp", 38),
        "arrowdown" | "down" => ("ArrowDown", 40),
        "arrowleft" | "left" => ("ArrowLeft", 37),
        "arrowright" | "right" => ("ArrowRight", 39),
        "home" => ("Home", 36),
        "end" => ("End", 35),
        "pageup" => ("PageUp", 33),
        "pagedown" => ("PageDown", 34),
        other => {
            let mut chars = other.chars();
            let ch = chars.next().ok_or_else(|| "按键为空".to_string())?;
            if chars.next().is_some() {
                return Err(format!("不支持的按键：{key}"));
            }
            let upper = ch.to_ascii_uppercase();
            return Ok((
                Box::leak(upper.to_string().into_boxed_str()),
                upper as u32,
            ));
        }
    };
    Ok(code)
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
        // 关闭浏览器控制后，已打开的会话也立即停用（close 仍放行）。
        if !matches!(self.action, Action::Close)
            && !crate::tool_scope::allowed(crate::tool_scope::Family::Browser, &self.scope)
        {
            return Err(rpi_agent::AgentError::Tool(crate::tool_scope::denial(
                crate::tool_scope::Family::Browser,
            )));
        }
        if matches!(self.action, Action::Screenshot) {
            let full_page = params
                .get("full_page")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            return self
                .screenshot(full_page)
                .await
                .map_err(rpi_agent::AgentError::Tool);
        }
        let text = self
            .run_text(params)
            .await
            .map_err(rpi_agent::AgentError::Tool)?;
        Ok(text_result(text))
    }
}

impl BrowserTool {
    async fn screenshot(&self, full_page: bool) -> Result<AgentToolResult, String> {
        let browser = get_session().await?;
        let result = browser
            .send(
                "Page.captureScreenshot",
                json!({"format": "png", "captureBeyondViewport": full_page}),
            )
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
                // 只允许 http/https，避免 file:// / javascript: 读到本机内容。
                let url = crate::shell_open::validate_url(url)?;
                browser.navigate(url).await
            }
            Action::Snapshot => {
                let selector = params.get("selector").and_then(Value::as_str);
                let script = match selector {
                    Some(selector) => format!(
                        "(() => {{ const el = document.querySelector({}); return el ? el.innerText : ''; }})()",
                        serde_json::to_string(selector).unwrap_or_default()
                    ),
                    None => "document.body ? document.body.innerText : ''".to_string(),
                };
                let text = browser.evaluate(&script).await?;
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
                let button = params
                    .get("button")
                    .and_then(Value::as_str)
                    .unwrap_or("left")
                    .to_ascii_lowercase();
                if !matches!(button.as_str(), "left" | "right" | "middle") {
                    return Err("button 只支持 left/right/middle".to_string());
                }
                let count = params
                    .get("count")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .clamp(1, 2) as u32;
                let Some((x, y)) = element_point(&browser, selector).await? else {
                    return Ok("元素不存在".to_string());
                };
                browser
                    .send(
                        "Input.dispatchMouseEvent",
                        json!({"type": "mouseMoved", "x": x, "y": y}),
                    )
                    .await?;
                for click_count in 1..=count {
                    browser
                        .send(
                            "Input.dispatchMouseEvent",
                            json!({
                                "type": "mousePressed",
                                "x": x,
                                "y": y,
                                "button": button,
                                "clickCount": click_count,
                            }),
                        )
                        .await?;
                    browser
                        .send(
                            "Input.dispatchMouseEvent",
                            json!({
                                "type": "mouseReleased",
                                "x": x,
                                "y": y,
                                "button": button,
                                "clickCount": click_count,
                            }),
                        )
                        .await?;
                }
                Ok(format!("已点击 {selector}（{button}，{count} 次）"))
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
                let append = params
                    .get("append")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                // 先聚焦并（默认）全选，再用 CDP 插入文本：
                // React/Vue 受控输入框能收到真实输入事件。
                let script = format!(
                    r#"(() => {{
                        const el = document.querySelector({sel});
                        if (!el) return "not found";
                        el.scrollIntoView({{block:"center"}});
                        el.focus();
                        if (!{append}) {{
                            if (el.select) el.select();
                            else if (el.isContentEditable) {{
                                const range = document.createRange();
                                range.selectNodeContents(el);
                                const sel = getSelection();
                                sel.removeAllRanges();
                                sel.addRange(range);
                            }}
                        }}
                        return "focused";
                    }})()"#,
                    sel = serde_json::to_string(selector).unwrap_or_default(),
                    append = append
                );
                let result = browser.evaluate(&script).await?;
                if result.as_str() == Some("not found") {
                    return Ok("元素不存在".to_string());
                }
                browser
                    .send("Input.insertText", json!({"text": text}))
                    .await?;
                Ok(format!("已输入 {selector}"))
            }
            Action::Back => {
                browser.evaluate("history.back();").await?;
                browser.wait_ready().await;
                Ok("已后退".to_string())
            }
            Action::Forward => {
                browser.evaluate("history.forward();").await?;
                browser.wait_ready().await;
                Ok("已前进".to_string())
            }
            Action::Reload => {
                browser.send("Page.reload", json!({"ignoreCache": true})).await?;
                browser.wait_ready().await;
                Ok("已刷新".to_string())
            }
            Action::Wait => {
                let timeout = params
                    .get("timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(5_000)
                    .clamp(200, 60_000);
                let selector = params.get("selector").and_then(Value::as_str);
                let text = params.get("text").and_then(Value::as_str);
                if selector.is_none() && text.is_none() {
                    return Err("wait 需要 selector 或 text".to_string());
                }
                let script = match (selector, text) {
                    (Some(selector), _) => format!(
                        "!!document.querySelector({})",
                        serde_json::to_string(selector).unwrap_or_default()
                    ),
                    (None, Some(text)) => format!(
                        "(document.body ? document.body.innerText : '').includes({})",
                        serde_json::to_string(text).unwrap_or_default()
                    ),
                    _ => unreachable!(),
                };
                let started = std::time::Instant::now();
                loop {
                    let hit = browser
                        .evaluate(&script)
                        .await
                        .unwrap_or(Value::Bool(false))
                        .as_bool()
                        .unwrap_or(false);
                    if hit {
                        return Ok("已满足等待条件".to_string());
                    }
                    if started.elapsed() >= std::time::Duration::from_millis(timeout) {
                        return Ok(format!("等待超时（{timeout}ms）"));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
            Action::Hover => {
                let selector = params
                    .get("selector")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "缺少 selector".to_string())?;
                let script = format!(
                    r#"(() => {{
                        const sel = {sel};
                        const el = sel.startsWith("text=")
                          ? [...document.querySelectorAll('a,button,[role=button],input,textarea')].find(n => (n.innerText||n.value||'').includes(sel.slice(5)))
                          : document.querySelector(sel);
                        if (!el) return "not found";
                        el.scrollIntoView({{block:"center"}});
                        const rect = el.getBoundingClientRect();
                        const opts = {{bubbles:true, clientX:rect.left+rect.width/2, clientY:rect.top+rect.height/2}};
                        el.dispatchEvent(new MouseEvent("mouseover", opts));
                        el.dispatchEvent(new MouseEvent("mouseenter", opts));
                        el.dispatchEvent(new MouseEvent("mousemove", opts));
                        return "hovered";
                    }})()"#,
                    sel = serde_json::to_string(selector).unwrap_or_default()
                );
                let result = browser.evaluate(&script).await?;
                Ok(result.as_str().unwrap_or("hovered").to_string())
            }
            Action::Select => {
                let selector = params
                    .get("selector")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "缺少 selector".to_string())?;
                let value = params
                    .get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "缺少 value".to_string())?;
                let script = format!(
                    r#"(() => {{
                        const el = document.querySelector({sel});
                        if (!el || el.tagName !== "SELECT") return "not a select";
                        const wanted = {value};
                        const option = [...el.options].find(o => o.value === wanted)
                          || [...el.options].find(o => (o.textContent||"").trim() === wanted)
                          || [...el.options].find(o => (o.textContent||"").includes(wanted));
                        if (!option) return "option not found";
                        el.value = option.value;
                        el.dispatchEvent(new Event("input", {{bubbles:true}}));
                        el.dispatchEvent(new Event("change", {{bubbles:true}}));
                        return (option.textContent || option.value).trim();
                    }})()"#,
                    sel = serde_json::to_string(selector).unwrap_or_default(),
                    value = serde_json::to_string(value).unwrap_or_default()
                );
                let result = browser.evaluate(&script).await?;
                Ok(format!("已选择：{}", result.as_str().unwrap_or("")))
            }
            Action::Press => {
                let keys = params
                    .get("keys")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "缺少 keys".to_string())?;
                let (modifiers, key) = parse_key_chord(keys)?;
                let (code, vk) = key_code(&key)?;
                browser
                    .send(
                        "Input.dispatchKeyEvent",
                        json!({
                            "type": "keyDown",
                            "modifiers": modifiers,
                            "key": key,
                            "code": code,
                            "windowsVirtualKeyCode": vk,
                            "nativeVirtualKeyCode": vk,
                        }),
                    )
                    .await?;
                browser
                    .send(
                        "Input.dispatchKeyEvent",
                        json!({
                            "type": "keyUp",
                            "modifiers": modifiers,
                            "key": key,
                            "code": code,
                            "windowsVirtualKeyCode": vk,
                            "nativeVirtualKeyCode": vk,
                        }),
                    )
                    .await?;
                Ok(format!("已按键：{keys}"))
            }
            Action::Scroll => {
                let pages = params.get("pages").and_then(Value::as_f64);
                let delta_y = params
                    .get("delta_y")
                    .and_then(Value::as_i64)
                    .or_else(|| pages.map(|pages| (pages * 700.0) as i64))
                    .unwrap_or(600);
                let result = browser
                    .evaluate(&format!("window.scrollBy(0, {delta_y}); window.scrollY"))
                    .await?;
                Ok(format!(
                    "已滚动 {delta_y}px（当前 scrollY={}）",
                    result.as_i64().unwrap_or(0)
                ))
            }
            Action::Links => {
                let script = r#"(() => {
                    const seen = new Set();
                    const out = [];
                    for (const a of document.querySelectorAll('a[href]')) {
                        const text = (a.innerText || '').trim().replace(/\s+/g, ' ');
                        const href = a.href;
                        if (!text || !href || seen.has(href)) continue;
                        seen.add(href);
                        out.push(text + " -> " + href);
                        if (out.length >= 100) break;
                    }
                    return out.join("
");
                })()"#;
                let result = browser.evaluate(script).await?;
                let text = result.as_str().unwrap_or("").trim();
                if text.is_empty() {
                    Ok("页面没有链接".to_string())
                } else {
                    Ok(text.to_string())
                }
            }
            Action::Html => {
                let selector = params.get("selector").and_then(Value::as_str);
                let script = match selector {
                    Some(selector) => format!(
                        "(() => {{ const el = document.querySelector({}); return el ? el.outerHTML : ''; }})()",
                        serde_json::to_string(selector).unwrap_or_default()
                    ),
                    None => "document.documentElement ? document.documentElement.outerHTML : ''".to_string(),
                };
                let result = browser.evaluate(&script).await?;
                let mut html = result.as_str().unwrap_or("").to_string();
                if html.is_empty() {
                    return Ok("没有匹配的 HTML".to_string());
                }
                if html.chars().count() > 24_000 {
                    html = format!("{}…", html.chars().take(24_000).collect::<String>());
                }
                Ok(html)
            }
            Action::Upload => {
                let selector = params
                    .get("selector")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "缺少 selector".to_string())?;
                let files = params
                    .get("files")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "缺少 files".to_string())?
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                if files.is_empty() {
                    return Err("files 为空".to_string());
                }
                for file in &files {
                    if !std::path::Path::new(file).is_file() {
                        return Err(format!("文件不存在：{file}"));
                    }
                }
                browser.send("DOM.enable", json!({})).await?;
                let doc = browser.send("DOM.getDocument", json!({"depth": 0})).await?;
                let root = doc
                    .get("root")
                    .and_then(|root| root.get("nodeId"))
                    .and_then(Value::as_u64)
                    .ok_or_else(|| "读取 DOM 根节点失败".to_string())?;
                let node = browser
                    .send(
                        "DOM.querySelector",
                        json!({"nodeId": root, "selector": selector}),
                    )
                    .await?;
                let node_id = node
                    .get("nodeId")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| "没有找到 file input".to_string())?;
                browser
                    .send(
                        "DOM.setFileInputFiles",
                        json!({"nodeId": node_id, "files": files}),
                    )
                    .await?;
                Ok(format!("已选择 {} 个文件", files.len()))
            }
            Action::Info => {
                let url = browser.evaluate("location.href").await?;
                let title = browser.evaluate("document.title").await?;
                let ready = browser.evaluate("document.readyState").await?;
                Ok(format!(
                    "URL: {}\n标题: {}\n状态: {}",
                    url.as_str().unwrap_or(""),
                    title.as_str().unwrap_or(""),
                    ready.as_str().unwrap_or("")
                ))
            }
            Action::Elements => {
                let script = r#"(() => {
                    const out = [];
                    const clean = (value) => (value || '').trim().replace(/\s+/g, ' ');
                    const visible = (el) => {
                        const rect = el.getBoundingClientRect();
                        const style = getComputedStyle(el);
                        return rect.width > 1 && rect.height > 1 && style.visibility !== 'hidden' && style.display !== 'none';
                    };
                    for (const el of document.querySelectorAll('a, button, input, textarea, select, [role=button], [contenteditable=true]')) {
                        if (!visible(el)) continue;
                        const tag = el.tagName.toLowerCase();
                        const type = (el.getAttribute('type') || '').toLowerCase();
                        if (tag === 'input' && type === 'hidden') continue;
                        const label = clean(el.innerText || el.value || el.getAttribute('aria-label') || el.getAttribute('placeholder') || el.getAttribute('name'));
                        const id = el.id ? '#' + el.id : '';
                        out.push(tag + (type ? '[' + type + ']' : '') + id + (label ? ' | ' + label.slice(0, 120) : ''));
                        if (out.length >= 80) break;
                    }
                    return out.join('\n');
                })()"#;
                let result = browser.evaluate(script).await?;
                let text = result.as_str().unwrap_or("").trim();
                if text.is_empty() {
                    Ok("没有可交互元素".to_string())
                } else {
                    Ok(text.to_string())
                }
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
    async fn browser_navigates_and_inputs() {
        let browser = Browser::launch().await.expect("launch browser");
        let text = browser.navigate("https://example.com").await.expect("navigate");
        assert!(text.contains("example"));
        let snapshot = browser
            .evaluate("document.body.innerText")
            .await
            .expect("snapshot");
        assert!(!snapshot.as_str().unwrap_or("").trim().is_empty());
        let _ = browser.send("Page.captureScreenshot", json!({"format": "png"})).await;

        // 本地表单页：真实鼠标点击、输入、文件选择。
        let page = "data:text/html,%3Cbutton%20id%3Db%20onclick%3D%22window.clicked%3D(window.clicked%7C%7C0)%2B1%22%3EGo%3C%2Fbutton%3E%3Cinput%20id%3Di%3E%3Cinput%20id%3Df%20type%3Dfile%3E";
        browser.navigate(page).await.expect("form page");
        let (x, y) = element_point(&browser, "#b")
            .await
            .expect("point")
            .expect("button point");
        browser
            .send(
                "Input.dispatchMouseEvent",
                json!({"type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1}),
            )
            .await
            .expect("mouse down");
        browser
            .send(
                "Input.dispatchMouseEvent",
                json!({"type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1}),
            )
            .await
            .expect("mouse up");
        assert_eq!(
            browser.evaluate("window.clicked").await.unwrap().as_i64(),
            Some(1)
        );

        browser.evaluate("document.querySelector('#i').focus()").await.unwrap();
        browser
            .send("Input.insertText", json!({"text": "hello"}))
            .await
            .expect("insert text");
        assert_eq!(
            browser.evaluate("document.querySelector('#i').value").await.unwrap().as_str(),
            Some("hello")
        );

        let file = std::env::temp_dir().join("mode-chat-upload.txt");
        std::fs::write(&file, "upload").expect("temp file");
        browser.send("DOM.enable", json!({})).await.expect("dom");
        let doc = browser.send("DOM.getDocument", json!({"depth": 0})).await.unwrap();
        let root = doc.pointer("/root/nodeId").and_then(Value::as_u64).unwrap();
        let node = browser
            .send("DOM.querySelector", json!({"nodeId": root, "selector": "#f"}))
            .await
            .unwrap();
        let node_id = node.get("nodeId").and_then(Value::as_u64).unwrap();
        browser
            .send(
                "DOM.setFileInputFiles",
                json!({"nodeId": node_id, "files": [file.display().to_string()]}),
            )
            .await
            .expect("set file");
        assert_eq!(
            browser
                .evaluate("document.querySelector('#f').files[0].name")
                .await
                .unwrap()
                .as_str(),
            Some("mode-chat-upload.txt")
        );
        let _ = std::fs::remove_file(file);
        close_session().await.expect("close");
    }
}
