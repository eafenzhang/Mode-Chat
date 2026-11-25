//! 群目标（轻量版，批次 H，spec `docs/specs/bot-group-goal.md`）。
//!
//! 纯域：无 IO、无时钟、无 Tauri 依赖。一个频道至多一个当前目标，阶段**按顺序**
//! 推进（done 前缀 + 至多一个 active + 其余 pending）。壳侧负责 `goal.json` 原子
//! 读写与命令接线；renderer 只渲染返回值。
//!
//! 与 DS-Bot 的差距（v1 非目标）：没有 goal-grill 对话草拟、证据校验、空闲自动推进，
//! 阶段也不与 `GroupTask` 强绑定；执行面继续复用现有任务树与流水线。

use serde::{Deserialize, Serialize};

pub const GOAL_SCHEMA_VERSION: u32 = 2;
/// 兼容读取的旧版本（v1 数据在内存里升级成 v2，下次写入落 v2）。
pub const LEGACY_GOAL_SCHEMA_VERSION: u32 = 1;
const MAX_STAGES: usize = 20;
const TITLE_MAX_CHARS: usize = 200;
const DESCRIPTION_MAX_CHARS: usize = 2_000;
const STAGE_TITLE_MAX_CHARS: usize = 120;
const STAGE_NOTE_MAX_CHARS: usize = 500;
/// 验收线 / 证据 / 打回原因的长度上限（与 DS `STAGE_ACCEPTANCE_MAX = 1000` 同量级）。
const ACCEPTANCE_MAX_CHARS: usize = 1_000;
const EVIDENCE_MAX_CHARS: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Paused,
    Completed,
    Stopped,
}

impl GoalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            GoalStatus::Active => "active",
            GoalStatus::Paused => "paused",
            GoalStatus::Completed => "completed",
            GoalStatus::Stopped => "stopped",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, GoalStatus::Completed | GoalStatus::Stopped)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    Pending,
    Active,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalStage {
    /// 创建时生成；校验要求非空且唯一。
    pub id: String,
    pub title: String,
    pub status: StageStatus,
    #[serde(default)]
    pub note: String,
    /// 这一阶段「做完的标准」（v2 新增；v1 数据为空）。[`ACCEPTANCE_MAX_CHARS`]
    #[serde(default)]
    pub acceptance: String,
    /// pass 时填的证据（命令输出/文件路径/截图说明）。[`EVIDENCE_MAX_CHARS`]
    #[serde(default)]
    pub evidence: String,
    /// pass 的时间（毫秒时间戳）；未通过为 None。
    #[serde(default)]
    pub passed_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupGoal {
    pub version: u32,
    pub status: GoalStatus,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// 整体「做完的样子」（v2 新增）。
    #[serde(default)]
    pub acceptance: String,
    #[serde(default)]
    pub stages: Vec<GoalStage>,
    pub created_at: u64,
    pub updated_at: u64,
    /// 最近一次打回的原因（v2 新增；用于 prompt 里的「上次打回」）。
    #[serde(default)]
    pub last_rework_note: String,
}

/// 壳命令 `bot_group_goal_apply` 的动作面。renderer 只构造动作，不做状态判断。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GoalAction {
    Create {
        title: String,
        #[serde(default)]
        description: String,
        /// 阶段标题；创建时至少一个，首阶段自动 active。
        #[serde(default)]
        stages: Vec<String>,
    },
    Update {
        title: String,
        #[serde(default)]
        description: String,
    },
    AddStage {
        title: String,
    },
    RemoveStage {
        stage_id: String,
    },
    RenameStage {
        stage_id: String,
        title: String,
    },
    SetStageNote {
        stage_id: String,
        note: String,
    },
    SetStageStatus {
        stage_id: String,
        status: StageStatus,
    },
    /// 设置验收线：`stage_id = None` 是目标级，`Some` 是阶段级（v2）。
    SetAcceptance {
        #[serde(default)]
        stage_id: Option<String>,
        acceptance: String,
    },
    /// 通过一个阶段（只接受当前进行中的阶段），并落证据（v2）。
    PassStage {
        stage_id: String,
        evidence: String,
    },
    /// 打回一个阶段（回到进行中），并记原因（v2）。
    ReworkStage {
        stage_id: String,
        note: String,
    },
    Pause,
    Resume,
    Complete,
    Stop,
    Clear,
}

