//! 名册状态派生（spec R2.4 / P2.1）：UI 只读运行态派生名册状态点，不写任何状态。
//!
//! 真源：renderer `roster/status.ts`（数据源是 sessions-index 的 SessionSummary，agent 侧权威）。
//! 本模块是它的 Rust 版：纯函数、无 IO；输入是归一后的会话快照字段子集
//! （phase / 待处理交互计数 / 是否结束 / 最近活动时间）。
//! 共享 Rust 会话索引可用后（Mode W3），名册派生直接切到这里、renderer 不再自己算；
//! 在此之前两边同用例对拍（TS 侧 `test/rosterStatus.test.mts` 与本文件 tests 同夹具）。

/// 名册状态点（与 renderer `TeammateStatus` 一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeammateStatus {
    Idle,
    Working,
    Waiting,
    /// 频道回合把活交给别的成员，正在等它交回；sessions-index 看不到这一档。
    WaitingOnBot,
    Error,
}

impl TeammateStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::WaitingOnBot => "waiting_on_bot",
            Self::Error => "error",
        }
    }
}

/// 派生需要读到的会话快照字段子集（`SessionSummary` 的映射见调用方）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterSessionSnapshot {
    pub session_id: String,
    /// `phase` 是开放字符串（completedSuccess / running / prewarming / error / draft …）。
    pub phase: String,
    /// `pendingInteractionSummary.permissionCount`（缺省 0）。
    pub pending_permission_count: u32,
    /// `pendingInteractionSummary.userInputCount`（缺省 0）。
    pub pending_user_input_count: u32,
    /// `sessionEnded`。
    pub session_ended: bool,
    /// `lastActivityAt`（缺省视为 0）。
    pub last_activity_at: Option<u64>,
}

/// 映射规则（与 renderer 同口径，优先级 error > waiting > working > idle，任一会话命中即取该档）：
/// - error   ：任一 `phase == "error"`
/// - waiting ：任一 `pendingInteractionSummary` 计数 > 0（审批/提问等待用户）
/// - working ：任一 `phase == "running" | "prewarming"`
/// - idle    ：其余（draft / completed* / 无会话）
pub fn derive_teammate_status(sessions: &[RosterSessionSnapshot]) -> TeammateStatus {
    let mut working = false;
    for session in sessions {
        if session.phase == "error" {
            return TeammateStatus::Error;
        }
        if session.pending_permission_count + session.pending_user_input_count > 0 {
            return TeammateStatus::Waiting;
        }
        if session.phase == "running" || session.phase == "prewarming" {
            working = true;
        }
    }
    if working {
        TeammateStatus::Working
    } else {
        TeammateStatus::Idle
    }
}

/// 会话恢复：最新一条未结束会话（`sessionEnded == false`，按 `lastActivityAt` 降序取第一）。
/// 与 TS 的 reduce 严格同口径：只有**严格更大**才替换，打平保留先出现的一条。
pub fn pick_resumable_session(
    sessions: &[RosterSessionSnapshot],
) -> Option<&RosterSessionSnapshot> {
    let mut latest: Option<&RosterSessionSnapshot> = None;
    for session in sessions.iter().filter(|session| !session.session_ended) {
        let activity = session.last_activity_at.unwrap_or(0);
        match latest {
            None => latest = Some(session),
            Some(current) if activity > current.last_activity_at.unwrap_or(0) => {
                latest = Some(session)
            }
            Some(_) => {}
        }
    }
    latest
}

#[cfg(test)]
mod tests {
    use super::{
        derive_teammate_status, pick_resumable_session, RosterSessionSnapshot, TeammateStatus,
    };

    fn base(session_id: &str) -> RosterSessionSnapshot {
        RosterSessionSnapshot {
            session_id: session_id.to_string(),
            phase: "completedSuccess".to_string(),
            pending_permission_count: 0,
            pending_user_input_count: 0,
            session_ended: true,
            last_activity_at: Some(0),
        }
    }

    fn running(session_id: &str, phase: &str) -> RosterSessionSnapshot {
        RosterSessionSnapshot {
            phase: phase.to_string(),
            session_ended: false,
            ..base(session_id)
        }
    }

    #[test]
    fn empty_list_is_idle() {
        assert_eq!(derive_teammate_status(&[]), TeammateStatus::Idle);
    }

    #[test]
    fn completed_success_is_idle() {
        assert_eq!(derive_teammate_status(&[base("a")]), TeammateStatus::Idle);
    }

    #[test]
    fn error_is_top_priority() {
        assert_eq!(
            derive_teammate_status(&[RosterSessionSnapshot {
                phase: "error".to_string(),
                ..base("a")
            }]),
            TeammateStatus::Error
        );
        // error 优先于 running。
        assert_eq!(
            derive_teammate_status(&[
                running("a", "running"),
                RosterSessionSnapshot {
                    phase: "error".to_string(),
                    ..base("b")
                },
            ]),
            TeammateStatus::Error
        );
    }

    #[test]
    fn running_and_prewarming_are_working() {
        assert_eq!(
            derive_teammate_status(&[running("a", "running")]),
            TeammateStatus::Working
        );
        assert_eq!(
            derive_teammate_status(&[running("a", "prewarming")]),
            TeammateStatus::Working
        );
    }

    #[test]
    fn pending_interactions_are_waiting_with_priority_over_working() {
        assert_eq!(
            derive_teammate_status(&[
                running("a", "running"),
                RosterSessionSnapshot {
                    pending_permission_count: 1,
                    ..base("b")
                },
            ]),
            TeammateStatus::Waiting
        );
        assert_eq!(
            derive_teammate_status(&[RosterSessionSnapshot {
                pending_user_input_count: 2,
                ..base("b")
            }]),
            TeammateStatus::Waiting
        );
    }

    #[test]
    fn status_strings_match_renderer() {
        assert_eq!(TeammateStatus::Idle.as_str(), "idle");
        assert_eq!(TeammateStatus::Working.as_str(), "working");
        assert_eq!(TeammateStatus::Waiting.as_str(), "waiting");
        assert_eq!(TeammateStatus::WaitingOnBot.as_str(), "waiting_on_bot");
        assert_eq!(TeammateStatus::Error.as_str(), "error");
    }

    #[test]
    fn resumable_picks_latest_open_session() {
        assert!(pick_resumable_session(&[]).is_none());
        assert!(pick_resumable_session(&[base("a")]).is_none());

        let sessions = [
            RosterSessionSnapshot {
                session_ended: false,
                last_activity_at: Some(1),
                ..base("old")
            },
            RosterSessionSnapshot {
                last_activity_at: Some(9),
                ..base("ended")
            },
            RosterSessionSnapshot {
                session_ended: false,
                last_activity_at: Some(5),
                ..base("new")
            },
        ];
        assert_eq!(
            pick_resumable_session(&sessions).map(|session| session.session_id.as_str()),
            Some("new")
        );
    }

    #[test]
    fn resumable_keeps_first_on_activity_ties() {
        let sessions = [
            RosterSessionSnapshot {
                session_ended: false,
                last_activity_at: Some(5),
                ..base("first")
            },
            RosterSessionSnapshot {
                session_ended: false,
                last_activity_at: Some(5),
                ..base("second")
            },
        ];
        assert_eq!(
            pick_resumable_session(&sessions).map(|session| session.session_id.as_str()),
            Some("first")
        );
    }
}
