//! 只读会话库视图（待办 / 用量）。
//!
//! 数据来自 Agent CLI 的会话库（`MODE_SESSION_DB_PATH`，与产品同一个文件）。这里**只读**：
//! 打开时用 `SQLITE_OPEN_READ_ONLY` + 短 busy timeout——主程序或 CLI 正在写时宁可这一帧
//! 拿不到数据，也不能把库锁住或写坏。
//!
//! 表结构见 `crates/agent-core` 的迁移（`todo` / `turn_usage`）；字段只取展示需要的列，
//! 缺表/缺列（旧库）一律当成「没有数据」，不让 UI 崩。

use std::path::PathBuf;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

/// 一条待办（CLI 的 `todo` 表）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub content: String,
    /// `pending` / `in_progress` / `completed`（未知值原样保留）。
    pub status: String,
    /// `high` / `medium` / `low`（未知值原样保留）。
    pub priority: String,
}

/// 一个会话的累计用量（来自 `turn_usage` 聚合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UsageSummary {
    pub turns: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    /// 最近一回合的首 token 延迟（毫秒）。
    pub last_ttft_ms: Option<i64>,
}

fn db_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var("MODE_SESSION_DB_PATH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Some(PathBuf::from(path));
    }
    // 试点单独启动（没有宿主注入 env）时按产品默认找库：**Bot 自己的**数据根优先
    // （`~/.mode-bot/.mode/cli/db/db.sqlite`），再退到 CLI 的 HOME 默认
    // （`~/.mode/cli/db/db.sqlite`，那是 Mode 桌面端的库，只在 Bot 库不存在时兜底）。
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    let home = PathBuf::from(home);
    [
        home.join(".mode-bot")
            .join(".mode")
            .join("cli")
            .join("db")
            .join("db.sqlite"),
        home.join(".mode").join("cli").join("db").join("db.sqlite"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

/// 只读打开（文件不存在/被独占时返回 None）。
fn open_ro() -> Option<Connection> {
    let path = db_path()?;
    if !path.is_file() {
        return None;
    }
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    // 写者持有锁时最多等 300ms：拿不到就当这一帧没数据。
    connection.busy_timeout(Duration::from_millis(300)).ok()?;
    Some(connection)
}

/// 会话的待办列表（按 position 排序；表不存在时为空）。
pub fn todos(session_id: &str) -> Vec<TodoItem> {
    let Some(connection) = open_ro() else {
        return Vec::new();
    };
    let mut statement = match connection.prepare(
        "SELECT content, status, priority FROM todo WHERE session_id = ?1 ORDER BY position ASC",
    ) {
        Ok(statement) => statement,
        Err(_) => return Vec::new(),
    };
    let rows = statement.query_map([session_id], |row| {
        Ok(TodoItem {
            content: row.get::<_, String>(0)?,
            status: row.get::<_, String>(1)?,
            priority: row.get::<_, String>(2)?,
        })
    });
    let Ok(rows) = rows else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

/// 会话累计用量；没有回合记录时返回 None。
pub fn usage(session_id: &str) -> Option<UsageSummary> {
    let connection = open_ro()?;
    let mut statement = connection
        .prepare(
            "SELECT COUNT(*),
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(computed_total_tokens), 0)
             FROM turn_usage WHERE session_id = ?1",
        )
        .ok()?;
    let mut summary = statement
        .query_row([session_id], |row| {
            Ok(UsageSummary {
                turns: row.get::<_, i64>(0)?,
                input_tokens: row.get::<_, i64>(1)?,
                output_tokens: row.get::<_, i64>(2)?,
                total_tokens: row.get::<_, i64>(3)?,
                last_ttft_ms: None,
            })
        })
        .ok()?;
    if summary.turns == 0 {
        return None;
    }
    summary.last_ttft_ms = connection
        .prepare(
            "SELECT time_to_first_token_ms FROM turn_usage
             WHERE session_id = ?1 AND time_to_first_token_ms IS NOT NULL
             ORDER BY started_at DESC LIMIT 1",
        )
        .ok()
        .and_then(|mut statement| {
            statement
                .query_row([session_id], |row| row.get::<_, i64>(0))
                .ok()
        });
    Some(summary)
}

/// 全库累计用量（设置 → 用量页；所有会话加总，不含首 token 延迟）。
pub fn usage_totals() -> Option<UsageSummary> {
    let connection = open_ro()?;
    let mut statement = connection
        .prepare(
            "SELECT COUNT(*),
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(computed_total_tokens), 0)
             FROM turn_usage",
        )
        .ok()?;
    let summary = statement
        .query_row([], |row| {
            Ok(UsageSummary {
                turns: row.get::<_, i64>(0)?,
                input_tokens: row.get::<_, i64>(1)?,
                output_tokens: row.get::<_, i64>(2)?,
                total_tokens: row.get::<_, i64>(3)?,
                last_ttft_ms: None,
            })
        })
        .ok()?;
    if summary.turns == 0 {
        return None;
    }
    Some(summary)
}

/// 一条全文搜索命中（消息文本级）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub session_id: String,
    pub title: String,
    /// 命中位置前后的片段（已压成单行）。
    pub snippet: String,
    pub at: u64,
}

