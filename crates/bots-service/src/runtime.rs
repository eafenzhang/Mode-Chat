//! 平台运行时共享原语（`channelRuntime.ts` 的 Rust 移植）。
//!
//! - 连接指纹：`sha256(JSON.stringify(parts))`，配置保存后据此判断是否需要重连；
//! - provider 回调结果校验：`ok=false` 是**可恢复业务失败**，外部游标必须在 ok 后才提交；
//! - 最新配置刷新队列：配置连续保存时串行执行，后一轮等待前一轮释放连接，且只有最新一轮
//!   的 `is_latest()` 为 true（失败不会阻断后续轮次）；
//! - 运行时锁：复用 `atomic-file` 的跨进程锁（目录 CAS + owner pid 存活判定），
//!   等锁超时视为「另一实例持有」返回 `None`（与 TS 的 30s 租约设计一致：不抢活锁）。
//! - 可取消等待：`wait_for(ms, signal)` / `wait_for_abort(signal)`。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use atomic_file::{acquire_file_lock, AtomicError, FileLock, WriteOptions};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

/// 运行时锁的重试等待（与 TS `BOT_RUNTIME_LOCK_RETRY_MS` 同量级）。
pub const BOT_RUNTIME_LOCK_RETRY_MS: u64 = 10_000;

pub fn connection_fingerprint(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    let json = serde_json::to_string(parts).unwrap_or_default();
    hasher.update(json.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// `BotProviderCallbackResult`：`ok=false` 携带可恢复状态（如 `status: "pending"`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotCallbackResult {
    pub ok: bool,
    pub status: Option<String>,
}

impl BotCallbackResult {
    pub fn ok() -> Self {
        Self {
            ok: true,
            status: None,
        }
    }

    pub fn failed(status: impl Into<String>) -> Self {
        Self {
            ok: false,
            status: Some(status.into()),
        }
    }
}

/// provider 回调失败必须显式抛出：外部消费游标只能在 ok 后提交，否则瞬时失败会被错误确认。
pub fn assert_callback_succeeded(provider: &str, result: &BotCallbackResult) -> Result<(), String> {
    if result.ok {
        return Ok(());
    }
    Err(format!(
        "{provider} callback failed: status={}",
        result.status.as_deref().unwrap_or("unknown")
    ))
}

/// 一次性刷新票据：`is_latest()` 表示是否仍是最新一轮配置。
#[derive(Clone)]
pub struct RefreshTicket {
    generation: u64,
    latest: Arc<AtomicU64>,
}

impl RefreshTicket {
    pub fn is_latest(&self) -> bool {
        self.latest.load(Ordering::SeqCst) == self.generation
    }
}

/// 串行 + 最新语义的刷新队列（见模块注释）。
#[derive(Clone, Default)]
pub struct LatestRuntimeRefreshQueue {
    generation: Arc<AtomicU64>,
    gate: Arc<Mutex<()>>,
}

impl LatestRuntimeRefreshQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn enqueue<F, Fut, T>(&self, reconcile: F) -> T
    where
        F: FnOnce(RefreshTicket) -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let _guard = self.gate.lock().await;
        reconcile(RefreshTicket {
            generation,
            latest: Arc::clone(&self.generation),
        })
        .await
    }

    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

/// 运行时可取消信号（轻量 AbortSignal）。
#[derive(Clone, Default)]
pub struct AbortSignal {
    aborted: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl AbortSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::SeqCst)
    }

    pub fn abort(&self) {
        if !self.aborted.swap(true, Ordering::SeqCst) {
            self.notify.notify_waiters();
        }
    }

    /// 等待 `ms` 毫秒；被 abort 时立即返回（与 TS `waitFor` 同义）。
    pub async fn wait_for(&self, ms: u64) {
        if self.is_aborted() {
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(ms)) => {}
            _ = self.notify.notified() => {}
        }
    }

    /// 等到 abort。
    pub async fn wait_for_abort(&self) {
        if self.is_aborted() {
            return;
        }
        self.notify.notified().await;
    }
}

