//! 企微通道共享组件（`wecomConnection.ts` + runtime 的去重/帧缓存纯逻辑）。
//!
//! 企微智能机器人走 `@wecom/aibot-node-sdk` 的 WebSocket 长连接（`openws.work.weixin.qq.com`），
//! Rust 侧没有对应 SDK。本模块先把**与协议无关**的部分移植过来，WS 帧协议实现留给宿主：
//! - 连接注册表（botId → 连接句柄；连接对象按 chatKey 保留最近一帧供流式回复/附件用）；
//! - 帧缓存上限 200（超限淘汰最旧）；
//! - 跨重启去重表：msgid 上限 200、保留 24h（覆盖 WS 重连重推窗口）；
//! - 会话键：群聊 `chatid`，单聊 `from.userid`。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

/// 入站帧按会话保留的最大数量；超出后淘汰最旧，避免长期运行内存增长。
pub const WECOM_LAST_FRAME_LIMIT: usize = 200;
/// 同一 bot 持久化的 msgid 上限与保留时长（覆盖 WS 重连重推窗口）。
pub const WECOM_MESSAGE_DEDUPE_LIMIT: usize = 200;
pub const WECOM_MESSAGE_DEDUPE_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// 从入站消息解析会话键：群聊用 chatid，单聊用发送者 userid。
pub fn resolve_chat_key(chat_id: Option<&str>, user_id: &str) -> String {
    chat_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(user_id.trim())
        .to_string()
}

/// 连接句柄（宿主注入）：协议实现由 WS 客户端提供，这里只要求按会话取最近入站帧。
pub trait WeComConnection: Send + Sync {
    fn get_last_frame(&self, chat_key: &str) -> Option<Value>;
}

#[derive(Clone, Default)]
pub struct WeComConnectionRegistry {
    connections: Arc<Mutex<HashMap<String, Arc<dyn WeComConnection>>>>,
}

impl WeComConnectionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_connection(&self, bot_id: &str) -> Option<Arc<dyn WeComConnection>> {
        self.connections.lock().unwrap().get(bot_id).cloned()
    }

    pub fn set_connection(&self, bot_id: &str, connection: Arc<dyn WeComConnection>) {
        self.connections
            .lock()
            .unwrap()
            .insert(bot_id.to_string(), connection);
    }

    /// 只有当前登记的连接才能被清除（换连接时旧连接的观望收尾不误删新连接）。
    pub fn clear_connection(&self, bot_id: &str, connection: &Arc<dyn WeComConnection>) {
        let mut map = self.connections.lock().unwrap();
        if map
            .get(bot_id)
            .is_some_and(|current| Arc::ptr_eq(current, connection))
        {
            map.remove(bot_id);
        }
    }
}

/// 按会话保留最近入站帧（TSDK 的 `getLastFrame` 语义）。
pub struct WeComFrameCache {
    limit: usize,
    order: Vec<String>,
    frames: HashMap<String, Value>,
}

