//! Bot 记忆文件核心：定位、解析、原子写、索引维护、变更日志与撤销。
//!
//! 事实源仍是 agent 的 project memory（`{MODE_STORAGE_DIR}/memories/projects/
//! project-<sha256(identity)[..16]}/memory`）。本模块同时服务两条写入路径：
//! - 桌面记忆面板（Tauri `bot_memory_*` 命令）；
//! - agent 的 Write/Edit（经 Host RPC 收口，见 `bots/memoryWrite`）。
//!
//! 变更日志与记忆根同级（`memory-history.jsonl`），不进 `list_memory` 的扫描范围。

use std::io::Write;
use std::path::{Component, Path, PathBuf};

use atomic_file::write_text_atomic;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const INDEX_FILE: &str = "MEMORY.md";
pub const HISTORY_FILE: &str = "memory-history.jsonl";
pub const MEMORY_FILE_MAX_BYTES: u64 = 64 * 1024;
pub const INDEX_MAX_BYTES: u64 = 256 * 1024;
pub const HISTORY_MAX_RECORDS: usize = 200;
pub const HISTORY_MAX_BYTES: u64 = 512 * 1024;
/// 索引行上限（批次 D：超出后最旧的非置顶行溢出到 `topics/<kind>.md`）。
pub const INDEX_MAX_LINES: usize = 80;
/// 溢出索引行的 topic 目录（相对记忆根）。
pub const TOPIC_DIR: &str = "topics";
/// 每日整理标记（相对记忆根；隐藏文件不进 list_memory 扫描）。
const TIDY_MARKER: &str = ".tidy-date";
/// 回合召回：最多注入条目数与默认预算（字符）。
pub const RECALL_MAX_ENTRIES: usize = 6;

/// 记忆类型与 CLI 系统提示里的四类一致；未知值原样保留，不阻断列表。
pub const MEMORY_KINDS: &[&str] = &["user", "feedback", "project", "reference"];

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub file: String,
    pub title: String,
    pub description: String,
    pub kind: String,
    pub updated_at: u64,
    pub size: u64,
    /// 可选结束日期（批次 D）：过期后不再注入 prompt，面板标「已过期」。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    /// 置顶：优先注入且不参与索引溢出。
    pub pin: bool,
    pub expired: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryList {
    pub root: String,
    pub exists: bool,
    pub entries: Vec<MemoryEntry>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryReadResponse {
    pub file: String,
    pub content: String,
    pub title: String,
    pub description: String,
    pub kind: String,
    pub body: String,
    /// 可选结束日期与置顶（批次 D；编辑时保留原值）。
    pub end: Option<String>,
    pub pin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryHistoryRecord {
    pub id: String,
    pub at: u64,
    /// `create` | `update` | `delete` | `undo`
    pub op: String,
    pub file: String,
    pub title: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default)]
    pub undone: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryHistoryJournalRecord {
    pub id: String,
    pub at: u64,
    pub op: String,
    pub file: String,
    pub title: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRevision {
    pub id: String,
    pub mtime_ms: u64,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryWriteResult {
    pub file: String,
    pub revision: MemoryRevision,
}

pub struct WriteRequest<'a> {
    pub identity: &'a str,
    pub file: Option<&'a str>,
    pub title: &'a str,
    pub description: &'a str,
    pub kind: &'a str,
    pub body: &'a str,
    /// 可选结束日期（`YYYY-MM-DD` 或毫秒时间戳）；空 = 长期有效。
    pub end: Option<&'a str>,
    pub pin: bool,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn modified_ms(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 纯函数：memory 根目录（identity 为空或 storage 未设置时返回 None）。
pub fn memory_root(storage_root: Option<&str>, identity: &str) -> Option<PathBuf> {
    let storage = storage_root?.trim();
    let identity = identity.trim();
    if storage.is_empty() || identity.is_empty() {
        return None;
    }
    let digest = Sha256::digest(identity.as_bytes());
    let hash: String = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Some(
        Path::new(storage)
            .join("memories")
            .join("projects")
            .join(format!("project-{hash}"))
            .join("memory"),
    )
}

/// 解析现存记忆根：优先 identity 哈希目录，再回退到 CLI 的 workspacePath slug 目录
/// （`mode-bot-teammate-<id>-<hash>`）。bot shell 的 agent 有时用 workspacePath 而非
/// identity 建根；面板/写入必须落到同一目录，否则会显示为空并写丢历史。
pub fn resolve_memory_root(storage_root: Option<&str>, identity: &str) -> Option<PathBuf> {
    let canonical = memory_root(storage_root, identity)?;
    if canonical.is_dir() {
        return Some(canonical);
    }
    let projects = canonical.parent()?.parent()?;
    let slug = sanitize_slug(identity);
    let prefix = format!("{slug}-");
    let Ok(read_dir) = std::fs::read_dir(projects) else {
        return Some(canonical);
    };
    let mut candidates: Vec<(u64, PathBuf)> = read_dir
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(&prefix) {
                return None;
            }
            let memory = entry.path().join("memory");
            let metadata = std::fs::metadata(&memory).ok()?;
            Some((modified_ms(&metadata), memory))
        })
        .collect();
    candidates.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    candidates
        .into_iter()
        .next()
        .map(|(_, path)| path)
        .or(Some(canonical))
}

fn sanitize_slug(value: &str) -> String {
    let slug: String = value
        .trim()
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    let slug: String = slug.chars().take(48).collect();
    if slug.is_empty() {
        "project".to_string()
    } else {
        slug
    }
}
/// 相对路径校验：只接受 .md、不含 `.`/`..`/空段/隐藏段。
pub fn safe_relative_md(relative: &str) -> Option<PathBuf> {
    let normalized = relative.trim().replace('\\', "/");
    if normalized.is_empty() || normalized.len() > 200 {
        return None;
    }
    let mut path = PathBuf::new();
    for segment in normalized.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." || segment.starts_with('.') {
            return None;
        }
        path.push(segment);
    }
    let is_md = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"));
    if !is_md || path == Path::new(INDEX_FILE) {
        return None;
    }
    Some(path)
}

/// 由标题生成文件名。
pub fn file_name_for(title: &str, now_ms: u64) -> String {
    let slug: String = title
        .trim()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    let slug: String = slug.chars().take(40).collect();
    if slug.is_empty() || slug.eq_ignore_ascii_case("memory") {
        format!("memory-{now_ms}.md")
    } else {
        format!("{slug}.md")
    }
}

