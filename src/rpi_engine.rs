//! 内嵌 pi-rust 引擎：每个聊天会话一个 `AgentHarness`，进程内运行。
//!
//! 设计要点：
//! - 会话落盘为 pi-rust 的 JSONL（`{data_root}/v2/mode-chat-sessions`），不再依赖
//!   Node/TS agent 运行时；
//! - 工具用 pi-rust 内置的 read/write/edit/bash，审批走 `before_tool_call` 钩子，
//!   复用 `bot_team::approval` 的 ask/auto/full + 白名单矩阵；
//! - 流式事件经自定义 `AgentEmitter` 直接映射成 `BackendEvent`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use rpi_agent::{
    AgentEmitter, AgentEvent, AgentTool, AgentToolResult, BeforeToolCall, BeforeToolCallContext,
    BeforeToolCallResult,
};
use rpi_ai::providers::anthropic::AnthropicProvider;
use rpi_ai::providers::openai_completions::OpenAiCompletionsProvider;
use rpi_ai::providers::openai_responses::OpenAiResponsesProvider;
use rpi_ai::{
    Api, AssistantMessageEvent, ImageContent, InputModality, Model, Provider, ThinkingLevel,
};
use rpi_harness::agent_harness::{AgentHarness, AgentLane};
use rpi_harness::session::jsonl::{
    JsonlSessionCreateOptions, JsonlSessionRepo, JsonlSessionRepoOptions, JsonlSessionStorage,
};
use rpi_harness::session::{DefaultIdGenerator, Session, SystemClock};
use rpi_harness::types::{AgentHarnessOptions, HarnessTool};
use rpi_tools::{
    create_bash_tool, create_edit_tool, create_read_tool, create_write_tool, ExecutionToolContext,
    OsExecutionEnv,
};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::backend::{
    AttachmentUpload, BackendEvent, PermissionPrompt, PromptOption, SessionEvent, ToolCard,
};
use crate::data::ModelSpec;

/// 内置 provider 模板目录（与 Mode 桌面端同一份；用于解析 templateId 的 api/baseUrl）。
pub(crate) const BUILTIN_CATALOG: &str = include_str!("../assets/provider/mode-builtin.json");

const IMAGE_MAX_BYTES: usize = 262_144;
const IMAGE_MAX_COUNT: usize = 4;

pub struct EngineModel {
    pub model: Model,
    pub provider: Arc<dyn Provider>,
}

/// 从 `{data_root}/v2/provider_config.json` + 内置模板目录解析出可用的 Model/Provider。
pub fn resolve_model(spec: &ModelSpec) -> Result<EngineModel, String> {
    let path = crate::data::data_root()
        .join("v2")
        .join("provider_config.json");
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("provider_config.json 读取失败（{error}）"))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("provider_config.json 解析失败（{error}）"))?;
    let config = value
        .get("config")
        .ok_or_else(|| "provider_config.json 缺少 config".to_string())?;
    let rules = config
        .pointer("/providerConfigRules/providerRules")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "provider_config.json 缺少 providerRules".to_string())?;
    let rule = rules
        .iter()
        .find(|rule| {
            rule.get("providerId").and_then(|value| value.as_str()) == Some(&spec.provider_id)
        })
        .ok_or_else(|| format!("provider 未配置：{}", spec.provider_id))?;
    let provider_config = rule
        .get("config")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Object(Default::default()));
    let api_key = provider_config
        .pointer("/access/apiKey")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if api_key.is_empty() {
        return Err(format!("provider 未配置 API Key：{}", spec.provider_id));
    }
    let template_id = rule
        .get("templateId")
        .and_then(|value| value.as_str())
        .unwrap_or("");

    let builtin: serde_json::Value = serde_json::from_str(BUILTIN_CATALOG)
        .map_err(|error| format!("内置 provider 模板解析失败（{error}）"))?;
    let templates = builtin
        .pointer("/config/providerConfigRules/templateRules")
        .and_then(|value| value.as_array());
    let template = templates.and_then(|items| {
        items.iter().find(|item| {
            item.get("templateId").and_then(|value| value.as_str()) == Some(template_id)
        })
    });

    let base_url = provider_config
        .pointer("/baseUrl")
        .and_then(|value| value.as_str())
        .or_else(|| provider_config.pointer("/api/baseUrl").and_then(|value| value.as_str()))
        .or_else(|| {
            template
                .and_then(|item| item.pointer("/config/api/baseUrl"))
                .and_then(|value| value.as_str())
        })
        .ok_or_else(|| format!("provider 缺少 baseUrl：{}", spec.provider_id))?
        .trim_end_matches('/')
        .to_string();

    let api_type = provider_config
        .pointer("/api/type")
        .and_then(|value| value.as_str())
        .or_else(|| {
            template
                .and_then(|item| item.pointer("/config/api/type"))
                .and_then(|value| value.as_str())
        })
        .unwrap_or("anthropic-messages");

    let (api, provider_id) = match api_type {
        "anthropic-messages" => (Api::AnthropicMessages, "anthropic"),
        "openai-responses" => (Api::OpenaiResponses, "mode"),
        "openai-completions" => (Api::OpenaiCompletions, "mode"),
        other => return Err(format!("不支持的 provider api 类型：{other}")),
    };

    let mut model = Model::new(
        spec.model_id.clone(),
        spec.model_id.clone(),
        api.clone(),
        provider_id,
        base_url,
    );
    model.input = vec![InputModality::Text, InputModality::Image];
    // 窗口/输出上限：用户显式值 > 内置模板 > 命名兜底常量（见 `src/model_limits.rs`）。
    let limits = crate::model_limits::read(&spec.provider_id, &spec.model_id);
    let (context_window, max_tokens) = crate::model_limits::resolve(&limits);
    model.context_window = context_window.max(1) as u64;
    model.max_tokens = max_tokens.max(1) as u64;
    model.reasoning = spec.reasoning_level.is_some();

    let http = reqwest012::Client::builder()
        .build()
        .map_err(|error| format!("HTTP 客户端初始化失败（{error}）"))?;
    let provider: Arc<dyn Provider> = match api {
        Api::AnthropicMessages => Arc::new(AnthropicProvider::with_models(
            Some(api_key),
            http,
            vec![model.clone()],
        )),
        Api::OpenaiResponses => Arc::new(OpenAiResponsesProvider::with_models(
            "mode",
            Some(api_key),
            http,
            vec![model.clone()],
        )),
        _ => Arc::new(OpenAiCompletionsProvider::with_models(
            "mode",
            Some(api_key),
            http,
            vec![model.clone()],
        )),
    };

    Ok(EngineModel { model, provider })
}

fn thinking_level(value: Option<&str>) -> ThinkingLevel {
    match value.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "minimal" => ThinkingLevel::Minimal,
        "low" => ThinkingLevel::Low,
        "medium" => ThinkingLevel::Medium,
        "high" => ThinkingLevel::High,
        "xhigh" | "x-high" => ThinkingLevel::Xhigh,
        "max" => ThinkingLevel::Max,
        _ => ThinkingLevel::Off,
    }
}

pub fn thinking_level_for(spec: &ModelSpec) -> ThinkingLevel {
    thinking_level(spec.reasoning_level.as_deref())
}

fn teammate_record(identity: &str, workspace: &str) -> Option<serde_json::Value> {
    // 私聊与群成员两种身份都要能定位到同一条记录；定位不到时退回按工作目录匹配。
    let id = bot_id_from_identity(identity).unwrap_or_default();
    let path = crate::data::data_root()
        .join("v2")
        .join("teammates.json");
    let bytes = std::fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let items = value.get("teammates")?.as_array()?;
    items
        .iter()
        .find(|item| {
            (!id.is_empty()
                && item.get("id").and_then(|value| value.as_str()) == Some(id.as_str()))
                || item
                    .get("directory")
                    .and_then(|value| value.as_str())
                    .is_some_and(|dir| dir.eq_ignore_ascii_case(workspace))
        })
        .cloned()
}

/// 该 Bot 的审批档位与白名单（读同一份 teammates.json）。
///
/// 身份 → Bot id（用于审批白名单、隐私归属、记忆注入）。两种形态都要认：
/// - 私聊：`mode-bot:teammate:<id>`
/// - 群成员：`mode-bot:group:<gid>:member:<id>`（见 `bot_team::group::group_member_workspace_identity`）
///
/// 只认前者会让群成员拿到 `None`，隐私守卫因此判不出「自己的目录」，
/// 群里读自己的文件也会弹越界卡，记忆工具和记忆注入一并静默消失。
pub fn bot_id_from_identity(identity: &str) -> Option<String> {
    let identity = identity.trim();
    if let Some(id) = identity
        .strip_prefix("mode-bot:teammate:")
        .filter(|id| !id.trim().is_empty())
    {
        return Some(id.to_string());
    }
    // 群成员：取 `:member:` 之后那一段（群 id 里可能自带冒号，用 rsplit 从右取）。
    let (prefix, member) = identity.rsplit_once(":member:")?;
    if !prefix.starts_with("mode-bot:group:") || member.trim().is_empty() {
        return None;
    }
    Some(member.to_string())
}

/// 隐私 Bot 的 id（只有 `mode-bot:teammate:<id>` 且记录了 isPrivate=true 才算）。
pub fn private_bot_id(identity: &str) -> Option<String> {
    let id = bot_id_from_identity(identity)?;
    let record = teammate_record(identity, "")?;
    record
        .get("isPrivate")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        .then_some(id)
}

fn safe_segment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// 目录/文件名用的安全片段（浏览器 profile、私有目录等共用一套规则）。
pub fn safe_segment_pub(value: &str) -> String {
    safe_segment(value)
}

pub fn approval_policy(identity: &str, workspace: &str) -> (bot_team::approval::ApprovalLevel, Vec<String>) {
    use bot_team::approval::{parse_level, ApprovalLevel};

    let Some(record) = teammate_record(identity, workspace) else {
        return (ApprovalLevel::Ask, Vec::new());
    };
    let level = record
        .pointer("/approvalPolicy/level")
        .and_then(|value| value.as_str())
        .and_then(|value| parse_level(value).ok())
        .unwrap_or(ApprovalLevel::Ask);
    let allowlist = record
        .pointer("/approvalPolicy/allowlist")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    (level, allowlist)
}

