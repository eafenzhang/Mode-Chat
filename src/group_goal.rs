//! 群目标 repo（GPUI 版，与壳同口径）。
//!
//! 文件：`{data_root}/v2/groups/<groupId>/goal.json`。整文件原子写，不走事件流；
//! 状态机复用 `bot_team::goal`。损坏文件按「无目标」处理并留日志，不阻塞群聊。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use atomic_file::write_json_atomic;
use bot_team::goal::{apply_goal_action, validate_goal, GoalAction, GroupGoal};

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

fn goal_path(data_root: &Path, group_id: &str) -> Result<PathBuf, String> {
    let id = validate_group_id(group_id)?;
    Ok(data_root
        .join("v2")
        .join("groups")
        .join(id)
        .join("goal.json"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// 进程内写锁：命令与回合读同一文件，RMW 必须串行。
fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 读取并校验；缺失/损坏都返回 None（损坏留日志，和壳的容错口径一致）。
pub fn read(data_root: &Path, group_id: &str) -> Option<GroupGoal> {
    let path = goal_path(data_root, group_id).ok()?;
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!("[goal] 读取群目标失败（{}）：{error}", path.display());
            return None;
        }
    };
    match serde_json::from_str::<GroupGoal>(&raw) {
        Ok(goal) => match validate_goal(goal) {
            Ok(goal) => Some(goal),
            Err(error) => {
                eprintln!("[goal] 群目标校验失败（{}）：{error}", path.display());
                None
            }
        },
        Err(error) => {
            eprintln!("[goal] 群目标解析失败（{}）：{error}", path.display());
            None
        }
    }
}

fn write(path: &Path, goal: &GroupGoal) -> Result<(), String> {
    let goal = validate_goal(goal.clone())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建群目标目录失败：{error}"))?;
    }
    let value = serde_json::to_value(&goal).map_err(|error| error.to_string())?;
    write_json_atomic(path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| error.to_string())
}

/// 应用动作并原子落盘；`Clear` 删除文件并返回 None。
pub fn apply(
    data_root: &Path,
    group_id: &str,
    action: GoalAction,
) -> Result<Option<GroupGoal>, String> {
    let path = goal_path(data_root, group_id)?;
    let _guard = write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let current = read(data_root, group_id);
    let next = apply_goal_action(current, action, now_ms())?;
    match &next {
        Some(goal) => write(&path, goal)?,
        None => {
            if path.exists() {
                std::fs::remove_file(&path).map_err(|error| format!("清除群目标失败：{error}"))?;
            }
        }
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_team::goal::{GoalStatus, StageStatus};

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mode-bot-gpui-goal-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn create_then_clear_roundtrip() {
        let root = sandbox("roundtrip");
        let goal = apply(
            &root,
            "g1",
            GoalAction::Create {
                title: "发货".to_string(),
                description: "一期".to_string(),
                stages: vec!["设计".to_string(), "实现".to_string()],
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(goal.status, GoalStatus::Active);
        assert_eq!(goal.stages[0].status, StageStatus::Active);
        assert_eq!(read(&root, "g1").unwrap().title, "发货");

        let cleared = apply(&root, "g1", GoalAction::Clear).unwrap();
        assert!(cleared.is_none());
        assert!(read(&root, "g1").is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_bad_group_id() {
        assert!(validate_group_id("../x").is_err());
        assert!(validate_group_id("").is_err());
        assert!(validate_group_id("g-1").is_ok());
    }
}