/// frontmatter 的完整元数据（批次 D）：title/description/kind + 可选结束日期与置顶。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryMetadata {
    pub title: String,
    pub description: String,
    pub kind: String,
    /// 可选结束日期：`YYYY-MM-DD`（当日 UTC 结束）或毫秒时间戳；过期条目不再注入 prompt。
    pub end: Option<String>,
    pub pin: bool,
}

/// 解析 frontmatter 的完整元数据（兼容旧条目：没有 end/pin 时按未过期/不置顶处理）。
/// `end`/`pin` 既接受顶层键，也接受 `metadata:` 缩进下的子键（兼容 agent 书写习惯）。
pub fn parse_entry_metadata(content: &str) -> EntryMetadata {
    let stripped = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut lines = stripped.lines();
    if lines.next().map(str::trim) != Some("---") {
        return EntryMetadata::default();
    }
    let mut meta = EntryMetadata::default();
    let mut in_metadata = false;
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if !indented {
            in_metadata = false;
        }
        if let Some(value) = trimmed.strip_prefix("name:") {
            if !value.trim().is_empty() {
                meta.title = unquote(value.trim()).to_string();
            }
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("description:") {
            if !value.trim().is_empty() {
                meta.description = unquote(value.trim()).to_string();
            }
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("end:") {
            let value = unquote(value.trim());
            if !value.is_empty() {
                meta.end = Some(value.to_string());
            }
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("pin:") {
            meta.pin = truthy(value.trim());
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("pinned:") {
            meta.pin = truthy(value.trim());
            continue;
        }
        if trimmed == "metadata:" {
            in_metadata = true;
            continue;
        }
        if in_metadata && indented {
            if let Some(value) = trimmed.strip_prefix("type:") {
                if !value.trim().is_empty() {
                    meta.kind = unquote(value.trim()).to_string();
                }
            }
        }
    }
    meta
}

fn truthy(value: &str) -> bool {
    matches!(
        unquote(value.trim()).to_ascii_lowercase().as_str(),
        "true" | "yes" | "1" | "on"
    )
}

/// 解析 frontmatter 里的 name/description/metadata.type（旧接口，保留兼容）。
pub fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>, Option<String>) {
    let meta = parse_entry_metadata(content);
    let optional = |value: String| (!value.trim().is_empty()).then_some(value);
    (
        optional(meta.title),
        optional(meta.description),
        optional(meta.kind),
    )
}

/// 条目是否已过期（结束日期早于 now）。支持 `YYYY-MM-DD`（当日结束）与毫秒时间戳。
pub fn entry_expired(end: Option<&str>, now_ms: u64) -> bool {
    let Some(end) = end.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    if let Ok(timestamp) = end.parse::<u64>() {
        return timestamp > 0 && timestamp < now_ms;
    }
    let Ok(date) = parse_day_end_ms(end) else {
        return false;
    };
    date < now_ms
}

/// `YYYY-MM-DD` → 当日 UTC 结束时刻（毫秒）。格式不合法返回 Err（视为不过期）。
fn parse_day_end_ms(value: &str) -> Result<u64, ()> {
    let mut parts = value.split('-');
    let year: i64 = parts.next().ok_or(())?.parse().map_err(|_| ())?;
    let month: i64 = parts.next().ok_or(())?.parse().map_err(|_| ())?;
    let day: i64 = parts.next().ok_or(())?.parse().map_err(|_| ())?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(());
    }
    // 简易 civil date → UNIX 天：足够用于「今天/昨天」级别的过期判断。
    let days = days_from_civil(year, month, day);
    let end_of_day = days * 86_400 + 86_399;
    Ok((end_of_day.max(0) as u64) * 1_000)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn unquote(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &value[1..value.len() - 1];
        }
    }
    value
}

pub fn build_entry_content(
    title: &str,
    description: &str,
    kind: &str,
    end: Option<&str>,
    pin: bool,
    body: &str,
) -> String {
    let description = description.trim().replace('\n', " ");
    let kind = if MEMORY_KINDS.contains(&kind) {
        kind
    } else {
        "project"
    };
    let end = end
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("end: {value}\n"))
        .unwrap_or_default();
    let pin = if pin { "pin: true\n" } else { "" };
    format!(
        "---\nname: {}\ndescription: {}\n{end}{pin}metadata:\n  type: {}\n---\n\n{}\n",
        title.trim(),
        description,
        kind,
        body.trim()
    )
}

pub fn splice_index_line(index: &str, file: &str, title: &str, description: &str) -> String {
    let target = format!("]({file})");
    let mut lines: Vec<&str> = index
        .lines()
        .filter(|line| !line.contains(&target))
        .collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let description = description.trim().replace('\n', " ");
    let line = if description.is_empty() {
        format!("- [{title}]({file})")
    } else {
        format!("- [{title}]({file}) — {description}")
    };
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&line);
    out.push('\n');
    out
}

