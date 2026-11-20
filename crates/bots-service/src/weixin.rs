//! 微信 iLink 传输核心（`weixinProvider.ts` 的请求/分类部分）。
//!
//! 本切片只做**可测试的传输与错误分类**：鉴权头、`base_info` 信封、`/getupdates`、
//! `/sendmessage`（文本）请求构造，以及 401/403 → `session_expired`、429/5xx →
//! `retryable`、`errmsg=prepare failed` → `context_expired` 的恢复分类。
//!
//! 尚未迁移：入站消息解析（媒体/附件/上下文 token）、markdown→纯文本、typing、
//! `context_token` 续期探测——随 channel runtime 一起补（这些依赖 `BotInboundMessage`
//! 等共享 schema 的 Rust 模型）。
//!
//! 端点（与 TS 一致）：`{base}/ilink/bot` + `/getupdates` | `/sendmessage` | `/getconfig`。

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::http::{HttpJsonPort, OutboundHttpRequest, DEFAULT_PROVIDER_TIMEOUT};

pub const DEFAULT_WEIXIN_ILINK_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
const WEIXIN_BOT_API_PREFIX: &str = "/ilink/bot";
const WEIXIN_CHANNEL_VERSION: &str = "2.0.0";
const WEIXIN_MESSAGE_TYPE_BOT: i64 = 2;
const WEIXIN_MESSAGE_STATE_FINISH: i64 = 2;
const WEIXIN_GET_UPDATES_TIMEOUT: Duration = Duration::from_secs(90);

/// 出站恢复分类（`BotSendFailureCode` 的镜像；None = 未知失败）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeixinFailureCode {
    SessionExpired,
    Retryable,
    ContextExpired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeixinRequestError {
    pub code: Option<WeixinFailureCode>,
    pub message: String,
}

