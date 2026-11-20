//! GPUI 内置的 automation 调度器（WebView 退役后补上）。
//!
//! 复用 `automation-runtime` 的状态机（20s 认领 → 派发 → 终态结算），本模块只做宿主
//! 接线：v4 `createSession → sendText` + 轮询快照到终态，以及一个透传的模型选择端口。
//! 会话归属仍是 Bot 的 workspace/identity，运行历史写同一个 `tasks-index.sqlite`。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_process::{AgentControlClient, AgentWorkspaceTarget, RuntimePolicy};
use agent_protocol::snapshot::SessionStateSnapshot;
use agent_protocol::v4::{CommandSessionId, CommandType, ConversationCommandEnvelope};
use automation_repo::{AutomationRepo, ModelSelection};
use automation_runtime::{
    AutomationRuntime, AutomationRuntimeOptions, DispatchPort, ModelSelectionPort,
    ModelSelectionView, PortDispatchRequest, PortDispatchResult, RuntimeFuture, TerminalOutcome,
    Unsubscribe,
};

const TERMINAL_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const DEFAULT_POLL_MS: u64 = 20_000;

/// 启动调度器；需要当前线程在 Tokio runtime 内（GPUI 后端满足）。
pub fn start(control: Arc<AgentControlClient>, data_root: &Path) -> Result<(), String> {
    let db_path = data_root.join("v2").join("tasks-index.sqlite");
    let repo = Arc::new(AutomationRepo::with_busy_timeout(db_path, 5_000));
    let dispatch: Arc<dyn DispatchPort> =
        if std::env::var("MODE_CHAT_ENGINE").ok().as_deref() == Some("v4") {
            Arc::new(V4DispatchPort {
                control,
                seq: Arc::new(AtomicU64::new(0)),
            })
        } else {
            Arc::new(RpiDispatchPort {
                data_root: data_root.to_path_buf(),
            })
        };
    let default_model = crate::data::available_models()
        .into_iter()
        .next()
        .map(|model| ModelSelection {
            provider_id: model.provider_id,
            model_id: model.model_id,
            options: None,
        });
    let model_selection = Arc::new(DefaultModelSelection { default_model });
    let mut options = AutomationRuntimeOptions::new(repo, dispatch, model_selection);
    options.utc_offset_secs = crate::time::local_offset_secs() as i32;
    options.poll_interval_ms = DEFAULT_POLL_MS;
    options.log = Arc::new(|level, message| {
        if level != "info" || std::env::var("BOT_PILOT_DEBUG").is_ok() {
            eprintln!("[automation] {level}: {message}");
        }
    });
    let runtime = AutomationRuntime::new(options);
    runtime.start();
    Ok(())
}

/// 内嵌 pi-rust 的 automation 派发：直接跑回合，终态由 AgentEnd/错误决定。
struct RpiDispatchPort {
    data_root: PathBuf,
}