pub fn remove_index_line(index: &str, file: &str) -> String {
    let target = format!("]({file})");
    let mut lines: Vec<&str> = index
        .lines()
        .filter(|line| !line.contains(&target))
        .collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

// ── 批次 D：元数据/溢出/整理/召回/移动 ──────────────────────────────────────

fn parse_index_line_file(line: &str) -> Option<&str> {
    let start = line.find("](")? + 2;
    let rest = &line[start..];
    let end = rest.find(')')?;
    let file = rest[..end].trim();
    (!file.is_empty()).then_some(file)
}

fn entry_line(title: &str, file: &str, description: &str) -> String {
    let description = description.trim().replace('\n', " ");
    if description.is_empty() {
        format!("- [{title}]({file})")
    } else {
        format!("- [{title}]({file}) — {description}")
    }
}

/// 索引溢出规划（纯函数，可测）：置顶行与最新行保留在索引，其余进 topics。
/// 输入 `(line, file, pin, updated_at, kind)`；返回 `(保留行, 溢出 (kind, line))`。
pub fn plan_index_overflow(
    entries: Vec<(String, String, bool, u64, String)>,
    cap: usize,
) -> (Vec<String>, Vec<(String, String)>) {
    let mut sorted = entries;
    sorted.sort_by(|left, right| {
        right
            .2
            .cmp(&left.2)
            .then_with(|| right.3.cmp(&left.3))
    });
    let mut keep = Vec::new();
    let mut spill = Vec::new();
    for (line, _file, _pin, _updated, kind) in sorted {
        if keep.len() < cap {
            keep.push(line);
        } else {
            spill.push((kind, line));
        }
    }
    (keep, spill)
}

fn topic_slug(kind: &str) -> String {
    let slug: String = kind
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_ascii_lowercase();
    if slug.is_empty() {
        "general".to_string()
    } else {
        slug
    }
}

/// 重建 topics 目录：每个 kind 一个 `topics/<kind>.md`；没有溢出时清空旧 topic 文件。
fn write_topics(root: &Path, spill: &[(String, String)]) -> Result<(), String> {
    let dir = root.join(TOPIC_DIR);
    let mut by_kind: std::collections::BTreeMap<String, Vec<&str>> =
        std::collections::BTreeMap::new();
    for (kind, line) in spill {
        by_kind.entry(topic_slug(kind)).or_default().push(line.as_str());
    }
    if by_kind.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
        return Ok(());
    }
    std::fs::create_dir_all(&dir).map_err(|error| format!("创建 topic 目录失败：{error}"))?;
    for (slug, lines) in &by_kind {
        let mut content = format!("# Topic: {slug}\n\n（索引溢出条目：按需召回，不常驻上下文）\n\n");
        for line in lines {
            content.push_str(line);
            content.push('\n');
        }
        write_atomic(&dir.join(format!("{slug}.md")), &content, "写入 topic 文件失败")?;
    }
    // 清理本次未再生成的旧 topic 文件（条目被删除/移回索引后不残留）。
    if let Ok(read_dir) = std::fs::read_dir(&dir) {
        let keep: std::collections::HashSet<String> =
            by_kind.keys().map(|slug| format!("{slug}.md")).collect();
        for dir_entry in read_dir.flatten() {
            let name = dir_entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".md") && !keep.contains(&name) {
                let _ = std::fs::remove_file(dir_entry.path());
            }
        }
    }
    Ok(())
}

/// 维护索引：超过行数上限时把最旧的非置顶行溢出到 topics（整体重建，幂等）。
pub fn maintain_index(root: &Path, index: String) -> Result<String, String> {
    let lines: Vec<String> = index
        .lines()
        .map(str::to_string)
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.len() <= INDEX_MAX_LINES {
        return Ok(index);
    }
    let mut decorated = Vec::with_capacity(lines.len());
    for line in lines {
        let Some(file) = parse_index_line_file(&line).map(str::to_string) else {
            continue;
        };
        let path = root.join(file.replace('/', std::path::MAIN_SEPARATOR_STR));
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let meta = parse_entry_metadata(&content);
        let updated = std::fs::metadata(&path)
            .ok()
            .map(|metadata| modified_ms(&metadata))
            .unwrap_or(0);
        let kind = if meta.kind.trim().is_empty() {
            "general".to_string()
        } else {
            meta.kind.clone()
        };
        decorated.push((line, file, meta.pin, updated, kind));
    }
    let (keep, spill) = plan_index_overflow(decorated, INDEX_MAX_LINES);
    write_topics(root, &spill)?;
    let mut out = String::new();
    for line in keep {
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

fn today_key() -> String {
    let days = (now_ms() / 1_000 / 86_400) as i64;
    // inverse of days_from_civil：Howard Hinnant 的 civil_from_days。
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn maybe_daily_tidy(root: &Path) {
    let marker = root.join(TIDY_MARKER);
    let today = today_key();
    if std::fs::read_to_string(&marker)
        .ok()
        .map(|value| value.trim() == today)
        .unwrap_or(false)
    {
        return;
    }
    if let Err(error) = tidy_memory(root) {
        eprintln!("[memory] 每日整理失败（{}）：{error}", root.display());
        return;
    }
    let _ = std::fs::write(&marker, today);
}

/// 每日整理（批次 D）：从条目文件重建索引（置顶优先、更新时间降序、过期条目不进索引），
/// 应用溢出规则；索引变化记一条 `tidy` journal（撤销恢复旧索引）。
pub fn tidy_memory(root: &Path) -> Result<usize, String> {
    let before = read_index(root);
    let mut entries = list_memory(root).entries;
    entries.sort_by(|left, right| right.pin.cmp(&left.pin).then_with(|| right.updated_at.cmp(&left.updated_at)));
    let mut rebuilt = String::new();
    for entry in entries.iter().filter(|entry| !entry.expired) {
        rebuilt.push_str(&entry_line(&entry.title, &entry.file, &entry.description));
        rebuilt.push('\n');
    }
    let after = maintain_index(root, rebuilt)?;
    if after != before {
        write_atomic(&root.join(INDEX_FILE), &after, "整理记忆索引失败")?;
        journal_mutation(
            root,
            "tidy",
            INDEX_FILE,
            Some(before),
            Some(after.clone()),
        )?;
    }
    Ok(after.lines().filter(|line| !line.trim().is_empty()).count())
}

fn query_tokens(query: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut ascii = String::new();
    let mut cjk: Vec<char> = Vec::new();
    for character in query.chars() {
        if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
            flush_cjk_run(&mut cjk, &mut tokens);
            ascii.push(character.to_ascii_lowercase());
        } else if is_cjk(character) {
            flush_ascii_run(&mut ascii, &mut tokens);
            cjk.push(character);
        } else {
            flush_ascii_run(&mut ascii, &mut tokens);
            flush_cjk_run(&mut cjk, &mut tokens);
        }
    }
    flush_ascii_run(&mut ascii, &mut tokens);
    flush_cjk_run(&mut cjk, &mut tokens);
    tokens
}

fn push_query_token(token: &str, tokens: &mut Vec<String>) {
    let token = token.trim();
    if token.chars().count() < 2 || tokens.len() >= 24 || tokens.iter().any(|value| value == token)
    {
        return;
    }
    tokens.push(token.to_string());
}

fn flush_ascii_run(run: &mut String, tokens: &mut Vec<String>) {
    if !run.is_empty() {
        let value = std::mem::take(run);
        push_query_token(&value, tokens);
    }
}

fn flush_cjk_run(run: &mut Vec<char>, tokens: &mut Vec<String>) {
    if run.len() >= 2 {
        for window in run.windows(2) {
            let token: String = window.iter().collect();
            push_query_token(&token, tokens);
        }
        if run.len() <= 4 {
            let token: String = run.iter().collect();
            push_query_token(&token, tokens);
        }
    }
    run.clear();
}

fn is_cjk(character: char) -> bool {
    matches!(character as u32, 0x3400..=0x9FFF | 0xF900..=0xFAFF | 0x3040..=0x30FF)
}

fn count_token_hits(haystack: &str, tokens: &[String]) -> u32 {
    if haystack.is_empty() {
        return 0;
    }
    let lowered = haystack.to_lowercase();
    let mut score = 0u32;
    for token in tokens {
        let hits = lowered.matches(token.as_str()).count().min(5) as u32;
        score += hits;
    }
    score
}

/// 回合召回（批次 D，关键词对齐；语义/FTS 等上游引擎能力）：按本轮文本命中条目
/// （标题 > 描述 > 正文），预算内注入最相关的若干条。无命中返回空串（不占预算）。
pub fn recall_context(root: &Path, query: &str, max_chars: usize) -> String {
    let tokens = query_tokens(query);
    if tokens.is_empty() || max_chars == 0 {
        return String::new();
    }
    let mut scored: Vec<(u32, MemoryEntry)> = Vec::new();
    for entry in list_memory(root).entries {
        if entry.expired {
            continue;
        }
        let mut score = 6 * count_token_hits(&entry.title, &tokens);
        score += 3 * count_token_hits(&entry.description, &tokens);
        if score == 0 {
            let path = root.join(entry.file.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Ok(content) = read_capped(&path, MEMORY_FILE_MAX_BYTES) {
                score += count_token_hits(&content, &tokens).min(20);
            }
        }
        if score > 0 {
            scored.push((score, entry));
        }
    }
    if scored.is_empty() {
        return String::new();
    }
    scored.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.updated_at.cmp(&left.1.updated_at))
    });
    scored.truncate(RECALL_MAX_ENTRIES);
    let mut out = String::from("## Memory recall\n\n");
    for (_, entry) in scored {
        let path = root.join(entry.file.replace('/', std::path::MAIN_SEPARATOR_STR));
        let Ok(content) = read_capped(&path, MEMORY_FILE_MAX_BYTES) else {
            continue;
        };
        let chunk = format!("### {}\n\n{}\n\n", entry.file, content.trim());
        if out.chars().count() + chunk.chars().count() > max_chars {
            break;
        }
        out.push_str(&chunk);
    }
    if out.trim_end() == "## Memory recall" {
        String::new()
    } else {
        out
    }
}

