//! 试点自己的会话映射：`botId → sessionId`，落盘在试点目录（不写产品数据）。
//!
//! 重启后按这张表 `session/resume` 恢复上次会话并读回历史；产品 `teammates.json` 里的
//! `lastSessionId` 只用于右栏展示（它指向主程序的会话库，试点不读写）。
//!
//! 默认路径：`{home}/.mode-bot-pilot/state.json`；`BOT_PILOT_STATE_FILE` 可覆盖。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const STATE_VERSION: u32 = 1;

#[derive(Debug, Default, Serialize, Deserialize)]
struct StateFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    sessions: BTreeMap<String, String>,
    /// 每个私聊 Bot 的历史会话列表（新→旧，不含主槽位当前值）。
    #[serde(default)]
    history: BTreeMap<String, Vec<String>>,
    /// 会话显示名（试点本地重命名；不改产品会话库）。
    #[serde(default)]
    titles: BTreeMap<String, String>,
    /// 每个 Bot 选择的本地智能体引擎（缺省 = mode，即 Mode Agent）。
    #[serde(default)]
    engines: BTreeMap<String, String>,
}

pub struct SessionStore {
    path: PathBuf,
    sessions: BTreeMap<String, String>,
    history: BTreeMap<String, Vec<String>>,
    titles: BTreeMap<String, String>,
    engines: BTreeMap<String, String>,
    error: Option<String>,
}

impl SessionStore {
    pub fn load() -> Self {
        let path = state_path();
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<StateFile>(&bytes) {
                Ok(file) => Self {
                    path,
                    sessions: file.sessions,
                    history: file.history,
                    titles: file.titles,
                    engines: file.engines,
                    error: None,
                },
                Err(error) => Self {
                    path,
                    sessions: BTreeMap::new(),
                    history: BTreeMap::new(),
                    titles: BTreeMap::new(),
                    engines: BTreeMap::new(),
                    error: Some(format!("试点状态文件解析失败（{error}）")),
                },
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self {
                path,
                sessions: BTreeMap::new(),
                history: BTreeMap::new(),
                titles: BTreeMap::new(),
                engines: BTreeMap::new(),
                error: None,
            },
            Err(error) => Self {
                path,
                sessions: BTreeMap::new(),
                history: BTreeMap::new(),
                titles: BTreeMap::new(),
                engines: BTreeMap::new(),
                error: Some(format!("试点状态文件读取失败（{error}）")),
            },
        }
    }

    pub fn get(&self, bot_id: &str) -> Option<String> {
        self.sessions.get(bot_id).cloned()
    }

    /// 记录会话 id；返回值表示是否发生变化（需要落盘）。
    pub fn set(&mut self, bot_id: &str, session_id: &str) -> bool {
        if self.sessions.get(bot_id).map(String::as_str) == Some(session_id) {
            return false;
        }
        self.sessions
            .insert(bot_id.to_string(), session_id.to_string());
        true
    }

    /// 「新会话」：旧主槽位 sessionId 移入 history 头部（按 bot 裸 id 索引）。
    pub fn push_history(&mut self, bot_id: &str, old_session_id: &str) {
        if old_session_id.trim().is_empty() {
            return;
        }
        let entry = self.history.entry(bot_id.to_string()).or_default();
        if !entry.iter().any(|id| id == old_session_id) {
            entry.insert(0, old_session_id.to_string());
        }
        entry.truncate(20);
    }

    pub fn history_of(&self, bot_id: &str) -> Vec<String> {
        self.history.get(bot_id).cloned().unwrap_or_default()
    }

    /// 删除 Bot 时清理它的槽位：私聊键 + 所有以 `\u{1}<bot_id>` 结尾的群成员槽。
    pub fn remove_slots_for_bot(&mut self, bot_id: &str) {
        // 私聊槽位键是裸 bot id（Ready 处理里 bot_id 已剥掉 bot: 前缀）。
        let suffix = format!("\u{1}{bot_id}");
        self.sessions
            .retain(|key, _| *key != bot_id && !key.ends_with(&suffix));
    }

    pub fn save(&mut self) {
        let file = StateFile {
            version: STATE_VERSION,
            sessions: self.sessions.clone(),
            history: self.history.clone(),
            titles: self.titles.clone(),
            engines: self.engines.clone(),
        };
        let Ok(text) = serde_json::to_string_pretty(&file) else {
            return;
        };
        if let Some(parent) = self.path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            self.error = Some(format!("试点状态目录创建失败（{error}）"));
            return;
        }
        if let Err(error) = std::fs::write(&self.path, text) {
            self.error = Some(format!("试点状态文件写入失败（{error}）"));
        }
    }

    /// 会话显示名（本地重命名；没有则 None）。
    pub fn title(&self, session_id: &str) -> Option<&str> {
        self.titles.get(session_id).map(String::as_str)
    }

    /// 设置会话显示名（空串 = 清除）；返回是否变化。
    pub fn set_title(&mut self, session_id: &str, title: &str) -> bool {
        let title = title.trim();
        if title.is_empty() {
            return self.titles.remove(session_id).is_some();
        }
        if self.titles.get(session_id).map(String::as_str) == Some(title) {
            return false;
        }
        self.titles
            .insert(session_id.to_string(), title.to_string());
        true
    }

    /// 该 Bot 选择的引擎（None/"mode" = Mode Agent）。
    pub fn engine_of(&self, bot_id: &str) -> Option<&str> {
        self.engines.get(bot_id).map(String::as_str)
    }

    /// 设置 Bot 的引擎（"mode"/空 = 清除）；返回是否变化。
    pub fn set_engine(&mut self, bot_id: &str, engine: &str) -> bool {
        let engine = engine.trim();
        if engine.is_empty() || engine.eq_ignore_ascii_case("mode") {
            return self.engines.remove(bot_id).is_some();
        }
        if self.engines.get(bot_id).map(String::as_str) == Some(engine) {
            return false;
        }
        self.engines.insert(bot_id.to_string(), engine.to_string());
        true
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// 状态文件路径：覆盖优先，否则 `{试点目录}/state.json`（目录解析统一在 `instance`）。
pub fn state_path() -> PathBuf {
    if let Some(path) = crate::instance::state_file_override() {
        return path;
    }
    crate::instance::state_dir().join("state.json")
}
