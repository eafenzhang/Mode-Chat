//! 微信/Bot 会话编排最小闭环（`processProviderCallback` 的核心骨架）。
//!
//! 本轮只实现「私聊/群聊文本 → 复用或创建 v4 会话 → 发送 → 等回复 → 回投 IM」：
//! - 会话上下文按 `botId + conversationKey` 持久化到 `bot-state.bots[botId].conversations`；
//! - 工作区默认取 bot 的第一个 `allowedWorkspaces`（绑定收敛后即当前 Bot 目录）；
//! - 首次消息创建会话并记住 sessionId，后续消息复用；
//! - 回复轮询由注入的 `BotTurnOps::read_reply` 决定（None=仍在运行），超时上限由调用方给；
//! - 回复由注入的 `BotReplySink` 投递（微信实现即 `WeixinClient::send_text`）。
//!
//! 未接入（后续切片）：命令解析（/status 等）、流式分段、审批桥接、群 @ 激活策略、
//! 附件物化、task/draft 切换与队列——本骨架刻意只覆盖「能对话」的最小闭环。

use crate::media::MediaKind;
use crate::BotsRepo;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

pub type RuntimeFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationContext {
    pub bot_id: String,
    pub conversation_key: String,
    /// `private` | `group`：来自入站消息的 `chat_type`，落盘后供心跳选目标与会话列表使用。
    ///
    /// 旧数据没有该字段：读取时按 key 前缀回退（历史 key 是原始 chat id，一律回退成
    /// `private`），下一次写入即更正。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_kind: Option<String>,
    pub workspace_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_label: Option<String>,
    /// 当前入站帧的回复凭据（企微 req_id）；只在本轮内有效，不落盘。
    #[serde(skip)]
    pub provider_context_token: Option<String>,
    /// `draft` | `task`：task 表示已落到桌面可见任务（`activeTaskId` 权威）。
    #[serde(default = "default_conversation_mode")]
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_task_id: Option<String>,
}

fn default_conversation_mode() -> String {
    "draft".to_string()
}

pub struct ConversationStore {
    repo: BotsRepo,
}

impl ConversationStore {
    pub fn new(repo: BotsRepo) -> Self {
        Self { repo }
    }

