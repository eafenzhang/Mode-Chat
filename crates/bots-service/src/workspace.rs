//! Bot 工作区解析（`workspaceHelpers.ts` + `@mode/shared` 的完整移植）。
//!
//! 工作区身份：`identity || path`（与 `getBotWorkspaceKey` 同义）；`allowedWorkspaces`
//! 空数组或含 `*` 表示全部；命令 `/workspace <值>` 支持序号（1 起）、id、label、path。

use crate::BotConfig;

/// 全部工作区的通配标记（`ALL_BOT_WORKSPACES`）。
pub const ALL_BOT_WORKSPACES: &str = "*";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRef {
    pub id: String,
    pub label: String,
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
}

pub fn workspace_key(workspace_path: &str, workspace_identity: Option<&str>) -> String {
    workspace_identity
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| workspace_path.to_string())
}

pub fn workspace_label(workspace_path: &str) -> String {
    workspace_path
        .split(['\\', '/'])
        .rfind(|segment| !segment.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| workspace_path.to_string())
}

pub fn create_workspace_ref(workspace_path: &str, workspace_identity: Option<&str>) -> WorkspaceRef {
    let identity = workspace_identity
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    WorkspaceRef {
        id: workspace_key(workspace_path, identity.as_deref()),
        label: workspace_label(workspace_path),
        workspace_path: workspace_path.to_string(),
        workspace_identity: identity,
    }
}

fn is_all_workspaces_allowed(allowed: &[String]) -> bool {
    allowed.is_empty() || allowed.iter().any(|item| item == ALL_BOT_WORKSPACES)
}

/// 空数组/含 `*` 归一化为 `["*"]`；其余 trim、去空、去重。
pub fn normalize_allowed_workspaces(allowed: &[String]) -> Vec<String> {
    let trimmed: Vec<String> = allowed
        .iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();
    if is_all_workspaces_allowed(&trimmed) {
        return vec![ALL_BOT_WORKSPACES.to_string()];
    }
    let mut unique: Vec<String> = Vec::new();
    for item in trimmed {
        if !unique.contains(&item) {
            unique.push(item);
        }
    }
    unique
}

pub fn is_workspace_allowed(workspace_id: &str, allowed: &[String]) -> bool {
    is_all_workspaces_allowed(allowed) || allowed.iter().any(|item| item == workspace_id)
}

pub fn filter_allowed_workspaces<'a>(
    workspaces: &'a [WorkspaceRef],
    allowed: &[String],
) -> Vec<&'a WorkspaceRef> {
    if is_all_workspaces_allowed(allowed) {
        return workspaces.iter().collect();
    }
    workspaces
        .iter()
        .filter(|workspace| allowed.iter().any(|item| item == &workspace.id))
        .collect()
}

/// id 归一化：同 path 且有唯一 identity 候选时优先 identity key，否则保留原值。
fn resolve_canonical_workspace_id(
    value: Option<&str>,
    workspaces: &[WorkspaceRef],
) -> Option<String> {
    let trimmed = value.map(str::trim).filter(|value| !value.is_empty())?;
    let same_path: Vec<&WorkspaceRef> = workspaces
        .iter()
        .filter(|workspace| workspace.workspace_path == trimmed)
        .collect();
    if let Some(exact) = workspaces.iter().find(|workspace| workspace.id == trimmed) {
        let identity_candidates: Vec<&&WorkspaceRef> = same_path
            .iter()
            .filter(|workspace| workspace.workspace_identity.is_some())
            .collect();
        if exact.workspace_identity.is_none() && identity_candidates.len() == 1 {
            return Some(identity_candidates[0].id.clone());
        }
        return Some(exact.id.clone());
    }
    if same_path.len() == 1 {
        return Some(same_path[0].id.clone());
    }
    Some(trimmed.to_string())
}

pub fn normalize_configured_allowed_workspaces(
    allowed: &[String],
    workspaces: &[WorkspaceRef],
) -> Vec<String> {
    let normalized = normalize_allowed_workspaces(allowed);
    if is_all_workspaces_allowed(&normalized) {
        return vec![ALL_BOT_WORKSPACES.to_string()];
    }
    let mut unique: Vec<String> = Vec::new();
    for item in &normalized {
        let resolved = resolve_canonical_workspace_id(Some(item), workspaces)
            .unwrap_or_else(|| item.clone());
        if !unique.contains(&resolved) {
            unique.push(resolved);
        }
    }
    unique
}

/// 存量绑定 key 升级：path-only 旧远端绑定升到 identity key；本地 key 原样保留。
pub fn resolve_legacy_binding_workspace_key(key: &str, workspaces: &[WorkspaceRef]) -> String {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return key.to_string();
    }
    if let Some(exact) = workspaces.iter().find(|workspace| workspace.id == trimmed) {
        return exact.id.clone();
    }
    let candidates: Vec<&WorkspaceRef> = workspaces
        .iter()
        .filter(|workspace| {
            workspace.workspace_path == trimmed && workspace.workspace_identity.is_some()
        })
        .collect();
    if candidates.len() == 1 {
        candidates[0].id.clone()
    } else {
        key.to_string()
    }
}

pub fn first_allowed_workspace<'a>(
    workspaces: &'a [WorkspaceRef],
    bot: &BotConfig,
) -> Option<&'a WorkspaceRef> {
    filter_allowed_workspaces(workspaces, &bot.allowed_workspaces)
        .into_iter()
        .next()
}

fn normalize_text(value: &str) -> String {
    value.trim().to_lowercase()
}