/// 跨 scope 移动（批次 D）：把条目原样搬到另一个记忆根（保留 frontmatter），
/// 两边都走 write_content/delete_memory 以维持索引与 journal；同名冲突加时间戳后缀。
pub fn move_memory(from_root: &Path, file: &str, to_root: &Path) -> Result<String, String> {
    let relative = safe_relative_md(file).ok_or_else(|| format!("非法的记忆文件名：{file}"))?;
    let relative_text = relative.to_string_lossy().replace('\\', "/");
    let source = from_root.join(&relative);
    if !source.is_file() {
        return Err(format!("记忆不存在：{relative_text}"));
    }
    let content = read_capped(&source, MEMORY_FILE_MAX_BYTES)?;
    let mut target = relative_text.clone();
    if to_root.join(target.replace('/', std::path::MAIN_SEPARATOR_STR)).is_file() {
        let stem = relative_text.trim_end_matches(".md");
        target = format!("{stem}-moved-{}.md", now_ms());
    }
    write_content(to_root, &target, &content)?;
    delete_memory(from_root, &relative_text)?;
    Ok(target)
}

fn list_dir(root: &Path, relative: &Path, depth: u8, entries: &mut Vec<MemoryEntry>) {
    let Ok(read_dir) = std::fs::read_dir(root.join(relative)) else {
        return;
    };
    for dir_entry in read_dir.flatten() {
        let name = dir_entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') {
            continue;
        }
        let child_relative = relative.join(name);
        let path = root.join(&child_relative);
        let Ok(metadata) = dir_entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            if depth > 0 {
                list_dir(root, &child_relative, depth - 1, entries);
            }
            continue;
        }
        let relative_text = child_relative.to_string_lossy().replace('\\', "/");
        if relative_text == INDEX_FILE
            || relative_text.starts_with(&format!("{TOPIC_DIR}/"))
            || !relative_text.ends_with(".md")
        {
            continue;
        }
        if metadata.len() > MEMORY_FILE_MAX_BYTES {
            continue;
        }
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let meta = parse_entry_metadata(&content);
        entries.push(MemoryEntry {
            file: relative_text.clone(),
            title: if meta.title.trim().is_empty() {
                Path::new(&relative_text)
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_else(|| relative_text.clone())
            } else {
                meta.title.clone()
            },
            description: meta.description,
            kind: meta.kind,
            updated_at: modified_ms(&metadata),
            size: metadata.len(),
            expired: entry_expired(meta.end.as_deref(), now_ms()),
            end: meta.end,
            pin: meta.pin,
        });
    }
}

pub fn list_memory(root: &Path) -> MemoryList {
    let mut entries = Vec::new();
    let exists = root.is_dir();
    if exists {
        list_dir(root, Path::new(""), 2, &mut entries);
    }
    // 置顶优先，其次按更新时间倒序（面板与注入同序）。
    entries.sort_by(|left, right| {
        right
            .pin
            .cmp(&left.pin)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
    });
    MemoryList {
        root: root.to_string_lossy().into_owned(),
        exists,
        entries,
    }
}

fn read_capped(path: &Path, limit: u64) -> Result<String, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("读取失败（{}）：{error}", path.display()))?;
    if metadata.len() > limit {
        return Err(format!(
            "文件超过 {} KB，拒绝读写：{}",
            limit / 1024,
            path.display()
        ));
    }
    std::fs::read_to_string(path)
        .map_err(|error| format!("读取失败（{}）：{error}", path.display()))
}