/// 在消息 part 里做 LIKE 搜索（本地库，几千~几万行量级够用）。
/// `limit` 限制条数；空查询直接返回空。
pub fn search_messages(query: &str, limit: usize) -> Vec<SearchHit> {
    let query = query.trim();
    if query.chars().count() < 2 || limit == 0 {
        return Vec::new();
    }
    let Some(connection) = open_ro() else {
        return Vec::new();
    };
    let pattern = format!("%{}%", escape_like(query));
    // 只搜正文（`$.text`），并按 rowid 倒序早停：
    // - 旧 SQL 是 `data LIKE` + `ORDER BY time_created DESC`：会命中 reasoning/tool 的
    //   JSON（连键名都算），而且强迫全表扫描 + 排序（1.3GB 库实测 2–11 秒）；
    // - 现在先按 type 过滤，再对 `$.text` 做 LIKE，按 id 倒序取到够用为止
    //   （实测 50–300ms）。
    // 多取一些再按会话去重：同一会话连着很多条命中时，不能把列表名额吃光。
    let scan = limit.saturating_mul(8).clamp(limit, 80);
    let Ok(mut statement) = connection.prepare(
        "SELECT p.session_id, s.title, json_extract(p.data, '$.text'), p.time_created
         FROM part p JOIN session s ON s.id = p.session_id
         WHERE json_extract(p.data, '$.type') IN ('text', 'markdown')
           AND json_extract(p.data, '$.text') LIKE ?1 ESCAPE '\\'
         ORDER BY p.id DESC
         LIMIT ?2",
    ) else {
        return Vec::new();
    };
    let rows = statement.query_map([pattern, scan.to_string()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)?,
        ))
    });
    let Ok(rows) = rows else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    for row in rows.filter_map(Result::ok) {
        let (session_id, title, text, at) = row;
        let Some(text) = text else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let Some(snippet) = snippet_around(&text, query) else {
            continue;
        };
        // 同一会话只留最新一条（列表更像「会话命中」而不是消息刷屏）。
        if hits
            .iter()
            .any(|hit: &SearchHit| hit.session_id == session_id)
        {
            continue;
        }
        hits.push(SearchHit {
            session_id,
            title,
            snippet,
            at: at.max(0) as u64,
        });
        if hits.len() >= limit {
            break;
        }
    }
    hits
}

/// LIKE 通配符转义（配合 SQL 里的 `ESCAPE '\\'`）。
fn escape_like(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '%' | '_' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            other => out.push(other),
        }
    }
    out
}

/// 命中位置前后各取一段（单行化；找不到查询词时取开头）。
fn snippet_around(text: &str, query: &str) -> Option<String> {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = flat.to_lowercase();
    let needle = query.to_lowercase();
    let (start, end) = match lower.find(&needle) {
        Some(position) => {
            let chars_before = lower[..position].chars().count();
            let chars_query = needle.chars().count();
            let total = flat.chars().count();
            let start = chars_before.saturating_sub(24);
            let end = (chars_before + chars_query + 36).min(total);
            (start, end)
        }
        None => (0, 60.min(flat.chars().count())),
    };
    if flat.is_empty() {
        return None;
    }
    let mut snippet: String = flat.chars().skip(start).take(end.saturating_sub(start)).collect();
    if start > 0 {
        snippet.insert(0, '…');
    }
    if end < flat.chars().count() {
        snippet.push('…');
    }
    Some(snippet)
}

