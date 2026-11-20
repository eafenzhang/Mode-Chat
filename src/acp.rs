//! 本地 Agent（ACP）客户端：spawn `pi-acp` / `deepseek-acp` / `zcode-acp-server` 这类
//! ACP JSON-RPC stdio 服务，把 ACP 的 session/update、权限请求翻译成试点的会话事件。
//!
//! 为什么是 ACP：用户 2026-11-14 拍板引擎本体不迁 Rust，本地 Agent 走各自 CLI/ACP；
//! Mode Agent 仍走 v4，ACP 引擎作为同一套 UI 的第二来源。
//!
//! 线程模型：一个 tokio 任务持有子进程；命令（prompt/stop/resolve）走 mpsc，
//! stdout 逐行解析 JSON-RPC（请求/响应/通知），事件经 UnboundedSender 回后端。

use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::backend::{
    ElicitationCard, ElicitationContent, ElicitationField, ElicitationFieldKind,
    ElicitationValue, PermissionPrompt, PromptOption, ToolCard,
};

/// ACP 引擎发回后端的事件。
#[derive(Debug, Clone)]
pub enum AcpEvent {
    Ready {
        session_id: String,
        /// 频道成员槽位/切换历史时的持久化键；私聊新建为 None。
        slot: Option<String>,
    },
    Delta(String),
    Tool(ToolCard),
    /// ACP 的 session/request_permission：转成现有审批卡（request_id = "acp-<jsonrpc id>"）。
    Permission(PermissionPrompt),
    /// ACP 的 elicitation/create（form）：转成表单卡（request_id = "acp-elicit-<jsonrpc id>"）。
    Elicitation(ElicitationCard),
    Done,
    Error(String),
}

/// 待作答的反向请求：JSON-RPC id 可能是数字或字符串（ACP `RequestId`）。
#[derive(Clone)]
struct PendingRequest {
    id: Value,
    kind: PendingKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingKind {
    Permission,
    Elicitation,
}

/// 后端发给 ACP 任务的命令。
pub enum AcpCommand {
    Prompt(String),
    Stop,
    /// 审批作答：request_id 是展示给 UI 的 id，option_id 是 ACP optionId。
    Resolve { request_id: String, option_id: String },
    /// 表单作答：None = decline（取消）。
    ResolveElicitation {
        request_id: String,
        content: Option<ElicitationContent>,
    },
}

#[derive(Clone)]
pub struct AcpHandle {
    tx: UnboundedSender<AcpCommand>,
}

impl AcpHandle {
    /// 发送一次提问；返回 false 表示 ACP 任务已退出（会话句柄失效）。
    pub fn prompt(&self, text: impl Into<String>) -> bool {
        self.tx
            .send(AcpCommand::Prompt(text.into()))
            .is_ok()
    }
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
    pub fn stop(&self) {
        let _ = self.tx.send(AcpCommand::Stop);
    }
    pub fn resolve(&self, request_id: impl Into<String>, option_id: impl Into<String>) {
        let _ = self.tx.send(AcpCommand::Resolve {
            request_id: request_id.into(),
            option_id: option_id.into(),
        });
    }
    pub fn resolve_elicitation(
        &self,
        request_id: impl Into<String>,
        content: Option<ElicitationContent>,
    ) {
        let _ = self.tx.send(AcpCommand::ResolveElicitation {
            request_id: request_id.into(),
            content,
        });
    }
}

/// 引擎名/路径 → 启动命令。内置名字走固定命令；绝对路径允许自定义 ACP 服务
/// （`.js/.mjs` 会用 node 启动，便于本地调试与第三方 adapter）。
pub fn engine_command(engine: &str) -> Option<String> {
    let raw = engine.trim();
    if raw.is_empty() {
        return None;
    }
    match raw.to_ascii_lowercase().as_str() {
        "pi" | "pi-acp" => return Some("pi-acp".to_string()),
        "deepseek" | "deepseek-acp" => return Some("deepseek-acp".to_string()),
        "zcode" | "zcode-acp-server" => return Some("zcode-acp-server".to_string()),
        // Hermes 自带 ACP 适配器（`hermes acp`）：权限/提问都走 ACP，不再用一次性 stream-json。
        "hermes" | "hermes-acp" => return Some("hermes".to_string()),
        _ => {}
    }
    let path = std::path::Path::new(raw);
    path.is_file().then(|| raw.to_string())
}

fn launch_args(command: &str) -> Vec<String> {
    let lower = command.to_ascii_lowercase();
    let file_stem = std::path::Path::new(command)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if lower.ends_with(".js") || lower.ends_with(".mjs") {
        vec!["node".to_string(), command.to_string()]
    } else if lower.ends_with("zcode-acp-server")
        || lower.ends_with("zcode-acp-server.cmd")
        || lower.ends_with("zcode-acp")
        || lower.ends_with("zcode-acp.cmd")
    {
        // zcode-acp 裸跑是交互 TUI；编辑器侧 ACP 走 `server` 子命令。
        vec![command.to_string(), "server".to_string()]
    } else if file_stem == "hermes" {
        // Hermes 的 ACP 适配器入口：`hermes acp`（stdout 只留 JSON-RPC）。
        vec![command.to_string(), "acp".to_string()]
    } else if file_stem == "hermes-acp" {
        // `hermes-acp` 本身就是 ACP 入口，不再追加子命令。
        vec![command.to_string()]
    } else {
        vec![command.to_string()]
    }
}

/// 启动一个 ACP 会话任务；`resume` 非空时用 session/load 续用已有会话。
pub fn spawn(
    engine_command: String,
    cwd: String,
    resume: Option<String>,
    slot: Option<String>,
    events: UnboundedSender<AcpEvent>,
) -> AcpHandle {
    let (tx, rx) = unbounded_channel();
    tokio::spawn(async move {
        if let Err(message) = run(engine_command, cwd, resume, slot, events.clone(), rx).await {
            let _ = events.send(AcpEvent::Error(message));
        }
    });
    AcpHandle { tx }
}

async fn run(
    engine_command: String,
    cwd: String,
    resume: Option<String>,
    slot: Option<String>,
    events: UnboundedSender<AcpEvent>,
    mut commands: UnboundedReceiver<AcpCommand>,
) -> Result<(), String> {
    // Windows 上这些是 .cmd/.exe：统一经 cmd /C 启动，避免 PATHEXT 解析问题。
    let mut child = tokio::process::Command::new("cmd")
        .arg("/C")
        .args(launch_args(&engine_command))
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("启动 {engine_command} 失败：{error}"))?;
    let mut stdin = BufWriter::new(child.stdin.take().ok_or("ACP 没有 stdin")?);
    let mut lines = BufReader::new(child.stdout.take().ok_or("ACP 没有 stdout")?).lines();

