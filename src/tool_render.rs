//! 工具卡内容解析（对齐 WebView 的 edit / execute 渲染口径）。
//!
//! 只做 JSON/纯文本 → 展示模型的转换，不依赖 gpui 与主题，方便单测。
//! 数据来源：
//! - `input`：工具调用参数 JSON（`AgentEvent::ToolExecutionStart`）。
//! - `details`：工具结果的结构化 details（edit 带 diff，bash 带 exit_code）。
//! - `output`：工具结果正文文本。
//!
//! 历史回放里没有 details 时，按 input 的 oldText/newText 自行拼出增删行；
//! 两者都没有就退回通用卡片，不猜测。

use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolFamily {
    Execute,
    Edit,
    Search,
    Read,
    Cua,
    Other,
}

/// 工具名 → 渲染/分组类别。名字可能来自 pi-rust 内置工具（snake_case），
/// 也可能是 ACP 的展示标题（"Read file"），所以用小写包含匹配。
pub fn family(name: &str) -> ToolFamily {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() {
        return ToolFamily::Other;
    }
    let has = |needle: &str| name.contains(needle);
    if name.starts_with("computer") || name.starts_with("cua") || has("_cua") || has("cua_") {
        ToolFamily::Cua
    } else if name == "bash"
        || name == "powershell"
        || name == "execute"
        || has("shell")
        || has("command")
        || has("exec")
        || has("terminal")
    {
        ToolFamily::Execute
    } else if name == "edit"
        || name == "write"
        || name == "apply_patch"
        || has("edit")
        || has("patch")
        || has("_write")
        || name.starts_with("write")
    {
        ToolFamily::Edit
    } else if name == "read"
        || name == "ls"
        || name == "list"
        || name.starts_with("read")
        || name.starts_with("ls_")
        || name.starts_with("list_")
        || has("file")
    {
        ToolFamily::Read
    } else if name == "grep"
        || name == "find"
        || name == "glob"
        || has("search")
        || has("grep")
        || has("find")
        || has("glob")
        || has("browser")
        || has("fetch")
        || has("http")
        || has("web")
    {
        ToolFamily::Search
    } else {
        ToolFamily::Other
    }
}

/// 是否值得把连续同类调用折叠成一组（WebView 的 execute/edit/explore/cua 组）。
pub fn groupable(family: ToolFamily) -> bool {
    !matches!(family, ToolFamily::Other)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    /// 行号（pi 的展示 diff 自带；input 兜底拼的行没有）。
    pub number: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct EditInfo {
    pub path: String,
    pub lines: Vec<DiffLine>,
    pub added: usize,
    pub removed: usize,
    /// 新建文件（write 工具，或 details.diff 为空但 input 带 content）。
    pub created: bool,
}

impl EditInfo {
    fn recount(&mut self) {
        self.added = self
            .lines
            .iter()
            .filter(|line| line.kind == DiffKind::Added)
            .count();
        self.removed = self
            .lines
            .iter()
            .filter(|line| line.kind == DiffKind::Removed)
            .count();
    }
}

/// 解析一次 edit/write 调用。`details` 里的 `diff` 是 pi 生成的带行号展示 diff，
/// 优先使用；没有就按 input 的 `edits[].oldText/newText` 拼增删行。
pub fn edit_info(name: &str, input: &str, details: &str) -> Option<EditInfo> {
    let mut info = EditInfo::default();
    if let Some(diff) = parse_json(details)
        .as_ref()
        .and_then(|value| value.get("diff"))
        .and_then(Value::as_str)
        && !diff.trim().is_empty()
    {
        info.lines = parse_display_diff(diff);
    }
    if let Some(input) = parse_json(input).as_ref().and_then(Value::as_object) {
        info.path = pick_str(input, &["path", "file_path", "filePath", "filename", "file"]);
        if info.lines.is_empty() {
            if let Some(edits) = input.get("edits").and_then(Value::as_array) {
                for edit in edits {
                    push_replacement(
                        &mut info.lines,
                        edit.get("oldText").and_then(Value::as_str),
                        edit.get("newText").and_then(Value::as_str),
                    );
                }
            }
            // 兼容 WebView/旧 TS 契约的顶层 oldText/newText。
            push_replacement(
                &mut info.lines,
                input.get("oldText").and_then(Value::as_str),
                input.get("newText").and_then(Value::as_str),
            );
            if info.lines.is_empty()
                && let Some(content) = input.get("content").and_then(Value::as_str)
            {
                info.lines = content
                    .lines()
                    .map(|line| DiffLine {
                        kind: DiffKind::Added,
                        number: None,
                        text: line.to_string(),
                    })
                    .collect();
                info.created = true;
            }
        } else {
            let lower = name.to_ascii_lowercase();
            info.created = lower.contains("write") || lower == "create_file";
        }
    }
    info.recount();
    if info.path.is_empty() && info.lines.is_empty() {
        return None;
    }
    Some(info)
}

/// pi（`generate_diff_string`）的展示 diff：`"{marker}{行号} {内容}"`，
/// 标记为 ` ` / `+` / `-`，超长上下文用 `" ..."` 占位。
pub fn parse_display_diff(diff: &str) -> Vec<DiffLine> {
    diff.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let marker = line.chars().next().unwrap_or(' ');
            let kind = match marker {
                '+' => DiffKind::Added,
                '-' => DiffKind::Removed,
                _ => DiffKind::Context,
            };
            let rest = &line[marker.len_utf8()..];
            let (number, text) = match rest.find(' ') {
                Some(position) => {
                    let raw = rest[..position].trim();
                    let number =
                        (!raw.is_empty() && raw.chars().all(|ch| ch.is_ascii_digit()))
                            .then(|| raw.to_string());
                    let text = &rest[position + 1..];
                    // 行号为空的是 "..." 占位（或空行），前面的空格是列对齐填充。
                    let text = if number.is_none() {
                        text.trim_start().to_string()
                    } else {
                        text.to_string()
                    };
                    (number, text)
                }
                None => (None, rest.to_string()),
            };
            DiffLine { kind, number, text }
        })
        .collect()
}