/// 未显式传模型时的回落：队友的 modelSelection → provider_config 的默认选择。
pub fn default_model_spec(identity: &str, workspace: &str) -> Option<ModelSpec> {
    if let Some(record) = teammate_record(identity, workspace)
        && let Some(selection) = record.get("modelSelection")
        && let (Some(provider_id), Some(model_id)) = (
            selection.get("providerId").and_then(|value| value.as_str()),
            selection.get("modelId").and_then(|value| value.as_str()),
        )
    {
        return Some(ModelSpec {
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
            reasoning_level: selection
                .pointer("/options/reasoningLevel")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        });
    }
    let bytes = std::fs::read(
        crate::data::data_root()
            .join("v2")
            .join("provider_config.json"),
    )
    .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let selection = value.pointer("/config/defaultModelSelection")?;
    Some(ModelSpec {
        provider_id: selection.get("providerId")?.as_str()?.to_string(),
        model_id: selection.get("modelId")?.as_str()?.to_string(),
        reasoning_level: selection
            .pointer("/options/reasoningLevel")
            .and_then(|value| value.as_str())
            .map(str::to_string),
    })
}

pub fn sessions_root() -> PathBuf {
    crate::data::data_root()
        .join("v2")
        .join("mode-chat-sessions")
}

/// 会话系统提示：工作目录 AGENTS.md（身份/团队约定）+ 团队记忆。
///
/// 不带 Bot 身份的老入口（自动化/隐藏任务仍是它）；带 Bot 的调用方用
/// [`system_prompt_for_bot`]，会额外注入该 Bot 自己的记忆。
#[allow(dead_code)]
pub fn system_prompt_for(workspace: &Path) -> String {
    system_prompt_for_bot(workspace, None, crate::model_limits::FALLBACK_CONTEXT_WINDOW)
}

/// 带 Bot 身份的系统提示（v0.5.0 W1）：在团队记忆之后再注入**自己的**记忆。
///
/// 预算按模型窗口算（`data::memory_budget`），不再写死 4000 字符；
/// `bot_id` 为空（IM/自动化/隐藏任务）时行为与旧版完全一致。
pub fn system_prompt_for_bot(workspace: &Path, bot_id: Option<&str>, context_window: i64) -> String {
    let mut prompt = String::from(
        "你是 Mode Chat 的 Bot 助手。用中文回答，遵守工作目录里的约定。",
    );
    if let Ok(agents) = std::fs::read_to_string(workspace.join("AGENTS.md")) {
        let agents = agents.trim();
        if !agents.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(agents);
        }
    }
    let memory = crate::data::team_memory();
    if !memory.trim().is_empty() {
        prompt.push_str("\n\n团队记忆：\n");
        prompt.push_str(memory.trim());
    }
    if let Some(bot_id) = bot_id.map(str::trim).filter(|value| !value.is_empty()) {
        let budget = crate::data::memory_budget(context_window);
        let own = crate::data::bot_memory_context(bot_id, budget);
        if !own.is_empty() {
            prompt.push_str("\n\n我的记忆（跨会话保留，可用 forget 删除）：\n");
            prompt.push_str(&own);
        }
    }
    prompt
}

fn tool_set(workspace: &Path) -> Vec<HarnessTool> {
    let env: Arc<OsExecutionEnv> = Arc::new(OsExecutionEnv::with_cwd(workspace.to_path_buf()));
    let context = ExecutionToolContext::new(env.clone(), Some(env));
    vec![
        HarnessTool::new(create_read_tool(&context, None)),
        HarnessTool::new(create_write_tool(&context)),
        HarnessTool::new(create_edit_tool(&context)),
        HarnessTool::new(create_bash_tool(&context, None)),
    ]
}

/// 审批闸门：`before_tool_call` 里按策略决定放行/询问；询问时把卡投给 UI 并等待作答。
pub struct ApprovalGate {
    events: UnboundedSender<BackendEvent>,
    key: String,
    member: Option<String>,
    level: bot_team::approval::ApprovalLevel,
    /// 可写：「始终允许」要热更新本会话的内存白名单，下一轮立即生效。
    allowlist: Mutex<Vec<String>>,
    /// 白名单持久化目标（teammates.json 的 Bot id；没有就不落盘）。
    bot_id: Option<String>,
    pending: Mutex<HashMap<String, PendingApproval>>,
}

struct PendingApproval {
    tool_name: String,
    sender: oneshot::Sender<bool>,
}

impl ApprovalGate {
    pub fn new(
        events: UnboundedSender<BackendEvent>,
        key: String,
        member: Option<String>,
        level: bot_team::approval::ApprovalLevel,
        allowlist: Vec<String>,
        bot_id: Option<String>,
    ) -> Self {
        Self {
            events,
            key,
            member,
            level,
            allowlist: Mutex::new(allowlist),
            bot_id,
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn hook(self: &Arc<Self>) -> BeforeToolCall {
        let gate = Arc::clone(self);
        Arc::new(move |context: BeforeToolCallContext<'_>, signal| {
            let gate = Arc::clone(&gate);
            let tool_name = context.tool_call.name.clone();
            let request_id = context.tool_call.id.clone();
            let args = context.args.clone();
            Box::pin(async move {
                gate.decide(tool_name, request_id, args, signal).await
            }) as BoxFuture<'static, Option<BeforeToolCallResult>>
        })
    }

    async fn decide(
        &self,
        tool_name: String,
        request_id: String,
        args: serde_json::Value,
        signal: tokio_util::sync::CancellationToken,
    ) -> Option<BeforeToolCallResult> {
        use bot_team::approval::{decide, ApprovalDecision, ApprovalLevel};

        // 无人值守测试/自动化：显式跳过审批（仅测试环境使用）。
        if std::env::var("BOT_PILOT_AUTO_APPROVE").ok().as_deref() == Some("1") {
            return None;
        }
        // 隐私守卫（W3）：路径类工具访问别人的/已归档的目录时，不管档位一律弹卡。
        // 例外：「完全自主」的语义是所有操作都不再询问，这里必须让位。
        if self.level != ApprovalLevel::Full {
            let cross_dir = crate::privacy::precheck(&tool_name, &args, self.bot_id.as_deref());
            if let Some(owner) = cross_dir {
                return self
                    .ask_permission(
                        &tool_name,
                        &request_id,
                        crate::privacy::reason_text(&tool_name, &args, &owner),
                        risk_of(&tool_name),
                        false,
                        true,
                        signal,
                    )
                    .await;
            }
        }
        let allowlist = self
            .allowlist
            .lock()
            .expect("approval allowlist poisoned")
            .clone();
        match decide(self.level, &allowlist, &tool_name) {
            ApprovalDecision::Allow => None,
            ApprovalDecision::Deny => Some(blocked("已拒绝（审批策略）")),
            ApprovalDecision::Ask => {
                // 风险档位按「实际副作用」分：本地执行/浏览器/电脑控制是 exec，写文件、
                // 定时任务与 MCP 外部工具是 write，其余（读文件、提问）是 read。
                // 自定义工具以前一律落到 read，审批卡上会把「建定时任务」标成只读。
                let lowered = tool_name.to_ascii_lowercase();
                // 定稿策略：Bash/MCP 永远询问，白名单对它们不生效，
                // 因此不给「始终允许」，避免用户以为点过之后不会再问。
                let rememberable = !matches!(
                    lowered.as_str(),
                    "bash" | "shell" | "run_command"
                ) && !lowered.starts_with("mcp__");
                let risk = risk_of(&tool_name);
                self.ask_permission(
                    &tool_name,
                    &request_id,
                    summarize_args(&args),
                    risk,
                    rememberable,
                    false,
                    signal,
                )
                .await
            }
        }
    }

    /// 弹一张审批卡并等作答；越界访问与常规审批共用同一条路径。
    /// `privacy=true` 表示隐私越界，UI 侧不得按档位自动放行（见 `PermissionPrompt::privacy`）。
    #[allow(clippy::too_many_arguments)]
    async fn ask_permission(
        &self,
        tool_name: &str,
        request_id: &str,
        reason: String,
        risk: &str,
        rememberable: bool,
        privacy: bool,
        signal: tokio_util::sync::CancellationToken,
    ) -> Option<BeforeToolCallResult> {
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("approval pending poisoned")
            .insert(
                request_id.to_string(),
                PendingApproval {
                    tool_name: tool_name.to_string(),
                    sender: tx,
                },
            );
        let _ = self.events.send(BackendEvent::Session {
            key: self.key.clone(),
            event: SessionEvent::Permission(PermissionPrompt {
                request_id: request_id.to_string(),
                tool_name: tool_name.to_string(),
                reason,
                risk: risk.to_string(),
                options: {
                    let mut options = vec![PromptOption {
                        option_id: "allow_once".to_string(),
                        label: "允许一次".to_string(),
                        kind: "allow_once".to_string(),
                    }];
                    if rememberable {
                        options.push(PromptOption {
                            option_id: "allow_always".to_string(),
                            label: "始终允许".to_string(),
                            kind: "allow_always".to_string(),
                        });
                    }
                    options.push(PromptOption {
                        option_id: "deny".to_string(),
                        label: "拒绝".to_string(),
                        kind: "deny".to_string(),
                    });
                    options
                },
                member: self.member.clone(),
                privacy,
            }),
        });
        tokio::select! {
            answer = rx => {
                self.pending
                    .lock()
                    .expect("approval pending poisoned")
                    .remove(request_id);
                let _ = self.events.send(BackendEvent::Session {
                    key: self.key.clone(),
                    event: SessionEvent::PermissionResolved {
                        request_id: request_id.to_string(),
                    },
                });
                if answer.unwrap_or(false) {
                    None
                } else {
                    Some(blocked("用户拒绝"))
                }
            }
            _ = signal.cancelled() => {
                self.pending
                    .lock()
                    .expect("approval pending poisoned")
                    .remove(request_id);
                Some(blocked("已取消"))
            }
        }
    }

    /// 返回 true 表示这个 request_id 属于本闸门（已作答）。
    /// 「始终允许」会把工具写入本会话白名单并落盘到 Bot 的 approvalPolicy。
    pub fn resolve(&self, request_id: &str, option_id: &str) -> bool {
        let pending = self
            .pending
            .lock()
            .expect("approval pending poisoned")
            .remove(request_id);
        let Some(pending) = pending else {
            return false;
        };
        let allow = !matches!(option_id, "deny");
        if allow && option_id == "allow_always" {
            let mut allowlist = self
                .allowlist
                .lock()
                .expect("approval allowlist poisoned");
            if !allowlist.iter().any(|name| name == &pending.tool_name) {
                allowlist.push(pending.tool_name.clone());
            }
            if let Some(bot_id) = &self.bot_id {
                let _ = crate::data::set_bot_allowlist(bot_id, allowlist.clone());
            }
        }
        let _ = pending.sender.send(allow);
        true
    }

    /// 白名单快照（测试用）。
    #[cfg(test)]
    pub fn allowlist(&self) -> Vec<String> {
        self.allowlist
            .lock()
            .expect("approval allowlist poisoned")
            .clone()
    }

}

fn blocked(reason: &str) -> BeforeToolCallResult {
    BeforeToolCallResult {
        block: true,
        reason: Some(reason.to_string()),
        terminate: false,
        args: None,
    }
}

fn summarize_args(args: &serde_json::Value) -> String {
    let text = serde_json::to_string(args).unwrap_or_default();
    if text.chars().count() > 400 {
        format!("{}…", text.chars().take(400).collect::<String>())
    } else {
        text
    }
}

/// 审批卡上的风险档：按工具有没有副作用分 exec / write / read。
///
/// 隐私越界卡和常规审批卡必须用同一套口径，否则 `read` 越界时卡上会写「风险 写入」，
/// 同一工具在常规审批里却写「风险 读取」，用户会当成两回事（D-10）。
fn risk_of(tool_name: &str) -> &'static str {
    let lowered = tool_name.to_ascii_lowercase();
    if lowered == "bash"
        || lowered == "powershell"
        || lowered.starts_with("computer_")
        || lowered.starts_with("browser_")
    {
        return "exec";
    }
    if lowered == "write"
        || lowered == "edit"
        || lowered.starts_with("automation_")
        || matches!(
            lowered.as_str(),
            "team_assign"
                | "team_goal_create"
                | "team_goal_stage"
                | "team_goal_pass"
                | "team_goal_rework"
                | "team_goal_acceptance"
                | "message_bot"
                | "post_to_group"
                | "remember"
                | "forget"
        )
        || lowered.starts_with("mcp__")
    {
        return "write";
    }
    "read"
}