impl WeixinRequestError {
    fn new(code: Option<WeixinFailureCode>, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// 请求失败 → 恢复分类（纯函数，与 TS `classifyWeixinRequestFailure` 同义）。
pub fn classify_request_failure(http_status: Option<u16>, errmsg: Option<&str>) -> Option<WeixinFailureCode> {
    if let Some(status) = http_status {
        if status == 401 || status == 403 {
            return Some(WeixinFailureCode::SessionExpired);
        }
        if status == 429 || status >= 500 {
            return Some(WeixinFailureCode::Retryable);
        }
        return None;
    }
    if errmsg == Some("prepare failed") {
        return Some(WeixinFailureCode::ContextExpired);
    }
    None
}

/// `X-WECHAT-UIN`：随机 u32 的十进制字符串再 base64（与 TS `buildRandomWechatUin` 同形）。
fn build_random_wechat_uin() -> String {
    let random = Uuid::new_v4().as_u128() as u32;
    base64_encode(random.to_string().as_bytes())
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn build_headers(token: &str) -> Vec<(String, String)> {
    vec![
        ("content-type".into(), "application/json".into()),
        ("AuthorizationType".into(), "ilink_bot_token".into()),
        ("Authorization".into(), format!("Bearer {token}")),
        ("X-WECHAT-UIN".into(), build_random_wechat_uin()),
    ]
}

fn append_base_info(body: Value) -> Value {
    let mut object = match body {
        Value::Object(map) => map,
        other => {
            let mut map = serde_json::Map::new();
            map.insert("body".into(), other);
            map
        }
    };
    object.insert(
        "base_info".into(),
        json!({ "channel_version": WEIXIN_CHANNEL_VERSION }),
    );
    Value::Object(object)
}

pub struct WeixinClient {
    base_url: String,
    http: Arc<dyn HttpJsonPort>,
}

impl WeixinClient {
    pub fn new(http: Arc<dyn HttpJsonPort>) -> Self {
        Self {
            base_url: DEFAULT_WEIXIN_ILINK_BASE_URL.to_string(),
            http,
        }
    }

    pub fn with_base_url(http: Arc<dyn HttpJsonPort>, base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
        }
    }

    async fn request_json(
        &self,
        token: &str,
        path: &str,
        body: Value,
        timeout: Duration,
    ) -> Result<Value, WeixinRequestError> {
        let token = token.trim();
        if token.is_empty() {
            return Err(WeixinRequestError::new(
                Some(WeixinFailureCode::SessionExpired),
                "Weixin iLink bot token is missing. Scan the Weixin login QR code first.",
            ));
        }
        let request = OutboundHttpRequest {
            url: format!("{}{}{}", self.base_url, WEIXIN_BOT_API_PREFIX, path),
            headers: build_headers(token),
            body: append_base_info(body).to_string(),
            timeout,
        };
        let response = self
            .http
            .post_json(&request)
            .await
            .map_err(|error| WeixinRequestError::new(Some(WeixinFailureCode::Retryable), error))?;
        if !(200..300).contains(&response.status) {
            let message = format!("Weixin iLink {path} failed: HTTP {}", response.status);
            let code = classify_request_failure(Some(response.status), None);
            return Err(WeixinRequestError::new(code, message));
        }
        let data = response.payload.as_object();
        let ret = data
            .and_then(|map| map.get("ret"))
            .and_then(Value::as_i64);
        let errcode = data
            .and_then(|map| map.get("errcode"))
            .and_then(Value::as_i64);
        if ret.is_some_and(|value| value != 0) || errcode.is_some_and(|value| value != 0) {
            let errmsg = data
                .and_then(|map| map.get("errmsg"))
                .and_then(Value::as_str)
                .or_else(|| {
                    data.and_then(|map| map.get("message"))
                        .and_then(Value::as_str)
                })
                .unwrap_or_default()
                .to_string();
            let summary = if errmsg.is_empty() {
                format!("ret={} errcode={}", ret.unwrap_or(0), errcode.unwrap_or(0))
            } else {
                errmsg.clone()
            };
            let code = classify_request_failure(None, Some(&errmsg));
            return Err(WeixinRequestError::new(
                code,
                format!("Weixin iLink {path} failed: {summary}"),
            ));
        }
        Ok(response.payload)
    }

    /// `/getupdates`：长轮询（90s）。返回原始 payload；消息解析随 runtime 迁移。
    pub async fn get_updates(
        &self,
        token: &str,
        buf: &str,
        timeout: Option<Duration>,
    ) -> Result<Value, WeixinRequestError> {
        self.request_json(
            token,
            "/getupdates",
            json!({ "get_updates_buf": buf }),
            timeout.unwrap_or(WEIXIN_GET_UPDATES_TIMEOUT),
        )
        .await
    }

    /// `/sendmessage`：纯文本（type 1 `text_item`）。文本的 markdown 降级在 runtime 切片补。
    pub async fn send_text(
        &self,
        token: &str,
        from_user_id: &str,
        to_user_id: &str,
        text: &str,
        context_token: Option<&str>,
    ) -> Result<Value, WeixinRequestError> {
        let mut msg = serde_json::Map::new();
        msg.insert("from_user_id".into(), json!(from_user_id));
        msg.insert("to_user_id".into(), json!(to_user_id));
        msg.insert("client_id".into(), json!(format!("mode-weixin-{}", Uuid::new_v4())));
        msg.insert("message_type".into(), json!(WEIXIN_MESSAGE_TYPE_BOT));
        msg.insert("message_state".into(), json!(WEIXIN_MESSAGE_STATE_FINISH));
        if let Some(context_token) = context_token.filter(|value| !value.is_empty()) {
            msg.insert("context_token".into(), json!(context_token));
        }
        msg.insert(
            "item_list".into(),
            json!([{ "type": 1, "text_item": { "text": text } }]),
        );
        self.request_json(
            token,
            "/sendmessage",
            json!({ "msg": Value::Object(msg) }),
            DEFAULT_PROVIDER_TIMEOUT,
        )
        .await
    }

    /// `/getconfig`：typing 票/上下文续期探测（best-effort 由调用方决定）。
    pub async fn get_config(&self, token: &str) -> Result<Value, WeixinRequestError> {
        self.request_json(token, "/getconfig", json!({}), DEFAULT_PROVIDER_TIMEOUT)
            .await
    }

    async fn fetch_typing_ticket(
        &self,
        token: &str,
        user_id: &str,
    ) -> Option<String> {
        let payload = self
            .request_json(
                token,
                "/getconfig",
                json!({ "ilink_user_id": user_id }),
                DEFAULT_PROVIDER_TIMEOUT,
            )
            .await
            .ok()?;
        ["typing_ticket", "typingTicket"]
            .iter()
            .find_map(|key| payload.get(*key).and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    /// 开启（status=1）/取消（status=2）原生「正在输入」：票复用失败时强刷重试一次。
    pub async fn send_typing(
        &self,
        token: &str,
        user_id: &str,
        status: u8,
    ) -> Result<(), WeixinRequestError> {
        let ticket = self.fetch_typing_ticket(token, user_id).await.ok_or_else(|| {
            WeixinRequestError::new(
                Some(WeixinFailureCode::ContextExpired),
                format!("Weixin typing_ticket unavailable user={user_id}"),
            )
        })?;
        let send_once = |ticket: String, user_id: String| async move {
            self.request_json(
                token,
                "/sendtyping",
                json!({
                    "ilink_user_id": user_id,
                    "typing_ticket": ticket,
                    "status": status,
                }),
                DEFAULT_PROVIDER_TIMEOUT,
            )
            .await
        };
        match send_once(ticket, user_id.to_string()).await {
            Ok(_) => Ok(()),
            Err(first_error) => {
                // 票过期：强刷一次再重试；仍失败则把首个错误交给上层。
                let Some(fresh) = self.fetch_typing_ticket(token, user_id).await else {
                    return Err(first_error);
                };
                send_once(fresh, user_id.to_string()).await.map(|_| ())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpJsonResponse;
    use std::sync::Mutex;

    struct FakeHttp {
        responses: Mutex<Vec<Result<HttpJsonResponse, String>>>,
        requests: Mutex<Vec<OutboundHttpRequest>>,
    }

    impl FakeHttp {
        fn new(responses: Vec<Result<HttpJsonResponse, String>>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(responses),
                requests: Mutex::new(Vec::new()),
            })
        }
    }

    impl HttpJsonPort for FakeHttp {
        fn post_json<'a>(
            &'a self,
            request: &'a OutboundHttpRequest,
        ) -> crate::http::HttpFuture<'a, Result<HttpJsonResponse, String>> {
            self.requests.lock().unwrap().push(request.clone());
            let response = self
                .responses
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| Err("no scripted response".to_string()));
            Box::pin(async move { response })
        }
    }

    fn ok(payload: serde_json::Value) -> Result<HttpJsonResponse, String> {
        Ok(HttpJsonResponse {
            status: 200,
            payload,
        })
    }

    #[test]
    fn failure_classification_matches_ts() {
        assert_eq!(
            classify_request_failure(Some(401), None),
            Some(WeixinFailureCode::SessionExpired)
        );
        assert_eq!(
            classify_request_failure(Some(403), None),
            Some(WeixinFailureCode::SessionExpired)
        );
        assert_eq!(
            classify_request_failure(Some(429), None),
            Some(WeixinFailureCode::Retryable)
        );
        assert_eq!(
            classify_request_failure(Some(503), None),
            Some(WeixinFailureCode::Retryable)
        );
        assert_eq!(
            classify_request_failure(None, Some("prepare failed")),
            Some(WeixinFailureCode::ContextExpired)
        );
        assert_eq!(classify_request_failure(Some(400), None), None);
    }

    #[tokio::test]
    async fn missing_token_is_session_expired_without_http() {
        let http = FakeHttp::new(vec![]);
        let client = WeixinClient::with_base_url(http, "https://example.test");
        let error = client.get_updates("  ", "", None).await.unwrap_err();
        assert_eq!(error.code, Some(WeixinFailureCode::SessionExpired));
    }

    #[tokio::test]
    async fn get_updates_builds_request_envelope() {
        let http = FakeHttp::new(vec![ok(json!({ "ret": 0, "data": { "messages": [] } }))]);
        let client = WeixinClient::with_base_url(Arc::clone(&http) as Arc<dyn HttpJsonPort>, "https://example.test/");
        client.get_updates("token-1", "cursor-1", None).await.unwrap();
        let requests = http.requests.lock().unwrap();
        let request = &requests[0];
        assert_eq!(request.url, "https://example.test/ilink/bot/getupdates");
        assert!(request
            .headers
            .iter()
            .any(|(key, value)| key == "Authorization" && value == "Bearer token-1"));
        assert!(request
            .headers
            .iter()
            .any(|(key, _)| key == "AuthorizationType"));
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["get_updates_buf"], "cursor-1");
        assert_eq!(body["base_info"]["channel_version"], "2.0.0");
    }

    #[tokio::test]
    async fn send_text_builds_sendmessage_payload() {
        let http = FakeHttp::new(vec![ok(json!({ "ret": 0 }))]);
        let client = WeixinClient::with_base_url(Arc::clone(&http) as Arc<dyn HttpJsonPort>, "https://example.test");
        client
            .send_text("token-1", "self", "user-1", "你好", Some("ctx-1"))
            .await
            .unwrap();
        let requests = http.requests.lock().unwrap();
        let request = &requests[0];
        assert_eq!(request.url, "https://example.test/ilink/bot/sendmessage");
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["msg"]["from_user_id"], "self");
        assert_eq!(body["msg"]["to_user_id"], "user-1");
        assert_eq!(body["msg"]["message_type"], 2);
        assert_eq!(body["msg"]["message_state"], 2);
        assert_eq!(body["msg"]["context_token"], "ctx-1");
        assert_eq!(body["msg"]["item_list"][0]["type"], 1);
        assert_eq!(body["msg"]["item_list"][0]["text_item"]["text"], "你好");
    }

