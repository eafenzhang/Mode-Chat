//! 群聊回合编排的纯决策核心（P3.1，spec mode-bot-rust-migration.md §7）。
//!
//! 唯一调用方是壳侧 `orchestrator.rs`：目标选择、轮询状态机、落库决策与 @交接一跳
//! 都在 Rust 决策、在壳执行；2026-11-09 W0 已删除 renderer 的 `turn.ts` /
//! `GroupChatView` 回退执行器。crate 边界不变：纯函数、无 IO、无时钟——时间由调用方
//! 注入（`elapsed_ms`）。

/// 首条 prompt 已发出但会话还没进 running 的宽限（避免把初始 idle 当成完成）。
pub const TURN_START_GRACE_MS: u64 = 20_000;

/// 单个成员回合的超时（与 renderer `turn.ts` 的 `TURN_TIMEOUT_MS` 同值）。
pub const TURN_TIMEOUT_MS: u64 = 240_000;

/// 终态集合：与 desktop-continuous 的会话状态枚举一致（spec §1 数据不变量）。
fn is_terminal(status: &str) -> bool {
    matches!(status, "idle" | "completed" | "error" | "paused")
}

/// 频道的应答规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponderPolicy {
    /// 默认：@ 优先，无 @ 回落管理员（现行行为）。
    MentionsFirst,
    /// 始终管理员应答，忽略 @。
    AdminOnly,
}

/// 解析持久化值；空/缺省 = `mentions-first`，未知值报错（写盘侧已先校验）。
pub fn parse_responder_policy(value: Option<&str>) -> Result<ResponderPolicy, String> {
    match value.map(str::trim) {
        None | Some("") | Some("mentions-first") => Ok(ResponderPolicy::MentionsFirst),
        Some("admin-only") => Ok(ResponderPolicy::AdminOnly),
        Some(other) => Err(format!(
            "responderPolicy 非法：{other}（只允许 mentions-first / admin-only）"
        )),
    }
}

/// 目标选择：按频道应答规则决定谁回答。
/// `MentionsFirst`：点名优先，无点名回落管理员；顺序保持点名顺序，不去重。
/// `AdminOnly`：始终管理员（无管理员 → 空队列），忽略点名。
pub fn select_targets(
    mentioned_ids: &[String],
    admin_id: Option<&str>,
    policy: ResponderPolicy,
    everyone: bool,
    member_ids: &[String],
) -> Vec<String> {
    // 显式 @所有人 覆盖应答策略（用户拍板：仅管理员频道也全员派发）。
    if everyone {
        let mut seen = std::collections::HashSet::new();
        return member_ids
            .iter()
            .filter(|id| seen.insert((*id).clone()))
            .cloned()
            .collect();
    }
    match policy {
        ResponderPolicy::AdminOnly => admin_id.map(str::to_string).into_iter().collect(),
        ResponderPolicy::MentionsFirst => {
            if !mentioned_ids.is_empty() {
                mentioned_ids.to_vec()
            } else {
                admin_id.map(str::to_string).into_iter().collect()
            }
        }
    }
}

/// 一轮回复的落库决策（send 队列对单个成员回合结果的处理）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplySink {
    /// 有正文且不是 `[PASS]`：追加进群记录。
    Append,
    /// `[PASS]`：闭嘴语义，不出气泡、不进群记录（DS-Bot 对齐）。
    SkipPass,
    /// 空回复且非 waiting：调用方记「空回复」提示（不进群记录）。
    EmptyNotice,
    /// 空回复且在等审批：不出提示行（审批提示由调用方单独出）。
    AwaitingApproval,
}

/// 落库决策纯函数。`waiting` 只影响空回复分支——TS 现状里 waiting 提示在落库判定
/// 之前设置、但不改变落库（有正文照走 Append/SkipPass）。
pub fn decide_reply_sink(has_text: bool, is_pass_reply: bool, waiting: bool) -> ReplySink {
    if has_text {
        if is_pass_reply {
            ReplySink::SkipPass
        } else {
            ReplySink::Append
        }
    } else if waiting {
        ReplySink::AwaitingApproval
    } else {
        ReplySink::EmptyNotice
    }
}