/// 人类可读的 token 数（1.2k / 34.5k / 1.2M）。
pub fn format_tokens(value: i64) -> String {
    let value = value.max(0);
    if value >= 1_000_000_000 {
        return format!("{:.1}B", value as f64 / 1_000_000_000.0);
    }
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 环境变量是进程级共享：改 `MODE_SESSION_DB_PATH` 的用例必须串行。
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    #[test]
    fn tokens_formatting() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_000), "1.0k");
        assert_eq!(format_tokens(34_512), "34.5k");
        assert_eq!(format_tokens(1_234_567), "1.2M");
        assert_eq!(format_tokens(15_709_800_000), "15.7B");
    }

    #[test]
    fn snippet_extraction() {
        let text = "第一行：换行测试
第二行：Shift+Enter
第三行：上下移动";
        let snippet = snippet_around(text, "Shift+Enter").expect("必须命中");
        assert!(snippet.contains("Shift+Enter"), "片段要包含查询词：{snippet}");
        assert!(!snippet.contains('\u{000a}'), "片段必须单行：{snippet}");
        // 不命中时给开头一段，不 panic
        let fallback = snippet_around(text, "不存在的词").expect("也要给开头");
        assert!(fallback.starts_with("第一行"));
        // 空文本不产生命中
        assert!(snippet_around("   ", "x").is_none(), "空文本不给片段");
    }

    #[test]
    fn missing_db_yields_no_data() {
        let _guard = env_lock();
        // SAFETY: 本模块内所有改环境变量的用例都由 env_lock 串行。
        unsafe { std::env::set_var("MODE_SESSION_DB_PATH", "C:/definitely/not/here.db") };
        assert!(todos("sess_x").is_empty());
        assert!(usage("sess_x").is_none());
        // SAFETY: 收尾。
        unsafe { std::env::remove_var("MODE_SESSION_DB_PATH") };
    }

    /// 搜索必须只看正文：type 为 reasoning/tool 的 part、以及 JSON 键名都不算命中。
    #[test]
    fn search_filters_part_kind_and_dedupes() {
        let _guard = env_lock();
        let path = std::env::temp_dir().join(format!("pilot-search-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let connection = Connection::open(&path).expect("建测试库");
            connection
                .execute_batch(
                    "CREATE TABLE session (id TEXT PRIMARY KEY, title TEXT);
                     CREATE TABLE part (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        message_id TEXT, session_id TEXT, time_created INTEGER,
                        time_updated INTEGER, data TEXT, sequence INTEGER);",
                )
                .expect("建表");
            connection
                .execute(
                    "INSERT INTO session (id, title) VALUES ('s1', '会话一'), ('s2', '会话二')",
                    [],
                )
                .expect("会话");
            let rows = [
                // 命中：正文里出现「部署」
                ("s1", "{\"type\":\"text\",\"text\":\"今天把部署脚本整理好了\"}", 10),
                // 同一会话的旧命中：去重后不该重复出现
                ("s1", "{\"type\":\"text\",\"text\":\"部署完成\"}", 20),
                // 不该命中：type 是 reasoning
                ("s2", "{\"type\":\"reasoning\",\"text\":\"部署的思考过程\"}", 30),
                // 不该命中：只在 JSON 键名里出现查询词
                ("s2", "{\"type\":\"text\",\"text\":\"部署二字不在键名\",\"deploy\":1}", 25),
                // 应该命中：markdown
                ("s2", "{\"type\":\"markdown\",\"text\":\"## 部署清单\"}", 15),
                // 通配符按字面处理（ESCAPE 生效才会命中这条）
                ("s2", "{\"type\":\"text\",\"text\":\"进度 50% 完成\"}", 5),
            ];
            for (session, data, at) in rows {
                connection
                    .execute(
                        "INSERT INTO part (message_id, session_id, time_created, time_updated, data, sequence)
                         VALUES ('m', ?1, ?2, ?2, ?3, 0)",
                        rusqlite::params![session, at, data],
                    )
                    .expect("插入 part");
            }
        }
        // SAFETY: 本模块内所有改环境变量的用例都由 env_lock 串行。
        unsafe { std::env::set_var("MODE_SESSION_DB_PATH", &path) };
        let hits = search_messages("部署", 8);

        assert_eq!(hits.len(), 2, "两个会话各一条：{hits:?}");
        assert_eq!(hits[0].session_id, "s2", "按 part id 倒序，markdown 那条更新");
        assert_eq!(hits[1].session_id, "s1");
        assert!(hits.iter().all(|hit| hit.snippet.contains("部署")));
        assert!(
            hits.iter().all(|hit| hit.title.starts_with("会话")),
            "标题要带出来"
        );
        // `%` 是字面量而不是通配符：ESCAPE 写错时这里会全表命中或直接失败。
        let percent = search_messages("50%", 8);
        assert_eq!(percent.len(), 1, "字面 % 要能命中：{percent:?}");
        assert!(percent[0].snippet.contains("50%"));
        // 少于 2 个字不查（避免全库扫）
        assert!(search_messages("部", 8).is_empty());
        // SAFETY: 收尾（所有查询都做完后再撤环境变量）。
        unsafe { std::env::remove_var("MODE_SESSION_DB_PATH") };
        let _ = std::fs::remove_file(&path);
    }
}
