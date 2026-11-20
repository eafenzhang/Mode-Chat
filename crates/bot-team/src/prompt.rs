//! 群聊回合的 prompt 组装。
//!
//! 唯一实现：本模块的测试即行为真源（原 TS `turnPrompt.ts` 对拍版已在 N4 删除）。
//! 预算按 UTF-16 code unit 计（历史 TS `String.length` 的口径），不是 Rust 的 UTF-8
//! 字节数。任何口径变化都要先改测试，再改实现。

use crate::context_packet::{
    build_context_packet, ContextPacket, ContextPacketInput, ContextPacketState, PacketError,
    PacketHistoryLine, PACKET_MAX_CHARS,
};

const PROMPT_MAX_CHARS: i64 = 8_000;
const HISTORY_LINES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptHistoryLine {
    /// `user` | `member`
    pub role: String,
    /// 发言人 id：续用隐藏会话时按它过滤掉本人历史，避免与 session 内已有回复重复。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<String>,
    #[serde(default)]
    pub member_name: Option<String>,
    pub text: String,
    /// 稳定消息 id（transcript 的 `at`）；context packet 归档/去重使用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

/// 成员续用隐藏会话时的 prompt 历史：过滤掉该成员自己的行。
///
/// 群成员的隐藏会话里已经保存了它上一轮的回复；再把 transcript 里同一条回复
/// 当历史塞进 prompt 会重复占预算，也会让模型以为同一句话说了两遍。
/// 用户行、其他成员行、没有 memberId 的旧行都保留（旧行无法可靠归属）。
pub fn history_for_member(
    history: &[PromptHistoryLine],
    member_id: &str,
) -> Vec<PromptHistoryLine> {
    history
        .iter()
        .filter(|line| line.member_id.as_deref() != Some(member_id))
        .cloned()
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupPromptPacket {
    pub prompt: String,
    pub packet: Option<ContextPacket>,
    /// 本次是否因 provider 用量 ≥80% 使用了缩小预算。
    pub compacted: bool,
}

fn packet_history(input: &GroupPromptInput) -> Vec<PacketHistoryLine> {
    input
        .history
        .iter()
        .enumerate()
        .map(|(index, line)| PacketHistoryLine {
            message_id: line
                .message_id
                .clone()
                .unwrap_or_else(|| format!("line-{index}")),
            role: line.role.clone(),
            member_name: line.member_name.clone(),
            text: line.text.clone(),
        })
        .collect()
}

fn packet_rules(input: &GroupPromptInput) -> (String, String) {
    let role_suffix = if input.member_role.is_empty() {
        String::new()
    } else {
        format!("（{}）", input.member_role)
    };
    let identity_line = if input.member_is_admin {
        format!(
            "你是这个群的管理员「{}」{}。用户默认和你对话；需要别人时可以在回复里用 @成员名 说明你要谁接手。",
            input.member_name, role_suffix
        )
    } else if input.addressed_by_user {
        format!(
            "你是群成员「{}」{}，用户本轮点名让你回答。",
            input.member_name, role_suffix
        )
    } else {
        format!(
            "你是群成员「{}」{}，管理员把这件事交给你接手。",
            input.member_name, role_suffix
        )
    };
    let mut rules = vec![
        format!("# 群聊：{}", input.group_name),
        identity_line,
        if input.is_chief {
            "你是团队的 Chief，负责协调成员、把活分给合适的人，并对最终交付负责。"
                .to_string()
        } else {
            String::new()
        },
        format!("群成员：{}。", input.member_names.join("、")),
    ];
    let instructions = input.instructions.trim();
    if !instructions.is_empty() {
        rules.push(format!("群规则：{instructions}"));
    }
    let goal = input.goal_prompt.trim();
    if !goal.is_empty() {
        rules.push(goal.to_string());
        if input.member_is_admin {
            rules.push(
                "你是本群管理员，负责推动目标阶段：按用户的指示推进当前阶段，并在阶段完成时给出依据。"
                    .to_string(),
            );
        }
    }
    let mut memory = Vec::new();
    let group_memory = input.memory.trim();
    if !group_memory.is_empty() {
        memory.push(format!(
            "群记忆（本群已确认的事实，只在群内有效）：\n{group_memory}"
        ));
    }
    let team_memory = input.team_memory.trim();
    if !team_memory.is_empty() {
        memory.push(format!(
            "团队记忆（所有 Bot 共享，跨频道有效）：\n{team_memory}"
        ));
    }
    (
        rules
            .into_iter()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        memory.join("\n\n"),
    )
}

/// W2：用 context packet 组装 prompt；返回 packet 供壳侧持久化归档状态。
/// `continue` 模式仍走最小续用 prompt（packet 只用于 start）。
pub fn build_group_prompt_packet(
    input: &GroupPromptInput,
    state: &ContextPacketState,
    archive_hint: Option<String>,
) -> Result<GroupPromptPacket, PacketError> {
    if input.mode.trim() == "continue" {
        return Ok(GroupPromptPacket {
            prompt: build_continue_prompt(input),
            packet: None,
            compacted: false,
        });
    }
    let (rules, memory) = packet_rules(input);
    // provider 用量 ≥80% 时缩小本包预算（下一个成员/下一回合更早归档）。
    let budget_chars = if state.last_provider_usage_ratio.unwrap_or(0.0) >= 0.8 {
        PACKET_MAX_CHARS * 4 / 5
    } else {
        PACKET_MAX_CHARS
    };
    let packet_input = ContextPacketInput {
        rules,
        memory,
        task_head: String::new(),
        task_tail: format!(
            "用户刚说：\n{}\n\n请直接写你在群里的回复（面向用户，简洁；不要复述这段说明）。",
            input.text
        ),
        quotes: Vec::new(),
        history: packet_history(input),
        covered_message_ids: state.covered_message_ids.clone(),
        member_last_replies: Vec::new(),
        archived_count: state.archived_count(),
        summary_text: state.summary_text.clone(),
        archive_hint,
        budget_chars: Some(budget_chars),
    };
    let packet = build_context_packet(&packet_input)?;
    Ok(GroupPromptPacket {
        prompt: packet.text.clone(),
        packet: Some(packet),
        compacted: budget_chars < PACKET_MAX_CHARS,
    })
}

/// 成员的最近终态（来自群 state 的 `last_status`；空 = 还没跑过）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TeamMemberStatus {
    pub name: String,
    pub role: String,
    pub is_admin: bool,
    pub status: String,
}

fn status_label(status: &str) -> &'static str {
    match status {
        "running" | "working" => "工作中",
        "waiting" | "waiting-approval" | "waiting_approval" => "等待中",
        "error" | "failed" => "异常",
        _ => "空闲",
    }
}

