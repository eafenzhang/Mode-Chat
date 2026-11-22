//! 群任务 JSONL repo（GPUI 版，与壳同口径）。
//!
//! 文件：`{data_root}/v2/groups/<groupId>/tasks.jsonl`，append-only；纯状态机复用
//! `bot_team::tasks`。GPUI 群回合在 UI 线程按成员创建/结算任务，与 WebView 的
//! 编排器台账兼容（同 schema）。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use bot_team::tasks::{
    apply_transition, GroupTask, TaskCreateInput, TaskEvent, TaskProjection, TaskState,
    TaskTransition, GROUP_ROOT_OWNER,
};

/// 群 id 只允许 ASCII 字母数字与 `-`（与壳 `validate_group_id` 同规则）。
pub fn validate_group_id(group_id: &str) -> Result<&str, String> {
    let trimmed = group_id.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return Err("非法的群 id".to_string());
    }
    if !trimmed
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err(format!("非法的群 id：{trimmed}"));
    }
    Ok(trimmed)
}

fn tasks_path(data_root: &Path, group_id: &str) -> Result<PathBuf, String> {
    let id = validate_group_id(group_id)?;
    Ok(data_root
        .join("v2")
        .join("groups")
        .join(id)
        .join("tasks.jsonl"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn read_projection(path: &Path) -> Result<TaskProjection, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TaskProjection::new());
        }
        Err(error) => return Err(format!("读取任务文件失败（{}）：{error}", path.display())),
    };
    let mut projection = TaskProjection::new();
    for (index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event = match serde_json::from_str::<TaskEvent>(line) {
            Ok(event) => event,
            Err(error) => {
                eprintln!("[tasks] 第 {} 行解析失败：{error}", index + 1);
                continue;
            }
        };
        if let Err(error) = projection.apply(&event) {
            eprintln!("[tasks] 第 {} 行回放失败：{error}", index + 1);
        }
    }
    Ok(projection)
}

fn append_event_locked(path: &Path, event: &TaskEvent) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建任务目录失败：{error}"))?;
    }
    let line = serde_json::to_string(event).map_err(|error| error.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("打开任务文件失败（{}）：{error}", path.display()))?;
    file.write_all(format!("{line}\n").as_bytes())
        .map_err(|error| format!("写入任务文件失败：{error}"))
}

/// 列出群聊任务（created_at, id 升序）。
pub fn list(data_root: &Path, group_id: &str) -> Result<Vec<GroupTask>, String> {
    let path = tasks_path(data_root, group_id)?;
    let mut tasks: Vec<GroupTask> = read_projection(&path)?.tasks().cloned().collect();
    tasks.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(tasks)
}

fn event_for_transition(at: u64, task: &GroupTask, transition: &TaskTransition) -> TaskEvent {
    match transition {
        TaskTransition::Dispatch => TaskEvent::Dispatched {
            at,
            task_id: task.id.clone(),
            state: task.state,
            revision: task.revision,
        },
        TaskTransition::Settle {
            result_message_id, ..
        } => TaskEvent::Settled {
            at,
            task_id: task.id.clone(),
            state: task.state,
            revision: task.revision,
            result_message_id: result_message_id.clone(),
        },
        TaskTransition::Wait => TaskEvent::Waiting {
            at,
            task_id: task.id.clone(),
            state: task.state,
            revision: task.revision,
        },
        TaskTransition::Wake => TaskEvent::Woken {
            at,
            task_id: task.id.clone(),
            state: task.state,
            revision: task.revision,
        },
        TaskTransition::Stop => TaskEvent::Stopped {
            at,
            task_id: task.id.clone(),
            state: task.state,
            revision: task.revision,
        },
        TaskTransition::Resume => TaskEvent::Resumed {
            at,
            task_id: task.id.clone(),
            state: task.state,
            revision: task.revision,
        },
        TaskTransition::Reassign { owner_member_id } => TaskEvent::Reassigned {
            at,
            task_id: task.id.clone(),
            state: task.state,
            revision: task.revision,
            owner_member_id: owner_member_id.clone(),
        },
        TaskTransition::Cancel => TaskEvent::Cancelled {
            at,
            task_id: task.id.clone(),
            state: task.state,
            revision: task.revision,
        },
    }
}

