//! Portable team template (bots + channels), safe to share.
//!
//! No absolute paths, secrets, transcripts or memory contents. Imported records
//! always go through the existing CRUD validation and get new ids.

use serde::{Deserialize, Serialize};

use crate::group::GroupRecord;
use crate::teammate::{ApprovalPolicy, TeammateRecord};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TeamBot {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<serde_json::Value>,
    /// Exported for completeness; import resets to ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<ApprovalPolicy>,
    /// Plugin ids this bot enabled; import keeps only installed plugins.
    #[serde(default)]
    pub plugin_ids: Vec<String>,
    /// Team Chief (organizational role).
    #[serde(default)]
    pub is_chief: bool,
    /// Private bots are not shareable; export skips them and import rejects them.
    #[serde(default)]
    pub private: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TeamChannel {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub memory: String,
    #[serde(default)]
    pub responder_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory_name: Option<String>,
    pub admin_key: String,
    #[serde(default)]
    pub member_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TeamFile {
    pub schema_version: u32,
    pub exported_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_name: Option<String>,
    #[serde(default)]
    pub bots: Vec<TeamBot>,
    #[serde(default)]
    pub channels: Vec<TeamChannel>,
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn directory_name(path: &str) -> Option<String> {    let value = path.trim().trim_end_matches(['\\', '/']);
    if value.is_empty() {
        return None;
    }
    let name = value.rsplit(['\\', '/']).next()?.trim();
    if name.is_empty() || name == "." || name == ".." {
        None
    } else {
        Some(name.to_string())
    }
}

/// Build an export from the live repos. Directory paths are reduced to bare names.
pub fn build_team_file(
    bots: &[TeammateRecord],
    channels: &[GroupRecord],
    exported_at: String,
) -> TeamFile {
    let private_keys: std::collections::HashSet<String> = bots
        .iter()
        .filter(|bot| bot.is_private)
        .map(|bot| bot.id.clone())
        .collect();
    TeamFile {
        schema_version: 2,
        exported_at,
        team_name: None,
        bots: bots
            .iter()
            .filter(|bot| !bot.is_private)
            .map(|bot| TeamBot {
                key: bot.id.clone(),
                name: bot.name.clone(),
                role: bot.role.clone(),
                description: bot.description.clone(),
                instructions: bot.instructions.clone(),
                directory_name: directory_name(&bot.directory),
                model_selection: bot.model_selection.clone(),
                approval_policy: bot.approval_policy.clone(),
                plugin_ids: bot.plugin_ids.clone(),
                is_chief: bot.is_chief,
                private: false,
            })
            .collect(),
        channels: channels
            .iter()
            .filter(|channel| {
                !private_keys.contains(&channel.admin_id)
                    && !channel
                        .member_ids
                        .iter()
                        .any(|member| private_keys.contains(member))
            })
            .map(|channel| TeamChannel {
                key: channel.id.clone(),
                name: channel.name.clone(),
                instructions: channel.instructions.clone(),
                memory: channel.memory.clone(),
                responder_policy: channel.responder_policy.clone(),
                approval_override: channel.approval_override.clone(),
                directory_name: channel.directory.as_deref().and_then(directory_name),
                admin_key: channel.admin_id.clone(),
                member_keys: channel.member_ids.clone(),
            })
            .collect(),
    }
}

fn valid_directory_name(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed != "."
        && trimmed != ".."
        && !trimmed.contains(['/', '\\', ':'])
}

/// Validate and normalize a team file before the UI creates anything.
pub fn validate_team_file(file: TeamFile) -> Result<TeamFile, String> {
    if file.schema_version != 1 && file.schema_version != 2 {
        return Err(format!(
            "team schemaVersion 只支持 1 或 2，实收 {}",
            file.schema_version
        ));
    }
    let mut bot_keys = std::collections::HashSet::new();
    let mut bots = Vec::with_capacity(file.bots.len());
    for (index, bot) in file.bots.into_iter().enumerate() {
        let key = bot.key.trim().to_string();
        if key.is_empty() {
            return Err(format!("bots[{index}] 的 key 为空"));
        }
        if !bot_keys.insert(key.clone()) {
            return Err(format!("bot key 重复：{key}"));
        }
        let name = bot.name.trim().to_string();
        if name.is_empty() {
            return Err(format!("bots[{index}] 的 name 为空"));
        }
        let directory_name = match bot.directory_name.map(|value| value.trim().to_string()) {
            Some(value) if value.is_empty() => None,
            Some(value) if valid_directory_name(&value) => Some(value),
            Some(value) => {
                return Err(format!("bots[{index}] 的 directoryName 非法：{value}"))
            }
            None => None,
        };
        if let Some(policy) = &bot.approval_policy {
            if !matches!(policy.level.trim(), "ask" | "auto" | "full") {
                return Err(format!("bots[{index}] 的 approvalPolicy.level 非法"));
            }
        }
        if bot.private {
            return Err(format!("bots[{index}] 是私有 Bot，不能进团队模板"));
        }
        bots.push(TeamBot {
            key,
            name,
            role: bot.role.trim().to_string(),
            description: bot.description,
            instructions: bot.instructions,
            directory_name,
            model_selection: bot.model_selection,
            approval_policy: bot.approval_policy,
            plugin_ids: {
                let mut ids: Vec<String> = Vec::new();
                for id in bot.plugin_ids {
                    if let Some(id) = non_empty_trimmed(&id) {
                        if !ids.contains(&id) {
                            ids.push(id);
                        }
                    }
                }
                ids
            },
            is_chief: bot.is_chief,
            private: false,
        });
    }

    let mut channel_keys = std::collections::HashSet::new();
    let mut channels = Vec::with_capacity(file.channels.len());
    for (index, channel) in file.channels.into_iter().enumerate() {
        let key = channel.key.trim().to_string();
        if key.is_empty() {
            return Err(format!("channels[{index}] 的 key 为空"));
        }
        if !channel_keys.insert(key.clone()) {
            return Err(format!("channel key 重复：{key}"));
        }
        let name = channel.name.trim().to_string();
        if name.is_empty() {
            return Err(format!("channels[{index}] 的 name 为空"));
        }
        let admin_key = channel.admin_key.trim().to_string();
        if !bot_keys.contains(&admin_key) {
            return Err(format!("channels[{index}] 的 adminKey 不在 bots 里"));
        }
        let mut member_keys: Vec<String> = Vec::new();
        for member in channel.member_keys {
            let member = member.trim().to_string();
            if !bot_keys.contains(&member) {
                return Err(format!("channels[{index}] 的 memberKeys 含未知 bot：{member}"));
            }
            if !member_keys.contains(&member) {
                member_keys.push(member);
            }
        }
        if member_keys.is_empty() {
            return Err(format!("channels[{index}] 的 memberKeys 为空"));
        }
        if !member_keys.contains(&admin_key) {
            return Err(format!("channels[{index}] 的 adminKey 必须在 memberKeys 里"));
        }
        let responder_policy = match channel.responder_policy.as_deref().map(str::trim) {
            None | Some("") => Some("mentions-first".to_string()),
            Some("mentions-first") => Some("mentions-first".to_string()),
            Some("admin-only") => Some("admin-only".to_string()),
            Some(other) => {
                return Err(format!(
                    "channels[{index}] 的 responderPolicy 非法：{other}"
                ))
            }
        };
        let approval_override = match channel.approval_override.as_deref().map(str::trim) {
            None | Some("") | Some("inherit") => None,
            Some("ask") => Some("ask".to_string()),
            Some("auto") => Some("auto".to_string()),
            Some("full") => Some("full".to_string()),
            Some(other) => {
                return Err(format!(
                    "channels[{index}] 的 approvalOverride 非法：{other}"
                ))
            }
        };
        let directory_name = match channel.directory_name.map(|value| value.trim().to_string()) {
            Some(value) if value.is_empty() => None,
            Some(value) if valid_directory_name(&value) => Some(value),
            Some(value) => {
                return Err(format!("channels[{index}] 的 directoryName 非法：{value}"))
            }
            None => None,
        };
        channels.push(TeamChannel {
            key,
            name,
            instructions: channel.instructions,
            memory: channel.memory,
            responder_policy,
            approval_override,
            directory_name,
            admin_key,
            member_keys,
        });
    }

    Ok(TeamFile {
        schema_version: 2,
        exported_at: file.exported_at.trim().to_string(),
        team_name: file
            .team_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        bots,
        channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teammate::ApprovalPolicy;

    fn bot(id: &str) -> TeammateRecord {
        TeammateRecord {
            id: id.into(),
            name: format!("Bot {id}"),
            role: "dev".into(),
            description: String::new(),
            directory: format!("D:\\ws\\{id}"),
            created_at: String::new(),
            last_session_id: None,
            pinned: false,
            instructions: String::new(),
            identity_sync: false,
            is_chief: false,
            is_private: false,
            model_selection: None,
            approval_policy: None,
            plugin_ids: Vec::new(),
            hidden: false,
            is_main: false,
        }
    }

    fn channel(id: &str) -> GroupRecord {
        GroupRecord {
            id: id.into(),
            name: format!("Channel {id}"),
            admin_id: "b1".into(),
            member_ids: vec!["b1".into(), "b2".into()],
            instructions: String::new(),
            memory: String::new(),
            directory: Some("D:\\ws\\team".into()),
            responder_policy: Some("mentions-first".into()),
            approval_override: None,
            archived: false,
            pinned: false,
            hidden: false,
            color: None,
            created_at: String::new(),
        }
    }

    #[test]
    fn export_reduces_paths_to_basenames_and_keeps_refs() {
        let file = build_team_file(
            &[bot("b1"), bot("b2")],
            &[channel("c1")],
            "now".into(),
        );
        assert_eq!(file.bots[0].directory_name.as_deref(), Some("b1"));
        assert_eq!(file.channels[0].directory_name.as_deref(), Some("team"));
        assert_eq!(file.channels[0].admin_key, "b1");
        assert_eq!(file.channels[0].member_keys, vec!["b1", "b2"]);
    }

    #[test]
    fn validates_keys_refs_and_paths() {
        let mut file = build_team_file(&[bot("b1")], &[], "now".into());
        assert!(validate_team_file(file.clone()).is_ok());

        let mut duplicate = file.clone();
        duplicate.bots.push(duplicate.bots[0].clone());
        assert!(validate_team_file(duplicate).is_err());

        let mut bad_ref = file.clone();
        bad_ref.channels.push(TeamChannel {
            key: "c1".into(),
            name: "C".into(),
            instructions: String::new(),
            memory: String::new(),
            responder_policy: None,
            approval_override: None,
            directory_name: None,
            admin_key: "missing".into(),
            member_keys: vec!["missing".into()],
        });
        assert!(validate_team_file(bad_ref).is_err());

        let mut bad_path = file.clone();
        bad_path.bots[0].directory_name = Some("../escape".into());
        assert!(validate_team_file(bad_path).is_err());

        file.bots[0].approval_policy = Some(ApprovalPolicy {
            level: "full".into(),
            allowlist: Vec::new(),
        });
        assert!(validate_team_file(file).is_ok());
    }

    #[test]
    fn exports_v2_and_skips_private_bots_and_their_channels() {
        let mut private = bot("b2");
        private.is_private = true;
        let file = build_team_file(&[bot("b1"), private], &[channel("c1")], "now".into());
        assert_eq!(file.schema_version, 2);
        assert_eq!(file.bots.len(), 1, "private bot must not be exported");
        assert!(file.channels.is_empty(), "channel referencing private bot must be skipped");
    }

    #[test]
    fn accepts_v1_and_migrates_to_v2() {
        let v1: TeamFile = serde_json::from_str(
            r#"{"schemaVersion":1,"exportedAt":"now","bots":[{"key":"b1","name":"B1"}],"channels":[]}"#,
        )
        .expect("deserialize v1");
        let normalized = validate_team_file(v1).expect("v1 remains importable");
        assert_eq!(normalized.schema_version, 2);
        assert_eq!(normalized.bots[0].key, "b1");
    }

    #[test]
    fn rejects_unknown_schema_and_private_flag_in_template() {
        let mut file = build_team_file(&[bot("b1")], &[], "now".into());
        file.schema_version = 3;
        assert!(validate_team_file(file).is_err());

        let mut file = build_team_file(&[bot("b1")], &[], "now".into());
        file.bots[0].private = true;
        assert!(validate_team_file(file).is_err());
    }
}
