//! Bot 记忆工具（v0.5.0 W1）：`remember` / `forget` / `recall`。
//!
//! 与 DS Bot 的 `memory.js:1075/1114/1151` 对齐：Agent 自己能写、能删、能查，
//! 而不是只靠人工在设置页维护。存储永远走 `bots_service::memory_store`
//! （与面板、群记忆回顾同一套 frontmatter / index / history）。
//!
//! 身份是 `mode-bot:teammate:<botId>`；`resolve_memory_root` 自带 slug 回退，
//! 老 Bot 目录里的历史记忆不需要迁移。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rpi_agent::{AgentTool, AgentToolResult, TextContentOrImage};
use rpi_ai::Schema;
use rpi_ai::types::Tool;
use rpi_harness::types::HarnessTool;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const REMEMBER_MAX_CHARS: usize = 800;
const TOPIC_MAX_CHARS: usize = 40;
const RECALL_DEFAULT_LIMIT: usize = 5;
const RECALL_MAX_LIMIT: usize = 20;
const RECALL_MAX_CHARS: usize = 6_000;

#[derive(Clone, Copy)]
enum Action {
    Remember,
    Forget,
    Recall,
}

struct MemoryTool {
    tool: Tool,
    action: Action,
    bot_id: String,
}

fn tool_def(name: &str, description: &str, parameters: Value) -> Tool {
    Tool {
        name: name.to_string(),
        description: description.to_string(),
        parameters: Schema(parameters),
        constrained_sampling: None,
    }
}

fn define(bot_id: &str, action: Action, tool: Tool) -> HarnessTool {
    HarnessTool::new(Arc::new(MemoryTool {
        tool,
        action,
        bot_id: bot_id.to_string(),
    }))
}

/// 为某个 Bot 构造记忆工具集（没有 bot id 时不构造，避免写到团队记忆里）。
pub fn tools(bot_id: Option<&str>) -> Vec<HarnessTool> {
    let Some(bot_id) = bot_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return Vec::new();
    };
    vec![
        define(
            bot_id,
            Action::Remember,
            tool_def(
                "remember",
                "把一件长期有效的事实写进你自己的记忆（跨会话保留）。\
                 只记用户偏好、约定、项目事实；不要记密码/令牌/证件号，也不要记一次性的中间结果。",
                json!({
                    "type": "object",
                    "properties": {
                        "text": {
                            "type": "string",
                            "description": "要记住的事实用一句话写清（≤800 字符）"
                        },
                        "topic": {
                            "type": "string",
                            "description": "可选主题名，同类事实放一起（≤40 字符）"
                        },
                        "until": {
                            "type": "string",
                            "description": "可选失效日期 YYYY-MM-DD；到期后不再注入 prompt"
                        }
                    },
                    "required": ["text"]
                }),
            ),
        ),
        define(
            bot_id,
            Action::Forget,
            tool_def(
                "forget",
                "删除你自己的一条记忆。用 query 按标题/描述匹配，或用 file 精确指定文件。",
                json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "关键词（匹配标题或描述）"},
                        "file": {"type": "string", "description": "精确文件名（可选，优先于 query）"}
                    }
                }),
            ),
        ),
        define(
            bot_id,
            Action::Recall,
            tool_def(
                "recall",
                "在自己的记忆里检索。返回命中的条目原文，用于回答“你还记得…吗”。",
                json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "关键词"},
                        "limit": {
                            "type": "integer",
                            "description": "最多返回几条（默认 5，上限 20）"
                        }
                    },
                    "required": ["query"]
                }),
            ),
        ),
    ]
}

#[async_trait]
impl AgentTool for MemoryTool {
    fn schema(&self) -> &Tool {
        &self.tool
    }