/// 工具结果里的图片落盘（截图类工具），返回本地路径供 UI 直接 img 渲染。
fn write_tool_image(dir: &Path, call_id: &str, index: usize, mime: &str, data: &str) -> Option<String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.as_bytes()).ok()?;
    let safe: String = call_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let ext = match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "png",
    };
    let path = dir.join(format!("{safe}-{index}.{ext}"));
    std::fs::write(&path, bytes).ok()?;
    Some(path.to_string_lossy().into_owned())
}

fn tool_image_dir() -> Option<PathBuf> {
    let dir = crate::instance::state_dir().join("tool-images");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// 只保留最近 300 张，避免截图工具把试点目录撑大。
fn prune_tool_images(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let modified = entry.metadata().ok()?.modified().ok()?;
            path.is_file().then_some((modified, path))
        })
        .collect();
    if files.len() <= 300 {
        return;
    }
    files.sort_by_key(|(modified, _)| *modified);
    for (_, path) in files.iter().take(files.len() - 300) {
        let _ = std::fs::remove_file(path);
    }
}

fn tool_images(call_id: &str, content: &[rpi_agent::TextContentOrImage]) -> Vec<String> {
    let Some(dir) = tool_image_dir() else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for (index, item) in content.iter().enumerate() {
        let rpi_agent::TextContentOrImage::Image(image) = item else {
            continue;
        };
        if let Some(path) = write_tool_image(&dir, call_id, index, &image.mime_type, &image.data) {
            paths.push(path);
        }
    }
    if !paths.is_empty() {
        prune_tool_images(&dir);
    }
    paths
}

/// 历史 JSONL 里的图片块（`{type:"image", data, mimeType}`）。
fn history_tool_images(call_id: &str, content: &[serde_json::Value]) -> Vec<String> {
    let Some(dir) = tool_image_dir() else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for (index, item) in content.iter().enumerate() {
        if item.get("type").and_then(|value| value.as_str()) != Some("image") {
            continue;
        }
        let data = item
            .get("data")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let mime = item
            .get("mimeType")
            .and_then(|value| value.as_str())
            .unwrap_or("image/png");
        if let Some(path) = write_tool_image(&dir, call_id, index, mime, data) {
            paths.push(path);
        }
    }
    if !paths.is_empty() {
        prune_tool_images(&dir);
    }
    paths
}

/// 结果 details 的 JSON 文本（edit 的 diff、bash 的 exit_code）。
/// 只用于展示，截断到 8k 字符，避免一条工具卡把整段 diff 全搬进 UI 状态。
fn tool_details(value: &serde_json::Value) -> String {
    if value.is_null() {
        return String::new();
    }
    let text = serde_json::to_string(value).unwrap_or_default();
    if text.chars().count() > 8_000 {
        format!("{}…", text.chars().take(8_000).collect::<String>())
    } else {
        text
    }
}

fn tool_detail(content: &[rpi_agent::TextContentOrImage]) -> String {
    let mut out = String::new();
    for item in content {
        match item {
            rpi_agent::TextContentOrImage::Text(text) => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&text.text);
            }
            rpi_agent::TextContentOrImage::Image(_) => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str("[image]");
            }
        }
    }
    if out.chars().count() > 6_000 {
        format!("{}…", out.chars().take(6_000).collect::<String>())
    } else {
        out
    }
}

/// 把 provider 的原始错误正文压缩成一句可读文案。
///
/// 典型形态：`http error 402: {"type":"error","error":{"type":"billing_error",
/// "message":"Insufficient Balance","request_id":"..."}}`。原样铺在气泡里
/// 是一整段 JSON，用户读不出「哪里错了、该怎么办」。这里只做两件事：
/// 取出 `error.message`、给常见类别补一句处置建议；拿不到就原样返回。
fn friendly_error_detail(detail: &str) -> String {
    let (status, body) = match detail.split_once(": ") {
        Some((prefix, body)) if prefix.starts_with("http error ") => {
            (Some(prefix.trim_start_matches("http error ").trim()), body)
        }
        _ => (None, detail),
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return detail.to_string();
    };
    let Some(message) = value
        .pointer("/error/message")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
    else {
        return detail.to_string();
    };
    let kind = value
        .pointer("/error/type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let request = value
        .pointer("/error/request_id")
        .and_then(|v| v.as_str())
        .or_else(|| value.pointer("/request_id").and_then(|v| v.as_str()));
    let mut out = String::new();
    if let Some(status) = status {
        out.push_str(&format!("HTTP {status} · "));
    }
    out.push_str(message);
    if kind == "billing_error" {
        out.push_str(if crate::i18n::is_en() {
            " (insufficient balance: top up or switch provider)"
        } else {
            "（账户余额不足：请充值或换一个 provider）"
        });
    }
    if let Some(request) = request {
        out.push_str(&format!(" · request_id {request}"));
    }
    out
}

/// 判断这一轮是否以失败收场；失败时返回给用户看的错误文案。
///
/// pi-agent 在 provider 报错时**不会**抛错，而是把 `stop_reason` 置成 `Error`、
/// 错误文本放进 `error_message`，然后照常发 `TurnEnd` + `AgentEnd`
/// （`crates/rpi-agent/src/agent_loop.rs:305`）。所以失败与成功在事件流里长得一样，
/// 唯一区别就在消息本身。`Aborted` 是用户点「停止」，不算失败，返回 `None`。
fn agent_end_failure(messages: &[rpi_agent::AgentMessage]) -> Option<String> {
    for message in messages.iter().rev() {
        let rpi_agent::AgentMessage::Assistant(assistant) = message else {
            continue;
        };
        match assistant.stop_reason {
            rpi_ai::StopReason::Error => {
                let detail = assistant
                    .error_message
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .unwrap_or("模型请求失败，未返回具体原因");
                return Some(format!(
                    "{}（可用「重试」再发一次）",
                    friendly_error_detail(detail)
                ));
            }
            // Aborted = 用户主动停止；Pending/Deferred 是中间态，都不该报错。
            rpi_ai::StopReason::Aborted
            | rpi_ai::StopReason::Pending
            | rpi_ai::StopReason::Deferred => return None,
            rpi_ai::StopReason::Stop
            | rpi_ai::StopReason::Length
            | rpi_ai::StopReason::ToolUse => continue,
        }
    }
    None
}

/// 把 pi-rust 的 AgentEvent 直接映射成 UI 事件。
struct UiEmitter {
    events: UnboundedSender<BackendEvent>,
    key: String,
    member: Option<String>,
}

impl UiEmitter {
    fn send(&self, event: SessionEvent) {
        let _ = self.events.send(BackendEvent::Session {
            key: self.key.clone(),
            event,
        });
    }
}

impl AgentEmitter for UiEmitter {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'static, ()> {
        self.try_emit(event);
        Box::pin(async {})
    }

    fn try_emit(&self, event: AgentEvent) {
        match event {
            AgentEvent::MessageUpdate {
                assistant_message_event,
                ..
            } => match assistant_message_event {
                AssistantMessageEvent::TextDelta { delta, .. } => {
                    if !delta.is_empty() {
                        crate::backend::v4_selftest_delta(&delta);
                        self.send(SessionEvent::Delta {
                            member: self.member.clone(),
                            text: delta,
                        });
                    }
                }
                AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                    if !delta.is_empty() {
                        self.send(SessionEvent::Reasoning {
                            member: self.member.clone(),
                            text: delta,
                            done: false,
                            reset: false,
                        });
                    }
                }
                AssistantMessageEvent::ThinkingEnd { .. } => {
                    self.send(SessionEvent::Reasoning {
                        member: self.member.clone(),
                        text: String::new(),
                        done: true,
                        reset: false,
                    });
                }
                _ => {}
            },
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
                ..
            } => {
                self.send(SessionEvent::Tool(ToolCard {
                    call_id: tool_call_id,
                    name: tool_name.clone(),
                    state: "running".to_string(),
                    title: tool_name,
                    detail: summarize_args(&args),
                    input: serde_json::to_string(&args).unwrap_or_default(),
                    details: String::new(),
                    images: Vec::new(),
                }));
            }
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                tool_name,
                args,
                partial_result,
                ..
            } => {
                let images = tool_images(&tool_call_id, &partial_result.content);
                self.send(SessionEvent::Tool(ToolCard {
                    call_id: tool_call_id,
                    name: tool_name.clone(),
                    state: "running".to_string(),
                    title: tool_name,
                    detail: tool_detail(&partial_result.content),
                    input: serde_json::to_string(&args).unwrap_or_default(),
                    details: tool_details(&partial_result.details),
                    images,
                }));
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
                ..
            } => {
                let images = tool_images(&tool_call_id, &result.content);
                self.send(SessionEvent::Tool(ToolCard {
                    call_id: tool_call_id,
                    name: tool_name.clone(),
                    state: if is_error { "error" } else { "done" }.to_string(),
                    title: tool_name,
                    detail: tool_detail(&result.content),
                    input: String::new(),
                    details: tool_details(&result.details),
                    images,
                }));
            }
            AgentEvent::RetryScheduled { error, attempt, max_retries, .. } => {
                self.send(SessionEvent::Retry {
                    member: self.member.clone(),
                    attempt,
                    max_retries,
                    reason: error,
                });
            }
            AgentEvent::AgentEnd { messages } => {
                let mut input_tokens = 0i64;
                let mut output_tokens = 0i64;
                let mut cache_read = 0i64;
                let mut reasoning_tokens = 0i64;
                for message in &messages {
                    if let rpi_agent::AgentMessage::Assistant(assistant) = message {
                        input_tokens += assistant.usage.input;
                        output_tokens += assistant.usage.output;
                        cache_read += assistant.usage.cache_read;
                        reasoning_tokens += assistant.usage.reasoning.unwrap_or(0);
                    }
                }
                if input_tokens > 0 || output_tokens > 0 {
                    self.send(SessionEvent::TurnUsage {
                        member: self.member.clone(),
                        input_tokens,
                        output_tokens,
                        cache_read,
                        reasoning_tokens,
                    });
                }
                // 失败判定：pi-agent 在 provider 报错时照样发 AgentEnd
                // （`agent_loop.rs:305` 对 `stop_reason == Error | Aborted` 直接 TurnEnd +
                // AgentEnd 后返回）。这里若无条件发 Done，UI 会当成正常结束——
                // 用户看到的就是「气泡上屏后 AI 再也不说话」，日志里却有 402/429/401。
                // Aborted 是用户自己点停止，不算错误，仍走 Done。
                match agent_end_failure(&messages) {
                    Some(message) => self.send(SessionEvent::Error {
                        member: self.member.clone(),
                        message,
                    }),
                    None => self.send(SessionEvent::Done {
                        member: self.member.clone(),
                    }),
                }
                crate::backend::v4_selftest_done();
            }
            _ => {}
        }
    }
}

