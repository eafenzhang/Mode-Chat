//! 设置 → IM 数据面：bot 配置读写与列表视图。
//!
//! 运行态（扫码/轮询/错误）在 `im_service` 与 `im_runtime` 里；这里只做同步的
//! 配置读写（与主程序共用 `v2/bot-config.v3.json`），供 GPUI 设置页直接调用。

use std::path::Path;
use std::sync::Arc;

use bots_service::{
    BotAccessMode, BotConfig, BotGroupActivation, BotGroupChat, BotHeartbeat, BotProvider,
    BotReplyMode, BotSaveParams, BotsService,
};

use crate::im_service::TokenVault;

pub struct ImBotRow {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub provider_label: &'static str,
    pub enabled: bool,
    pub access_mode: &'static str,
    pub workspace_count: usize,
    pub credential_registered: bool,
}

fn provider_label(provider: &str) -> &'static str {
    match provider {
        "weixin" => "微信",
        "wecom" => "企业微信",
        "feishu" => "飞书",
        "lark" => "Lark",
        "dingtalk" => "钉钉",
        "telegram" => "Telegram",
        "discord" => "Discord",
        "webhook" => "Webhook",
        _ => "其他",
    }
}

fn access_label(bot: &BotConfig) -> &'static str {
    match bot.access_mode {
        Some(BotAccessMode::Free) => "自由",
        Some(BotAccessMode::Whitelist) => "白名单",
        None => {
            if bot.private_chat_mode == Some(bots_service::BotPrivateChatMode::AllUsers) {
                "自由"
            } else {
                "白名单"
            }
        }
    }
}

fn row_of(bot: BotConfig) -> ImBotRow {
    ImBotRow {
        provider: bot.provider.as_str().to_string(),
        provider_label: provider_label(bot.provider.as_str()),
        id: bot.id.clone(),
        name: bot.name.clone(),
        enabled: bot.enabled,
        access_mode: access_label(&bot),
        workspace_count: bot.allowed_workspaces.len(),
        credential_registered: bot
            .credential_ref
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty()),
    }
}

/// 读取全部 IM Bot（配置顺序）。
pub fn list(data_root: &Path) -> Result<Vec<ImBotRow>, String> {
    let service = BotsService::new(data_root);
    Ok(service.list_bots()?.into_iter().map(row_of).collect())
}

pub fn bot(data_root: &Path, bot_id: &str) -> Result<Option<BotConfig>, String> {
    Ok(BotsService::new(data_root)
        .list_bots()?
        .into_iter()
        .find(|bot| bot.id == bot_id))
}

fn service_with_vault(data_root: &Path) -> BotsService {
    BotsService::new(data_root).with_credentials(Arc::new(TokenVault::new(data_root)))
}

/// 新建微信 bot（草稿）：扫码成功后由 `im_service` 写凭据与自身 id。
/// 空名回落默认「微信 Bot」。
pub fn create_weixin_bot_named(data_root: &Path, name: &str) -> Result<BotConfig, String> {
    let name = name.trim();
    create_draft_bot(
        data_root,
        BotProvider::Weixin,
        if name.is_empty() { "微信 Bot" } else { name },
    )
}

/// 新建企业微信 bot（草稿）：扫码创建后由 `im_service` 写 botid + secret。
/// 空名回落默认「企微 Bot」。
pub fn create_wecom_bot_named(data_root: &Path, name: &str) -> Result<BotConfig, String> {
    let name = name.trim();
    create_draft_bot(
        data_root,
        BotProvider::Wecom,
        if name.is_empty() { "企微 Bot" } else { name },
    )
}

fn create_draft_bot(
    data_root: &Path,
    provider: BotProvider,
    name: &str,
) -> Result<BotConfig, String> {
    let bot = BotConfig {
        id: format!("bot-{}", uuid::Uuid::new_v4()),
        name: name.to_string(),
        provider,
        enabled: true,
        credential_ref: None,
        webhook_secret_ref: None,
        webhook_url: None,
        webhook_auth_header_name: None,
        feishu_app_id: None,
        provider_user_id: None,
        provider_self_id: None,
        private_chat_mode: None,
        access_mode: Some(BotAccessMode::Free),
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
        reply_mode: BotReplyMode::AssistantChanges,
    };
    service_with_vault(data_root).save_bot(&BotSaveParams {
        bot,
        credential_value: None,
        webhook_secret_value: None,
    })
}

pub fn set_enabled(data_root: &Path, bot_id: &str, enabled: bool) -> Result<(), String> {
    let mut bot = bot(data_root, bot_id)?.ok_or_else(|| format!("Bot 不存在：{bot_id}"))?;
    bot.enabled = enabled;
    service_with_vault(data_root).save_bot(&BotSaveParams {
        bot,
        credential_value: None,
        webhook_secret_value: None,
    })?;
    Ok(())
}

/// 改显示名：创建弹窗先建草稿、用户再填名称，提交时把名字补上。
pub fn set_name(data_root: &Path, bot_id: &str, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(());
    }
    let mut bot = bot(data_root, bot_id)?.ok_or_else(|| format!("Bot 不存在：{bot_id}"))?;
    bot.name = name.to_string();
    service_with_vault(data_root).save_bot(&BotSaveParams {
        bot,
        credential_value: None,
        webhook_secret_value: None,
    })?;
    Ok(())
}