impl WeComFrameCache {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            order: Vec::new(),
            frames: HashMap::new(),
        }
    }

    pub fn put(&mut self, chat_key: &str, frame: Value) {
        let key = chat_key.to_string();
        order_remove(&mut self.order, &key);
        self.order.push(key.clone());
        self.frames.insert(key, frame);
        while self.order.len() > self.limit {
            let oldest = self.order.remove(0);
            self.frames.remove(&oldest);
        }
    }

    pub fn get(&self, chat_key: &str) -> Option<&Value> {
        self.frames.get(chat_key)
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

fn order_remove(order: &mut Vec<String>, key: &str) {
    if let Some(index) = order.iter().position(|candidate| candidate == key) {
        order.remove(index);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeComDedupeEntry {
    pub id: String,
    pub at: i64,
}

/// 跨重启去重表（消息 msgid）。宿主负责落盘（防抖合并写）。
pub struct WeComDedupe {
    limit: usize,
    ttl_ms: i64,
    entries: Vec<WeComDedupeEntry>,
}

impl WeComDedupe {
    pub fn from_entries(entries: Vec<WeComDedupeEntry>) -> Self {
        Self {
            limit: WECOM_MESSAGE_DEDUPE_LIMIT,
            ttl_ms: WECOM_MESSAGE_DEDUPE_TTL_MS,
            entries,
        }
    }

    /// 标记 msgid；`false` = 重推/重复，调用方应跳过。
    pub fn mark(&mut self, id: &str, now: i64) -> bool {
        let id = id.trim();
        if id.is_empty() {
            return true;
        }
        self.entries.retain(|entry| now - entry.at < self.ttl_ms);
        if self.entries.iter().any(|entry| entry.id == id) {
            return false;
        }
        self.entries.push(WeComDedupeEntry {
            id: id.to_string(),
            at: now,
        });
        while self.entries.len() > self.limit {
            self.entries.remove(0);
        }
        true
    }

    pub fn snapshot(&self) -> Vec<WeComDedupeEntry> {
        self.entries.clone()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct FakeConnection {
        frames: HashMap<String, Value>,
    }

    impl WeComConnection for FakeConnection {
        fn get_last_frame(&self, chat_key: &str) -> Option<Value> {
            self.frames.get(chat_key).cloned()
        }
    }

    #[test]
    fn chat_key_prefers_group_chat_id() {
        assert_eq!(resolve_chat_key(Some(" room-1 "), "user-1"), "room-1");
        assert_eq!(resolve_chat_key(Some("  "), "user-1"), "user-1");
        assert_eq!(resolve_chat_key(None, " user-1 "), "user-1");
    }

    #[test]
    fn frame_cache_evicts_oldest_over_limit() {
        let mut cache = WeComFrameCache::new(2);
        cache.put("a", json!({ "reqId": 1 }));
        cache.put("b", json!({ "reqId": 2 }));
        cache.put("c", json!({ "reqId": 3 }));
        assert!(cache.get("a").is_none());
        assert_eq!(cache.get("b").unwrap()["reqId"], 2);
        // 更新已有会话会刷新为最新。
        cache.put("b", json!({ "reqId": 20 }));
        cache.put("d", json!({ "reqId": 4 }));
        assert!(cache.get("c").is_none());
        assert_eq!(cache.get("b").unwrap()["reqId"], 20);
    }

    #[test]
    fn registry_clears_only_matching_connection() {
        let registry = WeComConnectionRegistry::new();
        let first: Arc<dyn WeComConnection> = Arc::new(FakeConnection {
            frames: HashMap::new(),
        });
        let second: Arc<dyn WeComConnection> = Arc::new(FakeConnection {
            frames: HashMap::new(),
        });
        registry.set_connection("b1", Arc::clone(&first));
        registry.set_connection("b1", Arc::clone(&second));
        // 旧连接收尾时不能误删新连接。
        registry.clear_connection("b1", &first);
        assert!(registry.get_connection("b1").is_some());
        registry.clear_connection("b1", &second);
        assert!(registry.get_connection("b1").is_none());
    }

    #[test]
    fn dedupe_marks_once_and_prunes_by_ttl() {
        let mut dedupe = WeComDedupe::from_entries(Vec::new());
        assert!(dedupe.mark("m1", 1_000));
        assert!(!dedupe.mark("m1", 1_100));
        // 空 id 不去重。
        assert!(dedupe.mark(" ", 1_100));
        // TTL 过期后可再次处理。
        assert!(dedupe.mark(
            "m1",
            1_000 + WECOM_MESSAGE_DEDUPE_TTL_MS + 1
        ));
    }

    #[test]
    fn dedupe_caps_entries() {
        let mut dedupe = WeComDedupe::from_entries(Vec::new());
        for index in 0..(WECOM_MESSAGE_DEDUPE_LIMIT + 5) {
            assert!(dedupe.mark(&format!("m{index}"), index as i64));
        }
        assert_eq!(dedupe.len(), WECOM_MESSAGE_DEDUPE_LIMIT);
        // 最旧的已被淘汰，可再次标记。
        assert!(dedupe.mark("m0", 10_000));
    }
}
