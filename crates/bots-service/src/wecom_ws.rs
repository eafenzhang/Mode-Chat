//! 企微 aibot WebSocket 传输（步骤 1：连接/认证/心跳/重连/双向帧泵）。
//!
//! 语义取自 `@wecom/aibot-node-sdk` 的 `WsConnectionManager`：
//! - 连上即发 `aibot_subscribe`；收到 req_id 前缀匹配且 `errcode=0` 才算认证成功；
//! - 认证后每 30s 发 `ping`，连续 2 次没有 pong 视为死连接并重连；
//! - 认证失败与断线使用**独立重试计数**（认证失败超过 5 次放弃）；
//! - 被服务端 `disconnected_event` 踢下线时**不重连**（重连会被再次踢）；
//! - 出站帧由调用方经 mpsc 提供；回执超时/回复串行由上层 `ReplyQueues` 负责。
//!
//! 传输只做 JSON 帧泵，不解释业务载荷。TLS 由 `tokio-tungstenite` 的
//! `rustls-tls-webpki-roots` 提供（work.weixin 域证书走系统根）。

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::tungstenite::Message;

use crate::runtime::AbortSignal;
use crate::wecom_protocol::{
    classify_server_frame, generate_req_id, heartbeat_frame, reply_frame, subscribe_frame,
    CMD_HEARTBEAT, CMD_SUBSCRIBE, DEFAULT_WS_URL,
};

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
pub const MAX_MISSED_PONGS: u32 = 2;
pub const MAX_AUTH_FAILURES: u32 = 5;
pub const RECONNECT_BASE_DELAY: Duration = Duration::from_secs(1);
pub const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct WeComWsConfig {
    pub url: String,
    pub bot_id: String,
    pub secret: String,
    pub heartbeat_interval: Duration,
    pub max_missed_pongs: u32,
    pub max_auth_failures: u32,
    pub reconnect_base_delay: Duration,
    pub reconnect_max_delay: Duration,
}

impl WeComWsConfig {
    pub fn new(bot_id: &str, secret: &str) -> Self {
        Self {
            url: DEFAULT_WS_URL.to_string(),
            bot_id: bot_id.to_string(),
            secret: secret.to_string(),
            heartbeat_interval: HEARTBEAT_INTERVAL,
            max_missed_pongs: MAX_MISSED_PONGS,
            max_auth_failures: MAX_AUTH_FAILURES,
            reconnect_base_delay: RECONNECT_BASE_DELAY,
            reconnect_max_delay: RECONNECT_MAX_DELAY,
        }
    }

