//! Bot 命令解析（`commandParser.ts` 的完整移植）。
//!
//! 只在消息以 `/` 开头时解析；`0` 是选择态取消的快捷方式。命令名大小写不敏感，
//! 中英文别名并存。未知命令保留 `name/raw` 供上层回「未知命令」提示。

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotCommand {
    Message { text: String },
    Bind { code: String },
    Help,
    SelectionCancel,
    Status,
    New,
    Reconnect,
    WorkspaceSet { value: String },
    WorkspaceList,
    ModelProviderSet { value: String },
    ModelSet { value: String },
    ModelList,
    ModeSet { value: String },
    ModeList,
    ThoughtLevelSet { value: String },
    ThoughtLevelList,
    TaskSet { value: String },
    TaskList,
    ReplySet { value: String },
    ReplyList,
    Stop,
    Send { value: String },
    PermissionRespond { value: String },
    ElicitationSubmit,
    ElicitationRespond { value: String },
    Approve { request_id: String, option_id: String },
    Deny { request_id: String },
    Unknown { name: String, raw: String },
}

fn split_command(text: &str) -> Option<(String, String)> {
    let trimmed = text.trim();
    let body = trimmed.strip_prefix('/')?;
    // 必须按字符边界切分：全角空格/NBSP 是多字节，`index + 1` 会落在字符中间 panic。
    match body.char_indices().find(|(_, ch)| ch.is_whitespace()) {
        None => Some((body.to_lowercase(), String::new())),
        Some((index, ch)) => Some((
            body[..index].to_lowercase(),
            body[index + ch.len_utf8()..].trim().to_string(),
        )),
    }
}

