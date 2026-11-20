//! 任务绑定与 UI 焦点收敛（`bindBotToTask`/`unbindBotFromTask`/`listBotTaskBindings`/
//! `notifyUiSessionFocus` 的状态语义）。
//!
//! - 绑定：会话进入 `mode=task` + activeTaskId，并把工作区字段一并换成目标任务的工作区；
//!   目标工作区必须在该 bot 授权范围内（`*` 或精确 id）；同一会话已有其它任务 → Busy。
//! - 解绑：回到草稿（清 activeTaskId/审批/队列），保留工作区。
//! - UI 焦点：工作区绑定的 bot 下，**尚未绑定任务**的草稿会话跟随当前工作区；
//!   已绑定任务的会话不被焦点夺走。

use serde_json::Value;

use crate::conversation::{write_draft_context, write_task_context_for_workspace};
use crate::workspace::{workspace_key, WorkspaceRef};
use crate::{BotConfig, BotsConfigFile, BotsRepo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskBinding {
    pub bot_id: String,
    pub provider: crate::BotProvider,
    pub conversation_key: String,
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub task_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindTaskResult {
    Ok,
    Missing,
    Workspace,
    Conversation,
    Busy,
}

pub fn list_task_bindings(state: &Value, config: &BotsConfigFile) -> Vec<TaskBinding> {
    let mut result = Vec::new();
    let Some(bots) = state.get("bots").and_then(Value::as_object) else {
        return result;
    };
    for (bot_id, channel) in bots {
        let Some(bot) = config.bots.iter().find(|bot| &bot.id == bot_id && bot.enabled) else {
            continue;
        };
        let Some(conversations) = channel.get("conversations").and_then(Value::as_object) else {
            continue;
        };
        for (key, context) in conversations {
            if context.get("mode").and_then(Value::as_str) != Some("task") {
                continue;
            }
            let Some(task_id) = context.get("activeTaskId").and_then(Value::as_str) else {
                continue;
            };
            result.push(TaskBinding {
                bot_id: bot.id.clone(),
                provider: bot.provider,
                conversation_key: key.clone(),
                workspace_path: context
                    .get("workspacePath")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                workspace_identity: context
                    .get("workspaceIdentity")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                task_id: task_id.to_string(),
            });
        }
    }
    result
}

fn workspace_allowed(bot: &BotConfig, workspace: &WorkspaceRef) -> bool {
    crate::workspace::is_workspace_allowed(&workspace.id, &bot.allowed_workspaces)
}

fn conversation_context(state: &Value, bot_id: &str, key: &str) -> Option<Value> {
    state
        .get("bots")
        .and_then(|bots| bots.get(bot_id))
        .and_then(|channel| channel.get("conversations"))
        .and_then(|conversations| conversations.get(key))
        .cloned()
}

/// 绑定任务到会话；`conversation_key` 省略时要求该 bot 恰好只有一个会话。
pub fn bind_bot_to_task(
    repo: &BotsRepo,
    bot: &BotConfig,
    conversation_key: Option<&str>,
    workspace: &WorkspaceRef,
    task_id: &str,
) -> Result<BindTaskResult, String> {
    if !bot.enabled {
        return Ok(BindTaskResult::Missing);
    }
    if !workspace_allowed(bot, workspace) {
        return Ok(BindTaskResult::Workspace);
    }
    let state = repo.read_state()?;
    let key = match conversation_key.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => {
            if conversation_context(&state, &bot.id, key).is_none() {
                return Ok(BindTaskResult::Conversation);
            }
            key.to_string()
        }
        None => {
            let conversations = state
                .get("bots")
                .and_then(|bots| bots.get(&bot.id))
                .and_then(|channel| channel.get("conversations"))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let mut keys: Vec<(String, i64)> = conversations
                .iter()
                .map(|(key, context)| {
                    (
                        key.clone(),
                        context
                            .get("updatedAt")
                            .and_then(Value::as_i64)
                            .unwrap_or(0),
                    )
                })
                .collect();
            keys.sort_by_key(|(_, updated)| std::cmp::Reverse(*updated));
            match keys.len() {
                0 => return Ok(BindTaskResult::Conversation),
                1 => keys[0].0.clone(),
                _ => return Ok(BindTaskResult::Busy),
            }
        }
    };
    if let Some(context) = conversation_context(&state, &bot.id, &key) {
        if let Some(existing) = context.get("activeTaskId").and_then(Value::as_str) {
            if existing != task_id {
                return Ok(BindTaskResult::Busy);
            }
        }
    }
    write_task_context_for_workspace(repo, &bot.id, &key, workspace, task_id)?;
    Ok(BindTaskResult::Ok)
}

/// 解绑任务：回到草稿，保留工作区。
pub fn unbind_bot_from_task(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
) -> Result<(), String> {
    write_draft_context(repo, bot_id, conversation_key, None)?;
    Ok(())
}

/// UI 焦点：工作区绑定的 bot 的草稿会话跟随当前工作区（已绑定任务的不动）。
pub fn apply_ui_focus(
    repo: &BotsRepo,
    config: &BotsConfigFile,
    bound_bot_ids: &[String],
    workspace: &WorkspaceRef,
) -> Result<usize, String> {
    let state = repo.read_state()?;
    let mut updates: Vec<(String, String)> = Vec::new();
    for bot_id in bound_bot_ids {
        if !config
            .bots
            .iter()
            .any(|bot| &bot.id == bot_id && bot.enabled)
        {
            continue;
        }
        let Some(conversations) = state
            .get("bots")
            .and_then(|bots| bots.get(bot_id))
            .and_then(|channel| channel.get("conversations"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (key, context) in conversations {
            if context.get("activeTaskId").and_then(Value::as_str).is_some() {
                continue;
            }
            let current_key = workspace_key(
                context
                    .get("workspacePath")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                context.get("workspaceIdentity").and_then(Value::as_str),
            );
            if current_key != workspace.id {
                updates.push((bot_id.clone(), key.clone()));
            }
        }
    }
    if updates.is_empty() {
        return Ok(0);
    }
    let count = updates.len();
    repo.mutate_state(|state| {
        for (bot_id, key) in &updates {
            let Some(context) = state
                .get_mut("bots")
                .and_then(|bots| bots.get_mut(bot_id))
                .and_then(|channel| channel.get_mut("conversations"))
                .and_then(Value::as_object_mut)
                .and_then(|conversations| conversations.get_mut(key))
                .and_then(Value::as_object_mut)
            else {
                continue;
            };
            context.insert(
                "workspacePath".into(),
                Value::String(workspace.workspace_path.clone()),
            );
            context.insert("workspaceId".into(), Value::String(workspace.id.clone()));
            match workspace.workspace_identity.as_deref() {
                Some(identity) => {
                    context.insert("workspaceIdentity".into(), Value::String(identity.to_string()));
                }
                None => {
                    context.remove("workspaceIdentity");
                }
            }
        }
        Ok(())
    })?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::create_workspace_ref;
    use std::path::PathBuf;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tasks-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn bot() -> BotConfig {
        BotConfig {
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
            allowed_workspaces: vec!["D:/ws".to_string()],
            group_chat: None,
            delivery_mode: None,
            heartbeat: None,
            allowed_commands: None,
            current_options: None,
            reply_mode: crate::BotReplyMode::AssistantChanges,
        }
    }

    fn seed(repo: &BotsRepo) {
        repo.mutate_state(|state| {
            state["bots"]["b1"]["conversations"]["private:u1"] = serde_json::json!({
                "workspacePath": "D:/ws",
                "mode": "draft",
                "updatedAt": 1
            });
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn bind_list_unbind_roundtrip() {
        let base = sandbox("bind");
        let repo = BotsRepo::new(&base);
        seed(&repo);
        let workspace = create_workspace_ref("D:/ws", None);
        assert_eq!(
            bind_bot_to_task(&repo, &bot(), Some("private:u1"), &workspace, "task-1").unwrap(),
            BindTaskResult::Ok
        );
        let state = repo.read_state().unwrap();
        let bindings = list_task_bindings(
            &state,
            &crate::BotsConfigFile {
                version: 3,
                bots: vec![bot()],
            },
        );
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].task_id, "task-1");
        assert_eq!(bindings[0].workspace_path, "D:/ws");
        // 已有其它任务 → busy。
        assert_eq!(
            bind_bot_to_task(&repo, &bot(), Some("private:u1"), &workspace, "task-2").unwrap(),
            BindTaskResult::Busy
        );
        // 未授权工作区 → workspace。
        assert_eq!(
            bind_bot_to_task(
                &repo,
                &bot(),
                Some("private:u1"),
                &create_workspace_ref("D:/other", None),
                "task-1"
            )
            .unwrap(),
            BindTaskResult::Workspace
        );
        unbind_bot_from_task(&repo, "b1", "private:u1").unwrap();
        let state = repo.read_state().unwrap();
        assert!(
            list_task_bindings(
                &state,
                &crate::BotsConfigFile {
                    version: 3,
                    bots: vec![bot()],
                }
            )
            .is_empty()
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn ui_focus_moves_only_draft_conversations() {
        let base = sandbox("focus");
        let repo = BotsRepo::new(&base);
        seed(&repo);
        repo.mutate_state(|state| {
            state["bots"]["b1"]["conversations"]["private:u2"] = serde_json::json!({
                "workspacePath": "D:/ws",
                "mode": "task",
                "activeTaskId": "task-9"
            });
            Ok(())
        })
        .unwrap();
        let updated = apply_ui_focus(
            &repo,
            &crate::BotsConfigFile {
                version: 3,
                bots: vec![bot()],
            },
            &["b1".to_string()],
            &create_workspace_ref("D:/ws2", None),
        )
        .unwrap();
        assert_eq!(updated, 1);
        let state = repo.read_state().unwrap();
        assert_eq!(
            state["bots"]["b1"]["conversations"]["private:u1"]["workspacePath"],
            "D:/ws2"
        );
        // 已绑定任务的会话不被焦点夺走。
        assert_eq!(
            state["bots"]["b1"]["conversations"]["private:u2"]["workspacePath"],
            "D:/ws"
        );
        std::fs::remove_dir_all(base).ok();
    }
}
