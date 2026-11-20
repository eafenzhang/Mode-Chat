//! 微信通道运行时（`weixinChannelRuntime.ts` 的轮询核心，切片 2h）。
//!
//! 循环语义（与 TS 一致，关键点）：`getUpdates` 拿到一页后逐条 dedupe → 交给回调处理；
//! **只有本页全部处理成功才提交下一游标**（`get_updates_buf`），回调失败时释放该条的
//! 去重记录并保留游标，下一轮重投时重试；游标持久化由宿主负责。
//!
//! 会话失效（`session_expired`）不是重试类错误：置 error 状态并停止轮询，等用户重新扫码。
//!
//! 本切片用注入的宿主 trait 隔离凭据读取/回调处理/游标持久化/状态投影；HTTP 仍走
//! `WeixinClient`（注入端口）。1s 级轮询/退避用 `AbortSignal::wait_for`。

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::http::HttpJsonPort;
use crate::inbound::{InboundDeliveryDedupe, InboundIdentity};
use crate::runtime::AbortSignal;
use crate::weixin::{WeixinClient, WeixinFailureCode, WeixinRequestError};
use crate::weixin_inbound::{parse_updates_payload, WeixinInboundMessage};

pub type RuntimeFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// 重试退避（可恢复失败）：从 5s 起，翻倍到 60s 封顶。
pub const WEIXIN_RETRY_BASE: Duration = Duration::from_secs(5);
pub const WEIXIN_RETRY_MAX: Duration = Duration::from_secs(60);
/// 无更新时的空闲轮询间隔（长轮询已经等 90s，这里只防热循环）。
pub const WEIXIN_IDLE_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeixinRuntimeStatus {
    pub bot_id: String,
    pub provider: &'static str,
    /// disabled | idle | polling | connected | error
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_update_at: Option<i64>,
}

impl WeixinRuntimeStatus {
    pub fn new(bot_id: &str, status: &'static str, message: Option<String>) -> Self {
        Self {
            bot_id: bot_id.to_string(),
            provider: "weixin",
            status,
            message,
            last_update_at: None,
        }
    }
}

/// 宿主注入面：凭据、回调处理、游标持久化、状态投影。
pub trait WeixinRuntimeHost: Send + Sync {
    fn load_token<'a>(&'a self, bot_id: &'a str) -> RuntimeFuture<'a, Option<String>>;
    /// 处理一条入站消息；`Err` = 可恢复失败（不提交游标，下一轮重投）。
    fn process_message<'a>(
        &'a self,
        bot_id: &'a str,
        message: &'a WeixinInboundMessage,
    ) -> RuntimeFuture<'a, Result<(), String>>;
    fn persist_buf<'a>(&'a self, bot_id: &'a str, buf: &'a str) -> RuntimeFuture<'a, Result<(), String>>;
    fn set_status(&self, status: WeixinRuntimeStatus);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollOutcome {
    /// 本页处理完成，游标已提交。
    Advanced { buf: Option<String>, messages: usize },
    /// 本页没有任何新消息。
    Idle,
    /// 回调失败：游标未提交，稍后重投。
    CallbackFailed { error: String },
    /// 会话失效 / 凭据缺失：停止轮询。
    SessionExpired { error: String },
    /// 网络/服务端可恢复错误：稍后重试。
    RetryLater { error: String },
    /// 已取消。
    Cancelled,
}

pub struct WeixinPollLoop {
    bot_id: String,
    client: WeixinClient,
    dedupe: InboundDeliveryDedupe,
    host: Arc<dyn WeixinRuntimeHost>,
    signal: AbortSignal,
    buf: String,
    retry_delay: Duration,
}

