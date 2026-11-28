//! 群记忆自动回顾（回合结束后由管理员模型整理群记忆）。
//!
//! 与 WebView `memory_review.rs` 同口径：延迟 5s、全局串行、严格 JSON 操作、
//! 只写 `bots_service::memory_store` 的 `mode-bot:group:<id>` 文件记忆。
//! 本模块只放纯逻辑（开关 / prompt / 解析 / 落盘），模型调用在 backend。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use bots_service::memory_store::{self as store, MemoryList, WriteRequest};
use serde::Deserialize;

const MAX_OPERATIONS: usize = 3;
const MAX_TITLE_CHARS: usize = 200;
const MAX_DESCRIPTION_CHARS: usize = 400;
const MAX_BODY_CHARS: usize = 8_000;
const MAX_PROMPT_CHARS: usize = 16_000;
const MAX_HISTORY_LINE_CHARS: usize = 800;
const MAX_MEMORY_BODY_CHARS: usize = 800;
const MAX_MEMORY_SECTION_CHARS: usize = 6_000;
const MAX_TRANSCRIPT_LINES: usize = 24;

fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max).collect();
    out.push('…');
    out
}

// ── 开关（`{data_root}/v2/memory-review.json`，默认开启）──────────────────

pub fn settings_path() -> PathBuf {
    crate::data::data_root()
        .join("v2")
        .join("memory-review.json")
}

pub fn enabled() -> bool {
    std::fs::read(settings_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value.get("enabled").and_then(serde_json::Value::as_bool))
        .unwrap_or(true)
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建记忆回顾设置目录失败：{error}"))?;
    }
    atomic_file::write_json_atomic(
        &path,
        &serde_json::json!({ "enabled": enabled }),
        &atomic_file::WriteOptions::default(),
    )
    .map_err(|error| format!("写入记忆回顾设置失败：{error}"))
}

// ── 回顾输入与 prompt ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TranscriptLine {
    /// 已带好角色前缀，例如「用户」「成员（娜娜）」「系统」。
    pub who: String,
    pub text: String,
}

/// 一轮群聊的可回顾快照。
#[derive(Debug, Clone)]
pub struct ReviewSnapshot {
    /// `group`（群聊）或 `bot`（私聊）；决定记忆根与身份前缀。
    pub scope: &'static str,
    /// 作用域 id：群 id 或 Bot id（两者都是 uuid，状态文件不冲突）。
    pub group_id: String,
    pub group_name: String,
    pub workspace: String,
    pub identity: String,
    /// `modelSelection` JSON（providerId/modelId/options）。
    pub model: serde_json::Value,
    pub transcript: Vec<TranscriptLine>,
}

pub fn build_prompt(snapshot: &ReviewSnapshot, memory_root: &Path) -> String {
    let memory_list = store::list_memory(memory_root);
    let mut out = String::new();
    out.push_str(
        "你是 Mode Bot 的记忆整理器。阅读本轮对话，判断是否值得写入长期记忆。\n\
只输出一个 JSON 对象，不要 Markdown 代码块，不要解释：\n\
{\"operations\":[{\"action\":\"add\",\"title\":\"...\",\"description\":\"...\",\"kind\":\"note\",\"body\":\"...\"},\
{\"action\":\"edit\",\"file\":\"xxx.md\",\"title\":\"...\",\"description\":\"...\",\"kind\":\"note\",\"body\":\"...\"},\
{\"action\":\"remove\",\"file\":\"xxx.md\"}]}\n\
最多 3 条操作；没有值得长期保存的信息就输出 {\"operations\":[]}。\n\
edit/remove 的 file 必须来自下方「现有记忆」列表。只记录长期有效的事实、偏好、决策与约定；\
不要记录寒暄、一次性状态、密码、Token、密钥或身份证号。\n\n",
    );
    let session_label = if snapshot.scope == "bot" { "会话" } else { "群名" };
    let transcript_label = if snapshot.scope == "bot" { "最近对话" } else { "最近群聊" };
    out.push_str(&format!(
        "[{session_label}]\n{}\n\n[{transcript_label}]\n",
        snapshot.group_name
    ));
    let start = snapshot.transcript.len().saturating_sub(MAX_TRANSCRIPT_LINES);
    let mut lines: Vec<String> = snapshot.transcript[start..]
        .iter()
        .map(|line| {
            format!(
                "{}：{}",
                line.who,
                truncate_chars(line.text.trim(), MAX_HISTORY_LINE_CHARS)
            )
        })
        .collect();
    while !lines.is_empty()
        && out.chars().count() + lines.iter().map(|line| line.chars().count() + 1).sum::<usize>()
            > MAX_PROMPT_CHARS
    {
        lines.remove(0);
    }
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("\n[现有记忆]\n");
    if memory_list.entries.is_empty() {
        out.push_str("（暂无）\n");
    } else {
        let mut budget = MAX_MEMORY_SECTION_CHARS;
        for entry in &memory_list.entries {
            let header = format!(
                "- {} | {} | {}\n",
                entry.file,
                truncate_chars(&entry.title, 120),
                truncate_chars(&entry.description, 160)
            );
            let header_len = header.chars().count();
            if header_len > budget {
                break;
            }
            budget -= header_len;
            out.push_str(&header);
            let body = store::read_memory(memory_root, &entry.file)
                .map(|entry| entry.body)
                .unwrap_or_default();
            let body = truncate_chars(body.trim(), MAX_MEMORY_BODY_CHARS);
            if !body.is_empty() {
                let body_line = format!("  正文：{body}\n");
                let body_len = body_line.chars().count();
                if body_len <= budget {
                    budget -= body_len;
                    out.push_str(&body_line);
                }
            }
        }
    }
    out
}