fn push_replacement(lines: &mut Vec<DiffLine>, old: Option<&str>, new: Option<&str>) {
    if let Some(old) = old {
        for line in old.lines() {
            lines.push(DiffLine {
                kind: DiffKind::Removed,
                number: None,
                text: line.to_string(),
            });
        }
    }
    if let Some(new) = new {
        for line in new.lines() {
            lines.push(DiffLine {
                kind: DiffKind::Added,
                number: None,
                text: line.to_string(),
            });
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExecInfo {
    pub command: String,
    pub description: String,
    pub exit_code: Option<i64>,
    pub failure_kind: Option<String>,
}

/// 解析一次命令执行调用（bash / powershell / 任意 command 类工具）。
pub fn exec_info(input: &str, details: &str) -> Option<ExecInfo> {
    let mut info = ExecInfo::default();
    if let Some(input) = parse_json(input).as_ref().and_then(Value::as_object) {
        info.command = pick_str(input, &["command", "cmd", "script"]);
        info.description = pick_str(input, &["description", "summary"]);
    }
    if let Some(details) = parse_json(details).as_ref().and_then(Value::as_object) {
        info.exit_code = details.get("exit_code").and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        });
        info.failure_kind = details
            .get("failure_kind")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    if info.command.is_empty() && info.description.is_empty() {
        return None;
    }
    Some(info)
}

/// 折叠成单行摘要（行内换行会撑破一行式工具卡）。
pub fn single_line(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        let ch = if ch == '\n' || ch == '\r' || ch == '\t' {
            ' '
        } else {
            ch
        };
        if ch == ' ' && out.ends_with(' ') {
            continue;
        }
        out.push(ch);
        if out.chars().count() >= max_chars {
            out.push('…');
            break;
        }
    }
    out.trim().to_string()
}

/// 工具行主文案：优先显示用户关心的对象（文件路径/命令/搜索词），
/// 没有就退回工具名。
pub fn subject(name: &str, input: &str, fallback: &str) -> String {
    let value = parse_json(input);
    let object = value.as_ref().and_then(Value::as_object);
    let mut subject = String::new();
    if let Some(object) = object {
        match family(name) {
            ToolFamily::Execute => {
                subject = pick_str(object, &["description", "summary"]);
                if subject.is_empty() {
                    subject = pick_str(object, &["command", "cmd", "script"]);
                }
            }
            ToolFamily::Edit | ToolFamily::Read => {
                subject = pick_str(object, &["path", "file_path", "filePath", "filename", "file"]);
            }
            ToolFamily::Search => {
                subject = pick_str(
                    object,
                    &["pattern", "query", "url", "text", "search_term", "searchTerm"],
                );
            }
            ToolFamily::Cua | ToolFamily::Other => {}
        }
    }
    if subject.trim().is_empty() {
        subject = fallback.trim().to_string();
    }
    subject
}

fn pick_str(object: &serde_json::Map<String, Value>, keys: &[&str]) -> String {
    for key in keys {
        if let Some(text) = object.get(*key).and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            return text.to_string();
        }
    }
    String::new()
}

