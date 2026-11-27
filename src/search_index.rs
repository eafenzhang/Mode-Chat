//! 全局消息搜索的 FTS5 索引（独立库，不碰外部 CLI 的 db.sqlite）。
//!
//! - 索引源：pi-rust 的 JSONL 会话文件（`{data_root}/v2/mode-chat-sessions`）；
//! - 增量：按 `(mtime, size)` 跳过没变的文件，变了的按 rowid 区间整段重建，
//!   首次建库整体灌一次；消息何时落库归 pi-rust harness 管，本仓库无从 hook，
//!   所以搜索入口先 `refresh()` 再查，保证新鲜；
//! - 分词：`trigram`（CJK 友好，LIKE/MATCH 都能走索引）；短查询（<3 字符）
//!   用 `body LIKE '%…%'`，长查询用 `MATCH` 排序质量更好；
//! - 隐私：`.private`/`.side` 目录与隐私 workspace 的会话不进索引（与
//!   `rpi_engine::collect_search` 同口径）；
//! - 索引落后或损坏时 `search()` 返回 `None`，调用方回退线性扫描。

use std::path::{Path, PathBuf};

use rusqlite::Connection;

/// 索引库路径：`{data_root}/v2/search-index.sqlite`。
pub fn index_path() -> PathBuf {
    crate::data::data_root().join("v2").join("search-index.sqlite")
}

fn open() -> Result<Connection, String> {
    open_at(&index_path())
}

fn open_at(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path)
        .map_err(|error| format!("打开搜索索引失败：{error}"))?;
    conn.busy_timeout(std::time::Duration::from_millis(300))
        .map_err(|error| format!("搜索索引忙超时设置失败：{error}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS files (
             path TEXT PRIMARY KEY,
             mtime INTEGER NOT NULL,
             size INTEGER NOT NULL,
             title TEXT NOT NULL,
             min_rowid INTEGER NOT NULL,
             max_rowid INTEGER NOT NULL
         );
         CREATE VIRTUAL TABLE IF NOT EXISTS fts_messages USING fts5(
             body,
             session_id UNINDEXED,
             at UNINDEXED,
             tokenize='trigram'
         );",
    )
    .map_err(|error| format!("初始化搜索索引失败：{error}"))?;
    Ok(conn)
}

/// 把索引增量到与 JSONL 目录一致；返回重建的文件数。
pub fn refresh() -> usize {
    let Ok(mut conn) = open() else {
        return 0;
    };
    let private_workspaces = crate::data::private_workspace_dirs();
    refresh_in(&mut conn, &crate::rpi_engine::sessions_root(), &private_workspaces)
}

