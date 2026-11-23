//! GPUI 内置的 automation 调度器（WebView 退役后补上）。
//!
//! 复用 `automation-runtime` 的状态机（20s 认领 → 派发 → 终态结算），本模块只做宿主
//! 接线：v4 `createSession → sendText` + 轮询快照到终态，以及一个透传的模型选择端口。
//! 会话归属仍是 Bot 的 workspace/identity，运行历史写同一个 `tasks-index.sqlite`。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use automation_repo::{AutomationRepo, ModelSelection};
use automation_runtime::{
    AutomationRuntime, AutomationRuntimeOptions, DispatchPort, ModelSelectionPort,
    ModelSelectionView, PortDispatchRequest, PortDispatchResult, RuntimeFuture, TerminalOutcome,
    Unsubscribe,
};

const TERMINAL_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const DEFAULT_POLL_MS: u64 = 20_000;

/// 启动调度器；需要当前线程在 Tokio runtime 内（GPUI 后端满足）。
pub fn start(data_root: &Path) -> Result<(), String> {
    let db_path = data_root.join("v2").join("tasks-index.sqlite");
    let repo = Arc::new(AutomationRepo::with_busy_timeout(db_path, 5_000));
    let dispatch: Arc<dyn DispatchPort> = Arc::new(RpiDispatchPort {
        data_root: data_root.to_path_buf(),
    });
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
            let mcp_tools = crate::mcp_host::for_workspace(&request.workspace_path)
                .await
                .harness_tools();
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
                bot_id: crate::rpi_engine::bot_id_from_identity(&identity),
                extra_tools: mcp_tools,
                private_bot: crate::rpi_engine::private_bot_id(&identity),
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
