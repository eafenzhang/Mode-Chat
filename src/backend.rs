//! 真实 Agent 控制面（与 Mode Bot 壳同一套 `agent-process`）。
//!
//! 与上一版的区别：**多会话**。每个 Bot 用 `workspaceIdentity = mode-bot:teammate:<id>`
//! 单独建会话、单独订阅；事件按 `key` 打回 UI，切换 Bot 不互相污染。
//!
//! 接口只有两条命令：`ensure`（懒建会话）与 `send`（发一轮输入）。失败原样上报，不吞错。
//!
//! 环境：
//! - `BOT_PILOT_WORKSPACE`：Bot 目录为空时的兜底工作目录；
//! - `BOT_PILOT_MODEL`：`provider/model`（可选；缺省用 Agent 自己的默认）；
//! - `MODE_BUILTIN_PROVIDER_CONFIG_FILE`：provider 配置（与 Mode Bot 启动参数一致）；
//! - `MODE_STORAGE_DIR` / `MODE_SESSION_DB_PATH`：数据隔离（建议指向 `.mode-bot-pilot`）。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agent_process::{
    resolve_agent_command, AgentControlClient, AgentProcessError, AgentResolveContext,
    AgentRuntimeRegistry, AgentWorkspaceTarget, SpecFactory,
};
use agent_protocol::message::{MessageModel, ModelOptions};
use agent_protocol::v4::{
    CommandSessionId, CommandType, ConversationCommandEnvelope, ResolveInteractionAction,
    ResolveInteractionAnswer,
};
use agent_protocol::v4_rows::ConversationRow;
use automation_repo::AutomationRepo;
use automation_service::{AutomationService, AutomationServiceOptions};
// 协议方法字面量（agent-process 的 permission_reverse 模块是私有的，常量不导出）。
const INTERACTION_REQUEST_PERMISSION_METHOD: &str = "interaction/requestPermission";
const INTERACTION_REQUEST_USER_INPUT_METHOD: &str = "interaction/requestUserInput";
use mode_protocol_client::{RequestHandler, RequestReply};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::data::ModelSpec;

#[derive(Debug, Clone)]
pub struct GroupMember {
    pub member_id: String,
    /// 上次会话 id（重启续用；UI 从试点状态文件带过来）。
    pub session_id: Option<String>,
    pub name: String,
    pub role: String,
    pub is_admin: bool,
    pub is_chief: bool,
    pub identity: String,
    pub workspace: String,
    pub model: Option<ModelSpec>,
}