/// `refresh` 的可测内核：把索引增量到与 `root` 目录一致。
fn refresh_in(conn: &mut Connection, root: &Path, private_workspaces: &[String]) -> usize {
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_jsonl(root, &mut paths);
    paths.retain(|path| !is_private_file(path, private_workspaces));

    // 清理已从磁盘消失的文件（索引里残留会话会一直被搜到）。
    let stale: Vec<(String, i64, i64)> = {
        let mut stmt = match conn.prepare("SELECT path, min_rowid, max_rowid FROM files") {
            Ok(stmt) => stmt,
            Err(_) => return 0,
        };
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    };
    for (path, min_rowid, max_rowid) in stale {
        if paths.iter().any(|p| p.to_string_lossy() == path) {
            continue;
        }
        let _ = conn.execute(
            "DELETE FROM fts_messages WHERE rowid BETWEEN ?1 AND ?2",
            rusqlite::params![min_rowid, max_rowid],
        );
        let _ = conn.execute("DELETE FROM files WHERE path = ?1", [path]);
    }

    let mut rebuilt = 0usize;
    for path in paths {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        let size = meta.len() as i64;
        let known = conn
            .query_row(
                "SELECT mtime, size FROM files WHERE path = ?1",
                [&path.to_string_lossy()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .ok();
        if known == Some((mtime, size)) {
            continue;
        }
        if rebuild_file(&mut *conn, &path, mtime, size) {
            rebuilt += 1;
        }
    }
    rebuilt
}

/// 搜索：`None` = 索引不可用（调用方回退线性扫描）。
pub fn search(query: &str, limit: usize) -> Option<Vec<crate::session_db::SearchHit>> {
    match open() {
        Ok(conn) => search_at(&conn, query, limit),
        Err(_) => None,
    }
}

/// `search` 的可测内核。
fn search_at(conn: &Connection, query: &str, limit: usize)
    -> Option<Vec<crate::session_db::SearchHit>>
{
    let query = query.trim();
    if query.chars().count() < 2 || limit == 0 {
        return Some(Vec::new());
    }
    let clause = match build_clause(query) {
        Some(clause) => clause,
        None => return Some(Vec::new()),
    };
    // LIMIT 直接拼（usize，无注入面）；绑字符串会被 SQLite coerce 成 0。
    let mut stmt = match conn.prepare(&format!(
        "SELECT session_id, at, snippet(fts_messages, 0, '', '', '…', 24)
         FROM fts_messages WHERE {} ORDER BY at DESC LIMIT {limit}",
        clause.kind
    )) {
        Ok(stmt) => stmt,
        Err(_) => return None, // 索引库半途损坏等：回退
    };
    let rows = stmt
        .query_map([clause.value], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .ok()?
        .flatten()
        .collect::<Vec<_>>();
    // 每个会话只留最新一条命中（与 session_db::search_messages 的去重口径一致）。
    let mut best: Vec<crate::session_db::SearchHit> = Vec::new();
    for (session_id, at, snippet) in rows {
        if best
            .iter()
            .any(|hit| hit.session_id == session_id && hit.at >= at)
        {
            continue;
        }
        best.retain(|hit| hit.session_id != session_id);
        let title = conn
            .query_row(
                "SELECT title FROM files WHERE path = ?1",
                [&session_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap_or_else(|_| "会话".to_string());
        best.push(crate::session_db::SearchHit {
            session_id,
            title,
            snippet,
            at,
        });
        if best.len() >= limit {
            break;
        }
    }
    best.sort_by_key(|hit| std::cmp::Reverse(hit.at));
    Some(best)
}

struct Clause {
    kind: &'static str,
    value: String,
}

/// 组查询：词全部 ≥3 字符走 MATCH（trigram），否则整串走 LIKE（trigram 表的
/// LIKE 也走索引）。返回 None = 没有可用词。
fn build_clause(query: &str) -> Option<Clause> {
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    if words.iter().all(|word| word.chars().count() >= 3) {
        let quoted = words
            .iter()
            .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ");
        return Some(Clause {
            kind: "fts_messages MATCH ?1",
            value: quoted,
        });
    }
    let like = format!(
        "%{}%",
        query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    Some(Clause {
        kind: "body LIKE ?1 ESCAPE '\\'",
        value: like,
    })
}

/// 重建一个文件：按 rowid 区间删旧行，再插全文件的消息行。
fn rebuild_file(conn: &mut Connection, path: &Path, mtime: i64, size: i64) -> bool {
    let session_id = path.to_string_lossy().into_owned();
    if let Err(error) = conn.execute(
        "DELETE FROM fts_messages WHERE rowid BETWEEN
             (SELECT min_rowid FROM files WHERE path = ?1)
             AND (SELECT max_rowid FROM files WHERE path = ?1)",
        [&session_id],
    ) {
        eprintln!("[search-index] 清旧行失败 {session_id}: {error}");
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        let _ = conn.execute("DELETE FROM files WHERE path = ?1", [&session_id]);
        return false;
    };
    let parsed = parse_file(&text);
    if parsed.is_empty() {
        // 没有可索引消息也登记（mtime 挡住重复解析），title 仍来自 header。
        let title = parsed_title(&text);
        let _ = conn.execute(
            "INSERT INTO files (path, mtime, size, title, min_rowid, max_rowid)
             VALUES (?1, ?2, ?3, ?4, 0, 0)
             ON CONFLICT(path) DO UPDATE SET mtime = ?2, size = ?3, title = ?4,
                 min_rowid = 0, max_rowid = 0",
            rusqlite::params![session_id, mtime, size, title],
        );
        return true;
    }
    let mut min_rowid = 0i64;
    let mut max_rowid = 0i64;
    for (at, body) in &parsed {
        if conn
            .execute(
                "INSERT INTO fts_messages (body, session_id, at) VALUES (?1, ?2, ?3)",
                rusqlite::params![body, session_id, at],
            )
            .is_ok()
        {
            let rowid = conn.last_insert_rowid();
            if min_rowid == 0 || rowid < min_rowid {
                min_rowid = rowid;
            }
            if rowid > max_rowid {
                max_rowid = rowid;
            }
        }
    }
    let _ = conn.execute(
        "INSERT INTO files (path, mtime, size, title, min_rowid, max_rowid)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(path) DO UPDATE SET mtime = ?2, size = ?3, title = ?4,
             min_rowid = ?5, max_rowid = ?6",
        rusqlite::params![
            session_id,
            mtime,
            size,
            parsed_title(&text),
            min_rowid,
            max_rowid
        ],
    );
    true
}

/// 从文件文本里解析 (timestamp, 消息正文)；口径与 `rpi_engine::collect_search`
/// 一致：header 取 cwd 尾段当标题，user/assistant 的 text 内容进索引。
fn parse_file(text: &str) -> Vec<(u64, String)> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|item| item.as_str()) != Some("message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let kind = message
            .get("kind")
            .and_then(|item| item.as_str())
            .unwrap_or("");
        if kind != "user" && kind != "assistant" {
            continue;
        }
        let mut body = String::new();
        if let Some(text) = message.get("content").and_then(|item| item.as_str()) {
            body.push_str(text);
        }
        for content in message
            .get("content")
            .and_then(|item| item.as_array())
            .into_iter()
            .flatten()
        {
            if content.get("type").and_then(|item| item.as_str()) == Some("text") {
                body.push_str(
                    content
                        .get("text")
                        .and_then(|item| item.as_str())
                        .unwrap_or(""),
                );
            }
        }
        let body = body.trim().to_string();
        if body.is_empty() {
            continue;
        }
        let at = value
            .get("timestamp")
            .and_then(|item| item.as_u64())
            .unwrap_or(0);
        rows.push((at, body));
    }
    rows
}

/// 标题：header 的 cwd 尾段（与 collect_search 同口径）。
fn parsed_title(text: &str) -> String {
    for line in text.lines().take(8) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("kind").and_then(|item| item.as_str()) != Some("header") {
            continue;
        }
        if let Some(cwd) = value.get("cwd").and_then(|item| item.as_str()) {
            return cwd
                .rsplit(['\\', '/'])
                .find(|part| !part.is_empty())
                .unwrap_or(cwd)
                .to_string();
        }
    }
    String::new()
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some(".private") | Some(".side")
            ) {
                continue;
            }
            collect_jsonl(&path, out);
            continue;
        }
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        if !name.ends_with(".jsonl") || name.ends_with(".progress.jsonl") {
            continue;
        }
        out.push(path);
    }
}

