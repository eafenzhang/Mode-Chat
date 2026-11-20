//! 群上下文包（N6 W2，spec `docs/specs/bot-context-packet.md`）。
//!
//! 纯域：无 IO/时钟。把规则/记忆/任务/引用/摘要/最近原文/成员上轮回复/任务尾段按固定
//! 顺序装进一个有界 packet；预算按 **UTF-16 code unit** 计（历史 `String.length` 口径），
//! 最近原文不超过 packet 的 1/2。摘要 v1 不调模型，只做确定性归档说明。

use std::collections::HashSet;
use std::fmt;

/// packet 预算（UTF-16 code unit）。
pub const PACKET_MAX_CHARS: usize = 32_000;
const RECENT_RATIO_DENOMINATOR: usize = 2;
const QUOTES_MAX_CHARS: usize = PACKET_MAX_CHARS / 8;
const LAST_REPLIES_MAX_CHARS: usize = PACKET_MAX_CHARS / 16;
/// 摘要/分段标题的保守预留，避免历史先吃满预算导致摘要挤爆。
const SUMMARY_RESERVE: usize = 256;
const TASK_HEAD_LABEL: &str = "当前任务";
const TASK_TAIL_LABEL: &str = "任务指令";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketHistoryLine {
    pub message_id: String,
    /// `user` | `member`
    pub role: String,
    pub member_name: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextPacketInput {
    pub rules: String,
    pub memory: String,
    pub task_head: String,
    pub task_tail: String,
    pub quotes: Vec<PacketHistoryLine>,
    /// 旧 → 新。
    pub history: Vec<PacketHistoryLine>,
    /// 已在旧摘要中覆盖、不应再进入原文段的消息 id。
    pub covered_message_ids: Vec<String>,
    pub member_last_replies: Vec<String>,
    /// 已归档的条数（来自 context.json；与 covered 不一致时取较大值）。
    pub archived_count: usize,
    /// 已存在的摘要文本；本轮没有新归档时直接复用。
    pub summary_text: Option<String>,
    /// 归档说明里的会话提示（通常成员 sessionId）。
    pub archive_hint: Option<String>,
    /// 预算覆盖（provider 用量 ≥80% 时由调用方缩小）；None = 32,000。
    pub budget_chars: Option<usize>,
}

/// `context.json` 的持久化状态（spec §5）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPacketState {
    pub version: u32,
    pub summary_seq: u64,
    pub covered_message_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_text: Option<String>,
    pub updated_at: u64,
    /// 连续两次 packet 构建失败后停止压缩（spec §3 熔断）；人工 reset 前不再尝试。
    #[serde(default)]
    pub compaction_stopped: bool,
    #[serde(default)]
    pub consecutive_failures: u32,
    /// 上一回合成员会话的 provider 用量（0.0–1.0）；≥0.8 时下一包缩小预算。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_provider_usage_ratio: Option<f64>,
    /// 连续「压缩后用量仍 ≥70%」的次数；连续 2 次熔断。
    #[serde(default)]
    pub consecutive_ineffective_compactions: u32,
}

impl Default for ContextPacketState {
    fn default() -> Self {
        Self {
            version: 1,
            summary_seq: 0,
            covered_message_ids: Vec::new(),
            summary_text: None,
            updated_at: 0,
            compaction_stopped: false,
            consecutive_failures: 0,
            last_provider_usage_ratio: None,
            consecutive_ineffective_compactions: 0,
        }
    }
}

impl ContextPacketState {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn archived_count(&self) -> usize {
        self.covered_message_ids.len()
    }

    /// 记录一次 packet 构建失败；返回是否刚刚触发熔断（连续 2 次）。
    pub fn record_packet_failure(&mut self, at: u64) -> bool {
        if self.compaction_stopped {
            return false;
        }
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.updated_at = at;
        if self.consecutive_failures >= 2 {
            self.compaction_stopped = true;
            return true;
        }
        false
    }

    /// 记录一次成功；返回是否需要保存（连续失败计数被清零）。
    pub fn record_packet_success(&mut self, at: u64) -> bool {
        if self.consecutive_failures == 0 {
            return false;
        }
        self.consecutive_failures = 0;
        self.updated_at = at;
        true
    }