    #[tokio::test]
    async fn typing_fetches_ticket_then_sends() {
        let http = FakeHttp::new(vec![
            ok(json!({ "ret": 0 })),
            ok(json!({ "typing_ticket": "ticket-1" })),
        ]);
        let client = WeixinClient::with_base_url(Arc::clone(&http) as Arc<dyn HttpJsonPort>, "https://example.test");
        client.send_typing("token", "user-1", 1).await.unwrap();
        let requests = http.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].url.ends_with("/getconfig"));
        let config_body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
        assert_eq!(config_body["ilink_user_id"], "user-1");
        assert!(requests[1].url.ends_with("/sendtyping"));
        let typing_body: serde_json::Value = serde_json::from_str(&requests[1].body).unwrap();
        assert_eq!(typing_body["typing_ticket"], "ticket-1");
        assert_eq!(typing_body["status"], 1);
    }

    #[tokio::test]
    async fn typing_refreshes_ticket_once_after_failure() {
        let http = FakeHttp::new(vec![
            ok(json!({ "ret": 0 })),
            ok(json!({ "typing_ticket": "ticket-2" })),
            Ok(HttpJsonResponse {
                status: 400,
                payload: json!({ "ret": 1, "errmsg": "prepare failed" }),
            }),
            ok(json!({ "typing_ticket": "ticket-1" })),
        ]);
        let client = WeixinClient::with_base_url(Arc::clone(&http) as Arc<dyn HttpJsonPort>, "https://example.test");
        client.send_typing("token", "user-1", 2).await.unwrap();
        let requests = http.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        let final_body: serde_json::Value = serde_json::from_str(&requests[3].body).unwrap();
        assert_eq!(final_body["typing_ticket"], "ticket-2");
        assert_eq!(final_body["status"], 2);
    }

    #[tokio::test]
    async fn errmsg_prepare_failed_is_context_expired() {
        let http = FakeHttp::new(vec![ok(json!({ "ret": 1, "errmsg": "prepare failed" }))]);
        let client = WeixinClient::with_base_url(http, "https://example.test");
        let error = client
            .send_text("token", "self", "user", "hi", None)
            .await
            .unwrap_err();
        assert_eq!(error.code, Some(WeixinFailureCode::ContextExpired));
        assert!(error.message.contains("prepare failed"), "{error:?}");
    }
}