    fn label(&self) -> &str {
        &self.tool.name
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: CancellationToken,
        _on_update: Arc<dyn Fn(rpi_agent::ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, rpi_agent::AgentError> {
        let text = self.run(params).map_err(rpi_agent::AgentError::Tool)?;
        Ok(AgentToolResult {
            content: vec![TextContentOrImage::Text(rpi_ai::TextContent {
                kind: rpi_ai::types::TextContentType,
                text,
                text_signature: None,
            })],
            details: Value::Null,
            usage: None,
            added_tool_names: Vec::new(),
            terminate: false,
        })
    }
}

impl MemoryTool {
    fn run(&self, params: Value) -> Result<String, String> {
        match self.action {
            Action::Remember => self.remember(&params),
            Action::Forget => self.forget(&params),
            Action::Recall => self.recall(&params),
        }
    }

    fn root(&self) -> Result<PathBuf, String> {
        crate::data::bot_memory_root(&self.bot_id).ok_or_else(|| {
            "记忆存储不可用（没有 MODE_STORAGE_DIR，默认目录 ~/.mode-bot/.mode/cli 也不存在）"
                .to_string()
        })
    }

    fn identity(&self) -> String {
        format!("mode-bot:teammate:{}", self.bot_id)
    }
    fn remember(&self, params: &Value) -> Result<String, String> {
        let text = params
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "缺少参数：text".to_string())?;
        if text.chars().count() > REMEMBER_MAX_CHARS {
            return Err(format!(
                "text 过长（{}/{} 字符）：请压成一句事实",
                text.chars().count(),
                REMEMBER_MAX_CHARS
            ));
        }
        if looks_sensitive(text) {
            return Err(
                "这段内容像凭据（密码/令牌/密钥/长数字），不写进记忆".to_string(),
            );
        }
        let topic = params
            .get("topic")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(topic) = topic
            && topic.chars().count() > TOPIC_MAX_CHARS
        {
            return Err(format!("topic 过长（最多 {TOPIC_MAX_CHARS} 字符）"));
        }
        let until = params
            .get("until")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(until) = until
            && !valid_date(until)
        {
            return Err("until 必须是 YYYY-MM-DD".to_string());
        }

        let root = self.root()?;
        // 标题取首句，描述取全文摘要：面板列表里一眼能认出来。
        let title = first_sentence(text, 40);
        let description = text.chars().take(120).collect::<String>();
        let kind = infer_kind(text);
        bots_service::memory_store::write_memory(
            &root,
            bots_service::memory_store::WriteRequest {
                identity: &self.identity(),
                file: None,
                title: &title,
                description: &description,
                kind,
                body: text,
                end: until,
                pin: false,
            },
        )?;
        let mut out = format!("已记住：{title}");
        if let Some(topic) = topic {
            out.push_str(&format!("（主题 {topic}）"));
        }
        if let Some(until) = until {
            out.push_str(&format!("，{until} 前有效"));
        }
        Ok(out)
    }

    fn forget(&self, params: &Value) -> Result<String, String> {
        let file = params
            .get("file")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let query = params
            .get("query")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if file.is_none() && query.is_none() {
            return Err("需要 query 或 file 之一".to_string());
        }
        let root = self.root()?;
        let entries = bots_service::memory_store::list_memory(&root).entries;
        let target = match (file, query) {
            (Some(file), _) => entries
                .into_iter()
                .find(|entry| entry.file.eq_ignore_ascii_case(file)),
            (None, Some(query)) => {
                let needle = query.to_ascii_lowercase();
                let mut matches: Vec<_> = entries
                    .into_iter()
                    .filter(|entry| {
                        entry.title.to_ascii_lowercase().contains(&needle)
                            || entry.description.to_ascii_lowercase().contains(&needle)
                    })
                    .collect();
                match matches.len() {
                    0 => None,
                    1 => matches.pop(),
                    _ => {
                        let names: Vec<String> = matches
                            .iter()
                            .take(5)
                            .map(|entry| format!("- {}（{}）", entry.title, entry.file))
                            .collect();
                        return Err(format!(
                            "匹配到 {} 条，请用 file 指定：\n{}",
                            matches.len(),
                            names.join("\n")
                        ));
                    }
                }
            }
            (None, None) => None,
        };
        let Some(target) = target else {
            return Ok("没有匹配的记忆".to_string());
        };
        bots_service::memory_store::delete_memory(&root, &target.file)?;
        Ok(format!("已删除：{}", target.title))
    }

    fn recall(&self, params: &Value) -> Result<String, String> {
        let query = params
            .get("query")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "缺少参数：query".to_string())?;
        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(RECALL_DEFAULT_LIMIT)
            .clamp(1, RECALL_MAX_LIMIT);
        let root = self.root()?;
        let entries = bots_service::memory_store::list_memory(&root).entries;
        let needle = query.to_ascii_lowercase();
        let mut matched: Vec<_> = entries
            .into_iter()
            .filter(|entry| {
                entry.title.to_ascii_lowercase().contains(&needle)
                    || entry.description.to_ascii_lowercase().contains(&needle)
            })
            .collect();
        if matched.is_empty() {
            // 退一步用全文检索（`recall_context` 会读文件正文）。
            let text =
                bots_service::memory_store::recall_context(&root, query, RECALL_MAX_CHARS);
            return Ok(if text.trim().is_empty() {
                "记忆里没有相关条目".to_string()
            } else {
                text
            });
        }
        matched.truncate(limit);
        let mut lines = Vec::new();
        for entry in matched {
            let body = bots_service::memory_store::read_memory(&root, &entry.file)
                .map(|read| read.body)
                .unwrap_or_default();
            lines.push(format!(
                "- [{}] {}\n  {}",
                entry.file,
                entry.title,
                body.trim()
            ));
        }
        Ok(lines.join("\n"))
    }
}

