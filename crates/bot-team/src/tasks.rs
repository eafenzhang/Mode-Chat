//! 群任务状态机（N6 W1，spec `docs/specs/bot-group-tasks.md`）。
//!
//! 纯域：无 IO、无时钟、无 Tauri 依赖。任务由 `GroupTask::create` 建立，动作经
//! [`apply_transition`] 迁移；`TaskProjection` 负责事件回放（壳侧 JSONL repo 逐行喂入）。
//! 路由 v1 是确定性规则（@提及 → 显式收件人 → 全群唯一 open task → 管理员）。

use std::collections::BTreeMap;
use std::fmt;

/// tasks.jsonl 事件 schema 版本（写入事件文件时由壳侧标注/迁移用）。
pub const TASK_SCHEMA_VERSION: u32 = 1;

/// 群回合根任务（容器）的合成 owner：不对应真实成员，避免与成员任务抢 Running 槽位。
pub const GROUP_ROOT_OWNER: &str = "__group__";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    Waiting,
    Stopped,
    Done,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskState::Done | TaskState::Failed | TaskState::Cancelled)
    }

    /// open = 仍可能继续推进（不含 Stopped；Stopped 需要显式 resume）。
    pub fn is_open(self) -> bool {
        matches!(
            self,
            TaskState::Queued | TaskState::Running | TaskState::Waiting
        )
    }

    /// 只有 Running 占全局 turn gate 槽位；Waiting 等依赖不占。
    pub fn occupies_turn_slot(self) -> bool {
        self == TaskState::Running
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Queued => "queued",
            TaskState::Running => "running",
            TaskState::Waiting => "waiting",
            TaskState::Stopped => "stopped",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
            TaskState::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupTask {
    /// 创建时生成，幂等键。
    pub id: String,
    pub group_id: String,
    /// 必填：没有用户请求不建任务。
    pub root_message_id: String,
    pub parent_task_id: Option<String>,
    pub owner_member_id: String,
    pub state: TaskState,
    /// 只有改派 +1；结果写回必须匹配。
    pub revision: u32,
    pub expected_result: String,
    pub result_message_id: Option<String>,
    pub created_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCreateInput {
    pub id: String,
    pub group_id: String,
    pub root_message_id: String,
    pub parent_task_id: Option<String>,
    pub owner_member_id: String,
    pub expected_result: String,
    pub created_at: u64,
}

impl GroupTask {
    pub fn create(input: TaskCreateInput) -> Result<Self, TaskError> {
        let non_empty = |value: &str, field: &str| -> Result<(), TaskError> {
            if value.trim().is_empty() {
                return Err(TaskError::Invalid(format!("{field} 不能为空")));
            }
            Ok(())
        };
        non_empty(&input.id, "task.id")?;
        non_empty(&input.group_id, "task.groupId")?;
        non_empty(&input.root_message_id, "task.rootMessageId")?;
        non_empty(&input.owner_member_id, "task.ownerMemberId")?;
        if let Some(parent) = &input.parent_task_id {
            non_empty(parent, "task.parentTaskId")?;
        }
        Ok(Self {
            id: input.id,
            group_id: input.group_id,
            root_message_id: input.root_message_id,
            parent_task_id: input.parent_task_id,
            owner_member_id: input.owner_member_id,
            state: TaskState::Queued,
            revision: 0,
            expected_result: input.expected_result,
            result_message_id: None,
            created_at: input.created_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskTransition {
    Dispatch,
    /// `expected_revision` 不符 → 拒绝（迟到结果）。
    Settle {
        expected_revision: u32,
        ok: bool,
        result_message_id: Option<String>,
    },
    Wait,
    /// 子任务全部终态后唤醒父任务（Waiting → Queued；不改变 revision）。
    Wake,
    Stop,
    Resume,
    Reassign {
        owner_member_id: String,
    },
    Cancel,
}

impl TaskTransition {
    fn action(&self) -> &'static str {
        match self {
            TaskTransition::Dispatch => "dispatch",
            TaskTransition::Settle { .. } => "settle",
            TaskTransition::Wait => "wait",
            TaskTransition::Wake => "wake",
            TaskTransition::Stop => "stop",
            TaskTransition::Resume => "resume",
            TaskTransition::Reassign { .. } => "reassign",
            TaskTransition::Cancel => "cancel",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskError {
    Invalid(String),
    NotFound(String),
    InvalidTransition {
        task_id: String,
        from: TaskState,
        action: &'static str,
    },
    RevisionMismatch {
        task_id: String,
        expected: u32,
        actual: u32,
    },
    DuplicateRunning {
        group_id: String,
        member_id: String,
    },
}

impl fmt::Display for TaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaskError::Invalid(message) => write!(formatter, "任务数据非法：{message}"),
            TaskError::NotFound(task_id) => write!(formatter, "任务 {task_id} 不存在"),
            TaskError::InvalidTransition {
                task_id,
                from,
                action,
            } => write!(
                formatter,
                "任务 {task_id} 不能从 {} 执行 {action}",
                from.as_str()
            ),
            TaskError::RevisionMismatch {
                task_id,
                expected,
                actual,
            } => write!(
                formatter,
                "任务 {task_id} revision 不匹配：期望 {expected}，实际 {actual}"
            ),
            TaskError::DuplicateRunning {
                group_id,
                member_id,
            } => write!(
                formatter,
                "成员 {member_id} 在频道 {group_id} 已有运行中的任务"
            ),
        }
    }
}

/// 单步迁移。调用方负责 Running 并发等需要投影上下文的不变量。
pub fn apply_transition(
    task: &GroupTask,
    transition: &TaskTransition,
) -> Result<GroupTask, TaskError> {
    let invalid = |state: TaskState| TaskError::InvalidTransition {
        task_id: task.id.clone(),
        from: state,
        action: transition.action(),
    };
    let mut next = task.clone();
    match transition {
        TaskTransition::Dispatch => {
            if task.state != TaskState::Queued {
                return Err(invalid(task.state));
            }
            next.state = TaskState::Running;
        }
        TaskTransition::Settle {
            expected_revision,
            ok,
            result_message_id,
        } => {
            if task.state != TaskState::Running {
                return Err(invalid(task.state));
            }
            if task.revision != *expected_revision {
                return Err(TaskError::RevisionMismatch {
                    task_id: task.id.clone(),
                    expected: *expected_revision,
                    actual: task.revision,
                });
            }
            next.state = if *ok {
                TaskState::Done
            } else {
                TaskState::Failed
            };
            next.result_message_id = result_message_id.clone();
        }
        TaskTransition::Wait => {
            if task.state != TaskState::Running {
                return Err(invalid(task.state));
            }
            next.state = TaskState::Waiting;
        }
        TaskTransition::Wake => {
            if task.state != TaskState::Waiting {
                return Err(invalid(task.state));
            }
            next.state = TaskState::Queued;
        }
        TaskTransition::Stop => {
            if !matches!(
                task.state,
                TaskState::Queued | TaskState::Running | TaskState::Waiting
            ) {
                return Err(invalid(task.state));
            }
            next.state = TaskState::Stopped;
        }
        TaskTransition::Resume => {
            if task.state != TaskState::Stopped {
                return Err(invalid(task.state));
            }
            next.state = TaskState::Queued;
        }
        TaskTransition::Reassign { owner_member_id } => {
            if owner_member_id.trim().is_empty() {
                return Err(TaskError::Invalid("reassign 的 ownerMemberId 不能为空".into()));
            }
            if !matches!(
                task.state,
                TaskState::Stopped | TaskState::Failed | TaskState::Waiting
            ) {
                return Err(invalid(task.state));
            }
            next.owner_member_id = owner_member_id.clone();
            next.state = TaskState::Queued;
            next.revision = task.revision.saturating_add(1);
            next.result_message_id = None;
        }
        TaskTransition::Cancel => {
            if !matches!(
                task.state,
                TaskState::Queued
                    | TaskState::Running
                    | TaskState::Waiting
                    | TaskState::Stopped
                    | TaskState::Failed
            ) {
                return Err(invalid(task.state));
            }
            next.state = TaskState::Cancelled;
        }
    }
    Ok(next)
}

/// tasks.jsonl 的一行。事件里的 `state` 是**迁移后的状态**。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum TaskEvent {
    Created {
        at: u64,
        task: GroupTask,
    },
    Dispatched {
        at: u64,
        task_id: String,
        state: TaskState,
        revision: u32,
    },
    Settled {
        at: u64,
        task_id: String,
        state: TaskState,
        revision: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_message_id: Option<String>,
    },
    Waiting {
        at: u64,
        task_id: String,
        state: TaskState,
        revision: u32,
    },
    Woken {
        at: u64,
        task_id: String,
        state: TaskState,
        revision: u32,
    },
    Stopped {
        at: u64,
        task_id: String,
        state: TaskState,
        revision: u32,
    },
    Resumed {
        at: u64,
        task_id: String,
        state: TaskState,
        revision: u32,
    },
    Reassigned {
        at: u64,
        task_id: String,
        state: TaskState,
        revision: u32,
        owner_member_id: String,
    },
    Cancelled {
        at: u64,
        task_id: String,
        state: TaskState,
        revision: u32,
    },
    /// 未知事件（前向兼容）：壳侧忽略，不影响已有投影。
    #[serde(other)]
    Unknown,
}

impl TaskEvent {
    pub fn at(&self) -> u64 {
        match self {
            TaskEvent::Created { at, .. }
            | TaskEvent::Dispatched { at, .. }
            | TaskEvent::Settled { at, .. }
            | TaskEvent::Waiting { at, .. }
            | TaskEvent::Woken { at, .. }
            | TaskEvent::Stopped { at, .. }
            | TaskEvent::Resumed { at, .. }
            | TaskEvent::Reassigned { at, .. }
            | TaskEvent::Cancelled { at, .. } => *at,
            TaskEvent::Unknown => 0,
        }
    }

    pub fn task_id(&self) -> Option<&str> {
        match self {
            TaskEvent::Created { task, .. } => Some(task.id.as_str()),
            TaskEvent::Dispatched { task_id, .. }
            | TaskEvent::Settled { task_id, .. }
            | TaskEvent::Waiting { task_id, .. }
            | TaskEvent::Woken { task_id, .. }
            | TaskEvent::Stopped { task_id, .. }
            | TaskEvent::Resumed { task_id, .. }
            | TaskEvent::Reassigned { task_id, .. }
            | TaskEvent::Cancelled { task_id, .. } => Some(task_id.as_str()),
            TaskEvent::Unknown => None,
        }
    }
}

/// 事件回放后的任务投影（壳侧唯一读取面）。
#[derive(Debug, Clone, Default)]
pub struct TaskProjection {
    tasks: BTreeMap<String, GroupTask>,
}

impl TaskProjection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub fn get(&self, task_id: &str) -> Result<&GroupTask, TaskError> {
        self.tasks
            .get(task_id)
            .ok_or_else(|| TaskError::NotFound(task_id.to_string()))
    }

    pub fn tasks(&self) -> impl Iterator<Item = &GroupTask> {
        self.tasks.values()
    }

    pub fn open_tasks_for_group(&self, group_id: &str) -> Vec<&GroupTask> {
        self.tasks
            .values()
            .filter(|task| task.group_id == group_id && task.state.is_open())
            .collect()
    }

    pub fn unique_open_task_for_group(&self, group_id: &str) -> Option<&GroupTask> {
        let open = self.open_tasks_for_group(group_id);
        if open.len() == 1 {
            open.first().copied()
        } else {
            None
        }
    }

    pub fn running_for(&self, group_id: &str, member_id: &str) -> Option<&GroupTask> {
        self.tasks.values().find(|task| {
            task.group_id == group_id
                && task.owner_member_id == member_id
                && task.state == TaskState::Running
        })
    }

    /// 回放一行事件。未知事件忽略；非法迁移返回错误（壳侧跳过坏行并 warning）。
    pub fn apply(&mut self, event: &TaskEvent) -> Result<(), TaskError> {
        match event {
            TaskEvent::Created { task, .. } => {
                if task.id.trim().is_empty() {
                    return Err(TaskError::Invalid("task.id 不能为空".into()));
                }
                if task.state != TaskState::Queued {
                    return Err(TaskError::Invalid(format!(
                        "created 事件的初始状态必须是 queued，实际 {}",
                        task.state.as_str()
                    )));
                }
                if self.tasks.contains_key(&task.id) {
                    return Err(TaskError::Invalid(format!("重复创建任务 {}", task.id)));
                }
                if let Some(parent_id) = &task.parent_task_id {
                    let parent = self.tasks.get(parent_id).ok_or_else(|| {
                        TaskError::NotFound(format!("{}（父任务）", parent_id))
                    })?;
                    if parent.group_id != task.group_id {
                        return Err(TaskError::Invalid(format!(
                            "任务 {} 与父任务 {} 不在同一频道",
                            task.id, parent.id
                        )));
                    }
                }
                self.tasks.insert(task.id.clone(), task.clone());
                Ok(())
            }
            TaskEvent::Dispatched {
                task_id,
                state,
                revision,
                ..
            } => {
                let current = self.get(task_id)?.clone();
                if current.revision != *revision {
                    return Err(TaskError::RevisionMismatch {
                        task_id: task_id.clone(),
                        expected: *revision,
                        actual: current.revision,
                    });
                }
                if current.state == TaskState::Queued
                    && self
                        .running_for(&current.group_id, &current.owner_member_id)
                        .is_some()
                {
                    return Err(TaskError::DuplicateRunning {
                        group_id: current.group_id.clone(),
                        member_id: current.owner_member_id.clone(),
                    });
                }
                self.apply_checked(&current, state, TaskTransition::Dispatch)
            }
            TaskEvent::Settled {
                task_id,
                state,
                revision,
                result_message_id,
                ..
            } => {
                let current = self.get(task_id)?.clone();
                let ok = match state {
                    TaskState::Done => true,
                    TaskState::Failed => false,
                    other => {
                        return Err(TaskError::Invalid(format!(
                            "settled 事件状态只能是 done/failed，实际 {}",
                            other.as_str()
                        )))
                    }
                };
                self.apply_checked(
                    &current,
                    state,
                    TaskTransition::Settle {
                        expected_revision: *revision,
                        ok,
                        result_message_id: result_message_id.clone(),
                    },
                )
            }
            TaskEvent::Waiting {
                task_id,
                state,
                revision,
                ..
            } => {
                let current = self.get(task_id)?.clone();
                self.check_revision(&current, *revision)?;
                self.apply_checked(&current, state, TaskTransition::Wait)
            }
            TaskEvent::Woken {
                task_id,
                state,
                revision,
                ..
            } => {
                let current = self.get(task_id)?.clone();
                self.check_revision(&current, *revision)?;
                self.apply_checked(&current, state, TaskTransition::Wake)
            }
            TaskEvent::Stopped {
                task_id,
                state,
                revision,
                ..
            } => {
                let current = self.get(task_id)?.clone();
                self.check_revision(&current, *revision)?;
                self.apply_checked(&current, state, TaskTransition::Stop)
            }
            TaskEvent::Resumed {
                task_id,
                state,
                revision,
                ..
            } => {
                let current = self.get(task_id)?.clone();
                self.check_revision(&current, *revision)?;
                self.apply_checked(&current, state, TaskTransition::Resume)
            }
            TaskEvent::Reassigned {
                task_id,
                state,
                revision,
                owner_member_id,
                ..
            } => {
                let current = self.get(task_id)?.clone();
                let expected_revision = current.revision.saturating_add(1);
                if *revision != expected_revision {
                    return Err(TaskError::Invalid(format!(
                        "reassigned 事件 revision 应为 {expected_revision}，实际 {revision}"
                    )));
                }
                self.apply_checked(
                    &current,
                    state,
                    TaskTransition::Reassign {
                        owner_member_id: owner_member_id.clone(),
                    },
                )
            }
            TaskEvent::Cancelled {
                task_id,
                state,
                revision,
                ..
            } => {
                let current = self.get(task_id)?.clone();
                self.check_revision(&current, *revision)?;
                self.apply_checked(&current, state, TaskTransition::Cancel)
            }
            TaskEvent::Unknown => Ok(()),
        }
    }

    fn check_revision(&self, task: &GroupTask, revision: u32) -> Result<(), TaskError> {
        if task.revision != revision {
            return Err(TaskError::RevisionMismatch {
                task_id: task.id.clone(),
                expected: revision,
                actual: task.revision,
            });
        }
        Ok(())
    }

    fn apply_checked(
        &mut self,
        current: &GroupTask,
        expected_state: &TaskState,
        transition: TaskTransition,
    ) -> Result<(), TaskError> {
        let next = apply_transition(current, &transition)?;
        if next.state != *expected_state {
            return Err(TaskError::Invalid(format!(
                "{} 事件状态应为 {}，实际 {}",
                transition.action(),
                next.state.as_str(),
                expected_state.as_str()
            )));
        }
        self.tasks.insert(next.id.clone(), next);
        Ok(())
    }

    /// 子树后序（子任务先于父任务）；根任务不存在 → NotFound。
    pub fn subtree_ids(&self, root_id: &str) -> Result<Vec<String>, TaskError> {
        self.get(root_id)?;
        let mut children: BTreeMap<&str, Vec<&GroupTask>> = BTreeMap::new();
        for task in self.tasks.values() {
            if let Some(parent_id) = task.parent_task_id.as_deref() {
                children.entry(parent_id).or_default().push(task);
            }
        }
        let mut ordered = Vec::new();
        fn visit<'a>(
            task_id: &str,
            children: &BTreeMap<&'a str, Vec<&'a GroupTask>>,
            ordered: &mut Vec<String>,
        ) {
            if let Some(items) = children.get(task_id) {
                for child in items {
                    visit(&child.id, children, ordered);
                }
            }
            ordered.push(task_id.to_string());
        }
        visit(root_id, &children, &mut ordered);
        Ok(ordered)
    }

    /// 重启恢复：所有子任务都已离开 open 状态的 Waiting 父任务 → Woken（Queued）。
    ///
    /// 只负责重算依赖，不自动派发；父任务回到 Queued 后由 UI/下一次调度决定继续或取消。
    /// 空子列表（损坏/旧数据）也视为依赖已满足，避免永久 Waiting。
    pub fn recovery_wake_events(&self, now: u64) -> Vec<TaskEvent> {
        let mut current = self.clone();
        let mut events = Vec::new();
        loop {
            let mut progressed = false;
            let waiting: Vec<GroupTask> = current
                .tasks()
                .filter(|task| task.state == TaskState::Waiting)
                .cloned()
                .collect();
            for task in waiting {
                let all_settled = current
                    .tasks()
                    .filter(|child| child.parent_task_id.as_deref() == Some(task.id.as_str()))
                    .all(|child| !child.state.is_open());
                if !all_settled {
                    continue;
                }
                let event = TaskEvent::Woken {
                    at: now,
                    task_id: task.id.clone(),
                    state: TaskState::Queued,
                    revision: task.revision,
                };
                if current.apply(&event).is_ok() {
                    events.push(event);
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }
        events
    }

    /// 级联取消事件（后序）。Done/Cancelled 子孙跳过；根任务已终态 → 报错。
    pub fn cancel_subtree_events(
        &self,
        root_id: &str,
        now: u64,
    ) -> Result<Vec<TaskEvent>, TaskError> {
        let root = self.get(root_id)?;
        if root.state.is_terminal() {
            return Err(TaskError::InvalidTransition {
                task_id: root.id.clone(),
                from: root.state,
                action: "cancel",
            });
        }
        let mut events = Vec::new();
        for task_id in self.subtree_ids(root_id)? {
            let task = self.get(&task_id)?;
            if matches!(task.state, TaskState::Done | TaskState::Cancelled) {
                continue;
            }
            events.push(TaskEvent::Cancelled {
                at: now,
                task_id: task.id.clone(),
                state: TaskState::Cancelled,
                revision: task.revision,
            });
        }
        Ok(events)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskRouteInput<'a> {
    pub mentioned_ids: &'a [String],
    pub explicit_recipient: Option<&'a str>,
    pub open_tasks: &'a [GroupTask],
    pub admin_id: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskRoute {
    Mentioned(Vec<String>),
    Explicit(String),
    UniqueOpenTask { task_id: String, member_id: String },
    Admin(String),
    Unrouted,
}

/// 路由 v1：@提及（按出现顺序）→ 显式收件人 → 全群唯一 open task → 管理员。
pub fn route_task(input: &TaskRouteInput<'_>) -> TaskRoute {
    if !input.mentioned_ids.is_empty() {
        return TaskRoute::Mentioned(input.mentioned_ids.to_vec());
    }
    if let Some(recipient) = input
        .explicit_recipient
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return TaskRoute::Explicit(recipient.to_string());
    }
    if let [task] = input.open_tasks {
        return TaskRoute::UniqueOpenTask {
            task_id: task.id.clone(),
            member_id: task.owner_member_id.clone(),
        };
    }
    if let Some(admin) = input.admin_id.map(str::trim).filter(|value| !value.is_empty()) {
        return TaskRoute::Admin(admin.to_string());
    }
    TaskRoute::Unrouted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str) -> GroupTask {
        GroupTask::create(TaskCreateInput {
            id: id.to_string(),
            group_id: "g1".to_string(),
            root_message_id: "m1".to_string(),
            parent_task_id: None,
            owner_member_id: "m1".to_string(),
            expected_result: "结果".to_string(),
            created_at: 1,
        })
        .unwrap()
    }

    fn task_with(id: &str, state: TaskState, revision: u32) -> GroupTask {
        GroupTask {
            state,
            revision,
            ..task(id)
        }
    }

    fn child(id: &str, parent: &str) -> GroupTask {
        GroupTask::create(TaskCreateInput {
            id: id.to_string(),
            group_id: "g1".to_string(),
            root_message_id: "m1".to_string(),
            parent_task_id: Some(parent.to_string()),
            owner_member_id: "m2".to_string(),
            expected_result: "结果".to_string(),
            created_at: 1,
        })
        .unwrap()
    }

    #[test]
    fn transition_table_allows_exactly_the_specified_states() {
        let all = [
            TaskState::Queued,
            TaskState::Running,
            TaskState::Waiting,
            TaskState::Stopped,
            TaskState::Done,
            TaskState::Failed,
            TaskState::Cancelled,
        ];
        struct Case {
            transition: TaskTransition,
            allowed: &'static [TaskState],
            next: TaskState,
        }
        let cases = [
            Case {
                transition: TaskTransition::Dispatch,
                allowed: &[TaskState::Queued],
                next: TaskState::Running,
            },
            Case {
                transition: TaskTransition::Wait,
                allowed: &[TaskState::Running],
                next: TaskState::Waiting,
            },
            Case {
                transition: TaskTransition::Wake,
                allowed: &[TaskState::Waiting],
                next: TaskState::Queued,
            },
            Case {
                transition: TaskTransition::Settle {
                    expected_revision: 0,
                    ok: true,
                    result_message_id: Some("r1".into()),
                },
                allowed: &[TaskState::Running],
                next: TaskState::Done,
            },
            Case {
                transition: TaskTransition::Stop,
                allowed: &[TaskState::Queued, TaskState::Running, TaskState::Waiting],
                next: TaskState::Stopped,
            },
            Case {
                transition: TaskTransition::Resume,
                allowed: &[TaskState::Stopped],
                next: TaskState::Queued,
            },
            Case {
                transition: TaskTransition::Reassign {
                    owner_member_id: "m9".into(),
                },
                allowed: &[TaskState::Stopped, TaskState::Failed, TaskState::Waiting],
                next: TaskState::Queued,
            },
            Case {
                transition: TaskTransition::Cancel,
                allowed: &[
                    TaskState::Queued,
                    TaskState::Running,
                    TaskState::Waiting,
                    TaskState::Stopped,
                    TaskState::Failed,
                ],
                next: TaskState::Cancelled,
            },
        ];
        for case in cases {
            for state in all {
                let current = task_with("t1", state, 0);
                let result = apply_transition(&current, &case.transition);
                if case.allowed.contains(&state) {
                    let next = result.unwrap_or_else(|error| {
                        panic!(
                            "{} 从 {} 应允许：{error}",
                            case.transition.action(),
                            state.as_str()
                        )
                    });
                    assert_eq!(next.state, case.next);
                } else {
                    assert!(
                        matches!(result, Err(TaskError::InvalidTransition { .. })),
                        "{} 从 {} 必须拒绝",
                        case.transition.action(),
                        state.as_str()
                    );
                }
            }
        }
    }

    #[test]
    fn settle_rejects_stale_revision_and_terminal_is_immutable() {
        let running = task_with("t1", TaskState::Running, 2);
        let stale = apply_transition(
            &running,
            &TaskTransition::Settle {
                expected_revision: 1,
                ok: true,
                result_message_id: None,
            },
        );
        assert!(matches!(stale, Err(TaskError::RevisionMismatch { .. })));

        let done = apply_transition(
            &running,
            &TaskTransition::Settle {
                expected_revision: 2,
                ok: true,
                result_message_id: Some("r1".into()),
            },
        )
        .unwrap();
        assert_eq!(done.state, TaskState::Done);
        assert_eq!(done.result_message_id.as_deref(), Some("r1"));
        for transition in [
            TaskTransition::Dispatch,
            TaskTransition::Wait,
            TaskTransition::Wake,
            TaskTransition::Stop,
            TaskTransition::Resume,
            TaskTransition::Cancel,
        ] {
            assert!(apply_transition(&done, &transition).is_err());
        }
    }

    #[test]
    fn reassign_increments_revision_and_clears_result() {
        let failed = GroupTask {
            result_message_id: Some("old".into()),
            ..task_with("t1", TaskState::Failed, 1)
        };
        let next = apply_transition(
            &failed,
            &TaskTransition::Reassign {
                owner_member_id: "m2".into(),
            },
        )
        .unwrap();
        assert_eq!(next.state, TaskState::Queued);
        assert_eq!(next.owner_member_id, "m2");
        assert_eq!(next.revision, 2);
        assert_eq!(next.result_message_id, None);
    }

    #[test]
    fn projection_replays_events_and_ignores_unknown() {
        let mut projection = TaskProjection::new();
        projection
            .apply(&TaskEvent::Created {
                at: 1,
                task: task("t1"),
            })
            .unwrap();
        projection
            .apply(&TaskEvent::Dispatched {
                at: 2,
                task_id: "t1".into(),
                state: TaskState::Running,
                revision: 0,
            })
            .unwrap();
        projection
            .apply(&TaskEvent::Waiting {
                at: 3,
                task_id: "t1".into(),
                state: TaskState::Waiting,
                revision: 0,
            })
            .unwrap();
        projection
            .apply(&TaskEvent::Stopped {
                at: 4,
                task_id: "t1".into(),
                state: TaskState::Stopped,
                revision: 0,
            })
            .unwrap();
        projection
            .apply(&TaskEvent::Resumed {
                at: 5,
                task_id: "t1".into(),
                state: TaskState::Queued,
                revision: 0,
            })
            .unwrap();
        projection
            .apply(&TaskEvent::Dispatched {
                at: 6,
                task_id: "t1".into(),
                state: TaskState::Running,
                revision: 0,
            })
            .unwrap();
        projection
            .apply(&TaskEvent::Settled {
                at: 7,
                task_id: "t1".into(),
                state: TaskState::Done,
                revision: 0,
                result_message_id: Some("r1".into()),
            })
            .unwrap();
        projection.apply(&TaskEvent::Unknown).unwrap();
        let task = projection.get("t1").unwrap();
        assert_eq!(task.state, TaskState::Done);
        assert_eq!(task.result_message_id.as_deref(), Some("r1"));
    }

    #[test]
    fn wake_moves_waiting_parent_back_to_queued() {
        let mut projection = TaskProjection::new();
        projection
            .apply(&TaskEvent::Created {
                at: 1,
                task: task("t1"),
            })
            .unwrap();
        projection
            .apply(&TaskEvent::Dispatched {
                at: 2,
                task_id: "t1".into(),
                state: TaskState::Running,
                revision: 0,
            })
            .unwrap();
        projection
            .apply(&TaskEvent::Waiting {
                at: 3,
                task_id: "t1".into(),
                state: TaskState::Waiting,
                revision: 0,
            })
            .unwrap();
        projection
            .apply(&TaskEvent::Woken {
                at: 4,
                task_id: "t1".into(),
                state: TaskState::Queued,
                revision: 0,
            })
            .unwrap();
        assert_eq!(projection.get("t1").unwrap().state, TaskState::Queued);
        let value = serde_json::to_value(TaskEvent::Woken {
            at: 4,
            task_id: "t1".into(),
            state: TaskState::Queued,
            revision: 0,
        })
        .unwrap();
        assert_eq!(value["type"], "woken");
    }

    #[test]
    fn projection_rejects_bad_event_state_and_revision() {
        let mut projection = TaskProjection::new();
        projection
            .apply(&TaskEvent::Created {
                at: 1,
                task: task("t1"),
            })
            .unwrap();
        assert!(projection
            .apply(&TaskEvent::Dispatched {
                at: 2,
                task_id: "t1".into(),
                state: TaskState::Done,
                revision: 0,
            })
            .is_err());
        assert!(projection
            .apply(&TaskEvent::Dispatched {
                at: 2,
                task_id: "t1".into(),
                state: TaskState::Running,
                revision: 3,
            })
            .is_err());
    }

    #[test]
    fn reassign_event_with_wrong_revision_does_not_mutate_projection() {
        let mut projection = TaskProjection::new();
        projection
            .apply(&TaskEvent::Created {
                at: 1,
                task: task_with("t1", TaskState::Queued, 0),
            })
            .unwrap();
        projection
            .apply(&TaskEvent::Stopped {
                at: 2,
                task_id: "t1".into(),
                state: TaskState::Stopped,
                revision: 0,
            })
            .unwrap();
        let result = projection.apply(&TaskEvent::Reassigned {
            at: 3,
            task_id: "t1".into(),
            state: TaskState::Queued,
            revision: 5,
            owner_member_id: "m9".into(),
        });
        assert!(result.is_err());
        let task = projection.get("t1").unwrap();
        assert_eq!(task.state, TaskState::Stopped, "坏事件不得部分应用");
        assert_eq!(task.revision, 0);
        assert_eq!(task.owner_member_id, "m1");
    }

    #[test]
    fn duplicate_running_is_rejected_per_group_member() {
        let mut projection = TaskProjection::new();
        for id in ["t1", "t2"] {
            projection
                .apply(&TaskEvent::Created {
                    at: 1,
                    task: task(id),
                })
                .unwrap();
        }
        projection
            .apply(&TaskEvent::Dispatched {
                at: 2,
                task_id: "t1".into(),
                state: TaskState::Running,
                revision: 0,
            })
            .unwrap();
        let duplicate = projection.apply(&TaskEvent::Dispatched {
            at: 3,
            task_id: "t2".into(),
            state: TaskState::Running,
            revision: 0,
        });
        assert!(matches!(duplicate, Err(TaskError::DuplicateRunning { .. })));
    }

    #[test]
    fn recovery_wakes_waiting_parent_when_children_are_settled() {
        let mut projection = TaskProjection::new();
        for task in [task("parent"), child("child", "parent")] {
            projection
                .apply(&TaskEvent::Created { at: 1, task })
                .unwrap();
        }
        projection
            .apply(&TaskEvent::Dispatched {
                at: 2,
                task_id: "parent".into(),
                state: TaskState::Running,
                revision: 0,
            })
            .unwrap();
        projection
            .apply(&TaskEvent::Dispatched {
                at: 3,
                task_id: "child".into(),
                state: TaskState::Running,
                revision: 0,
            })
            .unwrap();
        projection
            .apply(&TaskEvent::Settled {
                at: 4,
                task_id: "child".into(),
                state: TaskState::Done,
                revision: 0,
                result_message_id: Some("r1".into()),
            })
            .unwrap();
        projection
            .apply(&TaskEvent::Waiting {
                at: 5,
                task_id: "parent".into(),
                state: TaskState::Waiting,
                revision: 0,
            })
            .unwrap();

        let events = projection.recovery_wake_events(9);
        assert_eq!(events.len(), 1);
        projection.apply(&events[0]).unwrap();
        assert_eq!(
            projection.get("parent").unwrap().state,
            TaskState::Queued
        );
    }

    #[test]
    fn recovery_keeps_parent_waiting_while_child_is_open() {
        let mut projection = TaskProjection::new();
        for task in [task("parent"), child("child", "parent")] {
            projection
                .apply(&TaskEvent::Created { at: 1, task })
                .unwrap();
        }
        projection
            .apply(&TaskEvent::Dispatched {
                at: 2,
                task_id: "parent".into(),
                state: TaskState::Running,
                revision: 0,
            })
            .unwrap();
        projection
            .apply(&TaskEvent::Waiting {
                at: 3,
                task_id: "parent".into(),
                state: TaskState::Waiting,
                revision: 0,
            })
            .unwrap();
        assert!(projection.recovery_wake_events(9).is_empty());
        assert_eq!(
            projection.get("parent").unwrap().state,
            TaskState::Waiting
        );
    }

    #[test]
    fn recovery_wake_does_not_cascade_past_a_queued_parent() {
        let mut projection = TaskProjection::new();
        let mut leaf = child("leaf", "mid");
        leaf.owner_member_id = "m3".to_string();
        for task in [task("root"), child("mid", "root"), leaf] {
            projection
                .apply(&TaskEvent::Created { at: 1, task })
                .unwrap();
        }
        for (at, id) in [(2, "root"), (3, "mid"), (4, "leaf")] {
            projection
                .apply(&TaskEvent::Dispatched {
                    at,
                    task_id: id.into(),
                    state: TaskState::Running,
                    revision: 0,
                })
                .unwrap();
        }
        projection
            .apply(&TaskEvent::Settled {
                at: 5,
                task_id: "leaf".into(),
                state: TaskState::Done,
                revision: 0,
                result_message_id: None,
            })
            .unwrap();
        for (at, id) in [(6, "mid"), (7, "root")] {
            projection
                .apply(&TaskEvent::Waiting {
                    at,
                    task_id: id.into(),
                    state: TaskState::Waiting,
                    revision: 0,
                })
                .unwrap();
        }
        let events = projection.recovery_wake_events(9);
        assert_eq!(events.len(), 1, "只唤醒 mid；root 的 child 仍 open");
        assert_eq!(events[0].task_id(), Some("mid"));
        projection.apply(&events[0]).unwrap();
        assert_eq!(projection.get("mid").unwrap().state, TaskState::Queued);
        assert_eq!(
            projection.get("root").unwrap().state,
            TaskState::Waiting
        );
    }

    #[test]
    fn cancel_cascades_post_order_and_skips_done_children() {
        let mut projection = TaskProjection::new();
        for task in [task("root"), child("child", "root"), child("grand", "child")] {
            projection
                .apply(&TaskEvent::Created { at: 1, task })
                .unwrap();
        }
        // grand → Done；child → Running；root 保持 Queued。
        projection
            .apply(&TaskEvent::Dispatched {
                at: 2,
                task_id: "grand".into(),
                state: TaskState::Running,
                revision: 0,
            })
            .unwrap();
        projection
            .apply(&TaskEvent::Settled {
                at: 3,
                task_id: "grand".into(),
                state: TaskState::Done,
                revision: 0,
                result_message_id: None,
            })
            .unwrap();
        // child 的 owner 也是 m2（child 构造默认），grand 已 Done，不冲突。
        projection
            .apply(&TaskEvent::Dispatched {
                at: 4,
                task_id: "child".into(),
                state: TaskState::Running,
                revision: 0,
            })
            .unwrap();
        let events = projection.cancel_subtree_events("root", 9).unwrap();
        let ids: Vec<&str> = events
            .iter()
            .map(|event| event.task_id().unwrap())
            .collect();
        assert_eq!(ids, vec!["child", "root"], "后序且跳过 Done 子孙");
        for event in &events {
            projection.apply(event).unwrap();
        }
        assert_eq!(projection.get("grand").unwrap().state, TaskState::Done);
        assert_eq!(projection.get("child").unwrap().state, TaskState::Cancelled);
        assert_eq!(projection.get("root").unwrap().state, TaskState::Cancelled);
    }

    #[test]
    fn waiting_does_not_occupy_turn_slot() {
        assert!(!TaskState::Waiting.occupies_turn_slot());
        assert!(TaskState::Running.occupies_turn_slot());
        assert!(!TaskState::Queued.occupies_turn_slot());
    }

    #[test]
    fn routing_precedence_is_mentions_explicit_unique_open_then_admin() {
        let mentioned = vec!["m2".to_string(), "m1".to_string()];
        let mut open = task("t1");
        open.owner_member_id = "m3".into();
        assert_eq!(
            route_task(&TaskRouteInput {
                mentioned_ids: &mentioned,
                explicit_recipient: Some("m4"),
                open_tasks: &[open.clone()],
                admin_id: Some("admin"),
            }),
            TaskRoute::Mentioned(mentioned.clone())
        );
        assert_eq!(
            route_task(&TaskRouteInput {
                mentioned_ids: &[],
                explicit_recipient: Some(" m4 "),
                open_tasks: &[open.clone()],
                admin_id: Some("admin"),
            }),
            TaskRoute::Explicit("m4".into())
        );
        assert_eq!(
            route_task(&TaskRouteInput {
                mentioned_ids: &[],
                explicit_recipient: None,
                open_tasks: &[open],
                admin_id: Some("admin"),
            }),
            TaskRoute::UniqueOpenTask {
                task_id: "t1".into(),
                member_id: "m3".into(),
            }
        );
        assert_eq!(
            route_task(&TaskRouteInput {
                mentioned_ids: &[],
                explicit_recipient: None,
                open_tasks: &[],
                admin_id: Some("admin"),
            }),
            TaskRoute::Admin("admin".into())
        );
        assert_eq!(
            route_task(&TaskRouteInput {
                mentioned_ids: &[],
                explicit_recipient: None,
                open_tasks: &[],
                admin_id: None,
            }),
            TaskRoute::Unrouted
        );
    }

    #[test]
    fn parent_must_exist_and_share_group() {
        let mut projection = TaskProjection::new();
        let mut bad = child("t2", "missing");
        bad.group_id = "g2".into();
        assert!(matches!(
            projection.apply(&TaskEvent::Created { at: 1, task: bad }),
            Err(TaskError::NotFound(_))
        ));
        projection
            .apply(&TaskEvent::Created {
                at: 1,
                task: task("t1"),
            })
            .unwrap();
        let mut mismatch = child("t2", "t1");
        mismatch.group_id = "g2".into();
        assert!(matches!(
            projection.apply(&TaskEvent::Created {
                at: 2,
                task: mismatch
            }),
            Err(TaskError::Invalid(_))
        ));
    }

    #[test]
    fn task_events_round_trip_jsonl_shape() {
        let event = TaskEvent::Dispatched {
            at: 2,
            task_id: "t1".into(),
            state: TaskState::Running,
            revision: 0,
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["type"], "dispatched");
        assert_eq!(value["taskId"], "t1");
        assert_eq!(value["state"], "running");
        let back: TaskEvent = serde_json::from_value(value).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn unknown_event_json_is_tolerated() {
        let event: TaskEvent =
            serde_json::from_str(r#"{"type":"future_event","at":9,"taskId":"t1"}"#).unwrap();
        assert_eq!(event, TaskEvent::Unknown);
        let mut projection = TaskProjection::new();
        projection.apply(&event).unwrap();
    }
}