fn parse_json(text: &str) -> Option<Value> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    serde_json::from_str(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_cover_pi_and_acp_names() {
        assert_eq!(family("bash"), ToolFamily::Execute);
        assert_eq!(family("powershell"), ToolFamily::Execute);
        assert_eq!(family("Edit file"), ToolFamily::Edit);
        assert_eq!(family("write"), ToolFamily::Edit);
        assert_eq!(family("browser_open"), ToolFamily::Search);
        assert_eq!(family("grep"), ToolFamily::Search);
        assert_eq!(family("read"), ToolFamily::Read);
        assert_eq!(family("computer_windows"), ToolFamily::Cua);
        assert_eq!(family("ask_user_question"), ToolFamily::Other);
        assert_eq!(family("todo"), ToolFamily::Other);
    }

    #[test]
    fn parses_pi_display_diff() {
        let diff = " 1 keep\n-2 old one\n-3 old two\n+2 new\n 4 keep";
        let lines = parse_display_diff(diff);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[1].kind, DiffKind::Removed);
        assert_eq!(lines[1].number.as_deref(), Some("2"));
        assert_eq!(lines[1].text, "old one");
        assert_eq!(lines[3].kind, DiffKind::Added);
        assert_eq!(lines[3].text, "new");
        assert_eq!(lines[4].kind, DiffKind::Context);
        assert_eq!(lines[4].text, "keep");
    }

    #[test]
    fn preserves_indentation_in_context_lines() {
        let lines = parse_display_diff(" 12   indented");
        assert_eq!(lines[0].kind, DiffKind::Context);
        assert_eq!(lines[0].number.as_deref(), Some("12"));
        assert_eq!(lines[0].text, "  indented");
    }

    #[test]
    fn edit_info_uses_details_diff_first() {
        let input = r#"{"path":"src/a.rs","edits":[{"oldText":"a","newText":"b"}]}"#;
        let details = r#"{"diff":" 1 a\n-1 a\n+1 b"}"#;
        let info = edit_info("edit", input, details).unwrap();
        assert_eq!(info.path, "src/a.rs");
        assert_eq!(info.added, 1);
        assert_eq!(info.removed, 1);
    }

    #[test]
    fn edit_info_falls_back_to_edit_pairs() {
        let input =
            r#"{"path":"a.txt","edits":[{"oldText":"one\ntwo","newText":"three"}]}"#;
        let info = edit_info("edit", input, "").unwrap();
        assert_eq!(info.removed, 2);
        assert_eq!(info.added, 1);
        assert_eq!(info.lines[0].text, "one");
        assert_eq!(info.lines[2].kind, DiffKind::Added);
    }

    #[test]
    fn write_content_becomes_added_lines() {
        let input = r#"{"path":"new.txt","content":"a\nb\n"}"#;
        let info = edit_info("write", input, "").unwrap();
        assert!(info.created);
        assert_eq!(info.added, 2);
    }

    #[test]
    fn exec_info_reads_exit_code() {
        let input = r#"{"command":"cargo test","description":"run tests"}"#;
        let details = r#"{"exit_code":101,"failure_kind":"command_failed"}"#;
        let info = exec_info(input, details).unwrap();
        assert_eq!(info.command, "cargo test");
        assert_eq!(info.description, "run tests");
        assert_eq!(info.exit_code, Some(101));
        assert_eq!(info.failure_kind.as_deref(), Some("command_failed"));
    }

    #[test]
    fn subject_prefers_file_and_command() {
        assert_eq!(
            subject("read", r#"{"path":"src/main.rs"}"#, "read"),
            "src/main.rs"
        );
        assert_eq!(
            subject("bash", r#"{"command":"echo hi"}"#, "bash"),
            "echo hi"
        );
        assert_eq!(
            subject("browser_open", r#"{"url":"https://example.com"}"#, "browser_open"),
            "https://example.com"
        );
    }
}