    /// 记录一次压缩后用量：`compacted` = 本包用了缩小预算；
    /// 连续 2 次压缩后仍 ≥70% → 熔断。返回是否刚刚熔断。
    pub fn record_compaction_outcome(
        &mut self,
        compacted: bool,
        usage_ratio: Option<f64>,
        at: u64,
    ) -> bool {
        if self.compaction_stopped {
            return false;
        }
        let Some(ratio) = usage_ratio else {
            return false;
        };
        if !compacted {
            if self.consecutive_ineffective_compactions != 0 {
                self.consecutive_ineffective_compactions = 0;
                self.updated_at = at;
            }
            return false;
        }
        if ratio >= 0.70 {
            self.consecutive_ineffective_compactions =
                self.consecutive_ineffective_compactions.saturating_add(1);
            self.updated_at = at;
            if self.consecutive_ineffective_compactions >= 2 {
                self.compaction_stopped = true;
                return true;
            }
        } else {
            self.consecutive_ineffective_compactions = 0;
            self.updated_at = at;
        }
        false
    }

    /// 合并 packet 的新归档；返回是否发生变化。
    pub fn apply_packet(&mut self, packet: &ContextPacket, at: u64) -> bool {
        let mut changed = false;
        for id in &packet.archived_message_ids {
            if !self.covered_message_ids.contains(id) {
                self.covered_message_ids.push(id.clone());
                changed = true;
            }
        }
        if packet.summary_text != self.summary_text {
            self.summary_text = packet.summary_text.clone();
            changed = true;
        }
        if !changed {
            return false;
        }
        self.summary_seq = self.summary_seq.saturating_add(1);
        self.updated_at = at;
        true
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PacketStats {
    pub total_chars: usize,
    pub rules: usize,
    pub memory: usize,
    pub task_head: usize,
    pub quotes: usize,
    pub summary: usize,
    pub recent: usize,
    pub last_replies: usize,
    pub task_tail: usize,
    pub kept_lines: usize,
    pub archived_lines: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPacket {
    pub text: String,
    pub stats: PacketStats,
    /// 用于持久化到 context.json（覆盖范围 = 旧 covered + 新 archived）。
    pub archived_message_ids: Vec<String>,
    pub kept_message_ids: Vec<String>,
    pub total_archived_count: usize,
    pub summary_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketError {
    FixedSectionsOverBudget { fixed_chars: usize, budget: usize },
}

impl fmt::Display for PacketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PacketError::FixedSectionsOverBudget {
                fixed_chars,
                budget,
            } => write!(
                formatter,
                "上下文包固定段（规则/记忆/任务）已超预算：{fixed_chars} > {budget}（UTF-16）"
            ),
        }
    }
}

fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

fn section(label: &str, body: &str) -> Option<String> {
    let body = body.trim();
    if body.is_empty() {
        None
    } else {
        Some(format!("## {label}\n{body}"))
    }
}

fn render_line(line: &PacketHistoryLine) -> String {
    let speaker = if line.role == "user" {
        "用户".to_string()
    } else {
        format!("成员 {}", line.member_name.as_deref().unwrap_or(""))
            .trim()
            .to_string()
    };
    format!("[{speaker}] {}", line.text)
}

fn summary_for(
    input: &ContextPacketInput,
    base_archived: usize,
    archived_now: usize,
) -> Option<(String, usize)> {
    let total = base_archived.saturating_add(archived_now);
    if total == 0 {
        return None;
    }
    let text = if archived_now == 0 {
        input
            .summary_text
            .clone()
            .unwrap_or_else(|| format!("更早 {total} 条已归档"))
    } else if let Some(hint) = input.archive_hint.as_deref().filter(|hint| !hint.is_empty()) {
        format!("更早 {total} 条已归档，见成员会话 {hint}")
    } else {
        format!("更早 {total} 条已归档")
    };
    Some((text, total))
}

fn assemble(
    input: &ContextPacketInput,
    summary: Option<&str>,
    kept: &[&PacketHistoryLine],
    quotes: &[String],
    replies: &[String],
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(rules) = section("群聊规则", &input.rules) {
        parts.push(rules);
    }
    if let Some(memory) = section("群记忆", &input.memory) {
        parts.push(memory);
    }
    if let Some(task) = section(TASK_HEAD_LABEL, &input.task_head) {
        parts.push(task);
    }
    if !quotes.is_empty() {
        parts.push(format!("## 引用消息\n{}", quotes.join("\n")));
    }
    if let Some(summary) = summary {
        parts.push(format!("## 更早记录\n{summary}"));
    }
    if !kept.is_empty() {
        let lines: Vec<String> = kept.iter().map(|line| render_line(line)).collect();
        parts.push(format!("## 最近的群聊记录\n{}", lines.join("\n")));
    }
    if !replies.is_empty() {
        parts.push(format!("## 成员上轮回复\n{}", replies.join("\n")));
    }
    if let Some(task) = section(TASK_TAIL_LABEL, &input.task_tail) {
        parts.push(task);
    }
    parts.join("\n\n")
}

/// 组装上下文包。
///
/// 固定段（规则/记忆/任务头尾）不可裁剪；超预算直接报错。其余段按固定顺序贪心填充：
/// 摘要 → 最近原文（≤ packet 1/2）→ 引用消息 → 成员上轮回复；不足时从最旧开始归档。
pub fn build_context_packet(
    input: &ContextPacketInput,
) -> Result<ContextPacket, PacketError> {
    let budget = input.budget_chars.unwrap_or(PACKET_MAX_CHARS).max(1);
    let fixed_text = assemble(input, None, &[], &[], &[]);
    let fixed_chars = utf16_len(&fixed_text);
    if fixed_chars >= budget {
        return Err(PacketError::FixedSectionsOverBudget {
            fixed_chars,
            budget,
        });
    }

    let covered: HashSet<&str> = input
        .covered_message_ids
        .iter()
        .map(String::as_str)
        .collect();
    let visible: Vec<&PacketHistoryLine> = input
        .history
        .iter()
        .filter(|line| !covered.contains(line.message_id.as_str()))
        .collect();
    let base_archived = input.archived_count.max(input.covered_message_ids.len());

    let available = budget.saturating_sub(fixed_chars + SUMMARY_RESERVE);
    let recent_cap = (budget / RECENT_RATIO_DENOMINATOR)
        .min(available)
        .saturating_sub(utf16_len("## 最近的群聊记录\n"));
    let mut kept: Vec<&PacketHistoryLine> = Vec::new();
    let mut recent_used = 0usize;
    for line in visible.iter().rev() {
        let rendered = render_line(line);
        let length = utf16_len(&rendered) + 1;
        if recent_used + length > recent_cap {
            break;
        }
        recent_used += length;
        kept.push(line);
    }
    kept.reverse();

    // 摘要会随归档条数变化；若固定+摘要+最近原文超预算，继续从最旧归档。
    let (mut summary, mut total_archived) = {
        let (text, total) = summary_for(input, base_archived, visible.len() - kept.len())
            .map(|(text, total)| (Some(text), total))
            .unwrap_or((None, 0));
        (text, total)
    };
    let mut packed = assemble(input, summary.as_deref(), &kept, &[], &[]);
    while utf16_len(&packed) > budget && !kept.is_empty() {
        kept.remove(0);
        let (text, total) = summary_for(input, base_archived, visible.len() - kept.len())
            .map(|(text, total)| (Some(text), total))
            .unwrap_or((None, 0));
        summary = text;
        total_archived = total;
        packed = assemble(input, summary.as_deref(), &kept, &[], &[]);
    }
    if utf16_len(&packed) > budget {
        return Err(PacketError::FixedSectionsOverBudget {
            fixed_chars: utf16_len(&packed),
            budget,
        });
    }

    // 剩余预算：引用消息 → 成员上轮回复。
    let mut remaining = budget - utf16_len(&packed);
    // 引用/上轮回复的 section 标题也占预算，先扣除再逐条填充。
    let quotes_cap = QUOTES_MAX_CHARS.min(remaining).saturating_sub(utf16_len("## 引用消息\n"));
    let mut quotes: Vec<String> = Vec::new();
    let mut quotes_used = 0usize;
    for quote in &input.quotes {
        let rendered = render_line(quote);
        let length = utf16_len(&rendered) + 1;
        if quotes_used + length > quotes_cap {
            continue;
        }
        quotes_used += length;
        remaining = remaining.saturating_sub(length);
        quotes.push(rendered);
    }
    if !quotes.is_empty() {
        remaining = remaining.saturating_sub(utf16_len("## 引用消息\n"));
    }
    let replies_cap = LAST_REPLIES_MAX_CHARS
        .min(remaining)
        .saturating_sub(utf16_len("## 成员上轮回复\n"));
    let mut replies: Vec<String> = Vec::new();
    let mut replies_used = 0usize;
    for reply in &input.member_last_replies {
        let text = reply.trim();
        if text.is_empty() {
            continue;
        }
        let length = utf16_len(text) + 1;
        if replies_used + length > replies_cap {
            continue;
        }
        replies_used += length;
        remaining = remaining.saturating_sub(length);
        replies.push(text.to_string());
    }

    let text = assemble(input, summary.as_deref(), &kept, &quotes, &replies);
    let archived_now = visible.len() - kept.len();
    let stats = PacketStats {
        total_chars: utf16_len(&text),
        rules: section("群聊规则", &input.rules)
            .map(|value| utf16_len(&value))
            .unwrap_or(0),
        memory: section("群记忆", &input.memory)
            .map(|value| utf16_len(&value))
            .unwrap_or(0),
        task_head: section(TASK_HEAD_LABEL, &input.task_head)
            .map(|value| utf16_len(&value))
            .unwrap_or(0),
        quotes: if quotes.is_empty() {
            0
        } else {
            utf16_len(&format!("## 引用消息\n{}", quotes.join("\n")))
        },
        summary: summary
            .as_deref()
            .map(|value| utf16_len(&format!("## 更早记录\n{value}")))
            .unwrap_or(0),
        recent: if kept.is_empty() {
            0
        } else {
            let lines: Vec<String> = kept.iter().map(|line| render_line(line)).collect();
            utf16_len(&format!("## 最近的群聊记录\n{}", lines.join("\n")))
        },
        last_replies: if replies.is_empty() {
            0
        } else {
            utf16_len(&format!("## 成员上轮回复\n{}", replies.join("\n")))
        },
        task_tail: section(TASK_TAIL_LABEL, &input.task_tail)
            .map(|value| utf16_len(&value))
            .unwrap_or(0),
        kept_lines: kept.len(),
        archived_lines: archived_now,
        truncated: archived_now > 0
            || quotes.len() < input.quotes.len()
            || replies.len() < input.member_last_replies.iter().filter(|r| !r.trim().is_empty()).count(),
    };
    Ok(ContextPacket {
        text,
        stats,
        archived_message_ids: visible[..archived_now]
            .iter()
            .map(|line| line.message_id.clone())
            .collect(),
        kept_message_ids: kept.iter().map(|line| line.message_id.clone()).collect(),
        total_archived_count: total_archived,
        summary_text: summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, role: &str, text: &str) -> PacketHistoryLine {
        PacketHistoryLine {
            message_id: id.to_string(),
            role: role.to_string(),
            member_name: (role == "member").then(|| "小白".to_string()),
            text: text.to_string(),
        }
    }

    fn input() -> ContextPacketInput {
        ContextPacketInput {
            rules: "先跑测试".into(),
            memory: "周五发版".into(),
            task_head: "用户要求：准备发版".into(),
            task_tail: "直接回复用户".into(),
            quotes: vec![line("q1", "user", "引用这条")],
            history: vec![
                line("m1", "user", "准备发版"),
                line("m2", "member", "收到"),
            ],
            covered_message_ids: Vec::new(),
            member_last_replies: vec!["上一位成员已完成".into()],
            archived_count: 0,
            summary_text: None,
            archive_hint: Some("sess-1".into()),
            budget_chars: None,
        }
    }

    #[test]
    fn builds_sections_in_fixed_order() {
        let packet = build_context_packet(&input()).unwrap();
        let rules = packet.text.find("## 群聊规则").unwrap();
        let memory = packet.text.find("## 群记忆").unwrap();
        let task = packet.text.find("## 当前任务").unwrap();
        let quotes = packet.text.find("## 引用消息").unwrap();
        let recent = packet.text.find("## 最近的群聊记录").unwrap();
        let replies = packet.text.find("## 成员上轮回复").unwrap();
        let tail = packet.text.find("## 任务指令").unwrap();
        assert!(rules < memory && memory < task && task < quotes && quotes < recent);
        assert!(recent < replies && replies < tail);
        assert!(packet.text.contains("[用户] 准备发版"));
        assert!(packet.text.contains("[成员 小白] 收到"));
        assert_eq!(packet.stats.kept_lines, 2);
        assert!(!packet.stats.truncated);
    }

    #[test]
    fn omits_empty_optional_sections() {
        let mut value = input();
        value.quotes.clear();
        value.member_last_replies.clear();
        value.history.clear();
        value.summary_text = None;
        value.memory.clear();
        let packet = build_context_packet(&value).unwrap();
        assert!(packet.text.contains("## 群聊规则"));
        assert!(!packet.text.contains("## 群记忆"));
        assert!(!packet.text.contains("## 引用消息"));
        assert!(!packet.text.contains("## 最近的群聊记录"));
        assert!(!packet.text.contains("## 成员上轮回复"));
    }

    #[test]
    fn recent_history_is_capped_at_half_packet_and_archives_oldest() {
        let mut value = input();
        value.history = (0..200)
            .map(|index| {
                line(
                    &format!("m{index}"),
                    "user",
                    &format!("第{index}条 {}", "x".repeat(200)),
                )
            })
            .collect();
        let packet = build_context_packet(&value).unwrap();
        assert!(packet.stats.total_chars <= PACKET_MAX_CHARS);
        assert!(packet.stats.recent <= PACKET_MAX_CHARS / 2);
        assert!(packet.stats.archived_lines > 0);
        assert!(packet.text.contains("第199条"));
        assert!(!packet.text.contains("第0条"));
        assert!(packet.text.contains("## 更早记录"));
        assert!(packet.summary_text.unwrap().contains("已归档"));
    }

    #[test]
    fn existing_summary_is_reused_when_nothing_new_is_archived() {
        let mut value = input();
        value.archived_count = 40;
        value.summary_text = Some("更早 40 条已归档，见成员会话 sess-old".into());
        let packet = build_context_packet(&value).unwrap();
        assert_eq!(
            packet.summary_text.as_deref(),
            Some("更早 40 条已归档，见成员会话 sess-old")
        );
        assert_eq!(packet.total_archived_count, 40);
        assert!(packet.archived_message_ids.is_empty());
    }

    #[test]
    fn new_archive_updates_summary_with_hint() {
        let mut value = input();
        value.history = (0..200)
            .map(|index| {
                line(
                    &format!("m{index}"),
                    "user",
                    &format!("第{index}条 {}", "x".repeat(200)),
                )
            })
            .collect();
        value.archived_count = 5;
        let packet = build_context_packet(&value).unwrap();
        let summary = packet.summary_text.unwrap();
        assert!(summary.starts_with("更早 "));
        assert!(summary.ends_with("见成员会话 sess-1"));
        assert_eq!(packet.total_archived_count, 5 + packet.stats.archived_lines);
    }

    #[test]
    fn covered_messages_are_filtered_and_summary_is_reused() {
        let mut value = input();
        value.history = vec![
            line("m1", "user", "已归档一"),
            line("m2", "member", "已归档二"),
            line("m3", "user", "新消息"),
        ];
        value.covered_message_ids = vec!["m1".into(), "m2".into()];
        value.archived_count = 2;
        value.summary_text = Some("更早 2 条已归档，见成员会话 sess-old".into());
        let packet = build_context_packet(&value).unwrap();
        assert!(!packet.text.contains("已归档一"));
        assert!(!packet.text.contains("已归档二"));
        assert!(packet.text.contains("新消息"));
        assert_eq!(packet.kept_message_ids, vec!["m3"]);
        assert!(packet.archived_message_ids.is_empty());
        assert_eq!(
            packet.summary_text.as_deref(),
            Some("更早 2 条已归档，见成员会话 sess-old")
        );
        assert_eq!(packet.total_archived_count, 2);
    }

    #[test]
    fn state_merges_new_archive_and_summary() {
        let mut value = input();
        value.history = (0..200)
            .map(|index| {
                line(
                    &format!("m{index}"),
                    "user",
                    &format!("第{index}条 {}", "x".repeat(200)),
                )
            })
            .collect();
        let packet = build_context_packet(&value).unwrap();
        let mut state = ContextPacketState::empty();
        assert!(state.apply_packet(&packet, 99));
        assert_eq!(state.summary_seq, 1);
        assert_eq!(state.updated_at, 99);
        assert_eq!(
            state.covered_message_ids.len(),
            packet.archived_message_ids.len()
        );
        assert!(state.summary_text.is_some());
        assert!(!state.apply_packet(&packet, 100), "重复合并不应再变化");
    }

    #[test]
    fn two_consecutive_packet_failures_stop_compaction() {
        let mut state = ContextPacketState::empty();
        assert!(!state.record_packet_failure(1));
        assert_eq!(state.consecutive_failures, 1);
        assert!(!state.compaction_stopped);
        assert!(state.record_packet_failure(2), "第二次失败应熔断");
        assert!(state.compaction_stopped);
        assert!(!state.record_packet_failure(3), "已熔断不重复触发");
        assert!(state.record_packet_success(4));
        assert_eq!(state.consecutive_failures, 0);
        assert!(state.compaction_stopped, "成功不自动解除熔断，需人工 reset");
        assert!(!state.record_packet_success(5));
    }

    #[test]
    fn budget_override_shrinks_packet() {
        let mut value = input();
        value.history = (0..300)
            .map(|index| {
                line(
                    &format!("m{index}"),
                    "user",
                    &format!("第{index}条 {}", "x".repeat(200)),
                )
            })
            .collect();
        value.budget_chars = Some(8_000);
        let packet = build_context_packet(&value).unwrap();
        assert!(packet.stats.total_chars <= 8_000);
        assert!(packet.stats.recent <= 4_000);
        assert!(packet.stats.archived_lines > 0);
    }

    #[test]
    fn two_ineffective_compactions_stop() {
        let mut state = ContextPacketState::empty();
        assert!(!state.record_compaction_outcome(true, Some(0.75), 1));
        assert_eq!(state.consecutive_ineffective_compactions, 1);
        assert!(state.record_compaction_outcome(true, Some(0.72), 2));
        assert!(state.compaction_stopped);
        assert!(!state.record_compaction_outcome(true, Some(0.9), 3));

        let mut reset = ContextPacketState::empty();
        assert!(!reset.record_compaction_outcome(true, Some(0.5), 1));
        assert_eq!(reset.consecutive_ineffective_compactions, 0);
        assert!(!reset.record_compaction_outcome(true, Some(0.75), 2));
        assert!(!reset.record_compaction_outcome(false, Some(0.75), 3));
        assert_eq!(reset.consecutive_ineffective_compactions, 0);
    }

    #[test]
    fn fixed_sections_over_budget_are_rejected() {
        let mut value = input();
        value.rules = "x".repeat(PACKET_MAX_CHARS);
        assert!(matches!(
            build_context_packet(&value),
            Err(PacketError::FixedSectionsOverBudget { .. })
        ));
    }

    #[test]
    fn utf16_budget_counts_surrogate_pairs() {
        // 7,000 个 😀 = 14,000 UTF-16 code units（UTF-8 是 28,000 字节）。
        // 若误按 UTF-8 计数，这条会被归档；按 UTF-16 应完整保留。
        let mut value = input();
        value.history = vec![line("e1", "user", &"😀".repeat(7_000))];
        value.quotes.clear();
        value.member_last_replies.clear();
        let packet = build_context_packet(&value).unwrap();
        assert!(packet.stats.total_chars <= PACKET_MAX_CHARS);
        assert_eq!(packet.kept_message_ids.len(), 1, "按 UTF-16 应保留");
        assert_eq!(packet.stats.archived_lines, 0);
        assert!(packet.stats.recent <= PACKET_MAX_CHARS / 2);
    }

    #[test]
    fn quotes_and_last_replies_are_bounded() {
        let mut value = input();
        value.quotes = (0..1000)
            .map(|index| line(&format!("q{index}"), "user", "引用内容"))
            .collect();
        value.member_last_replies = (0..1000).map(|_| "上轮回复".to_string()).collect();
        let packet = build_context_packet(&value).unwrap();
        assert!(packet.stats.total_chars <= PACKET_MAX_CHARS);
        assert!(packet.stats.quotes <= QUOTES_MAX_CHARS);
        assert!(packet.stats.last_replies <= LAST_REPLIES_MAX_CHARS);
        assert!(packet.stats.truncated);
    }
}
