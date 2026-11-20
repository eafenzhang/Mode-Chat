//! 真实 Agent 控制面：内嵌 pi-rust 引擎 + 本地 ACP 引擎（无 Node/TS 运行时）。
//!
//! 每个 Bot 用 `workspaceIdentity = mode-bot:teammate:<id>` 单独建会话；事件按
//! `key` 打回 UI，切换 Bot 不互相污染。
//!
//! 环境：
//! - `BOT_PILOT_WORKSPACE`：Bot 目录为空时的兜底工作目录；
//! - `BOT_PILOT_MODEL`：`provider/model`（可选；缺省用 Bot 的 modelSelection）；
//! - `MODE_STORAGE_DIR` / `MODE_SESSION_DB_PATH`：数据隔离（建议指向 `.mode-bot-pilot`）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::data::ModelSpec;

#[derive(Debug, Clone)]
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
        None => EngineChoice::Rpi,
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
    let mut extra_tools = crate::mcp_host::for_workspace(workspace)
        .await
        .harness_tools();
    extra_tools.extend(crate::automation_tools::tools(
        &crate::data::data_root(),
        workspace,
        identity,
    ));
    extra_tools.extend(crate::browser_tools::tools());
    extra_tools.extend(crate::computer_tools::tools());
    extra_tools.extend(crate::cua_tree::tools());
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
        extra_tools,
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

