//! 入站消息幂等去重（`inboundDedupe.ts` 的 Rust 移植）。
//!
//! 平台会重投同一 provider message id（飞书 WS 重连、微信/企微重试），普通消息有创建
//! 会话/发送任务的副作用，因此必须在业务处理前按「bot+provider+会话+messageId」幂等。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 去重窗口：覆盖平台重连/重投的最长间隔。
pub const BOT_INBOUND_DEDUPE_TTL: Duration = Duration::from_secs(120);

/// 去重键所需的入站消息身份（`BotInboundMessage.actor` 的最小投影）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundIdentity {
    pub bot_id: String,
    pub provider: String,
    pub provider_user_id: String,
    pub chat_id: Option<String>,
    pub provider_message_id: Option<String>,
}

/// 对话身份 + provider message id 构成去重键；没有 message id 时返回 None（不去重）。
pub fn build_inbound_delivery_key(identity: &InboundIdentity) -> Option<String> {
    let provider_message_id = identity
        .provider_message_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let conversation = identity
        .chat_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(identity.provider_user_id.as_str());
    Some(format!(
        "{}::{}::{}::{}",
        identity.bot_id, identity.provider, conversation, provider_message_id
    ))
}

pub struct InboundDeliveryDedupe {
    ttl: Duration,
    recent_at_by_key: Mutex<HashMap<String, Instant>>,
}

impl InboundDeliveryDedupe {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            recent_at_by_key: Mutex::new(HashMap::new()),
        }
    }

    /// 生成去重键；平台未提供 message id 时返回 None。
    pub fn key(&self, identity: &InboundIdentity) -> Option<String> {
        build_inbound_delivery_key(identity)
    }

    /// 标记已处理；`false` = 窗口内重复，调用方应跳过。
    pub fn mark(&self, identity: &InboundIdentity, now: Instant) -> bool {
        let Some(key) = build_inbound_delivery_key(identity) else {
            return true;
        };
        let mut recent = self.recent_at_by_key.lock().unwrap();
        recent.retain(|_, at| now.duration_since(*at) < self.ttl);
        if recent.contains_key(&key) {
            return false;
        }
        recent.insert(key, now);
        true
    }

    /// 处理失败时释放记录，允许平台重投后重试。
    pub fn release(&self, identity: &InboundIdentity) {
        if let Some(key) = build_inbound_delivery_key(identity) {
            self.recent_at_by_key.lock().unwrap().remove(&key);
        }
    }

    /// 清空全部记录（dispose）。
    pub fn clear(&self) {
        self.recent_at_by_key.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(message_id: Option<&str>) -> InboundIdentity {
        InboundIdentity {
            bot_id: "b1".into(),
            provider: "feishu".into(),
            provider_user_id: "u1".into(),
            chat_id: Some("c1".into()),
            provider_message_id: message_id.map(str::to_string),
        }
    }

    #[test]
    fn same_message_id_is_deduped_within_ttl() {
        let dedupe = InboundDeliveryDedupe::new(BOT_INBOUND_DEDUPE_TTL);
        let at = Instant::now();
        assert!(dedupe.mark(&identity(Some("m1")), at));
        assert!(!dedupe.mark(&identity(Some("m1")), at));
        // 不同会话互不影响。
        let mut other = identity(Some("m1"));
        other.chat_id = Some("c2".into());
        assert!(dedupe.mark(&other, at));
        // 没有 message id 的消息不去重。
        assert!(dedupe.mark(&identity(None), at));
        assert!(dedupe.mark(&identity(None), at));
    }

    #[test]
    fn release_allows_retry_and_ttl_expires() {
        let dedupe = InboundDeliveryDedupe::new(Duration::from_millis(100));
        let at = Instant::now();
        assert!(dedupe.mark(&identity(Some("m1")), at));
        dedupe.release(&identity(Some("m1")));
        assert!(dedupe.mark(&identity(Some("m1")), at));
        // 窗口过期后可再次处理。
        assert!(dedupe.mark(
            &identity(Some("m1")),
            at + Duration::from_millis(150)
        ));
    }
}