// ── JSON 操作解析与应用 ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct RawReviewOperation {
    action: String,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewOperation {
    Add {
        title: String,
        description: String,
        kind: String,
        body: String,
    },
    Edit {
        file: String,
        title: Option<String>,
        description: Option<String>,
        kind: Option<String>,
        body: Option<String>,
    },
    Remove {
        file: String,
    },
}

fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

pub fn parse_operations(text: &str) -> Result<Vec<ReviewOperation>, String> {
    let json_text =
        extract_json_object(text).ok_or_else(|| "模型输出没有 JSON".to_string())?;
    #[derive(Deserialize)]
    struct Payload {
        #[serde(default)]
        operations: Vec<RawReviewOperation>,
    }
    let payload: Payload =
        serde_json::from_str(json_text).map_err(|error| format!("回顾 JSON 解析失败：{error}"))?;
    if payload.operations.len() > MAX_OPERATIONS {
        return Err(format!(
            "回顾操作过多：{} > {MAX_OPERATIONS}",
            payload.operations.len()
        ));
    }
    let mut operations = Vec::new();
    for raw in payload.operations {
        let action = raw.action.trim().to_ascii_lowercase();
        let file = raw
            .file
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        match action.as_str() {
            "add" => {
                let title = raw.title.unwrap_or_default().trim().to_string();
                let body = raw.body.unwrap_or_default().trim().to_string();
                if title.is_empty() || body.is_empty() {
                    continue;
                }
                operations.push(ReviewOperation::Add {
                    title: truncate_chars(&title, MAX_TITLE_CHARS),
                    description: truncate_chars(
                        raw.description.unwrap_or_default().trim(),
                        MAX_DESCRIPTION_CHARS,
                    ),
                    kind: normalize_kind(raw.kind.as_deref()),
                    body: truncate_chars(&body, MAX_BODY_CHARS),
                });
            }
            "edit" => {
                let Some(file) = file else { continue };
                let body = raw
                    .body
                    .map(|value| truncate_chars(value.trim(), MAX_BODY_CHARS))
                    .filter(|value| !value.is_empty());
                let title = raw
                    .title
                    .map(|value| truncate_chars(value.trim(), MAX_TITLE_CHARS))
                    .filter(|value| !value.is_empty());
                let description = raw
                    .description
                    .map(|value| truncate_chars(value.trim(), MAX_DESCRIPTION_CHARS));
                let kind = raw.kind.as_deref().and_then(|value| {
                    let value = value.trim();
                    (!value.is_empty()).then(|| normalize_kind(Some(value)))
                });
                if title.is_none() && description.is_none() && kind.is_none() && body.is_none()
                {
                    continue;
                }
                operations.push(ReviewOperation::Edit {
                    file,
                    title,
                    description,
                    kind,
                    body,
                });
            }
            "remove" => {
                if let Some(file) = file {
                    operations.push(ReviewOperation::Remove { file });
                }
            }
            _ => return Err(format!("未知的回顾操作：{}", raw.action)),
        }
    }
    Ok(operations)
}

fn normalize_kind(kind: Option<&str>) -> String {
    let kind = kind.unwrap_or("note").trim();
    if kind.is_empty() {
        "note".to_string()
    } else {
        truncate_chars(kind, 40)
    }
}

/// 不把凭据/证件样式内容写进记忆（宁可漏记）。
fn looks_sensitive(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "password",
        "passwd",
        "api_key",
        "api-key",
        "apikey",
        "secret",
        "token",
        "-----begin",
        "密码",
        "密钥",
        "身份证",
    ];
    if MARKERS.iter().any(|marker| lower.contains(marker)) {
        return true;
    }
    let mut digits = 0;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits += 1;
            if digits >= 17 {
                return true;
            }
        } else {
            digits = 0;
        }
    }
    false
}