impl DispatchPort for RpiDispatchPort {
    fn dispatch(
        &self,
        request: PortDispatchRequest,
        on_terminal: Arc<dyn Fn(TerminalOutcome) + Send + Sync>,
    ) -> RuntimeFuture<'_, Result<(PortDispatchResult, Unsubscribe), String>> {
        let _data_root = self.data_root.clone();
        Box::pin(async move {
            let identity = request.workspace_identity.clone().unwrap_or_default();
            let spec = crate::data::ModelSpec {
                provider_id: request.model_selection.provider_id.clone(),
                model_id: request.model_selection.model_id.clone(),
                reasoning_level: request
                    .model_selection
                    .options
                    .as_ref()
                    .and_then(|options| options.reasoning_level.clone()),
            };
            let engine_model = crate::rpi_engine::resolve_model(&spec)?;
            let (approval_level, allowlist) =
                crate::rpi_engine::approval_policy(&identity, &request.workspace_path);
            let system_prompt =
                crate::rpi_engine::system_prompt_for(Path::new(&request.workspace_path));
            let (tx, mut rx) =
                tokio::sync::mpsc::unbounded_channel::<crate::backend::BackendEvent>();
            let options = crate::rpi_engine::OpenOptions {
                workspace: request.workspace_path.clone().into(),
                resume_path: None,
                model: engine_model,
                thinking: crate::rpi_engine::thinking_level_for(&spec),
                system_prompt,
                events: tx,
                key: request.run_id.clone(),
                member: None,
                approval_level,
                allowlist,
            };
            let session = Arc::new(crate::rpi_engine::RpiSession::open(options).await?);
            let state = Arc::new(Mutex::new(RpiRunState::default()));
            let state_for_reader = Arc::clone(&state);
            tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    let crate::backend::BackendEvent::Session { event, .. } = event else {
                        continue;
                    };
                    match event {
                        crate::backend::SessionEvent::Delta { text, .. } => {
                            state_for_reader
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .text
                                .push_str(&text);
                        }
                        crate::backend::SessionEvent::Done { .. } => {
                            state_for_reader
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .running = false;
                        }
                        crate::backend::SessionEvent::Error { message, .. } => {
                            let mut state = state_for_reader
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            state.error = Some(message);
                            state.running = false;
                        }
                        _ => {}
                    }
                }
            });
            state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .running = true;
            let session_for_send = Arc::clone(&session);
            let prompt = request.prompt.clone();
            tokio::spawn(async move {
                session_for_send.send(prompt, Vec::new()).await;
            });

            let task_id = session.path.clone();
            let input_id = request.run_id.clone();
            let stop = Arc::new(AtomicBool::new(false));
            let stop_flag = Arc::clone(&stop);
            let session_for_watch = Arc::clone(&session);
            let state_for_watch = Arc::clone(&state);
            let task_for_watch = task_id.clone();
            tokio::spawn(async move {
                watch_rpi_terminal(
                    session_for_watch,
                    state_for_watch,
                    task_for_watch,
                    input_id,
                    stop_flag,
                    on_terminal,
                )
                .await;
            });
            let unsubscribe: Unsubscribe = Box::new(move || stop.store(true, Ordering::SeqCst));
            Ok((
                PortDispatchResult {
                    task_id: task_id.clone(),
                    session_id: session.path.clone(),
                },
                unsubscribe,
            ))
        })
    }

    fn mark_task_unread(
        &self,
        _task_id: &str,
        _workspace_path: &str,
        _workspace_identity: Option<&str>,
    ) {
    }
}

#[derive(Default)]
struct RpiRunState {
    running: bool,
    text: String,
    error: Option<String>,
}

async fn watch_rpi_terminal(
    session: Arc<crate::rpi_engine::RpiSession>,
    state: Arc<Mutex<RpiRunState>>,
    task_id: String,
    input_id: String,
    stop: Arc<AtomicBool>,
    on_terminal: Arc<dyn Fn(TerminalOutcome) + Send + Sync>,
) {
    let started_at = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if stop.load(Ordering::SeqCst) {
            session.abort().await;
            return;
        }
        if started_at.elapsed() > TERMINAL_TIMEOUT {
            session.abort().await;
            on_terminal(TerminalOutcome {
                task_id,
                input_id,
                outcome: "failed".to_string(),
                error: Some("automation turn timeout".to_string()),
            });
            return;
        }
        let (running, error) = {
            let state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (state.running, state.error.clone())
        };
        if running {
            continue;
        }
        let (outcome, error) = match error {
            Some(error) => ("failed".to_string(), Some(error)),
            None => ("succeeded".to_string(), None),
        };
        on_terminal(TerminalOutcome {
            task_id,
            input_id,
            outcome,
            error,
        });
        return;
    }
}

struct V4DispatchPort {
    control: Arc<AgentControlClient>,
    seq: Arc<AtomicU64>,
}