pub struct OpenOptions {
    pub workspace: PathBuf,
    pub resume_path: Option<String>,
    pub model: EngineModel,
    pub thinking: ThinkingLevel,
    pub system_prompt: String,
    pub events: UnboundedSender<BackendEvent>,
    pub key: String,
    pub member: Option<String>,
    pub approval_level: bot_team::approval::ApprovalLevel,
    pub allowlist: Vec<String>,
    /// 「始终允许」的白名单落盘目标（Bot id；IM/自动化会话可为空）。
    pub bot_id: Option<String>,
    /// Bot 名字（写进会话 header 的 `metadata.botName`，用量分组显示用）。
    pub bot_name: Option<String>,
    /// 额外工具（MCP / 浏览器 / 电脑控制等宿主能力）。
    pub extra_tools: Vec<HarnessTool>,
    /// 隐私 Bot id：非空时会话落到 `.private/<id>/`，全局搜索会跳过。
    pub private_bot: Option<String>,
}

pub struct RpiSession {
    _harness: AgentHarness,
    lane: Arc<dyn AgentLane>,
    pub path: String,
    pub gate: Arc<ApprovalGate>,
    pub questions: Arc<QuestionGate>,
    /// 压缩进度订阅（`HarnessEvent::CompactionProgress`）；随会话 drop 停止。
    _compaction_watch: rpi_harness::events::WatchHandle<()>,
}

impl RpiSession {
    // AgentHarnessOptions 字段很多且默认值有意义；逐字段赋值比巨型结构体字面量更清晰。
    #[allow(clippy::field_reassign_with_default)]
    pub async fn open(options: OpenOptions) -> Result<Self, String> {
        let debug = std::env::var("BOT_PILOT_DEBUG").is_ok();
        if debug {
            eprintln!(
                "[rpi] open start workspace={} resume={:?}",
                options.workspace.display(),
                options.resume_path
            );
        }
        let root = match options.private_bot.as_deref() {
            Some(bot_id) => sessions_root().join(".private").join(safe_segment(bot_id)),
            // 旁路会话单独落盘：可以恢复/审计，但不进历史列表和全局搜索。
            None if options.key.starts_with("side:") => sessions_root().join(".side"),
            None => sessions_root(),
        };
        std::fs::create_dir_all(&root)
            .map_err(|error| format!("会话目录创建失败（{}）：{error}", root.display()))?;
        let workspace = options.workspace;
        let fs: Arc<dyn rpi_tools::FileSystem> =
            Arc::new(OsExecutionEnv::with_cwd(workspace.clone()));
        let clock = Arc::new(SystemClock);
        let ids = Arc::new(DefaultIdGenerator::new());
        let repo = JsonlSessionRepo::with_env_cwd(JsonlSessionRepoOptions {
            fs: fs.clone(),
            sessions_root: root.to_string_lossy().into_owned(),
            clock: clock.clone(),
            ids: ids.clone(),
        });

        let (storage, path) = match options
            .resume_path
            .as_deref()
            .filter(|path| Path::new(path).is_file())
        {
            Some(path) => {
                let storage = JsonlSessionStorage::load(
                    fs.clone(),
                    path,
                    clock.clone(),
                    ids.clone(),
                )
                .await
                .map_err(|error| format!("会话恢复失败：{error}"))?;
                (storage, path.to_string())
            }
            None => {
                // 会话 header 带 botId/botName（W9）：用量按 Bot 分组要用。
                // 老会话没有这个键 → UI 归「未归属」，不需要重写老文件。
                let metadata = options.bot_id.as_ref().map(|bot_id| {
                    let mut map = serde_json::Map::new();
                    map.insert(
                        "botId".to_string(),
                        serde_json::Value::String(bot_id.clone()),
                    );
                    if let Some(name) = options.bot_name.as_ref() {
                        map.insert(
                            "botName".to_string(),
                            serde_json::Value::String(name.clone()),
                        );
                    }
                    map
                });
                let storage = repo
                    .create_typed(&JsonlSessionCreateOptions {
                        id: None,
                        parent_session_id: None,
                        cwd: workspace.to_string_lossy().into_owned(),
                        metadata,
                    })
                    .await
                    .map_err(|error| format!("会话创建失败：{error}"))?;
                let path = storage.jsonl_metadata().path.clone();
                (storage, path)
            }
        };

        if debug {
            eprintln!("[rpi] storage ok path={path}");
        }
        let session = Session::new(Arc::new(storage), None);
        let gate = Arc::new(ApprovalGate::new(
            options.events.clone(),
            options.key.clone(),
            options.member.clone(),
            options.approval_level,
            options.allowlist,
            options.bot_id,
        ));
        let questions = Arc::new(QuestionGate::new(
            options.events.clone(),
            options.key.clone(),
            options.member.clone(),
        ));

        let mut harness_options = AgentHarnessOptions::default();
        harness_options.model = options.model.model;
        harness_options.models = vec![options.model.provider];
        harness_options.tools = tool_set(&workspace);
        harness_options.tools.extend(options.extra_tools);
        harness_options.tools.push(questions.tool());
        // O05：IM 通道不提供 request_secret（密钥不经 IM 传输/输入）。
        if options.key != "im" {
            harness_options
                .tools
                .push(questions.secret_tool(crate::data::data_root()));
        }
        harness_options.active_tool_names = harness_options
            .tools
            .iter()
            .map(|tool| tool.tool.schema().name.clone())
            .collect();
        harness_options.system_prompt = Some(options.system_prompt);
        harness_options.thinking_level = options.thinking;
        harness_options.session = session;
        // 恢复已有 JSONL 会话时必须显式允许；否则 harness 拒绝带记录的会话。
        harness_options.allow_existing_session = true;
        harness_options.agent_emitter = Some(Arc::new(UiEmitter {
            events: options.events.clone(),
            key: options.key.clone(),
            member: options.member.clone(),
        }));
        harness_options.before_tool_call = Some(gate.hook());
        // 工具串行更贴近 Mode 的回合语义；并行会让审批卡顺序不可预测。
        harness_options.tool_execution = rpi_harness::types::HarnessToolExecution::Sequential;

        let tool_count = harness_options.active_tool_names.len();
        let key_for_events = options.key.clone();
        let member_for_events = options.member.clone();
        let harness = AgentHarness::create(harness_options)
            .await
            .map_err(|error| format!("Agent 会话初始化失败：{error}"))?;
        // 压缩进度 → UI 状态行（W4）。`CompactionProgressEvent` 没有实现
        // `HarnessEventOf`（pi-rust rev 29d8686），所以走 watch 拿原始事件再按标签过滤。
        let mut compaction_watch = harness.events().watch(|| ());
        {
            let events = options.events.clone();
            let key = key_for_events;
            let member = member_for_events;
            compaction_watch.start(Arc::new(move |event: &rpi_harness::events::HarnessEvent| {
                if let rpi_harness::events::HarnessEvent::CompactionProgress(progress) = event
                    && progress.lane == "main"
                {
                    let _ = events.send(BackendEvent::Session {
                        key: key.clone(),
                        event: SessionEvent::Compaction {
                            active: progress.active,
                            manual: progress.manual,
                        },
                    });
                    let _ = &member;
                }
            }));
        }
        if debug {
            eprintln!(
                "[rpi] harness ok id={} tools={tool_count}",
                harness.session_id()
            );
        }
        let lane = harness.lane("main");
        Ok(Self {
            _harness: harness,
            lane,
            path,
            gate,
            questions,
            _compaction_watch: compaction_watch,
        })
    }