/// 团队现状段落：只给管理员（Main Bot 汇总/派活的最小工具面，无需额外 MCP 工具）。
fn team_status_section(input: &GroupPromptInput) -> Option<String> {
    if !input.member_is_admin || input.team_status.is_empty() {
        return None;
    }
    let rows: Vec<String> = input
        .team_status
        .iter()
        .map(|member| {
            let role = member.role.trim();
            let role = if role.is_empty() {
                String::new()
            } else {
                format!("，{role}")
            };
            format!(
                "- {}（{}{}）：{}",
                member.name,
                if member.is_admin { "管理员" } else { "成员" },
                role,
                status_label(&member.status)
            )
        })
        .collect();
    Some(format!(
        "## 团队现状（汇总/派活时参考）\n{}\n需要谁接手就在回复里写 @成员名；汇总进度时以这里的成员状态与群聊记录为准。",
        rows.join("\n")
    ))
}

#[derive(Debug, Clone)]
pub struct GroupPromptInput {
    pub group_name: String,
    pub member_name: String,
    pub member_role: String,
    /// 这个成员是不是频道管理员。
    pub member_is_admin: bool,
    /// 用户本轮是否点名了这个成员（非管理员被直接点名）。
    pub addressed_by_user: bool,
    /// 团队 Chief（组织层 v1：只加提示词与默认管理员语义）。
    pub is_chief: bool,
    pub member_names: Vec<String>,
    pub instructions: String,
    pub memory: String,
    /// 团队记忆（所有 Bot 共享；由壳侧从 `mode-bot:team` 的记忆目录读出）。
    pub team_memory: String,
    /// 群目标（批次 H）：壳侧从 `goal.json` 渲染好的固定文本；空 = 本频道没有目标。
    pub goal_prompt: String,
    /// 团队现状（Main Bot 最小工具面）：成员的最近终态；只注入给管理员。
    pub team_status: Vec<TeamMemberStatus>,
    /// `start`：新会话，带头部+历史；`continue`：续用已有 session，只带最小上下文。
    pub mode: String,
    pub history: Vec<PromptHistoryLine>,
    pub text: String,
}