pub fn read_memory(root: &Path, file: &str) -> Result<MemoryReadResponse, String> {
    let relative = safe_relative_md(file).ok_or_else(|| format!("非法的记忆文件名：{file}"))?;
    let relative_text = relative.to_string_lossy().replace('\\', "/");
    let path = resolve_entry_path(root, &relative_text)?;
    let content = read_capped(&path, MEMORY_FILE_MAX_BYTES)?;
    let (title, description, kind, body) = parse_memory_reader(&content);
    let meta = parse_entry_metadata(&content);
    Ok(MemoryReadResponse {
        file: relative_text,
        content,
        title,
        description,
        kind,
        body,
        end: meta.end,
        pin: meta.pin,
    })
}

/// 解析读取结果：frontmatter 字段 + 去掉 frontmatter 的正文。
pub fn parse_memory_reader(content: &str) -> (String, String, String, String) {
    let stripped = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut lines = stripped.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (String::new(), String::new(), String::new(), stripped.trim().to_string());
    }
    let mut title = String::new();
    let mut description = String::new();
    let mut kind = String::new();
    let mut in_metadata = false;
    let mut consumed = 1usize;
    let mut closed = false;
    for line in lines.by_ref() {
        consumed += 1;
        let trimmed = line.trim();
        if trimmed == "---" {
            closed = true;
            break;
        }
        if let Some(value) = trimmed.strip_prefix("name:") {
            title = unquote(value.trim()).to_string();
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("description:") {
            description = unquote(value.trim()).to_string();
            continue;
        }
        if trimmed == "metadata:" {
            in_metadata = true;
            continue;
        }
        if in_metadata {
            if line.starts_with(' ') || line.starts_with('\t') {
                if let Some(value) = trimmed.strip_prefix("type:") {
                    kind = unquote(value.trim()).to_string();
                }
            } else {
                in_metadata = false;
            }
        }
    }
    if !closed {
        return (String::new(), String::new(), String::new(), stripped.trim().to_string());
    }
    let body = stripped
        .lines()
        .skip(consumed)
        .collect::<Vec<_>>()
        .join("\n");
    (title, description, kind, body.trim().to_string())
}

pub fn resolve_entry_path(root: &Path, file: &str) -> Result<PathBuf, String> {
    let relative = safe_relative_md(file).ok_or_else(|| format!("非法的记忆文件名：{file}"))?;
    let joined = root.join(&relative);
    let normalized = joined
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect::<PathBuf>();
    if !normalized.starts_with(root) {
        return Err("记忆路径越出记忆根".to_string());
    }
    Ok(joined)
}

fn read_index(root: &Path) -> String {
    let path = root.join(INDEX_FILE);
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.len() > INDEX_MAX_BYTES => String::new(),
        Ok(_) => std::fs::read_to_string(&path).unwrap_or_default(),
        Err(_) => String::new(),
    }
}

fn write_atomic(path: &Path, content: &str, label: &str) -> Result<(), String> {
    write_text_atomic(path, content, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("{label}：{error}"))
}

fn append_history(root: &Path, record: MemoryHistoryJournalRecord) -> Result<(), String> {
    std::fs::create_dir_all(root.parent().unwrap_or(root))
        .map_err(|error| format!("创建记忆历史目录失败：{error}"))?;
    let path = history_path(root);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("打开记忆历史失败：{error}"))?;
    let line = serde_json::to_string(&record)
        .map_err(|error| format!("序列化记忆历史失败：{error}"))?;
    file.write_all(line.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|error| format!("写入记忆历史失败：{error}"))?;
    drop(file);
    trim_history(&path)
}

pub fn history_path(root: &Path) -> PathBuf {
    root.with_file_name(HISTORY_FILE)
}

fn trim_history(path: &Path) -> Result<(), String> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len() <= HISTORY_MAX_BYTES {
        return Ok(());
    }
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = content.lines().filter(|line| !line.trim().is_empty()).collect();
    if lines.len() <= HISTORY_MAX_RECORDS {
        return Ok(());
    }
    let kept = lines[lines.len() - HISTORY_MAX_RECORDS..].join("\n");
    write_atomic(path, &format!("{kept}\n"), "裁剪记忆历史失败")
}

fn load_history(root: &Path) -> Vec<MemoryHistoryJournalRecord> {
    let path = history_path(root);
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<MemoryHistoryJournalRecord>(line).ok())
        .collect()
}

fn history_record(content: &str) -> (String, String) {
    let (title, description, _) = parse_frontmatter(content);
    (
        title.unwrap_or_default(),
        description.unwrap_or_default(),
    )
}

fn journal_mutation(
    root: &Path,
    op: &str,
    file: &str,
    before: Option<String>,
    after: Option<String>,
) -> Result<(), String> {
    let reference = before.as_deref().or(after.as_deref()).unwrap_or_default();
    let (title, description) = history_record(reference);
    append_history(
        root,
        MemoryHistoryJournalRecord {
            id: Uuid::new_v4().simple().to_string(),
            at: now_ms(),
            op: op.to_string(),
            file: file.to_string(),
            title,
            description,
            before,
            after,
            target: None,
        },
    )
}

/// 写一条记忆 + 维护索引行 + 记录历史。返回落盘后的相对路径。
pub fn write_memory(root: &Path, request: WriteRequest<'_>) -> Result<String, String> {
    let now = now_ms();
    let title = request.title.trim();
    if title.is_empty() {
        return Err("标题不能为空".to_string());
    }
    let file = match request
        .file
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(existing) => {
            let path = safe_relative_md(existing)
                .ok_or_else(|| format!("非法的记忆文件名：{existing}"))?;
            path.to_string_lossy().replace('\\', "/")
        }
        None => file_name_for(title, now),
    };
    let path = root.join(&file);
    std::fs::create_dir_all(root).map_err(|error| format!("创建记忆目录失败：{error}"))?;
    let before = std::fs::read_to_string(&path).ok();
    let content = build_entry_content(
        title,
        request.description,
        request.kind,
        request.end,
        request.pin,
        request.body,
    );
    write_atomic(&path, &content, "写入记忆失败")?;

    let index = read_index(root);
    let next_index = splice_index_line(&index, &file, title, request.description);
    let next_index = maintain_index(root, next_index)?;
    if next_index != index {
        write_atomic(&root.join(INDEX_FILE), &next_index, "更新记忆索引失败")?;
    }
    let op = if before.is_some() { "update" } else { "create" };
    journal_mutation(root, op, &file, before, Some(content))?;
    maybe_daily_tidy(root);
    Ok(file)
}