fn trimmed(value: &str) -> &str {
    value.trim()
}

fn require_title(value: &str, field: &str, max_chars: usize) -> Result<String, String> {
    let value = trimmed(value);
    if value.is_empty() {
        return Err(format!("{field}不能为空"));
    }
    let length = value.chars().count();
    if length > max_chars {
        return Err(format!("{field}过长（{length}/{max_chars} 字符）"));
    }
    Ok(value.to_string())
}

/// 按顺序归一化阶段状态：done 前缀之后最多一个 active，其余 pending。
///
/// 同时拒绝「done 出现在未完成阶段之后」的顺序破坏（手工编辑文件也可能触发）。
fn normalize_stages(stages: &mut [GoalStage]) -> Result<(), String> {
    let mut open_seen = false;
    for stage in stages.iter() {
        match stage.status {
            StageStatus::Done => {
                if open_seen {
                    return Err(
                        "阶段状态必须按顺序：已完成阶段不能出现在未完成阶段之后".to_string()
                    );
                }
            }
            StageStatus::Pending | StageStatus::Active => open_seen = true,
        }
    }
    let first_open = stages
        .iter()
        .position(|stage| stage.status != StageStatus::Done);
    for (index, stage) in stages.iter_mut().enumerate() {
        if Some(index) == first_open {
            stage.status = StageStatus::Active;
        } else if stage.status == StageStatus::Active {
            stage.status = StageStatus::Pending;
        }
    }
    Ok(())
}

/// v1 → v2 内存迁移：只补字段，不动状态与时间戳。
///
/// 读路径（`src/group_goal.rs`）先迁移再校验，所以旧 `goal.json` 不用手改；
/// 下一次写操作才会落 v2 并留 `.v1.bak`。
pub fn migrate_goal(mut goal: GroupGoal) -> GroupGoal {
    if goal.version >= GOAL_SCHEMA_VERSION {
        return goal;
    }
    goal.version = GOAL_SCHEMA_VERSION;
    for stage in &mut goal.stages {
        if stage.acceptance.is_empty() {
            stage.acceptance = String::new();
        }
        if stage.evidence.is_empty() {
            stage.evidence = String::new();
        }
        if stage.passed_at.is_none() && stage.status == StageStatus::Done {
            // v1 里 done 没有时间：用目标更新时间兜底，保证 UI 能显示相对时间。
            stage.passed_at = Some(goal.updated_at);
        }
    }
    goal
}

