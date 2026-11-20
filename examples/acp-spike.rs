//! ACP 本地 Agent 探针：spawn `pi-acp`，走 initialize → session/new → session/prompt，
//! 打印 ACP 的 session/update 通知与权限请求，验证本地 Agent 路线的真实协议形状。
//!
//! 运行：`cargo run --example acp-spike -- "只回复两个字：收到" [cwd]`

use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};

#[tokio::main]
async fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let prompt = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "只回复两个字：收到".to_string());
    let cwd = args.get(2).cloned().unwrap_or_else(|| {
        std::env::temp_dir()
            .join("mode-acp-spike")
            .to_string_lossy()
            .into_owned()
    });
    let engine = args.get(3).cloned().unwrap_or_else(|| "pi-acp".to_string());
    let resume = args.get(4).cloned().filter(|value| !value.trim().is_empty());
    let _ = std::fs::create_dir_all(&cwd);

    // Windows 上这些命令是 .cmd/.exe：经 cmd /C 启动。
    let mut child = tokio::process::Command::new("cmd")
        .arg("/C")
        .arg(&engine)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("spawn {engine} 失败：{error}"))?;
    let mut stdin = BufWriter::new(child.stdin.take().ok_or("没有 stdin")?);
    let mut lines = BufReader::new(child.stdout.take().ok_or("没有 stdout")?).lines();

    let mut next_id = 1u64;
    let mut prompt_sent = false;
    // session/new 或 session/load 的请求 id：响应到了再发 prompt。
    let mut session_request_id: Option<u64> = None;

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": next_id,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": { "readTextFile": false, "writeTextFile": false },
                    "terminal": false
                }
            }
        }),
    )
    .await?;
    let init_id = next_id;
    next_id += 1;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while tokio::time::Instant::now() < deadline {
        let Ok(Ok(Some(line))) =
            tokio::time::timeout(Duration::from_secs(20), lines.next_line()).await
        else {
            println!("[timeout]");
            break;
        };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            println!("[non-json] {}", truncate(&line, 200));
            continue;
        };
        let method = value.get("method").and_then(Value::as_str);
        let id = value.get("id").and_then(Value::as_u64);
        match (method, id) {
            // Agent → Client 请求
            (Some(method), Some(id)) => {
                println!("[request id={id}] {method} {}", truncate(&value.to_string(), 300));
                let result = match method {
                    "session/request_permission" => {
                        // 探针：优先选 allow 类选项，否则取消。
                        let options = value
                            .get("params")
                            .and_then(|params| params.get("options"))
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let allow = options.iter().find(|option| {
                            let kind = option.get("kind").and_then(Value::as_str).unwrap_or("");
                            let name = option
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_ascii_lowercase();
                            kind.contains("allow") || name.contains("allow") || name.contains("允许")
                        });
                        match allow {
                            Some(option) => json!({
                                "outcome": {
                                    "outcome": "selected",
                                    "optionId": option.get("optionId").cloned().unwrap_or(Value::Null)
                                }
                            }),
                            None => json!({ "outcome": { "outcome": "cancelled" } }),
                        }
                    }
                    _ => json!({ "error": "unsupported by spike" }),
                };
                send(
                    &mut stdin,
                    json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                )
                .await?;
            }
            // 响应 / 通知
            (None, Some(id)) => {
                println!(
                    "[response id={id}] {}",
                    truncate(&value.to_string(), 400)
                );
                if id == init_id {
                    let (method, params) = match resume.as_deref() {
                        Some(session) => (
                            "session/load",
                            json!({ "sessionId": session, "cwd": cwd, "mcpServers": [] }),
                        ),
                        None => ("session/new", json!({ "cwd": cwd, "mcpServers": [] })),
                    };
                    send(
                        &mut stdin,
                        json!({
                            "jsonrpc": "2.0",
                            "id": next_id,
                            "method": method,
                            "params": params
                        }),
                    )
                    .await?;
                    session_request_id = Some(next_id);
                    next_id += 1;
                } else if session_request_id == Some(id) {
                    if let Some(error) = value.get("error") {
                        println!("[session-error] {}", truncate(&error.to_string(), 300));
                        break;
                    }
                    // session/load 的响应不带 sessionId（用请求里的 id）；session/new 带。
                    let found = resume.clone().or_else(|| {
                        value
                            .get("result")
                            .and_then(|result| result.get("sessionId"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    });
                    println!("[session] {found:?}");
                    let Some(found) = found else { continue };
                    if !prompt_sent {
                        prompt_sent = true;
                        send(
                            &mut stdin,
                            json!({
                                "jsonrpc": "2.0",
                                "id": next_id,
                                "method": "session/prompt",
                                "params": {
                                    "sessionId": found,
                                    "prompt": [{ "type": "text", "text": prompt }]
                                }
                            }),
                        )
                        .await?;
                        next_id += 1;
                    }
                } else if id != init_id
                    && value.get("result").and_then(|r| r.get("stopReason")).is_some()
                {
                    println!("[done] prompt stopReason={:?}", value["result"]["stopReason"]);
                    break;
                }
            }
            (Some(method), None) => {
                let update = value
                    .get("params")
                    .and_then(|params| params.get("update"));
                let kind = update
                    .and_then(|update| update.get("sessionUpdate"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if kind == "agent_message_chunk"
                    && let Some(text) = update
                        .and_then(|update| update.get("content"))
                        .and_then(|content| content.get("text"))
                        .and_then(Value::as_str)
                {
                    println!("[chunk] {text}");
                } else {
                    println!(
                        "[notify] {method} {kind} {}",
                        truncate(&value.to_string(), 300)
                    );
                }
            }
            _ => {}
        }
    }

    let _ = child.start_kill();
    Ok(())
}

async fn send(stdin: &mut BufWriter<tokio::process::ChildStdin>, value: Value) -> Result<(), String> {
    let line = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|error| format!("写 stdin 失败：{error}"))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|error| format!("写 stdin 失败：{error}"))?;
    stdin
        .flush()
        .await
        .map_err(|error| format!("flush 失败：{error}"))?;
    Ok(())
}

fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}