    let mut next_id: u64 = 1;
    let init_id = json!(next_id);
    // 待作答的反向请求：展示给 UI 的 request_id → JSON-RPC id + 类型。
    let mut pending: std::collections::HashMap<String, PendingRequest> =
        std::collections::HashMap::new();
    let mut session_id: Option<String> = None;
    // 只允许一次 initialize/session 流程；后面的 prompt 直接发。
    let init = json!({
        "jsonrpc": "2.0",
        "id": next_id,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": { "readTextFile": false, "writeTextFile": false },
                "terminal": false,
                // 声明 form elicitation：zcode-acp-server / Hermes 才走表单而非权限兜底。
                "elicitation": { "form": {} }
            }
        }
    });
    send(&mut stdin, init).await?;
    next_id += 1;
    let mut session_requested = false;
    let mut session_request_id: Option<Value> = None;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut ready = false;
    loop {
        if !ready && tokio::time::Instant::now() > deadline {
            return Err(format!("{engine_command} 初始化超时"));
        }
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                match command {
                    AcpCommand::Prompt(text) => {
                        let Some(session) = session_id.as_deref() else {
                            let _ = events.send(AcpEvent::Error("ACP 会话尚未建立".to_string()));
                            continue;
                        };
                        if std::env::var("BOT_PILOT_DEBUG").ok().as_deref() == Some("1") {
                            eprintln!(
                                "[acp] prompt session={session} chars={}",
                                text.chars().count()
                            );
                        }
                        let payload = json!({
                            "jsonrpc": "2.0",
                            "id": next_id,
                            "method": "session/prompt",
                            "params": {
                                "sessionId": session,
                                "prompt": [{ "type": "text", "text": text }]
                            }
                        });
                        next_id += 1;
                        send(&mut stdin, payload).await?;
                    }
                    AcpCommand::Stop => {
                        if let Some(session) = session_id.as_deref() {
                            let payload = json!({
                                "jsonrpc": "2.0",
                                "method": "session/cancel",
                                "params": { "sessionId": session }
                            });
                            send(&mut stdin, payload).await?;
                        }
                        // ACP 要求取消时把挂起的权限/表单请求一并作答，否则 agent 可能在
                        // 等待作答而不返回 stopReason，UI 会卡在「生成中」。
                        for request in pending.drain().map(|(_, request)| request) {
                            let result = match request.kind {
                                PendingKind::Permission => {
                                    json!({ "outcome": { "outcome": "cancelled" } })
                                }
                                PendingKind::Elicitation => {
                                    json!({ "action": "cancel" })
                                }
                            };
                            send(
                                &mut stdin,
                                json!({ "jsonrpc": "2.0", "id": request.id, "result": result }),
                            )
                            .await?;
                        }
                    }
                    AcpCommand::Resolve {
                        request_id,
                        option_id,
                    } => {
                        if let Some(request) = pending.remove(&request_id)
                            && request.kind == PendingKind::Permission
                        {
                            send(
                                &mut stdin,
                                json!({
                                    "jsonrpc": "2.0",
                                    "id": request.id,
                                    "result": {
                                        "outcome": { "outcome": "selected", "optionId": option_id }
                                    }
                                }),
                            )
                            .await?;
                        }
                    }
                    AcpCommand::ResolveElicitation {
                        request_id,
                        content,
                    } => {
                        if let Some(request) = pending.remove(&request_id)
                            && request.kind == PendingKind::Elicitation
                        {
                            let result = match content {
                                Some(content) => json!({
                                    "action": "accept",
                                    "content": content_json(&content)
                                }),
                                None => json!({ "action": "decline" }),
                            };
                            send(
                                &mut stdin,
                                json!({ "jsonrpc": "2.0", "id": request.id, "result": result }),
                            )
                            .await?;
                        }
                    }
                }
            }
            line = lines.next_line() => {
                let Ok(Some(line)) = line else { break };
                let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
                let method = value.get("method").and_then(Value::as_str);
                // ACP RequestId 允许数字或字符串；null 才是通知。
                let id = value
                    .get("id")
                    .filter(|id| !id.is_null())
                    .cloned();
                if std::env::var("BOT_PILOT_DEBUG").ok().as_deref() == Some("1") {
                    eprintln!(
                        "[acp] <- method={:?} id={:?} has_result={} has_error={}",
                        method,
                        id,
                        value.get("result").is_some(),
                        value.get("error").is_some()
                    );
                }
                match (method, id) {
                    (Some(method), Some(id)) => {
                        handle_agent_request(
                            method,
                            &id,
                            &value,
                            &mut stdin,
                            &mut pending,
                            &events,
                        )
                        .await?;
                    }
                    (None, Some(id)) => {
                        let stop = value
                            .get("result")
                            .and_then(|result| result.get("stopReason"))
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        if let Some(stop) = stop {
                            let _ = events.send(AcpEvent::Done);
                            if stop == "cancelled" {
                                continue;
                            }
                        }
                        if id == init_id && !session_requested {
                            session_requested = true;
                            let (method, params) = match resume.as_deref() {
                                Some(session) => (
                                    "session/load",
                                    json!({ "sessionId": session, "cwd": cwd, "mcpServers": [] }),
                                ),
                                None => ("session/new", json!({ "cwd": cwd, "mcpServers": [] })),
                            };
                            session_request_id = Some(json!(next_id));
                            let payload = json!({
                                "jsonrpc": "2.0",
                                "id": next_id,
                                "method": method,
                                "params": params
                            });
                            next_id += 1;
                            send(&mut stdin, payload).await?;
                        } else if session_request_id.as_ref() == Some(&id)
                            && value.get("error").is_some()
                        {
                            // session/load 失败（会话已删/引擎不支持）：直接报错退出，
                            // 否则会一直等到初始化超时，界面只显示「会话建立中」。
                            let message = value
                                .get("error")
                                .and_then(|error| error.get("message"))
                                .and_then(Value::as_str)
                                .unwrap_or("会话加载失败")
                                .to_string();
                            let _ = events.send(AcpEvent::Error(format!(
                                "建立会话失败：{message}"
                            )));
                            break;
                        } else if !ready
                            && session_request_id.as_ref() == Some(&id)
                            && let Some(session) = resume.as_deref()
                        {
                            // session/load 的响应不带 sessionId：用请求里的 id 作为会话 id。
                            session_id = Some(session.to_string());
                            ready = true;
                            let _ = events.send(AcpEvent::Ready {
                                session_id: session.to_string(),
                                slot: slot.clone(),
                            });
                        } else if !ready
                            && let Some(session) = value
                                .get("result")
                                .and_then(|result| result.get("sessionId"))
                                .and_then(Value::as_str)
                        {
                            session_id = Some(session.to_string());
                            ready = true;
                            let _ = events.send(AcpEvent::Ready {
                                session_id: session.to_string(),
                                slot: slot.clone(),
                            });
                        } else if let Some(error) = value.get("error") {
                            // 回合级错误（额度不足/模型拒绝等）：界面要立刻出错误卡，
                            // 不能等成「初始化超时」。
                            let message = error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("智能体返回错误")
                                .to_string();
                            let _ = events.send(AcpEvent::Error(message));
                        }
                    }
                    (Some(method), None) => {
                        // session/load 会在响应前重放历史消息；ready 之前一律丢弃，
                        // 否则旧回复会被当成新回合灌进当前气泡。
                        if ready {
                            handle_notification(method, &value, &events);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let _ = child.start_kill();
    Ok(())
}

async fn handle_agent_request(
    method: &str,
    id: &Value,
    value: &Value,
    stdin: &mut BufWriter<tokio::process::ChildStdin>,
    pending: &mut std::collections::HashMap<String, PendingRequest>,
    events: &UnboundedSender<AcpEvent>,
) -> Result<(), String> {
    match method {
        "session/request_permission" => {
            let tool = value
                .get("params")
                .and_then(|params| params.get("toolCall"))
                .cloned()
                .unwrap_or(Value::Null);
            let summary = tool
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("工具调用")
                .to_string();
            let tool_name = tool
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let options: Vec<PromptOption> = value
                .get("params")
                .and_then(|params| params.get("options"))
                .and_then(Value::as_array)
                .map(|options| {
                    options
                        .iter()
                        .map(|option| PromptOption {
                            option_id: option
                                .get("optionId")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            label: option
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            kind: normalize_option_kind(
                                option
                                    .get("kind")
                                    .and_then(Value::as_str)
                                    .unwrap_or(""),
                            ),
                        })
                        .collect()
                })
                .unwrap_or_default();
            if options.is_empty() {
                send(
                    stdin,
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "outcome": { "outcome": "cancelled" } }
                    }),
                )
                .await?;
                return Ok(());
            }
            let request_id = format!("acp-{}", request_key(id));
            pending.insert(
                request_id.clone(),
                PendingRequest {
                    id: id.clone(),
                    kind: PendingKind::Permission,
                },
            );
            let _ = events.send(AcpEvent::Permission(PermissionPrompt {
                request_id,
                tool_name,
                reason: summary,
                risk: String::new(),
                options,
                member: None,
            }));
        }
        "elicitation/create" => {
            let params = value.get("params");
            let mode = params
                .and_then(|params| params.get("mode"))
                .and_then(Value::as_str)
                .unwrap_or("form");
            if mode != "form" {
                // URL 模式本轮不支持：明确 decline，别让 agent 挂着。
                send(
                    stdin,
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "action": "decline" }
                    }),
                )
                .await?;
                return Ok(());
            }
            let message = params
                .and_then(|params| params.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("请回答问题")
                .to_string();
            let schema = params
                .and_then(|params| params.get("requestedSchema"))
                .cloned()
                .unwrap_or(Value::Null);
            let required: Vec<String> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let Some(fields) = parse_elicitation_fields(&schema, &required) else {
                send(
                    stdin,
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "action": "decline" }
                    }),
                )
                .await?;
                let _ = events.send(AcpEvent::Error(
                    "表单包含暂不支持的字段类型，已拒绝".to_string(),
                ));
                return Ok(());
            };
            if fields.is_empty() {
                send(
                    stdin,
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "action": "decline" }
                    }),
                )
                .await?;
                return Ok(());
            }
            let request_id = format!("acp-elicit-{}", request_key(id));
            pending.insert(
                request_id.clone(),
                PendingRequest {
                    id: id.clone(),
                    kind: PendingKind::Elicitation,
                },
            );
            let _ = events.send(AcpEvent::Elicitation(ElicitationCard {
                request_id,
                message,
                fields,
                member: None,
            }));
        }
        // 文件/终端能力我们声明为 false；仍被请求时明确拒绝，别让 agent 挂住。
        _ => {
            send(
                stdin,
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("client does not support {method}") }
                }),
            )
            .await?;
        }
    }
    Ok(())
}

