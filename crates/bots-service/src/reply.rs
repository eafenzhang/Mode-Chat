//! 回复粒度（`replyMode`）与选项值解析（`bots.ts` 的 supported/default/normalize + 命令层）。
//!
//! - 飞书族只支持 `streaming_card`；企微/钉钉（以及已下线的 telegram）支持四种；
//!   其余通道（微信）不支持 `streaming_card`（纯文本通道无法编辑卡片）。
//! - 未显式设置时企微默认 `streaming_card`，其余默认 `assistant_changes`。
//! - `/reply <值>` 用与 `/model` 相同的解析：序号（1 起）/id/label（大小写不敏感），
//!   非法值回可选列表。设置结果写 bot 配置并做 provider 归一化。

use serde::{Deserialize, Serialize};

use crate::{BotConfig, BotProvider, BotsRepo};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyMode {
    AssistantChanges,
    AssistantToolcallsChanges,
    SummaryChanges,
    StreamingCard,
}

impl ReplyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AssistantChanges => "assistant_changes",
            Self::AssistantToolcallsChanges => "assistant_toolcalls_changes",
            Self::SummaryChanges => "summary_changes",
            Self::StreamingCard => "streaming_card",
        }
    }

    /// 与 BotConfig 的序列化枚举互相转换（两者变体一一对应）。
    pub fn to_bot(self) -> crate::BotReplyMode {
        match self {
            Self::AssistantChanges => crate::BotReplyMode::AssistantChanges,
            Self::AssistantToolcallsChanges => crate::BotReplyMode::AssistantToolcallsChanges,
            Self::SummaryChanges => crate::BotReplyMode::SummaryChanges,
            Self::StreamingCard => crate::BotReplyMode::StreamingCard,
        }
    }

    pub fn from_bot(value: crate::BotReplyMode) -> Self {
        match value {
            crate::BotReplyMode::AssistantChanges => Self::AssistantChanges,
            crate::BotReplyMode::AssistantToolcallsChanges => Self::AssistantToolcallsChanges,
            crate::BotReplyMode::SummaryChanges => Self::SummaryChanges,
            crate::BotReplyMode::StreamingCard => Self::StreamingCard,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "assistant_changes" => Some(Self::AssistantChanges),
            "assistant_toolcalls_changes" => Some(Self::AssistantToolcallsChanges),
            "summary_changes" => Some(Self::SummaryChanges),
            "streaming_card" => Some(Self::StreamingCard),
            _ => None,
        }
    }
}

pub const ALL_REPLY_MODES: &[ReplyMode] = &[
    ReplyMode::AssistantChanges,
    ReplyMode::AssistantToolcallsChanges,
    ReplyMode::SummaryChanges,
    ReplyMode::StreamingCard,
];

/// 该通道支持的粒度集合。
pub fn supported_reply_modes(provider: BotProvider) -> Vec<ReplyMode> {
    if provider.is_feishu_family() {
        return vec![ReplyMode::StreamingCard];
    }
    if matches!(provider, BotProvider::Telegram | BotProvider::Wecom | BotProvider::Dingtalk) {
        return ALL_REPLY_MODES.to_vec();
    }
    ALL_REPLY_MODES
        .iter()
        .copied()
        .filter(|mode| *mode != ReplyMode::StreamingCard)
        .collect()
}

/// provider 感知的默认粒度（不做支持性检查）。
pub fn default_reply_mode(provider: BotProvider) -> ReplyMode {
    if provider == BotProvider::Wecom {
        ReplyMode::StreamingCard
    } else {
        ReplyMode::AssistantChanges
    }
}

/// 归一化：候选不支持时退到该通道第一个支持项。
pub fn normalize_reply_mode(provider: BotProvider, candidate: Option<ReplyMode>) -> ReplyMode {
    let supported = supported_reply_modes(provider);
    let candidate = candidate.unwrap_or_else(|| default_reply_mode(provider));
    if supported.contains(&candidate) {
        candidate
    } else {
        supported[0]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectOption {
    pub id: String,
    pub label: String,
}

/// 选项解析：序号（1 起）→ id/label 精确匹配（大小写不敏感）。
pub fn resolve_option_by_value<'a>(items: &'a [SelectOption], value: &str) -> Option<&'a SelectOption> {
    let trimmed = value.trim();
    if let Ok(index) = trimmed.parse::<usize>() {
        if index > 0 {
            return items.get(index - 1);
        }
    }
    let normalized = trimmed.to_lowercase();
    items.iter().find(|item| {
        item.id.to_lowercase() == normalized || item.label.to_lowercase() == normalized
    })
}