    pub fn with_url(mut self, url: &str) -> Self {
        self.url = url.to_string();
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum WeComWsEvent {
    /// TCP/WS 建立（尚未认证）。
    Connected,
    Authenticated,
    /// 可重试断线（会按退避重连）。
    Disconnected { reason: String },
    /// 服务端因新连接踢下线（不再重连）。
    ServerDisconnect { reason: String },
    Message { req_id: String, body: Value },
    Event { req_id: String, body: Value },
    /// 回复回执（req_id 命中上层 pending 表时）。
    ReplyAck {
        req_id: String,
        errcode: i64,
        errmsg: String,
        body: Value,
    },
    /// 认证失败（计数；达到上限后停止）。
    AuthFailed { errcode: i64, errmsg: String },
    Error { message: String },
}

pub type EventSink = Arc<dyn Fn(WeComWsEvent) + Send + Sync>;
/// 判定 req_id 是否在等待回执（上层 `ReplyQueues::is_pending`）。
pub type PendingPredicate = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// 运行一条 bot 连接直到取消/认证耗尽/被踢。
///
/// `outbound` 收到的帧按序发给服务端；连接重建期间保留在通道里（不丢）。
pub async fn run_wecom_ws(
    config: WeComWsConfig,
    mut outbound: UnboundedReceiver<Value>,
    on_event: EventSink,
    is_pending: PendingPredicate,
    signal: AbortSignal,
) {
    let mut reconnect_attempts = 0u32;
    let mut auth_failures = 0u32;
    loop {
        if signal.is_aborted() {
            return;
        }
        let connect = tokio_tungstenite::connect_async(&config.url).await;
        let (stream, _response) = match connect {
            Ok(value) => value,
            Err(error) => {
                reconnect_attempts += 1;
                on_event(WeComWsEvent::Error {
                    message: format!("wecom ws connect failed: {error}"),
                });
                let delay = backoff(config.reconnect_base_delay, config.reconnect_max_delay, reconnect_attempts);
                signal.wait_for(delay.as_millis() as u64).await;
                continue;
            }
        };
        on_event(WeComWsEvent::Connected);
        let (mut sink, mut source) = stream.split();
        // 认证帧。
        let auth_req_id = generate_req_id(CMD_SUBSCRIBE);
        let auth = subscribe_frame(&config.bot_id, &config.secret, &auth_req_id);
        if let Err(error) = sink.send(Message::Text(auth.to_string().into())).await {
            on_event(WeComWsEvent::Error {
                message: format!("wecom ws auth send failed: {error}"),
            });
            continue;
        }
        let mut authenticated = false;
        let mut missed_pongs = 0u32;
        let mut heartbeat = tokio::time::interval(config.heartbeat_interval);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // tokio interval 的第一次 tick 是立即返回的：先吃掉，避免认证刚完成就误发 ping。
        heartbeat.tick().await;
        let mut server_disconnect = false;
        loop {
            tokio::select! {
                _ = signal.wait_for_abort() => {
                    let _ = sink.send(Message::Close(None)).await;
                    return;
                }
                outgoing = outbound.recv() => {
                    let Some(frame) = outgoing else {
                        let _ = sink.send(Message::Close(None)).await;
                        return;
                    };
                    if let Err(error) = sink.send(Message::Text(frame.to_string().into())).await {
                        on_event(WeComWsEvent::Error { message: format!("wecom ws send failed: {error}") });
                        break;
                    }
                }
                incoming = source.next() => {
                    match incoming {
                        Some(Ok(Message::Text(text))) => {
                            let frame: Value = match serde_json::from_str(&text) {
                                Ok(frame) => frame,
                                Err(error) => {
                                    on_event(WeComWsEvent::Error { message: format!("wecom ws frame parse failed: {error}") });
                                    continue;
                                }
                            };
                            match classify_server_frame(&frame, |id| is_pending(id)) {
                                crate::wecom_protocol::ServerFrame::Authenticated => {
                                    authenticated = true;
                                    reconnect_attempts = 0;
                                    auth_failures = 0;
                                    on_event(WeComWsEvent::Authenticated);
                                }
                                crate::wecom_protocol::ServerFrame::AuthFailed { errcode, errmsg } => {
                                    auth_failures += 1;
                                    on_event(WeComWsEvent::AuthFailed { errcode, errmsg });
                                    if auth_failures >= config.max_auth_failures {
                                        let _ = sink.send(Message::Close(None)).await;
                                        return;
                                    }
                                    break;
                                }
                                crate::wecom_protocol::ServerFrame::HeartbeatAck
                                | crate::wecom_protocol::ServerFrame::HeartbeatAckError { .. } => {
                                    missed_pongs = 0;
                                }
                                crate::wecom_protocol::ServerFrame::Message { req_id, body } => {
                                    on_event(WeComWsEvent::Message { req_id, body });
                                }
                                crate::wecom_protocol::ServerFrame::Event { req_id, body, disconnected } => {
                                    if disconnected {
                                        server_disconnect = true;
                                        on_event(WeComWsEvent::ServerDisconnect { reason: "New connection established, server disconnected this connection".to_string() });
                                        break;
                                    }
                                    on_event(WeComWsEvent::Event { req_id, body });
                                }
                                crate::wecom_protocol::ServerFrame::ReplyAck { req_id, errcode, errmsg, body } => {
                                    on_event(WeComWsEvent::ReplyAck { req_id, errcode, errmsg, body });
                                }
                                crate::wecom_protocol::ServerFrame::Unknown { .. } => {}
                            }
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            let _ = sink.send(Message::Pong(payload)).await;
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            on_event(WeComWsEvent::Disconnected { reason: "socket closed".to_string() });
                            break;
                        }
                        Some(Ok(_)) => {}
                        Some(Err(error)) => {
                            on_event(WeComWsEvent::Disconnected { reason: error.to_string() });
                            break;
                        }
                    }
                }
                _ = heartbeat.tick() => {
                    if !authenticated { continue; }
                    if missed_pongs >= config.max_missed_pongs {
                        on_event(WeComWsEvent::Disconnected { reason: format!("no heartbeat ack for {missed_pongs} pings") });
                        break;
                    }
                    missed_pongs += 1;
                    let frame = heartbeat_frame(&generate_req_id(CMD_HEARTBEAT));
                    if let Err(error) = sink.send(Message::Text(frame.to_string().into())).await {
                        on_event(WeComWsEvent::Error { message: format!("wecom ws heartbeat failed: {error}") });
                        break;
                    }
                }
            }
        }
        let _ = sink.send(Message::Close(None)).await;
        if server_disconnect {
            return;
        }
        reconnect_attempts += 1;
        let delay = backoff(config.reconnect_base_delay, config.reconnect_max_delay, reconnect_attempts);
        signal.wait_for(delay.as_millis() as u64).await;
    }
}

fn backoff(base: Duration, max: Duration, attempt: u32) -> Duration {
    let factor = 1u32 << attempt.saturating_sub(1).min(5);
    (base * factor).min(max)
}

/// 便捷：发送一帧（业务层构造完 reply_frame 后调用）。
pub fn enqueue_frame(
    outbound: &tokio::sync::mpsc::UnboundedSender<Value>,
    cmd: &str,
    req_id: &str,
    body: Value,
) -> Result<(), String> {
    outbound
        .send(reply_frame(cmd, req_id, body))
        .map_err(|_| "wecom ws outbound channel closed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(Duration::from_secs(1), Duration::from_secs(30), 1), Duration::from_secs(1));
        assert_eq!(backoff(Duration::from_secs(1), Duration::from_secs(30), 3), Duration::from_secs(4));
        assert_eq!(backoff(Duration::from_secs(1), Duration::from_secs(30), 10), Duration::from_secs(30));
    }

    #[tokio::test]
    async fn connects_subscribes_and_pumps_frames() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            // 1) 认证帧。
            let auth = ws.next().await.unwrap().unwrap().into_text().unwrap();
            let auth: Value = serde_json::from_str(&auth).unwrap();
            assert_eq!(auth["cmd"], CMD_SUBSCRIBE);
            assert_eq!(auth["body"]["bot_id"], "bot-1");
            assert_eq!(auth["body"]["secret"], "secret-1");
            let auth_req = auth["headers"]["req_id"].as_str().unwrap().to_string();
            // 2) 认证回执。
            ws.send(Message::Text(
                serde_json::json!({ "headers": { "req_id": auth_req }, "errcode": 0 })
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
            // 3) 推送一条消息。
            ws.send(Message::Text(
                serde_json::json!({
                    "cmd": "aibot_msg_callback",
                    "headers": { "req_id": "cb-1" },
                    "body": { "msgid": "m1", "msgtype": "text", "text": { "content": "hi" } }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
            // 4) 等客户端主动发出的帧（业务回复）。
            let outgoing = ws.next().await.unwrap().unwrap().into_text().unwrap();
            let outgoing: Value = serde_json::from_str(&outgoing).unwrap();
            assert_eq!(outgoing["cmd"], "aibot_send_msg");
            assert_eq!(outgoing["body"]["msgid"], "m1");
        });
        let config = WeComWsConfig::new("bot-1", "secret-1")
            .with_url(&format!("ws://{address}"))
            .with_fast_heartbeat_for_tests();
        let events: Arc<Mutex<Vec<WeComWsEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let events_sink = Arc::clone(&events);
        let (outbound_tx, outbound_rx) = tokio::sync::mpsc::unbounded_channel();
        let signal = AbortSignal::new();
        let runner = tokio::spawn(run_wecom_ws(
            config,
            outbound_rx,
            Arc::new(move |event| events_sink.lock().unwrap().push(event)),
            Arc::new(|_| false),
            signal.clone(),
        ));
        // 等认证完成后再发业务帧。
        tokio::time::sleep(Duration::from_millis(150)).await;
        enqueue_frame(&outbound_tx, "aibot_send_msg", "send-1", serde_json::json!({ "msgid": "m1" }))
            .unwrap();
        server.await.unwrap();
        signal.abort();
        let _ = tokio::time::timeout(Duration::from_secs(2), runner).await;
        let events = events.lock().unwrap();
        assert!(events.contains(&WeComWsEvent::Authenticated), "{events:?}");
        assert!(
            events.iter().any(|event| matches!(
                event,
                WeComWsEvent::Message { req_id, .. } if req_id == "cb-1"
            )),
            "{events:?}"
        );
    }

    impl WeComWsConfig {
        fn with_fast_heartbeat_for_tests(mut self) -> Self {
            // 测试不需要心跳抢戏：interval 设长。
            self.heartbeat_interval = Duration::from_secs(3600);
            self
        }
    }
}