fn transition_locked(
    path: &Path,
    projection: &TaskProjection,
    task_id: &str,
    transition: TaskTransition,
) -> Result<GroupTask, String> {
    let current = projection
        .get(task_id)
        .map_err(|error| error.to_string())?
        .clone();
    let next = apply_transition(&current, &transition).map_err(|error| error.to_string())?;
    append_event_locked(path, &event_for_transition(now_ms(), &next, &transition))?;
    Ok(next)
}

fn create_locked(
    path: &Path,
    projection: &TaskProjection,
    group_id: &str,
    root_message_id: &str,
    parent_task_id: Option<&str>,
    owner_member_id: &str,
    expected_result: &str,
) -> Result<GroupTask, String> {
    let task = GroupTask::create(TaskCreateInput {
        id: uuid::Uuid::new_v4().to_string(),
        group_id: group_id.to_string(),
        root_message_id: root_message_id.to_string(),
        parent_task_id: parent_task_id.map(str::to_string),
        owner_member_id: owner_member_id.to_string(),
        expected_result: expected_result.to_string(),
        created_at: now_ms(),
    })
    .map_err(|error| error.to_string())?;
    append_event_locked(
        path,
        &TaskEvent::Created {
            at: task.created_at,
            task: task.clone(),
        },
    )?;
    let _ = projection;
    Ok(task)
}

/// 一轮群发言的台账：根容器 + 每个被路由成员的子任务；全部 Dispatch。
/// 返回 (根任务, [(成员名, 子任务 id)])。
pub fn create_turn(
    data_root: &Path,
    group_id: &str,
    root_message_id: &str,
    expected_result: &str,
    members: &[(String, String)],
) -> Result<(GroupTask, Vec<(String, String)>), String> {
    let path = tasks_path(data_root, group_id)?;
    let _guard = write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut projection = read_projection(&path)?;
    let expected: String = expected_result.chars().take(120).collect();
    let mut root = create_locked(
        &path,
        &projection,
        group_id,
        root_message_id,
        None,
        GROUP_ROOT_OWNER,
        &expected,
    )?;
    projection = read_projection(&path)?;
    root = transition_locked(&path, &projection, &root.id, TaskTransition::Dispatch)?;
    let mut children = Vec::new();
    for (member_id, member_name) in members {
        let child = create_locked(
            &path,
            &projection,
            group_id,
            root_message_id,
            Some(&root.id),
            member_id,
            &expected,
        )?;
        projection = read_projection(&path)?;
        let child = transition_locked(&path, &projection, &child.id, TaskTransition::Dispatch)?;
        children.push((member_name.clone(), child.id));
    }
    Ok((root, children))
}

/// 群内交接：在现有根任务下追加一个成员子任务并立即 Dispatch。
pub fn create_handoff(
    data_root: &Path,
    group_id: &str,
    root_message_id: &str,
    root_task_id: &str,
    owner_member_id: &str,
    expected_result: &str,
) -> Result<GroupTask, String> {
    let path = tasks_path(data_root, group_id)?;
    let _guard = write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let projection = read_projection(&path)?;
    let expected: String = expected_result.chars().take(120).collect();
    let task = create_locked(
        &path,
        &projection,
        group_id,
        root_message_id,
        Some(root_task_id),
        owner_member_id,
        &expected,
    )?;
    let projection = read_projection(&path)?;
    transition_locked(&path, &projection, &task.id, TaskTransition::Dispatch)
}

