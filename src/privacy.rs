//! 隐私 Bot 的路径归属守卫（v0.5.0 W3）。
//!
//! 与 DS Bot `ds-bot/src/host/privacy.js`（`guardFiles` / `guardTeam` / `privateClash`）
//! 对齐的最小实现：**文件类工具**在访问别人（或已归档隐私目录）的路径时强制走审批卡。
//!
//! 三层输入：
//! 1. `teammates.json` 里每个 Bot 的 `directory`（谁拥有这个目录）；
//! 2. `v2/private-archives.json` 里已删除但仍受保护的隐私目录；
//! 3. 调用者自己（`bot_id`）——自己目录直接放行，不弹卡。
//!
//! 只作用于 `read/write/edit/bash/powershell/mcp__*`；`computer_*`/`browser_*` 不在
//! 本期范围（它们没有可判定的路径参数）。
//!
//! 数据结构：新文件 `{data_root}/v2/private-archives.json`，`{version:1, archives:[…]}`，
//! **缺失 = 空列表**，所以旧版本升级上来零迁移。

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 一条归档记录：隐私 Bot 被删除后，目录继续受保护。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedPrivateBot {
    pub id: String,
    pub name: String,
    pub directory: String,
    /// 归档时间（毫秒时间戳）。
    #[serde(default)]
    pub archived_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ArchivesFile {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    archives: Vec<ArchivedPrivateBot>,
}

fn default_version() -> u32 {
    1
}

fn archives_path() -> PathBuf {
    crate::data::data_root().join("v2").join("private-archives.json")
}

/// 读归档列表；文件缺失/损坏都当空列表（不阻塞审批链路）。
pub fn archived_private_bots() -> Vec<ArchivedPrivateBot> {
    let Ok(bytes) = std::fs::read(archives_path()) else {
        return Vec::new();
    };
    match serde_json::from_slice::<ArchivesFile>(&bytes) {
        Ok(file) => file.archives,
        Err(error) => {
            eprintln!("[privacy] private-archives.json 解析失败：{error}");
            Vec::new()
        }
    }
}

fn save_archives(archives: Vec<ArchivedPrivateBot>) -> Result<(), String> {
    let path = archives_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 v2 目录失败：{error}"))?;
    }
    let file = ArchivesFile {
        version: default_version(),
        archives,
    };
    let value = serde_json::to_value(&file).map_err(|error| error.to_string())?;
    atomic_file::write_json_atomic(&path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("private-archives.json 写入失败：{error}"))
}

/// 登记一条归档（同 id 覆盖）。返回 Err 只用于日志，删除流程不因此中断。
pub fn archive_private_bot(id: &str, name: &str, directory: &str) -> Result<(), String> {
    let directory = directory.trim();
    if directory.is_empty() {
        return Err("目录为空，不登记归档".to_string());
    }
    let mut archives = archived_private_bots();
    archives.retain(|item| item.id != id);
    archives.push(ArchivedPrivateBot {
        id: id.to_string(),
        name: name.to_string(),
        directory: directory.to_string(),
        archived_at: now_ms(),
    });
    save_archives(archives)
}

/// 解除保护：只删登记项，**目录与文件原样保留**（契约 §2.4，设置页 UI 待接）。
#[allow(dead_code)]
pub fn release_archive(id: &str) -> Result<(), String> {
    let mut archives = archived_private_bots();
    let before = archives.len();
    archives.retain(|item| item.id != id);
    if archives.len() == before {
        return Err(format!("没有这条归档记录：{id}"));
    }
    save_archives(archives)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// 规范化路径用于前缀比较：小写、去 `.`、解析 `..`（不要求存在）。
///
/// 不用 `canonicalize` 是为了让「准备新建的文件」也能判定；两侧得到同一套
/// 字面规则即可（Windows 大小写不敏感，统一小写）。
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    PathBuf::from(out.to_string_lossy().to_lowercase())
}

/// 目录归属：返回最长前缀匹配的目录（原始大小写）。
fn owner_of(path: &Path, dirs: &[(String, String)]) -> Option<(String, String)> {
    let target = normalize(path);
    let mut best: Option<(usize, (String, String))> = None;
    for (label, directory) in dirs {
        if directory.trim().is_empty() {
            continue;
        }
        let dir = normalize(Path::new(directory));
        if target == dir || target.starts_with(&dir) {
            let length = dir.as_os_str().len();
            if best.as_ref().is_none_or(|(current, _)| length > *current) {
                best = Some((length, (label.clone(), directory.clone())));
            }
        }
    }
    best.map(|(_, value)| value)
}

/// 所有「有主」的目录：Bot 工作目录（标签 `bot:<id>:<name>`）+ 归档隐私目录
/// （标签 `archived:<id>:<name>`）。
fn guarded_dirs() -> Vec<(String, String)> {
    let roster = crate::data::Roster::load();
    let mut dirs: Vec<(String, String)> = roster
        .bots
        .iter()
        .chain(roster.hidden_bots.iter())
        .filter(|bot| !bot.directory.trim().is_empty())
        .map(|bot| (format!("bot:{}:{}", bot.id, bot.name), bot.directory.clone()))
        .collect();
    for item in archived_private_bots() {
        dirs.push((
            format!("archived:{}:{}", item.id, item.name),
            item.directory.clone(),
        ));
    }
    dirs
}

