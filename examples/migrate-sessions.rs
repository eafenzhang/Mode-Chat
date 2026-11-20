//! 会话库迁移（P4-1 产品级）：隔离试点库 → 主程序会话库。
//!
//! 决策 D4 = 共用主程序会话库；本工具把试点期间产生的会话搬进主库，供切换默认 UI 前
//! 一次性执行。**默认只做演练（dry-run）**，加 `--apply` 才落盘。
//!
//! 语义：
//! - 以 `session` 表为根，把会话连同其消息/part/事件日志/用量一起搬过去；
//! - 目标库已存在同名会话 id 的整条跳过（**幂等**，可重复执行）；
//! - 两库 schema 若有漂移，按「两边都有的列」交集搬运，并在输出里点名缺列；
//! - 目标库必须**已存在**（拒绝新建：路径打错时不会悄悄建一个空库）。
//!
//! 用法：
//! ```text
//! cargo run --example migrate-sessions                 # 演练：只报告将要迁移什么
//! cargo run --example migrate-sessions -- --apply      # 真正写库
//! cargo run --example migrate-sessions -- --from <隔离库> --to <主库> [--apply]
//! ```
//! 默认路径：`--from` = `{试点目录}/sessions.db`；`--to` = `MODE_SESSION_DB_PATH`
//! （未设置时 `{home}/.mode-bot/.mode/cli/db/db.sqlite`，与壳 `data_root.rs` 同口径）。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

/// 需要搬运的表（`session` 先插，其余按 `session_id` 过滤）。
const CHILD_TABLES: &[&str] = &[
    "message",
    "part",
    "session_entry",
    "session_input",
    "session_target",
    "todo",
    "tool_usage",
    "turn_usage",
    "model_usage",
];

struct Args {
    from: PathBuf,
    to: PathBuf,
    apply: bool,
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("[migrate] 参数错误：{error}");
            std::process::exit(2);
        }
    };
    if let Err(error) = run(&args) {
        eprintln!("[migrate] 失败：{error}");
        std::process::exit(1);
    }
}

fn parse_args() -> Result<Args, String> {
    let mut from: Option<PathBuf> = None;
    let mut to: Option<PathBuf> = None;
    let mut apply = false;
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--apply" => apply = true,
            "--from" => {
                let value = iter.next().ok_or("--from 缺少路径")?;
                from = Some(PathBuf::from(value));
            }
            "--to" => {
                let value = iter.next().ok_or("--to 缺少路径")?;
                to = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                println!("{}", HELP);
                std::process::exit(0);
            }
            other => return Err(format!("未知参数：{other}")),
        }
    }
    let from = from.unwrap_or_else(default_from);
    let to = to.or_else(env_session_db).unwrap_or_else(default_to);
    Ok(Args { from, to, apply })
}

const HELP: &str = "\
会话库迁移：隔离试点库 → 主程序会话库（默认演练，加 --apply 落盘）

  --from <path>   源库（默认 {试点目录}/sessions.db）
  --to   <path>   目标库（默认 MODE_SESSION_DB_PATH 或 {home}/.mode-bot/.mode/cli/db/db.sqlite）
  --apply         真正写库（默认只报告）
  -h, --help      本帮助";

fn default_from() -> PathBuf {
    pilot_dir().join("sessions.db")
}

fn default_to() -> PathBuf {
    home_dir()
        .join(".mode-bot")
        .join(".mode")
        .join("cli")
        .join("db")
        .join("db.sqlite")
}

fn pilot_dir() -> PathBuf {
    if let Some(path) = env_path("BOT_PILOT_STATE_FILE")
        && let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        return parent.to_path_buf();
    }
    home_dir().join(".mode-bot-pilot")
}