/// 校验并归一化目标。损坏/越界输入返回可读错误，调用方决定拒绝写入还是按空处理。
pub fn validate_goal(goal: GroupGoal) -> Result<GroupGoal, String> {
    if goal.version != GOAL_SCHEMA_VERSION && goal.version != LEGACY_GOAL_SCHEMA_VERSION {
        return Err(format!(
            "goal version 必须为 {GOAL_SCHEMA_VERSION}（或兼容的 {LEGACY_GOAL_SCHEMA_VERSION}），实收 {}",
            goal.version
        ));
    }
    // 走到校验这一步一律按 v2 语义处理（只补字段，不重排状态）。
    let goal = migrate_goal(goal);
    let title = require_title(&goal.title, "目标标题", TITLE_MAX_CHARS)?;
    let description = trimmed(&goal.description).to_string();
    let description_length = description.chars().count();
    if description_length > DESCRIPTION_MAX_CHARS {
        return Err(format!(
            "目标说明过长（{description_length}/{DESCRIPTION_MAX_CHARS} 字符）"
        ));
    }
    let acceptance = trimmed(&goal.acceptance).to_string();
    if acceptance.chars().count() > ACCEPTANCE_MAX_CHARS {
        return Err(format!(
            "目标验收线过长（{}/{ACCEPTANCE_MAX_CHARS} 字符）",
            acceptance.chars().count()
        ));
    }
    if goal.stages.len() > MAX_STAGES {
        return Err(format!(
            "阶段数超限（{}/{}）",
            goal.stages.len(),
            MAX_STAGES
        ));
    }
    if goal.stages.is_empty() {
        return Err("目标至少需要一个阶段".to_string());
    }
    let mut stages = Vec::with_capacity(goal.stages.len());
    let mut seen_ids = std::collections::HashSet::new();
    for (index, stage) in goal.stages.into_iter().enumerate() {
        let id = trimmed(&stage.id);
        if id.is_empty() {
            return Err(format!("stage[{index}] 的 id 为空"));
        }
        if !seen_ids.insert(id.to_string()) {
            return Err(format!("阶段 id 重复：{id}"));
        }
        let title = require_title(
            &stage.title,
            &format!("stage[{index}] 标题"),
            STAGE_TITLE_MAX_CHARS,
        )?;
        let note = trimmed(&stage.note).to_string();
        let note_length = note.chars().count();
        if note_length > STAGE_NOTE_MAX_CHARS {
            return Err(format!(
                "stage[{index}] 备注过长（{note_length}/{STAGE_NOTE_MAX_CHARS} 字符）"
            ));
        }
        let stage_acceptance = trimmed(&stage.acceptance).to_string();
        if stage_acceptance.chars().count() > ACCEPTANCE_MAX_CHARS {
            return Err(format!(
                "stage[{index}] 验收线过长（{}/{ACCEPTANCE_MAX_CHARS} 字符）",
                stage_acceptance.chars().count()
            ));
        }
        let evidence = trimmed(&stage.evidence).to_string();
        if evidence.chars().count() > EVIDENCE_MAX_CHARS {
            return Err(format!(
                "stage[{index}] 证据过长（{}/{EVIDENCE_MAX_CHARS} 字符）",
                evidence.chars().count()
            ));
        }
        stages.push(GoalStage {
            id: id.to_string(),
            title,
            status: stage.status,
            note,
            acceptance: stage_acceptance,
            evidence,
            passed_at: stage.passed_at,
        });
    }
    normalize_stages(&mut stages)?;
    if goal.status == GoalStatus::Completed
        && stages.iter().any(|stage| stage.status != StageStatus::Done)
    {
        return Err("目标已完成，但仍有阶段未完成".to_string());
    }
    let last_rework_note = trimmed(&goal.last_rework_note).to_string();
    Ok(GroupGoal {
        version: GOAL_SCHEMA_VERSION,
        status: goal.status,
        title,
        description,
        acceptance,
        stages,
        created_at: goal.created_at,
        updated_at: goal.updated_at,
        last_rework_note,
    })
}

fn find_stage(stages: &mut [GoalStage], stage_id: &str) -> Result<usize, String> {
    let stage_id = trimmed(stage_id);
    if stage_id.is_empty() {
        return Err("stageId 不能为空".to_string());
    }
    stages
        .iter()
        .position(|stage| stage.id == stage_id)
        .ok_or_else(|| format!("阶段不存在：{stage_id}"))
}

fn stage_id_at(now: u64, index: usize) -> String {
    format!("s{now}-{index}")
}

fn require_editable(goal: &GroupGoal) -> Result<(), String> {
    if goal.status.is_terminal() {
        return Err(format!(
            "目标已{}，不能再修改；如需新目标请先清除",
            match goal.status {
                GoalStatus::Completed => "完成",
                GoalStatus::Stopped => "停止",
                _ => "结束",
            }
        ));
    }
    Ok(())
}

