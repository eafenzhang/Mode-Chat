//! 入站闸门（`botsService` 的 blocked / 群激活 / 私聊绑定判定）。
//!
//! 判定顺序与 TS 一致：
//! 1. 黑名单：群消息看群，私聊/其它看用户；命中 → 静默 Ignore（不回消息，不回落私聊）。
//! 2. 群：`activation=disabled` 或缺少群 id → Ignore；未批准的群在 free 模式自动晋升
//!    （PromoteGroup，宿主登记 approved+auto），白名单模式 Ignore；mention 模式未被 @
//!    只入上下文（Ignore）；其余 Allow。
//! 3. 私聊：`access_mode=free` 或 `private_chat_mode=all_users` → Allow；否则需要绑定
//!    （RequiresBinding，宿主查绑定表后再决定）。
//!
//! 群内任何成员都不再被私聊绑定关系拦一次（群授权在闸门完成）。

use crate::{BotAccessMode, BotConfig, BotGroupActivation, BotGroupPermissionStatus, BotPrivateChatMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    Allow,
    /// 静默忽略（黑名单/群未批准/未 @/群聊关闭）；消息可只入上下文。
    Ignore,
    /// free 模式的未批准群：宿主应登记 approved（addedBy=auto）。
    PromoteGroup,
    /// 白名单模式的未知群：宿主登记 pending（不回复），供 UI 审批。
    RegisterPendingGroup,
    /// 私聊需要绑定：宿主查绑定后决定 Allow / 回复绑定指引。
    RequiresBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateMessage {
    pub chat_id: Option<String>,
    /// private | group
    pub chat_type: String,
    pub provider_user_id: String,
    pub is_mention: bool,
}

pub fn is_user_blocked(bot: &BotConfig, provider_user_id: &str) -> bool {
    bot.blocked_users
        .as_ref()
        .is_some_and(|users| users.iter().any(|user| user == provider_user_id))
}

pub fn is_group_blocked(bot: &BotConfig, group_id: &str) -> bool {
    if group_id.is_empty() {
        return false;
    }
    bot.blocked_groups
        .as_ref()
        .is_some_and(|groups| groups.iter().any(|group| group.group_id == group_id))
}

pub fn is_group_approved(bot: &BotConfig, group_id: &str) -> bool {
    bot.group_chat
        .as_ref()
        .and_then(|group_chat| group_chat.permissions.as_ref())
        .is_some_and(|permissions| {
            permissions.iter().any(|permission| {
                permission.group_id == group_id
                    && permission.status == BotGroupPermissionStatus::Approved
            })
        })
}

fn free_access_mode(bot: &BotConfig) -> bool {
    // TS `resolveBotAccessMode`：显式 accessMode 优先；缺省时旧 all_users 视为 free。
    bot.access_mode == Some(BotAccessMode::Free)
        || bot.private_chat_mode == Some(BotPrivateChatMode::AllUsers)
}

/// 私聊已绑定用户：`providerUserId` 相等，或显式 `allowedUsers` 包含。
fn is_bound_user(bot: &BotConfig, provider_user_id: &str) -> bool {
    let uid = provider_user_id.trim();
    if uid.is_empty() {
        return false;
    }
    bot.provider_user_id.as_deref().map(str::trim) == Some(uid)
        || bot
            .allowed_users
            .as_ref()
            .is_some_and(|users| users.iter().any(|user| user.trim() == uid))
}

pub fn evaluate_inbound_gate(bot: &BotConfig, message: &GateMessage) -> GateDecision {
    if message.chat_type == "group" {
        let chat_id = message.chat_id.as_deref().unwrap_or("").trim();
        if is_group_blocked(bot, chat_id) {
            return GateDecision::Ignore;
        }
    } else if is_user_blocked(bot, &message.provider_user_id) {
        return GateDecision::Ignore;
    }
    if message.chat_type == "private" {
        if free_access_mode(bot) || is_bound_user(bot, &message.provider_user_id) {
            return GateDecision::Allow;
        }
        return GateDecision::RequiresBinding;
    }
    let chat_id = message.chat_id.as_deref().unwrap_or("").trim();
    let activation = bot
        .group_chat
        .as_ref()
        .map(|group_chat| group_chat.activation)
        .unwrap_or(BotGroupActivation::Disabled);
    if activation == BotGroupActivation::Disabled || chat_id.is_empty() {
        // 关闭群聊或缺少群身份：静默结束，不能把群消息回落到私聊或猜测目标。
        return GateDecision::Ignore;
    }
    if !is_group_approved(bot, chat_id) {
        if free_access_mode(bot) {
            return GateDecision::PromoteGroup;
        }
        // 已绑定/显式允许的用户在未批准群仍可驱动（TS 同口径）；未知群登记 pending。
        if !is_bound_user(bot, &message.provider_user_id) {
            return GateDecision::RegisterPendingGroup;
        }
    }
    if activation == BotGroupActivation::Mention && !message.is_mention {
        return GateDecision::Ignore;
    }
    GateDecision::Allow
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BotGroupChat, BotGroupPermission, BotProvider, BotReplyMode};

    fn bot(access: Option<BotAccessMode>, group: Option<BotGroupChat>) -> BotConfig {
        BotConfig {
            id: "b1".to_string(),
            name: "bot".to_string(),
            provider: BotProvider::Weixin,
            enabled: true,
            credential_ref: None,
            webhook_secret_ref: None,
            webhook_url: None,
            webhook_auth_header_name: None,
            feishu_app_id: None,
            provider_user_id: None,
            provider_self_id: None,
            private_chat_mode: None,
            access_mode: access,
            blocked_users: Some(vec!["bad-user".to_string()]),
            blocked_groups: Some(vec![crate::BotBlockedGroup {
                group_id: "blocked-room".to_string(),
                group_name: None,
                platform: BotProvider::Weixin,
                blocked_at: 1,
            }]),
            user_registry: None,
            display_name: None,
            wecom_bot_id: None,
            dingtalk_client_id: None,
            dingtalk_use_ai_card: None,
            dingtalk_card_template_id: None,
            allowed_users: None,
            allowed_workspaces: vec!["*".to_string()],
            group_chat: group,
            delivery_mode: None,
            heartbeat: None,
            allowed_commands: None,
            current_options: None,
            reply_mode: BotReplyMode::AssistantChanges,
        }
    }

    fn group_chat(activation: BotGroupActivation, approved: &[&str]) -> BotGroupChat {
        BotGroupChat {
            activation,
            allowed_groups: None,
            permissions: Some(
                approved
                    .iter()
                    .map(|group_id| BotGroupPermission {
                        group_id: (*group_id).to_string(),
                        group_name: "群".to_string(),
                        platform: BotProvider::Weixin,
                        status: BotGroupPermissionStatus::Approved,
                        discovered_at: 1,
                        added_by: None,
                    })
                    .collect(),
            ),
        }
    }

    fn private(user: &str) -> GateMessage {
        GateMessage {
            chat_id: None,
            chat_type: "private".to_string(),
            provider_user_id: user.to_string(),
            is_mention: false,
        }
    }

    fn group(chat_id: &str, mention: bool) -> GateMessage {
        GateMessage {
            chat_id: Some(chat_id.to_string()),
            chat_type: "group".to_string(),
            provider_user_id: "u1".to_string(),
            is_mention: mention,
        }
    }

    #[test]
    fn blocked_user_and_group_are_ignored() {
        let config = bot(None, None);
        assert_eq!(
            evaluate_inbound_gate(&config, &private("bad-user")),
            GateDecision::Ignore
        );
        let config = bot(None, Some(group_chat(BotGroupActivation::Always, &["room-1"])));
        assert_eq!(
            evaluate_inbound_gate(&config, &group("blocked-room", true)),
            GateDecision::Ignore
        );
    }

    #[test]
    fn private_gate_respects_free_and_all_users() {
        assert_eq!(
            evaluate_inbound_gate(&bot(None, None), &private("u1")),
            GateDecision::RequiresBinding
        );
        assert_eq!(
            evaluate_inbound_gate(&bot(Some(BotAccessMode::Free), None), &private("u1")),
            GateDecision::Allow
        );
        let mut all_users = bot(None, None);
        all_users.private_chat_mode = Some(BotPrivateChatMode::AllUsers);
        assert_eq!(
            evaluate_inbound_gate(&all_users, &private("u1")),
            GateDecision::Allow
        );
    }

    #[test]
    fn group_gates_cover_disabled_unapproved_mention_and_always() {
        // 关闭群聊。
        assert_eq!(
            evaluate_inbound_gate(&bot(None, None), &group("room-1", true)),
            GateDecision::Ignore
        );
        // 白名单模式未批准群：登记 pending（不回复），供 UI 审批。
        assert_eq!(
            evaluate_inbound_gate(
                &bot(None, Some(group_chat(BotGroupActivation::Mention, &[]))),
                &group("room-1", true)
            ),
            GateDecision::RegisterPendingGroup
        );
        // 已绑定用户在未批准群仍可驱动。
        let mut bound = bot(None, Some(group_chat(BotGroupActivation::Mention, &[])));
        bound.provider_user_id = Some("u1".to_string());
        assert_eq!(
            evaluate_inbound_gate(&bound, &group("room-1", true)),
            GateDecision::Allow
        );
        // free 模式未批准群：自动晋升。
        assert_eq!(
            evaluate_inbound_gate(
                &bot(
                    Some(BotAccessMode::Free),
                    Some(group_chat(BotGroupActivation::Mention, &[]))
                ),
                &group("room-1", true)
            ),
            GateDecision::PromoteGroup
        );
        // mention 模式未 @：忽略；@ 后放行。
        let config = bot(None, Some(group_chat(BotGroupActivation::Mention, &["room-1"])));
        assert_eq!(
            evaluate_inbound_gate(&config, &group("room-1", false)),
            GateDecision::Ignore
        );
        assert_eq!(
            evaluate_inbound_gate(&config, &group("room-1", true)),
            GateDecision::Allow
        );
        let always = bot(None, Some(group_chat(BotGroupActivation::Always, &["room-1"])));
        assert_eq!(
            evaluate_inbound_gate(&always, &group("room-1", false)),
            GateDecision::Allow
        );
    }
}