fn env_session_db() -> Option<PathBuf> {
    env_path("MODE_SESSION_DB_PATH")
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn home_dir() -> PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn run(args: &Args) -> Result<(), String> {
    if !args.from.is_file() {
        return Err(format!("源库不存在：{}", args.from.display()));
    }
    if !args.to.is_file() {
        return Err(format!(
            "目标库不存在：{}（本工具不新建目标库；先在主程序里跑一次会话再迁移）",
            args.to.display()
        ));
    }
    if same_file(&args.from, &args.to) {
        return Err("源库与目标库是同一个文件，无需迁移".to_string());
    }

    let mode = if args.apply { "落盘" } else { "演练（不改动，加 --apply 执行）" };
    println!("[migrate] 模式：{mode}");
    println!("[migrate] 源库：{}", args.from.display());
    println!("[migrate] 目标：{}", args.to.display());
    warn_if_running(&args.to);

    // 目标库读写打开，源库 ATTACH 进来：全程一条连接、一个事务。
    let conn = Connection::open_with_flags(
        &args.to,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| format!("打开目标库失败：{error}"))?;
    conn.execute(
        "ATTACH DATABASE ?1 AS src",
        [args.from.to_string_lossy().to_string()],
    )
    .map_err(|error| format!("挂载源库失败：{error}"))?;

    let sessions = collect_sessions(&conn)?;
    if sessions.is_empty() {
        println!("[migrate] 源库没有任何会话，无事可做");
        return Ok(());
    }

    // 待迁会话 = 源库有、目标库没有的 id。
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS picked (id TEXT PRIMARY KEY);
         DELETE FROM picked;
         INSERT OR IGNORE INTO picked (id) SELECT id FROM src.session;
         DELETE FROM picked WHERE id IN (SELECT id FROM main.session);",
    )
    .map_err(|error| format!("准备待迁会话集合失败：{error}"))?;
    let picked: i64 = conn
        .query_row("SELECT COUNT(*) FROM picked", [], |row| row.get(0))
        .map_err(|error| format!("统计待迁会话失败：{error}"))?;
    let skipped = sessions.len() as i64 - picked;
    println!(
        "[migrate] 源库会话 {} 个：待迁 {picked}，目标库已存在跳过 {skipped}",
        sessions.len()
    );
    for session in &sessions {
        let already: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM main.session WHERE id = ?1)",
                [&session.id],
                |row| row.get(0),
            )
            .unwrap_or(false);
        println!(
            "  {} {} {} 条消息  {}{}",
            if already { "跳过" } else { "迁移" },
            short(&session.id),
            session.messages,
            truncate(&session.title, 28),
            if already { "（目标库已有）" } else { "" }
        );
    }

    if picked == 0 {
        println!("[migrate] 没有需要迁移的会话（幂等）");
        return Ok(());
    }
    if !args.apply {
        println!("[migrate] 演练结束：未写入任何数据。确认无误后加 --apply 执行。");
        return Ok(());
    }

    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("开启事务失败：{error}"))?;

    // 1) 会话根行。
    let session_columns = shared_columns(&conn, "session")?;
    let inserted_sessions = copy_rows(
        &conn,
        "session",
        &session_columns,
        "id IN (SELECT id FROM picked)",
    )?;
    // 2) 子表：按 session_id 过滤。
    let mut table_report: Vec<(String, i64, Vec<String>)> = Vec::new();
    for table in CHILD_TABLES {
        if !table_exists(&conn, "src", table)? {
            table_report.push((table.to_string(), 0, vec![]));
            continue;
        }
        if !table_exists(&conn, "main", table)? {
            return Err(format!("目标库缺少表 {table}（主程序版本过旧？）"));
        }
        let columns = shared_columns(&conn, table)?;
        let filter = "session_id IN (SELECT id FROM picked)";
        let inserted = copy_rows(&conn, table, &columns, filter)?;
        let missing = missing_columns(&conn, table)?;
        table_report.push((table.to_string(), inserted, missing));
    }

    transaction
        .commit()
        .map_err(|error| format!("提交事务失败：{error}"))?;

    println!("[migrate] 已写入：session {inserted_sessions} 行");
    for (table, inserted, missing) in table_report {
        let note = if missing.is_empty() {
            String::new()
        } else {
            format!("（目标库缺列已跳过：{}）", missing.join(", "))
        };
        println!("  {table:<14} {inserted:>6} 行{note}");
    }

    // 3) 搬完复核：逐会话比对消息条数（只读校验，失败不算迁移失败但大声报）。
    let mut mismatched = 0usize;
    for session in &sessions {
        let expected = session.messages;
        let actual: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM main.message WHERE session_id = ?1",
                [&session.id],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        if actual != expected {
            mismatched += 1;
            eprintln!(
                "  [migrate] 复核不一致：{} 源 {expected} / 目标 {actual}",
                short(&session.id)
            );
        }
    }
    if mismatched == 0 {
        println!("[migrate] 复核通过：{} 个会话的消息条数与源库一致", sessions.len());
    } else {
        println!("[migrate] 复核发现 {mismatched} 个会话条数不一致（见上）");
    }
    Ok(())
}