    pub fn load(&self, bot_id: &str, conversation_key: &str) -> Result<Option<ConversationContext>, String> {
        let state = self.repo.read_state()?;
        let Some(entry) = state
            .get("bots")
            .and_then(|bots| bots.get(bot_id))
            .and_then(|entry| entry.get("conversations"))
            .and_then(|conversations| conversations.get(conversation_key))
        else {
            return Ok(None);
        };
        // 绑定到桌面任务的会话以 activeTaskId 为权威（TS `readContext` 同语义）：
        // 这条会话的 v4 session 就是任务本身，不能另起 sessionId。
        let active_task_id = entry
            .get("activeTaskId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let session_id = active_task_id
            .clone()
            .or_else(|| {
                entry
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            });
        let Some(session_id) = session_id else {
            return Ok(None);
        };
        let mode = entry
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("draft")
            .to_string();
        Ok(Some(ConversationContext {
            bot_id: bot_id.to_string(),
            conversation_key: conversation_key.to_string(),
            conversation_kind: entry
                .get("conversationKind")
                .and_then(Value::as_str)
                .map(str::to_string),
            workspace_path: entry
                .get("workspacePath")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            workspace_identity: entry
                .get("workspaceIdentity")
                .and_then(Value::as_str)
                .map(str::to_string),
            session_id: Some(session_id),
            conversation_label: entry
                .get("conversationLabel")
                .and_then(Value::as_str)
                .map(str::to_string),
            provider_context_token: None,
            mode,
            active_task_id,
        }))
    }

    /// 把会话提升为桌面可见任务（`mode=task` + `activeTaskId`）。
    ///
    /// TS 在草稿态首条消息时 `createTask` 并写回上下文；桌面侧据此把该会话
    /// 当成任务行，并通过 `bots:task` 广播让 UI 跟随（双向同步的绑定基础）。
    pub fn mark_task(
        &self,
        bot_id: &str,
        conversation_key: &str,
        task_id: &str,
    ) -> Result<(), String> {
        write_task_context(&self.repo, bot_id, conversation_key, task_id)?;
        Ok(())
    }

    pub fn save(&self, context: &ConversationContext) -> Result<(), String> {
        let bot_id = context.bot_id.clone();
        let key = context.conversation_key.clone();
        self.repo.mutate_state(|state| {
            let bots = state
                .get_mut("bots")
                .and_then(|value| value.as_object_mut())
                .ok_or_else(|| "bot-state 缺少 bots 对象".to_string())?;
            let entry = bots
                .entry(bot_id.clone())
                .or_insert_with(|| serde_json::json!({ "botId": bot_id }));
            let object = entry
                .as_object_mut()
                .ok_or_else(|| format!("bot-state[{bot_id}] 不是对象"))?;
            let conversations = object
                .entry("conversations")
                .or_insert_with(|| serde_json::json!({}));
            let map = conversations
                .as_object_mut()
                .ok_or_else(|| "conversations 不是对象".to_string())?;
            // 合并写：保留 TS 侧字段（queue/审批/反馈），并补齐 zod 必填项。
            let mut next = map
                .get(&key)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            next.insert("botId".into(), Value::String(bot_id.clone()));
            next.insert("conversationKey".into(), Value::String(key.clone()));
            let (fallback_kind, id) = split_conversation_key(&key);
            // 真实类型来自入站 `chat_type`；key 只是回退（历史 key 是原始 chat id，
            // 前缀推断会一律得到 private，群会话会被误判）。
            let kind = context
                .conversation_kind
                .as_deref()
                .map(str::trim)
                .filter(|kind| matches!(*kind, "private" | "group"))
                .unwrap_or(fallback_kind);
            next.insert("conversationKind".into(), Value::String(kind.into()));
            next.insert("conversationId".into(), Value::String(id));
            next.insert(
                "workspacePath".into(),
                Value::String(context.workspace_path.clone()),
            );
            match context.workspace_identity.as_deref() {
                Some(identity) => {
                    next.insert(
                        "workspaceIdentity".into(),
                        Value::String(identity.to_string()),
                    );
                }
                None => {
                    next.remove("workspaceIdentity");
                }
            }
            match context.session_id.as_deref() {
                Some(session_id) => {
                    next.insert("sessionId".into(), Value::String(session_id.to_string()));
                }
                None => {
                    next.remove("sessionId");
                }
            }
            match context.conversation_label.as_deref() {
                Some(label) => {
                    next.insert(
                        "conversationLabel".into(),
                        Value::String(label.to_string()),
                    );
                }
                None => {
                    next.remove("conversationLabel");
                }
            }
            next.entry("mode")
                .or_insert_with(|| Value::String("draft".into()));
            next.entry("activeTaskId").or_insert(Value::Null);
            next.insert("updatedAt".into(), serde_json::json!(now_millis()));
            map.insert(key.clone(), Value::Object(next));
            object.insert("updatedAt".into(), serde_json::json!(now_millis()));
            Ok(())
        })
    }
}

/// 会话键前缀：`private:<id>` / `group:<id>`；无前缀（历史数据）按私聊处理。
fn split_conversation_key(key: &str) -> (&'static str, String) {
    if let Some(id) = key.strip_prefix("group:") {
        ("group", id.to_string())
    } else if let Some(id) = key.strip_prefix("private:") {
        ("private", id.to_string())
    } else {
        ("private", key.to_string())
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// 进入草稿上下文：清掉旧任务/审批/队列（`writeDraftContext` 的状态语义）。
pub fn write_draft_context(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
    draft_options: Option<Value>,
) -> Result<Value, String> {
    mutate_conversation(repo, bot_id, conversation_key, |object| {
        object.insert("mode".into(), Value::String("draft".into()));
        object.insert("activeTaskId".into(), Value::Null);
        match &draft_options {
            Some(options) => {
                object.insert("draftOptions".into(), options.clone());
            }
            None => {
                object.remove("draftOptions");
            }
        }
        for key in [
            "pendingPermissionOptions",
            "pendingElicitation",
            "queuedMessages",
            "pendingDeliveryQueue",
            "deliveryNoticeAt",
        ] {
            object.remove(key);
        }
        Ok(())
    })
}

/// 绑定到任务上下文（`mode=task` + activeTaskId，清掉草稿选项）。
pub fn write_task_context(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
    task_id: &str,
) -> Result<Value, String> {
    if task_id.trim().is_empty() {
        return Err("taskId 不能为空".to_string());
    }
    mutate_conversation(repo, bot_id, conversation_key, |object| {
        object.insert("mode".into(), Value::String("task".into()));
        object.insert("activeTaskId".into(), Value::String(task_id.to_string()));
        object.remove("draftOptions");
        Ok(())
    })
}

/// 绑定任务并把工作区字段一并换成目标工作区（`bindBotToTask` 的单次写入口）。
pub fn write_task_context_for_workspace(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
    workspace: &crate::workspace::WorkspaceRef,
    task_id: &str,
) -> Result<Value, String> {
    if task_id.trim().is_empty() {
        return Err("taskId 不能为空".to_string());
    }
    mutate_conversation(repo, bot_id, conversation_key, |object| {
        object.insert("mode".into(), Value::String("task".into()));
        object.insert("activeTaskId".into(), Value::String(task_id.to_string()));
        object.insert(
            "workspacePath".into(),
            Value::String(workspace.workspace_path.clone()),
        );
        object.insert("workspaceId".into(), Value::String(workspace.id.clone()));
        match workspace.workspace_identity.as_deref() {
            Some(identity) => {
                object.insert("workspaceIdentity".into(), Value::String(identity.to_string()));
            }
            None => {
                object.remove("workspaceIdentity");
            }
        }
        object.remove("draftOptions");
        Ok(())
    })
}

/// 切换工作区并进入草稿：workspace 字段更新 + 清旧任务/审批/队列（`/workspace`、`/new`）。
pub fn switch_workspace_context(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
    workspace: &crate::workspace::WorkspaceRef,
    draft_options: Option<Value>,
) -> Result<Value, String> {
    mutate_conversation(repo, bot_id, conversation_key, |object| {
        object.insert(
            "workspacePath".into(),
            Value::String(workspace.workspace_path.clone()),
        );
        object.insert("workspaceId".into(), Value::String(workspace.id.clone()));
        match workspace.workspace_identity.as_deref() {
            Some(identity) => {
                object.insert("workspaceIdentity".into(), Value::String(identity.to_string()));
            }
            None => {
                object.remove("workspaceIdentity");
            }
        }
        object.insert("mode".into(), Value::String("draft".into()));
        object.insert("activeTaskId".into(), Value::Null);
        match &draft_options {
            Some(options) => {
                object.insert("draftOptions".into(), options.clone());
            }
            None => {
                object.remove("draftOptions");
            }
        }
        for key in [
            "pendingPermissionOptions",
            "pendingElicitation",
            "queuedMessages",
            "pendingDeliveryQueue",
            "deliveryNoticeAt",
        ] {
            object.remove(key);
        }
        Ok(())
    })
}

/// 标记某条 pending 审批已处理（只在 v4 ACK 成功后调用）。
pub fn mark_permission_handled(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
    request_id: &str,
    handled_at: i64,
) -> Result<(), String> {
    let request_id = request_id.to_string();
    mutate_conversation(repo, bot_id, conversation_key, |object| {
        let Some(options) = object
            .get_mut("pendingPermissionOptions")
            .and_then(|value| value.as_array_mut())
        else {
            return Ok(());
        };
        for option in options.iter_mut() {
            if option.get("requestId").and_then(Value::as_str) == Some(request_id.as_str()) {
                if let Some(object) = option.as_object_mut() {
                    object.insert("handledAt".into(), Value::Number(handled_at.into()));
                }
            }
        }
        Ok(())
    })?;
    Ok(())
}

/// 写入草稿选项的单个字段（`/mode`、`/model` 等；保留其它 draft 字段）。
pub fn set_draft_option(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
    key: &str,
    value: Value,
) -> Result<Value, String> {
    if key.trim().is_empty() {
        return Err("draft option key 不能为空".to_string());
    }
    let key = key.trim().to_string();
    mutate_conversation(repo, bot_id, conversation_key, |object| {
        let draft = object
            .entry("draftOptions")
            .or_insert_with(|| serde_json::json!({}));
        let draft_object = draft
            .as_object_mut()
            .ok_or_else(|| "draftOptions 不是对象".to_string())?;
        draft_object.insert(key.clone(), value.clone());
        Ok(())
    })
}

fn mutate_conversation(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
    mutate: impl FnOnce(&mut serde_json::Map<String, Value>) -> Result<(), String>,
) -> Result<Value, String> {
    let bot_id = bot_id.to_string();
    let key = conversation_key.to_string();
    repo.mutate_state(|state| {
        let bots = state
            .get_mut("bots")
            .and_then(|value| value.as_object_mut())
            .ok_or_else(|| "bot-state 缺少 bots 对象".to_string())?;
        let entry = bots
            .entry(bot_id.clone())
            .or_insert_with(|| serde_json::json!({ "botId": bot_id }));
        let object = entry
            .as_object_mut()
            .ok_or_else(|| format!("bot-state[{bot_id}] 不是对象"))?;
        let conversations = object
            .entry("conversations")
            .or_insert_with(|| serde_json::json!({}));
        let map = conversations
            .as_object_mut()
            .ok_or_else(|| "conversations 不是对象".to_string())?;
        let (kind, id) = split_conversation_key(&key);
        let context = map.entry(key.clone()).or_insert_with(|| {
            serde_json::json!({
                "botId": bot_id,
                "conversationKey": key,
                "conversationKind": kind,
                "conversationId": id,
                "mode": "draft",
                "activeTaskId": null,
                "updatedAt": now_millis(),
            })
        });
        let context_object = context
            .as_object_mut()
            .ok_or_else(|| "会话上下文不是对象".to_string())?;
        mutate(context_object)?;
        context_object.insert("updatedAt".into(), serde_json::json!(now_millis()));
        Ok(context.clone())
    })
}

/// v4 会话操作（宿主注入；bot-tauri 实现走 AgentControlClient）。
/// 审批请求的通道无关投影（提示文案由宿主渲染）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPermissionPrompt {
    pub request_id: String,
    pub tool_name: String,
    pub reason: String,
    pub risk_level: String,
    pub options: Vec<PermissionPromptOption>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPromptOption {
    pub option_id: String,
    /// allow_once | allow_always | deny | ...
    pub kind: String,
    pub name: String,
}

pub trait BotTurnOps: Send + Sync {
    fn create_session<'a>(
        &'a self,
        workspace_path: &'a str,
        workspace_identity: Option<&'a str>,
    ) -> RuntimeFuture<'a, Result<String, String>>;
    fn send_text<'a>(
        &'a self,
        context: &'a ConversationContext,
        text: &'a str,
    ) -> RuntimeFuture<'a, Result<(), String>>;
    /// 读取最新 assistant 文本；`None` = 回合仍在运行。
    fn read_reply<'a>(
        &'a self,
        context: &'a ConversationContext,
    ) -> RuntimeFuture<'a, Result<Option<String>, String>>;