impl DispatchPort for V4DispatchPort {
    fn dispatch(
        &self,
        request: PortDispatchRequest,
        on_terminal: Arc<dyn Fn(TerminalOutcome) + Send + Sync>,
    ) -> RuntimeFuture<'_, Result<(PortDispatchResult, Unsubscribe), String>> {
        let control = Arc::clone(&self.control);
        let seq = Arc::clone(&self.seq);
        Box::pin(async move {
            let mut target = AgentWorkspaceTarget::new(request.workspace_path.clone());
            if let Some(identity) = request
                .workspace_identity
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                target = target.with_identity(identity.to_string());
            }
            let client_id = format!("bot-gpui-automation-{}", request.run_id);
            let session_id = create_session(
                &control,
                &seq,
                &target,
                &request.workspace_path,
                request.workspace_identity.as_deref(),
                &client_id,
            )
            .await?;
            let selection = serde_json::to_value(&request.model_selection)
                .map_err(|error| format!("modelSelection 序列化失败：{error}"))?;
            send_text(
                &control,
                &seq,
                &target,
                &client_id,
                &session_id,
                &request.prompt,
                Some(selection),
            )
            .await?;

            let stop = Arc::new(AtomicBool::new(false));
            let stop_flag = Arc::clone(&stop);
            let task_id = session_id.clone();
            let input_id = request.run_id.clone();
            let control_for_watch = Arc::clone(&control);
            let target_for_watch = target.clone();
            let session_for_watch = session_id.clone();
            tokio::spawn(async move {
                watch_terminal(
                    control_for_watch,
                    target_for_watch,
                    session_for_watch,
                    task_id,
                    input_id,
                    stop_flag,
                    on_terminal,
                )
                .await;
            });
            let unsubscribe: Unsubscribe = Box::new(move || stop.store(true, Ordering::SeqCst));
            Ok((
                PortDispatchResult {
                    task_id: session_id.clone(),
                    session_id,
                },
                unsubscribe,
            ))
        })
    }

    fn mark_task_unread(
        &self,
        task_id: &str,
        workspace_path: &str,
        _workspace_identity: Option<&str>,
    ) {
        // GPUI 没有 Node task 的未读标记位；保留诊断，不阻断结算。
        if std::env::var("BOT_PILOT_DEBUG").is_ok() {
            eprintln!("[automation] markTaskUnread skipped task={task_id} ws={workspace_path}");
        }
    }
}