/// 结算子任务；若根容器下所有子任务都已终态，同时结算根容器。
pub fn settle(
    data_root: &Path,
    group_id: &str,
    task_id: &str,
    ok: bool,
    root_id: Option<&str>,
) -> Result<(), String> {
    let path = tasks_path(data_root, group_id)?;
    let _guard = write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let projection = read_projection(&path)?;
    let current = match projection.get(task_id) {
        Ok(task) => task.clone(),
        Err(_) => return Ok(()),
    };
    if !current.state.is_open() {
        return Ok(());
    }
    let transition = TaskTransition::Settle {
        expected_revision: current.revision,
        ok,
        result_message_id: None,
    };
    transition_locked(&path, &projection, task_id, transition)?;
    if let Some(root_id) = root_id {
        let projection = read_projection(&path)?;
        settle_root_if_done_locked(&path, &projection, group_id, root_id)?;
    }
    Ok(())
}

fn settle_root_if_done_locked(
    path: &Path,
    projection: &TaskProjection,
    group_id: &str,
    root_id: &str,
) -> Result<(), String> {
    let _ = group_id;
    let Ok(root) = projection.get(root_id) else {
        return Ok(());
    };
    if !root.state.is_open() {
        return Ok(());
    }
    let children: Vec<&GroupTask> = projection
        .tasks()
        .filter(|task| task.parent_task_id.as_deref() == Some(root_id))
        .collect();
    if children.is_empty() || children.iter().any(|task| !task.state.is_terminal()) {
        return Ok(());
    }
    let ok = children
        .iter()
        .all(|task| matches!(task.state, TaskState::Done));
    // 根容器是 Running/Queued 才能直接 settle。
    let transition = TaskTransition::Settle {
        expected_revision: root.revision,
        ok,
        result_message_id: None,
    };
    transition_locked(path, projection, root_id, transition)?;
    Ok(())
}

/// 停止一组开放任务（用户 Stop all）：Queued/Running/Waiting → Stopped。
pub fn stop_open(data_root: &Path, group_id: &str) -> Vec<String> {
    let Ok(path) = tasks_path(data_root, group_id) else {
        return Vec::new();
    };
    let _guard = write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok(projection) = read_projection(&path) else {
        return Vec::new();
    };
    let open: Vec<String> = projection
        .tasks()
        .filter(|task| task.state.is_open())
        .map(|task| task.id.clone())
        .collect();
    let mut stopped = Vec::new();
    for id in open {
        if transition_locked(&path, &projection, &id, TaskTransition::Stop).is_ok() {
            stopped.push(id);
        }
    }
    stopped
}

pub fn stop_task(data_root: &Path, group_id: &str, task_id: &str) -> Result<(), String> {
    let path = tasks_path(data_root, group_id)?;
    let _guard = write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let projection = read_projection(&path)?;
    let current = projection
        .get(task_id)
        .map_err(|error| error.to_string())?
        .clone();
    if !current.state.is_open() {
        return Ok(());
    }
    transition_locked(&path, &projection, task_id, TaskTransition::Stop)?;
    Ok(())
}

/// 重试/改派：把 Stopped/Failed 任务推回 Queued 并 Dispatch。
///
/// `new_owner` 为 None 时沿用原 owner（Stopped 走 Resume；Failed 走同 owner 的
/// Reassign，因为状态机只允许 Stopped Resume）。返回派发后的任务。
pub fn retry_task(
    data_root: &Path,
    group_id: &str,
    task_id: &str,
    new_owner: Option<&str>,
) -> Result<GroupTask, String> {
    let path = tasks_path(data_root, group_id)?;
    let _guard = write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let projection = read_projection(&path)?;
    let current = projection
        .get(task_id)
        .map_err(|error| error.to_string())?
        .clone();
    if current.state.is_open() {
        return Ok(current);
    }
    if current.state == TaskState::Done || current.state == TaskState::Cancelled {
        return Err("已完成/已取消的任务不能重试".to_string());
    }
    let mut task = current.clone();
    if let Some(owner) = new_owner
        && owner != current.owner_member_id
    {
        task = transition_locked(
            &path,
            &projection,
            task_id,
            TaskTransition::Reassign {
                owner_member_id: owner.to_string(),
            },
        )?;
    } else if current.state == TaskState::Stopped {
        task = transition_locked(&path, &projection, task_id, TaskTransition::Resume)?;
    } else if current.state == TaskState::Failed {
        // Failed 不能 Resume：同 owner Reassign 会回到 Queued（revision +1）。
        task = transition_locked(
            &path,
            &projection,
            task_id,
            TaskTransition::Reassign {
                owner_member_id: current.owner_member_id.clone(),
            },
        )?;
    }
    if task.state == TaskState::Queued {
        let projection = read_projection(&path)?;
        task = transition_locked(&path, &projection, task_id, TaskTransition::Dispatch)?;
    }
    Ok(task)
}