pub fn parse_bot_command(text: &str) -> BotCommand {
    let Some((name, rest)) = split_command(text) else {
        if text.trim() == "0" {
            return BotCommand::SelectionCancel;
        }
        return BotCommand::Message {
            text: text.to_string(),
        };
    };
    let unknown = || BotCommand::Unknown {
        name: name.clone(),
        raw: text.to_string(),
    };
    match name.as_str() {
        "bind" => {
            if rest.is_empty() {
                unknown()
            } else {
                BotCommand::Bind { code: rest }
            }
        }
        "help" | "帮助" | "start" | "开始" => BotCommand::Help,
        "cancel" | "取消" => BotCommand::SelectionCancel,
        "status" | "状态" => BotCommand::Status,
        "new" | "clear" | "新建" => BotCommand::New,
        "reconnect" | "重连" => BotCommand::Reconnect,
        "workspace" | "project" | "项目" => {
            if rest.is_empty() {
                BotCommand::WorkspaceList
            } else {
                BotCommand::WorkspaceSet { value: rest }
            }
        }
        "model" | "模型" => {
            if rest.is_empty() {
                return BotCommand::ModelList;
            }
            if let Some(value) = rest.strip_prefix("provider ") {
                return BotCommand::ModelProviderSet {
                    value: value.trim().to_string(),
                };
            }
            if let Some(value) = rest.strip_prefix("model ") {
                return BotCommand::ModelSet {
                    value: value.trim().to_string(),
                };
            }
            BotCommand::ModelSet { value: rest }
        }
        "mode" | "模式" => {
            if rest.is_empty() {
                BotCommand::ModeList
            } else {
                BotCommand::ModeSet { value: rest }
            }
        }
        "thoughtlevel" | "thought_level" | "thought-level" | "think" | "思考" => {
            if rest.is_empty() {
                BotCommand::ThoughtLevelList
            } else {
                BotCommand::ThoughtLevelSet { value: rest }
            }
        }
        "task" => {
            if rest.is_empty() {
                BotCommand::TaskList
            } else {
                BotCommand::TaskSet { value: rest }
            }
        }
        "reply" | "回复" => {
            if rest.is_empty() {
                BotCommand::ReplyList
            } else {
                BotCommand::ReplySet { value: rest }
            }
        }
        "stop" | "停止" => BotCommand::Stop,
        "send" | "发送" => {
            if rest.is_empty() {
                unknown()
            } else {
                BotCommand::Send { value: rest }
            }
        }
        "permission" => {
            if rest.is_empty() {
                unknown()
            } else {
                BotCommand::PermissionRespond { value: rest }
            }
        }
        "elicitation" | "answer" | "回答" => {
            if rest.is_empty() {
                return unknown();
            }
            if ["submit", "done", "完成", "提交"].contains(&rest.to_lowercase().as_str()) {
                return BotCommand::ElicitationSubmit;
            }
            BotCommand::ElicitationRespond { value: rest }
        }
        "approve" => {
            let mut parts = rest.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some(request_id), Some(option_id)) => BotCommand::Approve {
                    request_id: request_id.to_string(),
                    option_id: option_id.to_string(),
                },
                _ => unknown(),
            }
        }
        "deny" => {
            if rest.is_empty() {
                unknown()
            } else {
                BotCommand::Deny { request_id: rest }
            }
        }
        _ => unknown(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_width_whitespace_does_not_panic() {
        assert!(matches!(parse_bot_command("/new "), BotCommand::New));
        assert!(matches!(
            parse_bot_command("/bind　ABC123"),
            BotCommand::Bind { code } if code == "ABC123"
        ));
    }

    #[test]
    fn plain_text_and_zero() {
        assert_eq!(
            parse_bot_command("你好"),
            BotCommand::Message {
                text: "你好".to_string()
            }
        );
        assert_eq!(parse_bot_command(" 0 "), BotCommand::SelectionCancel);
    }

    #[test]
    fn parses_help_aliases_case_insensitively() {
        for text in ["/help", "/HELP", "/帮助", "/start", "/开始"] {
            assert_eq!(parse_bot_command(text), BotCommand::Help, "{text}");
        }
    }

    #[test]
    fn parses_model_variants() {
        assert_eq!(parse_bot_command("/model"), BotCommand::ModelList);
        assert_eq!(
            parse_bot_command("/model provider deepseek"),
            BotCommand::ModelProviderSet {
                value: "deepseek".to_string()
            }
        );
        assert_eq!(
            parse_bot_command("/模型 model deepseek-flash"),
            BotCommand::ModelSet {
                value: "deepseek-flash".to_string()
            }
        );
        assert_eq!(
            parse_bot_command("/model deepseek-flash"),
            BotCommand::ModelSet {
                value: "deepseek-flash".to_string()
            }
        );
    }

    #[test]
    fn parses_approve_deny_and_elicitation() {
        assert_eq!(
            parse_bot_command("/approve req-1 opt-2"),
            BotCommand::Approve {
                request_id: "req-1".to_string(),
                option_id: "opt-2".to_string()
            }
        );
        assert!(matches!(
            parse_bot_command("/approve only-one"),
            BotCommand::Unknown { .. }
        ));
        assert_eq!(
            parse_bot_command("/deny req-1"),
            BotCommand::Deny {
                request_id: "req-1".to_string()
            }
        );
        assert_eq!(
            parse_bot_command("/回答 提交"),
            BotCommand::ElicitationSubmit
        );
        assert_eq!(
            parse_bot_command("/回答 42"),
            BotCommand::ElicitationRespond {
                value: "42".to_string()
            }
        );
    }

    #[test]
    fn unknown_and_missing_arguments() {
        assert!(matches!(
            parse_bot_command("/bogus"),
            BotCommand::Unknown { name, .. } if name == "bogus"
        ));
        assert!(matches!(parse_bot_command("/bind"), BotCommand::Unknown { .. }));
        assert!(matches!(parse_bot_command("/send"), BotCommand::Unknown { .. }));
        // `0` 带空格仍是选择取消；`/0` 不是命令。
        assert!(matches!(
            parse_bot_command("/0"),
            BotCommand::Unknown { .. }
        ));
    }
}