/// 应用一个动作；`Clear` 返回 `Ok(None)`，其余动作返回最新目标。
pub fn apply_goal_action(
    current: Option<GroupGoal>,
    action: GoalAction,
    now: u64,
) -> Result<Option<GroupGoal>, String> {
    if let GoalAction::Clear = action {
        return Ok(None);
    }
    if let GoalAction::Create {
        title,
        description,
        stages,
    } = action
    {
        if current.is_some() {
            return Err("已有群目标；请先清除当前目标再新建".to_string());
        }
        if stages.is_empty() {
            return Err("至少需要一个阶段".to_string());
        }
        if stages.len() > MAX_STAGES {
            return Err(format!("阶段数超限（{}/{MAX_STAGES}）", stages.len()));
        }
        let mut built = Vec::with_capacity(stages.len());
        for (index, stage_title) in stages.iter().enumerate() {
            let stage_title = require_title(stage_title, "阶段标题", STAGE_TITLE_MAX_CHARS)?;
            built.push(GoalStage {
                id: stage_id_at(now, index),
                title: stage_title,
                status: StageStatus::Pending,
                note: String::new(),
                acceptance: String::new(),
                evidence: String::new(),
                passed_at: None,
            });
        }
        let goal = validate_goal(GroupGoal {
            version: GOAL_SCHEMA_VERSION,
            status: GoalStatus::Active,
            title,
            description,
            acceptance: String::new(),
            stages: built,
            created_at: now,
            updated_at: now,
            last_rework_note: String::new(),
        })?;
        return Ok(Some(goal));
    }

    let mut goal = migrate_goal(current.ok_or_else(|| "当前没有群目标".to_string())?);
    require_editable(&goal)?;

    match action {
        GoalAction::Update { title, description } => {
            goal.title = title;
            goal.description = description;
        }
        GoalAction::AddStage { title } => {
            if goal.stages.len() >= MAX_STAGES {
                return Err(format!("阶段数已达上限 {MAX_STAGES}"));
            }
            let title = require_title(&title, "阶段标题", STAGE_TITLE_MAX_CHARS)?;
            let index = goal.stages.len();
            goal.stages.push(GoalStage {
                id: stage_id_at(now, index),
                title,
                status: StageStatus::Pending,
                note: String::new(),
                acceptance: String::new(),
                evidence: String::new(),
                passed_at: None,
            });
        }
        GoalAction::RemoveStage { stage_id } => {
            if goal.stages.len() <= 1 {
                return Err("至少保留一个阶段".to_string());
            }
            let index = find_stage(&mut goal.stages, &stage_id)?;
            goal.stages.remove(index);
        }
        GoalAction::RenameStage { stage_id, title } => {
            let index = find_stage(&mut goal.stages, &stage_id)?;
            goal.stages[index].title = title;
        }
        GoalAction::SetStageNote { stage_id, note } => {
            let index = find_stage(&mut goal.stages, &stage_id)?;
            goal.stages[index].note = note;
        }
        GoalAction::SetStageStatus { stage_id, status } => {
            let index = find_stage(&mut goal.stages, &stage_id)?;
            match status {
                StageStatus::Done => {
                    if goal.stages[..index]
                        .iter()
                        .any(|stage| stage.status != StageStatus::Done)
                    {
                        return Err("阶段必须按顺序完成：请先完成前面的阶段".to_string());
                    }
                }
                StageStatus::Pending => {
                    if goal.stages[index + 1..]
                        .iter()
                        .any(|stage| stage.status == StageStatus::Done)
                    {
                        return Err("不能重开：后面的阶段已完成，请先重开后面的阶段".to_string());
                    }
                }
                StageStatus::Active => {}
            }
            // 手工把阶段标 done 时也补时间戳（UI 相对时间才不会空）。
            if status == StageStatus::Done {
                goal.stages[index].passed_at = Some(now);
            }
            goal.stages[index].status = status;
        }
        GoalAction::SetAcceptance {
            stage_id,
            acceptance,
        } => {
            let acceptance = trimmed(&acceptance).to_string();
            if acceptance.chars().count() > ACCEPTANCE_MAX_CHARS {
                return Err(format!(
                    "验收线过长（{}/{ACCEPTANCE_MAX_CHARS} 字符）",
                    acceptance.chars().count()
                ));
            }
            match stage_id {
                None => goal.acceptance = acceptance,
                Some(stage_id) => {
                    let index = find_stage(&mut goal.stages, &stage_id)?;
                    goal.stages[index].acceptance = acceptance;
                }
            }
        }
        GoalAction::PassStage {
            stage_id,
            evidence,
        } => {
            let index = find_stage(&mut goal.stages, &stage_id)?;
            if goal.stages[index].status != StageStatus::Active {
                return Err("只有进行中的阶段可以通过".to_string());
            }
            let evidence = trimmed(&evidence).to_string();
            if evidence.is_empty() {
                return Err("证据不能为空".to_string());
            }
            if evidence.chars().count() > EVIDENCE_MAX_CHARS {
                return Err(format!(
                    "证据过长（{}/{EVIDENCE_MAX_CHARS} 字符）",
                    evidence.chars().count()
                ));
            }
            goal.stages[index].status = StageStatus::Done;
            goal.stages[index].evidence = evidence;
            goal.stages[index].passed_at = Some(now);
        }
        GoalAction::ReworkStage { stage_id, note } => {
            let index = find_stage(&mut goal.stages, &stage_id)?;
            if goal.stages[index].status == StageStatus::Pending {
                return Err("只有已开始或已完成的阶段可以打回".to_string());
            }
            let note = trimmed(&note).to_string();
            if note.is_empty() {
                return Err("打回原因不能为空".to_string());
            }
            if note.chars().count() > STAGE_NOTE_MAX_CHARS {
                return Err(format!(
                    "打回原因过长（{}/{STAGE_NOTE_MAX_CHARS} 字符）",
                    note.chars().count()
                ));
            }
            // 打回第 i 阶段 = 它自己回进行中，后面的阶段一起回待开始（顺序不变式）。
            goal.stages[index].status = StageStatus::Active;
            goal.stages[index].passed_at = None;
            goal.stages[index].evidence = String::new();
            goal.stages[index].note = note.clone();
            for later in goal.stages[index + 1..].iter_mut() {
                later.status = StageStatus::Pending;
                later.passed_at = None;
                later.evidence = String::new();
            }
            goal.last_rework_note = note;
        }
        GoalAction::Pause => {
            if goal.status != GoalStatus::Active {
                return Err("只有进行中的目标可以暂停".to_string());
            }
            goal.status = GoalStatus::Paused;
        }
        GoalAction::Resume => {
            if goal.status != GoalStatus::Paused {
                return Err("只有已暂停的目标可以继续".to_string());
            }
            goal.status = GoalStatus::Active;
        }
        GoalAction::Complete => {
            if goal
                .stages
                .iter()
                .any(|stage| stage.status != StageStatus::Done)
            {
                return Err("所有阶段完成后才能完成目标".to_string());
            }
            goal.status = GoalStatus::Completed;
        }
        GoalAction::Stop => {
            goal.status = GoalStatus::Stopped;
        }
        GoalAction::Create { .. } | GoalAction::Clear => unreachable!("已在上面处理"),
    }

    goal.updated_at = now;
    let goal = validate_goal(goal)?;
    Ok(Some(goal))
}

