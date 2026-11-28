//! 后台任务记录：`{data_root}/v2/tasks/index.json`（原子写，单文件）。
//!
//! 任务 = 隶属某个 Bot 的一次独立引擎运行（与主会话并行，会话文件落
//! `sessions_root/.tasks/<bot>/`）。本模块只管**记录**：状态机、列表、
//! 保留上限；真正的执行在 `backend::run_agent` 的 Task 命令分支里。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 每 Bot 保留的已结束任务条数（running 不受限，另有并发上限）。
pub const KEEP_FINISHED_PER_BOT: usize = 50;
/// 每 Bot 同时运行的任务上限。
pub const MAX_RUNNING_PER_BOT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Done,
    Failed,
    Stopped,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Queued => "queued",
            TaskStatus::Running => "running",
            TaskStatus::Done => "done",
            TaskStatus::Failed => "failed",
            TaskStatus::Stopped => "stopped",
        }
    }

    pub fn is_finished(self) -> bool {
        matches!(self, TaskStatus::Done | TaskStatus::Failed | TaskStatus::Stopped)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub bot_id: String,
    #[serde(default)]
    pub bot_name: String,
    /// UI key（bot:<id>）；任务列表按它过滤。
    pub ui_key: String,
    pub title: String,
    pub prompt: String,
    pub status: TaskStatus,
    pub created_at: u64,
    #[serde(default)]
    pub finished_at: u64,
    #[serde(default)]
    pub session_path: String,
    /// 最近一行输出（流式期间节流更新）。
    #[serde(default)]
    pub last_line: String,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub error: String,
}

fn index_path(data_root: &Path) -> PathBuf {
    data_root.join("v2").join("tasks").join("index.json")
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TaskIndex {
    #[serde(default)]
    tasks: Vec<TaskRecord>,
}

fn load_raw(data_root: &Path) -> TaskIndex {
    std::fs::read(index_path(data_root))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<TaskIndex>(&bytes).ok())
        .unwrap_or_default()
}

fn save_raw(data_root: &Path, index: &TaskIndex) -> Result<(), String> {
    let path = index_path(data_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建任务目录失败：{error}"))?;
    }
    let value = serde_json::to_value(index)
        .map_err(|error| format!("任务索引序列化失败：{error}"))?;
    atomic_file::write_json_atomic(&path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("写任务索引失败：{error}"))
}

/// 全部任务（新→旧）。
pub fn list_all(data_root: &Path) -> Vec<TaskRecord> {
    let mut tasks = load_raw(data_root).tasks;
    tasks.sort_by_key(|task| std::cmp::Reverse(task.created_at));
    tasks
}

/// 某 UI key 的任务（新→旧），附带全局运行中计数在调用方算。
pub fn list_for(data_root: &Path, ui_key: &str) -> Vec<TaskRecord> {
    list_all(data_root)
        .into_iter()
        .filter(|task| task.ui_key == ui_key)
        .collect()
}

pub fn get(data_root: &Path, task_id: &str) -> Option<TaskRecord> {
    load_raw(data_root)
        .tasks
        .into_iter()
        .find(|task| task.id == task_id)
}

pub fn running_count(data_root: &Path, bot_id: &str) -> usize {
    load_raw(data_root)
        .tasks
        .iter()
        .filter(|task| task.bot_id == bot_id && task.status == TaskStatus::Running)
        .count()
}

/// 插入或整条替换；同时按 Bot 裁剪已结束任务到 [`KEEP_FINISHED_PER_BOT`]。
pub fn upsert(data_root: &Path, record: &TaskRecord) -> Result<(), String> {
    let mut index = load_raw(data_root);
    match index.tasks.iter_mut().find(|task| task.id == record.id) {
        Some(slot) => *slot = record.clone(),
        None => index.tasks.push(record.clone()),
    }
    // 裁剪：按 Bot 分组，finished 的只留最近 N 条。
    let mut kept: Vec<TaskRecord> = Vec::with_capacity(index.tasks.len());
    let mut finished_seen: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut sorted = index.tasks;
    sorted.sort_by_key(|task| std::cmp::Reverse(task.created_at));
    for task in sorted {
        if task.status.is_finished() {
            let count = finished_seen.entry(task.bot_id.clone()).or_insert(0);
            if *count >= KEEP_FINISHED_PER_BOT {
                continue;
            }
            *count += 1;
        }
        kept.push(task);
    }
    save_raw(data_root, &TaskIndex { tasks: kept })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mc-task-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn record(id: &str, status: TaskStatus, at: u64) -> TaskRecord {
        TaskRecord {
            id: id.to_string(),
            bot_id: "b1".to_string(),
            bot_name: "Bot".to_string(),
            ui_key: "bot:b1".to_string(),
            title: "测试任务".to_string(),
            prompt: "跑一下".to_string(),
            status,
            created_at: at,
            finished_at: 0,
            session_path: String::new(),
            last_line: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            error: String::new(),
        }
    }

    #[test]
    fn upsert_and_filter_and_trim() {
        let root = temp_root("upsert");
        upsert(&root, &record("t-1", TaskStatus::Running, 1)).unwrap();
        upsert(&root, &record("t-2", TaskStatus::Done, 2)).unwrap();
        assert_eq!(list_all(&root).len(), 2);
        assert_eq!(list_for(&root, "bot:b1").len(), 2);
        assert_eq!(list_for(&root, "bot:other").len(), 0);
        assert_eq!(running_count(&root, "b1"), 1);
        // 同 id 更新不新增
        let mut updated = record("t-1", TaskStatus::Done, 1);
        updated.last_line = "完成".to_string();
        upsert(&root, &updated).unwrap();
        assert_eq!(list_all(&root).len(), 2);
        assert_eq!(get(&root, "t-1").unwrap().last_line, "完成");
        // 裁剪：写入 60 条已结束 + 1 条运行中，只留 50 条 ended
        for i in 0..60u64 {
            upsert(&root, &record(&format!("old-{i}"), TaskStatus::Done, 100 + i)).unwrap();
        }
        upsert(&root, &record("live", TaskStatus::Running, 500)).unwrap();
        let all = list_all(&root);
        let finished = all.iter().filter(|t| t.status.is_finished()).count();
        let running = all.iter().filter(|t| t.status == TaskStatus::Running).count();
        assert_eq!(finished, KEEP_FINISHED_PER_BOT);
        assert_eq!(running, 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