pub fn set_access_mode(data_root: &Path, bot_id: &str, free: bool) -> Result<(), String> {
    let mode = if free {
        BotAccessMode::Free
    } else {
        BotAccessMode::Whitelist
    };
    BotsService::new(data_root).set_bot_access_mode(bot_id, mode)
}

pub fn delete(data_root: &Path, bot_id: &str) -> Result<(), String> {
    service_with_vault(data_root).delete_bot(bot_id)
}

/// 群聊激活策略：不响应 / 仅 @ / 始终。
pub fn set_group_activation(
    data_root: &Path,
    bot_id: &str,
    activation: BotGroupActivation,
) -> Result<(), String> {
    let mut bot = bot(data_root, bot_id)?.ok_or_else(|| format!("Bot 不存在：{bot_id}"))?;
    match &mut bot.group_chat {
        Some(group) => group.activation = activation,
        None => {
            bot.group_chat = Some(BotGroupChat {
                activation,
                allowed_groups: None,
                permissions: None,
            });
        }
    }
    service_with_vault(data_root).save_bot(&BotSaveParams {
        bot,
        credential_value: None,
        webhook_secret_value: None,
    })?;
    Ok(())
}

pub fn approve_group(data_root: &Path, bot_id: &str, group_id: &str) -> Result<(), String> {
    BotsService::new(data_root).approve_bot_group(bot_id, group_id)
}

pub fn reject_group(data_root: &Path, bot_id: &str, group_id: &str) -> Result<(), String> {
    BotsService::new(data_root).reject_bot_group(bot_id, group_id)
}

/// 机器人心跳：0 = 关闭；正数为间隔分钟（保留现有 activeHours）。
pub fn set_heartbeat(data_root: &Path, bot_id: &str, minutes: i64) -> Result<(), String> {
    let mut bot = bot(data_root, bot_id)?.ok_or_else(|| format!("Bot 不存在：{bot_id}"))?;
    if minutes <= 0 {
        bot.heartbeat = None;
    } else {
        let active_hours = bot
            .heartbeat
            .as_ref()
            .and_then(|heartbeat| heartbeat.active_hours.clone());
        bot.heartbeat = Some(BotHeartbeat {
            enabled: true,
            interval_minutes: minutes,
            active_hours,
        });
    }
    service_with_vault(data_root).save_bot(&BotSaveParams {
        bot,
        credential_value: None,
        webhook_secret_value: None,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempRoot {
        dir: std::path::PathBuf,
    }

    impl TempRoot {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("bot-gpui-im-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(dir.join("v2")).expect("create temp dir");
            Self { dir }
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn create_wecom_bot_uses_wecom_provider() {
        let temp = TempRoot::new();
        let bot = create_wecom_bot_named(&temp.dir, "").expect("create wecom");
        assert_eq!(bot.provider, BotProvider::Wecom);
        assert!(bot.wecom_bot_id.is_none());
        assert_eq!(bot.name, "企微 Bot");
        let rows = list(&temp.dir).expect("list");
        assert_eq!(rows[0].provider_label, "企业微信");
    }

    #[test]
    fn heartbeat_and_group_activation_roundtrip() {
        let temp = TempRoot::new();
        let created = create_weixin_bot_named(&temp.dir, "").expect("create");
        set_heartbeat(&temp.dir, &created.id, 30).expect("heartbeat");
        let saved = bot(&temp.dir, &created.id).expect("read").expect("exists");
        let heartbeat = saved.heartbeat.expect("heartbeat set");
        assert!(heartbeat.enabled);
        assert_eq!(heartbeat.interval_minutes, 30);
        set_heartbeat(&temp.dir, &created.id, 0).expect("heartbeat off");
        assert!(bot(&temp.dir, &created.id).expect("read").expect("exists").heartbeat.is_none());
        set_group_activation(&temp.dir, &created.id, BotGroupActivation::Always).expect("group");
        let saved = bot(&temp.dir, &created.id).expect("read").expect("exists");
        assert_eq!(
            saved.group_chat.map(|group| group.activation),
            Some(BotGroupActivation::Always)
        );
    }

    #[test]
    fn create_then_toggle_and_delete() {
        let temp = TempRoot::new();
        let bot = create_weixin_bot_named(&temp.dir, "").expect("create");
        assert!(bot.id.starts_with("bot-"));
        let rows = list(&temp.dir).expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].provider_label, "微信");
        assert_eq!(rows[0].access_mode, "自由");
        assert!(rows[0].enabled);
        set_enabled(&temp.dir, &bot.id, false).expect("disable");
        assert!(!list(&temp.dir).expect("list")[0].enabled);
        set_access_mode(&temp.dir, &bot.id, false).expect("whitelist");
        assert_eq!(list(&temp.dir).expect("list")[0].access_mode, "白名单");
        delete(&temp.dir, &bot.id).expect("delete");
        assert!(list(&temp.dir).expect("list").is_empty());
    }
}
