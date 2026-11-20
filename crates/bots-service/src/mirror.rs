//! 桌面提示的 IM 回显镜像（`armConversationReplyMirror` / `notifyDesktopUserMessage` 的
//! 目标发现与策略部分）。
//!
//! 语义：
//! - 只在 Bot 启用、且存在“绑定到该 taskId + 同 workspaceKey、mode=task”的会话时镜像；
//! - 目标取会话的 `conversationId`（群 chatId / 私聊 userId），缺失则跳过；
//! - 桌面附件不随镜像上传（隐式数据外发），只把数量交给宿主渲染“未同步”提示；
//! - 空文本不镜像。
//!
//! 镜像发送本身由宿主执行（微信 `send_text` / 企微 `aibot_send_msg`）。

use serde_json::Value;

use crate::workspace::workspace_key;
use crate::{BotConfig, BotsConfigFile};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorTarget {
    pub bot_id: String,
    pub provider: crate::BotProvider,
    pub conversation_key: String,
    /// 群 chatId / 私聊 userId。
    pub target_id: String,
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopPromptMirror {
    pub text: String,
    pub attachment_count: usize,
    pub targets: Vec<MirrorTarget>,
}

/// 找出该 task 在该 workspace 下所有需要镜像的会话目标（跨 bot）。
pub fn find_mirror_targets(
    state: &Value,
    config: &BotsConfigFile,
    task_id: &str,
    workspace_path: &str,
    workspace_identity: Option<&str>,
) -> Vec<MirrorTarget> {
    let wanted_key = workspace_key(workspace_path, workspace_identity);
    let mut targets = Vec::new();
    let Some(bots) = state.get("bots").and_then(Value::as_object) else {
        return targets;
    };
    for (bot_id, channel) in bots {
        let Some(bot) = config.bots.iter().find(|bot| &bot.id == bot_id && bot.enabled) else {
            continue;
        };
        let Some(conversations) = channel.get("conversations").and_then(Value::as_object) else {
            continue;
        };
        for (conversation_key, context) in conversations {
            if context.get("mode").and_then(Value::as_str) != Some("task") {
                continue;
            }
            if context.get("activeTaskId").and_then(Value::as_str) != Some(task_id) {
                continue;
            }
            let context_workspace = context
                .get("workspacePath")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let context_identity = context.get("workspaceIdentity").and_then(Value::as_str);
            if workspace_key(context_workspace, context_identity) != wanted_key {
                continue;
            }
            let target_id = context
                .get("conversationId")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(conversation_key.as_str());
            if target_id.trim().is_empty() {
                continue;
            }
            targets.push(MirrorTarget {
                bot_id: bot.id.clone(),
                provider: bot.provider,
                conversation_key: conversation_key.clone(),
                target_id: target_id.to_string(),
                workspace_path: context_workspace.to_string(),
                workspace_identity: context_identity.map(str::to_string),
            });
        }
    }
    targets
}

/// 桌面提示镜像策略：空文本不镜像；附件只记数量（由宿主渲染提示，绝不上传）。
pub fn build_desktop_prompt_mirror(
    state: &Value,
    config: &BotsConfigFile,
    task_id: &str,
    workspace_path: &str,
    workspace_identity: Option<&str>,
    text: &str,
    attachment_count: usize,
) -> Option<DesktopPromptMirror> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let targets = find_mirror_targets(
        state,
        config,
        task_id,
        workspace_path,
        workspace_identity,
    );
    if targets.is_empty() {
        return None;
    }
    Some(DesktopPromptMirror {
        text: text.to_string(),
        attachment_count,
        targets,
    })
}

pub fn enabled_bot<'a>(config: &'a BotsConfigFile, bot_id: &str) -> Option<&'a BotConfig> {
    config
        .bots
        .iter()
        .find(|bot| bot.id == bot_id && bot.enabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> BotsConfigFile {
        let bot = |id: &str, provider: crate::BotProvider, enabled: bool| crate::BotConfig {
            id: id.to_string(),
            name: id.to_string(),
            provider,
            enabled,
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
            allowed_workspaces: vec!["*".to_string()],
            group_chat: None,
            delivery_mode: None,
            heartbeat: None,
            allowed_commands: None,
            current_options: None,
            reply_mode: crate::BotReplyMode::AssistantChanges,
        };
        BotsConfigFile {
            version: 3,
            bots: vec![
                bot("b1", crate::BotProvider::Weixin, true),
                bot("b2", crate::BotProvider::Wecom, false),
            ],
        }
    }

    fn state() -> Value {
        json!({
            "bots": {
                "b1": { "conversations": {
                    "private:u1": { "mode": "task", "activeTaskId": "t1", "workspacePath": "D:/ws", "conversationId": "u1" },
                    "group:g1": { "mode": "task", "activeTaskId": "t1", "workspacePath": "D:/ws", "conversationId": "g1" },
                    "draft": { "mode": "draft", "workspacePath": "D:/ws" }
                }},
                "b2": { "conversations": {
                    "private:u2": { "mode": "task", "activeTaskId": "t1", "workspacePath": "D:/ws" }
                }},
                "b3": { "conversations": {} }
            }
        })
    }

    #[test]
    fn finds_targets_only_for_enabled_bots_and_matching_task_workspace() {
        let targets =
            find_mirror_targets(&state(), &config(), "t1", "D:/ws", None);
        assert_eq!(targets.len(), 2, "{targets:?}");
        assert!(targets.iter().all(|target| target.bot_id == "b1"));
        assert!(targets
            .iter()
            .any(|target| target.conversation_key == "group:g1" && target.target_id == "g1"));
        // 不同 task / 不同 workspace 不镜像。
        assert!(find_mirror_targets(&state(), &config(), "t2", "D:/ws", None).is_empty());
        assert!(find_mirror_targets(&state(), &config(), "t1", "D:/other", None).is_empty());
    }

    #[test]
    fn mirror_policy_skips_empty_text_and_counts_attachments() {
        let mirror = build_desktop_prompt_mirror(
            &state(),
            &config(),
            "t1",
            "D:/ws",
            None,
            " 桌面输入 ",
            2,
        )
        .unwrap();
        assert_eq!(mirror.text, "桌面输入");
        assert_eq!(mirror.attachment_count, 2);
        assert_eq!(mirror.targets.len(), 2);
        assert!(build_desktop_prompt_mirror(&state(), &config(), "t1", "D:/ws", None, "  ", 1).is_none());
        assert!(enabled_bot(&config(), "b2").is_none());
    }
}