    /// 手动压缩上下文（命令面板「立即压缩」）。
    ///
    /// 返回 `Ok(文案)` = 这一轮正常有了结论（压缩完成、或「上下文还很短」这类
    /// 主动跳过），`Err(文案)` = 真失败。**别按文案前缀判断成败**：早前调用方
    /// 只认 `✅` 开头，把「未压缩：上下文还很短」当成错误弹了红条（验收 D-08）。
    pub async fn compact(&self) -> Result<String, String> {
        use rpi_harness::agent_harness::CompactionOutcome;
        match self.lane.compact(None).await {
            Ok(result) => match result.outcome {
                CompactionOutcome::Completed { .. } => Ok("✅ 上下文已压缩".to_string()),
                CompactionOutcome::Declined { .. } => Ok("未压缩：上下文还很短".to_string()),
                CompactionOutcome::Aborted { .. } => Ok("未压缩：已中止".to_string()),
                CompactionOutcome::Failed { error, .. } => {
                    Err(format!("压缩失败：{}", error.message))
                }
            },
            Err(error) => Err(format!("压缩失败：{error}")),
        }
    }

    pub async fn send(&self, text: String, images: Vec<ImageContent>) {
        if let Err(error) = self.lane.prompt_text(&text, images).await {
            let _ = self.gate.events.send(BackendEvent::Session {
                key: self.gate.key.clone(),
                event: SessionEvent::Error {
                    member: self.gate.member.clone(),
                    message: error.to_string(),
                },
            });
        }
    }

    /// 回合进行中插话：交给 lane 的 steering 队列，在当前工具批次之后
    /// 作为新的用户输入进入模型上下文，不新开回合。
    ///
    /// `images` 非空时把消息组成 `Blocks([Text, Image...])`——
    /// 和正常发送走同一条 `UserContent` 路径，模型看到的内容格式一致。
    pub async fn steer(&self, text: String, images: Vec<ImageContent>) {
        let content: rpi_ai::types::UserContent = if images.is_empty() {
            rpi_ai::types::UserContent::Text(text)
        } else {
            let mut blocks = vec![rpi_ai::types::Content::text(text)];
            blocks.extend(images.into_iter().map(rpi_ai::types::Content::Image));
            rpi_ai::types::UserContent::Blocks(blocks)
        };
        let message = rpi_agent::AgentMessage::User(rpi_ai::types::UserMessage::new(
            content,
            crate::time::now_ms() as i64,
        ));
        if let Err(error) = self.lane.steer(message).await {
            let _ = self.gate.events.send(BackendEvent::Session {
                key: self.gate.key.clone(),
                event: SessionEvent::Error {
                    member: self.gate.member.clone(),
                    message: format!("插话失败：{error}"),
                },
            });
        }
    }

    pub async fn abort(&self) {
        let _ = self.lane.abort().await;
    }
}


/// 把导入的 Markdown 转成一条可 resume 的 pi-rust JSONL 会话。
/// entries 为 (是否用户, 文本)；返回新会话文件路径。
pub fn import_markdown_session(
    workspace: &str,
    entries: &[(bool, String)],
) -> Result<String, String> {
    if entries.is_empty() {
        return Err("没有可导入的消息".to_string());
    }
    let dir = sessions_root().join(format!(
        "--{}--",
        workspace
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect::<String>()
    ));
    std::fs::create_dir_all(&dir).map_err(|error| format!("创建会话目录失败：{error}"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let session_id = uuid::Uuid::new_v4().to_string();
    let path = dir.join(format!("{now}_import_{session_id}.jsonl"));
    let mut lines: Vec<String> = Vec::new();
    let header = serde_json::json!({
        "kind": "header",
        "version": 4,
        "id": session_id,
        "createdAt": now,
        "cwd": workspace,
        "progressFormat": "assistant-progress-v1",
    });
    lines.push(serde_json::to_string(&header).map_err(|error| error.to_string())?);
    let mut parent: Option<String> = None;
    for (index, (user, text)) in entries.iter().enumerate() {
        let entry_id = uuid::Uuid::new_v4().to_string();
        let timestamp = now + index as u64;
        let message = if *user {
            serde_json::json!({
                "kind": "user",
                "role": "user",
                "content": text,
                "timestamp": timestamp,
            })
        } else {
            // assistant 消息是 provider 快照：content 为内容块数组，且带 api/provider/model/usage。
            serde_json::json!({
                "kind": "assistant",
                "role": "assistant",
                "content": [{"type": "text", "text": text}],
                "api": "anthropic-messages",
                "provider": "anthropic",
                "model": "imported",
                "usage": {
                    "input": 0,
                    "output": 0,
                    "cacheRead": 0,
                    "cacheWrite": 0,
                    "reasoning": 0,
                    "totalTokens": 0,
                    "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
                },
                "stopReason": "stop",
                "timestamp": timestamp,
            })
        };
        let entry = serde_json::json!({
            "kind": "entry",
            "lane": "main",
            "type": "message",
            "id": entry_id,
            "seq": index as u64 + 1,
            "parentId": parent,
            "timestamp": timestamp,
            "message": message,
        });
        lines.push(serde_json::to_string(&entry).map_err(|error| error.to_string())?);
        parent = Some(entry_id);
    }
    let body = format!("{}\n", lines.join("\n"));
    atomic_file::write_text_atomic(
        &path,
        &body,
        &atomic_file::WriteOptions::default(),
    )
    .map_err(|error| format!("写入会话文件失败：{error}"))?;
    // pi-rust resume 要求 history 与 progress companion 同时存在。
    let progress_path = PathBuf::from(format!("{}.progress.jsonl", path.display()));
    let progress = serde_json::json!({
        "format": "assistant-progress-v1",
        "session_id": session_id,
        "state": {"sequence": 0, "frames": []},
    });
    atomic_file::write_text_atomic(
        &progress_path,
        &format!("{}\n", serde_json::to_string(&progress).unwrap_or_default()),
        &atomic_file::WriteOptions::default(),
    )
    .map_err(|error| format!("写入会话进度失败：{error}"))?;
    Ok(path.to_string_lossy().into_owned())
}

/// 从 JSONL 会话文件还原用户/助手文本（重启后的历史回放）。
pub fn history_entries(path: &str) -> Vec<crate::backend::HistoryEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // assistant 消息里的 toolCall（含参数）先记下，等 tool_result 到达时配回去。
    let mut tool_args: HashMap<String, String> = HashMap::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|item| item.as_str()) != Some("message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let kind = message
            .get("kind")
            .and_then(|item| item.as_str())
            .unwrap_or("");
        let at = value
            .get("timestamp")
            .and_then(|item| item.as_u64())
            .unwrap_or(0);
        let mut text_body = String::new();
        let mut reasoning = String::new();
        // 纯文本用户消息在 JSONL 里是 `"content": "…"`（不是数组）；只处理数组会把
        // 重启后的用户发言整段丢掉，导出 Markdown 也只剩助手消息。
        if let Some(text) = message.get("content").and_then(|item| item.as_str()) {
            text_body.push_str(text);
        }
        // 工具结果不是「助手发言」：历史里还原成工具卡（否则浏览器快照等原始输出
        // 会伪装成 Bot 说的话上屏，导出 Markdown 也会混进去）。
        if kind == "tool_result" || kind == "toolResult" {
            let mut output = String::new();
            for content in message
                .get("content")
                .and_then(|item| item.as_array())
                .into_iter()
                .flatten()
            {
                if content.get("type").and_then(|item| item.as_str()) == Some("text") {
                    output.push_str(
                        content
                            .get("text")
                            .and_then(|item| item.as_str())
                            .unwrap_or(""),
                    );
                }
            }
            if !output.trim().is_empty() {
                let name = message
                    .get("toolName")
                    .and_then(|item| item.as_str())
                    .unwrap_or("tool")
                    .to_string();
                let call_id = message
                    .get("toolCallId")
                    .and_then(|item| item.as_str())
                    .unwrap_or("")
                    .to_string();
                let is_error = message
                    .get("isError")
                    .and_then(|item| item.as_bool())
                    .unwrap_or(false);
                let detail: String = output.trim().chars().take(6_000).collect();
                let input = tool_args.remove(&call_id).unwrap_or_default();
                let details = tool_details(
                    message.get("details").unwrap_or(&serde_json::Value::Null),
                );
                let images = message
                    .get("content")
                    .and_then(|value| value.as_array())
                    .map(|items| history_tool_images(&call_id, items))
                    .unwrap_or_default();
                out.push(crate::backend::HistoryEntry {
                    user: false,
                    reasoning: false,
                    text: String::new(),
                    tool: Some(crate::backend::ToolCard {
                        call_id,
                        name: name.clone(),
                        state: if is_error { "error" } else { "done" }.to_string(),
                        title: name,
                        detail,
                        input,
                        details,
                        images,
                    }),
                    attachments: Vec::new(),
                    member: None,
                    at,
                });
            }
            continue;
        }
        for content in message
            .get("content")
            .and_then(|item| item.as_array())
            .into_iter()
            .flatten()
        {
            match content.get("type").and_then(|item| item.as_str()) {
                Some("toolCall") => {
                    if let Some(id) = content.get("id").and_then(|item| item.as_str()) {
                        let args = content
                            .get("arguments")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
                        tool_args.insert(
                            id.to_string(),
                            serde_json::to_string(&args).unwrap_or_default(),
                        );
                    }
                }
                Some("text") => text_body.push_str(
                    content
                        .get("text")
                        .and_then(|item| item.as_str())
                        .unwrap_or(""),
                ),
                Some("thinking") => reasoning.push_str(
                    content
                        .get("thinking")
                        .and_then(|item| item.as_str())
                        .unwrap_or(""),
                ),
                _ => {}
            }
        }
        if !reasoning.trim().is_empty() {
            out.push(crate::backend::HistoryEntry {
                user: false,
                reasoning: true,
                text: reasoning.trim().to_string(),
                tool: None,
                attachments: Vec::new(),
                member: None,
                at,
            });
        }
        if !text_body.trim().is_empty() {
            out.push(crate::backend::HistoryEntry {
                user: kind == "user",
                reasoning: false,
                text: text_body.trim().to_string(),
                tool: None,
                attachments: Vec::new(),
                member: None,
                at,
            });
        }
    }
    out
}