/// @交接计划（一跳不变量）：管理员回复里 @到的、还没处理过且不是发言者本人的成员。
/// **只在管理员回合调用**——一跳由「只有管理员回复产生交接」+ handled 集合共同保证；
/// 交接成员自己的回复不再产生交接（它们的回合不是管理员回合）。顺序保持 @ 出现顺序。
pub fn plan_handoffs(
    current_member_id: &str,
    mentioned_ids: &[String],
    handled_ids: &[String],
) -> Vec<String> {
    mentioned_ids
        .iter()
        .filter(|id| id.as_str() != current_member_id && !handled_ids.contains(id))
        .cloned()
        .collect()
}

/// 回合内轮询的一步决策输入（从会话快照归一而来，纯数据）。
#[derive(Debug, Clone)]
pub struct PollTurnInput<'a> {
    /// `snapshot.session.status`。
    pub status: &'a str,
    /// 本轮读到了 assistant 正文（`extractLastAssistantText` 非空）。
    pub has_text: bool,
    /// `isAwaitingApproval(snapshot)`：waiting 状态，或审批挂起
    /// （权威信号是 `runtime.pendingRequestIds`，status 在审批期间可能仍是 running）。
    pub awaiting_approval: bool,
    /// 到本轮为止 started 的累计值。
    pub started_so_far: bool,
    /// 本轮距回合开始的毫秒数。
    pub elapsed_ms: u64,
}

/// 一步决策输出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnPollDecision {
    /// 继续轮询。
    Continue,
    /// 成员在等审批——正常状态，调用方提示用户去成员会话处理，不抛错。
    Waiting,
    /// 会话终态，回合结束；status 透传（idle/completed/error/paused）。
    Finished { status: String },
}

/// 轮询状态机的一步：返回 (更新后的 started, 决策)。
///
/// 与 renderer `runGroupTurn` 轮询体逐行对齐：
/// 1. started 累计：读到正文，或 status 进入 running/waiting；
/// 2. `awaiting_approval` 权威优先 → Waiting（即使 status 还是 running）；
/// 3. 终态且（已启动 或 已过宽限）→ Finished；
/// 4. 其余 Continue；超时（elapsed ≥ [`TURN_TIMEOUT_MS`]）由调用方判定后抛错。
pub fn poll_turn_step(input: PollTurnInput) -> (bool, TurnPollDecision) {
    let started = input.started_so_far
        || input.has_text
        || input.status == "running"
        || input.status == "waiting";
    if input.awaiting_approval {
        return (started, TurnPollDecision::Waiting);
    }
    if is_terminal(input.status) && (started || input.elapsed_ms > TURN_START_GRACE_MS) {
        return (
            started,
            TurnPollDecision::Finished {
                status: input.status.to_string(),
            },
        );
    }
    (started, TurnPollDecision::Continue)
}

#[cfg(test)]
mod tests {
    use super::{
        decide_reply_sink, plan_handoffs, poll_turn_step, select_targets, PollTurnInput,
        ReplySink, ResponderPolicy, TurnPollDecision,
    };

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn mention_priority_over_admin() {
        assert_eq!(
            select_targets(
                &ids(&["t2", "t1"]),
                Some("admin"),
                ResponderPolicy::MentionsFirst,
                false,
                &[]
            ),
            ids(&["t2", "t1"])
        );
    }

    #[test]
    fn falls_back_to_admin_when_not_mentioned() {
        assert_eq!(
            select_targets(&[], Some("admin"), ResponderPolicy::MentionsFirst, false, &[]),
            vec!["admin".to_string()]
        );
    }

    #[test]
    fn admin_only_ignores_mentions() {
        assert_eq!(
            select_targets(&ids(&["t2"]), Some("admin"), ResponderPolicy::AdminOnly, false, &[]),
            ids(&["admin"])
        );
        assert!(select_targets(&ids(&["t2"]), None, ResponderPolicy::AdminOnly, false, &[]).is_empty());
    }

    #[test]
    fn no_targets_without_mention_or_admin() {
        assert!(select_targets(&[], None, ResponderPolicy::MentionsFirst, false, &[]).is_empty());
    }