fn request_key(id: &Value) -> String {
    if let Some(number) = id.as_u64() {
        return number.to_string();
    }
    if let Some(text) = id.as_str() {
        return text.to_string();
    }
    id.to_string()
}

/// ACP 权限选项 kind（`allow_once`/`reject_once`/…）→ 试点 UI 的
/// `allowOnce`/`allowAlways`/`deny` 口径（样式与 Alt+A/D 靠它匹配）。
fn normalize_option_kind(kind: &str) -> String {
    match kind {
        "allow_once" => "allowOnce".to_string(),
        "allow_always" => "allowAlways".to_string(),
        "reject_once" | "reject_always" => "deny".to_string(),
        other => other.to_string(),
    }
}

fn parse_elicitation_fields(
    schema: &Value,
    required: &[String],
) -> Option<Vec<ElicitationField>> {
    let properties = schema.get("properties")?.as_object()?;
    if properties.len() > 12 {
        return None;
    }
    properties
        .iter()
        .map(|(key, property)| {
            let title = property
                .get("title")
                .and_then(Value::as_str)
                .filter(|title| !title.trim().is_empty())
                .unwrap_or(key)
                .to_string();
            let kind = match property.get("type").and_then(Value::as_str) {
                Some("array") => {
                    let items = property.get("items");
                    let options = items.map(options_from_schema).unwrap_or_default();
                    if !options.is_empty() {
                        ElicitationFieldKind::Multi(options)
                    } else if items
                        .and_then(|items| items.get("type"))
                        .and_then(Value::as_str)
                        == Some("string")
                    {
                        ElicitationFieldKind::TextList
                    } else {
                        return None;
                    }
                }
                Some("boolean") => ElicitationFieldKind::Boolean,
                Some("integer") => ElicitationFieldKind::Number { integer: true },
                Some("number") => ElicitationFieldKind::Number { integer: false },
                Some("string") => {
                    let options = options_from_schema(property);
                    if options.is_empty() {
                        ElicitationFieldKind::Text
                    } else {
                        ElicitationFieldKind::Single(options)
                    }
                }
                _ => return None,
            };
            if matches!(&kind, ElicitationFieldKind::Single(options) | ElicitationFieldKind::Multi(options) if options.len() > 50)
            {
                return None;
            }
            Some(ElicitationField {
                key: key.clone(),
                title,
                kind,
                required: required.iter().any(|name| name == key),
            })
        })
        .collect()
}