/// 在本地 JSONL 会话里做全文搜索（每个会话最多一条命中，按时间倒序）。
///
/// 会话已全面切到 pi-rust JSONL，旧 `db.sqlite` 的 LIKE 搜索对新会话永远空。
pub fn search_sessions(query: &str, limit: usize) -> Vec<crate::session_db::SearchHit> {
    let needle = query.trim().to_lowercase();
    if needle.chars().count() < 2 || limit == 0 {
        return Vec::new();
    }
    let mut hits = Vec::new();
    let private_workspaces = crate::data::private_workspace_dirs();
    collect_search(
        &sessions_root(),
        &needle,
        limit,
        &private_workspaces,
        &mut hits,
    );
    hits.sort_by_key(|hit| std::cmp::Reverse(hit.at));
    hits.truncate(limit);
    hits
}

fn collect_search(
    dir: &std::path::Path,
    needle: &str,
    limit: usize,
    private_workspaces: &[String],
    out: &mut Vec<crate::session_db::SearchHit>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // 隐私 Bot / 旁路会话的根：不从全局搜索里出现。
            if matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some(".private") | Some(".side")
            ) {
                continue;
            }
            collect_search(&path, needle, limit, private_workspaces, out);
            if out.len() >= limit * 4 {
                return;
            }
            continue;
        }
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        if !name.ends_with(".jsonl") || name.ends_with(".progress.jsonl") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        // 旧版隐私 Bot 会话可能还在主目录：按 header cwd 排除。
        if !private_workspaces.is_empty()
            && text.lines().take(8).any(|line| {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                    return false;
                };
                if value.get("kind").and_then(|item| item.as_str()) != Some("header") {
                    return false;
                }
                value
                    .get("cwd")
                    .and_then(|item| item.as_str())
                    .is_some_and(|cwd| {
                        private_workspaces
                            .iter()
                            .any(|dir| cwd.eq_ignore_ascii_case(dir))
                    })
            })
        {
            continue;
        }
        let mut title = String::new();
        let mut best: Option<(u64, String)> = None;
        for line in text.lines() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if value.get("kind").and_then(|item| item.as_str()) == Some("header") {
                if let Some(cwd) = value.get("cwd").and_then(|item| item.as_str()) {
                    title = cwd
                        .rsplit(['\\', '/'])
                        .find(|part| !part.is_empty())
                        .unwrap_or(cwd)
                        .to_string();
                }
                continue;
            }
            if value.get("type").and_then(|item| item.as_str()) != Some("message") {
                continue;
            }
            let Some(message) = value.get("message") else {
                continue;
            };
            let kind = message.get("kind").and_then(|item| item.as_str()).unwrap_or("");
            if kind != "user" && kind != "assistant" {
                continue;
            }
            let mut body = String::new();
            if let Some(text) = message.get("content").and_then(|item| item.as_str()) {
                body.push_str(text);
            }
            for content in message
                .get("content")
                .and_then(|item| item.as_array())
                .into_iter()
                .flatten()
            {
                if content.get("type").and_then(|item| item.as_str()) == Some("text") {
                    body.push_str(
                        content
                            .get("text")
                            .and_then(|item| item.as_str())
                            .unwrap_or(""),
                    );
                }
            }
            let lower = body.to_lowercase();
            let Some(byte_pos) = lower.find(needle) else {
                continue;
            };
            let at = value
                .get("timestamp")
                .and_then(|item| item.as_u64())
                .unwrap_or(0);
            let char_pos = lower[..byte_pos].chars().count();
            let chars: Vec<char> = body.chars().collect();
            let start = char_pos.saturating_sub(24);
            let end = (char_pos + needle.chars().count() + 40).min(chars.len());
            let mut snippet: String = chars[start..end].iter().collect();
            snippet = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
            if best.as_ref().is_none_or(|(best_at, _)| at >= *best_at) {
                best = Some((at, snippet));
            }
        }
        if let Some((at, snippet)) = best {
            out.push(crate::session_db::SearchHit {
                session_id: path.to_string_lossy().into_owned(),
                title: if title.is_empty() {
                    "会话".to_string()
                } else {
                    title
                },
                snippet,
                at,
            });
        }
    }
}

/// 最近 `days` 天（含今天）的用量分日聚合：`(day, input, output, cacheRead)`。
///
/// 用量页现在走 [`usage_grouped`]（一次扫描出按天/按 Bot/按模型三张表），
/// 这个入口保留给旧调用方/测试，标 `dead_code` 不算债。
#[allow(dead_code)]
pub fn usage_daily(days: usize) -> Vec<(String, i64, i64, i64)> {
    let mut buckets: std::collections::HashMap<String, (i64, i64, i64)> =
        std::collections::HashMap::new();
    collect_daily(&sessions_root(), &mut buckets);
    let now = crate::time::now_ms();
    (0..days)
        .rev()
        .map(|back| {
            let ms = now.saturating_sub(back as u64 * 86_400_000);
            let day = crate::time::format_day(ms);
            let (input, output, cache) = buckets.get(&day).copied().unwrap_or((0, 0, 0));
            (day, input, output, cache)
        })
        .collect()
}

/// 一次扫描产出的分组用量（W9 / 契约 §2.8）。
///
/// UI 侧（`settings_page_usage`）本期由前端接线，所以对未接线项放开 `dead_code`。
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct GroupedUsage {
    pub total: crate::session_db::UsageSummary,
    /// `(day, input, output, cacheRead)`
    pub by_day: Vec<(String, i64, i64, i64)>,
    /// `(botName, turns, input, output)`；老会话没有 `metadata.botId` → 「未归属」。
    pub by_bot: Vec<(String, i64, i64, i64)>,
    /// `(provider, model, turns, input, output)`
    pub by_model: Vec<(String, String, i64, i64, i64)>,
    /// 扫描范围内的会话数（UI 用来判断"有没有数据"）。
    pub sessions: usize,
}

/// 按天 / 按 Bot / 按模型一次性聚合最近 `days` 天的用量。
///
/// 只扫一遍盘：每个 JSONL 文件读 header（拿 `metadata.botId/botName`）+
/// 逐条 assistant usage。会话所属 Bot 优先用 header，缺失时按 `cwd` 反查名册目录
/// （老会话没有 botId，这条兜底能让大部分历史会话归到正确的 Bot 上）。
#[allow(dead_code)]
pub fn usage_grouped(days: usize) -> GroupedUsage {
    let now = crate::time::now_ms();
    let days = days.max(1);
    let since = now.saturating_sub((days as u64).saturating_sub(1) * 86_400_000);
    let cutoff_day = crate::time::format_day(since);
    let roster = crate::data::Roster::load();
    let mut out = GroupedUsage::default();
    let mut by_day: std::collections::BTreeMap<String, (i64, i64, i64)> =
        std::collections::BTreeMap::new();
    let mut by_bot: std::collections::HashMap<String, (i64, i64, i64)> =
        std::collections::HashMap::new();
    let mut by_model: std::collections::HashMap<(String, String), (i64, i64, i64)> =
        std::collections::HashMap::new();
    collect_grouped(
        &sessions_root(),
        &cutoff_day,
        &roster,
        &mut out,
        &mut by_day,
        &mut by_bot,
        &mut by_model,
    );
    out.by_day = by_day
        .into_iter()
        .map(|(day, (input, output, cache))| (day, input, output, cache))
        .collect();
    out.by_bot = {
        let mut rows: Vec<(String, i64, i64, i64)> = by_bot
            .into_iter()
            .map(|(name, (turns, input, output))| (name, turns, input, output))
            .collect();
        rows.sort_by(|left, right| right.3.cmp(&left.3).then_with(|| left.0.cmp(&right.0)));
        rows
    };
    out.by_model = {
        let mut rows: Vec<(String, String, i64, i64, i64)> = by_model
            .into_iter()
            .map(|((provider, model), (turns, input, output))| {
                (provider, model, turns, input, output)
            })
            .collect();
        rows.sort_by(|left, right| right.4.cmp(&left.4).then_with(|| left.1.cmp(&right.1)));
        rows
    };
    out
}