/// 只取首句做标题（面板一行能放下）。
fn first_sentence(text: &str, max_chars: usize) -> String {
    let head: String = text
        .chars()
        .take_while(|ch| !matches!(ch, '。' | '！' | '？' | '\n' | '.' | '!' | '?'))
        .collect();
    let head = head.trim();
    let candidate = if head.is_empty() { text.trim() } else { head };
    if candidate.chars().count() <= max_chars {
        candidate.to_string()
    } else {
        format!("{}…", candidate.chars().take(max_chars).collect::<String>())
    }
}

/// 粗分类：偏好类事实归 user，约定/规则归 feedback，其余 note。
fn infer_kind(text: &str) -> &'static str {
    let lower = text.to_ascii_lowercase();
    const PREFERENCE: &[&str] = &["我喜欢", "我习惯", "我偏好", "prefer", "习惯用"];
    const RULES: &[&str] = &["不要", "必须", "一律", "禁止", "约定", "always", "never"];
    if PREFERENCE.iter().any(|marker| lower.contains(marker)) {
        "user"
    } else if RULES.iter().any(|marker| lower.contains(marker)) {
        "feedback"
    } else {
        "note"
    }
}

fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let year = value[0..4].parse::<u32>().ok();
    let month = value[5..7].parse::<u32>().ok();
    let day = value[8..10].parse::<u32>().ok();
    match (year, month, day) {
        (Some(year), Some(month), Some(day)) => {
            (1970..=2999).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day)
        }
        _ => false,
    }
}

/// 拒绝把凭据写进记忆（与 `src/memory_review.rs:286` 同一套标记 + 长数字检测）。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_text_is_rejected() {
        assert!(looks_sensitive("my api_key is abc"));
        assert!(looks_sensitive("密码是 123"));
        assert!(looks_sensitive("6222021234567890123"));
        assert!(!looks_sensitive("用户习惯用中文回答"));
    }

    #[test]
    fn first_sentence_truncates() {
        assert_eq!(first_sentence("用户习惯用中文。其他", 40), "用户习惯用中文");
        assert_eq!(first_sentence("abcdef", 3), "abc…");
    }

    #[test]
    fn kind_inference() {
        assert_eq!(infer_kind("我习惯用中文回答"), "user");
        assert_eq!(infer_kind("提交前必须跑测试"), "feedback");
        assert_eq!(infer_kind("项目用 Rust + GPUI"), "note");
    }

    #[test]
    fn date_validation() {
        assert!(valid_date("2026-12-31"));
        assert!(!valid_date("2026-13-01"));
        assert!(!valid_date("2026/12/31"));
        assert!(!valid_date("2026-1-1"));
    }

    #[test]
    fn no_tools_without_bot_id() {
        assert!(tools(None).is_empty());
        assert!(tools(Some("  ")).is_empty());
        assert_eq!(tools(Some("b1")).len(), 3);
    }
}
