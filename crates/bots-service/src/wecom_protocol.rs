//! 企微 aibot WebSocket 帧协议（纯逻辑，`@wecom/aibot-node-sdk` dist 的可读源码移植）。
//!
//! 协议要点（SDK `index.cjs.js`）：
//! - 地址 `wss://openws.work.weixin.qq.com`；连上后立即发认证帧
//!   `{cmd:"aibot_subscribe", headers:{req_id}, body:{bot_id, secret}}`；
//! - 心跳 `{cmd:"ping", headers:{req_id}}`（30s，连续 2 次无 pong 视为死连接）；
//! - 服务端推送：`aibot_msg_callback` / `aibot_event_callback`（`disconnected_event`=
//!   被新连接踢下线）；
//! - 所有「响应/回执」帧**没有 cmd**，靠 `headers.req_id` 前缀区分（subscribe/ping/其它
//!   =回复回执），`errcode: 0` 为成功；
//! - 回复按 req_id 串行：一帧发出后等回执（5s 超时）再发下一帧；回复 cmd 有
//!   `aibot_respond_msg` / `aibot_respond_welcome_msg` / `aibot_respond_update_msg` /
//!   `aibot_send_msg`（主动消息）/ 媒体三段上传。
//!
//! 本模块只做**帧编解码与队列状态机**，不持有连接；WS 传输在宿主侧实现。

use std::collections::HashMap;

use serde_json::{json, Value};
use uuid::Uuid;

pub const DEFAULT_WS_URL: &str = "wss://openws.work.weixin.qq.com";

pub const CMD_SUBSCRIBE: &str = "aibot_subscribe";
pub const CMD_HEARTBEAT: &str = "ping";
pub const CMD_RESPONSE: &str = "aibot_respond_msg";
pub const CMD_RESPONSE_WELCOME: &str = "aibot_respond_welcome_msg";
pub const CMD_RESPONSE_UPDATE: &str = "aibot_respond_update_msg";
pub const CMD_SEND_MSG: &str = "aibot_send_msg";
pub const CMD_UPLOAD_MEDIA_INIT: &str = "aibot_upload_media_init";
pub const CMD_UPLOAD_MEDIA_CHUNK: &str = "aibot_upload_media_chunk";
pub const CMD_UPLOAD_MEDIA_FINISH: &str = "aibot_upload_media_finish";
pub const CMD_CALLBACK: &str = "aibot_msg_callback";
pub const CMD_EVENT_CALLBACK: &str = "aibot_event_callback";

/// `{prefix}_{timestamp}_{random8}`（与 SDK `generateReqId` 同形）。
pub fn generate_req_id(prefix: &str) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let random = Uuid::new_v4().simple().to_string();
    format!("{prefix}_{millis}_{}", &random[..8])
}

pub fn subscribe_frame(bot_id: &str, secret: &str, req_id: &str) -> Value {
    json!({
        "cmd": CMD_SUBSCRIBE,
        "headers": { "req_id": req_id },
        "body": { "bot_id": bot_id, "secret": secret },
    })
}

pub fn heartbeat_frame(req_id: &str) -> Value {
    json!({ "cmd": CMD_HEARTBEAT, "headers": { "req_id": req_id } })
}

pub fn reply_frame(cmd: &str, req_id: &str, body: Value) -> Value {
    json!({ "cmd": cmd, "headers": { "req_id": req_id }, "body": body })
}

/// 服务端帧分类结果。
#[derive(Debug, Clone, PartialEq)]
pub enum ServerFrame {
    /// 消息推送（body 原样交给上层解析）。
    Message { req_id: String, body: Value },
    /// 事件推送；`disconnected_event` 表示被新连接踢下线。
    Event { req_id: String, body: Value, disconnected: bool },
    /// 认证成功（req_id 以 aibot_subscribe 开头且 errcode=0）。
    Authenticated,
    /// 认证失败。
    AuthFailed { errcode: i64, errmsg: String },
    /// 心跳回执。
    HeartbeatAck,
    /// 心跳回执错误（仍算收到 pong，SDK 只记日志）。
    HeartbeatAckError { errcode: i64, errmsg: String },
    /// 回复回执（req_id 命中 pending）。
    ReplyAck {
        req_id: String,
        errcode: i64,
        errmsg: String,
        body: Value,
    },
    /// 未识别的帧（只记诊断，不交上层）。
    Unknown { cmd: String, req_id: String },
}