#[derive(Debug, Clone)]
pub struct GroupInfo {
    /// 频道 id（读 goal.json 用）。
    pub group_id: String,
    pub group_name: String,
    pub instructions: String,
    pub memory: String,
    pub responder_policy: String,
    pub member_names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GroupSpec {
    pub group_key: String,
    pub info: GroupInfo,
    pub members: Vec<GroupMember>,
}

#[derive(Debug, Clone)]
pub struct ToolCard {
    pub call_id: String,
    pub name: String,
    /// pending | running | done | error
    pub state: String,
    pub title: String,
    /// 展开详情（输入/输出/错误），最多约 6k 字符。
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct HistoryAttachment {
    pub file_name: String,
    pub mime: String,
    pub attachment_ref: String,
    pub row_id: u64,
    pub entity_id: String,
    pub index: u64,
    /// 群聊历史：该行的成员 id / session id（加载历史图需要定位到成员会话）。
    pub member_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub user: bool,
    /// 历史里的 reasoning 行（折叠展示）。
    pub reasoning: bool,
    pub text: String,
    pub tool: Option<ToolCard>,
    /// 用户消息附件（历史回放；文件名 + 读取句柄）。
    pub attachments: Vec<HistoryAttachment>,
    /// 频道成员名（群聊历史；私聊 None）。
    pub member: Option<String>,
    /// 消息创建时间（毫秒；跨成员归并排序用）。
    pub at: u64,
}

#[derive(Debug, Clone)]
pub struct PromptOption {
    pub option_id: String,
    pub label: String,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct QuestionCard {
    pub request_id: String,
    pub question: String,
    /// (value, label)
    pub options: Vec<(String, String)>,
    /// 频道成员归属（私聊为 None）；卡头标注「来自 X」。
    pub member: Option<String>,
}

/// ACP `elicitation/create`（form）的一个字段。
#[derive(Debug, Clone)]
pub struct ElicitationField {
    pub key: String,
    pub title: String,
    pub kind: ElicitationFieldKind,
    pub required: bool,
}

#[derive(Debug, Clone)]
pub enum ElicitationFieldKind {
    /// 自由文本（string 无 enum）。
    Text,
    /// 单选：value + label。
    Single(Vec<(String, String)>),
    /// 多选：value + label。
    Multi(Vec<(String, String)>),
    /// boolean。
    Boolean,
    /// number / integer（文本输入，提交时按类型解析）。
    Number { integer: bool },
    /// 无枚举的 string 数组（文本输入，逗号/换行分隔）。
    TextList,
}

/// 一张 ACP elicitation 表单卡。
#[derive(Debug, Clone)]
pub struct ElicitationCard {
    pub request_id: String,
    pub message: String,
    pub fields: Vec<ElicitationField>,
    /// 频道成员归属（私聊为 None）。
    pub member: Option<String>,
}

/// 表单回填值。
#[derive(Debug, Clone, PartialEq)]
pub enum ElicitationValue {
    Text(String),
    List(Vec<String>),
    Bool(bool),
    Integer(i64),
    Number(f64),
}

pub type ElicitationContent = HashMap<String, ElicitationValue>;

#[derive(Debug, Clone)]
pub struct PermissionPrompt {
    pub request_id: String,
    pub tool_name: String,
    pub reason: String,
    pub risk: String,
    pub options: Vec<PromptOption>,
    /// 频道成员归属（私聊为 None）；卡头标注「来自 X」。
    pub member: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// 恢复出来的历史消息（先于 Ready 到达，UI 用它替换 transcript）。
    History(Vec<HistoryEntry>),
    /// 会话已建好（订阅就绪）；slot 非空时是频道成员会话的持久化槽位。
    Ready {
        session_id: String,
        slot: Option<String>,
    },
    /// 试点自动消息（demo/selftest）也走这里，保证气泡与真实发送同一路径。
    UserEcho(String),
    Delta {
        member: Option<String>,
        text: String,
    },
    /// 思考过程流（reasoning 行）；`reset` 时 UI 用 text 替换整段。
    Reasoning {
        member: Option<String>,
        text: String,
        done: bool,
        reset: bool,
    },
    Done { member: Option<String> },
    Error {
        member: Option<String>,
        message: String,
    },
    /// 工具调用需要用户确认（v4 会话流 `permission.requested`）。
    Permission(PermissionPrompt),
    PermissionResolved { request_id: String },
    /// AskUserQuestion 等「向用户提问」（legacy 反向请求 + v4 userInput 事件双源）。
    Question(QuestionCard),
    QuestionResolved { request_id: String },
    /// 本地 ACP 引擎的 elicitation 表单（`elicitation/create`）。
    Elicitation(ElicitationCard),
    /// 工具活动（scheduled/started/progress/result/error → 一张卡按 call_id 原地更新）。
    Tool(ToolCard),
}

#[derive(Debug, Clone)]
pub enum BackendEvent {
    /// 控制面级状态（连接中/就绪）。
    Status(String),
    Session { key: String, event: SessionEvent },
    /// 插件市场概览（host `plugins/overview`）。
    PluginsOverview {
        key: String,
        bot_id: String,
        result: Result<agent_protocol::plugin::PluginsOverviewResult, String>,
    },
    /// 插件写操作结果（装/卸/启停/更新）。
    PluginAction {
        key: String,
        bot_id: String,
        label: String,
        result: Result<String, String>,
    },
    /// 插件配置 schema/当前值读取结果。
    PluginConfig {
        key: String,
        bot_id: String,
        plugin_id: String,
        result: Result<Box<agent_protocol::plugin::PluginInfo>, String>,
    },
    /// 历史附件图片读取结果（本地缓存路径）。
    AttachmentImage {
        attachment_ref: String,
        path: Option<String>,
        error: Option<String>,
    },
}

/// 待上传附件：UI 已读入内存（上限 20MiB 双侧校验）。
#[derive(Debug, Clone)]
pub struct AttachmentUpload {
    pub file_name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// 与 `PROTOCOL_V4_LIMITS.attachmentMaxBytes` 同口径。
const ATTACHMENT_MAX_BYTES: usize = 20 * 1024 * 1024;
/// 384KiB 可被 3 整除，除末片外 base64 不带 padding（与产品上传事务一致）。
const ATTACHMENT_CHUNK_BYTES: usize = 384 * 1024;

fn sha256_prefixed(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity("sha256:".len() + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 逐个上传附件（begin → chunk… → commit），返回可放进 `sendText.attachments` 的数组。
/// 失败时显式 abort，避免服务端留下半截 staging。
async fn upload_attachments(
    control: &AgentControlClient,
    target: &AgentWorkspaceTarget,
    session_id: &str,
    files: Vec<AttachmentUpload>,
) -> Result<Vec<serde_json::Value>, String> {
    use agent_protocol::v4_queries::{
        AttachmentBeginParams, AttachmentBeginResult, AttachmentChunkParams,
        AttachmentTerminalParams,
    };
    use base64::Engine as _;

    let mut refs = Vec::with_capacity(files.len());
    for file in files {
        let total_bytes = file.bytes.len();
        if total_bytes > ATTACHMENT_MAX_BYTES {
            return Err(format!(
                "附件 {} 超过 20MiB（{total_bytes} bytes）",
                file.file_name
            ));
        }
        let total_chunks = total_bytes.div_ceil(ATTACHMENT_CHUNK_BYTES) as u64;
        let upload_id = format!("upload-{}", uuid::Uuid::new_v4());
        let begin = control
            .attachment_begin_v4(
                target,
                AttachmentBeginParams {
                    connection_id: V4_CONNECTION_ID.to_string(),
                    upload_id: upload_id.clone(),
                    session_id: session_id.to_string(),
                    file_name: file.file_name.clone(),
                    mime: file.mime.clone(),
                    total_bytes: total_bytes as u64,
                    total_chunks,
                    checksum: sha256_prefixed(&file.bytes),
                },
                agent_process::RuntimePolicy::StartIfNeeded,
            )
            .await
            .map_err(|error| format!("附件上传开始失败：{error}"))?;
        let reference = match begin {
            AttachmentBeginResult::Committed(committed) => committed.attachment_ref,
            AttachmentBeginResult::Staging(staging) => {
                let mut next_chunk = staging.next_chunk_index;
                let uploaded: Result<String, String> = async {
                    while next_chunk < total_chunks {
                        let start = next_chunk as usize * ATTACHMENT_CHUNK_BYTES;
                        let end = (start + ATTACHMENT_CHUNK_BYTES).min(total_bytes);
                        let chunk = control
                            .attachment_chunk_v4(
                                target,
                                AttachmentChunkParams {
                                    connection_id: V4_CONNECTION_ID.to_string(),
                                    upload_id: upload_id.clone(),
                                    session_id: session_id.to_string(),
                                    chunk_index: next_chunk,
                                    data_base64: base64::engine::general_purpose::STANDARD
                                        .encode(&file.bytes[start..end]),
                                },
                                agent_process::RuntimePolicy::StartIfNeeded,
                            )
                            .await
                            .map_err(|error| format!("附件分片上传失败：{error}"))?;
                        if chunk.next_chunk_index != next_chunk + 1 {
                            return Err(format!(
                                "附件分片进度非法：期望 {} 实际 {}",
                                next_chunk + 1,
                                chunk.next_chunk_index
                            ));
                        }
                        next_chunk += 1;
                    }
                    let committed = control
                        .attachment_commit_v4(
                            target,
                            AttachmentTerminalParams {
                                connection_id: V4_CONNECTION_ID.to_string(),
                                upload_id: upload_id.clone(),
                                session_id: session_id.to_string(),
                            },
                            agent_process::RuntimePolicy::StartIfNeeded,
                        )
                        .await
                        .map_err(|error| format!("附件提交失败：{error}"))?;
                    Ok(committed.attachment_ref)
                }
                .await;
                match uploaded {
                    Ok(reference) => reference,
                    Err(error) => {
                        let _ = control
                            .attachment_abort_v4(
                                target,
                                AttachmentTerminalParams {
                                    connection_id: V4_CONNECTION_ID.to_string(),
                                    upload_id,
                                    session_id: session_id.to_string(),
                                },
                                agent_process::RuntimePolicy::StartIfNeeded,
                            )
                            .await;
                        return Err(error);
                    }
                }
            }
        };
        refs.push(serde_json::json!({
            "ref": reference,
            "fileName": file.file_name,
            "mime": file.mime,
            "bytes": total_bytes,
        }));
    }
    Ok(refs)
}

// ---- v4 会话面（实验开关 BOT_PILOT_V4=1；私聊先行） ----------------------------

const V4_CONNECTION_ID: &str = "rust-inprocess";

/// 本地智能体引擎（`BOT_PILOT_ENGINE=pi-acp|deepseek-acp|zcode-acp-server|hermes`）。
/// 只影响私聊；频道仍由 Mode Agent 执行群编排。Hermes 也走 ACP（`hermes acp`），
/// 这样权限/提问与其它 ACP 引擎同一条链路。
pub fn local_engine() -> Option<String> {
    let raw = std::env::var("BOT_PILOT_ENGINE").ok()?;
    crate::acp::engine_command(&raw)
}

/// 当前引擎展示名（设置 → 智能体 页用）。
pub fn active_engine_label() -> String {
    local_engine()
        .map(|command| acp_engine_label(&command))
        .unwrap_or_else(|| "Mode Agent".to_string())
}

fn acp_engine_label(command: &str) -> String {
    let stem = std::path::Path::new(command)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if stem == "hermes" || stem == "hermes-acp" {
        "Hermes Agent（ACP）".to_string()
    } else {
        command.to_string()
    }
}

/// 有效引擎：每个 Bot 的选择优先，其次环境变量，最后内嵌 pi-rust。
pub enum EngineChoice {
    /// 内嵌 pi-rust（Mode Chat 默认）。
    Rpi,
    Mode,
    Acp(String),
}

pub fn resolve_engine(selected: Option<&str>) -> EngineChoice {
    if let Some(raw) = selected
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("mode"))
        && let Some(command) = crate::acp::engine_command(raw)
    {
        return EngineChoice::Acp(command);
    }
    match local_engine() {
        Some(command) => EngineChoice::Acp(command),
        None => {
            // 迁移期逃生门：MODE_CHAT_ENGINE=v4 仍可回退旧的 Mode Agent 链路。
            if std::env::var("MODE_CHAT_ENGINE").ok().as_deref() == Some("v4") {
                EngineChoice::Mode
            } else {
                EngineChoice::Rpi
            }
        }
    }
}

/// 该 Bot 的回合是否由完整引擎执行（内嵌 rpi 或旧 Mode Agent；ACP 引擎能力受限）。
pub fn engine_is_mode(selected: Option<&str>) -> bool {
    !matches!(resolve_engine(selected), EngineChoice::Acp(_))
}

/// 建一个内嵌 rpi 会话（新会话/恢复历史共用）。
async fn open_rpi_session(
    key: &str,
    workspace: &str,
    identity: &str,
    model: Option<ModelSpec>,
    resume: Option<String>,
    member: Option<String>,
    events: &UnboundedSender<BackendEvent>,
) -> Result<(crate::rpi_engine::RpiSession, ModelSpec), String> {
    let spec = model
        .or_else(env_model)
        .or_else(|| crate::rpi_engine::default_model_spec(identity, workspace))
        .ok_or_else(|| "未选择模型：请先在右栏或设置里为这个 Bot 选一个模型".to_string())?;
    let engine_model = crate::rpi_engine::resolve_model(&spec)?;
    let (approval_level, allowlist) = crate::rpi_engine::approval_policy(identity, workspace);
    let system_prompt = crate::rpi_engine::system_prompt_for(std::path::Path::new(workspace));
    let options = crate::rpi_engine::OpenOptions {
        workspace: workspace.into(),
        resume_path: resume.filter(|path| std::path::Path::new(path).is_file()),
        model: engine_model,
        thinking: crate::rpi_engine::thinking_level_for(&spec),
        system_prompt,
        events: events.clone(),
        key: key.to_string(),
        member,
        approval_level,
        allowlist,
    };
    let session = crate::rpi_engine::RpiSession::open(options).await?;
    Ok((session, spec))
}

/// 启动一个 ACP 会话并把事件转成 BackendEvent。
fn spawn_acp_session(
    command: String,
    workspace: String,
    resume: Option<String>,
    slot: Option<String>,
    key: String,
    events: UnboundedSender<BackendEvent>,
) -> crate::acp::AcpHandle {
    let (acp_tx, mut acp_rx) = unbounded_channel::<crate::acp::AcpEvent>();
    let handle = crate::acp::spawn(command, workspace, resume, slot, acp_tx);
    tokio::spawn(async move {
        while let Some(event) = acp_rx.recv().await {
            let session = match event {
                crate::acp::AcpEvent::Ready { session_id, slot } => SessionEvent::Ready {
                    session_id,
                    slot,
                },
                crate::acp::AcpEvent::Delta(text) => SessionEvent::Delta {
                    member: None,
                    text,
                },
                crate::acp::AcpEvent::Tool(card) => SessionEvent::Tool(card),
                crate::acp::AcpEvent::Permission(prompt) => SessionEvent::Permission(prompt),
                crate::acp::AcpEvent::Elicitation(card) => SessionEvent::Elicitation(card),
                crate::acp::AcpEvent::Done => SessionEvent::Done { member: None },
                crate::acp::AcpEvent::Error(message) => SessionEvent::Error {
                    member: None,
                    message,
                },
            };
            if events
                .send(BackendEvent::Session {
                    key: key.clone(),
                    event: session,
                })
                .is_err()
            {
                break;
            }
        }
    });
    handle
}

pub(crate) fn v4_selftest() -> bool {
    std::env::var("BOT_PILOT_SELFTEST").ok().as_deref() == Some("1")
}

pub(crate) fn v4_selftest_delta(text: &str) {
    if v4_selftest() {
        use std::io::Write as _;
        print!("{text}");
        std::io::stdout().flush().ok();
    }
}

pub(crate) fn v4_selftest_done() {
    if v4_selftest() {
        println!("
[selftest] turn completed");
        std::process::exit(0);
    }
}

fn v4_envelope(
    client_id: &str,
    session_id: Option<&str>,
    command_type: CommandType,
    payload: serde_json::Value,
) -> ConversationCommandEnvelope {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    ConversationCommandEnvelope {
        ttft: None,
        command_id: format!(
            "bot-gpui-{}-{}-{}",
            command_type_str(command_type),
            millis,
            uuid::Uuid::new_v4()
        ),
        client_id: client_id.to_string(),
        session_id: CommandSessionId(session_id.map(str::to_string)),
        base_revision: None,
        base_log_epoch: None,
        command_type,
        payload,
        issued_at: serde_json::Number::from(millis),
    }
}

fn command_type_str(command_type: CommandType) -> &'static str {
    match command_type {
        CommandType::CreateSession => "create",
        CommandType::SendText => "send",
        _ => "cmd",
    }
}

async fn v4_create_session(
    control: &AgentControlClient,
    target: &AgentWorkspaceTarget,
    client_id: &str,
) -> Result<String, String> {
    // 连接器通过 MCP 进回合：与壳同一条 createSession 路径（缺密钥/坏库按空清单）。
    let connector_servers = crate::connectors::payload_for_workspace(
        &crate::data::data_root(),
        &target.workspace_path,
    );
    let mut payload = serde_json::json!({
        "workspaceId": target.workspace_path,
        "mcpServers": connector_servers,
    });
    if let Some(identity) = target
        .workspace_identity
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        payload["workspaceIdentity"] = serde_json::Value::String(identity.to_string());
    }
    let envelope = v4_envelope(client_id, None, CommandType::CreateSession, payload);
    let ack = control
        .send_conversation_command_v4(target, &envelope)
        .await
        .map_err(|error| format!("v4 createSession 失败：{error}"))?;
    if !ack.status.is_success() {
        return Err(format!(
            "v4 createSession 未受理（{}）：{}",
            ack.status.as_str(),
            ack.message.unwrap_or_default()
        ));
    }
    ack.result
        .and_then(|value| {
            value
                .get("sessionId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| "v4 createSession 没有返回 sessionId".to_string())
}

async fn v4_subscribe(
    control: &AgentControlClient,
    target: &AgentWorkspaceTarget,
    session_id: &str,
) -> Result<agent_process::ConversationFrameSubscription, String> {
    control
        .subscribe_conversation_frames(
            target,
            session_id,
            V4_CONNECTION_ID,
            agent_protocol::v4_transport::V4ClientMode::DesktopContinuous,
            None,
            None,
        )
        .await
        .map_err(|error| format!("v4 订阅失败：{error}"))
}

/// v4 快照 rows → 历史条目（只收 userInput / assistantText / toolCall）。
fn v4_history(
    snapshot: &agent_protocol::v4_snapshot::ConversationSnapshot,
    member: Option<String>,
    member_id: Option<&str>,
    session_id: Option<&str>,
) -> Vec<HistoryEntry> {
    let mut entries = Vec::new();
    for row in &snapshot.rows.window {
        match row {
            ConversationRow::UserInput(row) => {
                let attachments: Vec<HistoryAttachment> = row
                    .attachments
                    .as_ref()
                    .map(|items| {
                        items
                            .iter()
                            .enumerate()
                            .map(|(index, item)| HistoryAttachment {
                                file_name: item.file_name.clone(),
                                mime: item.mime.clone(),
                                attachment_ref: item.r#ref.clone(),
                                row_id: row.base.row_id.as_u64().unwrap_or(0),
                                entity_id: row.base.entity_id.clone().unwrap_or_default(),
                                index: index as u64,
                                member_id: member_id.map(str::to_string),
                                session_id: session_id.map(str::to_string),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // 纯附件消息没有正文，不能按空文本丢掉。
                if row.text.trim().is_empty() && attachments.is_empty() {
                    continue;
                }
                entries.push(HistoryEntry {
                    user: true,
                    reasoning: false,
                    text: row.text.clone(),
                    tool: None,
                    attachments,
                    member: member.clone(),
                    at: row.base.created_at.as_u64().unwrap_or(0),
                });
            }
            ConversationRow::AssistantText(row) => {
                if row.text.trim().is_empty() {
                    continue;
                }
                entries.push(HistoryEntry {
                    user: false,
                    reasoning: false,
                    text: row.text.clone(),
                    tool: None,
                    attachments: Vec::new(),
                    member: member.clone(),
                    at: row.base.created_at.as_u64().unwrap_or(0),
                });
            }
            ConversationRow::Reasoning(row) => {
                if row.text.trim().is_empty() {
                    continue;
                }
                entries.push(HistoryEntry {
                    user: false,
                    reasoning: true,
                    text: row.text.clone(),
                    tool: None,
                    attachments: Vec::new(),
                    member: member.clone(),
                    at: row.base.created_at.as_u64().unwrap_or(0),
                });
            }
            ConversationRow::ToolCall(row) => {
                entries.push(HistoryEntry {
                    user: false,
                    reasoning: false,
                    text: String::new(),
                    tool: Some(tool_card_from_v4(row)),
                    attachments: Vec::new(),
                    member: member.clone(),
                    at: row.base.created_at.as_u64().unwrap_or(0),
                });
            }
            _ => {}
        }
    }
    entries
}

fn tool_card_from_v4(row: &agent_protocol::v4_rows::ToolCallRow) -> ToolCard {
    let state = match row.status {
        agent_protocol::v4_rows::ToolCallStatus::InputStreaming => "scheduled",
        agent_protocol::v4_rows::ToolCallStatus::PendingApproval => "scheduled",
        agent_protocol::v4_rows::ToolCallStatus::Running => "running",
        agent_protocol::v4_rows::ToolCallStatus::Success => "completed",
        agent_protocol::v4_rows::ToolCallStatus::Error => "error",
        agent_protocol::v4_rows::ToolCallStatus::Cancelled => "cancelled",
    };
    let title = row
        .display
        .as_ref()
        .and_then(|display| display.get("title"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| row.input_text.chars().take(80).collect());
    let mut detail = String::new();
    let input = row.input_text.trim();
    if !input.is_empty() {
        detail.push_str("输入
");
        detail.push_str(&input.chars().take(2000).collect::<String>());
    }
    if let Some(output) = &row.output {
        let text = output.text.trim();
        if !text.is_empty() {
            if !detail.is_empty() {
                detail.push_str("

");
            }
            detail.push_str("输出
");
            detail.push_str(&text.chars().take(3500).collect::<String>());
        }
    } else if let Some(preview) = &row.output_preview {
        let text = preview.text.trim();
        if !text.is_empty() {
            if !detail.is_empty() {
                detail.push_str("

");
            }
            detail.push_str("输出（预览）
");
            detail.push_str(&text.chars().take(3500).collect::<String>());
        }
    }
    if let Some(error) = &row.error {
        if !detail.is_empty() {
            detail.push_str("

");
        }
        detail.push_str("错误
");
        detail.push_str(&format!("{}: {}", error.code, error.message));
    }
    let detail: String = detail.chars().take(6000).collect();
    ToolCard {
        call_id: row.tool_call_id.clone(),
        name: row.tool_name.clone(),
        state: state.to_string(),
        title,
        detail,
    }
}

/// v4 pending_interactions → 现有审批/问题卡；返回本次出现的 id 集合。
fn v4_emit_interactions(
    list: &[agent_protocol::v4_snapshot::PendingInteraction],
    member: Option<&str>,
    active_permissions: &mut std::collections::HashSet<String>,
    active_questions: &mut std::collections::HashSet<String>,
    event_key: &str,
    events: &UnboundedSender<BackendEvent>,
) {
    use agent_protocol::v4_snapshot::{
        PendingInteractionKind, PendingInteractionPayload, V4PermissionOptionKind,
    };
    let mut permissions = std::collections::HashSet::new();
    let mut questions = std::collections::HashSet::new();
    for interaction in list {
        match (&interaction.kind, &interaction.payload) {
            (
                PendingInteractionKind::Permission,
                PendingInteractionPayload::Permission(payload),
            ) => {
                permissions.insert(interaction.interaction_id.clone());
                let options = payload
                    .options
                    .iter()
                    .map(|option| PromptOption {
                        option_id: option.option_id.clone(),
                        label: option.label.clone(),
                        kind: match option.kind {
                            V4PermissionOptionKind::AllowOnce => "allow_once",
                            V4PermissionOptionKind::AllowAlways => "allow_always",
                            V4PermissionOptionKind::Deny => "deny",
                            V4PermissionOptionKind::Custom => "custom",
                        }
                        .to_string(),
                    })
                    .collect();
                let risk = payload
                    .display
                    .as_ref()
                    .and_then(|display| display.get("risk"))
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| {
                        payload
                            .detail
                            .get("riskLevel")
                            .and_then(serde_json::Value::as_str)
                    })
                    .unwrap_or("")
                    .to_string();
                let _ = events.send(BackendEvent::Session {
                    key: event_key.to_string(),
                    event: SessionEvent::Permission(PermissionPrompt {
                        request_id: interaction.interaction_id.clone(),
                        tool_name: payload.tool_name.clone(),
                        reason: payload.summary.clone(),
                        risk,
                        options,
                        member: member.map(str::to_string),
                    }),
                });
            }
            (PendingInteractionKind::UserInput, PendingInteractionPayload::UserInput(payload)) => {
                questions.insert(interaction.interaction_id.clone());
                let question = payload
                    .questions
                    .as_ref()
                    .and_then(|questions| questions.first())
                    .map(|question| question.question.clone())
                    .unwrap_or_else(|| payload.prompt.clone());
                let options = payload
                    .questions
                    .as_ref()
                    .and_then(|questions| questions.first())
                    .map(|question| &question.options)
                    .map(|options| {
                        options
                            .iter()
                            .map(|option| (option.value.clone(), option.label.clone()))
                            .collect()
                    })
                    .or_else(|| {
                        payload.options.as_ref().map(|options| {
                            options
                                .iter()
                                .map(|option| (option.option_id.clone(), option.label.clone()))
                                .collect()
                        })
                    })
                    .unwrap_or_default();
                let _ = events.send(BackendEvent::Session {
                    key: event_key.to_string(),
                    event: SessionEvent::Question(QuestionCard {
                        request_id: interaction.interaction_id.clone(),
                        question,
                        options,
                        member: member.map(str::to_string),
                    }),
                });
            }
            _ => {}
        }
    }
    for id in active_permissions.iter() {
        if !permissions.contains(id) {
            let _ = events.send(BackendEvent::Session {
                key: event_key.to_string(),
                event: SessionEvent::PermissionResolved {
                    request_id: id.clone(),
                },
            });
        }
    }
    for id in active_questions.iter() {
        if !questions.contains(id) {
            let _ = events.send(BackendEvent::Session {
                key: event_key.to_string(),
                event: SessionEvent::QuestionResolved {
                    request_id: id.clone(),
                },
            });
        }
    }
    *active_permissions = permissions;
    *active_questions = questions;
}

/// 把 v4 conversation 帧翻译成既有 UI 事件（文本流 / 工具卡 / Done / Error）。
fn spawn_v4_forwarder(
    event_key: String,
    member: Option<String>,
    emit_history: bool,
    turn_started: std::sync::Arc<std::sync::atomic::AtomicBool>,
    mut frames: agent_process::ConversationFrameSubscription,
    events: UnboundedSender<BackendEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use agent_protocol::v4_delta::ConversationDelta;
        use agent_protocol::v4_transport::ConversationFramePayload;
        // rowId → 已下发的助手正文（用长度差补前缀，避免 row.appended 与 row.delta 重复）。
        let mut rendered: HashMap<String, String> = HashMap::new();
        let mut saw_error: Option<String> = None;
        let mut active_permissions: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut active_questions: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // 已完成的 assistantText / reasoning 行：后续 upsert/delta 不能再回放。
        let mut completed_rows: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // reasoning 行单独累计，不能混进 assistant 正文的 rendered。
        let mut reasoning_rendered: HashMap<String, String> = HashMap::new();
        let mut reasoning_rows: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // 当前回合是否已有内容（完成时只发一次 Done）。
        let mut turn_active = false;
        while let Some(frame) = frames.recv().await {
            let agent_protocol::v4_transport::ConversationWireFrame::Complete(complete) = frame
            else {
                continue;
            };
            match complete.frame.payload {
                ConversationFramePayload::Snapshot(snapshot) => {
                    // 快照里的旧行先登记：resume 后 host 会重放这些 row 的 upsert，
                    // 不登记就会把上一回合的回复当成本回合内容灌进气泡。
                    for row in &snapshot.snapshot.rows.window {
                        match row {
                            ConversationRow::AssistantText(row) => {
                                let id = row.base.row_id.to_string();
                                rendered.insert(id.clone(), row.text.clone());
                                if !matches!(
                                    row.state,
                                    agent_protocol::v4_rows::AssistantTextState::Streaming
                                ) {
                                    completed_rows.insert(id);
                                }
                            }
                            ConversationRow::Reasoning(row) => {
                                let id = row.base.row_id.to_string();
                                reasoning_rows.insert(id.clone());
                                reasoning_rendered.insert(id.clone(), row.text.clone());
                                if !matches!(
                                    row.state,
                                    agent_protocol::v4_rows::ReasoningState::Streaming
                                ) {
                                    completed_rows.insert(id);
                                }
                            }
                            _ => {}
                        }
                    }
                    if emit_history {
                        let history =
                            v4_history(&snapshot.snapshot, member.clone(), None, None);
                        let _ = events.send(BackendEvent::Session {
                            key: event_key.clone(),
                            event: SessionEvent::History(history),
                        });
                    }
                    v4_emit_interactions(
                        &snapshot.snapshot.pending_interactions,
                        member.as_deref(),
                        &mut active_permissions,
                        &mut active_questions,
                        &event_key,
                        &events,
                    );
                }
                ConversationFramePayload::Deltas(deltas) => {
                    for delta in deltas.deltas {
                        if std::env::var("BOT_PILOT_DEBUG").ok().as_deref() == Some("1")
                            && let Ok(value) = serde_json::to_value(&delta)
                        {
                            let op = value
                                .get("op")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("?");
                            let kind = value
                                .get("row")
                                .and_then(|row| row.get("kind"))
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("");
                            eprintln!("[v4] member={member:?} op={op} rowKind={kind}");
                        }
                        match delta {
                            ConversationDelta::RowAppended(delta) => {
                                apply_v4_row(
                                    &delta.row,
                                    &mut rendered,
                                    &mut completed_rows,
                                    &mut reasoning_rendered,
                                    &mut reasoning_rows,
                                    &mut turn_active,
                                    &turn_started,
                                    &event_key,
                                    member.as_deref(),
                                    &events,
                                );
                            }
                            ConversationDelta::RowUpserted(delta) => {
                                apply_v4_row(
                                    &delta.row,
                                    &mut rendered,
                                    &mut completed_rows,
                                    &mut reasoning_rendered,
                                    &mut reasoning_rows,
                                    &mut turn_active,
                                    &turn_started,
                                    &event_key,
                                    member.as_deref(),
                                    &events,
                                );
                            }
                            ConversationDelta::RowDelta(delta) => {
                                let key = delta.row_id.to_string();
                                // only append known rows: reasoning 与 assistantText 都有
                                // path=text 增量，必须按 row 类型分流，否则思考进回复气泡。
                                if delta.path
                                    == agent_protocol::v4_delta::StreamablePath::Text
                                    && !completed_rows.contains(&key)
                                {
                                    if reasoning_rows.contains(&key) {
                                        if let Some(text) = reasoning_rendered.get_mut(&key) {
                                            text.push_str(&delta.append);
                                            let _ = events.send(BackendEvent::Session {
                                                key: event_key.clone(),
                                                event: SessionEvent::Reasoning {
                                                    member: member.clone(),
                                                    text: delta.append,
                                                    done: false,
                                                    reset: false,
                                                },
                                            });
                                        }
                                    } else if let Some(text) = rendered.get_mut(&key) {
                                        text.push_str(&delta.append);
                                        turn_active = true;
                                        v4_selftest_delta(&delta.append);
                                        let _ = events.send(BackendEvent::Session {
                                            key: event_key.clone(),
                                            event: SessionEvent::Delta {
                                                member: member.clone(),
                                                text: delta.append,
                                            },
                                        });
                                    }
                                }
                            }
                            ConversationDelta::StateUpdated(delta) => {
                                if let Some(interactions) = &delta.patch.pending_interactions {
                                    v4_emit_interactions(
                                        interactions,
                                        member.as_deref(),
                                        &mut active_permissions,
                                        &mut active_questions,
                                        &event_key,
                                        &events,
                                    );
                                }
                                if let Some(control) = &delta.patch.control {
                                    if let Some(error) = &control.last_error {
                                        let code = format!("{}:{}", error.code, error.message);
                                        if saw_error.as_deref() != Some(code.as_str()) {
                                            saw_error = Some(code);
                                            let _ = events.send(BackendEvent::Session {
                                                key: event_key.clone(),
                                                event: SessionEvent::Error {
                                                    member: member.clone(),
                                                    message: error.message.clone(),
                                                },
                                            });
                                        }
                                    } else if matches!(
                                        control.phase,
                                        agent_protocol::v4_snapshot::SessionPhase::CompletedSuccess
                                            | agent_protocol::v4_snapshot::SessionPhase::CompletedInterrupted
                                    ) {
                                        // 取消可能发生在首个 token 之前：此时没有正文，
                                        // 但回合已经发出过，仍要发 Done 让 UI 退出流式状态。
                                        let had_text = turn_active;
                                        let started = turn_started
                                            .swap(false, std::sync::atomic::Ordering::Relaxed);
                                        if had_text || started {
                                            turn_active = false;
                                            let _ = events.send(BackendEvent::Session {
                                                key: event_key.clone(),
                                                event: SessionEvent::Done {
                                                    member: member.clone(),
                                                },
                                            });
                                            if had_text {
                                                v4_selftest_done();
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn apply_v4_row(
    row: &ConversationRow,
    rendered: &mut HashMap<String, String>,
    completed_rows: &mut std::collections::HashSet<String>,
    reasoning_rendered: &mut HashMap<String, String>,
    reasoning_rows: &mut std::collections::HashSet<String>,
    turn_active: &mut bool,
    turn_started: &std::sync::atomic::AtomicBool,
    event_key: &str,
    member: Option<&str>,
    events: &UnboundedSender<BackendEvent>,
) {
    use agent_protocol::v4_rows::{AssistantTextState, ConversationRow};
    match row {
        ConversationRow::AssistantText(row) => {
            let key = row.base.row_id.to_string();
            if completed_rows.contains(&key) {
                return;
            }
            let seen = rendered.entry(key).or_default();
            // row 里是「到目前为止的全文」：只补没下发过的后缀。
            if row.text.starts_with(seen.as_str()) {
                let suffix = row.text[seen.len()..].to_string();
                if !suffix.is_empty() {
                    seen.push_str(&suffix);
                    *turn_active = true;
                    v4_selftest_delta(&suffix);
                    let _ = events.send(BackendEvent::Session {
                        key: event_key.to_string(),
                        event: SessionEvent::Delta {
                            member: member.map(str::to_string),
                            text: suffix,
                        },
                    });
                }
            } else if seen.is_empty() && !row.text.is_empty() {
                seen.push_str(&row.text);
                *turn_active = true;
                v4_selftest_delta(&row.text);
                let _ = events.send(BackendEvent::Session {
                    key: event_key.to_string(),
                    event: SessionEvent::Delta {
                        member: member.map(str::to_string),
                        text: row.text.clone(),
                    },
                });
            }
            if matches!(
                row.state,
                AssistantTextState::Complete
                    | AssistantTextState::Interrupted
                    | AssistantTextState::Failed
            ) {
                // 取消可能发生在首个 token 之前：这一行没有正文，但回合已经发出过，
                // 仍要发 Done，否则 UI 停在「等待回复…」、新会话也被拦住。
                let had_text = *turn_active;
                let started =
                    turn_started.swap(false, std::sync::atomic::Ordering::Relaxed);
                if had_text || started {
                    *turn_active = false;
                    let _ = events.send(BackendEvent::Session {
                        key: event_key.to_string(),
                        event: SessionEvent::Done {
                            member: member.map(str::to_string),
                        },
                    });
                    if had_text {
                        v4_selftest_done();
                    }
                }
                completed_rows.insert(row.base.row_id.to_string());
            }
        }
        ConversationRow::Reasoning(row) => {
            let key = row.base.row_id.to_string();
            if completed_rows.contains(&key) {
                return;
            }
            reasoning_rows.insert(key.clone());
            let seen = reasoning_rendered.entry(key.clone()).or_default();
            let (text, reset) = if row.text.starts_with(seen.as_str()) {
                (row.text[seen.len()..].to_string(), false)
            } else {
                // 行被替换：让 UI 用全文覆盖这一段。
                (row.text.clone(), true)
            };
            if !text.is_empty() {
                if reset {
                    seen.clear();
                }
                seen.push_str(&text);
                let _ = events.send(BackendEvent::Session {
                    key: event_key.to_string(),
                    event: SessionEvent::Reasoning {
                        member: member.map(str::to_string),
                        text,
                        done: false,
                        reset,
                    },
                });
            }
            if !matches!(
                row.state,
                agent_protocol::v4_rows::ReasoningState::Streaming
            ) {
                completed_rows.insert(key);
                let _ = events.send(BackendEvent::Session {
                    key: event_key.to_string(),
                    event: SessionEvent::Reasoning {
                        member: member.map(str::to_string),
                        text: String::new(),
                        done: true,
                        reset: false,
                    },
                });
            }
        }
        ConversationRow::ToolCall(row) => {
            let _ = events.send(BackendEvent::Session {
                key: event_key.to_string(),
                event: SessionEvent::Tool(tool_card_from_v4(row)),
            });
        }
        _ => {}
    }
}

#[derive(Debug)]
enum Command {
    Ensure {
        key: String,
        workspace: String,
        identity: String,
        model: Option<ModelSpec>,
        session_id: Option<String>,
        /// 该 Bot 选择的本地智能体引擎（None/"mode" = Mode Agent）。
        engine: Option<String>,
    },
    Send {
        key: String,
        text: String,
        attachments: Vec<AttachmentUpload>,
    },
    Resolve {
        key: String,
        request_id: String,
        option_id: String,
    },
    AnswerQuestion {
        key: String,
        request_id: String,
        question: String,
        answer: Option<String>,
    },
    /// ACP elicitation 表单作答；None = decline。
    AnswerElicitation {
        key: String,
        request_id: String,
        content: Option<ElicitationContent>,
    },
    /// 强制新建一条会话（不 resume；旧会话留在会话库）。
    /// 私有/频道都按 `key` 归属，会话 id 由 CLI 决定，所以不需要额外 bot_id。
    NewSession {
        key: String,
        workspace: String,
        identity: String,
        model: Option<ModelSpec>,
        engine: Option<String>,
    },
    /// 切换到历史会话：resume 指定 session_id，历史重灌 transcript；
    /// slot=bot_id（切换后的会话即持久化主槽位）。
    SwitchSession {
        key: String,
        workspace: String,
        identity: String,
        model: Option<ModelSpec>,
        bot_id: String,
        session_id: String,
        engine: Option<String>,
    },
    EnsureGroup { spec: GroupSpec },
    SendGroup {
        key: String,
        text: String,
        attachments: Vec<AttachmentUpload>,
    },
    /// 停止该 UI key（私聊/频道）下所有进行中的成员回合。
    Stop { key: String },
    /// 只停止频道里某个成员的进行中回合（群任务按条 Stop）。
    StopMember { key: String, member_id: String },
    /// 群任务重试/改派：给指定成员重新派发一个回合（不新增用户气泡）。
    ContinueTask {
        key: String,
        member_id: String,
        text: String,
    },
    /// 群记忆自动回顾（UI 在群回合结束后入队；5s 内新回合到达会作废旧任务）。
    MemoryReview {
        snapshot: crate::memory_review::ReviewSnapshot,
    },
    /// 插件市场/管理（host RPC；workspace 用选中 Bot 的目录与身份）。
    PluginsOverview {
        key: String,
        bot_id: String,
        workspace: String,
        identity: String,
    },
    PluginInstall {
        key: String,
        bot_id: String,
        workspace: String,
        identity: String,
        plugin_name: String,
        marketplace: String,
    },
    PluginUninstall {
        key: String,
        bot_id: String,
        workspace: String,
        identity: String,
        plugin_id: String,
    },
    PluginSetEnabled {
        key: String,
        bot_id: String,
        workspace: String,
        identity: String,
        plugin_id: String,
        enabled: bool,
    },
    PluginUpdate {
        key: String,
        bot_id: String,
        workspace: String,
        identity: String,
        plugin_id: Option<String>,
    },
    PluginMarketplaceUpdate {
        key: String,
        bot_id: String,
        workspace: String,
        identity: String,
    },
    /// 读取插件配置 schema（host `plugins/list` 里的 userConfig/configuredOptions）。
    PluginConfigLoad {
        key: String,
        bot_id: String,
        workspace: String,
        identity: String,
        plugin_id: String,
    },
    /// 写入插件配置（`plugins/configure`）。
    PluginConfigure {
        key: String,
        bot_id: String,
        workspace: String,
        identity: String,
        plugin_id: String,
        options: BTreeMap<String, serde_json::Value>,
        clear_keys: Vec<String>,
    },
    /// 清空插件用户配置（`plugins/resetConfig`）。
    PluginResetConfig {
        key: String,
        bot_id: String,
        workspace: String,
        identity: String,
        plugin_id: String,
    },
    LoadAttachmentImage {
        workspace: String,
        identity: String,
        session_id: String,
        attachment_ref: String,
        row_id: u64,
        entity_id: String,
        attachment_index: u64,
    },
    /// 丢弃该 UI key 的会话上下文（切换引擎时用；ACP/Hermes 子进程随之退出）。
    ResetSession { key: String },
}

pub struct BackendHandle {
    tx: UnboundedSender<Command>,
}

impl BackendHandle {
    /// 懒建会话：同一 key 只建一次；重复调用无副作用。
    pub fn ensure(
        &self,
        key: impl Into<String>,
        workspace: impl Into<String>,
        identity: impl Into<String>,
        model: Option<ModelSpec>,
        session_id: Option<String>,
        engine: Option<String>,
    ) {
        let _ = self.tx.send(Command::Ensure {
            key: key.into(),
            workspace: workspace.into(),
            identity: identity.into(),
            model,
            session_id,
            engine,
        });
    }

    pub fn send(&self, key: impl Into<String>, text: impl Into<String>) {
        self.send_with_attachments(key, text, Vec::new());
    }

    /// 带附件发送（附件先走 v4 attachment 事务；legacy 会话面会在后端拒绝）。
    pub fn send_with_attachments(
        &self,
        key: impl Into<String>,
        text: impl Into<String>,
        attachments: Vec<AttachmentUpload>,
    ) {
        let _ = self.tx.send(Command::Send {
            key: key.into(),
            text: text.into(),
            attachments,
        });
    }

    /// 建频道成员会话（每个成员一条隐藏会话，事件都打回同一个 group_key）。
    pub fn ensure_group(&self, spec: GroupSpec) {
        let _ = self.tx.send(Command::EnsureGroup { spec });
    }

    /// 丢弃一个会话（切换引擎后让下一次 Ensure 重新建）。
    pub fn reset_session(&self, key: impl Into<String>) {
        let _ = self.tx.send(Command::ResetSession { key: key.into() });
    }

    /// 停止进行中的回合（私聊/频道通用）。
    pub fn stop(&self, key: impl Into<String>) {
        let _ = self.tx.send(Command::Stop { key: key.into() });
    }

    /// 只停止频道里某个成员的回合（群任务行内 Stop）。
    pub fn stop_member(&self, key: impl Into<String>, member_id: impl Into<String>) {
        let _ = self.tx.send(Command::StopMember {
            key: key.into(),
            member_id: member_id.into(),
        });
    }

    /// 群任务重试/改派：给成员重新派发一个回合（文本由调用方拼好）。
    pub fn continue_task(
        &self,
        key: impl Into<String>,
        member_id: impl Into<String>,
        text: impl Into<String>,
    ) {
        let _ = self.tx.send(Command::ContinueTask {
            key: key.into(),
            member_id: member_id.into(),
            text: text.into(),
        });
    }

    /// 群回合结束后入队一次记忆回顾（backend 延迟 5s + 全局串行）。
    pub fn memory_review(&self, snapshot: crate::memory_review::ReviewSnapshot) {
        let _ = self.tx.send(Command::MemoryReview { snapshot });
    }

    /// 插件市场概览（host `plugins/overview`）。
    pub fn plugins_overview(
        &self,
        key: impl Into<String>,
        bot_id: impl Into<String>,
        workspace: impl Into<String>,
        identity: impl Into<String>,
    ) {
        let _ = self.tx.send(Command::PluginsOverview {
            key: key.into(),
            bot_id: bot_id.into(),
            workspace: workspace.into(),
            identity: identity.into(),
        });
    }

    pub fn plugin_install(
        &self,
        key: impl Into<String>,
        bot_id: impl Into<String>,
        workspace: impl Into<String>,
        identity: impl Into<String>,
        plugin_name: impl Into<String>,
        marketplace: impl Into<String>,
    ) {
        let _ = self.tx.send(Command::PluginInstall {
            key: key.into(),
            bot_id: bot_id.into(),
            workspace: workspace.into(),
            identity: identity.into(),
            plugin_name: plugin_name.into(),
            marketplace: marketplace.into(),
        });
    }

    pub fn plugin_uninstall(
        &self,
        key: impl Into<String>,
        bot_id: impl Into<String>,
        workspace: impl Into<String>,
        identity: impl Into<String>,
        plugin_id: impl Into<String>,
    ) {
        let _ = self.tx.send(Command::PluginUninstall {
            key: key.into(),
            bot_id: bot_id.into(),
            workspace: workspace.into(),
            identity: identity.into(),
            plugin_id: plugin_id.into(),
        });
    }

    pub fn plugin_set_enabled(
        &self,
        key: impl Into<String>,
        bot_id: impl Into<String>,
        workspace: impl Into<String>,
        identity: impl Into<String>,
        plugin_id: impl Into<String>,
        enabled: bool,
    ) {
        let _ = self.tx.send(Command::PluginSetEnabled {
            key: key.into(),
            bot_id: bot_id.into(),
            workspace: workspace.into(),
            identity: identity.into(),
            plugin_id: plugin_id.into(),
            enabled,
        });
    }

    pub fn plugin_update(
        &self,
        key: impl Into<String>,
        bot_id: impl Into<String>,
        workspace: impl Into<String>,
        identity: impl Into<String>,
        plugin_id: Option<String>,
    ) {
        let _ = self.tx.send(Command::PluginUpdate {
            key: key.into(),
            bot_id: bot_id.into(),
            workspace: workspace.into(),
            identity: identity.into(),
            plugin_id,
        });
    }

    pub fn plugin_marketplace_update(
        &self,
        key: impl Into<String>,
        bot_id: impl Into<String>,
        workspace: impl Into<String>,
        identity: impl Into<String>,
    ) {
        let _ = self.tx.send(Command::PluginMarketplaceUpdate {
            key: key.into(),
            bot_id: bot_id.into(),
            workspace: workspace.into(),
            identity: identity.into(),
        });
    }

    pub fn plugin_config_load(
        &self,
        key: impl Into<String>,
        bot_id: impl Into<String>,
        workspace: impl Into<String>,
        identity: impl Into<String>,
        plugin_id: impl Into<String>,
    ) {
        let _ = self.tx.send(Command::PluginConfigLoad {
            key: key.into(),
            bot_id: bot_id.into(),
            workspace: workspace.into(),
            identity: identity.into(),
            plugin_id: plugin_id.into(),
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn plugin_configure(
        &self,
        key: impl Into<String>,
        bot_id: impl Into<String>,
        workspace: impl Into<String>,
        identity: impl Into<String>,
        plugin_id: impl Into<String>,
        options: BTreeMap<String, serde_json::Value>,
        clear_keys: Vec<String>,
    ) {
        let _ = self.tx.send(Command::PluginConfigure {
            key: key.into(),
            bot_id: bot_id.into(),
            workspace: workspace.into(),
            identity: identity.into(),
            plugin_id: plugin_id.into(),
            options,
            clear_keys,
        });
    }

    pub fn plugin_reset_config(
        &self,
        key: impl Into<String>,
        bot_id: impl Into<String>,
        workspace: impl Into<String>,
        identity: impl Into<String>,
        plugin_id: impl Into<String>,
    ) {
        let _ = self.tx.send(Command::PluginResetConfig {
            key: key.into(),
            bot_id: bot_id.into(),
            workspace: workspace.into(),
            identity: identity.into(),
            plugin_id: plugin_id.into(),
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn load_attachment_image(
        &self,
        workspace: impl Into<String>,
        identity: impl Into<String>,
        session_id: impl Into<String>,
        attachment_ref: impl Into<String>,
        row_id: u64,
        entity_id: impl Into<String>,
        attachment_index: u64,
    ) {
        let _ = self.tx.send(Command::LoadAttachmentImage {
            workspace: workspace.into(),
            identity: identity.into(),
            session_id: session_id.into(),
            attachment_ref: attachment_ref.into(),
            row_id,
            entity_id: entity_id.into(),
            attachment_index,
        });
    }

    /// 强制新建一条私聊会话（旧会话留在会话库，UI 侧移入历史列表）。
    pub fn new_session(
        &self,
        key: impl Into<String>,
        workspace: impl Into<String>,
        identity: impl Into<String>,
        model: Option<ModelSpec>,
        engine: Option<String>,
    ) {
        let _ = self.tx.send(Command::NewSession {
            key: key.into(),
            workspace: workspace.into(),
            identity: identity.into(),
            model,
            engine,
        });
    }

    /// 切换到历史会话（resume + 重灌 transcript；旧会话留在历史列表）。
    #[allow(clippy::too_many_arguments)]
    pub fn switch_session(
        &self,
        key: impl Into<String>,
        workspace: impl Into<String>,
        identity: impl Into<String>,
        model: Option<ModelSpec>,
        bot_id: impl Into<String>,
        session_id: impl Into<String>,
        engine: Option<String>,
    ) {
        let _ = self.tx.send(Command::SwitchSession {
            key: key.into(),
            workspace: workspace.into(),
            identity: identity.into(),
            model,
            bot_id: bot_id.into(),
            session_id: session_id.into(),
            engine,
        });
    }

    /// 频道发言：按 @点名 / 回应策略选成员，逐成员组 prompt 后发送。
    pub fn send_group(
        &self,
        key: impl Into<String>,
        text: impl Into<String>,
        attachments: Vec<AttachmentUpload>,
    ) {
        let _ = self.tx.send(Command::SendGroup {
            key: key.into(),
            text: text.into(),
            attachments,
        });
    }

    /// 回答一次向用户的提问（v4 `resolveInteraction`，accept/cancel）。
    pub fn answer_question(
        &self,
        key: impl Into<String>,
        request_id: impl Into<String>,
        question: impl Into<String>,
        answer: Option<String>,
    ) {
        let _ = self.tx.send(Command::AnswerQuestion {
            key: key.into(),
            request_id: request_id.into(),
            question: question.into(),
            answer,
        });
    }

    /// 回答一次 ACP elicitation 表单；None = decline。
    pub fn answer_elicitation(
        &self,
        key: impl Into<String>,
        request_id: impl Into<String>,
        content: Option<ElicitationContent>,
    ) {
        let _ = self.tx.send(Command::AnswerElicitation {
            key: key.into(),
            request_id: request_id.into(),
            content,
        });
    }

    /// 回答一次审批（v4 `resolveInteraction`）。
    pub fn resolve(
        &self,
        key: impl Into<String>,
        request_id: impl Into<String>,
        option_id: impl Into<String>,
    ) {
        let _ = self.tx.send(Command::Resolve {
            key: key.into(),
            request_id: request_id.into(),
            option_id: option_id.into(),
        });
    }
}

fn home_dir() -> PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Bot 目录为空时的兜底工作目录。
pub fn fallback_workspace() -> PathBuf {
    std::env::var("BOT_PILOT_WORKSPACE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join("mode-bot-ws"))
}

fn env_model() -> Option<ModelSpec> {
    let raw = std::env::var("BOT_PILOT_MODEL").ok()?;
    let (provider_id, model_id) = raw.trim().split_once('/')?;
    if provider_id.is_empty() || model_id.is_empty() {
        return None;
    }
    let reasoning_level = std::env::var("BOT_PILOT_THOUGHT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "max".to_string());
    Some(ModelSpec {
        provider_id: provider_id.to_string(),
        model_id: model_id.to_string(),
        reasoning_level: Some(reasoning_level),
    })
}

fn message_model(model: &ModelSpec) -> MessageModel {
    MessageModel {
        provider_id: model.provider_id.clone(),
        model_id: model.model_id.clone(),
        options: Some(ModelOptions {
            reasoning_level: model.reasoning_level.clone(),
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoMode {
    None,
    Demo,
    Selftest,
}

/// sessionId → 会话归属（反向请求转发时定位 UI key）。
/// `member` 供审批/问题卡标注归属（私聊为 None）。
#[derive(Debug, Clone)]
struct SessionRef {
    event_key: String,
    internal_key: String,
    member: Option<String>,
}

/// 会话引擎：内嵌 pi-rust / 旧 Mode Agent（v4）/ 本地智能体（ACP）。
#[derive(Clone)]
enum SessionEngine {
    Rpi(Arc<crate::rpi_engine::RpiSession>),
    V4,
    Acp(crate::acp::AcpHandle),
}

struct SessionCtx {
    target: AgentWorkspaceTarget,
    session_id: String,
    model: Option<MessageModel>,
    /// 频道成员会话的展示名（私聊 None；Stop 命令的错误归属用）。
    member: Option<String>,
    engine: SessionEngine,
    /// 本会话是否已经发出过用户回合。Stop 可能发生在首个 token 之前，
    /// 此时 turn_active 仍为 false，但回合确实存在，收尾时仍要补发 Done，
    /// 否则 UI 会停在「等待回复…」且新会话被「生成中不换会话」拦住。
    turn_started: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// 启动后台线程：命令循环 + 每个会话的订阅转发任务。
pub fn spawn() -> (BackendHandle, UnboundedReceiver<BackendEvent>) {
    let (command_tx, mut command_rx) = unbounded_channel::<Command>();
    let (event_tx, event_rx) = unbounded_channel::<BackendEvent>();

    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = event_tx.send(BackendEvent::Status(format!("tokio 初始化失败：{error}")));
                return;
            }
        };
        runtime.block_on(async move {
            if let Err(error) = run_agent(&mut command_rx, &event_tx).await {
                let _ = event_tx.send(BackendEvent::Status(error));
            }
        });
    });

    (BackendHandle { tx: command_tx }, event_rx)
}

/// 安装包布局：`<exe 目录>/{agent,node,config}`（与主安装包的 resources 同名）。
/// 开发构建的 `target/debug` 没有这些目录，解析器会继续走 dev dist / PATH node。
struct PackagedRuntime {
    resources_dir: Option<PathBuf>,
    node_candidates: Vec<PathBuf>,
    builtin_provider: Option<PathBuf>,
}

fn packaged_runtime(exe: Option<&Path>) -> PackagedRuntime {
    let dir = exe.and_then(Path::parent).map(Path::to_path_buf);
    let node_name = if cfg!(windows) { "node.exe" } else { "node" };
    let node = dir
        .as_ref()
        .map(|dir| dir.join("node").join(node_name))
        .filter(|path| path.is_file());
    let builtin = dir
        .as_ref()
        .map(|dir| dir.join("config").join("provider").join("mode-builtin.json"))
        .filter(|path| path.is_file());
    PackagedRuntime {
        resources_dir: dir,
        node_candidates: node.into_iter().collect(),
        builtin_provider: builtin,
    }
}

async fn run_agent(
    commands: &mut UnboundedReceiver<Command>,
    events: &UnboundedSender<BackendEvent>,
) -> Result<(), String> {
    let _ = events.send(BackendEvent::Status("连接 Agent…".to_string()));

    let packaged = packaged_runtime(std::env::current_exe().ok().as_deref());
    let mut base_env: BTreeMap<String, String> = std::env::vars().collect();
    // resolver 只读 context.env；真正 spawn 的 env 要显式挂到 spec 上。
    let mut provider_env: Vec<(String, String)> = Vec::new();
    if let Some(builtin) = packaged.builtin_provider.as_ref()
        && !base_env.contains_key("MODE_BUILTIN_PROVIDER_CONFIG_FILE")
        && !base_env.contains_key("MODE_PERSONAL_PROVIDER_CONFIG_FILE")
    {
        let personal = crate::data::data_root()
            .join("v2")
            .join("provider_config.json");
        let builtin = builtin.to_string_lossy().into_owned();
        let personal = personal.to_string_lossy().into_owned();
        base_env.insert("MODE_BUILTIN_PROVIDER_CONFIG_FILE".to_string(), builtin.clone());
        base_env.insert(
            "MODE_PERSONAL_PROVIDER_CONFIG_FILE".to_string(),
            personal.clone(),
        );
        provider_env.push(("MODE_BUILTIN_PROVIDER_CONFIG_FILE".to_string(), builtin));
        provider_env.push(("MODE_PERSONAL_PROVIDER_CONFIG_FILE".to_string(), personal));
    }
    let resources_dir = packaged.resources_dir.clone();
    let node_candidates = packaged.node_candidates.clone();
    if std::env::var("BOT_PILOT_DEBUG").is_ok() {
        eprintln!(
            "[agent] resources_dir={resources_dir:?} node_candidates={node_candidates:?} bundled_provider={:?} provider_env={:?}",
            packaged.builtin_provider,
            provider_env.iter().map(|(key, _)| key).collect::<Vec<_>>()
        );
    }
    let upward_start = std::env::current_dir().unwrap_or_else(|_| fallback_workspace());
    let factory: SpecFactory = Arc::new(move |target| {
        let context = AgentResolveContext {
            workspace_path: PathBuf::from(&target.workspace_path),
            env: base_env.clone(),
            upward_start: upward_start.clone(),
            resources_dir: resources_dir.clone(),
            node_candidates: node_candidates.clone(),
            node_fallback: "node".to_string(),
            deployed_native_binary: None,
            presentation_surface: None,
            request_timeout: Duration::from_secs(120),
        };
        let resolved = resolve_agent_command(&context).map_err(|error| AgentProcessError::Spawn {
            command: "mode-agent".to_string(),
            message: error.to_string(),
        })?;
        let mut spec = resolved.spec;
        spec.env.extend(provider_env.clone());
        if std::env::var("BOT_PILOT_DEBUG").is_ok() {
            eprintln!(
                "[agent] source={:?} command={} args={:?} cwd={:?} spec_env={:?}",
                resolved.source,
                spec.command,
                spec.args,
                spec.cwd,
                spec.env.iter().map(|(key, _)| key).collect::<Vec<_>>()
            );
        }
        Ok(spec)
    });

    let control = Arc::new(AgentControlClient::new(Arc::new(AgentRuntimeRegistry::new(
        factory,
    ))));
    if let Err(error) =
        crate::automation_scheduler::start(Arc::clone(&control), &crate::data::data_root())
    {
        eprintln!("[automation] 调度器启动失败：{error}");
    }
    // IM（微信）运行时：与主程序共用 bot 配置/凭据/轮询锁；设置页通过全局句柄下发扫码。
    crate::im_service::start(crate::data::data_root(), Arc::clone(&control));
    let automation_db = crate::data::data_root()
        .join("v2")
        .join("tasks-index.sqlite");
    let automation_service = Arc::new(AutomationService::new(
        AutomationRepo::with_busy_timeout(automation_db.clone(), 5_000),
        AutomationServiceOptions {
            offset_secs: crate::time::local_offset_secs() as i32,
        },
    ));
    let automation_repo = Arc::new(AutomationRepo::with_busy_timeout(automation_db, 5_000));
    // 审批/问答反向请求：permission 持有（答案走 v4），requestUserInput 转发问题卡
    // 后持有（答案走 v4 {action, content}）。不持有会让 CLI 收到错误帧直接退出。
    // session_map: sessionId → 会话归属（UI key / 成员名 / 内部 key）；
    // request_map: 交互 requestId → 内部会话 key（卡片作答路由用）。
    let session_map: Arc<Mutex<HashMap<String, SessionRef>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let request_map: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    control.push_request_handler(pilot_interaction_handler(
        Arc::clone(&session_map),
        Arc::clone(&request_map),
        events.clone(),
        automation_service,
        automation_repo,
    ));

    let auto = if std::env::var("BOT_PILOT_SELFTEST").ok().as_deref() == Some("1") {
        AutoMode::Selftest
    } else if std::env::var("BOT_PILOT_DEMO").ok().as_deref() == Some("1") {
        AutoMode::Demo
    } else {
        AutoMode::None
    };

    let mut sessions: HashMap<String, SessionCtx> = HashMap::new();
    let mut groups: HashMap<String, GroupSpec> = HashMap::new();
    let mut member_talked: HashSet<String> = HashSet::new();
    let mut auto_sent = false;
    // UI key → 当前活跃会话的 internal key（新会话/切换会话后 Send 路由到新会话）。
    let mut active_internal: HashMap<String, String> = HashMap::new();
    // v4 订阅转发任务句柄：切会话/新会话时 abort 旧的，避免旧帧继续灌进同一个 chat。
    let mut v4_forwarders: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    // v4 命令的客户端标识（产品 renderer 也自造 client_id，格式对齐即可）。
    let client_id = format!("bot-gpui-{}", std::process::id());
    let stop_seq = std::sync::atomic::AtomicU64::new(0);
    let _ = events.send(BackendEvent::Status("就绪".to_string()));

    while let Some(command) = commands.recv().await {
        match command {
            Command::Ensure {
                key,
                workspace,
                identity,
                model,
                session_id: resume_id,
                engine,
            } => {
                if std::env::var("BOT_PILOT_DEBUG").is_ok() {
                    eprintln!(
                        "[cmd] ensure key={key} workspace={workspace} resume={resume_id:?} engine={engine:?}"
                    );
                }
                if let Some(ctx) = sessions.get(&key) {
                    // ACP 子进程退出（session/load 失败等）时句柄已关闭：
                    // 丢掉死会话，让本次 Ensure 重建，而不是永远 continue。
                    let closed = matches!(&ctx.engine, SessionEngine::Acp(handle) if handle.is_closed());
                    if !closed {
                        continue;
                    }
                    sessions.remove(&key);
                }
                match resolve_engine(engine.as_deref()) {
                    EngineChoice::Acp(command) => {
                        let target = AgentWorkspaceTarget::new(workspace.clone())
                            .with_identity(identity);
                        let handle = spawn_acp_session(
                            command,
                            workspace,
                            resume_id.clone(),
                            None,
                            key.clone(),
                            events.clone(),
                        );
                        sessions.insert(
                            key.clone(),
                            SessionCtx {
                                target,
                                session_id: resume_id.clone().unwrap_or_default(),
                                model: None,
                                member: None,
                                engine: SessionEngine::Acp(handle),
                                turn_started: std::sync::Arc::new(
                                    std::sync::atomic::AtomicBool::new(false),
                                ),
                            },
                        );
                        active_internal.insert(key.clone(), key.clone());
                        continue;
                    }
                    EngineChoice::Rpi => {
                        let spec = model
                            .clone()
                            .or_else(env_model)
                            .or_else(|| {
                                crate::rpi_engine::default_model_spec(&identity, &workspace)
                            });
                        let Some(spec) = spec else {
                            let _ = events.send(BackendEvent::Session {
                                key: key.clone(),
                                event: SessionEvent::Error {
                                    member: None,
                                    message: "未选择模型：请先在右栏或设置里为这个 Bot 选一个模型"
                                        .to_string(),
                                },
                            });
                            continue;
                        };
                        let engine_model = match crate::rpi_engine::resolve_model(&spec) {
                            Ok(model) => model,
                            Err(message) => {
                                let _ = events.send(BackendEvent::Session {
                                    key: key.clone(),
                                    event: SessionEvent::Error {
                                        member: None,
                                        message,
                                    },
                                });
                                continue;
                            }
                        };
                        let (approval_level, allowlist) =
                            crate::rpi_engine::approval_policy(&identity, &workspace);
                        let system_prompt = crate::rpi_engine::system_prompt_for(
                            std::path::Path::new(&workspace),
                        );
                        let resume_path = resume_id
                            .clone()
                            .filter(|path| std::path::Path::new(path).is_file());
                        let options = crate::rpi_engine::OpenOptions {
                            workspace: workspace.clone().into(),
                            resume_path,
                            model: engine_model,
                            thinking: crate::rpi_engine::thinking_level_for(&spec),
                            system_prompt,
                            events: events.clone(),
                            key: key.clone(),
                            member: None,
                            approval_level,
                            allowlist,
                        };
                        match crate::rpi_engine::RpiSession::open(options).await {
                            Ok(session) => {
                                if std::env::var("BOT_PILOT_DEBUG").is_ok() {
                                    eprintln!("[rpi] ensure open ok path={}", session.path);
                                }
                                let session = Arc::new(session);
                                let path = session.path.clone();
                                let history = crate::rpi_engine::history_entries(&path);
                                if !history.is_empty() {
                                    let _ = events.send(BackendEvent::Session {
                                        key: key.clone(),
                                        event: SessionEvent::History(history),
                                    });
                                }
                                sessions.insert(
                                    key.clone(),
                                    SessionCtx {
                                        target: AgentWorkspaceTarget::new(workspace.clone())
                                            .with_identity(identity),
                                        session_id: path.clone(),
                                        model: Some(message_model(&spec)),
                                        member: None,
                                        engine: SessionEngine::Rpi(Arc::clone(&session)),
                                        turn_started: std::sync::Arc::new(
                                            std::sync::atomic::AtomicBool::new(false),
                                        ),
                                    },
                                );
                                active_internal.insert(key.clone(), key.clone());
                                let _ = events.send(BackendEvent::Session {
                                    key: key.clone(),
                                    event: SessionEvent::Ready {
                                        session_id: path,
                                        slot: None,
                                    },
                                });
                                if auto != AutoMode::None && !auto_sent {
                                    auto_sent = true;
                                    let prompt = std::env::var("BOT_PILOT_PROMPT")
                                        .unwrap_or_else(|_| "只回复两个字：收到".to_string());
                                    tokio::spawn(async move {
                                        session.send(prompt, Vec::new()).await;
                                    });
                                }
                            }
                            Err(message) => {
                                let _ = events.send(BackendEvent::Session {
                                    key: key.clone(),
                                    event: SessionEvent::Error {
                                        member: None,
                                        message,
                                    },
                                });
                            }
                        }
                        continue;
                    }
                    EngineChoice::Mode => {}
                }
                                let target = AgentWorkspaceTarget::new(workspace).with_identity(identity);
                let model = model.or_else(env_model);
                // 并发建会话偶发 20s+（P5 观察项）：把 create/subscribe 耗时打出来，
                // 供真机复现时区分「CLI 冷启动」与「订阅慢」。
                let debug_timing = std::env::var("BOT_PILOT_DEBUG").ok().as_deref() == Some("1");
                let ensure_started = std::time::Instant::now();
                let mut chosen: Option<(
                    String,
                    agent_process::ConversationFrameSubscription,
                )> = None;
                if let Some(resume_id) = resume_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    && let Ok(frames) = v4_subscribe(&control, &target, resume_id).await
                {
                    chosen = Some((resume_id.to_string(), frames));
                }
                if chosen.is_none() {
                    let create_started = std::time::Instant::now();
                    match v4_create_session(&control, &target, &client_id).await {
                        Ok(session_id) => {
                            let created_ms = create_started.elapsed().as_millis();
                            let subscribe_started = std::time::Instant::now();
                            match v4_subscribe(&control, &target, &session_id).await {
                                Ok(frames) => {
                                    if debug_timing {
                                        eprintln!(
                                            "[timing] ensure {key} create={created_ms}ms subscribe={}ms",
                                            subscribe_started.elapsed().as_millis()
                                        );
                                    }
                                    chosen = Some((session_id, frames));
                                }
                                Err(message) => {
                                    let _ = events.send(BackendEvent::Session {
                                        key,
                                        event: SessionEvent::Error {
                                            member: None,
                                            message,
                                        },
                                    });
                                    continue;
                                }
                            }
                        }
                        Err(message) => {
                            let _ = events.send(BackendEvent::Session {
                                key,
                                event: SessionEvent::Error {
                                    member: None,
                                    message,
                                },
                            });
                            continue;
                        }
                    }
                }
                let Some((session_id, frames)) = chosen else {
                    continue;
                };
                if debug_timing {
                    eprintln!(
                        "[timing] ensure {key} total={}ms resume={}",
                        ensure_started.elapsed().as_millis(),
                        resume_id.is_some()
                    );
                }
                let turn_started =
                    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                sessions.insert(
                    key.clone(),
                    SessionCtx {
                        target: target.clone(),
                        session_id: session_id.clone(),
                        model: model.as_ref().map(message_model),
                        member: None,
                        engine: SessionEngine::V4,
                        turn_started: turn_started.clone(),
                    },
                );
                active_internal.insert(key.clone(), key.clone());
                if let Some(old) = v4_forwarders.insert(
                    key.clone(),
                    spawn_v4_forwarder(
                        key.clone(),
                        None,
                        true,
                        turn_started,
                        frames,
                        events.clone(),
                    ),
                ) {
                    old.abort();
                }
                let _ = events.send(BackendEvent::Session {
                    key: key.clone(),
                    event: SessionEvent::Ready {
                        session_id: session_id.clone(),
                        slot: None,
                    },
                });
                if auto != AutoMode::None && !auto_sent {
                    auto_sent = true;
                    let text = "只回复两个字：收到".to_string();
                    let _ = events.send(BackendEvent::Session {
                        key: key.clone(),
                        event: SessionEvent::UserEcho(text.clone()),
                    });
                    if let Some(model) = model.as_ref().map(message_model) {
                        let envelope = v4_envelope(
                            &client_id,
                            Some(&session_id),
                            CommandType::SendText,
                            serde_json::json!({
                                "text": text,
                                "modelSelection": model,
                            }),
                        );
                        let _ = control
                            .send_conversation_command_v4(&target, &envelope)
                            .await;
                    }
                }
                continue;
            
}
            Command::NewSession {
                key,
                workspace,
                identity,
                model,
                engine,
            } => {

                match resolve_engine(engine.as_deref()) {
                    EngineChoice::Acp(command) => {
                        let target = AgentWorkspaceTarget::new(workspace.clone())
                            .with_identity(identity);
                        let handle = spawn_acp_session(
                            command,
                            workspace,
                            None,
                            None,
                            key.clone(),
                            events.clone(),
                        );
                        sessions.insert(
                            key.clone(),
                            SessionCtx {
                                target,
                                session_id: String::new(),
                                model: None,
                                member: None,
                                engine: SessionEngine::Acp(handle),
                                turn_started: std::sync::Arc::new(
                                    std::sync::atomic::AtomicBool::new(false),
                                ),
                            },
                        );
                        active_internal.insert(key.clone(), key.clone());
                        continue;
                    }
                    EngineChoice::Rpi => {
                        match open_rpi_session(
                            &key,
                            &workspace,
                            &identity,
                            model.clone(),
                            None,
                            None,
                            events,
                        )
                        .await
                        {
                            Ok((session, spec)) => {
                                let path = session.path.clone();
                                sessions.insert(
                                    key.clone(),
                                    SessionCtx {
                                        target: AgentWorkspaceTarget::new(workspace.clone())
                                            .with_identity(identity),
                                        session_id: path.clone(),
                                        model: Some(message_model(&spec)),
                                        member: None,
                                        engine: SessionEngine::Rpi(Arc::new(session)),
                                        turn_started: std::sync::Arc::new(
                                            std::sync::atomic::AtomicBool::new(false),
                                        ),
                                    },
                                );
                                active_internal.insert(key.clone(), key.clone());
                                let _ = events.send(BackendEvent::Session {
                                    key: key.clone(),
                                    event: SessionEvent::Ready {
                                        session_id: path,
                                        slot: None,
                                    },
                                });
                            }
                            Err(message) => {
                                let _ = events.send(BackendEvent::Session {
                                    key: key.clone(),
                                    event: SessionEvent::Error {
                                        member: None,
                                        message,
                                    },
                                });
                            }
                        }
                        continue;
                    }
                    EngineChoice::Mode => {}
                }
                                let target = AgentWorkspaceTarget::new(workspace).with_identity(identity);
                let model = model.or_else(env_model);
                let session_id = match v4_create_session(&control, &target, &client_id).await {
                    Ok(session_id) => session_id,
                    Err(message) => {
                        let _ = events.send(BackendEvent::Session {
                            key,
                            event: SessionEvent::Error {
                                member: None,
                                message: format!("新建会话失败：{message}"),
                            },
                        });
                        continue;
                    }
                };
                match v4_subscribe(&control, &target, &session_id).await {
                    Ok(frames) => {
                        let turn_started =
                            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                        sessions.insert(
                            key.clone(),
                            SessionCtx {
                                target,
                                session_id: session_id.clone(),
                                model: model.as_ref().map(message_model),
                                member: None,
                                engine: SessionEngine::V4,
                                turn_started: turn_started.clone(),
                            },
                        );
                        active_internal.insert(key.clone(), key.clone());
                        if let Some(old) = v4_forwarders.insert(
                            key.clone(),
                            spawn_v4_forwarder(
                                key.clone(),
                                None,
                                true,
                                turn_started,
                                frames,
                                events.clone(),
                            ),
                        ) {
                            old.abort();
                        }
                        let _ = events.send(BackendEvent::Session {
                            key,
                            event: SessionEvent::Ready {
                                session_id,
                                slot: None,
                            },
                        });
                    }
                    Err(message) => {
                        let _ = events.send(BackendEvent::Session {
                            key,
                            event: SessionEvent::Error {
                                member: None,
                                message: format!("新建会话订阅失败：{message}"),
                            },
                        });
                    }
                }
                continue;
            
}
            Command::SwitchSession {
                key,
                workspace,
                identity,
                model,
                bot_id,
                session_id,
                engine,
            } => {

                // 本地引擎：切历史 = 用 session/load（Hermes 用 --resume）重建会话。
                match resolve_engine(engine.as_deref()) {
                    EngineChoice::Acp(command) => {
                        let target = AgentWorkspaceTarget::new(workspace.clone())
                            .with_identity(identity);
                        // 丢弃旧会话上下文（子进程随之退出），再按历史 id 重连。
                        sessions.remove(&key);
                        let handle = spawn_acp_session(
                            command,
                            workspace,
                            Some(session_id.clone()),
                            Some(bot_id.clone()),
                            key.clone(),
                            events.clone(),
                        );
                        sessions.insert(
                            key.clone(),
                            SessionCtx {
                                target,
                                session_id: session_id.clone(),
                                model: None,
                                member: None,
                                engine: SessionEngine::Acp(handle),
                                turn_started: std::sync::Arc::new(
                                    std::sync::atomic::AtomicBool::new(false),
                                ),
                            },
                        );
                        active_internal.insert(key.clone(), key.clone());
                        // Ready 由 ACP 的 session/load 成功响应触发（带 slot），
                        // 这里不能提前发：load 失败时 UI 会误以为会话已就绪。
                        continue;
                    }
                    EngineChoice::Rpi => {
                        sessions.remove(&key);
                        match open_rpi_session(
                            &key,
                            &workspace,
                            &identity,
                            model.clone(),
                            Some(session_id.clone()),
                            None,
                            events,
                        )
                        .await
                        {
                            Ok((session, spec)) => {
                                let session = Arc::new(session);
                                let path = session.path.clone();
                                sessions.insert(
                                    key.clone(),
                                    SessionCtx {
                                        target: AgentWorkspaceTarget::new(workspace.clone())
                                            .with_identity(identity),
                                        session_id: path.clone(),
                                        model: Some(message_model(&spec)),
                                        member: None,
                                        engine: SessionEngine::Rpi(Arc::clone(&session)),
                                        turn_started: std::sync::Arc::new(
                                            std::sync::atomic::AtomicBool::new(false),
                                        ),
                                    },
                                );
                                active_internal.insert(key.clone(), key.clone());
                                let _ = events.send(BackendEvent::Session {
                                    key: key.clone(),
                                    event: SessionEvent::Ready {
                                        session_id: path,
                                        slot: None,
                                    },
                                });
                                if auto != AutoMode::None && !auto_sent {
                                    auto_sent = true;
                                    let prompt = std::env::var("BOT_PILOT_PROMPT")
                                        .unwrap_or_else(|_| "只回复两个字：收到".to_string());
                                    tokio::spawn(async move {
                                        session.send(prompt, Vec::new()).await;
                                    });
                                }
                            }
                            Err(message) => {
                                let _ = events.send(BackendEvent::Session {
                                    key: key.clone(),
                                    event: SessionEvent::Error {
                                        member: None,
                                        message,
                                    },
                                });
                            }
                        }
                        continue;
                    }
                    EngineChoice::Mode => {}
                }
                                let target = AgentWorkspaceTarget::new(workspace).with_identity(identity);
                let model = model.clone().or_else(env_model);
                match v4_subscribe(&control, &target, &session_id).await {
                    Ok(frames) => {
                        let turn_started =
                            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                        if let Some(old) = v4_forwarders.remove(&key) {
                            old.abort();
                        }
                        if let Some(old) = v4_forwarders.insert(
                            key.clone(),
                            spawn_v4_forwarder(
                                key.clone(),
                                None,
                                true,
                                turn_started.clone(),
                                frames,
                                events.clone(),
                            ),
                        ) {
                            old.abort();
                        }
                        sessions.insert(
                            key.clone(),
                            SessionCtx {
                                target,
                                session_id: session_id.clone(),
                                model: model.as_ref().map(message_model),
                                member: None,
                                engine: SessionEngine::V4,
                                turn_started,
                            },
                        );
                        active_internal.insert(key.clone(), key.clone());
                        let _ = events.send(BackendEvent::Session {
                            key,
                            event: SessionEvent::Ready {
                                session_id,
                                slot: Some(bot_id),
                            },
                        });
                    }
                    Err(message) => {
                        let _ = events.send(BackendEvent::Session {
                            key,
                            event: SessionEvent::Error {
                                member: None,
                                message: format!("切换会话失败：{message}"),
                            },
                        });
                    }
                }
                continue;
            
}
            Command::Resolve {
                key,
                request_id,
                option_id,
            } => {
                // 内嵌 rpi 的审批卡：request_id 是工具调用 id，按会话闸门逐个尝试。
                let allow = matches!(option_id.as_str(), "allow_once" | "allow_always");
                let mut resolved = false;
                for ctx in sessions.values() {
                    if let SessionEngine::Rpi(session) = &ctx.engine
                        && session.gate.resolve(&request_id, allow)
                    {
                        resolved = true;
                        break;
                    }
                }
                if resolved {
                    continue;
                }
                let ctx_key = match sessions.get(&key) {
                    Some(_) => key.clone(),
                    None => request_map
                        .lock()
                        .ok()
                        .and_then(|map| map.get(&request_id).cloned())
                        .unwrap_or_else(|| key.clone()),
                };
                let Some(ctx) = sessions.get(&ctx_key) else {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: "会话尚未建立".to_string(),
                        },
                    });
                    continue;
                };
                if let SessionEngine::Acp(handle) = &ctx.engine {
                    handle.resolve(request_id, option_id);
                    continue;
                }
                let answer = ResolveInteractionAnswer {
                    option_id: Some(option_id),
                    free_text: None,
                    action: None,
                    content: None,
                };
                if let Err(error) = control
                    .resolve_interaction_v4(&ctx.target, &ctx.session_id, &request_id, answer)
                    .await
                {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: format!("审批回执失败：{error}"),
                        },
                    });
                }
            }
            Command::AnswerQuestion {
                key,
                request_id,
                question,
                answer,
            } => {
                let ctx_key = match sessions.get(&key) {
                    Some(_) => key.clone(),
                    None => request_map
                        .lock()
                        .ok()
                        .and_then(|map| map.get(&request_id).cloned())
                        .unwrap_or_else(|| key.clone()),
                };
                let Some(ctx) = sessions.get(&ctx_key) else {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: "会话尚未建立".to_string(),
                        },
                    });
                    continue;
                };
                let answer_frame = match answer {
                    Some(text) => ResolveInteractionAnswer {
                        action: Some(ResolveInteractionAction::Accept),
                        content: Some({
                            let mut answers = serde_json::Map::new();
                            answers.insert(
                                question.clone(),
                                serde_json::Value::String(text.clone()),
                            );
                            let mut map = serde_json::Map::new();
                            map.insert(
                                "answers".to_string(),
                                serde_json::Value::Object(answers),
                            );
                            map.insert(
                                "answer_0".to_string(),
                                serde_json::Value::String(text.clone()),
                            );
                            map.insert("answer".to_string(), serde_json::Value::String(text));
                            map
                        }),
                        ..Default::default()
                    },
                    None => ResolveInteractionAnswer {
                        action: Some(ResolveInteractionAction::Cancel),
                        ..Default::default()
                    },
                };
                if let Err(error) = control
                    .resolve_interaction_v4(&ctx.target, &ctx.session_id, &request_id, answer_frame)
                    .await
                {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: format!("提问回执失败：{error}"),
                        },
                    });
                }
            }
            Command::AnswerElicitation {
                key,
                request_id,
                content,
            } => {
                let ctx_key = match sessions.get(&key) {
                    Some(_) => key.clone(),
                    None => request_map
                        .lock()
                        .ok()
                        .and_then(|map| map.get(&request_id).cloned())
                        .unwrap_or_else(|| key.clone()),
                };
                let Some(ctx) = sessions.get(&ctx_key) else {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: "会话尚未建立".to_string(),
                        },
                    });
                    continue;
                };
                if let SessionEngine::Acp(handle) = &ctx.engine {
                    handle.resolve_elicitation(request_id, content);
                    continue;
                }
                let _ = events.send(BackendEvent::Session {
                    key,
                    event: SessionEvent::Error {
                        member: None,
                        message: "该引擎不支持表单提问".to_string(),
                    },
                });
            }
            Command::EnsureGroup { spec } => {

                groups.insert(spec.group_key.clone(), spec.clone());
                let mut merged_history: Vec<HistoryEntry> = Vec::new();
                for member in &spec.members {
                    let internal = member_session_key(&spec.group_key, &member.member_id);
                    if sessions.contains_key(&internal) {
                        continue;
                    }
                    if matches!(resolve_engine(None), EngineChoice::Rpi) {
                        let opened = open_rpi_session(
                            &spec.group_key,
                            &member.workspace,
                            &member.identity,
                            member.model.clone(),
                            member.session_id.clone(),
                            Some(member.name.clone()),
                            events,
                        )
                        .await;
                        match opened {
                            Ok((session, member_model)) => {
                                let path = session.path.clone();
                                sessions.insert(
                                    internal.clone(),
                                    SessionCtx {
                                        target: AgentWorkspaceTarget::new(
                                            member.workspace.clone(),
                                        )
                                        .with_identity(member.identity.clone()),
                                        session_id: path.clone(),
                                        model: Some(message_model(&member_model)),
                                        member: Some(member.name.clone()),
                                        engine: SessionEngine::Rpi(Arc::new(session)),
                                        turn_started: std::sync::Arc::new(
                                            std::sync::atomic::AtomicBool::new(false),
                                        ),
                                    },
                                );
                                let _ = events.send(BackendEvent::Session {
                                    key: spec.group_key.clone(),
                                    event: SessionEvent::Ready {
                                        session_id: path,
                                        slot: Some(internal.clone()),
                                    },
                                });
                            }
                            Err(message) => {
                                let _ = events.send(BackendEvent::Session {
                                    key: spec.group_key.clone(),
                                    event: SessionEvent::Error {
                                        member: Some(member.name.clone()),
                                        message: format!(
                                            "{} 建会话失败：{message}",
                                            member.name
                                        ),
                                    },
                                });
                            }
                        }
                        continue;
                    }
                                        let target = AgentWorkspaceTarget::new(member.workspace.clone())
                        .with_identity(member.identity.clone());
                    let model = member.model.clone().or_else(env_model);
                    let mut found: Option<(
                        String,
                        agent_process::ConversationFrameSubscription,
                    )> = None;
                    if let Some(resume_id) = member
                        .session_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        && let Ok(frames) = v4_subscribe(&control, &target, resume_id).await
                    {
                        found = Some((resume_id.to_string(), frames));
                    }
                    if found.is_none() {
                        match v4_create_session(&control, &target, &client_id).await {
                            Ok(session_id) => {
                                match v4_subscribe(&control, &target, &session_id).await {
                                    Ok(frames) => found = Some((session_id, frames)),
                                    Err(message) => {
                                        let _ = events.send(BackendEvent::Session {
                                            key: spec.group_key.clone(),
                                            event: SessionEvent::Error {
                                                member: Some(member.name.clone()),
                                                message: format!(
                                                    "{} 订阅会话失败：{message}",
                                                    member.name
                                                ),
                                            },
                                        });
                                        continue;
                                    }
                                }
                            }
                            Err(message) => {
                                let _ = events.send(BackendEvent::Session {
                                    key: spec.group_key.clone(),
                                    event: SessionEvent::Error {
                                        member: Some(member.name.clone()),
                                        message: format!(
                                            "{} 建会话失败：{message}",
                                            member.name
                                        ),
                                    },
                                });
                                continue;
                            }
                        }
                    }
                    let Some((session_id, mut frames)) = found else {
                        continue;
                    };
                    // 首帧是快照：先把该成员的历史取出来跨成员归并，再交给转发任务。
                    if let Ok(Some(
                        agent_protocol::v4_transport::ConversationWireFrame::Complete(complete),
                    )) = tokio::time::timeout(Duration::from_secs(3), frames.recv()).await
                        && let agent_protocol::v4_transport::ConversationFramePayload::Snapshot(
                            snapshot,
                        ) = complete.frame.payload
                    {
                        merged_history.extend(v4_history(
                            &snapshot.snapshot,
                            Some(member.name.clone()),
                            Some(&member.member_id),
                            Some(&session_id),
                        ));
                    }
                    if let Ok(mut map) = session_map.lock() {
                        map.insert(
                            session_id.clone(),
                            SessionRef {
                                event_key: spec.group_key.clone(),
                                internal_key: internal.clone(),
                                member: Some(member.name.clone()),
                            },
                        );
                    }
                    let turn_started =
                        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    if let Some(old) = v4_forwarders.insert(
                        internal.clone(),
                        spawn_v4_forwarder(
                            spec.group_key.clone(),
                            Some(member.name.clone()),
                            false,
                            turn_started.clone(),
                            frames,
                            events.clone(),
                        ),
                    ) {
                        old.abort();
                    }
                    sessions.insert(
                        internal.clone(),
                        SessionCtx {
                            target,
                            session_id: session_id.clone(),
                            model: model.as_ref().map(message_model),
                            member: Some(member.name.clone()),
                            engine: SessionEngine::V4,
                            turn_started,
                        },
                    );
                    let _ = events.send(BackendEvent::Session {
                        key: spec.group_key.clone(),
                        event: SessionEvent::Ready {
                            session_id,
                            slot: Some(internal.clone()),
                        },
                    });
                    continue;
                }
                let merged_history = merge_history(merged_history);
                if !merged_history.is_empty() {
                    let _ = events.send(BackendEvent::Session {
                        key: spec.group_key.clone(),
                        event: SessionEvent::History(merged_history),
                    });
                }
            }
            Command::SendGroup {
                key,
                text,
                attachments,
            } => {
                let Some(spec) = groups.get(&key).cloned() else {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: "频道会话尚未建立".to_string(),
                        },
                    });
                    continue;
                };
                // @点名走产品同源解析器（bot_team::mentions，含 @所有人/边界规则）；
                // 无人点名时按回应策略（all=全员，其余=管理员，无管理员回落首成员）。
                let member_pairs: Vec<(String, String)> = spec
                    .members
                    .iter()
                    .map(|member| (member.member_id.clone(), member.name.clone()))
                    .collect();
                let parsed = bot_team::mentions::parse_mentions(&text, &member_pairs);
                let mentioned: Vec<usize> = spec
                    .members
                    .iter()
                    .enumerate()
                    .filter(|(_, member)| parsed.ids.contains(&member.member_id))
                    .map(|(index, _)| index)
                    .collect();
                let chosen: Vec<usize> = if !mentioned.is_empty() {
                    mentioned.clone()
                } else if parsed.everyone || spec.info.responder_policy.trim() == "all" {
                    (0..spec.members.len()).collect()
                } else {
                    spec.members
                        .iter()
                        .position(|member| member.is_admin)
                        .or(Some(0))
                        .into_iter()
                        .collect()
                };
                for index in chosen {
                    let member = &spec.members[index];
                    let internal = member_session_key(&key, &member.member_id);
                    let Some(ctx) = sessions.get(&internal) else {
                        let _ = events.send(BackendEvent::Session {
                            key: key.clone(),
                            event: SessionEvent::Error {
                                member: Some(member.name.clone()),
                                message: format!("{} 的会话尚未建立", member.name),
                            },
                        });
                        continue;
                    };
                    let mode = if member_talked.contains(&internal) {
                        "continue"
                    } else {
                        "start"
                    };
                    member_talked.insert(internal.clone());
                    let addressed_by_user = mentioned.contains(&index) && !member.is_admin;
                    let prompt = bot_team::prompt::build_group_prompt(
                        &bot_team::prompt::GroupPromptInput {
                            group_name: spec.info.group_name.clone(),
                            team_memory: crate::data::team_memory(),
                            goal_prompt: crate::data::group_goal_prompt(&spec.info.group_id),
                            member_name: member.name.clone(),
                            member_role: member.role.clone(),
                            member_is_admin: member.is_admin,
                            addressed_by_user,
                            is_chief: member.is_chief,
                            member_names: spec.info.member_names.clone(),
                            instructions: spec.info.instructions.clone(),
                            memory: crate::data::group_memory_context(
                                &spec.info.group_id,
                                &spec.info.memory,
                            ),
                            team_status: Vec::new(),
                            mode: mode.to_string(),
                            history: Vec::new(),
                            text: text.clone(),
                        },
                    );
                                        let Some(model) = ctx.model.clone() else {
                        let _ = events.send(BackendEvent::Session {
                            key: key.clone(),
                            event: SessionEvent::Error {
                                member: Some(member.name.clone()),
                                message: format!("{} 缺少模型选择", member.name),
                            },
                        });
                        continue;
                    };
                    if let SessionEngine::Rpi(session) = &ctx.engine {
                        let images =
                            match crate::rpi_engine::images_from_attachments(&attachments) {
                                Ok(images) => images,
                                Err(message) => {
                                    let _ = events.send(BackendEvent::Session {
                                        key: key.clone(),
                                        event: SessionEvent::Error {
                                            member: Some(member.name.clone()),
                                            message: format!(
                                                "{}：{message}",
                                                member.name
                                            ),
                                        },
                                    });
                                    continue;
                                }
                            };
                        let session = Arc::clone(session);
                        let prompt = prompt.clone();
                        tokio::spawn(async move {
                            session.send(prompt, images).await;
                        });
                        continue;
                    }
                    if let SessionEngine::Acp(_) = &ctx.engine
                        && !attachments.is_empty()
                    {
                        let _ = events.send(BackendEvent::Session {
                            key: key.clone(),
                            event: SessionEvent::Error {
                                member: Some(member.name.clone()),
                                message: format!("{} 的本地引擎暂不支持附件", member.name),
                            },
                        });
                        continue;
                    }
                    // 附件按成员会话各传一份（v4 上传引用绑定 session）。
                    let attachment_refs = if attachments.is_empty() {
                        Vec::new()
                    } else {
                        match upload_attachments(
                            &control,
                            &ctx.target,
                            &ctx.session_id,
                            attachments.clone(),
                        )
                        .await
                        {
                            Ok(refs) => refs,
                            Err(message) => {
                                let _ = events.send(BackendEvent::Session {
                                    key: key.clone(),
                                    event: SessionEvent::Error {
                                        member: Some(member.name.clone()),
                                        message: format!(
                                            "{} 附件上传失败：{message}",
                                            member.name
                                        ),
                                    },
                                });
                                continue;
                            }
                        }
                    };
                    ctx.turn_started
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    let mut payload = serde_json::json!({
                        "text": prompt,
                        "modelSelection": model,
                    });
                    if !attachment_refs.is_empty() {
                        payload["attachments"] = serde_json::Value::Array(attachment_refs);
                    }
                    let envelope = v4_envelope(
                        &client_id,
                        Some(&ctx.session_id),
                        CommandType::SendText,
                        payload,
                    );
                    match control
                        .send_conversation_command_v4(&ctx.target, &envelope)
                        .await
                    {
                        Ok(ack) if ack.status.is_success() => {}
                        Ok(ack) => {
                            ctx.turn_started
                                .store(false, std::sync::atomic::Ordering::Relaxed);
                            let _ = events.send(BackendEvent::Session {
                                key: key.clone(),
                                event: SessionEvent::Error {
                                    member: Some(member.name.clone()),
                                    message: format!(
                                        "{} 发送未受理（{}）：{}",
                                        member.name,
                                        ack.status.as_str(),
                                        ack.message.unwrap_or_default()
                                    ),
                                },
                            });
                        }
                        Err(error) => {
                            ctx.turn_started
                                .store(false, std::sync::atomic::Ordering::Relaxed);
                            let _ = events.send(BackendEvent::Session {
                                key: key.clone(),
                                event: SessionEvent::Error {
                                    member: Some(member.name.clone()),
                                    message: format!("{} 发送失败：{error}", member.name),
                                },
                            });
                        }
                    }
                    continue;
                
                }
            
}
            Command::ResetSession { key } => {
                sessions.remove(&key);
                active_internal.remove(&key);
                if let Some(handle) = v4_forwarders.remove(&key) {
                    handle.abort();
                }
            }
            Command::Stop { key } => {
                // 停止该 UI key 下所有成员会话的进行中回合（v4 Stop 信封）。
                let millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis())
                    .unwrap_or_default();
                let targets: Vec<String> = sessions
                    .iter()
                    .filter(|(internal, ctx)| {
                        ctx.member.is_some()
                            && internal.starts_with(&format!("{key}\u{1}"))
                    })
                    .map(|(internal, _)| internal.clone())
                    .collect();
                let targets = if targets.is_empty() {
                    vec![active_internal
                        .get(&key)
                        .cloned()
                        .unwrap_or_else(|| key.clone())]
                } else {
                    targets
                };
                for internal in targets {
                    let Some(ctx) = sessions.get(&internal) else {
                        continue;
                    };
                    if let SessionEngine::Rpi(session) = &ctx.engine {
                        let session = Arc::clone(session);
                        tokio::spawn(async move {
                            session.abort().await;
                        });
                        continue;
                    }
                    if let SessionEngine::Acp(handle) = &ctx.engine {
                        handle.stop();
                        continue;
                    }
                    let seq = stop_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let envelope = ConversationCommandEnvelope {
                        ttft: None,
                        command_id: format!("bot-gpui-stop-{millis}-{seq}"),
                        client_id: client_id.clone(),
                        session_id: CommandSessionId(Some(ctx.session_id.clone())),
                        base_revision: None,
                        base_log_epoch: None,
                        command_type: CommandType::Stop,
                        payload: serde_json::json!({}),
                        issued_at: serde_json::Number::from(millis as i64),
                    };
                    if let Err(error) = control.send_conversation_command_v4(&ctx.target, &envelope).await {
                        let _ = events.send(BackendEvent::Session {
                            key: key.clone(),
                            event: SessionEvent::Error {
                                member: ctx.member.clone(),
                                message: format!("停止失败：{error}"),
                            },
                        });
                    }
                }
            }
            Command::StopMember { key, member_id } => {
                let internal = member_session_key(&key, &member_id);
                let Some(ctx) = sessions.get(&internal) else {
                    continue;
                };
                if let SessionEngine::Rpi(session) = &ctx.engine {
                    let session = Arc::clone(session);
                    tokio::spawn(async move {
                        session.abort().await;
                    });
                    continue;
                }
                if let SessionEngine::Acp(handle) = &ctx.engine {
                    handle.stop();
                    continue;
                }
                let millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis())
                    .unwrap_or_default();
                let seq = stop_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let envelope = ConversationCommandEnvelope {
                    ttft: None,
                    command_id: format!("bot-gpui-stop-member-{millis}-{seq}"),
                    client_id: client_id.clone(),
                    session_id: CommandSessionId(Some(ctx.session_id.clone())),
                    base_revision: None,
                    base_log_epoch: None,
                    command_type: CommandType::Stop,
                    payload: serde_json::json!({}),
                    issued_at: serde_json::Number::from(millis as i64),
                };
                if let Err(error) = control
                    .send_conversation_command_v4(&ctx.target, &envelope)
                    .await
                {
                    let _ = events.send(BackendEvent::Session {
                        key: key.clone(),
                        event: SessionEvent::Error {
                            member: ctx.member.clone(),
                            message: format!("停止成员失败：{error}"),
                        },
                    });
                }
            }
            Command::MemoryReview { snapshot } => {
                let control = control.clone();
                let client_id = client_id.clone();
                let epoch = note_review_epoch(&snapshot.group_id);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(5_000)).await;
                    if !is_current_review_epoch(&snapshot.group_id, epoch)
                        || !crate::memory_review::enabled()
                    {
                        return;
                    }
                    let _permit = review_lock().await;
                    if !is_current_review_epoch(&snapshot.group_id, epoch)
                        || !crate::memory_review::enabled()
                    {
                        return;
                    }
                    if let Err(error) =
                        run_memory_review(&control, &client_id, &snapshot).await
                    {
                        eprintln!(
                            "[memory-review] group={} failed: {error}",
                            snapshot.group_id
                        );
                    }
                });
            }
            Command::ContinueTask {
                key,
                member_id,
                text,
            } => {
                let Some(spec) = groups.get(&key).cloned() else {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: "频道会话尚未建立".to_string(),
                        },
                    });
                    continue;
                };
                let Some(member) = spec
                    .members
                    .iter()
                    .find(|member| member.member_id == member_id)
                    .cloned()
                else {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: "任务负责人不在频道成员里".to_string(),
                        },
                    });
                    continue;
                };
                let internal = member_session_key(&key, &member.member_id);
                let Some(ctx) = sessions.get(&internal) else {
                    let _ = events.send(BackendEvent::Session {
                        key: key.clone(),
                        event: SessionEvent::Error {
                            member: Some(member.name.clone()),
                            message: format!("{} 的会话尚未建立", member.name),
                        },
                    });
                    continue;
                };
                let prompt = bot_team::prompt::build_group_prompt(
                    &bot_team::prompt::GroupPromptInput {
                        group_name: spec.info.group_name.clone(),
                        team_memory: crate::data::team_memory(),
                        goal_prompt: crate::data::group_goal_prompt(&spec.info.group_id),
                        member_name: member.name.clone(),
                        member_role: member.role.clone(),
                        member_is_admin: member.is_admin,
                        addressed_by_user: false,
                        is_chief: member.is_chief,
                        member_names: spec.info.member_names.clone(),
                        instructions: spec.info.instructions.clone(),
                        memory: spec.info.memory.clone(),
                        team_status: Vec::new(),
                        mode: "continue".to_string(),
                        history: Vec::new(),
                        text: text.clone(),
                    },
                );
                let Some(model) = ctx.model.clone() else {
                    let _ = events.send(BackendEvent::Session {
                        key: key.clone(),
                        event: SessionEvent::Error {
                            member: Some(member.name.clone()),
                            message: format!("{} 缺少模型选择", member.name),
                        },
                    });
                    continue;
                };
                ctx.turn_started
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                let envelope = v4_envelope(
                    &client_id,
                    Some(&ctx.session_id),
                    CommandType::SendText,
                    serde_json::json!({
                        "text": prompt,
                        "modelSelection": model,
                    }),
                );
                match control
                    .send_conversation_command_v4(&ctx.target, &envelope)
                    .await
                {
                    Ok(ack) if ack.status.is_success() => {}
                    Ok(ack) => {
                        ctx.turn_started
                            .store(false, std::sync::atomic::Ordering::Relaxed);
                        let _ = events.send(BackendEvent::Session {
                            key: key.clone(),
                            event: SessionEvent::Error {
                                member: Some(member.name.clone()),
                                message: format!(
                                    "{} 发送未受理（{}）：{}",
                                    member.name,
                                    ack.status.as_str(),
                                    ack.message.unwrap_or_default()
                                ),
                            },
                        });
                    }
                    Err(error) => {
                        ctx.turn_started
                            .store(false, std::sync::atomic::Ordering::Relaxed);
                        let _ = events.send(BackendEvent::Session {
                            key: key.clone(),
                            event: SessionEvent::Error {
                                member: Some(member.name.clone()),
                                message: format!("{} 发送失败：{error}", member.name),
                            },
                        });
                    }
                }
            }
            Command::PluginsOverview {
                key,
                bot_id,
                workspace,
                identity,
            } => {
                let target = AgentWorkspaceTarget::new(workspace).with_identity(identity);
                let result = control
                    .get_plugins_overview(&target, None)
                    .await
                    .map_err(|error| error.to_string());
                let _ = events.send(BackendEvent::PluginsOverview {
                    key,
                    bot_id,
                    result,
                });
            }
            Command::PluginInstall {
                key,
                bot_id,
                workspace,
                identity,
                plugin_name,
                marketplace,
            } => {
                let target = AgentWorkspaceTarget::new(workspace).with_identity(identity);
                let outcome = control
                    .install_plugin(&target, &plugin_name, &marketplace, None, None, None)
                    .await;
                let ok = outcome.is_ok();
                let label = format!("安装 {plugin_name}");
                let result = outcome
                    .map(|installed| format!("已安装 {} 个插件", installed.installed_plugins.len()))
                    .map_err(|error| error.to_string());
                let _ = events.send(BackendEvent::PluginAction {
                    key: key.clone(),
                    bot_id: bot_id.clone(),
                    label,
                    result,
                });
                if ok {
                    let result = control
                        .get_plugins_overview(&target, None)
                        .await
                        .map_err(|error| error.to_string());
                    let _ = events.send(BackendEvent::PluginsOverview {
                        key,
                        bot_id,
                        result,
                    });
                }
            }
            Command::PluginUninstall {
                key,
                bot_id,
                workspace,
                identity,
                plugin_id,
            } => {
                let target = AgentWorkspaceTarget::new(workspace).with_identity(identity);
                let outcome = control
                    .uninstall_plugin(&target, Some(&plugin_id), None, None, None)
                    .await;
                let ok = outcome.is_ok();
                let label = format!("卸载 {plugin_id}");
                let result = outcome
                    .map(|_| "已卸载插件".to_string())
                    .map_err(|error| error.to_string());
                let _ = events.send(BackendEvent::PluginAction {
                    key: key.clone(),
                    bot_id: bot_id.clone(),
                    label,
                    result,
                });
                if ok {
                    let result = control
                        .get_plugins_overview(&target, None)
                        .await
                        .map_err(|error| error.to_string());
                    let _ = events.send(BackendEvent::PluginsOverview {
                        key,
                        bot_id,
                        result,
                    });
                }
            }
            Command::PluginSetEnabled {
                key,
                bot_id,
                workspace,
                identity,
                plugin_id,
                enabled,
            } => {
                let target = AgentWorkspaceTarget::new(workspace).with_identity(identity);
                let outcome = control
                    .set_plugin_enabled(&target, &plugin_id, enabled, None, None)
                    .await;
                let ok = outcome.is_ok();
                let label = if enabled { "启用插件" } else { "停用插件" };
                let result = outcome
                    .map(|_| {
                        if enabled {
                            "已启用插件".to_string()
                        } else {
                            "已停用插件".to_string()
                        }
                    })
                    .map_err(|error| error.to_string());
                let _ = events.send(BackendEvent::PluginAction {
                    key: key.clone(),
                    bot_id: bot_id.clone(),
                    label: label.to_string(),
                    result,
                });
                if ok {
                    let result = control
                        .get_plugins_overview(&target, None)
                        .await
                        .map_err(|error| error.to_string());
                    let _ = events.send(BackendEvent::PluginsOverview {
                        key,
                        bot_id,
                        result,
                    });
                }
            }
            Command::PluginUpdate {
                key,
                bot_id,
                workspace,
                identity,
                plugin_id,
            } => {
                let target = AgentWorkspaceTarget::new(workspace).with_identity(identity);
                let outcome = control
                    .update_plugin(&target, plugin_id.as_deref(), None)
                    .await;
                let ok = outcome.is_ok();
                let result = outcome
                    .map(|updated| format!("已更新 {} 个插件", updated.installed_plugins.len()))
                    .map_err(|error| error.to_string());
                let _ = events.send(BackendEvent::PluginAction {
                    key: key.clone(),
                    bot_id: bot_id.clone(),
                    label: "更新插件".to_string(),
                    result,
                });
                if ok {
                    let result = control
                        .get_plugins_overview(&target, None)
                        .await
                        .map_err(|error| error.to_string());
                    let _ = events.send(BackendEvent::PluginsOverview {
                        key,
                        bot_id,
                        result,
                    });
                }
            }
            Command::PluginMarketplaceUpdate {
                key,
                bot_id,
                workspace,
                identity,
            } => {
                let target = AgentWorkspaceTarget::new(workspace).with_identity(identity);
                let outcome = control.update_plugin_marketplace(&target, None, None).await;
                let ok = outcome.is_ok();
                let result = outcome
                    .map(|_| "市场索引已刷新".to_string())
                    .map_err(|error| error.to_string());
                let _ = events.send(BackendEvent::PluginAction {
                    key: key.clone(),
                    bot_id: bot_id.clone(),
                    label: "刷新市场".to_string(),
                    result,
                });
                if ok {
                    let result = control
                        .get_plugins_overview(&target, None)
                        .await
                        .map_err(|error| error.to_string());
                    let _ = events.send(BackendEvent::PluginsOverview {
                        key,
                        bot_id,
                        result,
                    });
                }
            }
            Command::PluginConfigLoad {
                key,
                bot_id,
                workspace,
                identity,
                plugin_id,
            } => {
                let target = AgentWorkspaceTarget::new(workspace).with_identity(identity);
                let result = match control.list_plugins(&target, None).await {
                    Ok(list) => list
                        .plugins
                        .into_iter()
                        .find(|plugin| plugin.id == plugin_id)
                        .ok_or_else(|| format!("插件未安装或不可见：{plugin_id}")),
                    Err(error) => Err(error.to_string()),
                };
                let _ = events.send(BackendEvent::PluginConfig {
                    key,
                    bot_id,
                    plugin_id,
                    result: result.map(Box::new),
                });
            }
            Command::PluginConfigure {
                key,
                bot_id,
                workspace,
                identity,
                plugin_id,
                options,
                clear_keys,
            } => {
                let target = AgentWorkspaceTarget::new(workspace).with_identity(identity);
                let clear = if clear_keys.is_empty() {
                    None
                } else {
                    Some(clear_keys)
                };
                let result = control
                    .configure_plugin(&target, &plugin_id, options, clear, None, None)
                    .await
                    .map(|_| "插件配置已保存".to_string())
                    .map_err(|error| error.to_string());
                let _ = events.send(BackendEvent::PluginAction {
                    key,
                    bot_id,
                    label: "保存插件配置".to_string(),
                    result,
                });
            }
            Command::PluginResetConfig {
                key,
                bot_id,
                workspace,
                identity,
                plugin_id,
            } => {
                let target = AgentWorkspaceTarget::new(workspace).with_identity(identity);
                let result = control
                    .reset_plugin_config(&target, &plugin_id, None)
                    .await
                    .map(|_| "插件配置已清空".to_string())
                    .map_err(|error| error.to_string());
                let _ = events.send(BackendEvent::PluginAction {
                    key,
                    bot_id,
                    label: "清空插件配置".to_string(),
                    result,
                });
            }
            Command::LoadAttachmentImage {
                workspace,
                identity,
                session_id,
                attachment_ref,
                row_id,
                entity_id,
                attachment_index,
            } => {
                let target = AgentWorkspaceTarget::new(workspace).with_identity(identity);
                let outcome: Result<String, String> = async {
                    use base64::Engine as _;
                    let mut offset = 0u64;
                    let mut bytes: Vec<u8> = Vec::new();
                    let mut media_type = String::new();
                    loop {
                        let result = control
                            .conversation_attachment_read_v4(
                                &target,
                                agent_protocol::v4_queries::ConversationAttachmentReadParams {
                                    session_id: session_id.clone(),
                                    attachment_ref: attachment_ref.clone(),
                                    target: agent_protocol::v4_queries::ConversationRowTarget {
                                        row_id,
                                        entity_id: entity_id.clone(),
                                    },
                                    attachment_index,
                                    offset,
                                    limit: 384 * 1024,
                                },
                                agent_process::RuntimePolicy::StartIfNeeded,
                            )
                            .await
                            .map_err(|error| error.to_string())?;
                        if media_type.is_empty() {
                            media_type = result.media_type.clone();
                        }
                        let chunk = base64::engine::general_purpose::STANDARD
                            .decode(result.data_base64.as_bytes())
                            .map_err(|error| format!("附件 base64 解码失败：{error}"))?;
                        bytes.extend_from_slice(&chunk);
                        if bytes.len() > 20 * 1024 * 1024 {
                            return Err("附件超过 20MiB，已停止加载".to_string());
                        }
                        match result.next_offset {
                            Some(next) if next > offset => offset = next,
                            _ => break,
                        }
                    }
                    let extension = match media_type.as_str() {
                        "image/png" => "png",
                        "image/jpeg" | "image/jpg" => "jpg",
                        "image/webp" => "webp",
                        "image/gif" => "gif",
                        "image/bmp" => "bmp",
                        _ => "bin",
                    };
                    let dir = std::env::temp_dir().join("mode-bot-gpui-attachments");
                    std::fs::create_dir_all(&dir)
                        .map_err(|error| format!("创建预览目录失败：{error}"))?;
                    let name = sha256_prefixed(&bytes)
                        .trim_start_matches("sha256:")
                        .chars()
                        .take(16)
                        .collect::<String>();
                    let path = dir.join(format!("history-{name}.{extension}"));
                    std::fs::write(&path, &bytes)
                        .map_err(|error| format!("写入预览图失败：{error}"))?;
                    Ok(path.to_string_lossy().into_owned())
                }
                .await;
                let (path, error) = match outcome {
                    Ok(path) => (Some(path), None),
                    Err(error) => (None, Some(error)),
                };
                let _ = events.send(BackendEvent::AttachmentImage {
                    attachment_ref,
                    path,
                    error,
                });
            }
            Command::Send {
                key,
                text,
                attachments,
            } => {

                // 新会话/切换会话后路由到当前活跃会话（internal），而非 UI key 本身。
                let ctx_key =
                    active_internal.get(&key).cloned().unwrap_or_else(|| key.clone());
                let Some(ctx) = sessions.get(&ctx_key) else {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: "会话尚未建立".to_string(),
                        },
                    });
                    continue;
                };
                if let SessionEngine::Rpi(session) = &ctx.engine {
                    let images = match crate::rpi_engine::images_from_attachments(&attachments) {
                        Ok(images) => images,
                        Err(message) => {
                            let _ = events.send(BackendEvent::Session {
                                key,
                                event: SessionEvent::Error {
                                    member: None,
                                    message,
                                },
                            });
                            continue;
                        }
                    };
                    let session = Arc::clone(session);
                    let text = text.clone();
                    tokio::spawn(async move {
                        session.send(text, images).await;
                    });
                    continue;
                }
                                if let SessionEngine::Acp(handle) = &ctx.engine {
                    if !attachments.is_empty() {
                        let _ = events.send(BackendEvent::Session {
                            key,
                            event: SessionEvent::Error {
                                member: None,
                                message: "智能体引擎暂不支持附件".to_string(),
                            },
                        });
                        continue;
                    }
                    let handle = handle.clone();
                    if !handle.prompt(text) {
                        // 子进程已退出（例如 session/load 失败）：丢掉死会话，
                        // 让下次 Ensure/新会话重建，而不是静默吞掉这条消息。
                        sessions.remove(&ctx_key);
                        let _ = events.send(BackendEvent::Session {
                            key,
                            event: SessionEvent::Error {
                                member: None,
                                message: "会话已断开，请重新发送（后端会重建会话）".to_string(),
                            },
                        });
                    }
                    continue;
                }
                let attachment_refs = if attachments.is_empty() {
                    Vec::new()
                } else {
                    match upload_attachments(
                        &control,
                        &ctx.target,
                        &ctx.session_id,
                        attachments,
                    )
                    .await
                    {
                        Ok(refs) => refs,
                        Err(message) => {
                            let _ = events.send(BackendEvent::Session {
                                key,
                                event: SessionEvent::Error {
                                    member: None,
                                    message: format!("附件上传失败：{message}"),
                                },
                            });
                            continue;
                        }
                    }
                };
                let Some(model) = ctx.model.clone() else {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: "v4 发送缺少模型选择".to_string(),
                        },
                    });
                    continue;
                };
                // 记下「本会话已有进行中的回合」：即使首个 token 之前就被 Stop，
                // 取消收尾也能凭这个标记补发 Done（否则 UI 停在等待回复）。
                ctx.turn_started
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                let envelope = v4_envelope(
                    &client_id,
                    Some(&ctx.session_id),
                    CommandType::SendText,
                    {
                        let mut payload = serde_json::json!({
                            "text": text,
                            "modelSelection": model,
                        });
                        if !attachment_refs.is_empty() {
                            payload["attachments"] =
                                serde_json::Value::Array(attachment_refs);
                        }
                        payload
                    },
                );
                match control
                    .send_conversation_command_v4(&ctx.target, &envelope)
                    .await
                {
                    Ok(ack) if ack.status.is_success() => {}
                    Ok(ack) => {
                        ctx.turn_started
                            .store(false, std::sync::atomic::Ordering::Relaxed);
                        let _ = events.send(BackendEvent::Session {
                            key,
                            event: SessionEvent::Error {
                                member: None,
                                message: format!(
                                    "v4 发送未受理（{}）：{}",
                                    ack.status.as_str(),
                                    ack.message.unwrap_or_default()
                                ),
                            },
                        });
                    }
                    Err(error) => {
                        ctx.turn_started
                            .store(false, std::sync::atomic::Ordering::Relaxed);
                        let _ = events.send(BackendEvent::Session {
                            key,
                            event: SessionEvent::Error {
                                member: None,
                                message: format!("v4 发送失败：{error}"),
                            },
                        });
                    }
                }
                continue;
            
}
        }
    }
    Ok(())
}

/// 频道成员会话在 sessions 表里的内部键（UI 只看 group_key）。
pub fn member_session_key(group_key: &str, member_id: &str) -> String {
    format!("{group_key}\u{1}{member_id}")
}

// ── 群记忆自动回顾 ─────────────────────────────────────────────────────────

fn review_epochs() -> &'static Mutex<HashMap<String, u64>> {
    static EPOCHS: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    EPOCHS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn note_review_epoch(group_id: &str) -> u64 {
    let mut epochs = review_epochs().lock().unwrap();
    let entry = epochs.entry(group_id.to_string()).or_insert(0);
    *entry = entry.saturating_add(1);
    *entry
}

fn is_current_review_epoch(group_id: &str, epoch: u64) -> bool {
    review_epochs()
        .lock()
        .unwrap()
        .get(group_id)
        .copied()
        .unwrap_or(0)
        == epoch
}

async fn review_lock() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

fn review_state_path(group_id: &str) -> Option<PathBuf> {
    if group_id.is_empty()
        || !group_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return None;
    }
    Some(
        crate::data::data_root()
            .join("v2")
            .join("groups")
            .join(group_id)
            .join("memory-review.json"),
    )
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewSessionState {
    #[serde(default)]
    session_id: Option<String>,
}

fn read_review_session(path: &Path) -> ReviewSessionState {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_review_session(path: &Path, session_id: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = atomic_file::write_json_atomic(
        path,
        &serde_json::json!({ "sessionId": session_id }),
        &atomic_file::WriteOptions::default(),
    );
}

fn collect_review_row(
    row: &ConversationRow,
    preexisting: &HashSet<String>,
    rows: &mut HashMap<String, (u64, String)>,
) -> bool {
    let ConversationRow::AssistantText(row) = row else {
        return false;
    };
    let id = row.base.row_id.to_string();
    if preexisting.contains(&id) {
        return false;
    }
    rows.insert(
        id,
        (
            row.base.created_at.as_u64().unwrap_or(0),
            row.text.clone(),
        ),
    );
    matches!(
        row.state,
        agent_protocol::v4_rows::AssistantTextState::Complete
            | agent_protocol::v4_rows::AssistantTextState::Interrupted
            | agent_protocol::v4_rows::AssistantTextState::Failed
    )
}

/// 跑一次回顾：复用隐藏会话（失败重建）→ 发整理 prompt → 收集回复 → 解析并落盘。
async fn run_memory_review(
    control: &AgentControlClient,
    client_id: &str,
    snapshot: &crate::memory_review::ReviewSnapshot,
) -> Result<(), String> {
    use agent_protocol::v4_delta::ConversationDelta;
    use agent_protocol::v4_snapshot::SessionPhase;
    use agent_protocol::v4_transport::ConversationFramePayload;

    if !crate::memory_review::enabled() {
        return Ok(());
    }
    let Some(memory_root) = crate::data::group_memory_root(&snapshot.group_id) else {
        return Err("群记忆存储不可用".to_string());
    };
    std::fs::create_dir_all(&memory_root)
        .map_err(|error| format!("创建群记忆目录失败：{error}"))?;

    let target = AgentWorkspaceTarget::new(snapshot.workspace.clone())
        .with_identity(snapshot.identity.clone());
    let state_path = review_state_path(&snapshot.group_id);
    let stored = state_path
        .as_deref()
        .map(read_review_session)
        .unwrap_or_default();

    let mut frames = None;
    let mut session_id = None;
    if let Some(resume) = stored.session_id.as_deref()
        && let Ok(subscription) = v4_subscribe(control, &target, resume).await
    {
        frames = Some(subscription);
        session_id = Some(resume.to_string());
    }
    let (session_id, mut frames) = match (session_id, frames) {
        (Some(session_id), Some(frames)) => (session_id, frames),
        _ => {
            let created = v4_create_session(control, &target, client_id).await?;
            let frames = v4_subscribe(control, &target, &created).await?;
            if let Some(path) = state_path.as_deref() {
                write_review_session(path, &created);
            }
            (created, frames)
        }
    };

    // 先吃掉初始快照：旧回合的 assistantText 行不能算进本次输出。
    let mut preexisting: HashSet<String> = HashSet::new();
    let snapshot_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = snapshot_deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let Ok(Some(frame)) = tokio::time::timeout(remaining, frames.recv()).await else {
            break;
        };
        let agent_protocol::v4_transport::ConversationWireFrame::Complete(complete) = frame
        else {
            continue;
        };
        if let ConversationFramePayload::Snapshot(snapshot) = complete.frame.payload {
            for row in &snapshot.snapshot.rows.window {
                if let ConversationRow::AssistantText(row) = row {
                    preexisting.insert(row.base.row_id.to_string());
                }
            }
            break;
        }
    }

    let prompt = crate::memory_review::build_prompt(snapshot, &memory_root);
    let payload = serde_json::json!({
        "text": prompt,
        "modelSelection": snapshot.model,
    });
    let envelope = v4_envelope(
        client_id,
        Some(&session_id),
        CommandType::SendText,
        payload,
    );
    let ack = control
        .send_conversation_command_v4(&target, &envelope)
        .await
        .map_err(|error| format!("回顾发送失败：{error}"))?;
    if !ack.status.is_success() {
        return Err(format!(
            "回顾发送未受理（{}）：{}",
            ack.status.as_str(),
            ack.message.unwrap_or_default()
        ));
    }

    let mut rows: HashMap<String, (u64, String)> = HashMap::new();
    let mut finished = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    while !finished {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("回顾超时（90s）".to_string());
        }
        let Ok(Some(frame)) = tokio::time::timeout(remaining, frames.recv()).await else {
            break;
        };
        let agent_protocol::v4_transport::ConversationWireFrame::Complete(complete) = frame
        else {
            continue;
        };
        match complete.frame.payload {
            ConversationFramePayload::Snapshot(snapshot) => {
                for row in &snapshot.snapshot.rows.window {
                    finished |= collect_review_row(row, &preexisting, &mut rows);
                }
                if matches!(
                    snapshot.snapshot.control.phase,
                    SessionPhase::CompletedSuccess | SessionPhase::CompletedInterrupted
                ) {
                    finished = true;
                }
            }
            ConversationFramePayload::Deltas(deltas) => {
                for delta in deltas.deltas {
                    match delta {
                        ConversationDelta::RowAppended(delta) => {
                            finished |=
                                collect_review_row(&delta.row, &preexisting, &mut rows);
                        }
                        ConversationDelta::RowUpserted(delta) => {
                            finished |=
                                collect_review_row(&delta.row, &preexisting, &mut rows);
                        }
                        ConversationDelta::RowDelta(delta) => {
                            if delta.path
                                == agent_protocol::v4_delta::StreamablePath::Text
                                && let Some((_, text)) =
                                    rows.get_mut(&delta.row_id.to_string())
                            {
                                text.push_str(&delta.append);
                            }
                        }
                        ConversationDelta::StateUpdated(delta) => {
                            if let Some(control) = &delta.patch.control
                                && matches!(
                                    control.phase,
                                    SessionPhase::CompletedSuccess
                                        | SessionPhase::CompletedInterrupted
                                )
                            {
                                finished = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    let mut items: Vec<(u64, String)> = rows.into_values().collect();
    items.sort_by_key(|(at, _)| *at);
    let answer = items
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join("\n");
    if answer.trim().is_empty() {
        return Ok(());
    }
    let operations = crate::memory_review::parse_operations(&answer)?;
    if operations.is_empty() {
        return Ok(());
    }
    let applied = crate::memory_review::apply_operations(
        &memory_root,
        &snapshot.identity,
        &operations,
    )?;
    if applied > 0 {
        eprintln!(
            "[memory-review] group={} applied {applied} operation(s)",
            snapshot.group_id
        );
    }
    Ok(())
}

/// 跨成员按时间归并，保留最近 80 条。
fn merge_history(mut entries: Vec<HistoryEntry>) -> Vec<HistoryEntry> {
    entries.sort_by_key(|entry| entry.at);
    let keep = 80;
    if entries.len() > keep {
        entries.drain(..entries.len() - keep);
    }
    entries
}

/// 反向请求处理器：requestUserInput 转问题卡后持有；permission 直接持有。
/// 答案都走 v4 `resolveInteraction`（CLI 对 legacy 请求与 v4 应答竞速，应答到达后
/// CLI 自己 abort legacy 请求）。
fn pilot_interaction_handler(
    session_map: Arc<Mutex<HashMap<String, SessionRef>>>,
    request_map: Arc<Mutex<HashMap<String, String>>>,
    events: UnboundedSender<BackendEvent>,
    automation_service: Arc<AutomationService>,
    automation_repo: Arc<AutomationRepo>,
) -> RequestHandler {
    Arc::new(move |request, _reply| {
        let method = request
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if std::env::var("BOT_PILOT_DEBUG").is_ok() {
            let params = request
                .get("params")
                .map(serde_json::Value::to_string)
                .unwrap_or_default();
            eprintln!(
                "[agent-request] method={method} params={}",
                params.chars().take(1200).collect::<String>()
            );
        }
        if let Some(reply) =
            crate::automation_bridge::handle_request(&automation_service, &automation_repo, request)
        {
            return reply;
        }
        if method == INTERACTION_REQUEST_USER_INPUT_METHOD {
            let params = request.get("params").cloned().unwrap_or(serde_json::Value::Null);
            let session_id = params
                .get("sessionId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let request_id = params
                .get("requestId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let found = session_map
                .lock()
                .ok()
                .and_then(|map| map.get(&session_id).cloned());
            let mut question = params
                .get("prompt")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let mut options = Vec::new();
            if let Some(questions) = params.get("questions").and_then(serde_json::Value::as_array)
                && let Some(first) = questions.first() {
                    question = first
                        .get("question")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&question)
                        .to_string();
                    if let Some(items) = first.get("options").and_then(serde_json::Value::as_array)
                    {
                        for item in items {
                            let value = item
                                .get("value")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default();
                            let label = item
                                .get("label")
                                .and_then(serde_json::Value::as_str)
                                .filter(|label| !label.trim().is_empty())
                                .unwrap_or(value);
                            if !value.is_empty() {
                                options.push((value.to_string(), label.to_string()));
                            }
                        }
                    }
                }
            if let Some(found) = found {
                if let Ok(mut map) = request_map.lock() {
                    map.insert(request_id.clone(), found.internal_key.clone());
                }
                let member = found.member.clone();
                let _ = events.send(BackendEvent::Session {
                    key: found.event_key,
                    event: SessionEvent::Question(QuestionCard {
                        request_id,
                        question,
                        options,
                        member,
                    }),
                });
            }
            return RequestReply::Deferred;
        }
        if method == INTERACTION_REQUEST_PERMISSION_METHOD {
            return RequestReply::Deferred;
        }
        RequestReply::NotHandled
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_runtime_detects_install_layout() {
        let dir = std::env::temp_dir().join(format!("bot-gpui-runtime-{}", uuid::Uuid::new_v4()));
        let node_name = if cfg!(windows) { "node.exe" } else { "node" };
        std::fs::create_dir_all(dir.join("node")).expect("node dir");
        std::fs::create_dir_all(dir.join("config").join("provider")).expect("config dir");
        std::fs::create_dir_all(dir.join("agent").join("glm")).expect("agent dir");
        std::fs::write(dir.join("node").join(node_name), b"").expect("node");
        std::fs::write(
            dir.join("config").join("provider").join("mode-builtin.json"),
            b"{}",
        )
        .expect("provider");
        std::fs::write(dir.join("agent").join("glm").join("mode.cjs"), b"").expect("agent");

        let runtime = packaged_runtime(Some(&dir.join("bot-gpui-pilot.exe")));
        assert_eq!(runtime.resources_dir.as_deref(), Some(dir.as_path()));
        assert_eq!(runtime.node_candidates.len(), 1);
        assert!(runtime.builtin_provider.is_some());

        let empty = std::env::temp_dir().join(format!("bot-gpui-empty-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&empty).expect("empty dir");
        let dev = packaged_runtime(Some(&empty.join("bot-gpui-pilot.exe")));
        assert!(dev.resources_dir.is_some());
        assert!(dev.node_candidates.is_empty());
        assert!(dev.builtin_provider.is_none());

        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(empty);
    }
}

#[cfg(test)]
mod reasoning_tests {
    use super::*;
    use agent_protocol::v4_rows::ConversationRow;

    fn row(text: &str, state: &str) -> ConversationRow {
        serde_json::from_value(serde_json::json!({
            "kind": "reasoning",
            "rowId": 1,
            "turnId": "t1",
            "createdAt": 1,
            "createdAtSeq": 1,
            "text": text,
            "state": state
        }))
        .expect("reasoning row")
    }

    #[test]
    fn reasoning_rows_emit_reasoning_events_not_text_deltas() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut rendered = HashMap::new();
        let mut completed = std::collections::HashSet::new();
        let mut reasoning_rendered = HashMap::new();
        let mut reasoning_rows = std::collections::HashSet::new();
        let mut turn_active = false;
        let started = std::sync::atomic::AtomicBool::new(true);

        apply_v4_row(
            &row("思考", "streaming"),
            &mut rendered,
            &mut completed,
            &mut reasoning_rendered,
            &mut reasoning_rows,
            &mut turn_active,
            &started,
            "bot:b1",
            None,
            &tx,
        );
        let first = rx.try_recv().expect("reasoning event");
        match first {
            BackendEvent::Session {
                event: SessionEvent::Reasoning { text, done, reset, .. },
                ..
            } => {
                assert_eq!(text, "思考");
                assert!(!done);
                assert!(!reset);
            }
            other => panic!("unexpected first event: {other:?}"),
        }
        assert!(rendered.is_empty(), "reasoning 不能进 assistant 正文");

        apply_v4_row(
            &row("思考", "complete"),
            &mut rendered,
            &mut completed,
            &mut reasoning_rendered,
            &mut reasoning_rows,
            &mut turn_active,
            &started,
            "bot:b1",
            None,
            &tx,
        );
        match rx.try_recv().expect("done event") {
            BackendEvent::Session {
                event: SessionEvent::Reasoning { done, .. },
                ..
            } => assert!(done),
            other => panic!("unexpected done event: {other:?}"),
        }
    }
}
