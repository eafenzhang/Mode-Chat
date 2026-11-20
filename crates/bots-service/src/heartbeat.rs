//! 机器人心跳的纯规则（N6 W4 quiet run）。
//!
//! 真源：`packages/services/src/bots/botHeartbeat.ts` 的行为——归一化间隔、到点判定、
//! 心跳目标只选私聊、整段回复只有 `HEARTBEAT_OK` 时保持安静。调度与投递在宿主层接线。

use crate::BotHeartbeat;

pub const BOT_HEARTBEAT_PROMPT_MARKER_ZH: &str = "[心跳] 主动检查";
pub const BOT_HEARTBEAT_PROMPT_MARKER_EN: &str = "[Heartbeat] Proactive check";
pub const BOT_HEARTBEAT_MIN_INTERVAL_MINUTES: i64 = 15;
pub const BOT_HEARTBEAT_MAX_INTERVAL_MINUTES: i64 = 24 * 60;
pub const BOT_HEARTBEAT_DEFAULT_INTERVAL_MINUTES: i64 = 60;
pub const HEARTBEAT_INSTRUCTIONS_MAX_UTF16: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizedHeartbeat {
    pub interval_minutes: i64,
}

/// 心跳配置归一化：未启用 → None；间隔裁剪到 15–1440 分钟。
pub fn normalize_heartbeat(config: Option<&BotHeartbeat>) -> Option<NormalizedHeartbeat> {
    let config = config?;
    if !config.enabled {
        return None;
    }
    Some(NormalizedHeartbeat {
        interval_minutes: config
            .interval_minutes
            .clamp(BOT_HEARTBEAT_MIN_INTERVAL_MINUTES, BOT_HEARTBEAT_MAX_INTERVAL_MINUTES),
    })
}

/// 距离上次心跳是否已满足间隔；缺失记录视为应触发。
pub fn is_heartbeat_due(
    now_ms: u64,
    last_heartbeat_at: Option<u64>,
    interval_minutes: i64,
) -> bool {
    let Some(last) = last_heartbeat_at else {
        return true;
    };
    let interval_ms = (interval_minutes.max(0) as u64).saturating_mul(60_000);
    now_ms.saturating_sub(last) >= interval_ms
}

/// 心跳提示词只由心跳生成，宿主据此把整轮标记为 quiet。
pub fn is_heartbeat_prompt_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with(BOT_HEARTBEAT_PROMPT_MARKER_ZH)
        || trimmed.starts_with(BOT_HEARTBEAT_PROMPT_MARKER_EN)
}

/// 整段回复只有 HEARTBEAT_OK（允许空白与强调符号）时保持安静。
pub fn is_heartbeat_ok_only(text: &str) -> bool {
    let stripped: String = text
        .chars()
        .filter(|character| {
            !character.is_whitespace() && !matches!(character, '*' | '_' | '`' | '~')
        })
        .collect::<String>()
        .to_ascii_uppercase();
    stripped == "HEARTBEAT_OK" || stripped == "HEARTBEATOK"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeartbeatConversation {
    pub conversation_key: String,
    /// `private` | `group`
    pub kind: String,
    /// `draft` | `task`
    pub mode: String,
    pub active_task_id: Option<String>,
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
}

/// 心跳只投递私聊；优先有 activeTaskId 的私聊，否则第一个私聊。
pub fn pick_heartbeat_conversation(
    conversations: &[HeartbeatConversation],
) -> Option<&HeartbeatConversation> {
    let private: Vec<&HeartbeatConversation> = conversations
        .iter()
        .filter(|conversation| conversation.kind == "private")
        .collect();
    private
        .iter()
        .copied()
        .find(|conversation| {
            conversation.mode == "task"
                && conversation
                    .active_task_id
                    .as_deref()
                    .is_some_and(|id| !id.trim().is_empty())
        })
        .or_else(|| private.first().copied())
}

fn truncate_utf16(value: &str, max_units: usize) -> String {
    let mut result = String::new();
    let mut used = 0usize;
    for character in value.chars() {
        let length = character.len_utf16();
        if used + length > max_units {
            break;
        }
        used += length;
        result.push(character);
    }
    result
}

/// 心跳提示词语言：设置值 `en*` 或 `fa-IR` → en-US，其余（含缺失）→ zh-CN。
pub fn heartbeat_locale(settings_locale: Option<&str>) -> &'static str {
    match settings_locale.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if value.starts_with("en") || value == "fa-ir" => "en-US",
        _ => "zh-CN",
    }
}

