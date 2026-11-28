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
    /// 群聊 id（读 goal.json 用）。
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
    /// 工具调用参数 JSON（edit 的 path/edits、bash 的 command）。更新事件里为空时沿用旧值。
    pub input: String,
    /// 工具结果的结构化 details JSON（edit 的 diff、bash 的 exit_code）。最多约 8k 字符。
    pub details: String,
    /// 工具结果里的图片（截图类工具），已落成临时 PNG/JPEG 的本地路径。
    pub images: Vec<String>,
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
    /// 群聊成员名（群聊历史；私聊 None）。
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
    /// 群聊成员归属（私聊为 None）；卡头标注「来自 X」。
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
    /// 群聊成员归属（私聊为 None）。
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
    /// 群聊成员归属（私聊为 None）；卡头标注「来自 X」。
    pub member: Option<String>,
    /// 隐私越界（访问别人/已归档的目录）。这类卡**不允许被审批档位自动放行**：
    /// `full` 的语义是「信任这个 Bot 在它自己的范围内行动」，不是「允许它读别人的隐私目录」。
    /// UI 侧（`main.rs::auto_permission`）见到 true 必须直接返回 None，把卡交给用户。
    pub privacy: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum SessionEvent {
    /// 恢复出来的历史消息（先于 Ready 到达，UI 用它替换 transcript）。
    History(Vec<HistoryEntry>),
    /// 会话已建好（订阅就绪）；slot 非空时是群聊成员会话的持久化槽位。
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
    /// 一轮结束的 token 用量（仅内嵌引擎上报；member=None 是私聊）。
    TurnUsage {
        member: Option<String>,
        input_tokens: i64,
        output_tokens: i64,
        cache_read: i64,
        reasoning_tokens: i64,
    },
    /// 供应商请求失败后的自动重试（不当作回合错误，重试成功后正常出字）。
    Retry {
        member: Option<String>,
        attempt: u32,
        max_retries: u32,
        reason: String,
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
    /// 上下文压缩状态（v0.5.0 W4）：`active=true` 开始、`false` 结束（成功或失败都发）。
    Compaction { active: bool, manual: bool },
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
    /// Chief 用 `team_assign` 派活：任务已落台账，UI 负责触发目标成员回合。
    TeamDispatch {
        key: String,
        member_id: String,
        member_name: String,
        text: String,
        task_id: String,
        root_task_id: String,
        root_message_id: String,
    },
    /// 别的 Bot 往群里发言（`post_to_group`）：UI 把它当一条用户消息插进群会话并派发。
    /// 后台任务状态/进度（节流广播；record 是整条快照）。
    Task { record: crate::task_store::TaskRecord },
    GroupPost {
        group_id: String,
        text: String,
        from_bot_id: String,
        from_bot_name: String,
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

/// 该 Bot 的回合是否由完整引擎执行（内嵌 rpi；ACP 引擎能力受限）。
pub fn engine_is_mode(selected: Option<&str>) -> bool {
    !matches!(resolve_engine(selected), EngineChoice::Acp(_))
}

/// 当前选中的引擎是否是外部 ACP（而不是内嵌 pi-rust）。
pub fn engine_is_acp(selected: Option<&str>) -> bool {
    matches!(resolve_engine(selected), EngineChoice::Acp(_))
}

/// Bot id → 显示名（只读名册；查不到就 None，用量页归「未归属」）。
pub fn bot_name_of(bot_id: &str) -> Option<String> {
    crate::data::Roster::load()
        .bot_by_id(bot_id)
        .map(|bot| bot.name.clone())
}

/// 建一个内嵌 rpi 会话（新会话/恢复历史共用）。
#[allow(clippy::too_many_arguments)]
async fn open_rpi_session(
    key: &str,
    workspace: &str,
    identity: &str,
    model: Option<ModelSpec>,
    resume: Option<String>,
    member: Option<String>,
    events: &UnboundedSender<BackendEvent>,
    session_tools: Vec<rpi_harness::types::HarnessTool>,
) -> Result<(crate::rpi_engine::RpiSession, ModelSpec), String> {
    let spec = model
        .or_else(env_model)
        .or_else(|| crate::rpi_engine::default_model_spec(identity, workspace))
        .ok_or_else(|| "未选择模型：请先在右栏或设置里为这个 Bot 选一个模型".to_string())?;
    let engine_model = crate::rpi_engine::resolve_model(&spec)?;
    let (approval_level, allowlist) = crate::rpi_engine::approval_policy(identity, workspace);
    let bot_id = crate::rpi_engine::bot_id_from_identity(identity);
    // 系统提示要带 Bot 自己的记忆；预算按模型窗口算，所以先解析窗口。
    let limits = crate::model_limits::read(&spec.provider_id, &spec.model_id);
    let (context_window, _) = crate::model_limits::resolve(&limits);
    let system_prompt = crate::rpi_engine::system_prompt_for_bot(
        std::path::Path::new(workspace),
        bot_id.as_deref(),
        context_window,
    );
    let mut extra_tools = crate::mcp_host::for_workspace(workspace)
        .await
        .harness_tools();
    extra_tools.extend(crate::automation_tools::tools(
        &crate::data::data_root(),
        workspace,
        identity,
    ));
    // 记忆回顾是隐藏的 JSON 任务，不给控制类工具，也不给记忆写入工具。
    let controls = !key.starts_with("memory-review:");
    if controls {
        // 后台任务工具（长任务）：Bot 可给自己派独立会话跑长时间工作。
        extra_tools.extend(crate::task_tools::tools(identity));
    }
    // 官方插件开关（workspace `.mode/config.json` 的 enabledPlugins）也参与放行：
    // 没有可读的 config.json 时不限制；显式关闭才停注对应工具族。
    let (browser_plugin_on, computer_plugin_on) =
        crate::data::read_plugin_switches(workspace).unwrap_or((true, true));
    if controls && browser_plugin_on && crate::tool_scope::allowed(crate::tool_scope::Family::Browser, identity) {
        extra_tools.extend(crate::browser_tools::tools(identity));
    }
    if controls && computer_plugin_on && crate::tool_scope::allowed(crate::tool_scope::Family::Computer, identity) {
        extra_tools.extend(crate::computer_tools::tools(identity));
        extra_tools.extend(crate::cua_tree::tools(identity));
    }
    extra_tools.extend(session_tools);
    let private_bot = crate::rpi_engine::private_bot_id(identity);
    // 标记为隐私后不再续用主目录里的旧会话；新会话落到 `.private/<id>`。
    let resume = resume.filter(|path| std::path::Path::new(path).is_file()).filter(|path| {
        private_bot.is_none() || path.to_ascii_lowercase().contains(".private")
    });
    let options = crate::rpi_engine::OpenOptions {
        workspace: workspace.into(),
        resume_path: resume,
        model: engine_model,
        thinking: crate::rpi_engine::thinking_level_for(&spec),
        system_prompt,
        events: events.clone(),
        key: key.to_string(),
        member,
        approval_level,
        allowlist,
        bot_id: bot_id.clone(),
        bot_name: bot_id.as_deref().and_then(bot_name_of),
        extra_tools,
        private_bot,
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
    /// 回合进行中实时插话：把文本 steer 给正在跑的 lane。
    /// `members` 为空 = 私聊/旁路会话；否则是群聊里当前正在流式的成员名。
    Interject {
        key: String,
        text: String,
        members: Vec<String>,
        /// 插话附带的图片（和正常发送同一套校验，见 images_from_attachments）。
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
    /// 私有/群聊都按 `key` 归属，会话 id 由 CLI 决定，所以不需要额外 bot_id。
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
        /// 本轮根任务（Chief 工具派活要用；旧调用方可以不带）。
        root_task_id: Option<String>,
        root_message_id: Option<String>,
    },
    /// 停止该 UI key（私聊/群聊）下所有进行中的成员回合。
    Stop { key: String },
    /// 只停止群聊里某个成员的进行中回合（群任务按条 Stop）。
    StopMember { key: String, member_id: String },
    /// 群任务重试/改派：给指定成员重新派发一个回合（不新增用户气泡）。
    ContinueTask {
        key: String,
        member_id: String,
        text: String,
        root_task_id: Option<String>,
        root_message_id: Option<String>,
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
    /// 立即压缩上下文（命令面板「立即压缩」，W4）。
    Compact { key: String },
    /// 开一个后台任务：独立 RpiSession（key 带任务 id），与主会话并行。
    TaskStart {
        ui_key: String,
        bot_id: String,
        title: String,
        prompt: String,
        reply: tokio::sync::oneshot::Sender<Result<crate::task_store::TaskRecord, String>>,
    },
    /// 给运行中的任务追加一条消息。
    TaskSend { task_id: String, text: String },
    /// 停止运行中的任务（lane abort）。
    TaskStop { task_id: String },
    /// 跨 Bot 投递（`message_bot`）：开一个不进侧栏的会话把文本发给目标 Bot，
    /// 回合结束把整段回复经 `oneshot` 回给调用者。
    AskBot {
        request_id: String,
        target_bot_id: String,
        target_bot_name: String,
        text: String,
        reply: tokio::sync::oneshot::Sender<Result<BotTurn, String>>,
    },
    /// 往群聊里发言（`post_to_group`）：UI 收 `BackendEvent::GroupPost` 后走既有群发送路径。
    PostGroup {
        group_id: String,
        text: String,
        from_bot_id: String,
        from_bot_name: String,
    },
}

pub struct BackendHandle {
    tx: UnboundedSender<Command>,
}

/// 跨 Bot 投递的等待上限（秒）。
///
/// 目标 Bot 可能要过审批卡、跑一整轮，等太久用户就是在群里干站着；
/// 但等不到**不能**当成没送到——消息早就进目标会话了，只是还没答完。
pub const ASK_BOT_TIMEOUT_SECS: u64 = 60;

/// 目标 Bot 那一轮的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotTurn {
    /// 目标答完了，拿到整段回复。
    Reply(String),
    /// 目标还在跑（多半卡在审批上），消息已送达，稍后它自己会答。
    StillRunning,
}

/// `message_bot` 对调用方的交代。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotDelivery {
    /// 目标答完了，拿到整段回复。
    Reply(String),
    /// 已投递，目标还在处理；不是失败，不要报「超时」。
    Delivered { target_bot_name: String },
}

impl BotDelivery {
    /// 给 Agent 工具看的文案。`Delivered` 必须让人看出「送到了、在等答复」，
    /// 不能读成「没送到」——用户会照着这个去重发一遍。
    pub fn describe(&self) -> String {
        match self {
            Self::Reply(text) => text.clone(),
            Self::Delivered { target_bot_name } => format!(
                "已投递给 {target_bot_name}，它还在处理（{ASK_BOT_TIMEOUT_SECS}s 内没答完）。\
                 消息已经送到了，它会稍后自己回复，不用重发。"
            ),
        }
    }
}

/// 等目标 Bot 的回复；超时返回 [`BotDelivery::Delivered`]，不当失败。
///
/// 抽出来单独测：超时必须走「已投递」这条分支，而不是报错（回归 D-11）。
async fn await_bot_reply(
    reply: tokio::sync::oneshot::Receiver<Result<BotTurn, String>>,
    wait: Duration,
    target_bot_name: &str,
) -> Result<BotDelivery, String> {
    match tokio::time::timeout(wait, reply).await {
        Ok(Ok(Ok(BotTurn::Reply(text)))) => Ok(BotDelivery::Reply(text)),
        Ok(Ok(Ok(BotTurn::StillRunning))) => Ok(BotDelivery::Delivered {
            target_bot_name: target_bot_name.to_string(),
        }),
        Ok(Ok(Err(message))) => Err(message),
        Ok(Err(_)) => Err("目标 Bot 未返回结果".to_string()),
        Err(_) => Ok(BotDelivery::Delivered {
            target_bot_name: target_bot_name.to_string(),
        }),
    }
}

impl Clone for BackendHandle {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

/// 进程级后台总线：工具（`message_bot` / `post_to_group`）在 Agent 线程里需要
/// 把活投回 run loop，但工具拿不到 `BackendHandle`，所以 `spawn()` 时登记一份。
static BUS: OnceLock<BackendHandle> = OnceLock::new();

/// 取进程级总线（未启动时返回 None）。
pub fn bus() -> Option<BackendHandle> {
    BUS.get().cloned()
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

    /// 回合进行中实时插话（私聊/群聊当前流式成员）。
    pub fn interject(
        &self,
        key: impl Into<String>,
        text: impl Into<String>,
        members: Vec<String>,
        attachments: Vec<AttachmentUpload>,
    ) {
        let _ = self.tx.send(Command::Interject {
            key: key.into(),
            text: text.into(),
            members,
            attachments,
        });
    }

    /// 建群聊成员会话（每个成员一条隐藏会话，事件都打回同一个 group_key）。
    pub fn ensure_group(&self, spec: GroupSpec) {
        let _ = self.tx.send(Command::EnsureGroup { spec });
    }

    /// 丢弃一个会话（切换引擎后让下一次 Ensure 重新建）。
    pub fn reset_session(&self, key: impl Into<String>) {
        let _ = self.tx.send(Command::ResetSession { key: key.into() });
    }

    /// 停止进行中的回合（私聊/群聊通用）。
    pub fn stop(&self, key: impl Into<String>) {
        let _ = self.tx.send(Command::Stop { key: key.into() });
    }

    /// 只停止群聊里某个成员的回合（群任务行内 Stop）。
    pub fn stop_member(&self, key: impl Into<String>, member_id: impl Into<String>) {
        let _ = self.tx.send(Command::StopMember {
            key: key.into(),
            member_id: member_id.into(),
        });
    }

    /// 群任务重试/改派：给成员重新派发一个回合（文本由调用方拼好）。
    /// 带根任务信息时，目标成员里的 Chief 工具可以继续派活。
    pub fn continue_task_in_turn(
        &self,
        key: impl Into<String>,
        member_id: impl Into<String>,
        text: impl Into<String>,
        root_task_id: Option<String>,
        root_message_id: Option<String>,
    ) {
        let _ = self.tx.send(Command::ContinueTask {
            key: key.into(),
            member_id: member_id.into(),
            text: text.into(),
            root_task_id,
            root_message_id,
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

    /// 群聊发言：按 @点名 / 回应策略选成员，逐成员组 prompt 后发送。
    /// 带根任务信息时，群里的 Chief 工具可以按当前根任务派活。
    pub fn send_group_in_turn(
        &self,
        key: impl Into<String>,
        text: impl Into<String>,
        attachments: Vec<AttachmentUpload>,
        root_task_id: Option<String>,
        root_message_id: Option<String>,
    ) {
        let _ = self.tx.send(Command::SendGroup {
            key: key.into(),
            text: text.into(),
            attachments,
            root_task_id,
            root_message_id,
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

    /// 跨 Bot 投递（`message_bot`）：等目标 Bot 回合结束拿到整段回复。
    ///
    /// 等待 [`ASK_BOT_TIMEOUT_SECS`]；等不到时**不算失败**——消息早已送达目标，
    /// 只是它还没答完（可能要过审批卡）。这时回 [`BotDelivery::Delivered`]，
    /// 让调用方转述「已投递，稍后它会自己回复」，而不是报「超时失败」让用户以为没送到。
    pub async fn ask_bot(
        &self,
        target_bot_id: String,
        target_bot_name: String,
        text: String,
    ) -> Result<BotDelivery, String> {
        let request_id = format!("ask-{}-{}", target_bot_id, crate::time::now_ms());
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Command::AskBot {
                request_id: request_id.clone(),
                target_bot_id,
                target_bot_name: target_bot_name.clone(),
                text,
                reply: reply_tx,
            })
            .map_err(|_| "后台引擎已停止，无法跨 Bot 投递".to_string())?;
        await_bot_reply(
            reply_rx,
            Duration::from_secs(ASK_BOT_TIMEOUT_SECS),
            &target_bot_name,
        )
        .await
    }

    /// 往群里发言（`post_to_group`）：只投一条事件，UI 走既有群发送路径。
    pub fn post_group(
        &self,
        group_id: String,
        text: String,
        from_bot_id: String,
        from_bot_name: String,
    ) -> Result<(), String> {
        self.tx
            .send(Command::PostGroup {
                group_id,
                text,
                from_bot_id,
                from_bot_name,
            })
            .map_err(|_| "后台引擎已停止，无法往群里发言".to_string())
    }

    /// 立即压缩该会话的上下文（命令面板项）。
    pub fn compact(&self, key: impl Into<String>) {
        let _ = self.tx.send(Command::Compact { key: key.into() });
    }

    /// 开一个后台任务；`reply` 在任务会话建好后给出记录（端到端错误在里面）。
    pub fn task_start(
        &self,
        ui_key: String,
        bot_id: String,
        title: String,
        prompt: String,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<crate::task_store::TaskRecord, String>>, String>
    {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Command::TaskStart {
                ui_key,
                bot_id,
                title,
                prompt,
                reply,
            })
            .map_err(|_| "后台引擎已停止，无法创建任务".to_string())?;
        Ok(rx)
    }

    pub fn task_send(&self, task_id: impl Into<String>, text: impl Into<String>) {
        let _ = self.tx.send(Command::TaskSend {
            task_id: task_id.into(),
            text: text.into(),
        });
    }

    pub fn task_stop(&self, task_id: impl Into<String>) {
        let _ = self.tx.send(Command::TaskStop {
            task_id: task_id.into(),
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
    /// 群聊成员会话的展示名（私聊 None；Stop 命令的错误归属用）。
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

    let handle = BackendHandle {
        tx: command_tx.clone(),
    };
    // 工具侧（message_bot / post_to_group）拿不到 handle，登记一份进程级总线。
    let _ = BUS.set(handle.clone());
    (handle, event_rx)
}

async fn run_agent(
    commands: &mut UnboundedReceiver<Command>,
    events: &UnboundedSender<BackendEvent>,
) -> Result<(), String> {
    let _ = events.send(BackendEvent::Status("连接 Agent…".to_string()));

    if let Err(error) = crate::automation_scheduler::start(&crate::data::data_root()) {
        eprintln!("[automation] 调度器启动失败：{error}");
    }
    // 后台任务恢复：上次退出时仍在跑的任务标 interrupted（不自动重跑）。
    let interrupted = crate::task_store::mark_interrupted(&crate::data::data_root());
    if interrupted > 0 {
        eprintln!("[tasks] {interrupted} 个后台任务标记为已中断");
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
    // 每个群一份 Chief 工具上下文（根任务 + 本回合活跃成员）。
    let mut team_turns: HashMap<String, crate::team_tools::TeamTurn> = HashMap::new();
    let mut member_talked: HashSet<String> = HashSet::new();
    let mut auto_sent = false;
    // UI key → 当前活跃会话的 internal key（新会话/切换会话后 Send 路由到新会话）。
    let mut active_internal: HashMap<String, String> = HashMap::new();
    // 后台任务：task_id → 运行中的会话句柄（记录本身在 task_store）。
    let mut tasks: HashMap<String, Arc<crate::rpi_engine::RpiSession>> = HashMap::new();
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
                        let run_bot_id = crate::rpi_engine::bot_id_from_identity(&identity);
                        let run_limits =
                            crate::model_limits::read(&spec.provider_id, &spec.model_id);
                        let (run_context_window, _) = crate::model_limits::resolve(&run_limits);
                        let system_prompt = crate::rpi_engine::system_prompt_for_bot(
                            std::path::Path::new(&workspace),
                            run_bot_id.as_deref(),
                            run_context_window,
                        );
                        let private_bot = crate::rpi_engine::private_bot_id(&identity);
                        let resume_path = resume_id
                            .clone()
                            .filter(|path| std::path::Path::new(path).is_file())
                            .filter(|path| {
                                private_bot.is_none()
                                    || path.to_ascii_lowercase().contains(".private")
                            });
                        let mut extra_tools = crate::mcp_host::for_workspace(&workspace)
                            .await
                            .harness_tools();
                        extra_tools.extend(crate::automation_tools::tools(
                            &crate::data::data_root(),
                            &workspace,
                            &identity,
                        ));
                        // 官方插件开关（enabledPlugins）参与放行：无可读 config.json 不限制。
                        let (browser_plugin_on, computer_plugin_on) =
                            crate::data::read_plugin_switches(&workspace)
                                .unwrap_or((true, true));
                        if browser_plugin_on
                            && crate::tool_scope::allowed(
                                crate::tool_scope::Family::Browser,
                                &identity,
                            )
                        {
                            extra_tools.extend(crate::browser_tools::tools(&identity));
                        }
                        if computer_plugin_on
                            && crate::tool_scope::allowed(
                                crate::tool_scope::Family::Computer,
                                &identity,
                            )
                        {
                            extra_tools.extend(crate::computer_tools::tools(&identity));
                            extra_tools.extend(crate::cua_tree::tools(&identity));
                        }
                        extra_tools.extend(crate::memory_tools::tools(run_bot_id.as_deref()));
                        extra_tools.extend(crate::bot_tools::tools(&identity));
                        extra_tools.extend(crate::task_tools::tools(&identity));
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
                            bot_id: run_bot_id.clone(),
                            bot_name: run_bot_id.as_deref().and_then(bot_name_of),
                            extra_tools,
                            private_bot,
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
                            Vec::new(),
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
                            Vec::new(),
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
                                // 切历史会话要把旧 transcript 回放给 UI（EnsureSession 同路径已发）。
                                // 之前这里只发 Ready，切过去是一张空白会话。
                                let history = crate::rpi_engine::history_entries(&path);
                                if !history.is_empty() {
                                    let _ = events.send(BackendEvent::Session {
                                        key: key.clone(),
                                        event: SessionEvent::History(history),
                                    });
                                }
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
                let mut resolved = false;
                for ctx in sessions.values() {
                    if let SessionEngine::Rpi(session) = &ctx.engine
                        && session.gate.resolve(&request_id, &option_id)
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
                let team_turn = crate::team_tools::turn_context();
                team_turns.insert(spec.group_key.clone(), Arc::clone(&team_turn));
                let member_infos: Vec<(String, String, String, bool)> = spec
                    .members
                    .iter()
                    .map(|member| {
                        (
                            member.member_id.clone(),
                            member.name.clone(),
                            member.role.clone(),
                            member.is_admin,
                        )
                    })
                    .collect();
                let mut merged_history: Vec<HistoryEntry> = Vec::new();
                for member in &spec.members {
                    let internal = member_session_key(&spec.group_key, &member.member_id);
                    if sessions.contains_key(&internal) {
                        continue;
                    }
                    // 只有 Chief 拿到团队管理工具；普通成员看不到目标/派活入口。
                    let session_tools = if member.is_chief {
                        crate::team_tools::tools(
                            &crate::data::data_root(),
                            &spec.info.group_id,
                            &spec.group_key,
                            &member.member_id,
                            &member.name,
                            member_infos.clone(),
                            Arc::clone(&team_turn),
                            events.clone(),
                        )
                    } else {
                        Vec::new()
                    };
                    let opened = open_rpi_session(
                        &spec.group_key,
                        &member.workspace,
                        &member.identity,
                        member.model.clone(),
                        member.session_id.clone(),
                        Some(member.name.clone()),
                        events,
                        session_tools,
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
                root_task_id,
                root_message_id,
            } => {
                let Some(spec) = groups.get(&key).cloned() else {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: "群聊会话尚未建立".to_string(),
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
                if let Some(turn) = team_turns.get(&key) {
                    let active: Vec<String> = chosen
                        .iter()
                        .map(|index| spec.members[*index].member_id.clone())
                        .collect();
                    crate::team_tools::begin_turn(
                        turn,
                        root_task_id.as_deref().unwrap_or(""),
                        root_message_id.as_deref().unwrap_or(""),
                        active,
                    );
                }
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
            Command::PostGroup {
                group_id,
                text,
                from_bot_id,
                from_bot_name,
            } => {
                let _ = events.send(BackendEvent::GroupPost {
                    group_id,
                    text,
                    from_bot_id,
                    from_bot_name,
                });
            }
            Command::TaskStart {
                ui_key,
                bot_id,
                title,
                prompt,
                reply,
            } => {
                let data_root = crate::data::data_root();
                let roster = crate::data::Roster::load();
                let Some(bot) = roster.bot_by_id(&bot_id).cloned() else {
                    let _ = reply.send(Err(format!("找不到 Bot：{bot_id}")));
                    continue;
                };
                if bot.is_private {
                    let _ = reply.send(Err("隐私 Bot 不能创建后台任务".to_string()));
                    continue;
                }
                if crate::task_store::running_count(&data_root, &bot_id)
                    >= crate::task_store::MAX_RUNNING_PER_BOT
                {
                    let _ = reply.send(Err(format!(
                        "同时运行的任务已达上限（{} 个），等一个结束再试",
                        crate::task_store::MAX_RUNNING_PER_BOT
                    )));
                    continue;
                }
                let task_id = format!("t-{}-{}", crate::im_service::now_ms(), tasks.len() + 1);
                let internal = format!("{ui_key}\u{2}{task_id}");
                let (tx, mut rx) = unbounded_channel::<BackendEvent>();
                let opened = open_rpi_session(
                    &internal,
                    &bot.directory,
                    &bot.identity(),
                    bot.model.clone(),
                    None,
                    None,
                    &tx,
                    Vec::new(),
                )
                .await;
                let session = match opened {
                    Ok((session, _spec)) => Arc::new(session),
                    Err(message) => {
                        let _ = reply.send(Err(format!("任务会话创建失败：{message}")));
                        continue;
                    }
                };
                let mut record = crate::task_store::TaskRecord {
                    id: task_id.clone(),
                    bot_id: bot_id.clone(),
                    bot_name: bot.name.clone(),
                    ui_key: ui_key.clone(),
                    title: if title.trim().is_empty() {
                        prompt.chars().take(30).collect()
                    } else {
                        title.clone()
                    },
                    prompt: prompt.clone(),
                    status: crate::task_store::TaskStatus::Running,
                    created_at: crate::im_service::now_ms() as u64,
                    finished_at: 0,
                    session_path: session.path.clone(),
                    last_line: String::new(),
                    input_tokens: 0,
                    output_tokens: 0,
                    error: String::new(),
                };
                if let Err(error) = crate::task_store::upsert(&data_root, &record) {
                    let _ = reply.send(Err(error));
                    continue;
                }
                let _ = reply.send(Ok(record.clone()));
                let _ = events.send(BackendEvent::Task {
                    record: record.clone(),
                });
                // 事件泵：流式更新 last_line/usage，节流落盘 + 广播；终态收口。
                let pump_events = events.clone();
                let pump_root = data_root.clone();
                tokio::spawn(async move {
                    let mut dirty = false;
                    let mut last_flush = std::time::Instant::now();
                    loop {
                        let received =
                            tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
                        match received {
                            Ok(Some(BackendEvent::Session { event, .. })) => match event {
                                SessionEvent::Delta { text, .. } => {
                                    record.last_line.push_str(&text);
                                    let chars = record.last_line.chars().count();
                                    if chars > 140 {
                                        let skip = chars - 140;
                                        record.last_line =
                                            record.last_line.chars().skip(skip).collect();
                                    }
                                    dirty = true;
                                }
                                SessionEvent::Tool(card) => {
                                    record.last_line = format!("· {}", card.title);
                                    dirty = true;
                                }
                                SessionEvent::TurnUsage {
                                    input_tokens,
                                    output_tokens,
                                    ..
                                } => {
                                    record.input_tokens += input_tokens;
                                    record.output_tokens += output_tokens;
                                    dirty = true;
                                }
                                SessionEvent::Done { .. } => {
                                    record.status = crate::task_store::TaskStatus::Done;
                                    record.finished_at = crate::im_service::now_ms() as u64;
                                    let _ = crate::task_store::upsert(&pump_root, &record);
                                    let _ = pump_events.send(BackendEvent::Task {
                                        record: record.clone(),
                                    });
                                    if let Some(service) = crate::im_service::handle() {
                                        service.notify_task_finished(
                                            record.bot_id.clone(),
                                            record.title.clone(),
                                            true,
                                        );
                                    }
                                    break;
                                }
                                SessionEvent::Error { message, .. } => {
                                    record.status = crate::task_store::TaskStatus::Failed;
                                    record.error = message;
                                    record.finished_at = crate::im_service::now_ms() as u64;
                                    let _ = crate::task_store::upsert(&pump_root, &record);
                                    let _ = pump_events.send(BackendEvent::Task {
                                        record: record.clone(),
                                    });
                                    if let Some(service) = crate::im_service::handle() {
                                        service.notify_task_finished(
                                            record.bot_id.clone(),
                                            record.title.clone(),
                                            false,
                                        );
                                    }
                                    break;
                                }
                                _ => {}
                            },
                            Ok(Some(_)) => {}
                            Ok(None) => break,
                            Err(_) => {} // 2s 无事件：继续等
                        }
                        if dirty && last_flush.elapsed() > Duration::from_secs(2) {
                            let _ = crate::task_store::upsert(&pump_root, &record);
                            let _ = pump_events.send(BackendEvent::Task {
                                record: record.clone(),
                            });
                            dirty = false;
                            last_flush = std::time::Instant::now();
                        }
                    }
                });
                tasks.insert(task_id, Arc::clone(&session));
                tokio::spawn(async move {
                    session.send(prompt, Vec::new()).await;
                });
            }
            Command::TaskSend { task_id, text } => {
                if let Some(session) = tasks.get(&task_id).cloned() {
                    tokio::spawn(async move {
                        session.send(text, Vec::new()).await;
                    });
                }
            }
            Command::TaskStop { task_id } => {
                if let Some(session) = tasks.remove(&task_id) {
                    let data_root = crate::data::data_root();
                    if let Some(mut record) = crate::task_store::get(&data_root, &task_id) {
                        record.status = crate::task_store::TaskStatus::Stopped;
                        record.finished_at = crate::im_service::now_ms() as u64;
                        let _ = crate::task_store::upsert(&data_root, &record);
                        let _ = events.send(BackendEvent::Task { record });
                    }
                    tokio::spawn(async move {
                        session.abort().await;
                    });
                }
            }
            Command::AskBot {
                request_id: _,
                target_bot_id,
                target_bot_name,
                text,
                reply,
            } => {
                let ctx_key = format!("bot:{target_bot_id}");
                // 目标 Bot 已经有会话在跑同一份 lane：直接拒绝，避免 A→B→A 互相等。
                if sessions.contains_key(&ctx_key) && active_internal.contains_key(&ctx_key) {
                    let _ = reply.send(Err(format!("{target_bot_name} 正在执行回合，请稍后再联系")));
                    continue;
                }
                let roster = crate::data::Roster::load();
                let Some(bot) = roster.bot_by_id(&target_bot_id) else {
                    let _ = reply.send(Err(format!("找不到 Bot：{target_bot_name}")));
                    continue;
                };
                if bot.is_private {
                    let _ = reply.send(Err("目标 Bot 已设为隐私，拒绝跨 Bot 读取".to_string()));
                    continue;
                }
                let (tx, mut rx) = unbounded_channel::<BackendEvent>();
                let opened = open_rpi_session(
                    &ctx_key,
                    &bot.directory,
                    &bot.identity(),
                    bot.model.clone(),
                    None,
                    None,
                    &tx,
                    Vec::new(),
                )
                .await;
                let session = match opened {
                    Ok((session, _spec)) => Arc::new(session),
                    Err(message) => {
                        let _ = reply.send(Err(format!("{target_bot_name} 建会话失败：{message}")));
                        continue;
                    }
                };
                let sender = Arc::clone(&session);
                tokio::spawn(async move {
                    sender.send(text, Vec::new()).await;
                });
                // 自己收流式事件，攒完整段回复（不往 UI 事件流里发假 delta）。
                tokio::spawn(async move {
                    let deadline =
                        tokio::time::Instant::now() + Duration::from_secs(ASK_BOT_TIMEOUT_SECS);
                    let mut answer = String::new();
                    loop {
                        let remaining =
                            deadline.saturating_duration_since(tokio::time::Instant::now());
                        if remaining.is_zero() {
                            // 消息已送达，只是目标还没答完：交「仍在跑」，由调用方
                            // 回「已投递，稍后它会自己回复」，不报超时失败（D-11）。
                            let _ = reply.send(Ok(BotTurn::StillRunning));
                            break;
                        }
                        let Ok(Some(event)) =
                            tokio::time::timeout(remaining, rx.recv()).await
                        else {
                            let _ = reply.send(Err("目标 Bot 事件通道关闭".to_string()));
                            break;
                        };
                        let BackendEvent::Session { event, .. } = event else {
                            continue;
                        };
                        match event {
                            SessionEvent::Delta { text, .. } => answer.push_str(&text),
                            SessionEvent::Done { .. } => {
                                let _ = reply.send(Ok(BotTurn::Reply(answer)));
                                break;
                            }
                            SessionEvent::Error { message, .. } => {
                                let _ = reply.send(Err(message));
                                break;
                            }
                            _ => {}
                        }
                    }
                });
            }
            Command::Compact { key } => {
                let ctx_key =
                    active_internal.get(&key).cloned().unwrap_or_else(|| key.clone());
                let Some(ctx) = sessions.get(&ctx_key) else {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: "会话尚未建立，无法压缩".to_string(),
                        },
                    });
                    continue;
                };
                let SessionEngine::Rpi(session) = &ctx.engine else {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: "该引擎不支持手动压缩".to_string(),
                        },
                    });
                    continue;
                };
                let session = Arc::clone(session);
                let events = events.clone();
                tokio::spawn(async move {
                    // Ok = 有结论（含「上下文还很短」这种主动跳过），Err = 真失败。
                    // 曾经这里按 `✅` 前缀判断，把正常的「未压缩」也当错误弹红条（D-08）。
                    let event = match session.compact().await {
                        Ok(message) => SessionEvent::UserEcho(message),
                        Err(message) => SessionEvent::Error {
                            member: None,
                            message,
                        },
                    };
                    let _ = events.send(BackendEvent::Session { key, event });
                });
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
                root_task_id,
                root_message_id,
            } => {
                let Some(spec) = groups.get(&key).cloned() else {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: "群聊会话尚未建立".to_string(),
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
                            message: "任务负责人不在群聊成员里".to_string(),
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
                if let Some(turn) = team_turns.get(&key) {
                    crate::team_tools::note_dispatch(
                        turn,
                        root_task_id.as_deref(),
                        root_message_id.as_deref(),
                        &member.member_id,
                    );
                }
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
                    // ACP 的 prompt 支持 ImageBlock；把附件转成 (base64, mime)。
                    // 解码失败时只发文本并提示，不整条丢弃。
                    let acp_images: Vec<(String, String)> =
                        match crate::rpi_engine::images_from_attachments(&attachments) {
                            Ok(images) => images
                                .into_iter()
                                .map(|image| {
                                    (image.data.to_string(), image.mime_type.clone())
                                })
                                .collect(),
                            Err(reason) => {
                                if !attachments.is_empty() {
                                    let _ = events.send(BackendEvent::Session {
                                        key: key.clone(),
                                        event: SessionEvent::Error {
                                            member: None,
                                            message: format!(
                                                "附件未随消息发送：{reason}"
                                            ),
                                        },
                                    });
                                }
                                Vec::new()
                            }
                        };
                    let handle = handle.clone();
                    if !handle.prompt_with_images(text, acp_images) {
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
            Command::Interject {
                key,
                text,
                members,
                attachments,
            } => {
                // 私聊/旁路：路由到当前活跃会话；群聊：只发给正在流式的成员。
                let targets: Vec<(String, Option<String>)> = if let Some(spec) = groups.get(&key)
                {
                    members
                        .iter()
                        .filter_map(|name| {
                            spec.members
                                .iter()
                                .find(|member| member.name == *name)
                                .map(|member| {
                                    (
                                        member_session_key(&key, &member.member_id),
                                        Some(member.name.clone()),
                                    )
                                })
                        })
                        .collect()
                } else {
                    vec![(
                        active_internal.get(&key).cloned().unwrap_or_else(|| key.clone()),
                        None,
                    )]
                };
                if targets.is_empty() {
                    let _ = events.send(BackendEvent::Session {
                        key,
                        event: SessionEvent::Error {
                            member: None,
                            message: "当前没有可以插话的进行中回合".to_string(),
                        },
                    });
                    continue;
                }
                for (internal, member) in targets {
                    let Some(ctx) = sessions.get(&internal) else {
                        continue;
                    };
                    if let SessionEngine::Rpi(session) = &ctx.engine {
                        let session = Arc::clone(session);
                        let text = text.clone();
                        // 附件解码失败时降级为纯文本插话，不整条丢弃。
                        let images = crate::rpi_engine::images_from_attachments(&attachments)
                            .unwrap_or_default();
                        tokio::spawn(async move {
                            session.steer(text, images).await;
                        });
                    } else if let Some(member) = member {
                        let _ = events.send(BackendEvent::Session {
                            key: key.clone(),
                            event: SessionEvent::Error {
                                member: Some(member),
                                message: "该引擎不支持实时插话".to_string(),
                            },
                        });
                    }
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

/// 群聊成员会话在 sessions 表里的内部键（UI 只看 group_key）。
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
        Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 超时不能让调用方以为消息没送到（D-11）：必须走「已投递」而不是 Err。
    #[tokio::test]
    async fn timeout_reports_delivered_not_failure() {
        // 五分钟没人答 → 用极短的等待把超时路径走一遍。
        let (_tx, rx) = tokio::sync::oneshot::channel::<Result<BotTurn, String>>();
        let delivery = await_bot_reply(rx, Duration::from_millis(10), "小助手")
            .await
            .expect("超时不算失败");
        assert_eq!(
            delivery,
            BotDelivery::Delivered {
                target_bot_name: "小助手".to_string()
            }
        );
        let text = delivery.describe();
        assert!(text.contains("已投递"), "{text}");
        assert!(text.contains("不用重发"), "{text}");
        // 不能出现过时的时间值，也不能读成失败。
        assert!(!text.contains("120"), "{text}");
        assert!(!text.contains("失败"), "{text}");
    }

    #[tokio::test]
    async fn reply_and_explicit_running_both_work() {
        // 答完了：原样带回文本。
        let (tx, rx) = tokio::sync::oneshot::channel::<Result<BotTurn, String>>();
        let _ = tx.send(Ok(BotTurn::Reply("PONG".to_string())));
        let delivery = await_bot_reply(rx, Duration::from_secs(1), "小助手")
            .await
            .expect("正常回复");
        assert_eq!(delivery, BotDelivery::Reply("PONG".to_string()));

        // 后端已经知道自己等不到了：同样回「已投递」，不是错误。
        let (tx, rx) = tokio::sync::oneshot::channel::<Result<BotTurn, String>>();
        let _ = tx.send(Ok(BotTurn::StillRunning));
        let delivery = await_bot_reply(rx, Duration::from_secs(1), "小助手")
            .await
            .expect("仍在跑不算失败");
        assert!(matches!(delivery, BotDelivery::Delivered { .. }));

        // 真正的错误（隐私、找不到）照旧是 Err。
        let (tx, rx) = tokio::sync::oneshot::channel::<Result<BotTurn, String>>();
        let _ = tx.send(Err("目标 Bot 已设为隐私，拒绝跨 Bot 读取".to_string()));
        let error = await_bot_reply(rx, Duration::from_secs(1), "小助手")
            .await
            .expect_err("隐私拒绝必须是错误");
        assert!(error.contains("隐私"), "{error}");
    }

    #[test]
    fn wait_budget_is_one_minute() {
        // 太短会让本可成功的投递失败，太长等于让用户干站着。
        assert_eq!(ASK_BOT_TIMEOUT_SECS, 60);
    }
}