    /// 冷恢复已存在的会话（agent 进程重启后 `sendText` 报 sessionNotFound 时调用）。
    ///
    /// 默认 `false`：测试 fake 不实现，runner 回落到新建会话；宿主实现走
    /// `session/resume`，保住连续对话而不是丢上下文重开。
    fn resume_session<'a>(
        &'a self,
        _context: &'a ConversationContext,
    ) -> RuntimeFuture<'a, Result<bool, String>> {
        Box::pin(async { Ok(false) })
    }

    /// 当前等待用户确认的权限请求（`/permission` 与审批提示用）。
    ///
    /// 默认空：测试 fake / 通道未接线；宿主实现读会话快照投影。
    fn pending_permissions<'a>(
        &'a self,
        _context: &'a ConversationContext,
    ) -> RuntimeFuture<'a, Result<Vec<PendingPermissionPrompt>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// 作答一条权限请求（v4 ResolveInteraction）。
    fn resolve_interaction<'a>(
        &'a self,
        _context: &'a ConversationContext,
        _request_id: &'a str,
        _option_id: &'a str,
    ) -> RuntimeFuture<'a, Result<(), String>> {
        Box::pin(async { Err("当前通道不支持审批作答".to_string()) })
    }

    /// 停止当前会话正在运行的生成（`/stop`）。
    ///
    /// 默认 `false`（测试 fake / 通道未接线）；宿主实现走 v4 Stop，返回是否真的发起了停止。
    fn stop<'a>(
        &'a self,
        _context: &'a ConversationContext,
    ) -> RuntimeFuture<'a, Result<bool, String>> {
        Box::pin(async { Ok(false) })
    }

    /// 回合仍在跑、但有话要对用户说（提问卡这类「必须用户作答才能继续」的中间态）。
    ///
    /// 返回 `Some(text)` 时 runner 会把它当一条普通回复发出去，**但继续轮询**：
    /// 用户答完（`/question`）后 agent 才会走到 Done，真正的答案由后面的
    /// `read_reply` 交付。默认 `None`（测试 fake / 通道未接线）。
    fn take_notice<'a>(
        &'a self,
        _context: &'a ConversationContext,
    ) -> RuntimeFuture<'a, Result<Option<String>, String>> {
        Box::pin(async { Ok(None) })
    }

    /// 作答一条提问卡（`ask_user_question`）。`answer=None` = 取消提问。
    fn resolve_question<'a>(
        &'a self,
        _context: &'a ConversationContext,
        _request_id: &'a str,
        _answer: Option<&'a str>,
    ) -> RuntimeFuture<'a, Result<(), String>> {
        Box::pin(async { Err("当前通道不支持提问作答".to_string()) })
    }

    /// 当前待作答的提问卡（`/question` 用）。
    fn pending_questions<'a>(
        &'a self,
        _context: &'a ConversationContext,
    ) -> RuntimeFuture<'a, Result<Vec<PendingQuestionPrompt>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// IM 里的提问卡（`ask_user_question`）：问题 + 选项。渲染由通道自己决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingQuestionPrompt {
    pub request_id: String,
    pub question: String,
    pub options: Vec<String>,
}

impl PendingQuestionPrompt {
    /// 通道无关的展示文本（问题 + 编号选项 + 作答提示）。
    pub fn render(&self) -> String {
        let mut lines = vec![format!("📋 {}", self.question)];
        for (index, option) in self.options.iter().enumerate() {
            lines.push(format!("{}. {option}", index + 1));
        }
        if self.options.is_empty() {
            lines.push("回复 /question <你的答案> 作答（/question 单独发送 = 取消）。".to_string());
        } else {
            lines.push("回复 /question <序号或答案> 作答（/question 单独发送 = 取消）。".to_string());
        }
        lines.join("\n")
    }
}