/// 取消任务及其子树；返回被取消的任务 id（含根）。
pub fn cancel_subtree(
    data_root: &Path,
    group_id: &str,
    task_id: &str,
) -> Result<Vec<String>, String> {
    let path = tasks_path(data_root, group_id)?;
    let _guard = write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let projection = read_projection(&path)?;
    let events = projection
        .cancel_subtree_events(task_id, now_ms())
        .map_err(|error| error.to_string())?;
    let mut ids = Vec::new();
    for event in &events {
        append_event_locked(&path, event)?;
        if let Some(id) = event.task_id() {
            ids.push(id.to_string());
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mode-bot-gpui-tasks-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn turn_settles_children_and_root() {
        let root = sandbox("settle");
        let (root_task, children) = create_turn(
            &root,
            "g1",
            "m1",
            "做一件事",
            &[
                ("a".to_string(), "A".to_string()),
                ("b".to_string(), "B".to_string()),
            ],
        )
        .unwrap();
        assert_eq!(children.len(), 2);
        let tasks = list(&root, "g1").unwrap();
        assert_eq!(tasks.len(), 3);
        assert!(tasks.iter().all(|task| task.state == TaskState::Running));

        settle(&root, "g1", &children[0].1, true, Some(&root_task.id)).unwrap();
        let tasks = list(&root, "g1").unwrap();
        let root_now = tasks.iter().find(|task| task.id == root_task.id).unwrap();
        assert!(root_now.state.is_open());

        settle(&root, "g1", &children[1].1, false, Some(&root_task.id)).unwrap();
        let tasks = list(&root, "g1").unwrap();
        let root_now = tasks.iter().find(|task| task.id == root_task.id).unwrap();
        assert_eq!(root_now.state, TaskState::Failed);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn retry_failed_and_stopped_tasks() {
        let root = sandbox("retry");
        let (_, children) = create_turn(
            &root,
            "g1",
            "m1",
            "重试",
            &[("a".to_string(), "A".to_string())],
        )
        .unwrap();
        let task_id = children[0].1.clone();
        settle(&root, "g1", &task_id, false, None).unwrap();
        let retried = retry_task(&root, "g1", &task_id, None).unwrap();
        assert_eq!(retried.state, TaskState::Running);
        assert_eq!(retried.revision, 1);

        stop_task(&root, "g1", &task_id).unwrap();
        let resumed = retry_task(&root, "g1", &task_id, Some("b")).unwrap();
        assert_eq!(resumed.state, TaskState::Running);
        assert_eq!(resumed.owner_member_id, "b");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cancel_subtree_collects_ids() {
        let root = sandbox("cancel");
        let (root_task, children) = create_turn(
            &root,
            "g1",
            "m1",
            "取消",
            &[("a".to_string(), "A".to_string())],
        )
        .unwrap();
        let ids = cancel_subtree(&root, "g1", &root_task.id).unwrap();
        assert!(ids.contains(&root_task.id));
        assert!(ids.contains(&children[0].1));
        let tasks = list(&root, "g1").unwrap();
        assert!(tasks.iter().all(|task| task.state == TaskState::Cancelled));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_bad_group_id() {
        assert!(validate_group_id("../x").is_err());
        assert!(validate_group_id("g-1").is_ok());
    }
}