async fn create_session(
    control: &AgentControlClient,
    seq: &AtomicU64,
    target: &AgentWorkspaceTarget,
    workspace_path: &str,
    workspace_identity: Option<&str>,
    client_id: &str,
) -> Result<String, String> {
    let connectors = crate::connectors::payload_for_workspace(&crate::data::data_root(), workspace_path);
    let mut payload = serde_json::json!({
        "workspaceId": workspace_path,
        "mcpServers": connectors,
    });
    if let Some(identity) = workspace_identity
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        payload["workspaceIdentity"] = serde_json::Value::String(identity.to_string());
    }
    let envelope = envelope(client_id, seq, CommandType::CreateSession, None, payload);
    let ack = control
        .send_conversation_command_v4(target, &envelope)
        .await
        .map_err(|error| format!("automation createSession 失败：{error}"))?;
    if !ack.status.is_success() {
        return Err(format!(
            "automation createSession 未受理（{}）：{}",
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
        .ok_or_else(|| "automation createSession 没有返回 sessionId".to_string())
}

async fn send_text(
    control: &AgentControlClient,
    seq: &AtomicU64,
    target: &AgentWorkspaceTarget,
    client_id: &str,
    session_id: &str,
    text: &str,
    model_selection: Option<serde_json::Value>,
) -> Result<(), String> {
    let mut payload = serde_json::json!({ "text": text });
    if let Some(selection) = model_selection {
        payload["modelSelection"] = selection;
    }
    let envelope = envelope(
        client_id,
        seq,
        CommandType::SendText,
        Some(session_id),
        payload,
    );
    let ack = control
        .send_conversation_command_v4(target, &envelope)
        .await
        .map_err(|error| format!("automation sendText 失败：{error}"))?;
    if ack.status.is_success() {
        Ok(())
    } else {
        Err(format!(
            "automation sendText 未受理（{}）：{}",
            ack.status.as_str(),
            ack.message.unwrap_or_default()
        ))
    }
}

fn envelope(
    client_id: &str,
    seq: &AtomicU64,
    command_type: CommandType,
    session_id: Option<&str>,
    payload: serde_json::Value,
) -> ConversationCommandEnvelope {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    ConversationCommandEnvelope {
        ttft: None,
        command_id: format!(
            "bot-gpui-automation-{}-{}",
            millis,
            seq.fetch_add(1, Ordering::Relaxed)
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

async fn watch_terminal(
    control: Arc<AgentControlClient>,
    target: AgentWorkspaceTarget,
    session_id: String,
    task_id: String,
    input_id: String,
    stop: Arc<AtomicBool>,
    on_terminal: Arc<dyn Fn(TerminalOutcome) + Send + Sync>,
) {
    let started_at = Instant::now();
    let mut seen_running = false;
    loop {
        tokio::time::sleep(Duration::from_millis(1000)).await;
        if stop.load(Ordering::SeqCst) {
            return;
        }
        if started_at.elapsed() > TERMINAL_TIMEOUT {
            on_terminal(TerminalOutcome {
                task_id,
                input_id,
                outcome: "failed".to_string(),
                error: Some("automation turn timeout".to_string()),
            });
            return;
        }
        let snapshot: SessionStateSnapshot = match control
            .read_session_with_policy(
                &target,
                &session_id,
                None,
                Some(30),
                None,
                RuntimePolicy::ExistingOnly,
            )
            .await
        {
            Ok(snapshot) => snapshot,
            // 冷会话读取抖动按继续处理；超时有上限兜底。
            Err(_) => continue,
        };
        let status = serde_json::to_value(snapshot.session.status)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default();
        if status != "idle" {
            seen_running = true;
        }
        let outcome = match status.as_str() {
            "completed" => Some(("succeeded", None)),
            "error" => Some(("failed", Some("session error".to_string()))),
            "idle" if seen_running => Some(("succeeded", None)),
            _ => None,
        };
        if let Some((outcome, error)) = outcome {
            on_terminal(TerminalOutcome {
                task_id,
                input_id,
                outcome: outcome.to_string(),
                error,
            });
            return;
        }
    }
}

/// 模型选择端口：有持久化选择就透传；没有就用数据根里第一个可用模型。
struct DefaultModelSelection {
    default_model: Option<ModelSelection>,
}

impl ModelSelectionPort for DefaultModelSelection {
    fn get_view(
        &self,
        selection: Option<ModelSelection>,
    ) -> RuntimeFuture<'_, Result<ModelSelectionView, String>> {
        let preferred = selection.or_else(|| self.default_model.clone());
        Box::pin(async move {
            Ok(ModelSelectionView {
                selection_issue: preferred.is_none(),
                effective_selection: preferred.clone(),
                preferred_selection: preferred,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(provider: &str, model: &str) -> ModelSelection {
        ModelSelection {
            provider_id: provider.to_string(),
            model_id: model.to_string(),
            options: None,
        }
    }

    #[tokio::test]
    async fn model_selection_uses_persisted_or_default() {
        let fallback = selection("p", "m");
        let port = DefaultModelSelection {
            default_model: Some(fallback.clone()),
        };
        let none = port.get_view(None).await.expect("view");
        assert!(!none.selection_issue);
        assert_eq!(none.effective_selection, Some(fallback.clone()));

        let persisted = selection("p2", "m2");
        let some = port.get_view(Some(persisted.clone())).await.expect("view");
        assert_eq!(some.effective_selection, Some(persisted));
        assert_eq!(some.preferred_selection, some.effective_selection);
    }

    #[tokio::test]
    async fn missing_default_reports_selection_issue() {
        let port = DefaultModelSelection { default_model: None };
        let view = port.get_view(None).await.expect("view");
        assert!(view.selection_issue);
        assert!(view.effective_selection.is_none());
        assert!(view.preferred_selection.is_none());
    }
}
