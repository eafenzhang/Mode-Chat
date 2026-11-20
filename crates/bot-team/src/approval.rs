//! 审批策略的纯决策核心。
//!
//! 规则（用户 2026-11-03 定稿）：`ask` 全问；`auto` 只放行只读工具；`full` 放行
//! 工作区内的读写工具，但 Bash/MCP 永远仍询问。白名单只对非 Bash/MCP 的工具名精确生效。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalLevel {
    Ask,
    Auto,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Allow,
    Deny,
    Ask,
}

const READ_TOOLS: &[&str] = &["read", "grep", "glob", "ls", "stat", "notebook_read"];
const WRITE_TOOLS: &[&str] = &["write", "edit", "apply_patch"];
const BASH_TOOLS: &[&str] = &["bash", "shell", "run_command"];

pub fn parse_level(value: &str) -> Result<ApprovalLevel, String> {
    match value.trim() {
        "ask" | "" => Ok(ApprovalLevel::Ask),
        "auto" => Ok(ApprovalLevel::Auto),
        "full" => Ok(ApprovalLevel::Full),
        other => Err(format!(
            "approval level 非法：{other}（只允许 ask / auto / full）"
        )),
    }
}

/// 群回合的有效审批等级：运行级覆盖 > 频道覆盖 > 成员策略。
///
/// `inherit`/空白表示「沿用下一层」；非法值同样落回下一层（renderer 侧旧的
/// `resolveGroupApprovalPolicy` 语义：覆盖只改「多严」，allowlist 仍用成员的）。
pub fn resolve_group_level(
    run_override: Option<&str>,
    group_override: Option<&str>,
    member_level: Option<&str>,
) -> ApprovalLevel {
    fn explicit(value: Option<&str>) -> Option<&str> {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "inherit")
    }
    fn parsed(value: Option<&str>) -> Option<ApprovalLevel> {
        explicit(value).and_then(|value| parse_level(value).ok())
    }
    parsed(run_override)
        .or_else(|| parsed(group_override))
        .or_else(|| member_level.and_then(|value| parse_level(value).ok()))
        .unwrap_or(ApprovalLevel::Ask)
}

fn is_read(tool: &str) -> bool {
    READ_TOOLS.contains(&tool)
}

fn is_write(tool: &str) -> bool {
    WRITE_TOOLS.contains(&tool)
}

fn is_bash(tool: &str) -> bool {
    BASH_TOOLS.contains(&tool)
}

fn is_mcp(tool: &str) -> bool {
    tool.starts_with("mcp__")
}