/// 运行时锁句柄：drop/`release` 前一直持有；锁目录随进程崩溃由 pid 存活判定回收。
pub struct BotRuntimeLock {
    _lock: FileLock,
    path: PathBuf,
}

impl BotRuntimeLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// 获取 bot 运行时锁；另一实例持有时返回 `None`（等锁上限与 TS 重试节奏一致）。
pub fn acquire_bot_runtime_lock(
    lock_root: &Path,
    namespace: &str,
    lock_key: &str,
) -> Result<Option<BotRuntimeLock>, String> {
    let key = connection_fingerprint(&[lock_key.trim()]);
    let path = lock_root.join(namespace).join(format!("{key}.lock"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建运行时锁目录失败: {error}"))?;
    }
    let options = WriteOptions {
        lock_max_wait_ms: BOT_RUNTIME_LOCK_RETRY_MS,
        ..WriteOptions::default()
    };
    match acquire_file_lock(&path, &options) {
        Ok(lock) => Ok(Some(BotRuntimeLock { _lock: lock, path })),
        Err(AtomicError::LockTimeout { .. }) => Ok(None),
        Err(error) => Err(format!("获取运行时锁失败: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_and_order_sensitive() {
        let a = connection_fingerprint(&["feishu", "app", "ref", "secret"]);
        let b = connection_fingerprint(&["feishu", "app", "ref", "secret"]);
        let c = connection_fingerprint(&["secret", "ref", "app", "feishu"]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn callback_failure_is_explicit() {
        assert!(assert_callback_succeeded("feishu", &BotCallbackResult::ok()).is_ok());
        let error = assert_callback_succeeded("feishu", &BotCallbackResult::failed("pending"))
            .unwrap_err();
        assert!(error.contains("status=pending"), "{error}");
    }

    #[tokio::test]
    async fn refresh_queue_serializes_and_marks_only_latest() {
        let queue = LatestRuntimeRefreshQueue::new();
        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let order_a = Arc::clone(&order);
        let queue_a = queue.clone();
        let first = tokio::spawn(async move {
            queue_a
                .enqueue(move |ticket| {
                    let order = Arc::clone(&order_a);
                    async move {
                        order.lock().await.push("first");
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        ticket.is_latest()
                    }
                })
                .await
        });
        // 第二轮在排队；第一轮执行期间 generation 已递增 → first 不是 latest。
        let second = tokio::spawn(async move {
            queue
                .enqueue(|ticket| async move { ticket.is_latest() })
                .await
        });
        let first_latest = first.await.unwrap();
        let second_latest = second.await.unwrap();
        assert!(!first_latest);
        assert!(second_latest);
        let order = order.lock().await;
        assert_eq!(*order, vec!["first"]);
    }

    #[tokio::test]
    async fn abort_signal_interrupts_wait_for() {
        let signal = AbortSignal::new();
        let cloned = signal.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cloned.abort();
        });
        let started = std::time::Instant::now();
        signal.wait_for(5_000).await;
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(signal.is_aborted());
    }

    #[test]
    fn runtime_lock_is_exclusive_within_process() {
        let dir = std::env::temp_dir().join(format!(
            "bots-runtime-lock-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let held = acquire_bot_runtime_lock(&dir, "feishu-websocket", "app-1")
            .unwrap()
            .expect("first lock");
        // 同进程第二次获取：atomic-file 的 owner 存活判定会等待后超时 → None。
        let short = WriteOptions {
            lock_max_wait_ms: 200,
            ..WriteOptions::default()
        };
        let path = held.path().to_path_buf();
        assert!(matches!(
            acquire_file_lock(&path, &short),
            Err(AtomicError::LockTimeout { .. })
        ));
        drop(held);
        assert!(
            acquire_bot_runtime_lock(&dir, "feishu-websocket", "app-1")
                .unwrap()
                .is_some()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