    #[test]
    fn everyone_overrides_responder_policy_and_dedupes() {
        let members = ids(&["t1", "t2", "t1"]);
        assert_eq!(
            select_targets(&[], Some("admin"), ResponderPolicy::AdminOnly, true, &members),
            ids(&["t1", "t2"])
        );
        assert_eq!(
            select_targets(&[], Some("admin"), ResponderPolicy::MentionsFirst, true, &members),
            ids(&["t1", "t2"])
        );
    }

    #[test]
    fn text_without_pass_appends() {
        assert_eq!(decide_reply_sink(true, false, false), ReplySink::Append);
        assert_eq!(decide_reply_sink(true, false, true), ReplySink::Append);
    }

    #[test]
    fn pass_reply_never_lands_even_when_waiting() {
        assert_eq!(decide_reply_sink(true, true, false), ReplySink::SkipPass);
        assert_eq!(decide_reply_sink(true, true, true), ReplySink::SkipPass);
    }

    #[test]
    fn empty_reply_distinguishes_waiting_from_silence() {
        assert_eq!(
            decide_reply_sink(false, false, true),
            ReplySink::AwaitingApproval
        );
        assert_eq!(
            decide_reply_sink(false, false, false),
            ReplySink::EmptyNotice
        );
    }

    #[test]
    fn handoff_excludes_self_and_handled() {
        // 入参来自 parse_mentioned_ids（上游已按出现顺序去重），不再重复去重。
        assert_eq!(
            plan_handoffs("t1", &ids(&["t2", "t1", "t3"]), &ids(&["t3"])),
            vec!["t2".to_string()]
        );
    }

    #[test]
    fn handoff_of_nothing_is_empty() {
        assert!(plan_handoffs("t1", &[], &[]).is_empty());
        assert!(plan_handoffs("t1", &ids(&["t1"]), &[]).is_empty());
    }

    fn step(
        status: &str,
        has_text: bool,
        awaiting: bool,
        started: bool,
        elapsed_ms: u64,
    ) -> (bool, TurnPollDecision) {
        poll_turn_step(PollTurnInput {
            status,
            has_text,
            awaiting_approval: awaiting,
            started_so_far: started,
            elapsed_ms,
        })
    }

    #[test]
    fn fresh_idle_within_grace_keeps_polling() {
        let (started, decision) = step("idle", false, false, false, 1_000);
        assert!(!started);
        assert_eq!(decision, TurnPollDecision::Continue);
    }

    #[test]
    fn idle_after_grace_finishes_even_without_start() {
        let (started, decision) = step("idle", false, false, false, 21_000);
        assert!(!started);
        assert_eq!(
            decision,
            TurnPollDecision::Finished {
                status: "idle".to_string()
            }
        );
    }

    #[test]
    fn text_marks_started_and_terminal_then_finishes() {
        let (started, decision) = step("completed", true, false, false, 3_000);
        assert!(started);
        assert_eq!(
            decision,
            TurnPollDecision::Finished {
                status: "completed".to_string()
            }
        );
    }

    #[test]
    fn running_counts_as_started_but_never_terminal() {
        let (started, decision) = step("running", false, false, false, 3_000);
        assert!(started);
        assert_eq!(decision, TurnPollDecision::Continue);
    }

    #[test]
    fn approval_wait_is_authoritative_even_when_running() {
        // status 在审批期间可能仍是 running：awaiting_approval 优先返回 Waiting。
        let (started, decision) = step("running", false, true, false, 3_000);
        assert!(started);
        assert_eq!(decision, TurnPollDecision::Waiting);
    }

    #[test]
    fn error_status_finishes_once_started() {
        let (started, decision) = step("error", false, false, true, 2_000);
        assert!(started);
        assert_eq!(
            decision,
            TurnPollDecision::Finished {
                status: "error".to_string()
            }
        );
    }

    #[test]
    fn started_accumulates_across_calls() {
        let (started_first, first) = step("running", false, false, false, 1_000);
        assert!(started_first);
        assert_eq!(first, TurnPollDecision::Continue);
        // 下一轮 status 回到 idle（快照抖动）也不能丢 started。
        let (_, second) = step("idle", false, false, started_first, 2_000);
        assert_eq!(
            second,
            TurnPollDecision::Finished {
                status: "idle".to_string()
            }
        );
    }
}
