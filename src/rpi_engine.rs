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
    AgentEmitter, AgentEvent, BeforeToolCall, BeforeToolCallContext, BeforeToolCallResult,
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
    if let Some(record) = teammate_record(identity, workspace) {
        if let Some(selection) = record.get("modelSelection") {
            if let (Some(provider_id), Some(model_id)) = (
                selection.get("providerId").and_then(|value| value.as_str()),
                selection.get("modelId").and_then(|value| value.as_str()),
            ) {
                return Some(ModelSpec {
                    provider_id: provider_id.to_string(),
                    model_id: model_id.to_string(),
                    reasoning_level: selection
                        .pointer("/options/reasoningLevel")
                        .and_then(|value| value.as_str())
                        .map(str::to_string),
                });
            }
        }
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

        match decide(self.level, &self.allowlist, &tool_name) {
            ApprovalDecision::Allow => None,
            ApprovalDecision::Deny => Some(blocked("已拒绝（审批策略）")),
            ApprovalDecision::Ask => {
                let (tx, rx) = oneshot::channel();
                self.pending
                    .lock()
                    .expect("approval pending poisoned")
                    .insert(request_id.clone(), tx);
                let risk = if tool_name.eq_ignore_ascii_case("bash") {
                    "exec"
                } else if tool_name.eq_ignore_ascii_case("write")
                    || tool_name.eq_ignore_ascii_case("edit")
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

    pub fn pending_len(&self) -> usize {
        self.pending.lock().expect("approval pending poisoned").len()
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
}

pub struct RpiSession {
    pub harness: AgentHarness,
    lane: Arc<dyn AgentLane>,
    pub session_id: String,
    pub path: String,
    pub gate: Arc<ApprovalGate>,
}

impl RpiSession {
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

        let mut harness_options = AgentHarnessOptions::default();
        harness_options.model = options.model.model;
        harness_options.models = vec![options.model.provider];
        harness_options.tools = tool_set(&workspace);
        harness_options.active_tool_names = ["read", "write", "edit", "bash"]
            .into_iter()
            .map(String::from)
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

        let harness = AgentHarness::create(harness_options)
            .await
            .map_err(|error| format!("Agent 会话初始化失败：{error}"))?;
        let session_id = harness.session_id();
        if debug {
            eprintln!("[rpi] harness ok id={session_id}");
        }
        let lane = harness.lane("main");
        Ok(Self {
            harness,
            lane,
            session_id,
            path,
            gate,
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