#[allow(dead_code)]
fn collect_grouped(
    dir: &std::path::Path,
    cutoff_day: &str,
    roster: &crate::data::Roster,
    out: &mut GroupedUsage,
    by_day: &mut std::collections::BTreeMap<String, (i64, i64, i64)>,
    by_bot: &mut std::collections::HashMap<String, (i64, i64, i64)>,
    by_model: &mut std::collections::HashMap<(String, String), (i64, i64, i64)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_grouped(&path, cutoff_day, roster, out, by_day, by_bot, by_model);
            continue;
        }
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        if !name.ends_with(".jsonl") || name.ends_with(".progress.jsonl") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut bot_label: Option<String> = None;
        let mut cwd = String::new();
        let mut touched = false;
        for line in text.lines() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            match value.get("kind").and_then(|item| item.as_str()) {
                Some("header") => {
                    cwd = value
                        .get("cwd")
                        .and_then(|item| item.as_str())
                        .unwrap_or("")
                        .to_string();
                    bot_label = value
                        .get("metadata")
                        .and_then(|metadata| {
                            metadata
                                .get("botName")
                                .and_then(|item| item.as_str())
                                .or_else(|| {
                                    metadata.get("botId").and_then(|item| item.as_str())
                                })
                        })
                        .map(str::to_string);
                    continue;
                }
                Some("entry") => {}
                _ => continue,
            }
            let Some(message) = value.get("message") else {
                continue;
            };
            if message.get("kind").and_then(|item| item.as_str()) != Some("assistant") {
                continue;
            }
            let Some(usage) = message.get("usage") else {
                continue;
            };
            let at = value
                .get("timestamp")
                .and_then(|item| item.as_u64())
                .or_else(|| message.get("timestamp").and_then(|item| item.as_u64()))
                .unwrap_or(0);
            if at == 0 {
                continue;
            }
            let day = crate::time::format_day(at);
            if day.as_str() < cutoff_day {
                continue;
            }
            touched = true;
            let input = usage.get("input").and_then(|v| v.as_i64()).unwrap_or(0);
            let output = usage.get("output").and_then(|v| v.as_i64()).unwrap_or(0);
            let cache = usage
                .get("cacheRead")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let total = usage
                .get("totalTokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(input + output);
            out.total.turns += 1;
            out.total.input_tokens += input;
            out.total.output_tokens += output;
            out.total.total_tokens += total;
            let slot = by_day.entry(day).or_insert((0, 0, 0));
            slot.0 += input;
            slot.1 += output;
            slot.2 += cache;
            let bot_key = bot_label
                .clone()
                .or_else(|| bot_name_from_cwd(&cwd, roster))
                .unwrap_or_else(|| "未归属".to_string());
            let bot_slot = by_bot.entry(bot_key).or_insert((0, 0, 0));
            bot_slot.0 += 1;
            bot_slot.1 += input;
            bot_slot.2 += output;
            let provider = message
                .get("provider")
                .and_then(|item| item.as_str())
                .unwrap_or("未知")
                .to_string();
            let model = message
                .get("model")
                .and_then(|item| item.as_str())
                .unwrap_or("未知")
                .to_string();
            let model_slot = by_model.entry((provider, model)).or_insert((0, 0, 0));
            model_slot.0 += 1;
            model_slot.1 += input;
            model_slot.2 += output;
        }
        if touched {
            out.sessions += 1;
        }
    }
}

/// 老会话没有 `metadata.botId`：按 header 的 `cwd` 反查名册目录做 best-effort 归属。
#[allow(dead_code)]
fn bot_name_from_cwd(cwd: &str, roster: &crate::data::Roster) -> Option<String> {
    let cwd = cwd.trim();
    if cwd.is_empty() {
        return None;
    }
    roster
        .bots
        .iter()
        .chain(roster.hidden_bots.iter())
        .find(|bot| !bot.directory.trim().is_empty() && bot.directory.eq_ignore_ascii_case(cwd))
        .map(|bot| bot.name.clone())
}

#[allow(dead_code)]
fn collect_daily(
    dir: &std::path::Path,
    buckets: &mut std::collections::HashMap<String, (i64, i64, i64)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_daily(&path, buckets);
            continue;
        }
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        if !name.ends_with(".jsonl") || name.ends_with(".progress.jsonl") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if value.get("kind").and_then(|item| item.as_str()) != Some("entry") {
                continue;
            }
            let Some(message) = value.get("message") else {
                continue;
            };
            if message.get("kind").and_then(|item| item.as_str()) != Some("assistant") {
                continue;
            }
            let Some(usage) = message.get("usage") else {
                continue;
            };
            let at = value
                .get("timestamp")
                .and_then(|item| item.as_u64())
                .or_else(|| message.get("timestamp").and_then(|item| item.as_u64()))
                .unwrap_or(0);
            if at == 0 {
                continue;
            }
            let day = crate::time::format_day(at);
            let input = usage.get("input").and_then(|v| v.as_i64()).unwrap_or(0);
            let output = usage.get("output").and_then(|v| v.as_i64()).unwrap_or(0);
            let cache = usage
                .get("cacheRead")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let slot = buckets.entry(day).or_insert((0, 0, 0));
            slot.0 += input;
            slot.1 += output;
            slot.2 += cache;
        }
    }
}

/// 一个 JSONL 会话的累计用量（assistant 消息里的 usage）。
///
/// 用量页过去读 `db.sqlite` 的 turn_usage：内嵌引擎的会话是 JSONL，那条路永远是空的。
pub fn session_usage(path: &str) -> Option<crate::session_db::UsageSummary> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut summary = crate::session_db::UsageSummary::default();
    accumulate_usage(&text, &mut summary);
    (summary.turns > 0).then_some(summary)
}

/// 全库累计：扫本机所有 JSONL 会话（进度文件不含用量，跳过）。
///
/// 用量页已改用 `usage_grouped(days).total`（同一份快照跟范围走），
/// 这个全库入口暂时没有调用方；保留给旧调用方/测试，标 `dead_code` 不算债。
#[allow(dead_code)]
pub fn usage_totals() -> Option<crate::session_db::UsageSummary> {
    let mut summary = crate::session_db::UsageSummary::default();
    collect_usage(&sessions_root(), &mut summary);
    (summary.turns > 0).then_some(summary)
}

fn accumulate_usage(text: &str, summary: &mut crate::session_db::UsageSummary) {
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("kind").and_then(|item| item.as_str()) != Some("entry") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        if message.get("kind").and_then(|item| item.as_str()) != Some("assistant") {
            continue;
        }
        let Some(usage) = message.get("usage") else {
            continue;
        };
        let input = usage.get("input").and_then(|v| v.as_i64()).unwrap_or(0);
        let output = usage.get("output").and_then(|v| v.as_i64()).unwrap_or(0);
        let total = usage
            .get("totalTokens")
            .and_then(|v| v.as_i64())
            .unwrap_or(input + output);
        summary.turns += 1;
        summary.input_tokens += input;
        summary.output_tokens += output;
        summary.total_tokens += total;
    }
}

#[allow(dead_code)]
fn collect_usage(dir: &std::path::Path, summary: &mut crate::session_db::UsageSummary) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_usage(&path, summary);
            continue;
        }
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        if !name.ends_with(".jsonl") || name.ends_with(".progress.jsonl") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            accumulate_usage(&text, summary);
        }
    }
}

/// 把待发附件转成 pi-rust 的 inline image；非图片/超限直接报错（当前引擎不支持任意文件）。
pub fn images_from_attachments(files: &[AttachmentUpload]) -> Result<Vec<ImageContent>, String> {
    use base64::Engine as _;

    let mut images = Vec::new();
    for file in files {
        let mime = match file.mime.trim().to_ascii_lowercase().as_str() {
            "image/png" | "png" => "image/png",
            "image/jpeg" | "image/jpg" | "jpeg" | "jpg" => "image/jpeg",
            _ => {
                return Err(format!(
                    "当前引擎只支持 PNG/JPEG 图片附件：{}",
                    file.file_name
                ))
            }
        };
        if file.bytes.len() > IMAGE_MAX_BYTES {
            return Err(format!(
                "图片超过 256KB：{}（{} 字节）",
                file.file_name,
                file.bytes.len()
            ));
        }
        if images.len() >= IMAGE_MAX_COUNT {
            return Err(format!("一次最多发送 {IMAGE_MAX_COUNT} 张图片"));
        }
        let data = base64::engine::general_purpose::STANDARD.encode(&file.bytes);
        let image: ImageContent = serde_json::from_value(serde_json::json!({
            "type": "image",
            "data": data,
            "mimeType": mime,
        }))
        .map_err(|error| format!("图片附件编码失败：{error}"))?;
        images.push(image);
    }
    Ok(images)
}

// ── AskUserQuestion：向用户提问并等待作答 ────────────────────────────────

pub struct QuestionGate {
    events: UnboundedSender<BackendEvent>,
    key: String,
    member: Option<String>,
    pending: Mutex<HashMap<String, oneshot::Sender<Option<String>>>>,
    seq: std::sync::atomic::AtomicU64,
}

impl QuestionGate {
    pub fn new(
        events: UnboundedSender<BackendEvent>,
        key: String,
        member: Option<String>,
    ) -> Self {
        Self {
            events,
            key,
            member,
            pending: Mutex::new(HashMap::new()),
            seq: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn tool(self: &Arc<Self>) -> HarnessTool {
        HarnessTool::new(Arc::new(AskUserQuestionTool {
            gate: Arc::clone(self),
            tool: rpi_ai::types::Tool {
                name: "ask_user_question".to_string(),
                description: "向用户提出一个问题并等待回答；可提供选项，也可让用户自由输入。"
                    .to_string(),
                parameters: rpi_ai::Schema(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "question": {"type": "string"},
                        "options": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "可选：给出几个选项"
                        }
                    },
                    "required": ["question"]
                })),
                constrained_sampling: None,
            },
        }))
    }

    /// O02：`request_secret` 工具——向用户要一个密钥值，落 `secrets.json`，
    /// 只把「名字」回给模型（明文不进对话历史、不进 prompt）。
    pub fn secret_tool(self: &Arc<Self>, data_root: std::path::PathBuf) -> HarnessTool {
        HarnessTool::new(Arc::new(RequestSecretTool {
            gate: Arc::clone(self),
            data_root,
            tool: rpi_ai::types::Tool {
                name: "request_secret".to_string(),
                description: "向用户安全地要一个密钥（API Key、token、密码等）：值直接存进本机密钥库，                              聊天记录与模型上下文里都不会出现明文。之后用 ${secret:NAME} 引用。"
                    .to_string(),
                parameters: rpi_ai::Schema(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "密钥名（保存与引用用，如 OPENAI_API_KEY）"},
                        "description": {"type": "string", "description": "这个密钥用在哪（给用户看的说明）"}
                    },
                    "required": ["name"]
                })),
                constrained_sampling: None,
            },
        }))
    }

    pub fn answer(&self, request_id: &str, answer: Option<String>) -> bool {
        let sender = self
            .pending
            .lock()
            .expect("question pending poisoned")
            .remove(request_id);
        match sender {
            Some(sender) => {
                let _ = sender.send(answer);
                true
            }
            None => false,
        }
    }

    async fn ask(
        &self,
        question: String,
        options: Vec<String>,
        signal: tokio_util::sync::CancellationToken,
    ) -> Result<String, String> {
        let id = format!(
            "question-{}-{}",
            std::process::id(),
            self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("question pending poisoned")
            .insert(id.clone(), tx);
        let _ = self.events.send(BackendEvent::Session {
            key: self.key.clone(),
            event: SessionEvent::Question(crate::backend::QuestionCard {
                request_id: id.clone(),
                question,
                options: options
                    .into_iter()
                    .map(|option| (option.clone(), option))
                    .collect(),
                member: self.member.clone(),
            }),
        });
        tokio::select! {
            answer = rx => {
                self.pending
                    .lock()
                    .expect("question pending poisoned")
                    .remove(&id);
                let _ = self.events.send(BackendEvent::Session {
                    key: self.key.clone(),
                    event: SessionEvent::QuestionResolved { request_id: id },
                });
                answer
                    .map_err(|_| "提问通道关闭".to_string())?
                    .ok_or_else(|| "用户取消了提问".to_string())
            }
            _ = signal.cancelled() => {
                self.pending
                    .lock()
                    .expect("question pending poisoned")
                    .remove(&id);
                Err("提问已取消".to_string())
            }
        }
    }
}