pub fn classify_server_frame(
    frame: &Value,
    is_pending_reply: impl Fn(&str) -> bool,
) -> ServerFrame {
    let cmd = frame.get("cmd").and_then(Value::as_str).unwrap_or("");
    let req_id = frame
        .get("headers")
        .and_then(|headers| headers.get("req_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let body = frame.get("body").cloned().unwrap_or(Value::Null);
    if cmd == CMD_CALLBACK {
        return ServerFrame::Message { req_id, body };
    }
    if cmd == CMD_EVENT_CALLBACK {
        let disconnected = body
            .get("event")
            .and_then(|event| event.get("eventtype"))
            .and_then(Value::as_str)
            == Some("disconnected_event");
        return ServerFrame::Event {
            req_id,
            body,
            disconnected,
        };
    }
    let errcode = frame.get("errcode").and_then(Value::as_i64).unwrap_or(0);
    let errmsg = frame
        .get("errmsg")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if req_id.starts_with(CMD_SUBSCRIBE) {
        return if errcode == 0 {
            ServerFrame::Authenticated
        } else {
            ServerFrame::AuthFailed { errcode, errmsg }
        };
    }
    if req_id.starts_with(CMD_HEARTBEAT) {
        return if errcode == 0 {
            ServerFrame::HeartbeatAck
        } else {
            ServerFrame::HeartbeatAckError { errcode, errmsg }
        };
    }
    if is_pending_reply(&req_id) {
        return ServerFrame::ReplyAck {
            req_id,
            errcode,
            errmsg,
            body,
        };
    }
    ServerFrame::Unknown { cmd: cmd.to_string(), req_id }
}

/// 回复回执超时（SDK `replyAckTimeout`）。
pub const REPLY_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// 单 req_id 回复队列上限（SDK `maxReplyQueueSize`）。
pub const MAX_REPLY_QUEUE_SIZE: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedReply {
    pub cmd: String,
    pub body: Value,
}

#[derive(Debug, Default)]
pub struct ReplyQueues {
    queues: HashMap<String, Vec<QueuedReply>>,
    /// 已发出、等待回执的 req_id（同一 req_id 串行）。
    pending: std::collections::HashSet<String>,
}

impl ReplyQueues {
    pub fn new() -> Self {
        Self::default()
    }

    /// 入队并返回「现在是否应立即发送」（队列原本为空）。
    pub fn enqueue(
        &mut self,
        req_id: &str,
        reply: QueuedReply,
    ) -> Result<bool, String> {
        let queue = self.queues.entry(req_id.to_string()).or_default();
        if queue.len() >= MAX_REPLY_QUEUE_SIZE {
            return Err(format!(
                "Reply queue for reqId {req_id} exceeds max size ({MAX_REPLY_QUEUE_SIZE})"
            ));
        }
        let was_empty = queue.is_empty();
        queue.push(reply);
        Ok(was_empty && !self.pending.contains(req_id))
    }

    /// 标记 req_id 已发出、等待回执。
    pub fn begin_send(&mut self, req_id: &str) {
        self.pending.insert(req_id.to_string());
    }

    pub fn is_pending(&self, req_id: &str) -> bool {
        self.pending.contains(req_id)
    }

    /// 回执到达：弹出队首，返回下一条待发回复（若有）。
    pub fn acknowledge(&mut self, req_id: &str) -> Option<QueuedReply> {
        self.pending.remove(req_id);
        let queue = self.queues.get_mut(req_id)?;
        if !queue.is_empty() {
            queue.remove(0);
        }
        if queue.is_empty() {
            self.queues.remove(req_id);
            return None;
        }
        queue.first().cloned()
    }

    /// 清理（断线/停止）：清空全部队列与 pending。
    pub fn clear(&mut self) -> usize {
        let count = self.queues.values().map(Vec::len).sum();
        self.queues.clear();
        self.pending.clear();
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_id_has_prefix_and_timestamp() {
        let id = generate_req_id(CMD_RESPONSE);
        assert!(id.starts_with("aibot_respond_msg_"), "{id}");
        let parts: Vec<&str> = id.split('_').collect();
        // 前缀本身可能含下划线（aibot_respond_msg）：只校验最后两段。
        let timestamp = parts[parts.len() - 2];
        let random = parts[parts.len() - 1];
        assert!(timestamp.parse::<u128>().is_ok(), "{id}");
        assert_eq!(random.len(), 8, "{id}");
    }

    #[test]
    fn builds_subscribe_and_heartbeat_frames() {
        assert_eq!(
            subscribe_frame("bot-1", "secret-1", "aibot_subscribe_1_abcd"),
            json!({
                "cmd": "aibot_subscribe",
                "headers": { "req_id": "aibot_subscribe_1_abcd" },
                "body": { "bot_id": "bot-1", "secret": "secret-1" }
            })
        );
        assert_eq!(
            heartbeat_frame("ping_1_abcd"),
            json!({ "cmd": "ping", "headers": { "req_id": "ping_1_abcd" } })
        );
        assert_eq!(
            reply_frame(CMD_SEND_MSG, "aibot_send_msg_1_abcd", json!({ "x": 1 })),
            json!({
                "cmd": "aibot_send_msg",
                "headers": { "req_id": "aibot_send_msg_1_abcd" },
                "body": { "x": 1 }
            })
        );
    }

    #[test]
    fn classifies_server_frames() {
        let pending = |id: &str| id == "aibot_respond_msg_1_abcd";
        assert_eq!(
            classify_server_frame(
                &json!({ "headers": { "req_id": "aibot_subscribe_1_x" }, "errcode": 0 }),
                pending
            ),
            ServerFrame::Authenticated
        );
        assert!(matches!(
            classify_server_frame(
                &json!({ "headers": { "req_id": "aibot_subscribe_1_x" }, "errcode": 40001, "errmsg": "bad secret" }),
                pending
            ),
            ServerFrame::AuthFailed { errcode: 40001, .. }
        ));
        assert_eq!(
            classify_server_frame(
                &json!({ "headers": { "req_id": "ping_1_x" }, "errcode": 0 }),
                pending
            ),
            ServerFrame::HeartbeatAck
        );
        assert!(matches!(
            classify_server_frame(
                &json!({ "cmd": "aibot_msg_callback", "headers": { "req_id": "r1" }, "body": { "msgid": "m1" } }),
                pending
            ),
            ServerFrame::Message { .. }
        ));
        assert!(matches!(
            classify_server_frame(
                &json!({
                    "cmd": "aibot_event_callback",
                    "headers": { "req_id": "r2" },
                    "body": { "event": { "eventtype": "disconnected_event" } }
                }),
                pending
            ),
            ServerFrame::Event { disconnected: true, .. }
        ));
        assert_eq!(
            classify_server_frame(
                &json!({ "headers": { "req_id": "aibot_respond_msg_1_abcd" }, "errcode": 0 }),
                pending
            ),
            ServerFrame::ReplyAck {
                req_id: "aibot_respond_msg_1_abcd".to_string(),
                errcode: 0,
                errmsg: String::new(),
                body: Value::Null,
            }
        );
        assert!(matches!(
            classify_server_frame(&json!({ "headers": { "req_id": "other" } }), pending),
            ServerFrame::Unknown { .. }
        ));
    }

    #[test]
    fn reply_queue_serializes_per_req_id() {
        let mut queues = ReplyQueues::new();
        assert!(queues
            .enqueue(
                "r1",
                QueuedReply {
                    cmd: CMD_RESPONSE.into(),
                    body: json!({ "n": 1 })
                }
            )
            .unwrap());
        assert!(!queues
            .enqueue(
                "r1",
                QueuedReply {
                    cmd: CMD_RESPONSE.into(),
                    body: json!({ "n": 2 })
                }
            )
            .unwrap());
        queues.begin_send("r1");
        assert!(queues.is_pending("r1"));
        let next = queues.acknowledge("r1").expect("next reply");
        assert_eq!(next.body["n"], 2);
        assert!(!queues.is_pending("r1"));
        // 第二条发完后队列清空。
        queues.begin_send("r1");
        assert!(queues.acknowledge("r1").is_none());
        // 断线清空。
        queues
            .enqueue(
                "r2",
                QueuedReply {
                    cmd: CMD_SEND_MSG.into(),
                    body: json!({}),
                },
            )
            .unwrap();
        assert_eq!(queues.clear(), 1);
        assert!(!queues.is_pending("r2"));
    }

    #[test]
    fn reply_queue_caps_size() {
        let mut queues = ReplyQueues::new();
        for index in 0..MAX_REPLY_QUEUE_SIZE {
            queues
                .enqueue(
                    "r1",
                    QueuedReply {
                        cmd: CMD_RESPONSE.into(),
                        body: json!({ "n": index }),
                    },
                )
                .unwrap();
        }
        assert!(queues
            .enqueue(
                "r1",
                QueuedReply {
                    cmd: CMD_RESPONSE.into(),
                    body: json!({}),
                },
            )
            .is_err());
    }
}