/// 目标进入 prompt 的文本。终态返回 None（不再干扰后续回合）。
pub fn render_goal_prompt(goal: &GroupGoal) -> Option<String> {
    if goal.status.is_terminal() {
        return None;
    }
    let done = goal
        .stages
        .iter()
        .filter(|stage| stage.status == StageStatus::Done)
        .count();
    let total = goal.stages.len();
    let current = goal
        .stages
        .iter()
        .find(|stage| stage.status == StageStatus::Active)
        .map(|stage| stage.title.as_str());
    let mut lines = vec![
        format!(
            "# 群目标（{}）",
            if goal.status == GoalStatus::Paused {
                "已暂停"
            } else {
                "进行中"
            }
        ),
        format!("目标：{}", goal.title),
    ];
    if !goal.description.trim().is_empty() {
        lines.push(goal.description.trim().to_string());
    }
    if !goal.acceptance.trim().is_empty() {
        lines.push(format!("验收线（整体）：{}", goal.acceptance.trim()));
    }
    lines.push(match current {
        Some(title) => format!("阶段进度：{done}/{total}，当前阶段「{title}」"),
        None => format!("阶段进度：{done}/{total}"),
    });
    for stage in &goal.stages {
        let mark = match stage.status {
            StageStatus::Done => "[x]",
            StageStatus::Active => "[→]",
            StageStatus::Pending => "[ ]",
        };
        lines.push(format!("- {mark} {}", stage.title));
        if !stage.acceptance.trim().is_empty() {
            lines.push(format!("  验收：{}", stage.acceptance.trim()));
        }
        if stage.status == StageStatus::Done && !stage.evidence.trim().is_empty() {
            lines.push(format!("  证据：{}", stage.evidence.trim()));
        }
    }
    if !goal.last_rework_note.trim().is_empty() {
        lines.push(format!("上次打回原因：{}", goal.last_rework_note.trim()));
    }
    lines.push(
        if goal.status == GoalStatus::Paused {
            "目标已暂停：不要自行推进阶段，按用户本轮指示处理。"
        } else {
            "请让本轮工作服务于当前阶段；阶段完成时用 team_goal_pass 提交证据（不要自己宣布通过）。"
        }
        .to_string(),
    );
    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(stages: &[&str]) -> GroupGoal {
        apply_goal_action(
            None,
            GoalAction::Create {
                title: "  发布 v1  ".into(),
                description: " 本周发版 ".into(),
                stages: stages.iter().map(|stage| stage.to_string()).collect(),
            },
            1_000,
        )
        .expect("create")
        .expect("goal")
    }

    fn stage_id(goal: &GroupGoal, index: usize) -> String {
        goal.stages[index].id.clone()
    }

    #[test]
    fn create_normalizes_first_stage_active() {
        let goal = create(&["调研", "实现", "验收"]);
        assert_eq!(goal.status, GoalStatus::Active);
        assert_eq!(goal.title, "发布 v1");
        assert_eq!(goal.stages.len(), 3);
        assert_eq!(goal.stages[0].status, StageStatus::Active);
        assert_eq!(goal.stages[1].status, StageStatus::Pending);
        assert!(goal.stages.iter().all(|stage| !stage.id.is_empty()));
    }

    #[test]
    fn done_advances_the_next_stage() {
        let goal = create(&["调研", "实现", "验收"]);
        let goal = apply_goal_action(
            Some(goal),
            GoalAction::SetStageStatus {
                stage_id: stage_id(&create(&["调研", "实现", "验收"]), 0),
                status: StageStatus::Done,
            },
            2_000,
        )
        .expect("done")
        .expect("goal");
        assert_eq!(goal.stages[0].status, StageStatus::Done);
        assert_eq!(goal.stages[1].status, StageStatus::Active);
        assert_eq!(goal.stages[2].status, StageStatus::Pending);
    }

    #[test]
    fn out_of_order_done_is_rejected() {
        let goal = create(&["调研", "实现", "验收"]);
        let error = apply_goal_action(
            Some(goal),
            GoalAction::SetStageStatus {
                stage_id: stage_id(&create(&["调研", "实现", "验收"]), 2),
                status: StageStatus::Done,
            },
            2_000,
        )
        .expect_err("must reject");
        assert!(error.contains("按顺序"));
    }

    #[test]
    fn reopen_requires_later_stages_first() {
        let goal = create(&["a", "b"]);
        let id0 = stage_id(&goal, 0);
        let id1 = stage_id(&goal, 1);
        let goal = apply_goal_action(
            Some(goal),
            GoalAction::SetStageStatus {
                stage_id: id0.clone(),
                status: StageStatus::Done,
            },
            2_000,
        )
        .unwrap()
        .unwrap();
        let goal = apply_goal_action(
            Some(goal),
            GoalAction::SetStageStatus {
                stage_id: id1.clone(),
                status: StageStatus::Done,
            },
            3_000,
        )
        .unwrap()
        .unwrap();
        let error = apply_goal_action(
            Some(goal.clone()),
            GoalAction::SetStageStatus {
                stage_id: id0,
                status: StageStatus::Pending,
            },
            4_000,
        )
        .expect_err("must reject while later stage done");
        assert!(error.contains("重开后"));
    }

    #[test]
    fn complete_requires_all_stages_done() {
        let goal = create(&["a", "b"]);
        let error = apply_goal_action(Some(goal.clone()), GoalAction::Complete, 2_000)
            .expect_err("must reject");
        assert!(error.contains("所有阶段"));

        let mut goal = goal;
        goal.stages[0].status = StageStatus::Done;
        goal.stages[1].status = StageStatus::Done;
        let goal = apply_goal_action(Some(goal), GoalAction::Complete, 3_000)
            .unwrap()
            .unwrap();
        assert_eq!(goal.status, GoalStatus::Completed);
        assert!(goal
            .stages
            .iter()
            .all(|stage| stage.status == StageStatus::Done));
    }

    #[test]
    fn terminal_goal_rejects_edits_but_allows_clear() {
        let goal = create(&["a"]);
        let goal = apply_goal_action(Some(goal), GoalAction::Stop, 2_000)
            .unwrap()
            .unwrap();
        assert_eq!(goal.status, GoalStatus::Stopped);
        assert!(apply_goal_action(
            Some(goal.clone()),
            GoalAction::AddStage { title: "b".into() },
            3_000
        )
        .is_err());
        assert_eq!(
            apply_goal_action(Some(goal), GoalAction::Clear, 4_000).unwrap(),
            None
        );
    }

    #[test]
    fn create_rejects_existing_goal_and_caps_stages() {
        let existing = create(&["a"]);
        assert!(apply_goal_action(
            Some(existing.clone()),
            GoalAction::Create {
                title: "另一个".into(),
                description: String::new(),
                stages: vec!["x".into()],
            },
            2_000
        )
        .is_err());
        let stages: Vec<String> = (0..=MAX_STAGES).map(|index| format!("s{index}")).collect();
        assert!(apply_goal_action(
            Some(existing),
            GoalAction::AddStage {
                title: "extra".into()
            },
            2_000
        )
        .is_ok());
        let mut goal = create(&["a"]);
        goal.stages = stages
            .iter()
            .enumerate()
            .map(|(index, title)| GoalStage {
                id: format!("id{index}"),
                title: title.clone(),
                status: if index == 0 {
                    StageStatus::Active
                } else {
                    StageStatus::Pending
                },
                note: String::new(),
                acceptance: String::new(),
                evidence: String::new(),
                passed_at: None,
            })
            .collect();
        assert!(validate_goal(goal).is_err());
    }

    #[test]
    fn remove_last_stage_is_rejected() {
        let goal = create(&["only"]);
        let id = stage_id(&goal, 0);
        let error = apply_goal_action(Some(goal), GoalAction::RemoveStage { stage_id: id }, 2_000)
            .expect_err("must reject");
        assert!(error.contains("至少保留"));
    }

    #[test]
    fn pause_and_resume_keep_stage_states() {
        let goal = create(&["a", "b"]);
        let goal = apply_goal_action(Some(goal), GoalAction::Pause, 2_000)
            .unwrap()
            .unwrap();
        assert_eq!(goal.status, GoalStatus::Paused);
        assert_eq!(goal.stages[0].status, StageStatus::Active);
        let goal = apply_goal_action(Some(goal), GoalAction::Resume, 3_000)
            .unwrap()
            .unwrap();
        assert_eq!(goal.status, GoalStatus::Active);
    }

    #[test]
    fn prompt_renders_progress_and_omits_terminal_goals() {
        let goal = create(&["调研", "实现"]);
        let rendered = render_goal_prompt(&goal).expect("active goal");
        assert!(rendered.contains("# 群目标（进行中）"));
        assert!(rendered.contains("阶段进度：0/2，当前阶段「调研」"));
        assert!(rendered.contains("- [→] 调研"));
        assert!(rendered.contains("- [ ] 实现"));

        let paused = apply_goal_action(Some(goal), GoalAction::Pause, 2_000)
            .unwrap()
            .unwrap();
        let rendered = render_goal_prompt(&paused).expect("paused goal");
        assert!(rendered.contains("# 群目标（已暂停）"));
        assert!(rendered.contains("不要自行推进阶段"));

        let stopped = apply_goal_action(Some(paused), GoalAction::Stop, 3_000)
            .unwrap()
            .unwrap();
        assert!(render_goal_prompt(&stopped).is_none());
    }

    #[test]
    fn action_round_trips_through_json() {
        let action: GoalAction =
            serde_json::from_str(r#"{"kind":"setStageStatus","stageId":"s1","status":"done"}"#)
                .expect("deserialize");
        assert_eq!(
            action,
            GoalAction::SetStageStatus {
                stage_id: "s1".into(),
                status: StageStatus::Done,
            }
        );
        let serialized = serde_json::to_string(&GoalAction::AddStage {
            title: "验收".into(),
        })
        .unwrap();
        assert_eq!(serialized, r#"{"kind":"addStage","title":"验收"}"#);
    }

    #[test]
    fn v1_goal_migrates_to_v2_without_losing_stages() {
        let v1 = r#"{
            "version": 1,
            "status": "active",
            "title": "发版",
            "description": "一期",
            "stages": [
                {"id": "s1", "title": "调研", "status": "done", "note": "已核"},
                {"id": "s2", "title": "实现", "status": "active", "note": ""}
            ],
            "createdAt": 1000,
            "updatedAt": 2000
        }"#;
        let goal: GroupGoal = serde_json::from_str(v1).expect("v1 要能解析");
        let migrated = migrate_goal(goal);
        assert_eq!(migrated.version, GOAL_SCHEMA_VERSION);
        assert_eq!(migrated.stages.len(), 2);
        assert_eq!(migrated.stages[0].title, "调研");
        // v1 的 done 阶段补上时间戳，UI 才能显示相对时间。
        assert_eq!(migrated.stages[0].passed_at, Some(2000));
        assert!(migrated.stages[1].acceptance.is_empty());
        let validated = validate_goal(migrated).expect("迁移后要能过校验");
        assert_eq!(validated.version, 2);
        assert_eq!(validated.stages[1].status, StageStatus::Active);
    }

    #[test]
    fn acceptance_and_pass_require_active_stage() {
        let goal = create(&["设计", "实现"]);
        let first = stage_id(&goal, 0);
        let second = stage_id(&goal, 1);
        // 目标级 + 阶段级验收线
        let goal = apply_goal_action(
            Some(goal),
            GoalAction::SetAcceptance {
                stage_id: None,
                acceptance: "发版到 v0.5.0".into(),
            },
            2_000,
        )
        .unwrap()
        .unwrap();
        assert_eq!(goal.acceptance, "发版到 v0.5.0");
        let goal = apply_goal_action(
            Some(goal),
            GoalAction::SetAcceptance {
                stage_id: Some(first.clone()),
                acceptance: "cargo test 全绿".into(),
            },
            2_100,
        )
        .unwrap()
        .unwrap();
        assert_eq!(goal.stages[0].acceptance, "cargo test 全绿");
        // 证据为空必须拒绝
        assert!(
            apply_goal_action(
                Some(goal.clone()),
                GoalAction::PassStage {
                    stage_id: first.clone(),
                    evidence: "  ".into(),
                },
                2_200
            )
            .is_err()
        );
        // 非进行中阶段不能 pass
        assert!(
            apply_goal_action(
                Some(goal.clone()),
                GoalAction::PassStage {
                    stage_id: second.clone(),
                    evidence: "ok".into(),
                },
                2_300
            )
            .is_err()
        );
        let goal = apply_goal_action(
            Some(goal),
            GoalAction::PassStage {
                stage_id: first.clone(),
                evidence: "cargo test: 130 passed".into(),
            },
            2_400,
        )
        .unwrap()
        .unwrap();
        assert_eq!(goal.stages[0].status, StageStatus::Done);
        assert_eq!(goal.stages[0].evidence, "cargo test: 130 passed");
        assert_eq!(goal.stages[0].passed_at, Some(2_400));
        // pass 后推进到下一阶段
        assert_eq!(goal.stages[1].status, StageStatus::Active);
    }

    #[test]
    fn rework_reopens_stage_and_resets_later_stages() {
        let goal = create(&["a", "b"]);
        let first = stage_id(&goal, 0);
        let goal = apply_goal_action(
            Some(goal),
            GoalAction::PassStage {
                stage_id: first.clone(),
                evidence: "done".into(),
            },
            2_000,
        )
        .unwrap()
        .unwrap();
        assert_eq!(goal.stages[1].status, StageStatus::Active);
        // 空原因拒绝
        assert!(
            apply_goal_action(
                Some(goal.clone()),
                GoalAction::ReworkStage {
                    stage_id: first.clone(),
                    note: " ".into(),
                },
                2_100
            )
            .is_err()
        );
        let goal = apply_goal_action(
            Some(goal),
            GoalAction::ReworkStage {
                stage_id: first.clone(),
                note: "缺少回归测试".into(),
            },
            2_200,
        )
        .unwrap()
        .unwrap();
        assert_eq!(goal.stages[0].status, StageStatus::Active);
        assert_eq!(goal.stages[0].passed_at, None);
        assert!(goal.stages[0].evidence.is_empty());
        assert_eq!(goal.stages[1].status, StageStatus::Pending);
        assert_eq!(goal.last_rework_note, "缺少回归测试");
        let rendered = render_goal_prompt(&goal).expect("active goal");
        assert!(rendered.contains("上次打回原因：缺少回归测试"));
    }
}
