//! 企微请求/回执关联（媒体上传 init/chunk/finish 与媒体发送）。
//!
//! WS 传输只做帧泵；需要读取 `body.upload_id` / `body.media_id` 的请求由本模块登记
//! `req_id`，运行时收到同 `req_id` 的回执时唤醒等待方。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::wecom_protocol::reply_frame;

#[derive(Debug, Clone, PartialEq)]
pub struct WeComAck {
    pub errcode: i64,
    pub errmsg: String,
    pub body: Value,
}

#[derive(Clone, Default)]
pub struct WeComRequestBroker {
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<WeComAck>>>>,
}

impl WeComRequestBroker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, req_id: &str) -> Result<oneshot::Receiver<WeComAck>, String> {
        let (sender, receiver) = oneshot::channel();
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if pending.contains_key(req_id) {
            return Err(format!("请求 {req_id} 已有等待中的回执。"));
        }
        pending.insert(req_id.to_string(), sender);
        Ok(receiver)
    }

    pub fn is_pending(&self, req_id: &str) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(req_id)
    }

    pub fn resolve(&self, req_id: &str, ack: WeComAck) {
        let sender = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(req_id);
        if let Some(sender) = sender {
            let _ = sender.send(ack);
        }
    }

    pub fn remove(&self, req_id: &str) {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(req_id);
    }
}

pub async fn request_frame(
    broker: &WeComRequestBroker,
    outbound: &mpsc::UnboundedSender<Value>,
    cmd: &str,
    req_id: &str,
    body: Value,
    timeout: Duration,
) -> Result<Value, String> {
    let receiver = broker.register(req_id)?;
    if outbound.send(reply_frame(cmd, req_id, body)).is_err() {
        broker.remove(req_id);
        return Err("企微连接已关闭，请求未发送。".to_string());
    }
    match tokio::time::timeout(timeout, receiver).await {
        Ok(Ok(ack)) => {
            if ack.errcode != 0 {
                return Err(format!("{cmd} 失败（{}）：{}", ack.errcode, ack.errmsg));
            }
            Ok(ack.body)
        }
        Ok(Err(_)) => {
            broker.remove(req_id);
            Err(format!("{cmd} 回执通道已关闭。"))
        }
        Err(_) => {
            broker.remove(req_id);
            Err(format!("{cmd} 回执超时。"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn duplicate_register_does_not_overwrite_the_first_waiter() {
        let broker = WeComRequestBroker::new();
        let _first = broker.register("r1").unwrap();
        assert!(broker.register("r1").is_err());
        assert!(broker.is_pending("r1"));
    }

    #[tokio::test]
    async fn register_is_pending_until_resolved() {
        let broker = WeComRequestBroker::new();
        let mut receiver = broker.register("r1").unwrap();
        assert!(broker.is_pending("r1"));
        broker.resolve(
            "r1",
            WeComAck {
                errcode: 0,
                errmsg: String::new(),
                body: json!({ "upload_id": "u1" }),
            },
        );
        let ack = receiver.try_recv().unwrap();
        assert_eq!(ack.body["upload_id"], "u1");
        assert!(!broker.is_pending("r1"));
    }

    #[tokio::test]
    async fn request_frame_returns_ack_body() {
        let broker = WeComRequestBroker::new();
        let (outbound, mut frames) = mpsc::unbounded_channel::<Value>();
        let worker = {
            let broker = broker.clone();
            tokio::spawn(async move {
                let frame = frames.recv().await.unwrap();
                assert_eq!(frame["cmd"], "aibot_upload_media_init");
                let req_id = frame["headers"]["req_id"].as_str().unwrap().to_string();
                assert_eq!(frame["body"]["filename"], "a.txt");
                broker.resolve(
                    &req_id,
                    WeComAck {
                        errcode: 0,
                        errmsg: String::new(),
                        body: json!({ "upload_id": "up-1" }),
                    },
                );
            })
        };
        let body = request_frame(
            &broker,
            &outbound,
            "aibot_upload_media_init",
            "aibot_upload_media_init_1_abcd",
            json!({ "filename": "a.txt" }),
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(body["upload_id"], "up-1");
        worker.await.unwrap();
    }

    #[tokio::test]
    async fn request_frame_surfaces_errcode() {
        let broker = WeComRequestBroker::new();
        let (outbound, mut frames) = mpsc::unbounded_channel::<Value>();
        let worker = {
            let broker = broker.clone();
            tokio::spawn(async move {
                let frame = frames.recv().await.unwrap();
                let req_id = frame["headers"]["req_id"].as_str().unwrap().to_string();
                broker.resolve(
                    &req_id,
                    WeComAck {
                        errcode: 40001,
                        errmsg: "bad media".to_string(),
                        body: json!({}),
                    },
                );
            })
        };
        let error = request_frame(
            &broker,
            &outbound,
            "aibot_upload_media_init",
            "aibot_upload_media_init_2_abcd",
            json!({}),
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert!(error.contains("40001"), "{error}");
        assert!(error.contains("bad media"), "{error}");
        worker.await.unwrap();
    }

    #[tokio::test]
    async fn request_frame_times_out_and_clears_pending() {
        let broker = WeComRequestBroker::new();
        let (outbound, _frames) = mpsc::unbounded_channel::<Value>();
        let error = request_frame(
            &broker,
            &outbound,
            "aibot_upload_media_finish",
            "aibot_upload_media_finish_3_abcd",
            json!({}),
            Duration::from_millis(20),
        )
        .await
        .unwrap_err();
        assert!(error.contains("超时"), "{error}");
        assert!(!broker.is_pending("aibot_upload_media_finish_3_abcd"));
    }
}