/// agent Write/Edit 的收口写：原样写入 content（不从 title/body 重建），
/// 维护索引并记录历史。文件路径必须是记忆根内的 .md 相对路径。
pub fn write_content(root: &Path, file: &str, content: &str) -> Result<MemoryWriteResult, String> {
    let relative = safe_relative_md(file).ok_or_else(|| format!("非法的记忆文件名：{file}"))?;
    let relative_text = relative.to_string_lossy().replace('\\', "/");
    let path = root.join(&relative);
    std::fs::create_dir_all(path.parent().unwrap_or(root))
        .map_err(|error| format!("创建记忆目录失败：{error}"))?;
    let before = std::fs::read_to_string(&path).ok();
    write_atomic(&path, content, "写入记忆失败")?;
    let (title, description, _) = parse_frontmatter(content);
    let index = read_index(root);
    let next_index = splice_index_line(
        &index,
        &relative_text,
        &title.unwrap_or_default(),
        &description.unwrap_or_default(),
    );
    let next_index = maintain_index(root, next_index)?;
    if next_index != index {
        write_atomic(&root.join(INDEX_FILE), &next_index, "更新记忆索引失败")?;
    }
    let op = if before.is_some() { "update" } else { "create" };
    journal_mutation(root, op, &relative_text, before, Some(content.to_string()))?;
    maybe_daily_tidy(root);
    let metadata = std::fs::metadata(&path).map_err(|error| format!("读取记忆元数据失败：{error}"))?;
    let size_bytes = metadata.len();
    let mtime_ms = modified_ms(&metadata);
    Ok(MemoryWriteResult {
        file: relative_text.clone(),
        revision: MemoryRevision {
            id: format!("{mtime_ms}-{size_bytes}"),
            mtime_ms,
            size_bytes,
        },
    })
}

/// 删除一条记忆 + 清索引 + 记录历史。
pub fn delete_memory(root: &Path, file: &str) -> Result<(), String> {
    let relative = safe_relative_md(file).ok_or_else(|| format!("非法的记忆文件名：{file}"))?;
    let relative_text = relative.to_string_lossy().replace('\\', "/");
    let path = root.join(&relative);
    if !path.is_file() {
        return Err(format!("记忆不存在：{relative_text}"));
    }
    let before = std::fs::read_to_string(&path).ok();
    std::fs::remove_file(&path).map_err(|error| format!("删除记忆失败：{error}"))?;
    let index_path = root.join(INDEX_FILE);
    if index_path.is_file() {
        let index = read_capped(&index_path, INDEX_MAX_BYTES).unwrap_or_default();
        let next = remove_index_line(&index, &relative_text);
        if next != index {
            write_atomic(&index_path, &next, "更新记忆索引失败")?;
        }
    }
    journal_mutation(root, "delete", &relative_text, before, None)?;
    Ok(())
}