/// `/workspace <值>`：序号（1 起）→ id/label/path/key 精确匹配（大小写不敏感）。
pub fn resolve_workspace_by_value<'a>(
    workspaces: &'a [WorkspaceRef],
    value: &str,
    allowed_ids: &[String],
) -> Option<&'a WorkspaceRef> {
    let trimmed = value.trim();
    let filtered = filter_allowed_workspaces(workspaces, allowed_ids);
    if let Ok(index) = trimmed.parse::<usize>() {
        if index > 0 {
            return filtered.get(index - 1).copied();
        }
    }
    let normalized = normalize_text(trimmed);
    filtered.into_iter().find(|workspace| {
        normalize_text(&workspace.id) == normalized
            || normalize_text(&workspace.label) == normalized
            || workspace.workspace_path == trimmed
            || workspace_key(&workspace.workspace_path, workspace.workspace_identity.as_deref())
                == trimmed
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspaces() -> Vec<WorkspaceRef> {
        vec![
            create_workspace_ref("D:/ws-a", None),
            create_workspace_ref("D:/ws-b", None),
            create_workspace_ref("C:/remote", Some("remote-identity")),
        ]
    }

    fn bot_with_allowed(allowed: &[&str]) -> BotConfig {
        let mut bot = crate::BotConfig {
            id: "b1".to_string(),
            name: "bot".to_string(),
            provider: crate::BotProvider::Weixin,
            enabled: true,
            credential_ref: None,
            webhook_secret_ref: None,
            webhook_url: None,
            webhook_auth_header_name: None,
            feishu_app_id: None,
            provider_user_id: None,
            provider_self_id: None,
            private_chat_mode: None,
            access_mode: None,
            blocked_users: None,
            blocked_groups: None,
            user_registry: None,
            display_name: None,
            wecom_bot_id: None,
            dingtalk_client_id: None,
            dingtalk_use_ai_card: None,
            dingtalk_card_template_id: None,
            allowed_users: None,
            allowed_workspaces: Vec::new(),
            group_chat: None,
            delivery_mode: None,
            heartbeat: None,
            allowed_commands: None,
            current_options: None,
            reply_mode: crate::BotReplyMode::AssistantChanges,
        };
        bot.allowed_workspaces = allowed.iter().map(|item| item.to_string()).collect();
        bot
    }

    #[test]
    fn key_label_and_creation() {
        assert_eq!(workspace_key("D:/ws", None), "D:/ws");
        assert_eq!(workspace_key("D:/ws", Some(" id ")), "id");
        assert_eq!(workspace_label("D:/a/b"), "b");
        let reference = create_workspace_ref("D:/a/b", Some("id"));
        assert_eq!(reference.id, "id");
        assert_eq!(reference.label, "b");
    }

    #[test]
    fn allowed_workspaces_normalize_and_filter() {
        assert_eq!(normalize_allowed_workspaces(&[]), vec!["*".to_string()]);
        assert_eq!(
            normalize_allowed_workspaces(&[" * ".to_string()]),
            vec!["*".to_string()]
        );
        assert_eq!(
            normalize_allowed_workspaces(&[
                " a ".to_string(),
                "a".to_string(),
                " ".to_string(),
                "b".to_string()
            ]),
            vec!["a".to_string(), "b".to_string()]
        );
        let all = workspaces();
        assert!(filter_allowed_workspaces(&all, &[]).len() == 3);
        let only_a = filter_allowed_workspaces(&all, &["D:/ws-a".to_string()]);
        assert_eq!(only_a.len(), 1);
        assert_eq!(only_a[0].id, "D:/ws-a");
    }

    #[test]
    fn canonical_and_legacy_keys() {
        let all = workspaces();
        // path-only 且唯一 identity 候选 → 升到 identity key。
        assert_eq!(
            normalize_configured_allowed_workspaces(&["C:/remote".to_string()], &all),
            vec!["remote-identity".to_string()]
        );
        assert_eq!(
            resolve_legacy_binding_workspace_key("C:/remote", &all),
            "remote-identity"
        );
        // 本地键不升级。
        assert_eq!(resolve_legacy_binding_workspace_key("D:/ws-a", &all), "D:/ws-a");
    }

    #[test]
    fn resolve_by_index_label_and_path() {
        let all = workspaces();
        assert_eq!(
            resolve_workspace_by_value(&all, "2", &[]).unwrap().id,
            "D:/ws-b"
        );
        assert_eq!(
            resolve_workspace_by_value(&all, "ws-a", &[]).unwrap().id,
            "D:/ws-a"
        );
        assert_eq!(
            resolve_workspace_by_value(&all, "C:/remote", &[]).unwrap().id,
            "remote-identity"
        );
        // 序号只在过滤后的允许列表内定位。
        assert_eq!(
            resolve_workspace_by_value(&all, "1", &["D:/ws-b".to_string()])
                .unwrap()
                .id,
            "D:/ws-b"
        );
        assert!(resolve_workspace_by_value(&all, "9", &[]).is_none());
    }

    #[test]
    fn first_allowed_workspace_respects_bot_scope() {
        let all = workspaces();
        let bot = bot_with_allowed(&["D:/ws-b"]);
        assert_eq!(first_allowed_workspace(&all, &bot).unwrap().id, "D:/ws-b");
        let bot_all = bot_with_allowed(&[]);
        assert_eq!(first_allowed_workspace(&all, &bot_all).unwrap().id, "D:/ws-a");
        let bot_none = bot_with_allowed(&["missing"]);
        assert!(first_allowed_workspace(&all, &bot_none).is_none());
    }
}
