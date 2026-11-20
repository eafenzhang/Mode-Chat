//! v4 会话面 spike：createSession → subscribe → sendText，打印帧结构。
//!
//! 用途：在把试点会话面切到 v4 之前，先确认命令信封/订阅/增量帧的真实形状。
//! 运行：
//!   MODE_STORAGE_DIR=<tmp> MODE_SESSION_DB_PATH=<tmp>/db.sqlite \
//!   MODE_DATA_BASE_DIR=<..> cargo run --example v4-spike -- <workspace> <identity> [prompt]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    resolve_agent_command, AgentControlClient, AgentProcessError, AgentResolveContext,
    AgentRuntimeRegistry, AgentWorkspaceTarget, SpecFactory,
};
use agent_protocol::v4::{
    CommandSessionId, CommandType, ConversationCommandEnvelope,
};
use agent_protocol::v4_transport::{ConversationWireFrame, V4ClientMode};

#[tokio::main]
async fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let workspace = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "C:\\Users\\Administrator\\mode-bot-ws".to_string());
    let identity = args.get(2).cloned().unwrap_or_default();
    let prompt = args
        .get(3)
        .cloned()
        .unwrap_or_else(|| "只回复两个字：收到".to_string());
    let attachment_path = args.get(4).cloned();

    let base_env: BTreeMap<String, String> = std::env::vars().collect();
    // 与试点同口径：从当前目录向上找 CLI（workspace 目录本身通常没有可执行文件）。
    let upward_start = std::env::current_dir().unwrap_or_else(|_| PathBuf::from(&workspace));
    let factory: SpecFactory = Arc::new(move |target| {
        let context = AgentResolveContext {
            workspace_path: PathBuf::from(&target.workspace_path),
            env: base_env.clone(),
            upward_start: upward_start.clone(),
            resources_dir: None,
            node_candidates: Vec::new(),
            node_fallback: "node".to_string(),
            deployed_native_binary: None,
            presentation_surface: None,
            request_timeout: Duration::from_secs(120),
        };
        resolve_agent_command(&context)
            .map(|resolved| resolved.spec)
            .map_err(|error| AgentProcessError::Spawn {
                command: "mode-agent".to_string(),
                message: error.to_string(),
            })
    });
    let control = AgentControlClient::new(Arc::new(AgentRuntimeRegistry::new(factory)));
    let target = AgentWorkspaceTarget::new(workspace.clone()).with_identity(identity.clone());
    let client_id = format!("v4-spike-{}", std::process::id());
    let millis = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0)
    };

    // 1) createSession
    let mut payload = serde_json::json!({ "workspaceId": workspace });
    if !identity.trim().is_empty() {
        payload["workspaceIdentity"] = serde_json::Value::String(identity.clone());
    }
    let envelope = ConversationCommandEnvelope {
        ttft: None,
        command_id: format!("spike-create-{}", millis()),
        client_id: client_id.clone(),
        session_id: CommandSessionId(None),
        base_revision: None,
        base_log_epoch: None,
        command_type: CommandType::CreateSession,
        payload,
        issued_at: serde_json::Number::from(millis()),
    };
    let ack = control
        .send_conversation_command_v4(&target, &envelope)
        .await
        .map_err(|error| format!("createSession 失败：{error}"))?;
    println!("create ack status={:?} result={:?}", ack.status, ack.result);
    let session_id = ack
        .result
        .as_ref()
        .and_then(|value| value.get("sessionId"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "createSession 没有返回 sessionId".to_string())?;
    println!("session={session_id}");

    // 2) subscribe
    let mut subscription = control
        .subscribe_conversation_frames(
            &target,
            &session_id,
            "rust-inprocess",
            V4ClientMode::DesktopContinuous,
            None,
            None,
        )
        .await
        .map_err(|error| format!("订阅失败：{error}"))?;
    println!("subscribe ack={:?}", subscription.ack);

    // 2.5) 可选：并发建 N 个会话，测「第 3+ 个会话偶发 20s+」是否可复现。
    let parallel = std::env::var("V4_PARALLEL")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    if parallel > 0 {
        let unique = std::env::var("V4_PARALLEL_UNIQUE").ok().as_deref() == Some("1");
        let mut set = tokio::task::JoinSet::new();
        for index in 0..parallel {
            let control = control.clone();
            let target = target.clone();
            let client_id = client_id.clone();
            let workspace = workspace.clone();
            let identity = identity.clone();
            set.spawn(async move {
                // unique：每个任务用独立 identity + 工作目录，逼出「各自冷启动 CLI」的场景。
                let (workspace, identity) = if unique {
                    let workspace = format!("{workspace}/w{index}");
                    let _ = std::fs::create_dir_all(&workspace);
                    (workspace, format!("mode-bot:teammate:probe{index}"))
                } else {
                    (workspace, identity)
                };
                let target = target.with_identity(identity.clone());
                let started = std::time::Instant::now();
                let mut payload = serde_json::json!({ "workspaceId": workspace });
                if !identity.trim().is_empty() {
                    payload["workspaceIdentity"] = serde_json::Value::String(identity);
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as i64)
                    .unwrap_or(0);
                let envelope = ConversationCommandEnvelope {
                    ttft: None,
                    command_id: format!("spike-par-{index}-{now}"),
                    client_id,
                    session_id: CommandSessionId(None),
                    base_revision: None,
                    base_log_epoch: None,
                    command_type: CommandType::CreateSession,
                    payload,
                    issued_at: serde_json::Number::from(now),
                };
                let result = control.send_conversation_command_v4(&target, &envelope).await;
                let elapsed = started.elapsed().as_millis();
                match result {
                    Ok(ack) => (
                        index,
                        elapsed,
                        ack.status.as_str().to_string(),
                        ack.result
                            .and_then(|value| {
                                value
                                    .get("sessionId")
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_string)
                            })
                            .unwrap_or_default(),
                    ),
                    Err(error) => (index, elapsed, format!("error: {error}"), String::new()),
                }
            });
        }
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((index, elapsed, status, session)) => {
                    println!("[parallel {index}] {elapsed}ms status={status} session={session}")
                }
                Err(error) => println!("[parallel] join error: {error}"),
            }
        }
        return Ok(());
    }

    // 2.5) 可选：上传附件（v4 attachment 事务），拿到 ref
    let mut attachment_refs = Vec::new();
    if let Some(path) = &attachment_path {
        use agent_protocol::v4_queries::{
            AttachmentBeginParams, AttachmentBeginResult, AttachmentChunkParams,
            AttachmentTerminalParams,
        };
        use base64::Engine as _;
        use sha2::Digest as _;
        let bytes = std::fs::read(path).map_err(|error| format!("读附件失败：{error}"))?;
        let digest = sha2::Sha256::digest(&bytes);
        let checksum = format!(
            "sha256:{}",
            digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
        );
        let file_name = PathBuf::from(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".to_string());
        let upload_id = format!("upload-{}", millis());
        let chunk_bytes = 384 * 1024;
        let total_chunks = bytes.len().div_ceil(chunk_bytes) as u64;
        let begin = control
            .attachment_begin_v4(
                &target,
                AttachmentBeginParams {
                    connection_id: "rust-inprocess".to_string(),
                    upload_id: upload_id.clone(),
                    session_id: session_id.clone(),
                    file_name: file_name.clone(),
                    mime: "text/markdown".to_string(),
                    total_bytes: bytes.len() as u64,
                    total_chunks,
                    checksum,
                },
                agent_process::RuntimePolicy::StartIfNeeded,
            )
            .await
            .map_err(|error| format!("attachment begin 失败：{error}"))?;
        let reference = match begin {
            AttachmentBeginResult::Committed(committed) => committed.attachment_ref,
            AttachmentBeginResult::Staging(staging) => {
                let mut next = staging.next_chunk_index;
                while next < total_chunks {
                    let start = next as usize * chunk_bytes;
                    let end = (start + chunk_bytes).min(bytes.len());
                    let chunk = control
                        .attachment_chunk_v4(
                            &target,
                            AttachmentChunkParams {
                                connection_id: "rust-inprocess".to_string(),
                                upload_id: upload_id.clone(),
                                session_id: session_id.clone(),
                                chunk_index: next,
                                data_base64: base64::engine::general_purpose::STANDARD
                                    .encode(&bytes[start..end]),
                            },
                            agent_process::RuntimePolicy::StartIfNeeded,
                        )
                        .await
                        .map_err(|error| format!("attachment chunk 失败：{error}"))?;
                    println!("chunk {next} -> {}", chunk.next_chunk_index);
                    next += 1;
                }
                let committed = control
                    .attachment_commit_v4(
                        &target,
                        AttachmentTerminalParams {
                            connection_id: "rust-inprocess".to_string(),
                            upload_id: upload_id.clone(),
                            session_id: session_id.clone(),
                        },
                        agent_process::RuntimePolicy::StartIfNeeded,
                    )
                    .await
                    .map_err(|error| format!("attachment commit 失败：{error}"))?;
                committed.attachment_ref
            }
        };
        println!("attachment ref={reference}");
        attachment_refs.push(serde_json::json!({
            "ref": reference,
            "fileName": file_name,
            "mime": "text/markdown",
            "bytes": bytes.len(),
        }));
    }

    // 3) sendText
    let send = ConversationCommandEnvelope {
        ttft: None,
        command_id: format!("spike-send-{}", millis()),
        client_id,
        session_id: CommandSessionId(Some(session_id.clone())),
        base_revision: None,
        base_log_epoch: None,
        command_type: CommandType::SendText,
        payload: {
            let mut payload = serde_json::json!({
                "text": prompt,
                "modelSelection": {
                    "providerId": "deepseek",
                    "modelId": "deepseek-flash",
                    "options": { "reasoningLevel": "max" }
                }
            });
            if !attachment_refs.is_empty() {
                payload["attachments"] = serde_json::Value::Array(attachment_refs);
            }
            payload
        },
        issued_at: serde_json::Number::from(millis()),
    };
    let ack = control
        .send_conversation_command_v4(&target, &send)
        .await
        .map_err(|error| format!("sendText 失败：{error}"))?;
    println!("send ack status={:?} message={:?}", ack.status, ack.message);

    // 4) 打印帧（截断），直到超时
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let mut frames = 0usize;
    while tokio::time::Instant::now() < deadline {
        let Ok(Some(frame)) = tokio::time::timeout(Duration::from_secs(20), subscription.recv()).await
        else {
            println!("[frame timeout]");
            break;
        };
        frames += 1;
        match frame {
            ConversationWireFrame::Complete(complete) => {
                let value = serde_json::to_value(&complete.frame).unwrap_or_default();
                let payload_kind = value
                    .get("payload")
                    .and_then(|payload| payload.get("kind"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("?");
                let text = serde_json::to_string(&value).unwrap_or_default();
                let head: String = text.chars().take(600).collect();
                println!(
                    "[frame {frames}] complete payload={payload_kind} bytes={} head={head}",
                    text.len()
                );
                if text.contains("\"text\":\"收到\"") || text.contains("收到") {
                    println!("[hit 收到]");
                }
            }
            ConversationWireFrame::Fragment(fragment) => {
                println!(
                    "[frame {frames}] fragment {}/{} bytes={}",
                    fragment.fragment_index, fragment.fragment_count, fragment.logical_bytes
                );
            }
        }
        if frames >= 40 {
            break;
        }
    }
    println!("done frames={frames}");
    Ok(())
}
