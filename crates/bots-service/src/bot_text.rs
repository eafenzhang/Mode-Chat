//! Bot 出站文本工具（`botText.ts` 的完整移植）。
//!
//! - `has_sentence_boundary`：流式首帧是否成形（≥20 字符 / 行尾换行 / 句末标点）；
//! - `split_bot_text`：按 `\n\n` → `\n` → 硬切分片，归并边界换行后不丢字符；
//! - `drain_complete_bot_reply_segments`：逐块刷出（强边界优先、<20 字符不单独成气泡、
//!   攒够上限退到弱边界、纯 CJK 无标点时继续等待）；
//! - `to_plain_text_reply`：给不渲染 markdown 的通道降级（围栏行、行内代码、粗体、
//!   标题、链接），不碰斜体与 glob/文件名，幂等。

use regex::Regex;
use std::sync::OnceLock;

pub const BOT_FIRST_SEND_MIN_CHARS: usize = 20;
pub const BOT_REPLY_SEGMENT_MIN_CHARS: usize = BOT_FIRST_SEND_MIN_CHARS;
pub const BOT_REPLY_SEGMENT_MAX_CHARS: usize = 600;

fn char_len(text: &str) -> usize {
    text.chars().count()
}

fn ends_with_strong_punctuation(text: &str) -> bool {
    matches!(
        text.chars().last(),
        Some('。' | '，' | '！' | '？' | '；' | '：' | ',' | '.' | '!' | '?' | ';' | ':')
    )
}

pub fn has_sentence_boundary(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    let trimmed = text.trim_end();
    if char_len(trimmed) >= BOT_FIRST_SEND_MIN_CHARS {
        return true;
    }
    if text.ends_with('\n') {
        return true;
    }
    ends_with_strong_punctuation(trimmed)
}

/// 按边界优先级分片：`\n\n` → `\n` → 硬切。
pub fn split_bot_text(text: &str, limit: usize) -> Vec<String> {
    if limit == 0 {
        return if text.trim().is_empty() {
            Vec::new()
        } else {
            vec![text.to_string()]
        };
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return if text.trim().is_empty() {
            Vec::new()
        } else {
            vec![text.to_string()]
        };
    }
    let mut chunks: Vec<String> = Vec::new();
    let mut remaining: Vec<char> = chars;
    while remaining.len() > limit {
        let window: String = remaining[..limit.min(remaining.len())].iter().collect();
        let mut cut = window.rfind("\n\n").map(|index| window[..index + 2].chars().count());
        if cut.is_none() || cut.is_some_and(|cut| cut < limit / 2) {
            cut = window.rfind('\n').map(|index| window[..index + 1].chars().count());
        }
        let cut = cut.filter(|cut| *cut >= limit / 2).unwrap_or(limit);
        let chunk: String = remaining[..cut].iter().collect();
        if !chunk.trim().is_empty() {
            chunks.push(chunk);
        }
        let mut rest: Vec<char> = remaining[cut..].to_vec();
        while rest.first() == Some(&'\n') {
            rest.remove(0);
        }
        remaining = rest;
    }
    let tail: String = remaining.into_iter().collect();
    if !tail.trim().is_empty() {
        chunks.push(tail);
    }
    chunks
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotReplySegmentDrain {
    pub messages: Vec<String>,
    pub rest: String,
}

fn last_boundary_index(chars: &[char], boundary: &[char], end: usize) -> usize {
    for index in (0..end.min(chars.len())).rev() {
        if boundary.contains(&chars[index]) {
            return index + 1;
        }
    }
    0
}

const STRONG_BOUNDARY: &[char] = &['\n', '。', '！', '？', '；', ';', '.', '!', '?'];
const WEAK_BOUNDARY: &[char] = &[' ', '\t', '，', '、', '：', ',', ':'];

pub fn drain_complete_bot_reply_segments(
    buffer: &str,
    limit: usize,
) -> BotReplySegmentDrain {
    let normalized = buffer.replace("\r\n", "\n");
    let chars: Vec<char> = normalized.chars().collect();
    let mut cut = last_boundary_index(&chars, STRONG_BOUNDARY, chars.len());
    if cut == 0 {
        if chars.len() < limit {
            return BotReplySegmentDrain {
                messages: Vec::new(),
                rest: normalized,
            };
        }
        cut = last_boundary_index(&chars, WEAK_BOUNDARY, limit.min(chars.len()));
    }
    if cut == 0 {
        return BotReplySegmentDrain {
            messages: Vec::new(),
            rest: normalized,
        };
    }
    let prefix: String = chars[..cut].iter().collect();
    let prefix = prefix.trim().to_string();
    if char_len(&prefix) < BOT_REPLY_SEGMENT_MIN_CHARS {
        return BotReplySegmentDrain {
            messages: Vec::new(),
            rest: normalized,
        };
    }
    let mut rest: Vec<char> = chars[cut..].to_vec();
    while rest.first() == Some(&'\n') {
        rest.remove(0);
    }
    BotReplySegmentDrain {
        messages: split_bot_text(&prefix, limit),
        rest: rest.into_iter().collect(),
    }
}

fn code_fence() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?m)^[ \t]*```[^\n]*$").expect("code fence regex"))
}

fn inline_code() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`([^`\n]+)`").expect("inline code regex"))
}

fn bold_asterisk() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?m)(^|[\s，。；：、！？,;:!?)（(\[【"'—-])\*\*([^*\n]+)\*\*"#)
            .expect("bold asterisk regex")
    })
}