/// 心跳 prompt（中/英），`instructions` 截断到 4096 UTF-16 code unit。
pub fn build_heartbeat_prompt(locale: &str, instructions: Option<&str>) -> String {
    let instructions = instructions
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| truncate_utf16(value, HEARTBEAT_INSTRUCTIONS_MAX_UTF16));
    if locale == "en-US" {
        [
            format!(
                "{BOT_HEARTBEAT_PROMPT_MARKER_EN}. Review the current workspace for anything the user should know about."
            ),
            instructions
                .map(|value| format!("Follow these project-specific instructions:\n{value}"))
                .unwrap_or_default(),
            "If there is something worth reporting, reply with a concise summary. If everything is fine, reply with exactly HEARTBEAT_OK and nothing else.".to_string(),
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
    } else {
        [
            format!(
                "{BOT_HEARTBEAT_PROMPT_MARKER_ZH}。请查看当前工作区是否有用户需要关注的变化。"
            ),
            instructions
                .map(|value| format!("按以下项目指引执行：\n{value}"))
                .unwrap_or_default(),
            "有需要汇报的内容时用简洁的语言说明；一切正常时只回复 HEARTBEAT_OK，不要输出其他任何内容。".to_string(),
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BotHeartbeatActiveHours;

    fn config(enabled: bool, interval: i64) -> BotHeartbeat {
        BotHeartbeat {
            enabled,
            interval_minutes: interval,
            active_hours: None,
        }
    }

    #[test]
    fn normalizes_and_clamps_interval() {
        assert!(normalize_heartbeat(None).is_none());
        assert!(normalize_heartbeat(Some(&config(false, 60))).is_none());
        assert_eq!(
            normalize_heartbeat(Some(&config(true, 1))),
            Some(NormalizedHeartbeat { interval_minutes: 15 })
        );
        assert_eq!(
            normalize_heartbeat(Some(&config(true, 5_000))),
            Some(NormalizedHeartbeat {
                interval_minutes: BOT_HEARTBEAT_MAX_INTERVAL_MINUTES
            })
        );
        let legacy = BotHeartbeat {
            enabled: true,
            interval_minutes: 60,
            active_hours: Some(BotHeartbeatActiveHours {
                start: "09:00".into(),
                end: "18:00".into(),
            }),
        };
        assert!(normalize_heartbeat(Some(&legacy)).is_some());
    }

    #[test]
    fn due_check_uses_last_timestamp() {
        assert!(is_heartbeat_due(1_000, None, 60));
        assert!(!is_heartbeat_due(1_000, Some(999_000), 60));
        assert!(is_heartbeat_due(3_601_000, Some(1_000), 60));
    }

    #[test]
    fn detects_heartbeat_prompt_markers() {
        assert!(is_heartbeat_prompt_text("[心跳] 主动检查。请查看…"));
        assert!(is_heartbeat_prompt_text("  [Heartbeat] Proactive check"));
        assert!(!is_heartbeat_prompt_text("普通消息"));
    }

    #[test]
    fn heartbeat_ok_only_tolerates_markdown_and_whitespace() {
        assert!(is_heartbeat_ok_only("HEARTBEAT_OK"));
        assert!(is_heartbeat_ok_only("  **HEARTBEAT_OK**  "));
        assert!(is_heartbeat_ok_only("`HEARTBEAT_OK`\n"));
        assert!(!is_heartbeat_ok_only("HEARTBEAT_OK 但有更新"));
        assert!(!is_heartbeat_ok_only("有更新"));
    }

    #[test]
    fn picks_private_conversation_and_prefers_active_task() {
        let conversations = vec![
            HeartbeatConversation {
                conversation_key: "group:g1".into(),
                kind: "group".into(),
                mode: "draft".into(),
                active_task_id: None,
                workspace_path: "D:/ws".into(),
                workspace_identity: None,
            },
            HeartbeatConversation {
                conversation_key: "private:u1".into(),
                kind: "private".into(),
                mode: "draft".into(),
                active_task_id: None,
                workspace_path: "D:/ws".into(),
                workspace_identity: None,
            },
            HeartbeatConversation {
                conversation_key: "private:u2".into(),
                kind: "private".into(),
                mode: "task".into(),
                active_task_id: Some("t2".into()),
                workspace_path: "D:/ws2".into(),
                workspace_identity: None,
            },
        ];
        let picked = pick_heartbeat_conversation(&conversations).unwrap();
        assert_eq!(picked.conversation_key, "private:u2");
        assert!(pick_heartbeat_conversation(&conversations[..1]).is_none());
    }

    #[test]
    fn locale_maps_settings_value_with_safe_fallback() {
        assert_eq!(heartbeat_locale(None), "zh-CN");
        assert_eq!(heartbeat_locale(Some("zh-CN")), "zh-CN");
        assert_eq!(heartbeat_locale(Some("en-US")), "en-US");
        assert_eq!(heartbeat_locale(Some("EN-gb")), "en-US");
        assert_eq!(heartbeat_locale(Some("fa-IR")), "en-US");
        assert_eq!(heartbeat_locale(Some("ja-JP")), "zh-CN");
    }

    #[test]
    fn prompt_includes_marker_and_truncates_instructions() {
        let prompt = build_heartbeat_prompt("zh-CN", Some(&"x".repeat(5_000)));
        assert!(prompt.starts_with(BOT_HEARTBEAT_PROMPT_MARKER_ZH));
        assert!(prompt.contains("HEARTBEAT_OK"));
        assert_eq!(prompt.matches('x').count(), HEARTBEAT_INSTRUCTIONS_MAX_UTF16);
        let english = build_heartbeat_prompt("en-US", None);
        assert!(english.starts_with(BOT_HEARTBEAT_PROMPT_MARKER_EN));
    }
}
