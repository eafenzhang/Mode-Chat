//! Bot 心跳的宿主侧公共件（微信/企微共用）。
//!
//! 纯规则在 `bots_service::heartbeat`（到点判定/目标选择/提示词）；这里只做 IO 与投影：
//! bot-state 的会话投影、上次心跳时间戳、`HEARTBEAT.md` 指令、locale 与运行中检查。

use std::time::{SystemTime, UNIX_EPOCH};

use bots_service::BotsRepo;
use bots_service::heartbeat::{
    HeartbeatConversation, is_heartbeat_due, normalize_heartbeat, pick_heartbeat_conversation,
};
use serde_json::Value;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// 从 bot-state 的 conversations 投影心跳目标；只保留有 workspacePath 的条目。
pub fn heartbeat_conversations(state: &Value, bot_id: &str) -> Vec<HeartbeatConversation> {
    let Some(map) = state
        .get("bots")
        .and_then(|bots| bots.get(bot_id))
        .and_then(|bot| bot.get("conversations"))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    map.iter()
        .filter_map(|(key, value)| {
            let object = value.as_object()?;
            let workspace_path = object
                .get("workspacePath")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())?
                .to_string();
            let kind = object
                .get("conversationKind")
                .and_then(Value::as_str)
                .filter(|kind| !kind.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if key.starts_with("group:") {
                        "group".to_string()
                    } else {
                        "private".to_string()
                    }
                });
            Some(HeartbeatConversation {
                conversation_key: key.clone(),
                kind,
                mode: object
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("draft")
                    .to_string(),
                active_task_id: object
                    .get("activeTaskId")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string),
                workspace_path,
                workspace_identity: object
                    .get("workspaceIdentity")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect()
}

/// 上次心跳时间戳（bot-state `bots.<id>.lastHeartbeatAt`）。
pub fn last_heartbeat_at(state: &Value, bot_id: &str) -> Option<u64> {
    state
        .get("bots")
        .and_then(|bots| bots.get(bot_id))
        .and_then(|entry| entry.get("lastHeartbeatAt"))
        .and_then(Value::as_u64)
}

/// 先写时间戳再跑回合：回合可能耗时，不能让它期间的下一 tick 重复触发。
pub fn record_heartbeat(repo: &BotsRepo, bot_id: &str, now: u64) -> Result<(), String> {
    repo.mutate_state(|state| {
        let bots = state
            .get_mut("bots")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| "bot-state 缺少 bots 对象".to_string())?;
        let entry = bots
            .entry(bot_id.to_string())
            .or_insert_with(|| serde_json::json!({ "botId": bot_id }));
        if let Some(object) = entry.as_object_mut() {
            object.insert("lastHeartbeatAt".into(), serde_json::json!(now));
        }
        Ok(())
    })
}

pub fn read_heartbeat_instructions(workspace_path: &str) -> Option<String> {
    let path = std::path::Path::new(workspace_path).join("HEARTBEAT.md");
    let value = std::fs::read_to_string(path).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// 心跳没有入站消息，语言取 bot 设置（`{dataRoot}/v2/setting.json` 的 `locale`）。
pub fn read_settings_locale(data_root: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(data_root.join("v2").join("setting.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value
        .get("locale")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeartbeatPlan {
    pub conversation: HeartbeatConversation,
    pub interval_minutes: i64,
}

/// 判定某个 bot 这一 tick 是否该跑心跳：到点 + 有私聊目标 + 投递通道可用。
pub fn plan_heartbeat(
    bot_id: &str,
    heartbeat: Option<&bots_service::BotHeartbeat>,
    state: &Value,
    now: u64,
    delivery_available: bool,
) -> Option<HeartbeatPlan> {
    if !delivery_available {
        return None;
    }
    let normalized = normalize_heartbeat(heartbeat)?;
    let last = last_heartbeat_at(state, bot_id);
    if !is_heartbeat_due(now, last, normalized.interval_minutes) {
        return None;
    }
    let conversations = heartbeat_conversations(state, bot_id);
    Some(HeartbeatPlan {
        conversation: pick_heartbeat_conversation(&conversations)?.clone(),
        interval_minutes: normalized.interval_minutes,
    })
}
