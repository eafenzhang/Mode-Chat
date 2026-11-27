//! 钉钉 Stream 模式协议（机器人收发消息）。
//!
//! 钉钉官方 Stream 模式的握手/心跳/事件帧是公开协议（dingtalk-stream-sdk）：
//!
//! 1. `POST /v1.0/oauth2/accessToken`（appKey+appSecret）拿 accessToken；
//! 2. `POST /v1.0/gateway/connections/open`（clientId+clientSecret+subscriptions）
//!    拿 `{endpoint, ticket}`；
//! 3. 连 `endpoint`（wss），发 REGISTER 帧（code=ticket），等 `REGISTER:ACK`；
//! 4. 保活：30s 一次 PING（`PING:ACK`）；下行 `EVENT` 帧回 `EVENT-ACK`；
//! 5. 消息事件 topic `/event/message`，回复直接 POST 事件自带的
//!    `sessionWebhook`（无需再签名）。

use serde_json::{Value, json};

pub const GATEWAY_OPEN_URL: &str = "https://api.dingtalk.com/v1.0/gateway/connections/open";
pub const ACCESS_TOKEN_URL: &str = "https://api.dingtalk.com/v1.0/oauth2/accessToken";
/// 消息事件 topic（机器人收单聊/群聊文本）。
pub const TOPIC_MESSAGE: &str = "/event/message";
/// 心跳间隔（秒）：官方 SDK 默认 30s 内必须有一次 PING。
pub const PING_INTERVAL_MS: u64 = 25_000;

/// gateway/connections/open 的请求体（拿 WS endpoint 与注册 ticket）。
pub fn gateway_open_body(client_id: &str, client_secret: &str) -> Value {
    json!({
        "clientId": client_id,
        "clientSecret": client_secret,
        "ua": "modechat-desktop/0.7",
        "localIp": "127.0.0.1",
        "subscriptions": [
            {
                "topic": TOPIC_MESSAGE,
                "type": "EVENT",
                "permissions": ["SUBSCRIPTION"]
            }
        ]
    })
}

/// WS 连上后的 REGISTER 帧。
pub fn register_frame(ticket: &str, client_id: &str, req_id: &str) -> Value {
    json!({
        "code": ticket,
        "frameType": "system",
        "type": "REGISTER",
        "contentType": "application/json",
        "reqId": req_id,
        "appKey": client_id,
        "ua": "modechat-desktop/0.7"
    })
}

/// 心跳帧。
pub fn ping_frame(req_id: &str) -> Value {
    json!({
        "frameType": "system",
        "type": "PING",
        "contentType": "application/json",
        "reqId": req_id
    })
}

/// 对一条下行 EVENT 帧的确认。
pub fn event_ack_frame(req_id: &str) -> Value {
    json!({
        "frameType": "system",
        "type": "EVENT-ACK",
        "contentType": "application/json",
        "reqId": req_id,
        "code": 200,
        "message": "OK"
    })
}

/// 解析下行帧的类型：`REGISTER:ACK` / `PING:ACK` / `EVENT` / 其它。
pub fn frame_type(frame: &Value) -> &str {
    frame
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// 一条机器人消息事件（钉钉 → 统一入站模型）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DingtalkMessageEvent {
    pub message_id: String,
    pub sender_id: String,
    pub sender_nick: Option<String>,
    /// `1` 单聊 / 其它=群聊（conversationType）。
    pub conversation_type: String,
    pub conversation_id: String,
    pub text: String,
    /// 回复直接 POST 这个 webhook（事件自带，无需签名）。
    pub session_webhook: String,
}

/// 从 EVENT 帧解析机器人消息；非消息事件返回 None。
///
/// `data` 是 JSON 字符串（官方 SDK 约定），少数版本也直接内嵌对象——两种都容。
pub fn parse_message_event(frame: &Value) -> Option<DingtalkMessageEvent> {
    let headers_topic = frame
        .get("headers")
        .and_then(|headers| headers.get("topic"))
        .and_then(Value::as_str);
    if headers_topic != Some(TOPIC_MESSAGE) {
        return None;
    }
    let data = match frame.get("data") {
        Some(Value::String(text)) => serde_json::from_str::<Value>(text).ok()?,
        Some(value @ Value::Object(_)) => value.clone(),
        _ => return None,
    };
    let text = data
        .get("text")
        .and_then(|text| text.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if text.is_empty() {
        return None;
    }
    Some(DingtalkMessageEvent {
        message_id: data
            .get("msgId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        sender_id: data
            .get("senderStaffId")
            .or_else(|| data.get("senderUnionId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        sender_nick: data
            .get("senderNick")
            .and_then(Value::as_str)
            .map(str::to_string),
        conversation_type: data
            .get("conversationType")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        conversation_id: data
            .get("conversationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        text,
        session_webhook: data
            .get("sessionWebhook")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// sessionWebhook 回复的请求体（Markdown 与 text 二选一；text 最稳）。
pub fn webhook_reply_body(text: &str) -> Value {
    json!({ "msgtype": "text", "text": { "content": text } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_and_frames_shapes() {
        let body = gateway_open_body("key", "sec");
        assert_eq!(body["clientId"], "key");
        assert_eq!(body["subscriptions"][0]["topic"], TOPIC_MESSAGE);
        let reg = register_frame("ticket", "key", "req-1");
        assert_eq!(reg["code"], "ticket");
        assert_eq!(reg["type"], "REGISTER");
        let ping = ping_frame("req-2");
        assert_eq!(ping["type"], "PING");
        let ack = event_ack_frame("req-3");
        assert_eq!(ack["type"], "EVENT-ACK");
        assert_eq!(ack["code"], 200);
    }

    #[test]
    fn parses_string_and_embedded_data() {
        let data = json!({
            "msgId": "m1",
            "senderStaffId": "u1",
            "senderNick": "娜娜",
            "conversationType": "1",
            "conversationId": "cid-1",
            "text": { "content": " 你好 " },
            "sessionWebhook": "https://oapi.dingtalk.com/robot/send?x=1"
        });
        let frame_string = json!({
            "type": "EVENT",
            "headers": { "topic": "/event/message" },
            "data": data.to_string(),
            "reqId": "r1"
        });
        let event = parse_message_event(&frame_string).expect("字符串 data 要能解析");
        assert_eq!(event.text, "你好");
        assert_eq!(event.sender_nick.as_deref(), Some("娜娜"));
        assert_eq!(event.conversation_type, "1");
        // data 内嵌对象同样支持。
        let frame_embedded = json!({
            "type": "EVENT",
            "headers": { "topic": "/event/message" },
            "data": data
        });
        let event = parse_message_event(&frame_embedded).expect("内嵌 data 要能解析");
        assert_eq!(event.session_webhook, "https://oapi.dingtalk.com/robot/send?x=1");
        // 非 topic 事件不解析。
        let other = json!({"type": "EVENT", "headers": {"topic": "/event/other"}, "data": "{}"});
        assert!(parse_message_event(&other).is_none());
    }

    #[test]
    fn webhook_body_shape() {
        let body = webhook_reply_body("回复");
        assert_eq!(body["msgtype"], "text");
        assert_eq!(body["text"]["content"], "回复");
    }
}