struct SourceSession {
    id: String,
    title: String,
    messages: i64,
}

fn collect_sessions(conn: &Connection) -> Result<Vec<SourceSession>, String> {
    let mut statement = conn
        .prepare(
            "SELECT s.id, s.title,
                    (SELECT COUNT(*) FROM src.message m WHERE m.session_id = s.id)
             FROM src.session s
             ORDER BY s.time_created ASC",
        )
        .map_err(|error| format!("读取源库会话失败：{error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(SourceSession {
                id: row.get(0)?,
                title: row.get(1)?,
                messages: row.get(2)?,
            })
        })
        .map_err(|error| format!("读取源库会话失败：{error}"))?;
    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row.map_err(|error| format!("读取源库会话行失败：{error}"))?);
    }
    Ok(sessions)
}

fn table_exists(conn: &Connection, schema: &str, table: &str) -> Result<bool, String> {
    let count: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM {schema}.sqlite_master WHERE type='table' AND name=?1"),
            [table],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查表 {schema}.{table} 失败：{error}"))?;
    Ok(count > 0)
}

fn columns_of(conn: &Connection, schema: &str, table: &str) -> Result<Vec<String>, String> {
    let mut statement = conn
        .prepare(&format!("PRAGMA {schema}.table_info({table})"))
        .map_err(|error| format!("读取 {schema}.{table} 列失败：{error}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("读取 {schema}.{table} 列失败：{error}"))?;
    let mut columns = Vec::new();
    for row in rows {
        columns.push(row.map_err(|error| format!("读取 {schema}.{table} 列失败：{error}"))?);
    }
    Ok(columns)
}

/// 目标库里存在、源库没有的列（搬不过去的信息要显式点名）。
fn missing_columns(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    let target = columns_of(conn, "main", table)?;
    let source = columns_of(conn, "src", table)?;
    Ok(target
        .into_iter()
        .filter(|column| !source.iter().any(|existing| existing == column))
        .collect())
}

/// 两边都有的列（按目标库列顺序，保持 DDL 兼容）。
fn shared_columns(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    let target = columns_of(conn, "main", table)?;
    let source = columns_of(conn, "src", table)?;
    let shared: Vec<String> = target
        .into_iter()
        .filter(|column| source.iter().any(|existing| existing == column))
        .collect();
    if shared.is_empty() {
        return Err(format!("表 {table} 在源库与目标库没有共同列"));
    }
    Ok(shared)
}

/// `INSERT OR IGNORE … SELECT`，返回实际写入行数。
fn copy_rows(
    conn: &Connection,
    table: &str,
    columns: &[String],
    filter: &str,
) -> Result<i64, String> {
    let list = columns.join(", ");
    let sql = format!(
        "INSERT OR IGNORE INTO main.{table} ({list}) SELECT {list} FROM src.{table} WHERE {filter}"
    );
    conn.execute(&sql, [])
        .map_err(|error| format!("搬运 {table} 失败：{error}"))?;
    Ok(conn.changes() as i64)
}

/// 目标库旁有 `-wal`/`-shm` 且主程序可能在跑时提醒用户先退出。
fn warn_if_running(target: &Path) {
    let wal = with_suffix(target, "-wal");
    let shm = with_suffix(target, "-shm");
    if wal.exists() || shm.exists() {
        eprintln!(
            "[migrate] 提醒：目标库旁存在 WAL/SHM 文件（{}），请先完全退出 Mode Bot 再执行迁移",
            target.display()
        );
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut raw = path.as_os_str().to_os_string();
    raw.push(suffix);
    PathBuf::from(raw)
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn short(id: &str) -> &str {
    if id.len() > 12 { &id[..12] } else { id }
}

fn truncate(text: &str, limit: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= limit {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}
