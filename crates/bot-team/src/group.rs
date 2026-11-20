//! 群组元数据模型与校验。
//!
//! 与 renderer 的 `groups/types.ts` 对齐：camelCase、管理员必须在成员里、
//! 成员 trim + 去重、群 id trim + 唯一。失败返回 `Err`，调用方拒绝写盘或隔离原文件。

use serde::{Deserialize, Serialize};

/// 单个群。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupRecord {
    pub id: String,
    pub name: String,
    pub admin_id: String,
    #[serde(default)]
    pub member_ids: Vec<String>,
    #[serde(default)]
    pub instructions: String,
    /// 群记忆（长期事实）：与群规则一起注入每个成员的回合前置上下文。
    #[serde(default)]
    pub memory: String,
    /// 频道共享工作目录；空 = 各成员自己的目录。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// 应答规则：`mentions-first`（默认，@优先）或 `admin-only`。
    #[serde(default)]
    pub responder_policy: Option<String>,
    /// 频道级审批覆盖：`inherit`（默认）/`ask`/`auto`/`full`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_override: Option<String>,
    /// 归档只影响 UI/排序，不改变成员会话与 transcript。
    #[serde(default)]
    pub archived: bool,
    /// 侧栏置顶（DS `room.pinned`）；只影响排序。
    #[serde(default)]
    pub pinned: bool,
    /// 从侧栏隐藏（DS `room.hidden`）；选中/搜索仍可到达。
    #[serde(default)]
    pub hidden: bool,
    /// 频道色（UI 预留）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub created_at: String,
}

/// 落盘文件的顶层形状。version 恒为 1。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupsFile {
    pub version: u32,
    #[serde(default)]
    pub groups: Vec<GroupRecord>,
}