/// 审批提示呈现（宿主实现：把通道无关的 prompt 渲染成 IM 文本并发出去）。
pub trait BotApprovalPresenter: Send + Sync {
    fn present<'a>(
        &'a self,
        context: &'a ConversationContext,
        prompts: &'a [PendingPermissionPrompt],
    ) -> RuntimeFuture<'a, Result<(), String>>;
}

/// 出站媒体（`/send` 与后续 SendFile 共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundMedia {
    pub filename: String,
    pub kind: MediaKind,
    pub mime_type: String,
    pub bytes: Vec<u8>,
    pub caption: Option<String>,
}

/// 回复投递（微信实现 = `WeixinClient::send_text`）。
pub trait BotReplySink: Send + Sync {
    fn send_reply<'a>(
        &'a self,
        bot_id: &'a str,
        conversation_key: &'a str,
        context_token: Option<&'a str>,
        text: &'a str,
    ) -> RuntimeFuture<'a, Result<(), String>>;

    /// 发送媒体（图片/文件）。默认拒绝：未实现的通道视为不支持发送文件。
    fn send_media<'a>(
        &'a self,
        _bot_id: &'a str,
        _conversation_key: &'a str,
        _context_token: Option<&'a str>,
        _media: OutboundMedia,
    ) -> RuntimeFuture<'a, Result<(), String>> {
        Box::pin(async { Err("该通道不支持发送文件。".to_string()) })
    }
}

/// 一次任务激活（quiet 回合延迟到整轮结束再由调度方合并通知）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskActivation {
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub task_id: String,
}

/// 会话落到桌面可见任务后的通知（宿主实现：广播 `bots:task` 的 `active_task_changed`）。
///
/// 双向同步的绑定基础：桌面 UI 只有知道「这条 IM 会话 = 这个任务」才会跟随打开，
/// 并在该任务上挂回复镜像；之前在草稿态只建 session 不建任务，真机表现为
/// 「微信有回复、桌面看不到会话」。
pub trait BotTaskObserver: Send + Sync {
    fn task_active(&self, workspace_path: &str, workspace_identity: Option<&str>, task_id: &str);