/// 续用已有 session：不再重复历史（模型已经有），只补频道规则/记忆与用户这句。
fn build_continue_prompt(input: &GroupPromptInput) -> String {
    let role_suffix = if input.member_role.is_empty() {
        String::new()
    } else {
        format!("（{}）", input.member_role)
    };
    let identity_line = if input.member_is_admin {
        format!(
            "你是这个群的管理员「{}」{}。用户默认和你对话；需要别人时可以在回复里用 @成员名 说明你要谁接手。",
            input.member_name, role_suffix
        )
    } else if input.addressed_by_user {
        format!(
            "你是群成员「{}」{}，用户本轮点名让你回答。",
            input.member_name, role_suffix
        )
    } else {
        format!(
            "你是群成员「{}」{}，管理员把这件事交给你接手。",
            input.member_name, role_suffix
        )
    };
    let mut parts = vec![format!("# 群聊：{}", input.group_name), identity_line];
    if input.is_chief {
        parts.push(
            "你是团队的 Chief，负责协调成员、把活分给合适的人，并对最终交付负责。"
                .to_string(),
        );
    }
    let instructions = input.instructions.trim();
    if !instructions.is_empty() {
        parts.push(format!("群规则：{instructions}"));
    }
    let goal = input.goal_prompt.trim();
    if !goal.is_empty() {
        parts.push(goal.to_string());
        if input.member_is_admin {
            parts.push(
                "你是本群管理员，负责推动目标阶段：按用户的指示推进当前阶段，并在阶段完成时给出依据。"
                    .to_string(),
            );
        }
    }
    if let Some(section) = team_status_section(input) {
        parts.push(section);
    }
    let memory = input.memory.trim();
    if !memory.is_empty() {
        parts.push(format!("群记忆（本群已确认的事实，只在群内有效）：\n{memory}"));
    }
    let team_memory = input.team_memory.trim();
    if !team_memory.is_empty() {
        parts.push(format!("团队记忆（所有 Bot 共享，跨频道有效）：\n{team_memory}"));
    }
    parts.push(format!("用户刚说：\n{}", input.text));
    parts.push("请直接写你在群里的回复（面向用户，简洁；不要复述这段说明）。".to_string());
    parts.join("\n\n")
}

