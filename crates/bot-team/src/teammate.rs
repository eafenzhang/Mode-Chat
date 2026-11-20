//! teammate 元数据模型与校验。
//!
//! 与 renderer 的 `teammates/types.ts` 保持同一套形状与归一化规则：
//! camelCase 序列化、必填 id/name/directory、可选字段按 TS 的默认值补齐、
//! 字符串 trim。校验失败返回 `Err`，调用方拒绝写盘或隔离原文件。

use serde::{Deserialize, Serialize};

/// 单个 bot 的审批策略（level + 工具白名单）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalPolicy {
    pub level: String,
    #[serde(default)]
    pub allowlist: Vec<String>,
}

/// 单条 teammate 元数据（字段与 TS `TeammateRecord` 一一对应）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TeammateRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub description: String,
    pub directory: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_session_id: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub identity_sync: bool,
    /// 团队 Chief：频道默认管理员；组织层 v1。
    #[serde(default)]
    pub is_chief: bool,
    /// 私有 Bot：不能进频道/当 Chief/被导出。
    #[serde(default)]
    pub is_private: bool,
    /// Mode `ModelSelection` JSON；缺省 = 全局默认模型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<serde_json::Value>,
    /// 审批策略；缺省 = ask。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<ApprovalPolicy>,
    /// 启用的本地标准插件 id（W5）。
    #[serde(default)]
    pub plugin_ids: Vec<String>,
    /// 从侧栏隐藏（DS hidden 对齐）；设置 → Bot 页可恢复显示。
    #[serde(default)]
    pub hidden: bool,
    /// 主卡位（DS `mainBotId` 对齐）：与团队 Chief 解耦，只影响侧栏主卡展示。
    #[serde(default)]
    pub is_main: bool,
}

/// 落盘文件的顶层形状。version 恒为 1。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TeammatesFile {
    pub version: u32,
    #[serde(default)]
    pub teammates: Vec<TeammateRecord>,
}

