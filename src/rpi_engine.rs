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
const BUILTIN_CATALOG: &str = include_str!("../assets/provider/mode-builtin.json");

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
    model.context_window = 128_000;
    model.max_tokens = 16_384;
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
    let id = identity
        .strip_prefix("mode-bot:teammate:")
        .unwrap_or(identity)
        .trim();
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
                && item.get("id").and_then(|value| value.as_str()) == Some(id))
                || item
                    .get("directory")
                    .and_then(|value| value.as_str())
                    .is_some_and(|dir| dir.eq_ignore_ascii_case(workspace))
        })
        .cloned()
}

/// 该 Bot 的审批档位与白名单（读同一份 teammates.json）。
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

fn sessions_root() -> PathBuf {
    crate::data::data_root()
        .join("v2")
        .join("mode-chat-sessions")
}

/// 会话系统提示：工作目录 AGENTS.md（身份/团队约定）+ 团队记忆。
pub fn system_prompt_for(workspace: &Path) -> String {
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
    allowlist: Vec<String>,
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

impl ApprovalGate {
    pub fn new(
        events: UnboundedSender<BackendEvent>,
        key: String,
        member: Option<String>,
        level: bot_team::approval::ApprovalLevel,
        allowlist: Vec<String>,
    ) -> Self {
        Self {
            events,
            key,
            member,
            level,
            allowlist,
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
        use bot_team::approval::{decide, ApprovalDecision};

        // 无人值守测试/自动化：显式跳过审批（仅测试环境使用）。
        if std::env::var("BOT_PILOT_AUTO_APPROVE").ok().as_deref() == Some("1") {
            return None;
        }
        match decide(self.level, &self.allowlist, &tool_name) {
            ApprovalDecision::Allow => None,
            ApprovalDecision::Deny => Some(blocked("已拒绝（审批策略）")),
            ApprovalDecision::Ask => {
                let (tx, rx) = oneshot::channel();
                self.pending
                    .lock()
                    .expect("approval pending poisoned")
                    .insert(request_id.clone(), tx);
                // 风险档位按「实际副作用」分：本地执行/浏览器/电脑控制是 exec，写文件、
                // 定时任务与 MCP 外部工具是 write，其余（读文件、提问）是 read。
                // 自定义工具以前一律落到 read，审批卡上会把「建定时任务」标成只读。
                let lowered = tool_name.to_ascii_lowercase();
                let risk = if lowered == "bash"
                    || lowered.starts_with("computer_")
                    || lowered.starts_with("browser_")
                {
                    "exec"
                } else if lowered == "write"
                    || lowered == "edit"
                    || lowered.starts_with("automation_")
                    || lowered.starts_with("mcp__")
                {
                    "write"
                } else {
                    "read"
                };
                let _ = self.events.send(BackendEvent::Session {
                    key: self.key.clone(),
                    event: SessionEvent::Permission(PermissionPrompt {
                        request_id: request_id.clone(),
                        tool_name: tool_name.clone(),
                        reason: summarize_args(&args),
                        risk: risk.to_string(),
                        options: vec![
                            PromptOption {
                                option_id: "allow_once".to_string(),
                                label: "允许一次".to_string(),
                                kind: "allow_once".to_string(),
                            },
                            PromptOption {
                                option_id: "allow_always".to_string(),
                                label: "始终允许".to_string(),
                                kind: "allow_always".to_string(),
                            },
                            PromptOption {
                                option_id: "deny".to_string(),
                                label: "拒绝".to_string(),
                                kind: "deny".to_string(),
                            },
                        ],
                        member: self.member.clone(),
                    }),
                });
                tokio::select! {
                    answer = rx => {
                        self.pending
                            .lock()
                            .expect("approval pending poisoned")
                            .remove(&request_id);
                        let _ = self.events.send(BackendEvent::Session {
                            key: self.key.clone(),
                            event: SessionEvent::PermissionResolved {
                                request_id: request_id.clone(),
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
                            .remove(&request_id);
                        Some(blocked("已取消"))
                    }
                }
            }
        }
    }

    /// 返回 true 表示这个 request_id 属于本闸门（已作答）。
    pub fn resolve(&self, request_id: &str, allow: bool) -> bool {
        let sender = self
            .pending
            .lock()
            .expect("approval pending poisoned")
            .remove(request_id);
        match sender {
            Some(sender) => {
                let _ = sender.send(allow);
                true
            }
            None => false,
        }
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
                }));
            }
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                tool_name,
                partial_result,
                ..
            } => {
                self.send(SessionEvent::Tool(ToolCard {
                    call_id: tool_call_id,
                    name: tool_name.clone(),
                    state: "running".to_string(),
                    title: tool_name,
                    detail: tool_detail(&partial_result.content),
                }));
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
                ..
            } => {
                self.send(SessionEvent::Tool(ToolCard {
                    call_id: tool_call_id,
                    name: tool_name.clone(),
                    state: if is_error { "error" } else { "done" }.to_string(),
                    title: tool_name,
                    detail: tool_detail(&result.content),
                }));
            }
            AgentEvent::RetryScheduled { error, attempt, max_retries, .. } => {
                self.send(SessionEvent::Error {
                    member: self.member.clone(),
                    message: format!("请求失败，重试中（{attempt}/{max_retries}）：{error}"),
                });
            }
            AgentEvent::AgentEnd { .. } => {
                self.send(SessionEvent::Done {
                    member: self.member.clone(),
                });
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
    /// 额外工具（MCP / 浏览器 / 电脑控制等宿主能力）。
    pub extra_tools: Vec<HarnessTool>,
}

pub struct RpiSession {
    _harness: AgentHarness,
    lane: Arc<dyn AgentLane>,
    pub path: String,
    pub gate: Arc<ApprovalGate>,
    pub questions: Arc<QuestionGate>,
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
        let root = sessions_root();
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
                let storage = repo
                    .create_typed(&JsonlSessionCreateOptions {
                        id: None,
                        parent_session_id: None,
                        cwd: workspace.to_string_lossy().into_owned(),
                        metadata: None,
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
        let harness = AgentHarness::create(harness_options)
            .await
            .map_err(|error| format!("Agent 会话初始化失败：{error}"))?;
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
        })
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

    pub async fn abort(&self) {
        let _ = self.lane.abort().await;
    }
}


/// 从 JSONL 会话文件还原用户/助手文本（重启后的历史回放）。
pub fn history_entries(path: &str) -> Vec<crate::backend::HistoryEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
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
    collect_search(&sessions_root(), &needle, limit, &mut hits);
    hits.sort_by_key(|hit| std::cmp::Reverse(hit.at));
    hits.truncate(limit);
    hits
}

fn collect_search(
    dir: &std::path::Path,
    needle: &str,
    limit: usize,
    out: &mut Vec<crate::session_db::SearchHit>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_search(&path, needle, limit, out);
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
}