    /// 一批任务同时激活：quiet 轮次整轮结束（心跳巡检收口）只通知一次。
    /// 默认逐条转发；宿主可覆盖为单次合并广播，避免跨会话连续打断。
    fn task_active_batch(&self, activations: &[TaskActivation]) {
        for activation in activations {
            self.task_active(
                &activation.workspace_path,
                activation.workspace_identity.as_deref(),
                &activation.task_id,
            );
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundTurn {
    pub bot_id: String,
    pub conversation_key: String,
    /// 入站消息的 `chat_type`（`private` | `group`）；心跳等主动回合按目标会话已有类型传。
    pub conversation_kind: Option<String>,
    pub text: String,
    pub provider_context_token: Option<String>,
    /// 未绑定时使用的默认工作区（通常为 bot.allowedWorkspaces[0]）。
    pub default_workspace: String,
    pub default_workspace_identity: Option<String>,
    /// 心跳/例程：整段回复只有 HEARTBEAT_OK 时不投递。
    pub quiet: bool,
}

fn is_missing_session_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("sessionnotfound") || lower.contains("session not found")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnOutcome {
    pub session_id: String,
    pub created: bool,
    pub reply: String,
    pub polls: usize,
    /// quiet 回合收到 HEARTBEAT_OK 时为 true（未调用 reply sink）。
    pub suppressed: bool,
    /// quiet 回合有实质回复时返回，由调度方在整轮收口合并通知。
    pub task_activation: Option<TaskActivation>,
}

pub struct ConversationRunner<'a> {
    store: &'a ConversationStore,
    ops: &'a dyn BotTurnOps,
    sink: &'a dyn BotReplySink,
    observer: Option<&'a dyn BotTaskObserver>,
    poll_interval: Duration,
    max_polls: usize,
}

impl<'a> ConversationRunner<'a> {
    pub fn new(
        store: &'a ConversationStore,
        ops: &'a dyn BotTurnOps,
        sink: &'a dyn BotReplySink,
    ) -> Self {
        Self {
            store,
            ops,
            sink,
            observer: None,
            poll_interval: Duration::from_secs(1),
            max_polls: 240,
        }
    }

    pub fn with_task_observer(mut self, observer: &'a dyn BotTaskObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    pub fn with_polling(mut self, interval: Duration, max_polls: usize) -> Self {
        self.poll_interval = interval;
        self.max_polls = max_polls;
        self
    }

    /// 草稿会话首次建/复用 session 后，把它登记成桌面任务并通知 UI。
    ///
    /// 返回 `true` = 本次发生了 draft→task（或任务 session 被重建），调用方应广播。
    fn promote_to_task(&self, context: &mut ConversationContext) -> Result<bool, String> {
        let Some(session_id) = context
            .session_id
            .clone()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(false);
        };
        if context.mode == "task" && context.active_task_id.as_deref() == Some(session_id.as_str()) {
            return Ok(false);
        }
        self.store
            .mark_task(&context.bot_id, &context.conversation_key, &session_id)?;
        context.mode = "task".to_string();
        context.active_task_id = Some(session_id);
        Ok(true)
    }

    fn notify_task_active(&self, context: &ConversationContext) {
        let (Some(observer), Some(task_id)) = (self.observer, context.active_task_id.as_deref())
        else {
            return;
        };
        observer.task_active(
            &context.workspace_path,
            context.workspace_identity.as_deref(),
            task_id,
        );
    }

    /// quiet 回合不中途通知，返回激活信息给调度方整轮合并。
    fn deferred_activation(context: &ConversationContext) -> Option<TaskActivation> {
        context.active_task_id.as_ref().map(|task_id| TaskActivation {
            workspace_path: context.workspace_path.clone(),
            workspace_identity: context.workspace_identity.clone(),
            task_id: task_id.clone(),
        })
    }

    pub async fn handle(&self, turn: InboundTurn) -> Result<TurnOutcome, String> {
        if turn.bot_id.trim().is_empty() || turn.conversation_key.trim().is_empty() {
            return Err("bot_id/conversation_key 不能为空".to_string());
        }
        let mut context = match self
            .store
            .load(&turn.bot_id, &turn.conversation_key)?
        {
            Some(context) if !context.workspace_path.trim().is_empty() => context,
            _ => ConversationContext {
                bot_id: turn.bot_id.clone(),
                conversation_key: turn.conversation_key.clone(),
                conversation_kind: turn.conversation_kind.clone(),
                workspace_path: turn.default_workspace.clone(),
                workspace_identity: turn.default_workspace_identity.clone(),
                session_id: None,
                conversation_label: None,
                provider_context_token: None,
                mode: "draft".to_string(),
                active_task_id: None,
            },
        };
        // 入站消息的 `chat_type` 是权威：旧数据按 key 前缀可能存成 private，这里随即更正。
        let previous_kind = context.conversation_kind.clone();
        if let Some(kind) = turn
            .conversation_kind
            .as_deref()
            .map(str::trim)
            .filter(|kind| matches!(*kind, "private" | "group"))
        {
            context.conversation_kind = Some(kind.to_string());
        }
        context.provider_context_token = turn.provider_context_token.clone();
        let created = match context.session_id.clone() {
            Some(session_id) if !session_id.trim().is_empty() => false,
            _ => {
                if context.workspace_path.trim().is_empty() {
                    return Err(format!(
                        "Bot {} 没有可用工作区（allowedWorkspaces 为空）",
                        turn.bot_id
                    ));
                }
                let session_id = self
                    .ops
                    .create_session(
                        &context.workspace_path,
                        context.workspace_identity.as_deref(),
                    )
                    .await?;
                context.session_id = Some(session_id);
                self.store.save(&context)?;
                true
            }
        };
        // 已有会话的类型被更正（旧版本按 key 前缀存错过）：写入一次，否则心跳永远把群当私聊。
        if !created && previous_kind != context.conversation_kind {
            // 这一写只影响投影里的展示/心跳选择，失败不回滚回合（下一轮还会再试）。
            let _ = self.store.save(&context);
        }
        // 先落任务上下文（后续 IM 消息直接进这个任务），但等首条消息受理后再通知 UI：
        // createSession 是 deferred 草稿，此时订阅只会拿到空快照；桌面真机表现为
        // 「切过去是空白，重启才看到消息」。
        let promoted = self.promote_to_task(&mut context)?;
        if let Err(error) = self.ops.send_text(&context, &turn.text).await {
            if !is_missing_session_error(&error) {
                return Err(error);
            }
            // 会话在 agent 进程重启后可能不存在（proto.sessionNotFound）。先冷恢复原会话
            // （保留上下文）；恢复失败才新建，否则每条消息都会换一个新会话且游标不推进。
            let mut resumed = false;
            if self.ops.resume_session(&context).await.unwrap_or(false) {
                resumed = self.ops.send_text(&context, &turn.text).await.is_ok();
            }
            if resumed {
                if promoted && !turn.quiet {
                    self.notify_task_active(&context);
                }
            } else {
                let session_id = self
                    .ops
                    .create_session(
                        &context.workspace_path,
                        context.workspace_identity.as_deref(),
                    )
                    .await?;
                context.session_id = Some(session_id);
                self.store.save(&context)?;
                // 任务 session 重建后 activeTaskId 也要换（旧任务已经不可用），并让桌面跟随。
                let repromoted = self.promote_to_task(&mut context)?;
                self.ops.send_text(&context, &turn.text).await?;
                if !turn.quiet && (promoted || repromoted) {
                    self.notify_task_active(&context);
                }
            }
        } else if promoted && !turn.quiet {
            self.notify_task_active(&context);
        }
        let mut polls = 0usize;
        while polls < self.max_polls {
            polls += 1;
            tokio::time::sleep(self.poll_interval).await;
            // 提问卡这类「回合内必须用户作答」的中间态：先把问题发出去，回合继续跑，
            // 用户答完（`resolve_question`）后 agent 才会 Done。
            // 注意：本 crate 是 edition 2021，let-chain 不可用，这里用嵌套 if。
            if let Ok(Some(notice)) = self.ops.take_notice(&context).await {
                if !notice.trim().is_empty() {
                    self.sink
                        .send_reply(
                            &turn.bot_id,
                            &turn.conversation_key,
                            turn.provider_context_token.as_deref(),
                            &notice,
                        )
                        .await?;
                }
            }
            match self.ops.read_reply(&context).await? {
                Some(reply) if !reply.trim().is_empty() => {
                    if turn.quiet && crate::heartbeat::is_heartbeat_ok_only(&reply) {
                        return Ok(TurnOutcome {
                            session_id: context.session_id.unwrap_or_default(),
                            created,
                            reply: String::new(),
                            polls,
                            suppressed: true,
                            task_activation: None,
                        });
                    }
                    let task_activation = if turn.quiet {
                        Self::deferred_activation(&context)
                    } else {
                        None
                    };
                    self.sink
                        .send_reply(
                            &turn.bot_id,
                            &turn.conversation_key,
                            turn.provider_context_token.as_deref(),
                            &reply,
                        )
                        .await?;
                    return Ok(TurnOutcome {
                        session_id: context.session_id.unwrap_or_default(),
                        created,
                        reply,
                        polls,
                        suppressed: false,
                        task_activation,
                    });
                }
                _ => continue,
            }
        }
        Err(format!(
            "Bot 回合超时：bot={} conversation={} polls={}",
            turn.bot_id, turn.conversation_key, polls
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("conversation-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct FakeOps {
        sessions: Mutex<Vec<String>>,
        sent: Mutex<Vec<String>>,
        replies: Mutex<Vec<Option<String>>>,
    }

    impl BotTurnOps for FakeOps {
        fn create_session<'a>(
            &'a self,
            workspace_path: &'a str,
            _identity: Option<&'a str>,
        ) -> RuntimeFuture<'a, Result<String, String>> {
            let mut sessions = self.sessions.lock().unwrap();
            let id = format!("sess-{}-{}", workspace_path.len(), sessions.len());
            sessions.push(workspace_path.to_string());
            Box::pin(async move { Ok(id) })
        }

        fn send_text<'a>(
            &'a self,
            _context: &'a ConversationContext,
            text: &'a str,
        ) -> RuntimeFuture<'a, Result<(), String>> {
            self.sent.lock().unwrap().push(text.to_string());
            Box::pin(async move { Ok(()) })
        }

        fn read_reply<'a>(
            &'a self,
            _context: &'a ConversationContext,
        ) -> RuntimeFuture<'a, Result<Option<String>, String>> {
            let reply = if self.replies.lock().unwrap().is_empty() {
                None
            } else {
                self.replies.lock().unwrap().remove(0)
            };
            Box::pin(async move { Ok(reply) })
        }
    }

    struct FakeSink(Mutex<Vec<(String, String)>>);

    impl BotReplySink for FakeSink {
        fn send_reply<'a>(
            &'a self,
            _bot_id: &'a str,
            conversation_key: &'a str,
            _context_token: Option<&'a str>,
            text: &'a str,
        ) -> RuntimeFuture<'a, Result<(), String>> {
            self.0
                .lock()
                .unwrap()
                .push((conversation_key.to_string(), text.to_string()));
            Box::pin(async move { Ok(()) })
        }
    }

    fn turn(key: &str, text: &str) -> InboundTurn {
        InboundTurn {
            bot_id: "b1".to_string(),
            conversation_key: key.to_string(),
            conversation_kind: None,
            text: text.to_string(),
            provider_context_token: Some("ctx".to_string()),
            default_workspace: "D:/ws".to_string(),
            default_workspace_identity: None,
            quiet: false,
        }
    }

    #[tokio::test]
    async fn first_turn_creates_session_and_sends_reply() {
        let base = sandbox("first");
        let store = ConversationStore::new(BotsRepo::new(&base));
        let ops = FakeOps {
            sessions: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            replies: Mutex::new(vec![None, Some("回复".to_string())]),
        };
        let sink = FakeSink(Mutex::new(Vec::new()));
        let runner = ConversationRunner::new(&store, &ops, &sink)
            .with_polling(Duration::from_millis(1), 5);
        let outcome = runner.handle(turn("u1", "你好")).await.unwrap();
        assert!(outcome.created);
        assert_eq!(outcome.polls, 2);
        assert_eq!(outcome.reply, "回复");
        assert_eq!(ops.sent.lock().unwrap().as_slice(), ["你好"]);
        assert_eq!(sink.0.lock().unwrap().as_slice(), [("u1".to_string(), "回复".to_string())]);
        // 上下文落盘：同一会话再进消息复用 sessionId。
        ops.replies
            .lock()
            .unwrap()
            .push(Some("再回复".to_string()));
        let second = runner.handle(turn("u1", "继续")).await.unwrap();
        assert!(!second.created);
        assert_eq!(second.session_id, outcome.session_id);
        assert_eq!(ops.sessions.lock().unwrap().len(), 1);
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn quiet_heartbeat_ok_reply_is_suppressed() {
        let base = sandbox("quiet-ok");
        let store = ConversationStore::new(BotsRepo::new(&base));
        let ops = FakeOps {
            sessions: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            replies: Mutex::new(vec![Some("**HEARTBEAT_OK**".to_string())]),
        };
        let sink = FakeSink(Mutex::new(Vec::new()));
        let runner = ConversationRunner::new(&store, &ops, &sink)
            .with_polling(Duration::from_millis(1), 3);
        let mut turn = turn("u1", "[心跳] 主动检查");
        turn.quiet = true;
        let outcome = runner.handle(turn).await.unwrap();
        assert!(outcome.suppressed);
        assert_eq!(outcome.reply, "");
        assert!(outcome.task_activation.is_none());
        assert!(sink.0.lock().unwrap().is_empty());
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn conversation_kind_follows_chat_type_not_key_prefix() {
        // 入站 key 是原始 chat id（微信企微都一样），前缀推断会一律得到 private；
        // 群会话必须按 chat_type 落成 group，否则心跳会把群当成私聊目标。
        let base = sandbox("kind-chat-type");
        let store = ConversationStore::new(BotsRepo::new(&base));
        let ops = FakeOps {
            sessions: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            replies: Mutex::new(vec![None, Some("收到".to_string()), Some("改判".to_string())]),
        };
        let sink = FakeSink(Mutex::new(Vec::new()));
        let runner = ConversationRunner::new(&store, &ops, &sink)
            .with_polling(Duration::from_millis(1), 5);
        let mut group_turn = turn("room-1", "群里说一句");
        group_turn.conversation_kind = Some("group".to_string());
        runner.handle(group_turn).await.unwrap();
        let loaded = store.load("b1", "room-1").unwrap().unwrap();
        assert_eq!(loaded.conversation_kind.as_deref(), Some("group"));

        // 已有会话再收到私聊类型（同一 key 的类型更正）时随即覆盖。
        let mut private_turn = turn("room-1", "改判为私聊");
        private_turn.conversation_kind = Some("private".to_string());
        runner.handle(private_turn).await.unwrap();
        let loaded = store.load("b1", "room-1").unwrap().unwrap();
        assert_eq!(loaded.conversation_kind.as_deref(), Some("private"));
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn quiet_heartbeat_with_content_is_delivered() {
        let base = sandbox("quiet-content");
        let store = ConversationStore::new(BotsRepo::new(&base));
        let ops = FakeOps {
            sessions: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            replies: Mutex::new(vec![Some("有更新：测试失败".to_string())]),
        };
        let sink = FakeSink(Mutex::new(Vec::new()));
        let runner = ConversationRunner::new(&store, &ops, &sink)
            .with_polling(Duration::from_millis(1), 3);
        let mut turn = turn("u1", "[心跳] 主动检查");
        turn.quiet = true;
        let outcome = runner.handle(turn).await.unwrap();
        assert!(!outcome.suppressed);
        assert_eq!(outcome.reply, "有更新：测试失败");
        assert_eq!(
            outcome
                .task_activation
                .as_ref()
                .map(|activation| activation.task_id.as_str()),
            Some(outcome.session_id.as_str()),
            "quiet 实质回复应由调度方整轮合并通知"
        );
        assert_eq!(
            sink.0.lock().unwrap().as_slice(),
            [("u1".to_string(), "有更新：测试失败".to_string())]
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn quiet_turn_defers_task_notification_to_batch() {
        let base = sandbox("promote-quiet");
        let store = ConversationStore::new(BotsRepo::new(&base));
        let ops = FakeOps {
            sessions: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            replies: Mutex::new(vec![Some("有更新".to_string())]),
        };
        let sink = FakeSink(Mutex::new(Vec::new()));
        let observer = RecordingObserver(Mutex::new(Vec::new()));
        let runner = ConversationRunner::new(&store, &ops, &sink)
            .with_task_observer(&observer)
            .with_polling(Duration::from_millis(1), 5);
        let mut quiet = turn("u1", "[心跳] 主动检查");
        quiet.quiet = true;
        let outcome = runner.handle(quiet).await.unwrap();
        assert!(
            observer.0.lock().unwrap().is_empty(),
            "quiet 回合不应中途打断桌面"
        );
        let activation = outcome.task_activation.expect("应返回待通知任务");
        assert_eq!(activation.task_id, outcome.session_id);
        observer.task_active_batch(&[activation]);
        assert_eq!(observer.0.lock().unwrap().len(), 1);
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn suppressed_quiet_turn_has_no_task_activation() {
        let base = sandbox("suppress-quiet");
        let store = ConversationStore::new(BotsRepo::new(&base));
        let ops = FakeOps {
            sessions: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            replies: Mutex::new(vec![Some("HEARTBEAT_OK".to_string())]),
        };
        let sink = FakeSink(Mutex::new(Vec::new()));
        let observer = RecordingObserver(Mutex::new(Vec::new()));
        let runner = ConversationRunner::new(&store, &ops, &sink)
            .with_task_observer(&observer)
            .with_polling(Duration::from_millis(1), 5);
        let mut quiet = turn("u1", "[心跳] 主动检查");
        quiet.quiet = true;
        let outcome = runner.handle(quiet).await.unwrap();
        assert!(outcome.suppressed);
        assert!(outcome.task_activation.is_none());
        assert!(observer.0.lock().unwrap().is_empty());
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn task_bound_conversation_uses_active_task_and_skips_create() {
        let base = sandbox("bound");
        let store = ConversationStore::new(BotsRepo::new(&base));
        let repo = BotsRepo::new(&base);
        repo.mutate_state(|state| {
            state["bots"]["b1"]["conversations"]["u1"] = serde_json::json!({
                "botId": "b1",
                "conversationKey": "u1",
                "workspacePath": "D:/ws",
                "mode": "task",
                // 桌面绑定只写 activeTaskId，没有 sessionId：会话必须直接进这个任务。
                "activeTaskId": "sess-desktop",
            });
            Ok(())
        })
        .unwrap();
        let ops = FakeOps {
            sessions: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            replies: Mutex::new(vec![Some("回复".to_string())]),
        };
        let sink = FakeSink(Mutex::new(Vec::new()));
        let runner = ConversationRunner::new(&store, &ops, &sink)
            .with_polling(Duration::from_millis(1), 5);
        let outcome = runner.handle(turn("u1", "你好")).await.unwrap();
        assert!(!outcome.created);
        assert_eq!(outcome.session_id, "sess-desktop");
        assert!(ops.sessions.lock().unwrap().is_empty(), "绑定任务不应新建 session");
        std::fs::remove_dir_all(base).ok();
    }

    struct RecordingObserver(Mutex<Vec<(String, Option<String>, String)>>);

    impl BotTaskObserver for RecordingObserver {
        fn task_active(
            &self,
            workspace_path: &str,
            workspace_identity: Option<&str>,
            task_id: &str,
        ) {
            self.0.lock().unwrap().push((
                workspace_path.to_string(),
                workspace_identity.map(str::to_string),
                task_id.to_string(),
            ));
        }
    }

    #[tokio::test]
    async fn draft_turn_promotes_to_task_and_notifies_observer() {
        let base = sandbox("promote");
        let store = ConversationStore::new(BotsRepo::new(&base));
        let ops = FakeOps {
            sessions: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            replies: Mutex::new(vec![Some("回复".to_string())]),
        };
        let sink = FakeSink(Mutex::new(Vec::new()));
        let observer = RecordingObserver(Mutex::new(Vec::new()));
        let runner = ConversationRunner::new(&store, &ops, &sink)
            .with_task_observer(&observer)
            .with_polling(Duration::from_millis(1), 5);
        let outcome = runner.handle(turn("u1", "你好")).await.unwrap();
        let state = BotsRepo::new(&base).read_state().unwrap();
        let context = &state["bots"]["b1"]["conversations"]["u1"];
        assert_eq!(context["mode"], "task");
        assert_eq!(context["activeTaskId"], outcome.session_id.as_str());
        assert_eq!(
            observer.0.lock().unwrap().as_slice(),
            [("D:/ws".to_string(), None, outcome.session_id.clone())],
            "草稿首回合必须通知桌面跟随该任务"
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn draft_context_clears_task_pending_and_queue() {
        let base = sandbox("draft");
        let repo = BotsRepo::new(&base);
        repo.mutate_state(|state| {
            state["bots"]["b1"]["conversations"]["c1"] = serde_json::json!({
                "botId": "b1",
                "conversationKey": "c1",
                "workspacePath": "D:/ws",
                "mode": "task",
                "activeTaskId": "task-1",
                "queuedMessages": [{ "text": "queued" }],
                "pendingPermissionOptions": [{ "requestId": "r1" }],
                "pendingElicitation": { "requestId": "e1" }
            });
            Ok(())
        })
        .unwrap();
        let context = write_draft_context(&repo, "b1", "c1", Some(serde_json::json!({ "model": "m" })))
            .unwrap();
        assert_eq!(context["mode"], "draft");
        assert!(context["activeTaskId"].is_null());
        assert_eq!(context["workspacePath"], "D:/ws");
        assert_eq!(context["draftOptions"]["model"], "m");
        assert!(context.get("queuedMessages").is_none());
        assert!(context.get("pendingPermissionOptions").is_none());
        assert!(context.get("pendingElicitation").is_none());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn task_context_sets_mode_and_task_id() {
        let base = sandbox("task");
        let repo = BotsRepo::new(&base);
        let context = write_task_context(&repo, "b1", "c1", "task-9").unwrap();
        assert_eq!(context["mode"], "task");
        assert_eq!(context["activeTaskId"], "task-9");
        assert!(context.get("draftOptions").is_none());
        assert!(write_task_context(&repo, "b1", "c1", " ").is_err());
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn missing_workspace_is_explicit_error() {
        let base = sandbox("no-workspace");
        let store = ConversationStore::new(BotsRepo::new(&base));
        let ops = FakeOps {
            sessions: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            replies: Mutex::new(Vec::new()),
        };
        let sink = FakeSink(Mutex::new(Vec::new()));
        let runner = ConversationRunner::new(&store, &ops, &sink)
            .with_polling(Duration::from_millis(1), 1);
        let mut input = turn("u1", "hi");
        input.default_workspace = String::new();
        let error = runner.handle(input).await.unwrap_err();
        assert!(error.contains("没有可用工作区"), "{error}");
        std::fs::remove_dir_all(base).ok();
    }
}


#[cfg(test)]
mod session_recovery_tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "conversation-recover-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct MissingSessionOps {
        sends: Mutex<u32>,
        replies: Mutex<Vec<Option<String>>>,
    }

    impl BotTurnOps for MissingSessionOps {
        fn create_session<'a>(
            &'a self,
            _workspace_path: &'a str,
            _identity: Option<&'a str>,
        ) -> RuntimeFuture<'a, Result<String, String>> {
            Box::pin(async move { Ok("sess-new".to_string()) })
        }

        fn send_text<'a>(
            &'a self,
            _context: &'a ConversationContext,
            _text: &'a str,
        ) -> RuntimeFuture<'a, Result<(), String>> {
            let mut sends = self.sends.lock().unwrap();
            *sends += 1;
            let first = *sends == 1;
            Box::pin(async move {
                if first {
                    Err("sendText 未受理：Rejected reason=Some(\"proto.sessionNotFound\")".to_string())
                } else {
                    Ok(())
                }
            })
        }

        fn read_reply<'a>(
            &'a self,
            _context: &'a ConversationContext,
        ) -> RuntimeFuture<'a, Result<Option<String>, String>> {
            let reply = self.replies.lock().unwrap().pop().unwrap_or(None);
            Box::pin(async move { Ok(reply) })
        }
    }

    struct Sink;
    impl BotReplySink for Sink {
        fn send_reply<'a>(
            &'a self,
            _bot_id: &'a str,
            _conversation_key: &'a str,
            _context_token: Option<&'a str>,
            _text: &'a str,
        ) -> RuntimeFuture<'a, Result<(), String>> {
            Box::pin(async move { Ok(()) })
        }
    }

    struct ResumableOps {
        sends: Mutex<u32>,
        created: Mutex<u32>,
        resumed: Mutex<bool>,
        replies: Mutex<Vec<Option<String>>>,
    }

    impl BotTurnOps for ResumableOps {
        fn create_session<'a>(
            &'a self,
            _workspace_path: &'a str,
            _identity: Option<&'a str>,
        ) -> RuntimeFuture<'a, Result<String, String>> {
            *self.created.lock().unwrap() += 1;
            Box::pin(async move { Ok("sess-new".to_string()) })
        }

        fn send_text<'a>(
            &'a self,
            _context: &'a ConversationContext,
            _text: &'a str,
        ) -> RuntimeFuture<'a, Result<(), String>> {
            *self.sends.lock().unwrap() += 1;
            let resumed = *self.resumed.lock().unwrap();
            Box::pin(async move {
                if resumed {
                    Ok(())
                } else {
                    Err("Rejected reason=Some(\"proto.sessionNotFound\")".to_string())
                }
            })
        }

        fn read_reply<'a>(
            &'a self,
            _context: &'a ConversationContext,
        ) -> RuntimeFuture<'a, Result<Option<String>, String>> {
            let reply = self.replies.lock().unwrap().pop().unwrap_or(None);
            Box::pin(async move { Ok(reply) })
        }

        fn resume_session<'a>(
            &'a self,
            _context: &'a ConversationContext,
        ) -> RuntimeFuture<'a, Result<bool, String>> {
            *self.resumed.lock().unwrap() = true;
            Box::pin(async move { Ok(true) })
        }
    }

    #[tokio::test]
    async fn session_not_found_resumes_before_recreating() {
        let base = sandbox("resume");
        let store = ConversationStore::new(BotsRepo::new(&base));
        store
            .save(&ConversationContext {
                bot_id: "b1".to_string(),
                conversation_key: "u1".to_string(),
                conversation_kind: None,
                workspace_path: "D:/ws".to_string(),
                workspace_identity: None,
                session_id: Some("sess-old".to_string()),
                conversation_label: None,
                provider_context_token: None,
                mode: "task".to_string(),
                active_task_id: Some("sess-old".to_string()),
            })
            .unwrap();
        let ops = ResumableOps {
            sends: Mutex::new(0),
            created: Mutex::new(0),
            resumed: Mutex::new(false),
            replies: Mutex::new(vec![Some("回复".to_string())]),
        };
        let runner = ConversationRunner::new(&store, &ops, &Sink)
            .with_polling(Duration::from_millis(1), 5);
        let outcome = runner
            .handle(InboundTurn {
                bot_id: "b1".to_string(),
                conversation_key: "u1".to_string(),
                conversation_kind: None,
                text: "hi".to_string(),
                provider_context_token: None,
                default_workspace: "D:/ws".to_string(),
                default_workspace_identity: None,
                quiet: false,
            })
            .await
            .unwrap();
        assert_eq!(outcome.session_id, "sess-old", "冷恢复必须留在原会话");
        assert_eq!(*ops.created.lock().unwrap(), 0, "恢复成功不应新建会话");
        assert_eq!(*ops.sends.lock().unwrap(), 2);
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn session_not_found_recreates_and_retries() {
        let base = sandbox("retry");
        let store = ConversationStore::new(BotsRepo::new(&base));
        store
            .save(&ConversationContext {
                bot_id: "b1".to_string(),
                conversation_key: "u1".to_string(),
                conversation_kind: None,
                workspace_path: "D:/ws".to_string(),
                workspace_identity: None,
                session_id: Some("sess-old".to_string()),
                conversation_label: None,
                provider_context_token: None,
                mode: "draft".to_string(),
                active_task_id: None,
            })
            .unwrap();
        let ops = MissingSessionOps {
            sends: Mutex::new(0),
            replies: Mutex::new(vec![Some("回复".to_string())]),
        };
        let runner = ConversationRunner::new(&store, &ops, &Sink)
            .with_polling(Duration::from_millis(1), 5);
        let outcome = runner
            .handle(InboundTurn {
                bot_id: "b1".to_string(),
                conversation_key: "u1".to_string(),
                conversation_kind: None,
                text: "hi".to_string(),
                provider_context_token: None,
                default_workspace: "D:/ws".to_string(),
                default_workspace_identity: None,
                quiet: false,
            })
            .await
            .unwrap();
        assert_eq!(outcome.session_id, "sess-new");
        assert_eq!(outcome.reply, "回复");
        assert_eq!(*ops.sends.lock().unwrap(), 2);
        let stored = store.load("b1", "u1").unwrap().unwrap();
        assert_eq!(stored.session_id.as_deref(), Some("sess-new"));
        std::fs::remove_dir_all(base).ok();
    }
}