/// 旧版隐私会话可能落在主目录：按 header cwd 排除（与 collect_search 同口径）。
/// 只读文件头（header 在第一行，64KB 足够），避免每次 refresh 全文读大文件。
fn is_private_file(path: &Path, private_workspaces: &[String]) -> bool {
    if private_workspaces.is_empty() {
        return false;
    }
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut head = String::new();
    if file.take(64 * 1024).read_to_string(&mut head).is_err() {
        return false;
    }
    head.lines().take(8).any(|line| {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            return false;
        };
        if value.get("kind").and_then(|item| item.as_str()) != Some("header") {
            return false;
        }
        value
            .get("cwd")
            .and_then(|item| item.as_str())
            .is_some_and(|cwd| {
                private_workspaces
                    .iter()
                    .any(|dir| cwd.eq_ignore_ascii_case(dir))
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "mc-search-index-{}-{}",
                tag,
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_session(path: &Path, cwd: &str, messages: &[&str]) {
        let header = serde_json::json!({"kind": "header", "cwd": cwd});
        let mut text = format!("{header}\n");
        for (index, message) in messages.iter().enumerate() {
            let line = serde_json::json!({
                "type": "message",
                "timestamp": 1700_000_000_000i64 + index as i64 * 1000,
                "message": {"kind": "assistant", "content": message}
            });
            text.push_str(&format!("{line}\n"));
        }
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn clause_builder() {
        // 词全 ≥3 字符：走 MATCH（trigram），引号要转义。
        let clause = build_clause("报错日志 a\"b").unwrap();
        assert_eq!(clause.kind, "fts_messages MATCH ?1");
        assert_eq!(clause.value, "\"报错日志\" AND \"a\"\"b\"");
        // 短词：整串 LIKE，通配符转义。
        let clause = build_clause("ab%").unwrap();
        assert_eq!(clause.kind, "fts_messages MATCH ?1");
        // 2 字符词触发 LIKE 路径。
        let clause = build_clause("ab").unwrap();
        assert_eq!(clause.kind, "body LIKE ?1 ESCAPE '\\'");
        assert_eq!(clause.value, "%ab%");
    }

    #[test]
    fn refresh_search_and_invalidate() {
        let dir = TempDir::new("refresh");
        let sessions = dir.0.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let file_a = sessions.join("a.jsonl");
        let file_b = sessions.join("b.jsonl");
        write_session(&file_a, "C:\\ws\\alpha", &["部署脚本报错了，需要修 CI 流水线", "再补一条回归测试"]);
        write_session(&file_b, "C:\\ws\\beta", &["今天天气不错"]);
        let conn_path = dir.0.join("index.sqlite");
        let mut conn = open_at(&conn_path).unwrap();

        assert_eq!(refresh_in(&mut conn, &sessions, &[]), 2);
        let hits = search_at(&conn, "部署脚本", 8).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "alpha");
        assert!(hits[0].snippet.contains("部署脚本"), "{}", hits[0].snippet);
        assert!(hits[0].session_id.ends_with("a.jsonl"));
        // 短查询走 LIKE 路径也能命中。
        assert_eq!(search_at(&conn, "天气", 8).unwrap().len(), 1);

        // 无变化：不再重建。
        assert_eq!(refresh_in(&mut conn, &sessions, &[]), 0);
        // 文件内容变化：重建 1 个。
        write_session(&file_a, "C:\\ws\\alpha", &["部署脚本已修复，流水线恢复绿色"]);
        assert_eq!(refresh_in(&mut conn, &sessions, &[]), 1);
        assert!(search_at(&conn, "流水线恢复", 8).unwrap().len() == 1);
        assert!(search_at(&conn, "回归测试", 8).unwrap().is_empty());
        // 文件删除：索引同步清掉。
        std::fs::remove_file(&file_b).unwrap();
        refresh_in(&mut conn, &sessions, &[]);
        assert!(search_at(&conn, "天气", 8).unwrap().is_empty());
    }

    #[test]
    fn private_files_are_excluded() {
        let dir = TempDir::new("private");
        let sessions = dir.0.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let file = sessions.join("p.jsonl");
        write_session(&file, "C:\\private-ws", &["私密内容不应进索引"]);
        let conn_path = dir.0.join("index.sqlite");
        let mut conn = open_at(&conn_path).unwrap();
        refresh_in(&mut conn, &sessions, &["C:\\private-ws".to_string()]);
        assert!(search_at(&conn, "私密内容", 8).unwrap().is_empty());
    }
}