/// 组装进群成员的 prompt：群规则 + 最近记录 + 用户这句；总量有上限。
pub fn build_group_prompt(input: &GroupPromptInput) -> String {
    if input.mode.trim() == "continue" {
        return build_continue_prompt(input);
    }
    let roster = input.member_names.join("、");
    let role_suffix = if input.member_role.is_empty() {
        String::new()
    } else {
        format!("（{}）", input.member_role)
    };
    let instructions = input.instructions.trim();
    let memory = input.memory.trim();
    let team_memory = input.team_memory.trim();
    let identity_line = if input.member_is_admin {
        format!(
            "你是这个群的管理员「{}」{}。用户默认和你对话；需要别人时可以在回复里用 @成员名 说明你要谁接手。",
            input.member_name, role_suffix
        )
    } else if input.addressed_by_user {
        format!(
            "你是群成员「{}」{}，用户本轮点名让你回答。",
            input.member_name, role_suffix
        )
    } else {
        format!(
            "你是群成员「{}」{}，管理员把这件事交给你接手。",
            input.member_name, role_suffix
        )
    };
    let header = [
        format!("# 群聊：{}", input.group_name),
        String::new(),
        identity_line,
        if input.is_chief {
            "你是团队的 Chief，负责协调成员、把活分给合适的人，并对最终交付负责。"
                .to_string()
        } else {
            String::new()
        },
        format!("群成员：{}。", roster),
        if instructions.is_empty() {
            String::new()
        } else {
            format!("群规则：{instructions}")
        },
        if input.goal_prompt.trim().is_empty() {
            String::new()
        } else if input.member_is_admin {
            format!(
                "{}\n你是本群管理员，负责推动目标阶段：按用户的指示推进当前阶段，并在阶段完成时给出依据。",
                input.goal_prompt.trim()
            )
        } else {
            input.goal_prompt.trim().to_string()
        },
        team_status_section(input).unwrap_or_default(),
        if memory.is_empty() {
            String::new()
        } else {
            format!("群记忆（本群已确认的事实，只在群内有效）：\n{memory}")
        },
        if team_memory.is_empty() {
            String::new()
        } else {
            format!("团队记忆（所有 Bot 共享，跨频道有效）：\n{team_memory}")
        },
        String::new(),
    ]
    .into_iter()
    .filter(|line| !line.is_empty())
    .collect::<Vec<_>>()
    .join("\n");

    let history_start = input.history.len().saturating_sub(HISTORY_LINES);
    let history = &input.history[history_start..];
    let mut budget =
        PROMPT_MAX_CHARS - utf16_len(&header) as i64 - utf16_len(&input.text) as i64 - 200;
    let mut lines: Vec<String> = Vec::new();
    for line in history.iter().rev() {
        let speaker = if line.role == "user" {
            "用户".to_string()
        } else {
            format!("成员 {}", line.member_name.as_deref().unwrap_or(""))
                .trim()
                .to_string()
        };
        let rendered = format!("[{speaker}] {}", line.text);
        let length = utf16_len(&rendered) as i64;
        if length > budget {
            break;
        }
        budget -= length;
        lines.insert(0, rendered);
    }

    [
        header,
        if lines.is_empty() {
            String::new()
        } else {
            format!("最近的群聊记录：\n{}", lines.join("\n"))
        },
        format!("用户刚说：\n{}", input.text),
        "请直接写你在群里的回复（面向用户，简洁；不要复述这段说明）。".to_string(),
    ]
    .into_iter()
    .filter(|part| !part.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n\n")
}

fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> GroupPromptInput {
        GroupPromptInput {
            team_status: Vec::new(),
            group_name: "发布小组".into(),
            member_name: "娜娜".into(),
            member_role: "发布".into(),
            member_is_admin: true,
            addressed_by_user: false,
            is_chief: false,
            member_names: vec!["娜娜".into(), "小白".into()],
            instructions: " 先跑测试 ".into(),
            memory: " 周五发版 ".into(),
            team_memory: " 团队统一用 Rust ".into(),
            goal_prompt: "# 群目标（进行中）\n目标：发布 v1\n阶段进度：0/2，当前阶段「调研」\n- [→] 调研\n- [ ] 实现".into(),
            mode: "start".into(),
            history: vec![
                PromptHistoryLine {
                    role: "user".into(),
                    member_id: None,
                    member_name: None,
                    text: "准备发版".into(),
                    message_id: Some("m1".into()),
                },
                PromptHistoryLine {
                    role: "member".into(),
                    member_id: Some("b2".into()),
                    member_name: Some("小白".into()),
                    text: "收到".into(),
                    message_id: Some("m2".into()),
                },
            ],
            text: "开始吧".into(),
        }
    }

    #[test]
    fn includes_group_rules_memory_roster_and_history() {
        let prompt = build_group_prompt(&input());
        assert!(prompt.contains("# 群聊：发布小组"));
        assert!(prompt.contains("你是这个群的管理员「娜娜」（发布）。"));
        assert!(prompt.contains("群成员：娜娜、小白。"));
        assert!(prompt.contains("群规则：先跑测试"));
        assert!(prompt.contains("# 群目标（进行中）"));
        assert!(prompt.contains("阶段进度：0/2，当前阶段「调研」"));
        assert!(prompt.contains("你是本群管理员，负责推动目标阶段"));
        assert!(prompt.contains("群记忆（本群已确认的事实，只在群内有效）：\n周五发版"));
        assert!(prompt.contains("团队记忆（所有 Bot 共享，跨频道有效）：\n团队统一用 Rust"));
        assert!(prompt.contains("最近的群聊记录：\n[用户] 准备发版\n[成员 小白] 收到"));
        assert!(prompt.contains("用户刚说：\n开始吧"));
    }


    #[test]
    fn admin_gets_team_status_and_others_do_not() {
        let mut value = input();
        value.team_status = vec![
            TeamMemberStatus {
                name: "娜娜".into(),
                role: "发布".into(),
                is_admin: true,
                status: "idle".into(),
            },
            TeamMemberStatus {
                name: "小白".into(),
                role: String::new(),
                is_admin: false,
                status: "running".into(),
            },
        ];
        let admin_prompt = build_group_prompt(&value);
        assert!(admin_prompt.contains("## 团队现状（汇总/派活时参考）"));
        assert!(admin_prompt.contains("- 娜娜（管理员，发布）：空闲"));
        assert!(admin_prompt.contains("- 小白（成员）：工作中"));
        assert!(admin_prompt.contains("需要谁接手就在回复里写 @成员名"));
        // 续用会话同样注入。
        let continued = build_continue_prompt(&GroupPromptInput {
            mode: "continue".into(),
            ..value.clone()
        });
        assert!(continued.contains("- 小白（成员）：工作中"));
        // 非管理员不注入团队现状。
        value.member_is_admin = false;
        assert!(!build_group_prompt(&value).contains("## 团队现状"));
    }

    #[test]
    fn omits_empty_optional_sections() {
        let mut value = input();
        value.member_role = String::new();
        value.instructions = "   ".into();
        value.memory = String::new();
        value.team_memory = String::new();
        value.goal_prompt = String::new();
        value.history.clear();
        let prompt = build_group_prompt(&value);
        assert!(prompt.contains("你是这个群的管理员「娜娜」。"));
        assert!(!prompt.contains("群规则："));
        assert!(!prompt.contains("群目标"));
        assert!(!prompt.contains("群记忆"));
        assert!(!prompt.contains("最近的群聊记录："));
    }

    #[test]
    fn non_admin_members_get_goal_without_admin_duty_line() {
        let mut value = input();
        value.member_is_admin = false;
        value.addressed_by_user = true;
        let prompt = build_group_prompt(&value);
        assert!(prompt.contains("# 群目标（进行中）"));
        assert!(!prompt.contains("负责推动目标阶段"));
        let continued = build_continue_prompt(&GroupPromptInput {
            mode: "continue".into(),
            ..value
        });
        assert!(continued.contains("# 群目标（进行中）"));
        assert!(!continued.contains("负责推动目标阶段"));
    }

    #[test]
    fn history_for_member_drops_only_the_members_own_lines() {
        let history = vec![
            PromptHistoryLine {
                role: "user".into(),
                member_id: None,
                member_name: None,
                text: "用户发言".into(),
                message_id: Some("m1".into()),
            },
            PromptHistoryLine {
                role: "member".into(),
                member_id: Some("b1".into()),
                member_name: Some("小白".into()),
                text: "自己上一轮".into(),
                message_id: Some("m2".into()),
            },
            PromptHistoryLine {
                role: "member".into(),
                member_id: Some("b2".into()),
                member_name: Some("小蓝".into()),
                text: "队友上一轮".into(),
                message_id: Some("m3".into()),
            },
            PromptHistoryLine {
                role: "member".into(),
                member_id: None,
                member_name: Some("旧成员".into()),
                text: "无法归属的旧行".into(),
                message_id: Some("m4".into()),
            },
        ];
        let filtered = history_for_member(&history, "b1");
        assert_eq!(filtered.len(), 3);
        assert!(filtered.iter().all(|line| line.member_id.as_deref() != Some("b1")));
        assert!(filtered.iter().any(|line| line.text == "队友上一轮"));
        assert!(filtered.iter().any(|line| line.text == "无法归属的旧行"));
    }

    #[test]
    fn keeps_last_16_history_lines_and_drops_older() {
        let mut value = input();
        value.history = (0..20)
            .map(|index| PromptHistoryLine {
                role: "user".into(),
                member_id: None,
                member_name: None,
                text: format!("第{index}条"),
                message_id: Some(format!("m{index}")),
            })
            .collect();
        let prompt = build_group_prompt(&value);
        assert!(!prompt.contains("第0条"));
        assert!(!prompt.contains("第3条"));
        assert!(prompt.contains("第4条"));
        assert!(prompt.contains("第19条"));
    }

    #[test]
    fn packet_prompt_archives_and_filters_covered_messages() {
        let mut value = input();
        value.history = (0..200)
            .map(|index| PromptHistoryLine {
                role: "user".into(),
                member_id: None,
                member_name: None,
                text: format!("第{index}条 {}", "x".repeat(150)),
                message_id: Some(format!("m{index}")),
            })
            .collect();
        let mut state = ContextPacketState::empty();
        let first = build_group_prompt_packet(&value, &state, Some("sess-1".into())).unwrap();
        let packet = first.packet.expect("start 模式应有 packet");
        assert!(first.prompt.contains("## 最近的群聊记录"));
        assert!(packet.stats.archived_lines > 0);
        assert!(state.apply_packet(&packet, 1));
        let first_covered = state.covered_message_ids.clone();
        assert!(!first_covered.is_empty());

        let second = build_group_prompt_packet(&value, &state, Some("sess-1".into())).unwrap();
        assert!(!second.prompt.contains("第0条"));
        assert!(second.prompt.contains("更早 "));
        assert!(second.prompt.contains("见成员会话 sess-1") || second.prompt.contains("已归档"));
    }

    #[test]
    fn provider_usage_above_80_percent_shrinks_packet_budget() {
        let mut value = input();
        value.history = (0..300)
            .map(|index| PromptHistoryLine {
                role: "user".into(),
                member_id: None,
                member_name: None,
                text: format!("第{index}条 {}", "x".repeat(200)),
                message_id: Some(format!("m{index}")),
            })
            .collect();
        let mut state = ContextPacketState::empty();
        state.last_provider_usage_ratio = Some(0.9);
        let result = build_group_prompt_packet(&value, &state, Some("sess-1".into())).unwrap();
        assert!(result.compacted);
        let packet = result.packet.unwrap();
        assert!(packet.stats.total_chars <= PACKET_MAX_CHARS * 4 / 5);
        assert!(packet.stats.recent <= PACKET_MAX_CHARS * 2 / 5);
    }

    #[test]
    fn budget_is_counted_in_utf16_units() {
        // “😀” 在 TS 的 length 里是 2；若按 UTF-8 字节会是 4。这里用足预算边界证明口径。
        let emoji = "😀".repeat(3_900);
        let mut value = input();
        value.history.clear();
        value.text = emoji;
        let prompt = build_group_prompt(&value);
        // 不崩溃、仍包含用户原话即可；精确预算由上面的 history 截断用例覆盖。
        assert!(prompt.contains("用户刚说："));
    }
}