impl GroupsFile {
    pub fn empty() -> Self {
        Self {
            version: 1,
            groups: Vec::new(),
        }
    }
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// 校验并归一化。规则与 TS `validateGroupsFile` 对齐。
pub fn validate_groups_file(file: GroupsFile) -> Result<GroupsFile, String> {
    if file.version != 1 {
        return Err(format!("groups version 必须为 1，实收 {}", file.version));
    }
    let mut groups = Vec::with_capacity(file.groups.len());
    let mut seen_ids = std::collections::HashSet::new();
    for (index, record) in file.groups.into_iter().enumerate() {
        let id =
            non_empty_trimmed(&record.id).ok_or_else(|| format!("group[{index}] 的 id 为空"))?;
        let name = non_empty_trimmed(&record.name)
            .ok_or_else(|| format!("group[{index}] 的 name 为空"))?;
        let admin_id = non_empty_trimmed(&record.admin_id)
            .ok_or_else(|| format!("group[{index}] 的 adminId 为空"))?;

        let mut member_ids: Vec<String> = Vec::with_capacity(record.member_ids.len());
        for member in record.member_ids {
            let member = non_empty_trimmed(&member)
                .ok_or_else(|| format!("group[{index}] 的 memberIds 含空值"))?;
            if !member_ids.iter().any(|existing| existing == &member) {
                member_ids.push(member);
            }
        }
        if member_ids.is_empty() {
            return Err(format!("group[{index}] 的 memberIds 不能为空"));
        }
        if !member_ids.iter().any(|member| member == &admin_id) {
            return Err(format!("group[{index}] 的 adminId 必须在 memberIds 里"));
        }
        if !seen_ids.insert(id.clone()) {
            return Err(format!("group id 重复：{id}"));
        }
        let directory = record.directory.as_deref().and_then(non_empty_trimmed);
        let responder_policy = match record.responder_policy.as_deref().map(str::trim) {
            None | Some("") => "mentions-first".to_string(),
            Some("mentions-first") => "mentions-first".to_string(),
            Some("admin-only") => "admin-only".to_string(),
            Some(other) => {
                return Err(format!(
                    "group[{index}] 的 responderPolicy 非法：{other}（只允许 mentions-first / admin-only）"
                ))
            }
        };
        let approval_override = match record.approval_override.as_deref().map(str::trim) {
            None | Some("") | Some("inherit") => None,
            Some("ask") => Some("ask".to_string()),
            Some("auto") => Some("auto".to_string()),
            Some("full") => Some("full".to_string()),
            Some(other) => {
                return Err(format!(
                    "group[{index}] 的 approvalOverride 非法：{other}（只允许 inherit / ask / auto / full）"
                ))
            }
        };
        let color = record.color.as_deref().and_then(non_empty_trimmed);
        groups.push(GroupRecord {
            id,
            name,
            admin_id,
            member_ids,
            instructions: record.instructions,
            memory: record.memory,
            directory,
            responder_policy: Some(responder_policy),
            approval_override,
            archived: record.archived,
            pinned: record.pinned,
            hidden: record.hidden,
            color,
            created_at: record.created_at.trim().to_string(),
        });
    }
    Ok(GroupsFile { version: 1, groups })
}

/// 群成员隐藏会话的身份键；必须与 session/agent 侧的 workspaceIdentity 口径一致。
pub fn group_member_workspace_identity(group_id: &str, teammate_id: &str) -> String {
    format!("mode-bot:group:{group_id}:member:{teammate_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_group() -> GroupRecord {
        GroupRecord {
            id: " g1 ".into(),
            name: " 发布小组 ".into(),
            admin_id: " t1 ".into(),
            member_ids: vec![" t1 ".into(), "t2".into(), "t2".into()],
            instructions: " 群规则 ".into(),
            memory: " 群记忆 ".into(),
            directory: Some("  D:\\ws\\channel  ".into()),
            responder_policy: Some(" admin-only ".into()),
            approval_override: Some(" auto ".into()),
            archived: true,
            pinned: false,
            hidden: false,
            color: Some("  blue  ".into()),
            created_at: " 2026-11-02T00:00:00Z ".into(),
        }
    }

    #[test]
    fn legacy_group_gets_channel_defaults() {
        let file: GroupsFile = serde_json::from_str(
            r#"{"version":1,"groups":[{"id":"g1","name":"Room","adminId":"t1","memberIds":["t1"]}]}"#,
        )
        .unwrap();
        let file = validate_groups_file(file).unwrap();
        assert_eq!(file.groups[0].directory, None);
        assert_eq!(
            file.groups[0].responder_policy.as_deref(),
            Some("mentions-first")
        );
        assert!(!file.groups[0].archived);
    }

    #[test]
    fn channel_fields_are_trimmed_and_policy_validated() {
        let file = GroupsFile {
            version: 1,
            groups: vec![valid_group()],
        };
        let normalized = validate_groups_file(file).expect("valid");
        let group = &normalized.groups[0];
        assert_eq!(group.directory.as_deref(), Some("D:\\ws\\channel"));
        assert_eq!(group.responder_policy.as_deref(), Some("admin-only"));
        assert!(group.archived);
        assert_eq!(group.color.as_deref(), Some("blue"));

        let mut bad = valid_group();
        bad.responder_policy = Some("everyone".into());
        assert!(validate_groups_file(GroupsFile {
            version: 1,
            groups: vec![bad],
        })
        .is_err());
    }

    #[test]
    fn normalizes_and_dedupes_members() {
        let file = GroupsFile {
            version: 1,
            groups: vec![valid_group()],
        };
        let normalized = validate_groups_file(file).expect("valid");
        let group = &normalized.groups[0];
        assert_eq!(group.id, "g1");
        assert_eq!(group.admin_id, "t1");
        assert_eq!(group.member_ids, vec!["t1", "t2"]);
        assert_eq!(group.memory, " 群记忆 ");
        assert_eq!(group.created_at, "2026-11-02T00:00:00Z");
    }

    #[test]
    fn rejects_admin_not_in_members() {
        let mut group = valid_group();
        group.admin_id = "t3".into();
        assert!(validate_groups_file(GroupsFile {
            version: 1,
            groups: vec![group],
        })
        .is_err());
    }

    #[test]
    fn rejects_empty_members_and_duplicate_group_ids() {
        let mut group = valid_group();
        group.member_ids = vec![];
        assert!(validate_groups_file(GroupsFile {
            version: 1,
            groups: vec![group],
        })
        .is_err());

        let first = valid_group();
        let mut second = valid_group();
        second.name = "另一个群".into();
        assert!(validate_groups_file(GroupsFile {
            version: 1,
            groups: vec![first, second],
        })
        .is_err());
    }

    #[test]
    fn rejects_unknown_version() {
        assert!(validate_groups_file(GroupsFile {
            version: 2,
            groups: Vec::new(),
        })
        .is_err());
    }

    #[test]
    fn identity_key_shape() {
        assert_eq!(
            group_member_workspace_identity("g1", "t1"),
            "mode-bot:group:g1:member:t1"
        );
    }
}
