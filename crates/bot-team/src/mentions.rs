//! 群聊 @提及解析与 `[PASS]` 判定。
//!
//! 规则与 `groups/mentions.ts` 对齐：`@名字` 精确匹配成员（大小写不敏感、整名匹配），
//! 同一位置取最长名字，按出现顺序去重；没有命中返回空数组（调用方回落管理员）。

/// 解析 `@名字`，返回按出现顺序去重的 teammate id。
///
/// `members` 的每一项是 `(id, name)`；name 会 trim，空名忽略。
pub fn parse_mentioned_ids(text: &str, members: &[(String, String)]) -> Vec<String> {
    let mut candidates: Vec<(String, String)> = members
        .iter()
        .filter_map(|(id, name)| {
            let name = name.trim();
            (!name.is_empty()).then(|| (name.to_lowercase(), id.clone()))
        })
        .collect();
    // 最长名字优先；`sort_by_key` 是稳定排序，同长度保持成员原顺序（与 TS 一致）。
    candidates.sort_by_key(|left| std::cmp::Reverse(utf16_len(&left.0)));

    let lower_text = text.to_lowercase();
    let mut hits: Vec<(usize, String)> = Vec::new();
    let mut cursor = 0usize;
    while cursor < lower_text.len() {
        let Some(relative) = lower_text[cursor..].find('@') else {
            break;
        };
        let at = cursor + relative;
        let after_at = at + 1;
        let matched = candidates
            .iter()
            .find(|(name, _)| lower_text[after_at..].starts_with(name));
        match matched {
            Some((name, id)) => {
                hits.push((at, id.clone()));
                cursor = after_at + name.len();
            }
            None => cursor = after_at,
        }
    }

    hits.sort_by_key(|(at, _)| *at);
    let mut seen = std::collections::HashSet::new();
    let mut ordered = Vec::new();
    for (_, id) in hits {
        if seen.insert(id.clone()) {
            ordered.push(id);
        }
    }
    ordered
}

/// 全员的保留词：`@所有人`（成员真的叫这个名字时按普通成员处理）。
pub const EVERYONE_TOKEN: &str = "所有人";

/// `@所有人` 的英文同义词（UI 英文界面插入 `@everyone`，两种都要认得）。
pub const EVERYONE_TOKEN_EN: &str = "everyone";

const EVERYONE_TOKENS: [&str; 2] = [EVERYONE_TOKEN, EVERYONE_TOKEN_EN];

#[derive(Debug, Clone, PartialEq)]
pub struct MentionParseResult {
    /// 按出现顺序去重的 teammate id（不含全员标记）。
    pub ids: Vec<String>,
    /// 文本里出现 `@所有人`（且不存在同名成员）时置真；调用方按「全员」派发。
    pub everyone: bool,
}

/// 解析 @提及：普通成员 id 列表 + 全员标记（单一事实源，供壳命令与测试共用）。
pub fn parse_mentions(text: &str, members: &[(String, String)]) -> MentionParseResult {
    let member_named_everyone = members.iter().any(|(_, name)| {
        let name = name.trim();
        EVERYONE_TOKENS
            .iter()
            .any(|token| name.eq_ignore_ascii_case(token))
    });
    MentionParseResult {
        ids: parse_mentioned_ids(text, members),
        everyone: !member_named_everyone && has_everyone_token(text),
    }
}

/// 全员触发词识别：`@` 必须在行首或空白之后（与 TS 的触发边界一致）。
fn has_everyone_token(text: &str) -> bool {
    let lower_text = text.to_lowercase();
    EVERYONE_TOKENS
        .iter()
        .any(|token| has_at_token(&lower_text, token))
}

fn has_at_token(lower_text: &str, token: &str) -> bool {
    let mut cursor = 0usize;
    while let Some(relative) = lower_text[cursor..].find('@') {
        let at = cursor + relative;
        let preceded_by_boundary = at == 0
            || lower_text[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        if preceded_by_boundary && lower_text[at + 1..].starts_with(token) {
            return true;
        }
        cursor = at + 1;
    }
    false
}

/// 成员「没有新东西可说」的约定回复：整段只有 `[PASS]` / `PASS`（大小写不敏感）。
///
/// 与 TS 正则 `^\s*\[?pass\]?\s*$` 对齐：允许只出现一侧方括号。
pub fn is_pass_reply(text: &str) -> bool {
    let mut value = text.trim();
    if let Some(rest) = value.strip_prefix('[') {
        value = rest;
    }
    if let Some(rest) = value.strip_suffix(']') {
        value = rest;
    }
    value.eq_ignore_ascii_case("pass")
}

fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members() -> Vec<(String, String)> {
        vec![
            ("t1".into(), "娜娜".into()),
            ("t2".into(), "小白".into()),
            ("t3".into(), "小".into()),
        ]
    }

    #[test]
    fn longest_name_wins_at_same_position() {
        let ids = parse_mentioned_ids("@小白 看一下", &members());
        assert_eq!(ids, vec!["t2"]);
    }

    #[test]
    fn preserves_order_and_dedupes() {
        let ids = parse_mentioned_ids("@小白 和 @娜娜，再 @小白", &members());
        assert_eq!(ids, vec!["t2", "t1"]);
    }

    #[test]
    fn matches_case_insensitively() {
        let members = vec![("t1".into(), "Alice".into())];
        assert_eq!(parse_mentioned_ids("@alice hi", &members), vec!["t1"]);
    }

    #[test]
    fn unmatched_or_empty_members_fall_back_to_empty() {
        assert!(parse_mentioned_ids("@不存在", &members()).is_empty());
        assert!(parse_mentioned_ids("@娜娜", &[]).is_empty());
    }

    #[test]
    fn everyone_token_respects_left_boundary() {
        let members = vec![("t1".to_string(), "小王".to_string())];
        assert!(parse_mentions("@所有人 开会", &members).everyone);
        assert!(parse_mentions("hi @所有人", &members).everyone);
        assert!(!parse_mentions("a@所有人", &members).everyone);
        assert!(!parse_mentions("没有提及", &members).everyone);
    }

    #[test]
    fn everyone_token_accepts_english_synonym() {
        let members = vec![("t1".to_string(), "小王".to_string())];
        assert!(parse_mentions("@everyone standup", &members).everyone);
        assert!(parse_mentions("@Everyone standup", &members).everyone);
        assert!(parse_mentions("ping @everyone", &members).everyone);
        assert!(!parse_mentions("a@everyone", &members).everyone);
        assert!(!parse_mentions("@everyday", &members).everyone);
    }

    #[test]
    fn member_named_everyone_wins_over_reserved_token() {
        let members = vec![("t1".to_string(), "所有人".to_string())];
        let parsed = parse_mentions("@所有人 开会", &members);
        assert!(!parsed.everyone);
        assert_eq!(parsed.ids, vec!["t1".to_string()]);
    }

    #[test]
    fn pass_reply_variants() {
        for text in ["[PASS]", "PASS", "pass", "  [pass]  ", "[pass", "pass]"] {
            assert!(is_pass_reply(text), "{text} should be PASS");
        }
        for text in ["", "pass please", "[PASS] 好的", "PASSED"] {
            assert!(!is_pass_reply(text), "{text} should not be PASS");
        }
    }
}