fn options_from_schema(schema: &Value) -> Vec<(String, String)> {
    let mut options = Vec::new();
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        for value in values {
            if let Some(text) = scalar_string(value) {
                options.push((text.clone(), text));
            }
        }
    }
    for key in ["oneOf", "anyOf"] {
        let Some(items) = schema.get(key).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let raw = item.get("const").or_else(|| item.get("value")).unwrap_or(item);
            let Some(value) = scalar_string(raw) else {
                continue;
            };
            let label = item
                .get("title")
                .and_then(Value::as_str)
                .filter(|title| !title.trim().is_empty())
                .unwrap_or(&value)
                .to_string();
            options.push((value, label));
        }
    }
    options
}

fn scalar_string(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if let Some(number) = value.as_i64() {
        return Some(number.to_string());
    }
    if let Some(number) = value.as_f64() {
        return Some(number.to_string());
    }
    value.as_bool().map(|flag| flag.to_string())
}

fn content_json(content: &ElicitationContent) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in content {
        let json_value = match value {
            ElicitationValue::Text(text) => json!(text),
            ElicitationValue::List(items) => json!(items),
            ElicitationValue::Bool(flag) => json!(flag),
            ElicitationValue::Integer(number) => json!(number),
            ElicitationValue::Number(number) => json!(number),
        };
        map.insert(key.clone(), json_value);
    }
    Value::Object(map)
}

