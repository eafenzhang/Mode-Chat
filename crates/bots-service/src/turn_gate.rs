//! Bot 回合全局准入门（`botTurnGate.ts` 的完整移植）。
//!
//! 单实例内全局有界并发（默认 8）；同一会话由上层按对话键串行，不同群/私聊在全局槽位内
//! 并行。超限进入全局 FIFO 等待队列；同 taskId 的 enter 幂等（重复调用复用同一结果，
//! `on_wait` 只由首个调用者触发一次）。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

pub const BOT_GLOBAL_TURN_CONCURRENCY_CAP: usize = 8;

#[derive(Debug, Default)]
struct GateState {
    workspace_by_task: HashMap<String, String>,
    waiters: HashSet<String>,
}

#[derive(Clone)]
pub struct BotTurnGate {
    limit: usize,
    state: Arc<Mutex<GateState>>,
    wake: Arc<Notify>,
}

impl BotTurnGate {
    pub fn new(cap: usize) -> Self {
        Self {
            limit: cap.max(1),
            state: Arc::new(Mutex::new(GateState::default())),
            wake: Arc::new(Notify::new()),
        }
    }

    pub fn with_default_cap() -> Self {
        Self::new(BOT_GLOBAL_TURN_CONCURRENCY_CAP)
    }

    /// 获取槽位；超限时等待。`on_wait` 在同 taskId 首次因超限挂起时触发一次。
    pub async fn enter(&self, workspace_key: &str, task_id: &str, mut on_wait: Option<Box<dyn FnOnce() + Send>>) {
        let mut wait_notified = false;
        loop {
            let mut state = self.state.lock().await;
            if state.workspace_by_task.contains_key(task_id) {
                return;
            }
            // 同 taskId 已有等待者：等一次唤醒后返回（与 TS 复用同一 promise 同义）。
            if state.waiters.contains(task_id) {
                drop(state);
                self.wake.notified().await;
                let state = self.state.lock().await;
                if state.workspace_by_task.contains_key(task_id) {
                    return;
                }
                continue;
            }
            if state.workspace_by_task.len() < self.limit {
                state
                    .workspace_by_task
                    .insert(task_id.to_string(), workspace_key.to_string());
                return;
            }
            state.waiters.insert(task_id.to_string());
            drop(state);
            if !wait_notified {
                wait_notified = true;
                if let Some(callback) = on_wait.take() {
                    callback();
                }
            }
            self.wake.notified().await;
            let mut state = self.state.lock().await;
            state.waiters.remove(task_id);
            if state.workspace_by_task.contains_key(task_id) {
                return;
            }
        }
    }

    /// 释放槽位；未知/重复 taskId 为 no-op，并唤醒等待者。
    pub async fn exit(&self, task_id: &str) {
        let removed = {
            let mut state = self.state.lock().await;
            state.workspace_by_task.remove(task_id).is_some()
        };
        if removed {
            self.wake.notify_waiters();
        }
    }

    /// 清空并唤醒全部等待者（进程收尾/对账）。
    pub async fn reset(&self) {
        {
            let mut state = self.state.lock().await;
            state.workspace_by_task.clear();
            state.waiters.clear();
        }
        self.wake.notify_waiters();
    }

    /// 全局运行中回合数；给定 workspace 时为该 workspace 的计数。
    pub async fn count(&self, workspace_key: Option<&str>) -> usize {
        let state = self.state.lock().await;
        match workspace_key {
            None => state.workspace_by_task.len(),
            Some(key) => state
                .workspace_by_task
                .values()
                .filter(|value| value.as_str() == key)
                .count(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn cap_blocks_then_exit_wakes_waiter() {
        let gate = BotTurnGate::new(1);
        gate.enter("ws", "t1", None).await;
        assert_eq!(gate.count(None).await, 1);
        let notified = Arc::new(AtomicUsize::new(0));
        let notified_clone = Arc::clone(&notified);
        let gate_clone = gate.clone();
        let waiter = tokio::spawn(async move {
            gate_clone
                .enter(
                    "ws",
                    "t2",
                    Some(Box::new(move || {
                        notified_clone.fetch_add(1, Ordering::SeqCst);
                    })),
                )
                .await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(gate.count(None).await, 1, "t2 应仍在等待");
        gate.exit("t1").await;
        tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("waiter 应被唤醒")
            .unwrap();
        assert_eq!(gate.count(None).await, 1);
        assert_eq!(notified.load(Ordering::SeqCst), 1);
        gate.exit("t2").await;
        assert_eq!(gate.count(None).await, 0);
    }

    #[tokio::test]
    async fn same_task_enter_is_idempotent_and_on_wait_once() {
        let gate = BotTurnGate::new(1);
        gate.enter("ws", "t1", None).await;
        let gate_clone = gate.clone();
        let waiter = tokio::spawn(async move {
            gate_clone.enter("ws", "t2", None).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        // 重复 enter 同一 task：复用等待，不新增槽位。
        let notified = Arc::new(AtomicUsize::new(0));
        let notified_clone = Arc::clone(&notified);
        let gate_clone = gate.clone();
        let duplicate = tokio::spawn(async move {
            gate_clone
                .enter(
                    "ws",
                    "t2",
                    Some(Box::new(move || {
                        notified_clone.fetch_add(1, Ordering::SeqCst);
                    })),
                )
                .await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        gate.exit("t1").await;
        tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("t2 应拿到槽位")
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), duplicate)
            .await
            .expect("重复 enter 应返回")
            .unwrap();
        // 只有一个槽位被 t2 占用（重复 enter 不重复占槽）。
        assert_eq!(gate.count(None).await, 1);
        assert_eq!(gate.count(Some("ws")).await, 1);
        assert_eq!(notified.load(Ordering::SeqCst), 0, "重复 enter 不再触发 onWait");
        gate.exit("t2").await;
    }

    #[tokio::test]
    async fn reset_wakes_all_waiters() {
        let gate = BotTurnGate::new(1);
        gate.enter("ws", "t1", None).await;
        let gate_clone = gate.clone();
        let waiter = tokio::spawn(async move {
            gate_clone.enter("ws", "t2", None).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        gate.reset().await;
        tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("reset 应唤醒等待者")
            .unwrap();
        // 被唤醒的 t2 重新获得槽位。
        assert_eq!(gate.count(None).await, 1);
        gate.exit("t2").await;
    }
}