struct AskUserQuestionTool {
    gate: Arc<QuestionGate>,
    tool: rpi_ai::types::Tool,
}

#[async_trait::async_trait]
impl AgentTool for AskUserQuestionTool {
    fn schema(&self) -> &rpi_ai::types::Tool {
        &self.tool
    }

    fn label(&self) -> &str {
        &self.tool.name
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        signal: tokio_util::sync::CancellationToken,
        _on_update: Arc<dyn Fn(rpi_agent::ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, rpi_agent::AgentError> {
        let question = params
            .get("question")
            .and_then(|value| value.as_str())
            .ok_or_else(|| rpi_agent::AgentError::Tool("缺少 question".to_string()))?
            .to_string();
        let options = params
            .get("options")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let answer = self
            .gate
            .ask(question, options, signal)
            .await
            .map_err(rpi_agent::AgentError::Tool)?;
        Ok(AgentToolResult {
            content: vec![rpi_agent::TextContentOrImage::Text(rpi_ai::TextContent {
                kind: rpi_ai::types::TextContentType,
                text: answer,
                text_signature: None,
            })],
            details: serde_json::Value::Null,
            usage: None,
            added_tool_names: Vec::new(),
            terminate: false,
        })
    }
}

struct RequestSecretTool {
    gate: Arc<QuestionGate>,
    data_root: std::path::PathBuf,
    tool: rpi_ai::types::Tool,
}

#[async_trait::async_trait]
impl AgentTool for RequestSecretTool {
    fn schema(&self) -> &rpi_ai::types::Tool {
        &self.tool
    }

    fn label(&self) -> &str {
        &self.tool.name
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        signal: tokio_util::sync::CancellationToken,
        _on_update: Arc<dyn Fn(rpi_agent::ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, rpi_agent::AgentError> {
        let name = params
            .get("name")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| rpi_agent::AgentError::Tool("缺少 name".to_string()))?
            .to_string();
        let purpose = params
            .get("description")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let question = if purpose.is_empty() {
            format!("请输入 {name} 的值（保存到本机密钥库，不会出现在对话里）")
        } else {
            format!("请输入 {name} 的值（{purpose}；保存到本机密钥库，不会出现在对话里）")
        };
        let answer = self
            .gate
            .ask(question, Vec::new(), signal)
            .await
            .map_err(rpi_agent::AgentError::Tool)?;
        let value = answer.trim().to_string();
        if value.is_empty() {
            return Err(rpi_agent::AgentError::Tool("已取消（没有输入）".to_string()));
        }
        crate::secrets::set(&self.data_root, &name, &value)
            .map_err(rpi_agent::AgentError::Tool)?;
        Ok(AgentToolResult {
            content: vec![rpi_agent::TextContentOrImage::Text(rpi_ai::TextContent {
                kind: rpi_ai::types::TextContentType,
                text: format!("已保存密钥 {name}（值不入对话；需要用 ${{secret:{name}}} 引用）"),
                text_signature: None,
            })],
            details: serde_json::Value::Null,
            usage: None,
            added_tool_names: Vec::new(),
            terminate: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ask_user_question_roundtrip() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<BackendEvent>();
        let gate = Arc::new(QuestionGate::new(tx, "test".to_string(), None));
        let tool = gate.tool();
        let task = tokio::spawn({
            let tool = Arc::clone(&tool.tool);
            async move {
                tool.execute(
                    "id",
                    serde_json::json!({"question":"选一个","options":["A","B"]}),
                    tokio_util::sync::CancellationToken::new(),
                    Arc::new(|_| {}),
                )
                .await
            }
        });
        let event = rx.recv().await.expect("event");
        let BackendEvent::Session { event: SessionEvent::Question(card), .. } = event else {
            panic!("expected question card");
        };
        assert_eq!(card.question, "选一个");
        assert!(gate.answer(&card.request_id, Some("A".to_string())));
        let result = task.await.expect("join").expect("execute");
        match &result.content[0] {
            rpi_agent::TextContentOrImage::Text(text) => assert_eq!(text.text, "A"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn allow_always_updates_gate_allowlist() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<BackendEvent>();
        let gate = ApprovalGate::new(
            tx,
            "test".to_string(),
            None,
            bot_team::approval::ApprovalLevel::Ask,
            Vec::new(),
            None,
        );
        let (answer, mut answered) = oneshot::channel();
        gate.pending
            .lock()
            .expect("approval pending")
            .insert(
                "call-1".to_string(),
                PendingApproval {
                    tool_name: "browser_open".to_string(),
                    sender: answer,
                },
            );
        assert!(gate.resolve("call-1", "allow_always"));
        assert_eq!(gate.allowlist(), vec!["browser_open".to_string()]);
        assert!(answered.try_recv().expect("answer sent"));
    }

    /// 造一条 assistant 消息，只关心 `stop_reason` / `error_message`。
    /// 用 JSON 反序列化构造，避免逐个字段改上游结构体时测试跟着塌。
    fn assistant(stop_reason: rpi_ai::StopReason, error: Option<&str>) -> rpi_agent::AgentMessage {
        let mut value = serde_json::json!({
            "role": "assistant",
            "content": [],
            "api": "test",
            "provider": "test",
            "model": "test-model",
            "usage": {
                "input": 0,
                "output": 0,
                "cacheRead": 0,
                "cacheWrite": 0,
                "totalTokens": 0,
                "cost": { "input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0 }
            },
            // StopReason 的 serde 名是 lowercase（`tooluse`），与 `as_str()` 的
            // `toolUse` 不同，必须走序列化取值，别手写字符串。
            "stopReason": serde_json::to_value(stop_reason).expect("stop reason"),
            "timestamp": 0
        });
        if let Some(text) = error {
            value["errorMessage"] = serde_json::Value::String(text.to_string());
        }
        let message: rpi_ai::AssistantMessage =
            serde_json::from_value(value).expect("assistant message fixture");
        rpi_agent::AgentMessage::Assistant(Box::new(message))
    }

    #[test]
    fn stop_reason_error_becomes_session_error() {
        // provider 报错（这里用真实的 402 文本）必须转成错误事件，
        // 否则 UI 会当成正常结束，用户只看到「AI 不回话」。
        let messages = vec![assistant(
            rpi_ai::StopReason::Error,
            Some("http error 402: Insufficient Balance"),
        )];
        let failure = agent_end_failure(&messages).expect("must be treated as failure");
        assert!(failure.contains("402"), "错误原文要带给用户：{failure}");
        assert!(failure.contains("Insufficient Balance"), "{failure}");
    }

    #[test]
    fn stop_reason_error_without_message_still_reports() {
        let messages = vec![assistant(rpi_ai::StopReason::Error, None)];
        let failure = agent_end_failure(&messages).expect("must be treated as failure");
        assert!(!failure.is_empty());
    }

    #[test]
    fn normal_and_user_aborted_turns_are_not_failures() {
        // 正常结束、截断、等工具结果都不算失败
        for reason in [
            rpi_ai::StopReason::Stop,
            rpi_ai::StopReason::Length,
            rpi_ai::StopReason::ToolUse,
        ] {
            assert!(
                agent_end_failure(&[assistant(reason, None)]).is_none(),
                "{reason:?} 不该报错"
            );
        }
        // 用户点「停止」不是错误
        assert!(agent_end_failure(&[assistant(rpi_ai::StopReason::Aborted, None)]).is_none());
        // 空消息（纯工具回合）也不报错
        assert!(agent_end_failure(&[]).is_none());
    }

    #[test]
    fn identity_parses_teammate_and_group_member() {        // 私聊
        assert_eq!(
            bot_id_from_identity("mode-bot:teammate:b12").as_deref(),
            Some("b12")
        );
        // 群成员（D-09）：不认这一形态会让群里的 bot_id 变 None，
        // 隐私守卫就把「读自己的目录」判成越界。
        assert_eq!(
            bot_id_from_identity("mode-bot:group:grp-accept-001:member:b12").as_deref(),
            Some("b12")
        );
        // 群 id 自带冒号也要从右取
        assert_eq!(
            bot_id_from_identity("mode-bot:group:9607:member:b12").as_deref(),
            Some("b12")
        );
        // 不是成员身份的群身份、空串、别的前缀都不算
        assert_eq!(bot_id_from_identity("mode-bot:group:grp-accept-001"), None);
        assert_eq!(bot_id_from_identity("mode-bot:group:g1:member:"), None);
        assert_eq!(bot_id_from_identity("other:member:b12"), None);
        assert_eq!(bot_id_from_identity(""), None);
        assert_eq!(bot_id_from_identity("mode-bot:teammate:"), None);
    }

    #[test]
    fn privacy_and_normal_prompts_agree_on_risk() {
        // D-10：同一个工具在隐私越界卡和常规审批卡上必须显示同一档风险。
        assert_eq!(risk_of("read"), "read");
        assert_eq!(risk_of("grep"), "read");
        assert_eq!(risk_of("write"), "write");
        assert_eq!(risk_of("edit"), "write");
        assert_eq!(risk_of("remember"), "write");
        assert_eq!(risk_of("mcp__fs__read"), "write");
        assert_eq!(risk_of("bash"), "exec");
        assert_eq!(risk_of("powershell"), "exec");
        assert_eq!(risk_of("browser_open"), "exec");
        assert_eq!(risk_of("computer_click"), "exec");
        // 大小写不敏感
        assert_eq!(risk_of("READ"), "read");
    }
}