fn handle_notification(
    method: &str,
    value: &Value,
    events: &UnboundedSender<AcpEvent>,
) {
    if method != "session/update" {
        return;
    }
    let Some(update) = value.get("params").and_then(|params| params.get("update")) else {
        return;
    };
    match update.get("sessionUpdate").and_then(Value::as_str) {
        Some("agent_message_chunk") => {
            if let Some(text) = update
                .get("content")
                .and_then(|content| content.get("text"))
                .and_then(Value::as_str)
                && !text.is_empty()
            {
                // pi-acp 启动时会先灌一段「## Context / ## Skills」信息块；不是回复，丢掉。
                let trimmed = text.trim_start();
                if trimmed.starts_with("## Context") {
                    return;
                }
                let _ = events.send(AcpEvent::Delta(text.to_string()));
            }
        }
        Some("tool_call") | Some("tool_call_update") => {
            let call_id = update
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let name = update
                .get("title")
                .and_then(Value::as_str)
                .or_else(|| update.get("kind").and_then(Value::as_str))
                .unwrap_or("tool")
                .to_string();
            let status = update
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending");
            let state = match status {
                "completed" => "completed",
                "failed" | "error" => "error",
                "in_progress" | "running" => "running",
                _ => "scheduled",
            };
            let _ = events.send(AcpEvent::Tool(ToolCard {
                call_id,
                name,
                state: state.to_string(),
                title: String::new(),
                detail: String::new(),
            }));
        }
        _ => {}
    }
}

