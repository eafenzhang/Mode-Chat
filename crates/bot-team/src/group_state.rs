//! 频道运行状态（持久化）：每个成员在本频道最后一次会话 id / 状态 / 回复预览。
//!
//! 为什么要单独一份：transcript 是用户可见历史，不能塞会话指针；session 指针必须
//! 跨 App 重启保留，下一轮才能续用同一段上下文。文件按频道隔离：
//! `{botBase}/.mode/v2/groups/<groupId>/state.json`。
//!
//! 纯模型 + 校验；IO 在 `apps/bot-tauri/src-tauri/src/group_state.rs`。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupMemberState {
    /// 该成员在本频道当前段的 v4 session id；缺省 = 下一轮新建。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// 最后一次回合开始/结束的 Unix 毫秒。
    #[serde(default)]
    pub last_turn_at: u64,
    /// 最后一次终态（completed / error / paused / waiting / stopped）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    /// 最近一次回复的短预览（只给 UI/排障，不参与 prompt）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reply_preview: Option<String>,
    /// 闲置守卫：连续只回 ack/重复文本的次数（实质回复清零；规则见 idle_guard）。
    #[serde(default)]
    pub idle_streak: u32,
    /// 闲置守卫：达到暂停线后接力（bot→bot 交接）跳过该成员。
    #[serde(default)]
    pub idle_paused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupState {
    pub version: u32,
    #[serde(default)]
    pub members: BTreeMap<String, GroupMemberState>,
    #[serde(default)]
    pub updated_at: u64,
}

impl GroupState {
    pub fn empty() -> Self {
        Self {
            version: 1,
            members: BTreeMap::new(),
            updated_at: 0,
        }
    }
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

const REPLY_PREVIEW_MAX_CHARS: usize = 500;

/// 校验并归一化：成员 id 不能为空；sessionId / lastStatus trim 后为空则丢弃；
/// 回复预览截到 500 字符（UTF-16 口径不敏感，UI 只展示）。
pub fn validate_group_state(state: GroupState) -> Result<GroupState, String> {
    if state.version != 1 {
        return Err(format!("group state version 必须为 1，实收 {}", state.version));
    }
    let mut members = BTreeMap::new();
    for (member_id, entry) in state.members {
        let member_id = non_empty_trimmed(&member_id)
            .ok_or_else(|| "group state 的成员 id 为空".to_string())?;
        let session_id = entry.session_id.as_deref().and_then(non_empty_trimmed);
        let last_status = entry.last_status.as_deref().and_then(non_empty_trimmed);
        let last_reply_preview = entry
            .last_reply_preview
            .as_deref()
            .and_then(non_empty_trimmed)
            .map(|text| text.chars().take(REPLY_PREVIEW_MAX_CHARS).collect());
        members.insert(
            member_id,
            GroupMemberState {
                session_id,
                last_turn_at: entry.last_turn_at,
                last_status,
                last_reply_preview,
                idle_streak: entry.idle_streak,
                idle_paused: entry.idle_paused,
            },
        );
    }
    Ok(GroupState {
        version: 1,
        members,
        updated_at: state.updated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_file_gets_empty_state() {
        let state: GroupState =
            serde_json::from_str(r#"{"version":1}"#).expect("deserialize");
        assert!(state.members.is_empty());
        assert_eq!(validate_group_state(state).expect("valid").version, 1);
    }

    #[test]
    fn trims_and_drops_blank_fields() {
        let mut members = BTreeMap::new();
        members.insert(
            " t1 ".to_string(),
            GroupMemberState {
                session_id: Some("  ".to_string()),
                last_turn_at: 5,
                last_status: Some(" completed ".to_string()),
                last_reply_preview: Some("  ok  ".to_string()),
                idle_streak: 0,
                idle_paused: false,
            },
        );
        let state = validate_group_state(GroupState {
            version: 1,
            members,
            updated_at: 7,
        })
        .expect("valid");
        let entry = state.members.get("t1").expect("trimmed key");
        assert_eq!(entry.session_id, None);
        assert_eq!(entry.last_status.as_deref(), Some("completed"));
        assert_eq!(entry.last_reply_preview.as_deref(), Some("ok"));
    }

    #[test]
    fn rejects_bad_version_and_blank_member() {
        assert!(validate_group_state(GroupState {
            version: 2,
            members: BTreeMap::new(),
            updated_at: 0,
        })
        .is_err());
        let mut members = BTreeMap::new();
        members.insert("  ".to_string(), GroupMemberState::default());
        assert!(validate_group_state(GroupState {
            version: 1,
            members,
            updated_at: 0,
        })
        .is_err());
    }

    #[test]
    fn truncates_long_reply_preview() {
        let mut members = BTreeMap::new();
        members.insert(
            "t1".to_string(),
            GroupMemberState {
                last_reply_preview: Some("x".repeat(800)),
                ..Default::default()
            },
        );
        let state = validate_group_state(GroupState {
            version: 1,
            members,
            updated_at: 0,
        })
        .expect("valid");
        assert_eq!(
            state.members["t1"].last_reply_preview.as_deref().map(str::len),
            Some(REPLY_PREVIEW_MAX_CHARS)
        );
    }
}