pub fn apply_operations(
    root: &Path,
    identity: &str,
    operations: &[ReviewOperation],
) -> Result<usize, String> {
    let memory_list: MemoryList = store::list_memory(root);
    let existing: HashSet<&str> = memory_list
        .entries
        .iter()
        .map(|entry| entry.file.as_str())
        .collect();
    let mut applied = 0usize;
    for operation in operations {
        match operation {
            ReviewOperation::Add {
                title,
                description,
                kind,
                body,
            } => {
                if looks_sensitive(&format!("{title}\n{description}\n{body}")) {
                    continue;
                }
                store::write_memory(
                    root,
                    WriteRequest {
                        identity,
                        file: None,
                        title,
                        description,
                        kind,
                        body,
                        end: None,
                        pin: false,
                    },
                )?;
                applied += 1;
            }
            ReviewOperation::Edit {
                file,
                title,
                description,
                kind,
                body,
            } => {
                if !existing.contains(file.as_str()) {
                    continue;
                }
                let changed = [
                    title.as_deref().unwrap_or_default(),
                    description.as_deref().unwrap_or_default(),
                    body.as_deref().unwrap_or_default(),
                ]
                .join("\n");
                if looks_sensitive(&changed) {
                    continue;
                }
                let current = store::read_memory(root, file)?;
                store::write_memory(
                    root,
                    WriteRequest {
                        identity,
                        file: Some(file),
                        title: title.as_deref().unwrap_or(&current.title),
                        description: description
                            .as_deref()
                            .unwrap_or(&current.description),
                        kind: kind.as_deref().unwrap_or(&current.kind),
                        body: body.as_deref().unwrap_or(&current.body),
                        end: current.end.as_deref(),
                        pin: current.pin,
                    },
                )?;
                applied += 1;
            }
            ReviewOperation::Remove { file } => {
                if !existing.contains(file.as_str()) {
                    continue;
                }
                store::delete_memory(root, file)?;
                applied += 1;
            }
        }
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mode-bot-gpui-memory-review-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_fenced_json_and_skips_incomplete_operations() {
        let text = "```json\n{\"operations\":[\
{\"action\":\"add\",\"title\":\"发布约定\",\"body\":\"周五下午发版\"},\
{\"action\":\"add\",\"title\":\"\",\"body\":\"缺标题\"},\
{\"action\":\"remove\",\"file\":\"notes.md\"}]}\n```";
        let operations = parse_operations(text).unwrap();
        assert_eq!(operations.len(), 2);
        assert!(matches!(
            &operations[0],
            ReviewOperation::Add { title, .. } if title == "发布约定"
        ));
        assert!(matches!(
            &operations[1],
            ReviewOperation::Remove { file } if file == "notes.md"
        ));
    }

    #[test]
    fn rejects_too_many_or_unknown_operations() {
        let too_many = format!(
            "{{\"operations\":[{}]}}",
            (0..4)
                .map(|i| format!(
                    "{{\"action\":\"add\",\"title\":\"t{i}\",\"body\":\"b\"}}"
                ))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(parse_operations(&too_many).is_err());
        assert!(parse_operations("{\"operations\":[{\"action\":\"nuke\"}]}").is_err());
    }

    #[test]
    fn applies_add_edit_remove_and_skips_sensitive() {
        let root = sandbox("apply");
        let identity = "mode-bot:group:g-test";
        let operations = parse_operations(
            "{\"operations\":[\
{\"action\":\"add\",\"title\":\"发布窗口\",\"description\":\"周报\",\"body\":\"固定周四\"},\
{\"action\":\"add\",\"title\":\"密码\",\"body\":\"api_key=sk-123\"}]}",
        )
        .unwrap();
        assert_eq!(apply_operations(&root, identity, &operations).unwrap(), 1);
        let list = store::list_memory(&root);
        assert_eq!(list.entries.len(), 1);
        let file = list.entries[0].file.clone();
        let edit = parse_operations(&format!(
            "{{\"operations\":[{{\"action\":\"edit\",\"file\":\"{file}\",\"body\":\"改为周五\"}}]}}"
        ))
        .unwrap();
        assert_eq!(apply_operations(&root, identity, &edit).unwrap(), 1);
        assert_eq!(
            store::read_memory(&root, &file).unwrap().body,
            "改为周五"
        );
        let remove = parse_operations(&format!(
            "{{\"operations\":[{{\"action\":\"remove\",\"file\":\"{file}\"}}]}}"
        ))
        .unwrap();
        assert_eq!(apply_operations(&root, identity, &remove).unwrap(), 1);
        assert!(store::list_memory(&root).entries.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