#[allow(dead_code)]
enum Command {
    Ensure {
        key: String,
        workspace: String,
        identity: String,
        model: Option<ModelSpec>,
        /// 上次会话 id（重启续用；内嵌引擎下是 JSONL 路径）。
        session_id: Option<String>,
        /// 该 Bot 选择的本地智能体引擎（None/"mode" = 内嵌引擎）。
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


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoMode {
    None,
    Demo,
    Selftest,
}

/// 会话引擎：内嵌 pi-rust / 本地智能体（ACP）。
#[derive(Clone)]
enum SessionEngine {
    Rpi(Arc<crate::rpi_engine::RpiSession>),
    Acp(crate::acp::AcpHandle),
}

struct SessionCtx {
    /// 频道成员会话的展示名（私聊 None；Stop 命令的错误归属用）。
    member: Option<String>,
    engine: SessionEngine,
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

async fn run_agent(
    commands: &mut UnboundedReceiver<Command>,
    events: &UnboundedSender<BackendEvent>,
) -> Result<(), String> {
    let _ = events.send(BackendEvent::Status("连接 Agent…".to_string()));

    if let Err(error) = crate::automation_scheduler::start(&crate::data::data_root()) {
        eprintln!("[automation] 调度器启动失败：{error}");
    }
    // IM 运行时：与主程序共用 bot 配置/凭据/轮询锁；回合走内嵌引擎。
    crate::im_service::start(crate::data::data_root());

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
                                member: None,
                                engine: SessionEngine::Acp(handle),
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
                        let mut extra_tools = crate::mcp_host::for_workspace(&workspace)
                            .await
                            .harness_tools();
                        extra_tools.extend(crate::automation_tools::tools(
                            &crate::data::data_root(),
                            &workspace,
                            &identity,
                        ));
                        extra_tools.extend(crate::browser_tools::tools());
                        extra_tools.extend(crate::computer_tools::tools());
                        extra_tools.extend(crate::cua_tree::tools());
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
                            extra_tools,
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
                                        member: None,
                                        engine: SessionEngine::Rpi(Arc::clone(&session)),
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
                }
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
                                member: None,
                                engine: SessionEngine::Acp(handle),
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
                            Ok((session, _spec)) => {
                                let path = session.path.clone();
                                sessions.insert(
                                    key.clone(),
                                    SessionCtx {
                                        member: None,
                                        engine: SessionEngine::Rpi(Arc::new(session)),
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
                }
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
                                member: None,
                                engine: SessionEngine::Acp(handle),
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
                            Ok((session, _spec)) => {
                                let session = Arc::new(session);
                                let path = session.path.clone();
                                sessions.insert(
                                    key.clone(),
                                    SessionCtx {
                                        member: None,
                                        engine: SessionEngine::Rpi(Arc::clone(&session)),
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
                }
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
                // ACP：request_id 直接交给该会话的句柄。
                if let Some(ctx) = sessions.get(&key)
                    && let SessionEngine::Acp(handle) = &ctx.engine
                {
                    handle.resolve(request_id, option_id);
                }
            }
            Command::AnswerQuestion {
                key,
                request_id,
                answer,
                ..
            } => {
                let mut answered = false;
                for ctx in sessions.values() {
                    if let SessionEngine::Rpi(session) = &ctx.engine
                        && session.questions.answer(&request_id, answer.clone())
                    {
                        answered = true;
                        break;
                    }
                }
                if !answered {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: "提问已失效或已被回答".to_string(),
                        },
                    });
                }
            }
            Command::AnswerElicitation {
                key,
                request_id,
                content,
            } => {
                let Some(ctx) = sessions.get(&key) else {
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
                        Ok((session, _model)) => {
                            let path = session.path.clone();
                            for mut entry in crate::rpi_engine::history_entries(&path) {
                                if entry.user {
                                    let Some(text) = group_user_text(&entry.text) else {
                                        continue;
                                    };
                                    entry.text = text;
                                } else {
                                    entry.member = Some(member.name.clone());
                                }
                                merged_history.push(entry);
                            }
                            sessions.insert(
                                internal.clone(),
                                SessionCtx {
                                    member: Some(member.name.clone()),
                                    engine: SessionEngine::Rpi(Arc::new(session)),
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
                                    message: format!("{} 建会话失败：{message}", member.name),
                                },
                            });
                        }
                    }
                }
                merged_history.sort_by_key(|entry| entry.at);
                if merged_history.len() > 80 {
                    merged_history.drain(..merged_history.len() - 80);
                }
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
                    if let SessionEngine::Rpi(session) = &ctx.engine {
                        let images =
                            match crate::rpi_engine::images_from_attachments(&attachments) {
                                Ok(images) => images,
                                Err(message) => {
                                    let _ = events.send(BackendEvent::Session {
                                        key: key.clone(),
                                        event: SessionEvent::Error {
                                            member: Some(member.name.clone()),
                                            message: format!("{}：{message}", member.name),
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
                    }
                }
            }
            Command::ResetSession { key } => {
                sessions.remove(&key);
                active_internal.remove(&key);
            }
            Command::Stop { key } => {
                // 停止该 UI key 下所有成员会话的进行中回合。
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
            }
            Command::MemoryReview { snapshot } => {
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
                    if let Err(error) = run_memory_review(&snapshot).await {
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
                if let SessionEngine::Rpi(session) = &ctx.engine {
                    let session = Arc::clone(session);
                    tokio::spawn(async move {
                        session.send(prompt, Vec::new()).await;
                    });
                }
            }
            Command::LoadAttachmentImage {
                attachment_ref,
                ..
            } => {
                let _ = events.send(BackendEvent::AttachmentImage {
                    attachment_ref,
                    path: None,
                    error: Some("当前引擎不支持历史附件".to_string()),
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
            }
        }
    }
    Ok(())
}

/// 从群 prompt 里取出用户原话（群成员的 user 消息是合成 prompt，不能直接上屏）。
fn group_user_text(prompt: &str) -> Option<String> {
    let marker = "用户刚说：";
    let start = prompt.find(marker)? + marker.len();
    let rest = prompt[start..].trim_start_matches('\n');
    let end = rest.find("\n\n请直接写").unwrap_or(rest.len());
    let text = rest[..end].trim();
    (!text.is_empty()).then(|| text.to_string())
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

/// 跑一次回顾：内嵌引擎的隐藏会话 → 发整理 prompt → 收集回复 → 解析并落盘。
async fn run_memory_review(
    snapshot: &crate::memory_review::ReviewSnapshot,
) -> Result<(), String> {
    if !crate::memory_review::enabled() {
        return Ok(());
    }
    let Some(memory_root) = crate::data::group_memory_root(&snapshot.group_id) else {
        return Err("群记忆存储不可用".to_string());
    };
    std::fs::create_dir_all(&memory_root)
        .map_err(|error| format!("创建群记忆目录失败：{error}"))?;
    let state_path = review_state_path(&snapshot.group_id);
    let stored = state_path
        .as_deref()
        .map(read_review_session)
        .unwrap_or_default();
    let model = ModelSpec {
        provider_id: snapshot
            .model
            .get("providerId")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        model_id: snapshot
            .model
            .get("modelId")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string(),
        reasoning_level: snapshot
            .model
            .pointer("/options/reasoningLevel")
            .and_then(|value| value.as_str())
            .map(str::to_string),
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<BackendEvent>();
    let (session, _review_model) = open_rpi_session(
        &format!("memory-review:{}", snapshot.group_id),
        &snapshot.workspace,
        &snapshot.identity,
        Some(model),
        stored.session_id.clone(),
        None,
        &tx,
    )
    .await?;
    if let Some(path) = state_path.as_deref() {
        write_review_session(path, &session.path);
    }
    let prompt = crate::memory_review::build_prompt(snapshot, &memory_root);
    let session = Arc::new(session);
    let sender = Arc::clone(&session);
    tokio::spawn(async move {
        sender.send(prompt, Vec::new()).await;
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let mut answer = String::new();
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("回顾超时（90s）".to_string());
        }
        let Ok(Some(event)) = tokio::time::timeout(remaining, rx.recv()).await else {
            return Err("回顾通道关闭".to_string());
        };
        let BackendEvent::Session { event, .. } = event else {
            continue;
        };
        match event {
            SessionEvent::Delta { text, .. } => answer.push_str(&text),
            SessionEvent::Done { .. } => break,
            SessionEvent::Error { message, .. } => return Err(message),
            _ => {}
        }
    }
    if answer.trim().is_empty() {
        return Ok(());
    }
    let operations = crate::memory_review::parse_operations(&answer)?;
    if operations.is_empty() {
        return Ok(());
    }
    let applied =
        crate::memory_review::apply_operations(&memory_root, &snapshot.identity, &operations)?;
    if applied > 0 {
        eprintln!(
            "[memory-review] group={} applied {applied} operation(s)",
            snapshot.group_id
        );
    }
    Ok(())
}