impl WeixinPollLoop {
    pub fn new(
        bot_id: impl Into<String>,
        http: Arc<dyn HttpJsonPort>,
        host: Arc<dyn WeixinRuntimeHost>,
        signal: AbortSignal,
        initial_buf: impl Into<String>,
    ) -> Self {
        Self {
            bot_id: bot_id.into(),
            client: WeixinClient::new(http),
            dedupe: InboundDeliveryDedupe::new(crate::inbound::BOT_INBOUND_DEDUPE_TTL),
            host,
            signal,
            buf: initial_buf.into(),
            retry_delay: WEIXIN_RETRY_BASE,
        }
    }

    pub fn buf(&self) -> &str {
        &self.buf
    }

    /// 一次轮询迭代（测试直接驱动它，生产由 `run` 循环调用）。
    pub async fn poll_once(&mut self, now_ms: i64) -> PollOutcome {
        if self.signal.is_aborted() {
            return PollOutcome::Cancelled;
        }
        let token = self.host.load_token(&self.bot_id).await;
        let Some(token) = token.filter(|token| !token.trim().is_empty()) else {
            let error = "Weixin iLink bot token is missing. Scan the Weixin login QR code first."
                .to_string();
            self.host
                .set_status(WeixinRuntimeStatus::new(&self.bot_id, "error", Some(error.clone())));
            return PollOutcome::SessionExpired { error };
        };
        match self.client.get_updates(&token, &self.buf, None).await {
            Ok(payload) => {
                let page = parse_updates_payload(&self.bot_id, &payload);
                let mut processed = 0usize;
                for message in &page.messages {
                    if self.signal.is_aborted() {
                        return PollOutcome::Cancelled;
                    }
                    let identity = identity_of(message);
                    if !self.dedupe.mark(&identity, instant_now()) {
                        continue;
                    }
                    // 回合在后台执行：轮询必须继续推进，否则 /stop 与 /permission
                    // 会排在长回合后面永远收不到。失败由 handler 回给用户（游标已提交）。
                    let host = Arc::clone(&self.host);
                    let bot_id = self.bot_id.clone();
                    let message = message.clone();
                    tokio::spawn(async move {
                        if let Err(error) = host.process_message(&bot_id, &message).await {
                            host.set_status(WeixinRuntimeStatus::new(
                                &bot_id,
                                "error",
                                Some(error),
                            ));
                        }
                    });
                    processed += 1;
                }
                let next_buf = page.buf.clone();
                if let Some(next) = next_buf.as_deref().filter(|value| !value.is_empty()) {
                    if let Err(error) = self.host.persist_buf(&self.bot_id, next).await {
                        return PollOutcome::RetryLater { error };
                    }
                    self.buf = next.to_string();
                }
                if page.messages.is_empty() && next_buf.is_none() {
                    return PollOutcome::Idle;
                }
                let mut status =
                    WeixinRuntimeStatus::new(&self.bot_id, "connected", None);
                status.last_update_at = Some(now_ms);
                self.host.set_status(status);
                self.retry_delay = WEIXIN_RETRY_BASE;
                PollOutcome::Advanced {
                    buf: next_buf,
                    messages: processed,
                }
            }
            Err(WeixinRequestError { code: Some(WeixinFailureCode::SessionExpired), message }) => {
                self.host.set_status(WeixinRuntimeStatus::new(
                    &self.bot_id,
                    "error",
                    Some(message.clone()),
                ));
                PollOutcome::SessionExpired { error: message }
            }
            Err(error) => {
                self.host.set_status(WeixinRuntimeStatus::new(
                    &self.bot_id,
                    "error",
                    Some(error.message.clone()),
                ));
                PollOutcome::RetryLater {
                    error: error.message,
                }
            }
        }
    }

    /// 长轮询循环：可恢复失败按 5s→60s 退避；会话失效退出。
    pub async fn run(&mut self) {
        loop {
            if self.signal.is_aborted() {
                return;
            }
            self.host
                .set_status(WeixinRuntimeStatus::new(&self.bot_id, "polling", None));
            let outcome = self.poll_once(now_millis()).await;
            match outcome {
                PollOutcome::Cancelled | PollOutcome::SessionExpired { .. } => return,
                PollOutcome::Advanced { .. } | PollOutcome::Idle => {
                    self.signal.wait_for(WEIXIN_IDLE_DELAY.as_millis() as u64).await;
                }
                PollOutcome::CallbackFailed { .. } | PollOutcome::RetryLater { .. } => {
                    let delay = self.retry_delay;
                    self.retry_delay = (self.retry_delay * 2).min(WEIXIN_RETRY_MAX);
                    self.signal.wait_for(delay.as_millis() as u64).await;
                }
            }
        }
    }
}