impl TeammatesFile {
    pub fn empty() -> Self {
        Self {
            version: 1,
            teammates: Vec::new(),
        }
    }
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// 校验并归一化。规则与 TS `validateTeammatesFile` 对齐：
/// - version 必须为 1；
/// - id/name/directory trim 后非空；
/// - role 与 createdAt trim；description/instructions 原样保留；
/// - lastSessionId trim 后为空则丢弃；
/// - pinned/identitySync 按布尔值保留。
pub fn validate_teammates_file(file: TeammatesFile) -> Result<TeammatesFile, String> {
    if file.version != 1 {
        return Err(format!("teammates version 必须为 1，实收 {}", file.version));
    }
    let mut teammates = Vec::with_capacity(file.teammates.len());
    for (index, record) in file.teammates.into_iter().enumerate() {
        let id =
            non_empty_trimmed(&record.id).ok_or_else(|| format!("teammate[{index}] 的 id 为空"))?;
        let name = non_empty_trimmed(&record.name)
            .ok_or_else(|| format!("teammate[{index}] 的 name 为空"))?;
        let directory = non_empty_trimmed(&record.directory)
            .ok_or_else(|| format!("teammate[{index}] 的 directory 为空"))?;
        let model_selection = match record.model_selection {
            Some(value) => {
                let object = value
                    .as_object()
                    .ok_or_else(|| format!("teammate[{index}] 的 modelSelection 必须是对象"))?;
                let provider = object
                    .get("providerId")
                    .and_then(|item| item.as_str())
                    .map(str::trim)
                    .unwrap_or("");
                let model = object
                    .get("modelId")
                    .and_then(|item| item.as_str())
                    .map(str::trim)
                    .unwrap_or("");
                if provider.is_empty() || model.is_empty() {
                    return Err(format!(
                        "teammate[{index}] 的 modelSelection 缺少 providerId/modelId"
                    ));
                }
                Some(value)
            }
            None => None,
        };
        let approval_policy = match record.approval_policy {
            Some(policy) => {
                let level = policy.level.trim();
                if !matches!(level, "ask" | "auto" | "full") {
                    return Err(format!(
                        "teammate[{index}] 的 approvalPolicy.level 非法：{level}"
                    ));
                }
                let mut allowlist: Vec<String> = Vec::new();
                for item in policy.allowlist {
                    if let Some(item) = non_empty_trimmed(&item) {
                        if !allowlist.contains(&item) {
                            allowlist.push(item);
                        }
                    }
                }
                Some(ApprovalPolicy {
                    level: level.to_string(),
                    allowlist,
                })
            }
            None => None,
        };
        teammates.push(TeammateRecord {
            id,
            name,
            role: record.role.trim().to_string(),
            description: record.description,
            directory,
            created_at: record.created_at.trim().to_string(),
            last_session_id: record
                .last_session_id
                .as_deref()
                .and_then(non_empty_trimmed),
            pinned: record.pinned,
            instructions: record.instructions,
            identity_sync: record.identity_sync,
            is_chief: record.is_chief,
            is_private: record.is_private,
            model_selection,
            approval_policy,
            plugin_ids: {
                let mut ids: Vec<String> = Vec::new();
                for id in record.plugin_ids {
                    if let Some(id) = non_empty_trimmed(&id) {
                        if !ids.contains(&id) {
                            ids.push(id);
                        }
                    }
                }
                ids
            },
            hidden: record.hidden,
            is_main: record.is_main,
        });
    }
    Ok(TeammatesFile {
        version: 1,
        teammates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_record() -> TeammateRecord {
        TeammateRecord {
            id: " t1 ".into(),
            name: " 娜娜 ".into(),
            role: " 发布 ".into(),
            description: "负责构建".into(),
            directory: " D:/work ".into(),
            created_at: " 2026-11-02T00:00:00Z ".into(),
            last_session_id: Some("  ".into()),
            pinned: true,
            instructions: " 先跑测试 ".into(),
            identity_sync: true,
            is_chief: false,
            is_private: false,
            model_selection: None,
            approval_policy: None,
            plugin_ids: Vec::new(),
            hidden: false,
            is_main: false,
        }
    }

    #[test]
    fn legacy_record_gets_no_model_and_no_approval_policy() {
        let raw = serde_json::json!({
            "version": 1,
            "teammates": [{ "id": "t1", "name": "娜娜", "directory": "D:/work" }]
        });
        let file: TeammatesFile = serde_json::from_value(raw).expect("deserialize");
        let normalized = validate_teammates_file(file).expect("valid");
        assert_eq!(normalized.teammates[0].model_selection, None);
        assert_eq!(normalized.teammates[0].approval_policy, None);
    }

    #[test]
    fn model_and_approval_round_trip_and_validate() {
        let mut record = valid_record();
        record.model_selection = Some(serde_json::json!({
            "providerId": "anthropic",
            "modelId": "claude-sonnet"
        }));
        record.approval_policy = Some(ApprovalPolicy {
            level: " auto ".into(),
            allowlist: vec![" read ".into(), "read".into(), " ".into()],
        });
        let normalized = validate_teammates_file(TeammatesFile {
            version: 1,
            teammates: vec![record],
        })
        .expect("valid");
        let record = &normalized.teammates[0];
        assert_eq!(
            record.model_selection.as_ref().unwrap()["providerId"],
            serde_json::json!("anthropic")
        );
        let policy = record.approval_policy.as_ref().unwrap();
        assert_eq!(policy.level, "auto");
        assert_eq!(policy.allowlist, vec!["read".to_string()]);
    }

    #[test]
    fn rejects_bad_model_or_approval_policy() {
        let mut bad_model = valid_record();
        bad_model.model_selection = Some(serde_json::json!({ "providerId": "" }));
        assert!(validate_teammates_file(TeammatesFile {
            version: 1,
            teammates: vec![bad_model],
        })
        .is_err());

        let mut bad_policy = valid_record();
        bad_policy.approval_policy = Some(ApprovalPolicy {
            level: "everyone".into(),
            allowlist: Vec::new(),
        });
        assert!(validate_teammates_file(TeammatesFile {
            version: 1,
            teammates: vec![bad_policy],
        })
        .is_err());
    }

    #[test]
    fn normalizes_trimmed_fields_and_drops_blank_session_id() {
        let file = TeammatesFile {
            version: 1,
            teammates: vec![valid_record()],
        };
        let normalized = validate_teammates_file(file).expect("valid");
        let record = &normalized.teammates[0];
        assert_eq!(record.id, "t1");
        assert_eq!(record.name, "娜娜");
        assert_eq!(record.role, "发布");
        assert_eq!(record.directory, "D:/work");
        assert_eq!(record.created_at, "2026-11-02T00:00:00Z");
        assert_eq!(record.last_session_id, None);
        assert_eq!(record.instructions, " 先跑测试 ");
    }

    #[test]
    fn rejects_empty_identity_fields() {
        let mut record = valid_record();
        record.id = "  ".into();
        assert!(validate_teammates_file(TeammatesFile {
            version: 1,
            teammates: vec![record],
        })
        .is_err());

        let mut record = valid_record();
        record.name = "".into();
        assert!(validate_teammates_file(TeammatesFile {
            version: 1,
            teammates: vec![record],
        })
        .is_err());

        let mut record = valid_record();
        record.directory = "\t".into();
        assert!(validate_teammates_file(TeammatesFile {
            version: 1,
            teammates: vec![record],
        })
        .is_err());
    }

    #[test]
    fn rejects_unknown_version() {
        assert!(validate_teammates_file(TeammatesFile {
            version: 2,
            teammates: Vec::new(),
        })
        .is_err());
    }

    #[test]
    fn accepts_missing_optional_fields_via_defaults() {
        let raw = serde_json::json!({
            "version": 1,
            "teammates": [{
                "id": "t1",
                "name": "娜娜",
                "directory": "D:/work"
            }]
        });
        let file: TeammatesFile = serde_json::from_value(raw).expect("deserialize");
        let normalized = validate_teammates_file(file).expect("valid");
        let record = &normalized.teammates[0];
        assert_eq!(record.role, "");
        assert_eq!(record.description, "");
        assert_eq!(record.created_at, "");
        assert_eq!(record.last_session_id, None);
        assert!(!record.pinned);
        assert!(!record.identity_sync);
    }
}