/// 该工具是否带路径参数（只有这些参与归属判定）。
fn path_tools(tool_name: &str) -> bool {
    let lowered = tool_name.to_ascii_lowercase();
    matches!(lowered.as_str(), "read" | "write" | "edit" | "bash" | "powershell")
        || lowered.starts_with("mcp__")
}

/// 从工具参数里抽路径（`path` / `file_path` / `file`；bash 只看 `cwd`）。
fn path_from_args(tool_name: &str, args: &serde_json::Value) -> Option<String> {
    let lowered = tool_name.to_ascii_lowercase();
    if lowered == "bash" || lowered == "powershell" {
        // 命令内容无法静态判定；只在显式给了 cwd 时判定。
        return args
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
    }
    ["path", "file_path", "file", "filePath", "target"]
        .iter()
        .find_map(|key| args.get(*key).and_then(serde_json::Value::as_str))
        .map(str::to_string)
}

/// 越界判定：返回 `Some(归属标签)` 表示「不该由调用者访问」。
///
/// 规则：
/// - 没有路径参数 / 不是路径类工具 → `None`（交给原有审批档位）；
/// - 路径落在调用者自己的目录里 → `None`；
/// - 路径落在别人（或已归档隐私目录）里 → `Some("bot:<id>:<name>" | "archived:…")`。
pub fn precheck(
    tool_name: &str,
    args: &serde_json::Value,
    caller_bot_id: Option<&str>,
) -> Option<String> {
    if !path_tools(tool_name) {
        return None;
    }
    let raw = path_from_args(tool_name, args)?;
    if raw.trim().is_empty() {
        return None;
    }
    // 相对路径按调用者工作目录解析（审批卡上要显示绝对路径才看得懂）。
    let caller_dir = caller_bot_id
        .and_then(|id| crate::data::Roster::load().bot_by_id(id).cloned())
        .map(|bot| bot.directory)
        .filter(|directory| !directory.trim().is_empty());
    let candidate = PathBuf::from(&raw);
    let absolute = if candidate.is_absolute() {
        candidate
    } else if let Some(directory) = caller_dir.as_deref() {
        Path::new(directory).join(candidate)
    } else {
        candidate
    };
    let dirs = guarded_dirs();
    let owner = owner_of(&absolute, &dirs)?;
    // 自己目录直接放行。
    if let Some(caller_id) = caller_bot_id {
        let own_tag = format!("bot:{caller_id}:");
        if owner.0.starts_with(&own_tag) {
            return None;
        }
    }
    Some(owner.0)
}

/// 审批卡上的一句话（契约 §2.4 固定格式）。
pub fn reason_text(tool_name: &str, args: &serde_json::Value, owner: &str) -> String {
    let path = path_from_args(tool_name, args).unwrap_or_else(|| "?".to_string());
    let owner_label = if let Some(rest) = owner.strip_prefix("bot:") {
        rest.split_once(':').map(|(_, name)| name).unwrap_or(rest)
    } else if let Some(rest) = owner.strip_prefix("archived:") {
        let name = rest.split_once(':').map(|(_, name)| name).unwrap_or(rest);
        &format!("已归档隐私目录 {name}")
    } else {
        owner
    };
    format!("越界访问：{tool_name} 试图访问 {path}，归属 {owner_label}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_resolves_parent_dir_and_case() {
        let left = normalize(Path::new("C:\\Data\\A\\..\\B\\file.md"));
        let right = normalize(Path::new("c:\\data\\b\\file.md"));
        assert_eq!(left, right);
    }

    #[test]
    fn longest_prefix_wins() {
        let dirs = vec![
            ("bot:b1:外".to_string(), "C:\\ws".to_string()),
            ("bot:b2:内".to_string(), "C:\\ws\\private".to_string()),
        ];
        let owner = owner_of(Path::new("C:\\ws\\private\\a.md"), &dirs).unwrap();
        assert_eq!(owner.0, "bot:b2:内");
        let owner = owner_of(Path::new("C:\\ws\\other\\a.md"), &dirs).unwrap();
        assert_eq!(owner.0, "bot:b1:外");
        assert!(owner_of(Path::new("C:\\elsewhere\\a.md"), &dirs).is_none());
    }

    #[test]
    fn non_path_tools_are_ignored() {
        assert!(!path_tools("ask_user_question"));
        assert!(!path_tools("browser_open"));
        assert!(path_tools("read"));
        assert!(path_tools("mcp__github__create_issue"));
    }

    #[test]
    fn reason_text_matches_contract() {
        let args = serde_json::json!({"path": "C:\\other\\a.md"});
        let text = reason_text("read", &args, "bot:b2:小助手");
        assert_eq!(text, "越界访问：read 试图访问 C:\\other\\a.md，归属 小助手");
    }

    #[test]
    fn missing_archives_file_means_empty() {
        // 只验「解析失败/缺失都给空」，不碰真实数据根。
        let file: ArchivesFile = serde_json::from_str(r#"{"version":1,"archives":[]}"#).unwrap();
        assert!(file.archives.is_empty());
        let legacy: ArchivesFile = serde_json::from_str(r#"{"archives":[]}"#).unwrap();
        // 缺 version 时按 v1 兜底（`#[serde(default = "default_version")]`）。
        assert_eq!(legacy.version, 1);
        assert!(legacy.archives.is_empty());
        // 整个文件缺失也是空列表（read 里 `Err` 直接返回 Vec::new）。
        assert!(serde_json::from_str::<ArchivesFile>("").is_err());
    }
}