async fn send(
    stdin: &mut BufWriter<tokio::process::ChildStdin>,
    value: Value,
) -> Result<(), String> {
    let line = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|error| format!("写 ACP stdin 失败：{error}"))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|error| format!("写 ACP stdin 失败：{error}"))?;
    stdin
        .flush()
        .await
        .map_err(|error| format!("flush ACP stdin 失败：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::mpsc::UnboundedReceiver;

    async fn recv(rx: &mut UnboundedReceiver<AcpEvent>) -> AcpEvent {
        tokio::time::timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("ACP 事件超时")
            .expect("ACP 事件通道关闭")
    }

    fn mock_acp() -> Option<String> {
        // mock 需要 node；没有 node 的开发机（或 CI）直接跳过，不误报失败。
        std::process::Command::new("node")
            .arg("--version")
            .output()
            .ok()
            .filter(|output| output.status.success())?;
        Some(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/mock-acp.mjs").to_string())
    }

    fn workspace() -> String {
        let dir = std::env::temp_dir().join("bot-gpui-mock-acp");
        let _ = std::fs::create_dir_all(&dir);
        dir.to_string_lossy().into_owned()
    }

    /// session/load：pi-acp 会在 load 响应前重放历史；客户端必须把 ready 前的帧丢掉，
    /// 但 ready 之后照常对话（续聊成功）。
    #[tokio::test]
    async fn session_load_drops_replay_and_resumes() {
        let Some(mock) = mock_acp() else {
            eprintln!("跳过：本机没有 node");
            return;
        };
        let cwd = workspace();

        // 第一段：新会话 + 一次审批回合（顺便确认 mock 本身正常）。
        let (tx, mut rx) = unbounded_channel();
        let handle = spawn(mock.clone(), cwd.clone(), None, None, tx);
        let AcpEvent::Ready { session_id, .. } = recv(&mut rx).await else {
            panic!("新会话没有先发 Ready");
        };
        assert_eq!(session_id, "mock-session-1");
        handle.prompt("hi");
        let mut first = String::new();
        loop {
            match recv(&mut rx).await {
                AcpEvent::Permission(prompt) => handle.resolve(prompt.request_id, "allow"),
                AcpEvent::Delta(text) => first.push_str(&text),
                AcpEvent::Done => break,
                AcpEvent::Error(message) => panic!("会话出错：{message}"),
                _ => {}
            }
        }
        assert!(first.contains("已允许"), "第一段回复异常：{first}");
        drop(handle);

        // 第二段：带 resume 重建；Ready 之前不能出现任何 Delta（历史重放）。
        let (tx, mut rx) = unbounded_channel();
        let handle = spawn(mock, cwd, Some("mock-session-1".to_string()), None, tx);
        let mut before_ready = Vec::new();
        let resumed = loop {
            match recv(&mut rx).await {
                AcpEvent::Ready { session_id, .. } => break session_id,
                AcpEvent::Delta(text) => before_ready.push(text),
                AcpEvent::Error(message) => panic!("续聊出错：{message}"),
                _ => {}
            }
        };
        assert_eq!(resumed, "mock-session-1");
        assert!(
            before_ready.is_empty(),
            "历史重放不应进当前气泡：{before_ready:?}"
        );

        handle.prompt("我刚才让你记住的数字是多少？");
        let mut second = String::new();
        loop {
            match recv(&mut rx).await {
                AcpEvent::Permission(prompt) => handle.resolve(prompt.request_id, "allow"),
                AcpEvent::Delta(text) => second.push_str(&text),
                AcpEvent::Done => break,
                AcpEvent::Error(message) => panic!("续聊出错：{message}"),
                _ => {}
            }
        }
        assert!(second.contains("已允许"), "续聊回复异常：{second}");
        assert!(
            !second.contains("历史"),
            "重放文本不能出现在新回合：{second}"
        );
    }

    /// elicitation/create（form）：多字段（单选/多选/文本）→ 表单卡 → content 回填。
    #[tokio::test]
    async fn elicitation_form_roundtrip() {
        let Some(mock) = mock_acp() else {
            eprintln!("跳过：本机没有 node");
            return;
        };
        let (tx, mut rx) = unbounded_channel();
        let handle = spawn(mock, workspace(), None, None, tx);
        let AcpEvent::Ready { .. } = recv(&mut rx).await else {
            panic!("没有先发 Ready");
        };
        handle.prompt("elicit 一个表单");
        let mut reply = String::new();
        loop {
            match recv(&mut rx).await {
                AcpEvent::Elicitation(card) => {
                    assert_eq!(card.message, "请选择并填写备注");
                    assert_eq!(card.fields.len(), 3);
                    let q0 = card
                        .fields
                        .iter()
                        .find(|field| field.key == "q_0")
                        .expect("缺少 q_0");
                    assert!(q0.required);
                    match &q0.kind {
                        ElicitationFieldKind::Single(options) => {
                            assert_eq!(
                                options,
                                &vec![
                                    ("A".to_string(), "A 选项".to_string()),
                                    ("B".to_string(), "B 选项".to_string())
                                ]
                            );
                        }
                        other => panic!("q_0 不是单选：{other:?}"),
                    }
                    let tags = card
                        .fields
                        .iter()
                        .find(|field| field.key == "tags")
                        .expect("缺少 tags");
                    assert!(matches!(tags.kind, ElicitationFieldKind::Multi(_)));
                    let note = card
                        .fields
                        .iter()
                        .find(|field| field.key == "note")
                        .expect("缺少 note");
                    assert!(matches!(note.kind, ElicitationFieldKind::Text));
                    let mut content = std::collections::HashMap::new();
                    content.insert(
                        "q_0".to_string(),
                        ElicitationValue::Text("A".to_string()),
                    );
                    content.insert(
                        "tags".to_string(),
                        ElicitationValue::List(vec!["x".to_string(), "y".to_string()]),
                    );
                    content.insert(
                        "note".to_string(),
                        ElicitationValue::Text("你好".to_string()),
                    );
                    handle.resolve_elicitation(card.request_id, Some(content));
                }
                AcpEvent::Delta(text) => reply.push_str(&text),
                AcpEvent::Done => break,
                AcpEvent::Error(message) => panic!("表单回合出错：{message}"),
                _ => {}
            }
        }
        assert!(reply.contains("表单回执：accept"), "回执异常：{reply}");
        assert!(reply.contains("\"q_0\":\"A\""), "缺少 q_0：{reply}");
        assert!(reply.contains("\"note\":\"你好\""), "缺少 note：{reply}");
    }

    #[test]
    fn launch_args_cover_known_acp_entries() {
        assert_eq!(launch_args("hermes"), vec!["hermes", "acp"]);
        assert_eq!(
            launch_args("C:/Tools/hermes.exe"),
            vec!["C:/Tools/hermes.exe", "acp"]
        );
        assert_eq!(
            launch_args("C:/Tools/hermes-acp.exe"),
            vec!["C:/Tools/hermes-acp.exe"]
        );
        assert_eq!(
            launch_args("zcode-acp-server"),
            vec!["zcode-acp-server", "server"]
        );
        assert_eq!(
            launch_args("C:/Tools/mock.mjs"),
            vec!["node", "C:/Tools/mock.mjs"]
        );
    }

    #[test]
    fn elicitation_schema_supports_numbers_and_text_lists() {
        let schema = json!({
            "type": "object",
            "properties": {
                "count": { "type": "integer", "title": "数量" },
                "ratio": { "type": "number", "title": "比例" },
                "tags": { "type": "array", "title": "标签", "items": { "type": "string" } },
                "enabled": { "type": "boolean", "title": "启用" }
            }
        });
        let fields = parse_elicitation_fields(&schema, &[]).expect("schema 应可解析");
        let by_key = |key: &str| fields.iter().find(|field| field.key == key).expect("字段");
        assert!(matches!(
            by_key("count").kind,
            ElicitationFieldKind::Number { integer: true }
        ));
        assert!(matches!(
            by_key("ratio").kind,
            ElicitationFieldKind::Number { integer: false }
        ));
        assert!(matches!(by_key("tags").kind, ElicitationFieldKind::TextList));
        assert!(matches!(by_key("enabled").kind, ElicitationFieldKind::Boolean));
    }

    #[test]
    fn unsupported_schema_is_declined() {
        let schema = json!({
            "type": "object",
            "properties": { "blob": { "type": "object", "title": "对象" } }
        });
        assert!(parse_elicitation_fields(&schema, &[]).is_none());
    }

    #[test]
    fn permission_kinds_normalize_to_ui_names() {
        assert_eq!(normalize_option_kind("allow_once"), "allowOnce");
        assert_eq!(normalize_option_kind("allow_always"), "allowAlways");
        assert_eq!(normalize_option_kind("reject_once"), "deny");
        assert_eq!(normalize_option_kind("reject_always"), "deny");
        assert_eq!(normalize_option_kind("custom"), "custom");
    }

    /// 句柄是可 Clone 的（SessionCtx 克隆/传递很常见）；克隆体 drop 不能把会话关掉。
    #[tokio::test]
    async fn dropping_clone_keeps_session_alive() {
        let Some(mock) = mock_acp() else {
            eprintln!("跳过：本机没有 node");
            return;
        };
        let (tx, mut rx) = unbounded_channel();
        let handle = spawn(mock, workspace(), None, None, tx);
        let AcpEvent::Ready { .. } = recv(&mut rx).await else {
            panic!("没有先发 Ready");
        };
        drop(handle.clone());
        assert!(handle.prompt("hi"));
        let mut reply = String::new();
        loop {
            match recv(&mut rx).await {
                AcpEvent::Permission(prompt) => handle.resolve(prompt.request_id, "allow"),
                AcpEvent::Delta(text) => reply.push_str(&text),
                AcpEvent::Done => break,
                AcpEvent::Error(message) => panic!("克隆体 drop 后会话已断：{message}"),
                _ => {}
            }
        }
        assert!(reply.contains("已允许"), "回复异常：{reply}");
    }

    #[tokio::test]
    #[ignore]
    async fn hermes_acp_smoke() {
        let (tx, mut rx) = unbounded_channel();
        let handle = spawn(
            "hermes".to_string(),
            workspace(),
            None,
            None,
            tx,
        );
        let AcpEvent::Ready { session_id, .. } = recv(&mut rx).await else {
            panic!("没有先发 Ready");
        };
        eprintln!("ready: {session_id}");
        assert!(handle.prompt("hello"));
        let mut reply = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        loop {
            if tokio::time::Instant::now() > deadline {
                panic!("Hermes 超时，已收到：{reply}");
            }
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Some(AcpEvent::Delta(text))) => reply.push_str(&text),
                Ok(Some(AcpEvent::Done)) => break,
                Ok(Some(AcpEvent::Error(message))) => panic!("Hermes 出错：{message}"),
                Ok(Some(_)) => {}
                Ok(None) => panic!("ACP 通道关闭，已收到：{reply}"),
                Err(_) => {}
            }
        }
        eprintln!("reply: {reply}");
    }
}
