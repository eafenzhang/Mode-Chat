//! 单实例锁 + 崩溃落盘（P4-2 产品级横切面）。
//!
//! 锁协议**复用** `atomic_file::acquire_file_lock`（目录 CAS + owner pid 存活判定），
//! 与壳的跨进程锁是同一套机制——不新发明 IPC。第二个实例拿不到锁时把已有窗口置前
//! 然后退出：不建第二个窗口、不写第二份会话状态。
//!
//! 崩溃落盘：panic hook 把消息/位置/线程/回溯写到 `{state_dir}/crash-<ts>.log`，
//! 再转给上一个 hook（默认行为不变）。日志只落试点目录，不碰产品数据。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use atomic_file::{acquire_file_lock, AtomicError, FileLock, WriteOptions};

/// 占住单实例锁的句柄：进程存活期间必须一直持有（Drop 时释放）。
pub struct SingleInstance {
    _lock: FileLock,
}

/// 拿锁结果。
pub enum Acquire {
    Acquired(SingleInstance),
    /// 已有实例持有锁（正常并发启动，不是错误）。
    AlreadyRunning,
    /// 锁机制本身失败（权限/磁盘）；调用方决定是继续还是退出。
    Failed(String),
}

/// 以 `target` 为锁目标拿单实例锁（实际锁目录是 `{target}.lock`）。
///
/// 第二个实例只等 1 秒：正常收敛的落盘锁会在几十毫秒内释放，超过即认定为
/// 「另一实例常驻」，不在启动路径上长时间阻塞。
pub fn acquire(target: &Path) -> Acquire {
    let options = WriteOptions {
        lock_max_wait_ms: 1_000,
        lock_retry_delays_ms: vec![100, 200, 300],
        ..Default::default()
    };
    match acquire_file_lock(target, &options) {
        Ok(lock) => Acquire::Acquired(SingleInstance { _lock: lock }),
        Err(AtomicError::LockTimeout { .. }) => Acquire::AlreadyRunning,
        Err(error) => Acquire::Failed(error.to_string()),
    }
}

/// 试点状态文件覆盖（`BOT_PILOT_STATE_FILE`，全路径）。
///
/// 放在这里而不是 `state` 模块：`instance` 要被 `examples/panic-probe` 单文件引用，
/// 不能反向依赖 crate 里的其他模块。
pub fn state_file_override() -> Option<PathBuf> {
    std::env::var("BOT_PILOT_STATE_FILE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// 试点数据目录（状态文件与崩溃日志同目录；`BOT_PILOT_STATE_FILE` 一并隔离）。
pub fn state_dir() -> PathBuf {
    if let Some(path) = state_file_override()
        && let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        return parent.to_path_buf();
    }
    home_dir().join(".mode-bot-pilot")
}

fn home_dir() -> PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// 安装 panic hook：先落盘，再交给原 hook（终端输出/退出码语义不变）。
pub fn install_panic_hook(dir: &Path) {
    let dir = dir.to_path_buf();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|value| value.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "（非字符串 panic）".to_string());
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "未知位置".to_string());
        let thread = std::thread::current()
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| "unnamed".to_string());
        let text = format!(
            "panic\n时间: {}\n线程: {thread}\n位置: {location}\n消息: {payload}\n\n回溯:\n{}\n",
            iso_now(),
            std::backtrace::Backtrace::force_capture()
        );
        if let Err(error) = write_crash_log(&dir, &text) {
            eprintln!("[crash] 崩溃日志写入失败：{error}");
        }
        previous(info);
    }));
}

/// 写一条崩溃日志（原子替换；文件名带时间戳，保留最近 20 份）。
fn write_crash_log(dir: &Path, text: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("创建目录失败（{}）：{error}", dir.display()))?;
    let path = dir.join(format!("crash-{}.log", stamp()));
    // 崩溃路径上不抢跨进程锁（可能正被占着）：临时文件 + rename 已够。
    atomic_file::write_text_atomic(
        &path,
        text,
        &WriteOptions {
            use_file_lock: false,
            ..Default::default()
        },
    )
    .map_err(|error| format!("写入失败（{}）：{error}", path.display()))?;
    prune_crash_logs(dir, 20);
    Ok(path)
}