fn bold_underscore() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?m)(^|[\s，。；：、！？,;:!?)（(\[【"'—-])__([^_\n]+)__"#)
            .expect("bold underscore regex")
    })
}

fn heading_marker() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?m)^[ \t]{0,3}#{1,6}[ \t]+").expect("heading regex"))
}

fn markdown_link() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[([^\]\n]*)\]\(([^)\s]+)\)").expect("link regex"))
}

/// 纯文本通道的 markdown 降级（幂等）。
pub fn to_plain_text_reply(text: &str) -> String {
    let text = code_fence().replace_all(text, "");
    let text = inline_code().replace_all(&text, "$1");
    let text = bold_asterisk().replace_all(&text, "$1$2");
    let text = bold_underscore().replace_all(&text, "$1$2");
    let text = heading_marker().replace_all(&text, "");
    markdown_link().replace_all(&text, "$1 ($2)").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_boundary_rules() {
        assert!(!has_sentence_boundary("  "));
        assert!(!has_sentence_boundary("短句"));
        assert!(has_sentence_boundary("这是一个超过二十个字符的完整句子，足够长了"));
        assert!(has_sentence_boundary("短句。"));
        assert!(has_sentence_boundary("line\n"));
    }

    #[test]
    fn split_prefers_paragraph_then_line_then_hard() {
        let text = "第一段内容比较长需要切分\n\n第二段\n第三行";
        let chunks = split_bot_text(text, 10);
        assert!(chunks.len() >= 2, "{chunks:?}");
        assert!(chunks[0].starts_with("第一段"), "{chunks:?}");
        // 不丢字符（忽略边界换行）。
        let joined: String = chunks.concat().replace('\n', "");
        let original: String = text.replace('\n', "");
        assert_eq!(joined, original);
    }

    #[test]
    fn split_short_text_is_single_chunk() {
        assert_eq!(split_bot_text("short", 100), vec!["short".to_string()]);
        assert!(split_bot_text("   ", 100).is_empty());
    }

    #[test]
    fn drain_waits_for_boundary_and_emits_complete_prefix() {
        let waiting = drain_complete_bot_reply_segments("短", 600);
        assert!(waiting.messages.is_empty());
        let text = "这是第一句完整的回复，内容已经超过二十个字符了。后面还有尾巴";
        let drained = drain_complete_bot_reply_segments(text, 600);
        assert_eq!(
            drained.messages,
            vec!["这是第一句完整的回复，内容已经超过二十个字符了。".to_string()]
        );
        assert_eq!(drained.rest, "后面还有尾巴");
        // 前缀不足最小长度时不单独成气泡（保持等待）。
        let short = drain_complete_bot_reply_segments("短句。尾巴", 600);
        assert!(short.messages.is_empty());
    }

    #[test]
    fn plain_text_strips_owned_markup_only() {
        let input = "## 标题\n使用 `src/*.ts` 与 **重要** 命令\n```bash\nls\n```\n[文档](https://x.y)";
        let plain = to_plain_text_reply(input);
        assert!(plain.contains("标题"), "{plain}");
        assert!(plain.contains("src/*.ts"), "{plain}");
        assert!(plain.contains("重要"), "{plain}");
        assert!(!plain.contains('`'), "{plain}");
        assert!(!plain.contains("##"), "{plain}");
        assert!(plain.contains("文档 (https://x.y)"), "{plain}");
        // glob 与文件名里的单星号/下划线不被吃掉。
        let text = "路径 src/*.ts 和文件 my_file_name.md";
        assert_eq!(to_plain_text_reply(text), text);
        // 幂等。
        assert_eq!(to_plain_text_reply(&plain), plain);
    }
}
