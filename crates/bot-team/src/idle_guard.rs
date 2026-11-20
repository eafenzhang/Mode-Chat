//! 闲置守卫的纯规则（DS-Bot 对标 P1，2026-11-11）。
//!
//! 语义（`docs/reviews/2026-11-09-ds-bot-benchmark.md` §能力对照）：**连续 N 次只有
//! ack/重复文本且 0 工具调用的 Bot 间接力 → 暂停后续 @**。这里只放判定与计数，
//! 状态落盘与「跳过接力」由宿主（orchestrator + group state）接线。
//!
//! 简化说明：v1 判定不读工具调用计数——ack 词表是严格单词确认，短确认回复基本不
//! 伴随工具调用；误伤面远小于把 transcript 工具解析搬进纯域的成本（已记入 spec）。

/// 连续多少次「空转回复」后暂停该成员的接力（DS-Bot 口径：6）。
pub const IDLE_ACK_LIMIT: u32 = 6;

/// 严格的确认词表：整条回复（trim 后，忽略结尾标点）命中其一才算 ack。
/// 故意保持很短——宁可不判定为闲置，也不能把正经短回复吞掉。
const ACK_WORDS: &[&str] = &[
    "ok", "okay", "好", "好的", "在", "在的", "收到", "嗯", "嗯嗯", "行", "可以", "明白",
    "了解", "没问题", "到", "来了", "done", "sure", "yes", "yeah", "yep", "received",
    "noted", "hi", "hello", "在吗", "hello!",
];

fn normalize_reply(reply: &str) -> &str {
    let trimmed = reply.trim();
    trimmed.trim_end_matches(['。', '！', '!', '？', '?', '~', '.', ','])
}

/// 这条回复是否「只有 ack」：非空、命中确认词表。
pub fn is_ack_reply(reply: &str) -> bool {
    let normalized = normalize_reply(reply);
    if normalized.is_empty() || normalized.chars().count() > 12 {
        return false;
    }
    let lower = normalized.to_lowercase();
    ACK_WORDS.iter().any(|word| lower == *word)
}

/// 这条回复是否与上一次回复重复（重复文本也算空转；至少要有实质内容长度才比对，
/// 避免「空/。」这类噪声被当成重复）。
pub fn is_repeat_reply(reply: &str, previous_reply: Option<&str>) -> bool {
    let normalized = normalize_reply(reply);
    let Some(previous) = previous_reply else {
        return false;
    };
    let previous = normalize_reply(previous);
    if normalized.is_empty() || previous.is_empty() {
        return false;
    }
    normalized == previous
}

/// 一次成员收口要记录什么：`record` 的输入。
pub struct IdleReplyInput<'a> {
    pub reply: &'a str,
    pub previous_reply: Option<&'a str>,
}

/// 单个成员的闲置计数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IdleStreak {
    /// 连续空转回复次数（实质回复清零）。
    pub streak: u32,
    /// 已达暂停线：接力（bot→bot 的 @ 交接）跳过该成员，直到其给出实质回复。
    pub paused: bool,
}

impl IdleStreak {
    /// 成员收口：更新计数。返回 (新状态, 本条是否判为空转)。
    pub fn record(self, input: IdleReplyInput<'_>) -> (IdleStreak, bool) {
        let idle = is_ack_reply(input.reply) || is_repeat_reply(input.reply, input.previous_reply);
        let mut next = self;
        if idle {
            next.streak = self.streak.saturating_add(1);
            if next.streak >= IDLE_ACK_LIMIT {
                next.paused = true;
            }
        } else {
            // 实质回复立即恢复：清零并解除暂停（「用户发言后恢复」的成员侧等价）。
            next.streak = 0;
            next.paused = false;
        }
        (next, idle)
    }

    /// 该成员当前是否应被接力跳过。
    pub fn should_skip_relay(self) -> bool {
        self.paused
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_ack_word_list_matches_confirmations_only() {
        for word in ["好的", "OK", "收到", "在", "没问题", "Done", "嗯。", "行！"] {
            assert!(is_ack_reply(word), "{word} 应判为 ack");
        }
        for reply in ["好的，我马上开始写", "已完成 lint 修复", "版本号是 2.4.0", ""] {
            assert!(!is_ack_reply(reply), "{reply} 不应判为 ack");
        }
    }

    #[test]
    fn repeat_detection_needs_both_sides_nonempty() {
        assert!(is_repeat_reply("重来", Some("重来")));
        assert!(!is_repeat_reply("重来", None));
        assert!(!is_repeat_reply("重来", Some("")));
        assert!(!is_repeat_reply("", Some("重来")));
    }

    #[test]
    fn streak_increments_and_pauses_at_limit() {
        let mut state = IdleStreak::default();
        for _ in 0..IDLE_ACK_LIMIT {
            let (next, idle) = state.record(IdleReplyInput {
                reply: "收到",
                previous_reply: None,
            });
            state = next;
            assert!(idle);
        }
        assert_eq!(state.streak, IDLE_ACK_LIMIT);
        assert!(state.should_skip_relay());
    }

    #[test]
    fn substantive_reply_resets_and_unpauses() {
        let mut state = IdleStreak {
            streak: IDLE_ACK_LIMIT,
            paused: true,
        };
        let (next, idle) = state.record(IdleReplyInput {
            reply: "已经把 2.4.0 的 changelog 写好了",
            previous_reply: Some("收到"),
        });
        state = next;
        assert!(!idle);
        assert_eq!(state.streak, 0);
        assert!(!state.paused);
        assert!(!state.should_skip_relay());
    }

    #[test]
    fn repeat_also_counts_toward_pause() {
        let mut state = IdleStreak::default();
        for _ in 0..IDLE_ACK_LIMIT {
            let (next, _) = state.record(IdleReplyInput {
                reply: "在",
                previous_reply: Some("在"),
            });
            state = next;
        }
        assert!(state.should_skip_relay());
    }
}
