//! 任务运行中的入站消息队列（`messageQueue.ts` 的 Rust 移植）。
//!
//! 队列随 BotState 持久化（运行时波次接入）；这里只做纯函数：入队/去重/上限丢弃/出队。
//! - 上限 20，超出丢弃最旧（语义 = 最新指令优先）；
//! - 相同文本 60s 内视为平台重试；**带附件不按文本去重**（纯附件消息 text 可能为空）；
//! - 附件字节预算 20MiB（调用方据此决定「不入队」）。

use serde::{Deserialize, Serialize};

pub const BOT_MESSAGE_QUEUE_LIMIT: usize = 20;
pub const BOT_QUEUED_ATTACHMENT_BUDGET_BYTES: u64 = 20 * 1024 * 1024;
const DEDUPE_WINDOW_MS: i64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotQueuedAttachment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// 其余展示/路径字段由运行时建模；本层只关心字节预算。
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotHandoff {
    pub source_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotQueuedMessage {
    pub text: String,
    pub received_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quoted_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<BotQueuedAttachment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<BotHandoff>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BotQueuedMessageInput {
    pub text: String,
    pub quoted_text: Option<String>,
    pub provider_user_id: Option<String>,
    pub display_name: Option<String>,
    pub chat_id: Option<String>,
    pub attachments: Option<Vec<BotQueuedAttachment>>,
    pub handoff: Option<BotHandoff>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueResult {
    pub queue: Vec<BotQueuedMessage>,
    /// 本次入队后的位次（1 开始）；被去重时返回已有位次。
    pub position: usize,
    /// 因队列满被丢弃的最旧消息。
    pub dropped: Option<BotQueuedMessage>,
}

pub fn attachment_bytes(attachments: Option<&[BotQueuedAttachment]>) -> u64 {
    attachments
        .unwrap_or(&[])
        .iter()
        .map(|item| item.size_bytes.unwrap_or(0))
        .sum()
}

pub fn queued_attachment_bytes(queue: Option<&[BotQueuedMessage]>) -> u64 {
    queue
        .unwrap_or(&[])
        .iter()
        .map(|item| attachment_bytes(item.attachments.as_deref()))
        .sum()
}

pub fn enqueue_queued_message(
    queue: Option<&[BotQueuedMessage]>,
    input: &BotQueuedMessageInput,
    now: i64,
) -> EnqueueResult {
    let existing: Vec<BotQueuedMessage> = queue.unwrap_or(&[]).to_vec();
    let trimmed = input.text.trim().to_string();
    let attachments = input
        .attachments
        .clone()
        .filter(|items| !items.is_empty());
    let duplicate_index = if attachments.is_some() {
        None
    } else {
        existing.iter().position(|item| {
            item.text == trimmed && !item.text.is_empty() && now - item.received_at < DEDUPE_WINDOW_MS
        })
    };
    if let Some(index) = duplicate_index {
        return EnqueueResult {
            queue: existing,
            position: index + 1,
            dropped: None,
        };
    }
    let mut next = existing;
    next.push(BotQueuedMessage {
        text: trimmed,
        received_at: now,
        quoted_text: input.quoted_text.clone(),
        provider_user_id: input.provider_user_id.clone(),
        display_name: input.display_name.clone(),
        chat_id: input.chat_id.clone(),
        attachments,
        handoff: input.handoff.clone(),
    });
    let mut dropped = None;
    while next.len() > BOT_MESSAGE_QUEUE_LIMIT {
        dropped = Some(next.remove(0));
    }
    let position = next.len();
    EnqueueResult {
        queue: next,
        position,
        dropped,
    }
}

pub fn dequeue_queued_message(
    queue: Option<&[BotQueuedMessage]>,
) -> (Vec<BotQueuedMessage>, Option<BotQueuedMessage>) {
    let existing: Vec<BotQueuedMessage> = queue.unwrap_or(&[]).to_vec();
    if existing.is_empty() {
        return (existing, None);
    }
    let mut iter = existing.into_iter();
    let message = iter.next();
    (iter.collect(), message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(text: &str) -> BotQueuedMessageInput {
        BotQueuedMessageInput {
            text: text.to_string(),
            ..BotQueuedMessageInput::default()
        }
    }

    #[test]
    fn enqueue_trims_and_reports_position() {
        let result = enqueue_queued_message(None, &input("  hi  "), 1_000);
        assert_eq!(result.position, 1);
        assert_eq!(result.queue[0].text, "hi");
        assert!(result.dropped.is_none());
    }

    #[test]
    fn duplicate_text_within_window_is_skipped() {
        let first = enqueue_queued_message(None, &input("hi"), 1_000);
        let second = enqueue_queued_message(Some(&first.queue), &input("hi"), 1_500);
        assert_eq!(second.position, 1);
        assert_eq!(second.queue.len(), 1);
        // 窗口外可再次入队。
        let third = enqueue_queued_message(Some(&first.queue), &input("hi"), 100_000);
        assert_eq!(third.position, 2);
    }

    #[test]
    fn attachments_bypass_text_dedupe_and_count_bytes() {
        let attachment = BotQueuedAttachment {
            size_bytes: Some(2 * 1024 * 1024),
            extra: serde_json::Map::new(),
        };
        let first = enqueue_queued_message(
            None,
            &BotQueuedMessageInput {
                attachments: Some(vec![attachment.clone()]),
                ..BotQueuedMessageInput::default()
            },
            1_000,
        );
        let second = enqueue_queued_message(
            Some(&first.queue),
            &BotQueuedMessageInput {
                attachments: Some(vec![attachment]),
                ..BotQueuedMessageInput::default()
            },
            1_100,
        );
        assert_eq!(second.queue.len(), 2);
        assert_eq!(queued_attachment_bytes(Some(&second.queue)), 4 * 1024 * 1024);
    }

    #[test]
    fn limit_drops_oldest() {
        let mut queue: Option<Vec<BotQueuedMessage>> = None;
        let mut dropped = Vec::new();
        for index in 0..(BOT_MESSAGE_QUEUE_LIMIT + 2) {
            let result = enqueue_queued_message(queue.as_deref(), &input(&format!("m{index}")), index as i64);
            queue = Some(result.queue);
            if let Some(item) = result.dropped {
                dropped.push(item.text);
            }
        }
        let queue = queue.unwrap();
        assert_eq!(queue.len(), BOT_MESSAGE_QUEUE_LIMIT);
        assert_eq!(dropped, vec!["m0".to_string(), "m1".to_string()]);
        assert_eq!(queue[0].text, "m2");
    }

    #[test]
    fn dequeue_is_fifo() {
        let first = enqueue_queued_message(None, &input("a"), 1);
        let second = enqueue_queued_message(Some(&first.queue), &input("b"), 2);
        let (rest, message) = dequeue_queued_message(Some(&second.queue));
        assert_eq!(message.unwrap().text, "a");
        let (rest, message) = dequeue_queued_message(Some(&rest));
        assert_eq!(message.unwrap().text, "b");
        assert!(dequeue_queued_message(Some(&rest)).1.is_none());
    }
}