fn reply_mode_options(provider: BotProvider) -> Vec<SelectOption> {
    supported_reply_modes(provider)
        .into_iter()
        .map(|mode| SelectOption {
            id: mode.as_str().to_string(),
            label: mode.as_str().to_string(),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyCommandReply {
    /// 设置成功（已归一化）。
    Set { mode: ReplyMode },
    /// 无参或非法值：返回可选列表与当前值。
    List {
        options: Vec<SelectOption>,
        current: ReplyMode,
    },
}

/// `/reply` 与 `/reply <值>`：写 bot 配置（provider 归一化）。
pub fn handle_reply_command(
    repo: &BotsRepo,
    bot_id: &str,
    value: Option<&str>,
) -> Result<ReplyCommandReply, String> {
    let config = repo.read_config()?;
    let bot: &BotConfig = config
        .bots
        .iter()
        .find(|bot| bot.id == bot_id)
        .ok_or_else(|| format!("Bot not found: {bot_id}"))?;
    let provider = bot.provider;
    let current = normalize_reply_mode(provider, Some(ReplyMode::from_bot(bot.reply_mode)));
    let Some(value) = value else {
        return Ok(ReplyCommandReply::List {
            options: reply_mode_options(provider),
            current,
        });
    };
    let options = reply_mode_options(provider);
    let selected = resolve_option_by_value(&options, value)
        .and_then(|option| ReplyMode::parse(&option.id));
    let Some(mode) = selected else {
        return Ok(ReplyCommandReply::List { options, current });
    };
    let mode = normalize_reply_mode(provider, Some(mode));
    repo.mutate_config(|config| {
        if let Some(bot) = config.bots.iter_mut().find(|bot| bot.id == bot_id) {
            bot.reply_mode = mode.to_bot();
        }
        Ok(())
    })?;
    Ok(ReplyCommandReply::Set { mode })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("reply-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn bot(id: &str, provider: BotProvider, reply_mode: ReplyMode) -> BotConfig {
        BotConfig {
            id: id.to_string(),
            name: id.to_string(),
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
            reply_mode: reply_mode.to_bot(),
        }
    }

    #[test]
    fn supported_and_defaults_match_ts() {
        assert_eq!(
            supported_reply_modes(BotProvider::Feishu),
            vec![ReplyMode::StreamingCard]
        );
        assert_eq!(supported_reply_modes(BotProvider::Wecom).len(), 4);
        assert!(!supported_reply_modes(BotProvider::Weixin).contains(&ReplyMode::StreamingCard));
        assert_eq!(default_reply_mode(BotProvider::Wecom), ReplyMode::StreamingCard);
        assert_eq!(default_reply_mode(BotProvider::Weixin), ReplyMode::AssistantChanges);
        // 微信候选 streaming_card 被归一化到第一个支持项。
        assert_eq!(
            normalize_reply_mode(BotProvider::Weixin, Some(ReplyMode::StreamingCard)),
            ReplyMode::AssistantChanges
        );
    }

    #[test]
    fn reply_command_sets_and_rejects() {
        let base = sandbox("command");
        let repo = BotsRepo::new(&base);
        repo.write_config(&crate::BotsConfigFile {
            version: 3,
            bots: vec![bot("b1", BotProvider::Weixin, ReplyMode::AssistantChanges)],
        })
        .unwrap();
        // 无参：列表 + 当前值。
        assert!(matches!(
            handle_reply_command(&repo, "b1", None).unwrap(),
            ReplyCommandReply::List { .. }
        ));
        // 序号选择 summary_changes（微信支持）。
        match handle_reply_command(&repo, "b1", Some("3")).unwrap() {
            ReplyCommandReply::Set { mode } => assert_eq!(mode, ReplyMode::SummaryChanges),
            other => panic!("unexpected {other:?}"),
        }
        let saved = ReplyMode::from_bot(repo.read_config().unwrap().bots[0].reply_mode);
        assert_eq!(saved, ReplyMode::SummaryChanges);
        // 微信选择 streaming_card：不在支持列表 → 回列表而不是设置。
        assert!(matches!(
            handle_reply_command(&repo, "b1", Some("streaming_card")).unwrap(),
            ReplyCommandReply::List { .. }
        ));
        std::fs::remove_dir_all(base).ok();
    }
}