/// 单次权限请求的决策。`allowlist` 是工具名精确匹配（大小写不敏感）；
/// Bash/MCP 不受白名单影响。Agent 快照里的工具名是 `Write`/`Bash` 这类
/// 首字母大写形式，矩阵统一按 trim + ASCII 小写归一后再比较。
pub fn decide(level: ApprovalLevel, allowlist: &[String], tool_name: &str) -> ApprovalDecision {
    let tool = tool_name.trim().to_ascii_lowercase();
    if tool.is_empty() {
        return ApprovalDecision::Ask;
    }
    if !is_bash(&tool)
        && !is_mcp(&tool)
        && allowlist
            .iter()
            .any(|item| item.trim().eq_ignore_ascii_case(&tool))
    {
        return ApprovalDecision::Allow;
    }
    match level {
        ApprovalLevel::Ask => ApprovalDecision::Ask,
        ApprovalLevel::Auto => {
            if is_read(&tool) {
                ApprovalDecision::Allow
            } else {
                ApprovalDecision::Ask
            }
        }
        ApprovalLevel::Full => {
            if is_bash(&tool) || is_mcp(&tool) {
                ApprovalDecision::Ask
            } else if is_read(&tool) || is_write(&tool) {
                ApprovalDecision::Allow
            } else {
                ApprovalDecision::Ask
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn ask_always_asks() {
        assert_eq!(decide(ApprovalLevel::Ask, &[], "read"), ApprovalDecision::Ask);
        assert_eq!(decide(ApprovalLevel::Ask, &[], "bash"), ApprovalDecision::Ask);
    }

    #[test]
    fn auto_allows_reads_only() {
        assert_eq!(decide(ApprovalLevel::Auto, &[], "read"), ApprovalDecision::Allow);
        assert_eq!(decide(ApprovalLevel::Auto, &[], "grep"), ApprovalDecision::Allow);
        assert_eq!(decide(ApprovalLevel::Auto, &[], "write"), ApprovalDecision::Ask);
        assert_eq!(decide(ApprovalLevel::Auto, &[], "bash"), ApprovalDecision::Ask);
    }

    #[test]
    fn full_allows_read_write_but_never_bash_or_mcp() {
        assert_eq!(decide(ApprovalLevel::Full, &[], "write"), ApprovalDecision::Allow);
        assert_eq!(decide(ApprovalLevel::Full, &[], "apply_patch"), ApprovalDecision::Allow);
        assert_eq!(decide(ApprovalLevel::Full, &[], "bash"), ApprovalDecision::Ask);
        assert_eq!(decide(ApprovalLevel::Full, &[], "mcp__github__pr"), ApprovalDecision::Ask);
        assert_eq!(decide(ApprovalLevel::Full, &[], "unknown_tool"), ApprovalDecision::Ask);
    }

    #[test]
    fn agent_capitalized_tool_names_match_the_matrix() {
        // v4 snapshot 真实投递 "Write"/"Bash"/"Read"；大小写不归一会让 full 静默变 ask。
        assert_eq!(decide(ApprovalLevel::Full, &[], "Write"), ApprovalDecision::Allow);
        assert_eq!(decide(ApprovalLevel::Full, &[], "Read"), ApprovalDecision::Allow);
        assert_eq!(decide(ApprovalLevel::Full, &[], "Bash"), ApprovalDecision::Ask);
        assert_eq!(decide(ApprovalLevel::Auto, &[], "Read"), ApprovalDecision::Allow);
        assert_eq!(decide(ApprovalLevel::Auto, &[], "Write"), ApprovalDecision::Ask);
        assert_eq!(decide(ApprovalLevel::Full, &[], "MCP__github__pr"), ApprovalDecision::Ask);
    }

    #[test]
    fn allowlist_ignores_case_and_whitespace() {
        let allowlist = ids(&[" Write "]);
        assert_eq!(decide(ApprovalLevel::Ask, &allowlist, "write"), ApprovalDecision::Allow);
        assert_eq!(decide(ApprovalLevel::Ask, &allowlist, "Write"), ApprovalDecision::Allow);
        assert_eq!(decide(ApprovalLevel::Ask, &allowlist, "Bash"), ApprovalDecision::Ask);
    }

    #[test]
    fn allowlist_matches_exact_names_and_never_bash_mcp() {
        let allowlist = ids(&["write", "bash", "mcp__x__y"]);
        assert_eq!(decide(ApprovalLevel::Ask, &allowlist, "write"), ApprovalDecision::Allow);
        assert_eq!(decide(ApprovalLevel::Ask, &allowlist, "bash"), ApprovalDecision::Ask);
        assert_eq!(decide(ApprovalLevel::Ask, &allowlist, "mcp__x__y"), ApprovalDecision::Ask);
        assert_eq!(decide(ApprovalLevel::Ask, &allowlist, "writee"), ApprovalDecision::Ask);
    }

    #[test]
    fn parses_levels() {
        assert_eq!(parse_level("ask").unwrap(), ApprovalLevel::Ask);
        assert_eq!(parse_level(" auto ").unwrap(), ApprovalLevel::Auto);
        assert_eq!(parse_level("full").unwrap(), ApprovalLevel::Full);
        assert!(parse_level("everyone").is_err());
    }

    #[test]
    fn group_level_precedence_is_run_then_channel_then_member() {
        // 运行级覆盖 > 频道覆盖 > 成员策略。
        assert_eq!(
            resolve_group_level(Some("full"), Some("auto"), Some("ask")),
            ApprovalLevel::Full
        );
        assert_eq!(
            resolve_group_level(Some("inherit"), Some("auto"), Some("ask")),
            ApprovalLevel::Auto
        );
        assert_eq!(
            resolve_group_level(None, Some("inherit"), Some("full")),
            ApprovalLevel::Full
        );
        // 非法值不阻断：落回下一层；全部缺失 = ask（安全默认）。
        assert_eq!(
            resolve_group_level(Some("bogus"), Some("auto"), Some("ask")),
            ApprovalLevel::Auto
        );
        assert_eq!(resolve_group_level(None, None, None), ApprovalLevel::Ask);
    }
}