fn identity_of(message: &WeixinInboundMessage) -> InboundIdentity {
    InboundIdentity {
        bot_id: message.bot_id.clone(),
        provider: "weixin".to_string(),
        provider_user_id: message.actor.provider_user_id.clone(),
        chat_id: message.actor.chat_id.clone(),
        provider_message_id: message.actor.provider_message_id.clone(),
    }
}

fn instant_now() -> std::time::Instant {
    std::time::Instant::now()
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{HttpBytesResponse, HttpFuture, HttpJsonResponse, OutboundHttpRequest};
    use std::sync::Mutex;

    struct FakeHttp {
        responses: Mutex<Vec<Result<HttpJsonResponse, String>>>,
    }

    impl HttpJsonPort for FakeHttp {
        fn post_json<'a>(
            &'a self,
            _request: &'a OutboundHttpRequest,
        ) -> HttpFuture<'a, Result<HttpJsonResponse, String>> {
            let response = self
                .responses
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| Err("no scripted response".to_string()));
            Box::pin(async move { response })
        }

        fn get_bytes<'a>(
            &'a self,
            _url: &'a str,
            _headers: &'a [(String, String)],
            _timeout: Duration,
        ) -> HttpFuture<'a, Result<HttpBytesResponse, String>> {
            Box::pin(async move { Err("not used".to_string()) })
        }
    }

    struct FakeHost {
        token: Option<String>,
        fail_message_once: Mutex<bool>,
        processed: Mutex<Vec<String>>,
        persisted: Mutex<Vec<String>>,
        statuses: Mutex<Vec<String>>,
    }

    impl FakeHost {
        fn new(token: &str) -> Arc<Self> {
            Arc::new(Self {
                token: Some(token.to_string()),
                fail_message_once: Mutex::new(false),
                processed: Mutex::new(Vec::new()),
                persisted: Mutex::new(Vec::new()),
                statuses: Mutex::new(Vec::new()),
            })
        }
    }

    impl WeixinRuntimeHost for FakeHost {
        fn load_token<'a>(&'a self, _bot_id: &'a str) -> RuntimeFuture<'a, Option<String>> {
            let token = self.token.clone();
            Box::pin(async move { token })
        }

        fn process_message<'a>(
            &'a self,
            _bot_id: &'a str,
            message: &'a WeixinInboundMessage,
        ) -> RuntimeFuture<'a, Result<(), String>> {
            let mut fail = self.fail_message_once.lock().unwrap();
            if *fail {
                *fail = false;
                return Box::pin(async move { Err("callback failed".to_string()) });
            }
            drop(fail);
            self.processed
                .lock()
                .unwrap()
                .push(message.actor.provider_message_id.clone().unwrap_or_default());
            Box::pin(async move { Ok(()) })
        }

        fn persist_buf<'a>(
            &'a self,
            _bot_id: &'a str,
            buf: &'a str,
        ) -> RuntimeFuture<'a, Result<(), String>> {
            self.persisted.lock().unwrap().push(buf.to_string());
            Box::pin(async move { Ok(()) })
        }

        fn set_status(&self, status: WeixinRuntimeStatus) {
            self.statuses.lock().unwrap().push(status.status.to_string());
        }
    }

    fn page(messages: serde_json::Value, buf: &str) -> Result<HttpJsonResponse, String> {
        Ok(HttpJsonResponse {
            status: 200,
            payload: serde_json::json!({
                "ret": 0,
                "data": { "messages": messages, "get_updates_buf": buf }
            }),
        })
    }

    fn loop_for(http: Arc<dyn HttpJsonPort>, host: Arc<FakeHost>) -> WeixinPollLoop {
        WeixinPollLoop::new("bot-1", http, host, AbortSignal::new(), "")
    }

    #[tokio::test]
    async fn commits_buf_only_after_messages_processed() {
        let http = Arc::new(FakeHttp {
            responses: Mutex::new(vec![page(
                serde_json::json!([{
                    "from_user_id": "u1",
                    "message_id": "m1",
                    "item_list": [{ "type": 1, "text_item": { "text": "hi" } }]
                }]),
                "buf-1",
            )]),
        });
        let host = FakeHost::new("token");
        let mut poll = loop_for(http, host.clone());
        let outcome = poll.poll_once(1_000).await;
        assert_eq!(
            outcome,
            PollOutcome::Advanced {
                buf: Some("buf-1".to_string()),
                messages: 1
            }
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(*host.processed.lock().unwrap(), vec!["m1".to_string()]);
        assert_eq!(*host.persisted.lock().unwrap(), vec!["buf-1".to_string()]);
        assert_eq!(poll.buf(), "buf-1");
    }

    #[tokio::test]
    async fn callback_failure_is_reported_without_blocking_the_cursor() {
        let http = Arc::new(FakeHttp {
            responses: Mutex::new(vec![page(
                serde_json::json!([{
                    "from_user_id": "u1",
                    "message_id": "m1",
                    "item_list": [{ "type": 1, "text_item": { "text": "hi" } }]
                }]),
                "buf-1",
            )]),
        });
        let host = FakeHost::new("token");
        *host.fail_message_once.lock().unwrap() = true;
        let mut poll = loop_for(http, host.clone());
        // 回合在后台执行：轮询立即推进游标，失败后由状态表上报（不再重投同一页）。
        let first = poll.poll_once(1_000).await;
        assert!(matches!(first, PollOutcome::Advanced { messages: 1, .. }));
        assert_eq!(poll.buf(), "buf-1");
        assert_eq!(*host.persisted.lock().unwrap(), vec!["buf-1".to_string()]);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(host.processed.lock().unwrap().is_empty());
        assert!(host
            .statuses
            .lock()
            .unwrap()
            .iter()
            .any(|status| status == "error"));
    }

    #[tokio::test]
    async fn duplicate_message_id_is_skipped_within_page_retries() {
        let http = Arc::new(FakeHttp {
            responses: Mutex::new(vec![
                page(
                    serde_json::json!([{
                        "from_user_id": "u1",
                        "message_id": "m1",
                        "item_list": [{ "type": 1, "text_item": { "text": "hi" } }]
                    }]),
                    "buf-2",
                ),
                page(
                    serde_json::json!([{
                        "from_user_id": "u1",
                        "message_id": "m1",
                        "item_list": [{ "type": 1, "text_item": { "text": "hi" } }]
                    }]),
                    "buf-1",
                ),
            ]),
        });
        let host = FakeHost::new("token");
        let mut poll = loop_for(http, host.clone());
        poll.poll_once(1_000).await;
        poll.poll_once(2_000).await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        // 第二轮同一 messageId 被去重，不重复处理。
        assert_eq!(*host.processed.lock().unwrap(), vec!["m1".to_string()]);
    }

    #[tokio::test]
    async fn missing_token_stops_with_session_expired() {
        let http = Arc::new(FakeHttp {
            responses: Mutex::new(vec![]),
        });
        let host = Arc::new(FakeHost {
            token: None,
            fail_message_once: Mutex::new(false),
            processed: Mutex::new(Vec::new()),
            persisted: Mutex::new(Vec::new()),
            statuses: Mutex::new(Vec::new()),
        });
        let mut poll = loop_for(http, host);
        assert!(matches!(
            poll.poll_once(1_000).await,
            PollOutcome::SessionExpired { .. }
        ));
    }
}