/// 只留最近 `keep` 份 `crash-*.log`（按文件名时间戳排序即可）。
fn prune_crash_logs(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .map(|name| {
                    let name = name.to_string_lossy();
                    name.starts_with("crash-") && name.ends_with(".log")
                })
                .unwrap_or(false)
        })
        .collect();
    if logs.len() <= keep {
        return;
    }
    logs.sort();
    let remove_count = logs.len() - keep;
    for path in logs.into_iter().take(remove_count) {
        let _ = std::fs::remove_file(path);
    }
}

fn stamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{:03}", now.as_secs(), now.subsec_millis())
}

fn iso_now() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let millis_total = now.as_millis() as i64;
    let secs = millis_total.div_euclid(1000);
    let millis = millis_total.rem_euclid(1000);
    let days = secs.div_euclid(86400);
    let secs_of_day = secs.rem_euclid(86400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// 天数（1970-01-01 起）转年月日（Howard Hinnant 算法）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 把已存在的试点窗口置前；找不到时静默（**绝不新建窗口**）。
///
/// 用 gpui-windows 注册的窗口类名 `Zed::Window` + 试点标题定位——比只按标题找
/// 更精确（主程序的 Tauri 窗口类是另一套，不会误命中）。
#[cfg(windows)]
pub fn focus_existing_window(title: &str) {
    use std::ffi::c_void;
    type Hwnd = *mut c_void;
    const SW_RESTORE: i32 = 9;
    const HWND_TOPMOST: Hwnd = -1isize as Hwnd;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_SHOWWINDOW: u32 = 0x0040;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd;
        fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
        fn SetForegroundWindow(hwnd: Hwnd) -> i32;
        fn SetWindowPos(
            hwnd: Hwnd,
            after: Hwnd,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let class = wide("Zed::Window");
    let wanted_title = wide(title);
    // SAFETY: 两个指针都指向以 NUL 结尾的宽字符串，生命周期覆盖调用。
    let hwnd = unsafe { FindWindowW(class.as_ptr(), wanted_title.as_ptr()) };
    if hwnd.is_null() {
        return;
    }
    // SAFETY: hwnd 来自 FindWindowW，非空已校验；常量参数与 Win32 约定一致。
    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
        // 直接 SetForegroundWindow 在前台锁竞争下会失败；先置顶再落到前台。
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
        SetForegroundWindow(hwnd);
        SetWindowPos(
            hwnd,
            -2isize as Hwnd,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
    }
}

#[cfg(not(windows))]
pub fn focus_existing_window(_title: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例独立目录，结束不清理（临时目录，系统会回收）。
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bot-pilot-instance-{}-{}-{}",
            name,
            std::process::id(),
            stamp()
        ));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    #[test]
    fn second_acquire_reports_already_running_until_released() {
        let dir = scratch("lock");
        let target = dir.join("pilot-instance");
        let first = match acquire(&target) {
            Acquire::Acquired(lock) => lock,
            Acquire::AlreadyRunning => panic!("首个实例不该被判为已在运行"),
            Acquire::Failed(error) => panic!("首个实例拿锁失败：{error}"),
        };
        assert!(
            matches!(acquire(&target), Acquire::AlreadyRunning),
            "持锁期间第二个实例必须被判定为已在运行"
        );
        drop(first);
        assert!(
            matches!(acquire(&target), Acquire::Acquired(_)),
            "释放后必须能重新拿锁"
        );
    }

    #[test]
    fn crash_log_written_and_pruned() {
        let dir = scratch("crash");
        for index in 0..25 {
            let text = format!("panic #{index}\n");
            let path = write_crash_log(&dir, &text).expect("写崩溃日志");
            assert!(path.exists(), "崩溃日志必须落盘：{}", path.display());
        }
        let count = std::fs::read_dir(&dir)
            .expect("读目录")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("crash-")
            })
            .count();
        assert!(count <= 20, "崩溃日志必须只留最近 20 份，实际 {count}");
        let newest = std::fs::read_dir(&dir)
            .expect("读目录")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.to_string_lossy().contains("crash-"))
            .max()
            .expect("至少一份日志");
        let text = std::fs::read_to_string(newest).expect("读日志");
        assert!(text.contains("panic #24"), "最新日志内容缺失：{text}");
    }
}