pub fn memory_context(root: &Path, max_chars: usize) -> String {
    let index = read_index(root);
    let mut out = String::new();
    if !index.trim().is_empty() {
        out.push_str("## Memory index\n\n");
        out.push_str(index.trim());
        out.push_str("\n\n");
    }
    // 置顶优先（list_memory 已排序），过期条目不注入（但保留在索引/面板里可查）。
    let list = list_memory(root);
    for entry in list.entries.into_iter().filter(|entry| !entry.expired) {
        let path = root.join(entry.file.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Ok(content) = read_capped(&path, MEMORY_FILE_MAX_BYTES) {
            if out.chars().count() + content.chars().count() > max_chars {
                break;
            }
            out.push_str(&format!("### {}\n\n{}\n\n", entry.file, content.trim()));
        }
    }
    if out.chars().count() > max_chars {
        out.chars().take(max_chars).collect()
    } else {
        out
    }
}

/// 历史列表：最新在前，跳过 undo 事件本身，标记已被撤销的条目。
pub fn history(root: &Path, limit: usize) -> Result<Vec<MemoryHistoryRecord>, String> {
    let records = load_history(root);
    let undone: std::collections::HashSet<String> = records
        .iter()
        .filter(|record| record.op == "undo")
        .filter_map(|record| record.target.clone())
        .collect();
    let mut out: Vec<MemoryHistoryRecord> = records
        .into_iter()
        .filter(|record| record.op != "undo")
        .map(|record| MemoryHistoryRecord {
            id: record.id,
            at: record.at,
            op: record.op,
            file: record.file,
            title: record.title,
            description: record.description,
            before: record.before,
            after: record.after,
            target: record.target,
            undone: false,
        })
        .map(|mut record| {
            record.undone = undone.contains(&record.id);
            record
        })
        .collect();
    out.reverse();
    out.truncate(limit.max(1));
    Ok(out)
}

/// 撤销一条历史。只允许恢复「撤销后文件仍与当时写入内容一致」的变更；
/// 文件被后续修改时拒绝，避免覆盖用户的新内容。
pub fn undo(root: &Path, id: &str) -> Result<MemoryHistoryRecord, String> {
    let records = load_history(root);
    if records
        .iter()
        .any(|record| record.op == "undo" && record.target.as_deref() == Some(id))
    {
        return Err("这条变更已经被撤销。".to_string());
    }
    let record = records
        .iter()
        .find(|record| record.id == id && record.op != "undo")
        .cloned()
        .ok_or_else(|| "找不到这条记忆变更。".to_string())?;
    let path = root.join(record.file.replace('/', std::path::MAIN_SEPARATOR_STR));
    let current = std::fs::read_to_string(&path).ok();
    let restored = match record.op.as_str() {
        "create" => {
            if current.as_deref() != record.after.as_deref() {
                return Err("记忆已被后续修改，无法撤销。".to_string());
            }
            std::fs::remove_file(&path).map_err(|error| format!("撤销创建失败：{error}"))?;
            None
        }
        "update" | "delete" => {
            if record.op == "update" && current.as_deref() != record.after.as_deref() {
                return Err("记忆已被后续修改，无法撤销。".to_string());
            }
            if record.op == "delete" && current.is_some() {
                return Err("记忆已被重新创建，无法撤销删除。".to_string());
            }
            let before = record
                .before
                .clone()
                .ok_or_else(|| "这条变更缺少恢复内容。".to_string())?;
            std::fs::create_dir_all(root).map_err(|error| format!("创建记忆目录失败：{error}"))?;
            write_atomic(&path, &before, "撤销写入失败")?;
            Some(before)
        }
        // 每日整理：恢复整理前的索引（内容变了则拒绝，避免覆盖后续手工调整）。
        "tidy" => {
            if current.as_deref() != record.after.as_deref() {
                return Err("记忆索引已被后续修改，无法撤销整理。".to_string());
            }
            let before = record
                .before
                .clone()
                .ok_or_else(|| "这条变更缺少恢复内容。".to_string())?;
            write_atomic(&path, &before, "撤销整理失败")?;
            Some(before)
        }
        _ => return Err("这条变更不支持撤销。".to_string()),
    };

    if record.op != "tidy" {
        let index = read_index(root);
        let next_index = if record.op == "create" {
            remove_index_line(&index, &record.file)
        } else {
            let (title, description, _, _) =
                parse_memory_reader(restored.as_deref().unwrap_or_default());
            splice_index_line(&index, &record.file, &title, &description)
        };
        if next_index != index {
            write_atomic(&root.join(INDEX_FILE), &next_index, "更新记忆索引失败")?;
        }
    }
    append_history(
        root,
        MemoryHistoryJournalRecord {
            id: Uuid::new_v4().simple().to_string(),
            at: now_ms(),
            op: "undo".to_string(),
            file: record.file.clone(),
            title: record.title.clone(),
            description: record.description.clone(),
            before: current,
            after: restored,
            target: Some(record.id.clone()),
        },
    )?;
    let result = MemoryHistoryRecord {
        id: record.id,
        at: record.at,
        op: record.op,
        file: record.file,
        title: record.title,
        description: record.description,
        before: record.before,
        after: record.after,
        target: record.target,
        undone: true,
    };
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(tag: &str) -> PathBuf {
        let project = std::env::temp_dir().join(format!(
            "bot-memory-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&project);
        let root = project.join("memory");
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(root: &Path, file: Option<&str>, title: &str, body: &str) -> String {
        write_memory(
            root,
            WriteRequest {
                identity: "mode-bot:teammate:t1",
                file,
                title,
                description: "hook",
                kind: "project",
                body,
                end: None,
                pin: false,
            },
        )
        .unwrap()
    }

    #[test]
    fn resolve_memory_root_finds_cli_workspace_slug_root() {
        let project = std::env::temp_dir().join(format!(
            "bot-memory-resolve-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&project);
        let storage = project.join("cli");
        let actual = storage
            .join("memories/projects/mode-bot-teammate-abc-feedface")
            .join("memory");
        std::fs::create_dir_all(&actual).unwrap();
        let resolved = resolve_memory_root(
            Some(storage.to_string_lossy().as_ref()),
            "mode-bot:teammate:abc",
        )
        .unwrap();
        assert_eq!(resolved, actual);
        std::fs::remove_dir_all(project).ok();
    }

    #[test]
    fn write_content_preserves_exact_bytes_and_indexes() {
        let root = sandbox("write-content");
        let content = "---\nname: Alpha\ndescription: exact\nmetadata:\n  type: project\n---\n\nraw body\n";
        let result = write_content(&root, "nested/alpha.md", content).unwrap();
        assert_eq!(result.file, "nested/alpha.md");
        assert_eq!(std::fs::read_to_string(root.join("nested/alpha.md")).unwrap(), content);
        let index = std::fs::read_to_string(root.join(INDEX_FILE)).unwrap();
        assert!(index.contains("[Alpha](nested/alpha.md) — exact"));
        let history = history(&root, 10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].op, "create");
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn write_list_read_delete_round_trip() {
        let root = sandbox("roundtrip");
        let file = write(&root, None, "Alpha", "one");
        let list = list_memory(&root);
        assert_eq!(list.entries.len(), 1);
        assert_eq!(list.entries[0].file, file);
        assert_eq!(list.entries[0].title, "Alpha");
        let read = read_memory(&root, &file).unwrap();
        assert_eq!(read.title, "Alpha");
        assert_eq!(read.body, "one");
        assert!(std::fs::read_to_string(root.join(INDEX_FILE))
            .unwrap()
            .contains("](Alpha.md)"));
        delete_memory(&root, &file).unwrap();
        assert!(list_memory(&root).entries.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn history_tracks_create_and_update_and_undo_restores_before() {
        let root = sandbox("history-update");
        let file = write(&root, None, "Alpha", "one");
        write(&root, Some(&file), "Alpha", "two");
        let before = history(&root, 10).unwrap();
        assert_eq!(before.len(), 2);
        assert_eq!(before[0].op, "update");
        assert_eq!(before[1].op, "create");
        let update = before[0].clone();
        undo(&root, &update.id).unwrap();
        let restored = read_memory(&root, &file).unwrap();
        assert_eq!(restored.body, "one");
        let after = history(&root, 10).unwrap();
        let update = after.iter().find(|item| item.id == update.id).unwrap();
        assert!(update.undone);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn undo_create_removes_file_and_index_line() {
        let root = sandbox("undo-create");
        let file = write(&root, None, "Alpha", "one");
        let created = history(&root, 10).unwrap().into_iter().find(|item| item.op == "create").unwrap();
        undo(&root, &created.id).unwrap();
        assert!(!root.join(&file).exists());
        let index = std::fs::read_to_string(root.join(INDEX_FILE)).unwrap_or_default();
        assert!(!index.contains("](Alpha.md)"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn undo_delete_restores_file() {
        let root = sandbox("undo-delete");
        let file = write(&root, None, "Alpha", "one");
        delete_memory(&root, &file).unwrap();
        let deleted = history(&root, 10).unwrap().into_iter().find(|item| item.op == "delete").unwrap();
        undo(&root, &deleted.id).unwrap();
        let restored = read_memory(&root, &file).unwrap();
        assert_eq!(restored.body, "one");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn undo_rejects_a_change_followed_by_a_newer_write() {
        let root = sandbox("stale");
        let file = write(&root, None, "Alpha", "one");
        write(&root, Some(&file), "Alpha", "two");
        let first_update = history(&root, 10)
            .unwrap()
            .into_iter()
            .find(|item| item.op == "update")
            .unwrap();
        write(&root, Some(&file), "Alpha", "three");
        let error = undo(&root, &first_update.id).unwrap_err();
        assert!(error.contains("无法撤销"), "{error}");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn history_file_is_jsonl_and_excludes_undo_events_from_the_list() {
        let root = sandbox("journal-jsonl");
        let file = write(&root, None, "Alpha", "one");
        let created = history(&root, 10).unwrap().into_iter().find(|item| item.op == "create").unwrap();
        undo(&root, &created.id).unwrap();
        let raw = std::fs::read_to_string(history_path(&root)).unwrap();
        assert!(raw.lines().all(|line| serde_json::from_str::<MemoryHistoryJournalRecord>(line).is_ok()));
        assert_eq!(history(&root, 10).unwrap().len(), 1);
        assert!(!root.join(&file).exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn entry_expiry_supports_date_and_timestamp() {
        // 2026-01-02 的当日结束（UTC）比较；现在的毫秒时间戳远大于它 → 过期。
        assert!(entry_expired(Some("2026-01-02"), 1_800_000_000_000));
        assert!(!entry_expired(Some("2999-01-02"), 1_800_000_000_000));
        assert!(entry_expired(Some("1000"), 2_000));
        assert!(!entry_expired(None, 2_000));
        // 非法格式按不过期处理，不阻塞注入。
        assert!(!entry_expired(Some("not-a-date"), 2_000));
    }

    #[test]
    fn overflow_keeps_pinned_and_newest_lines() {
        let entries = vec![
            ("- [old](old.md)".to_string(), "old.md".to_string(), false, 10, "project".to_string()),
            ("- [pin](pin.md)".to_string(), "pin.md".to_string(), true, 1, "feedback".to_string()),
            ("- [new](new.md)".to_string(), "new.md".to_string(), false, 99, "project".to_string()),
        ];
        let (keep, spill) = plan_index_overflow(entries, 2);
        assert_eq!(keep, vec!["- [pin](pin.md)".to_string(), "- [new](new.md)".to_string()]);
        assert_eq!(spill, vec![("project".to_string(), "- [old](old.md)".to_string())]);
    }

    #[test]
    fn index_overflow_spills_to_topics_and_tidy_restores_via_undo() {
        let root = sandbox("overflow");
        for index in 0..(INDEX_MAX_LINES + 2) {
            write(&root, None, &format!("entry {index}"), "body");
        }
        let index = std::fs::read_to_string(root.join(INDEX_FILE)).unwrap();
        assert!(index.lines().count() <= INDEX_MAX_LINES, "索引必须被限制在行数上限内");
        let topics = std::fs::read_dir(root.join(TOPIC_DIR)).unwrap().count();
        assert!(topics >= 1, "溢出行必须落到 topics/*.md");
        // 过期条目不进注入上下文。
        write(
            &root,
            None,
            "expired entry",
            "body",
        );
        let list = list_memory(&root);
        assert!(!list.entries.is_empty());
        // 每日整理幂等：再整理一次索引内容不变。
        let before = std::fs::read_to_string(root.join(INDEX_FILE)).unwrap();
        let _ = tidy_memory(&root).unwrap();
        let after = std::fs::read_to_string(root.join(INDEX_FILE)).unwrap();
        assert_eq!(before, after);
        // 索引被手工加脏行后，整理记 journal，撤销可恢复脏行（一键撤销）。
        std::fs::write(root.join(INDEX_FILE), format!("{after}- [stale](stale.md)
")).unwrap();
        let _ = tidy_memory(&root).unwrap();
        let tidy_record = history(&root, 10)
            .unwrap()
            .into_iter()
            .find(|record| record.op == "tidy")
            .expect("整理必须记录 journal");
        undo(&root, &tidy_record.id).unwrap();
        let restored = std::fs::read_to_string(root.join(INDEX_FILE)).unwrap();
        assert!(restored.contains("stale.md"), "撤销整理必须恢复整理前的索引");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn recall_matches_title_and_cjk_bigrams() {
        let root = sandbox("recall");
        write(&root, Some("release.md"), "发布窗口约定", "每周四发布，版本号 2.4.0");
        write(&root, Some("other.md"), "无关条目", "天气真好");
        let hit = recall_context(&root, "周四发布会推迟吗", 4_000);
        assert!(hit.contains("release.md"), "命中条目必须被召回：{hit}");
        assert!(!hit.contains("other.md"));
        assert!(recall_context(&root, "完全不相干的查询", 4_000).is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn expired_entry_is_not_injected_into_context() {
        let root = sandbox("expired");
        write_memory(
            &root,
            WriteRequest {
                identity: "mode-bot:teammate:t1",
                file: Some("old.md"),
                title: "旧约定",
                description: "已过期",
                kind: "project",
                body: "旧内容",
                end: Some("2000-01-01"),
                pin: false,
            },
        )
        .unwrap();
        write_memory(
            &root,
            WriteRequest {
                identity: "mode-bot:teammate:t1",
                file: Some("pinned.md"),
                title: "置顶约定",
                description: "长期",
                kind: "project",
                body: "置顶内容",
                end: None,
                pin: true,
            },
        )
        .unwrap();
        let context = memory_context(&root, 8_000);
        assert!(!context.contains("旧内容"), "过期条目不得注入");
        assert!(context.contains("置顶内容"));
        let list = list_memory(&root);
        assert!(list.entries.first().is_some_and(|entry| entry.pin), "置顶排最前");
        assert!(list.entries.iter().any(|entry| entry.file == "old.md" && entry.expired));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn move_memory_carries_entry_across_scopes() {
        let from = sandbox("move-from");
        let to = sandbox("move-to");
        write(&from, Some("memo.md"), "可移动", "内容");
        let moved = move_memory(&from, "memo.md", &to).unwrap();
        assert_eq!(moved, "memo.md");
        assert!(!from.join("memo.md").exists());
        assert!(to.join("memo.md").exists());
        assert!(list_memory(&to).entries.iter().any(|entry| entry.title == "可移动"));
        // 源端留下 delete journal，目标端留下 create journal。
        assert!(history(&from, 10).unwrap().iter().any(|record| record.op == "delete"));
        assert!(history(&to, 10).unwrap().iter().any(|record| record.op == "create"));
        std::fs::remove_dir_all(from).ok();
        std::fs::remove_dir_all(to).ok();
    }
}

