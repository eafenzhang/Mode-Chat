//! 微信运行时的宿主接线（凭据 / 入站回调 / 游标持久化 / 状态投影）。
//!
//! 把 `WeixinPollLoop` 需要的四个宿主能力落到 bot 数据层：
//! - 凭据：由宿主注入的 `WeixinCredentialSource` 读取（bot 侧实现走 credential store）；
//! - 入站回调：`WeixinMessageHandler`（会话编排在端口化完成后接入；当前由宿主决定）；
//! - 游标：写 `bot-state` 的 `bots[botId].weixinGetUpdatesBuf`（锁内 RMW，重启不丢）；
//! - 状态：进程内 `botId → WeixinRuntimeStatus` 投影，供 `bots.getStatus` 合并。
//!
//! 状态文件按不透明 JSON 保留（切片 1 决策），这里只触碰自己的键，不动其它通道字段。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::weixin_inbound::WeixinInboundMessage;
use crate::weixin_runtime::{RuntimeFuture, WeixinRuntimeHost, WeixinRuntimeStatus};
use crate::BotsRepo;

pub trait WeixinCredentialSource: Send + Sync {
    fn load_token<'a>(&'a self, bot_id: &'a str) -> RuntimeFuture<'a, Option<String>>;
}

pub trait WeixinMessageHandler: Send + Sync {
    fn handle<'a>(
        &'a self,
        bot_id: &'a str,
        message: &'a WeixinInboundMessage,
    ) -> RuntimeFuture<'a, Result<(), String>>;
}

/// 运行时状态登记表（getStatus 的活态来源）。
#[derive(Clone, Default)]
pub struct WeixinStatusRegistry {
    statuses: Arc<Mutex<HashMap<String, WeixinRuntimeStatus>>>,
}

impl WeixinStatusRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, status: WeixinRuntimeStatus) {
        self.statuses
            .lock()
            .unwrap()
            .insert(status.bot_id.clone(), status);
    }

    pub fn get(&self, bot_id: &str) -> Option<WeixinRuntimeStatus> {
        self.statuses.lock().unwrap().get(bot_id).cloned()
    }

    pub fn remove(&self, bot_id: &str) {
        self.statuses.lock().unwrap().remove(bot_id);
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub struct WeixinBotHost {
    repo: BotsRepo,
    credentials: Arc<dyn WeixinCredentialSource>,
    handler: Arc<dyn WeixinMessageHandler>,
    statuses: WeixinStatusRegistry,
}

impl WeixinBotHost {
    pub fn new(
        repo: BotsRepo,
        credentials: Arc<dyn WeixinCredentialSource>,
        handler: Arc<dyn WeixinMessageHandler>,
        statuses: WeixinStatusRegistry,
    ) -> Self {
        Self {
            repo,
            credentials,
            handler,
            statuses,
        }
    }

    /// 读取已持久化的同步游标（无记录为空串）。
    pub fn read_buf(&self, bot_id: &str) -> Result<String, String> {
        let state = self.repo.read_state()?;
        Ok(state
            .get("bots")
            .and_then(|bots| bots.get(bot_id))
            .and_then(|entry| entry.get("weixinGetUpdatesBuf"))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string())
    }
}

impl WeixinRuntimeHost for WeixinBotHost {
    fn load_token<'a>(&'a self, bot_id: &'a str) -> RuntimeFuture<'a, Option<String>> {
        self.credentials.load_token(bot_id)
    }

    fn process_message<'a>(
        &'a self,
        bot_id: &'a str,
        message: &'a WeixinInboundMessage,
    ) -> RuntimeFuture<'a, Result<(), String>> {
        self.handler.handle(bot_id, message)
    }

    fn persist_buf<'a>(
        &'a self,
        bot_id: &'a str,
        buf: &'a str,
    ) -> RuntimeFuture<'a, Result<(), String>> {
        let bot_id = bot_id.to_string();
        let buf = buf.to_string();
        let result = self.repo.mutate_state(|state| {
            let bots = state
                .get_mut("bots")
                .and_then(|value| value.as_object_mut())
                .ok_or_else(|| "bot-state 缺少 bots 对象".to_string())?;
            let entry = bots.entry(bot_id.clone()).or_insert_with(|| {
                serde_json::json!({
                    "botId": bot_id,
                    "conversations": {},
                    "updatedAt": now_millis(),
                })
            });
            let object = entry
                .as_object_mut()
                .ok_or_else(|| format!("bot-state[{bot_id}] 不是对象"))?;
            // TS `botChannelStateSchema` 要求 conversations/updatedAt 必填：这里补齐，
            // 避免 Rust 写出的状态被 Node/zod 整份拒绝。
            object
                .entry("conversations")
                .or_insert_with(|| serde_json::json!({}));
            object.insert("updatedAt".into(), serde_json::json!(now_millis()));
            object.insert(
                "weixinGetUpdatesBuf".to_string(),
                serde_json::Value::String(buf),
            );
            Ok(())
        });
        Box::pin(async move { result })
    }

    fn set_status(&self, status: WeixinRuntimeStatus) {
        self.statuses.set(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct StaticToken(Option<String>);
    impl WeixinCredentialSource for StaticToken {
        fn load_token<'a>(&'a self, _bot_id: &'a str) -> RuntimeFuture<'a, Option<String>> {
            let token = self.0.clone();
            Box::pin(async move { token })
        }
    }

    struct EchoHandler(Mutex<Vec<String>>);
    impl WeixinMessageHandler for EchoHandler {
        fn handle<'a>(
            &'a self,
            _bot_id: &'a str,
            message: &'a WeixinInboundMessage,
        ) -> RuntimeFuture<'a, Result<(), String>> {
            self.0
                .lock()
                .unwrap()
                .push(message.actor.provider_message_id.clone().unwrap_or_default());
            Box::pin(async move { Ok(()) })
        }
    }

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("weixin-host-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn message() -> WeixinInboundMessage {
        WeixinInboundMessage {
            bot_id: "b1".to_string(),
            text: "hi".to_string(),
            attachments: Vec::new(),
            actor: crate::weixin_inbound::WeixinInboundActor {
                provider_user_id: "u1".to_string(),
                display_name: None,
                chat_type: "private".to_string(),
                chat_id: None,
                provider_message_id: Some("m1".to_string()),
                provider_context_token: None,
            },
        }
    }

    #[tokio::test]
    async fn persists_buf_and_projects_status() {
        let base = sandbox("buf");
        let statuses = WeixinStatusRegistry::new();
        let host = WeixinBotHost::new(
            BotsRepo::new(&base),
            Arc::new(StaticToken(Some("token".to_string()))),
            Arc::new(EchoHandler(Mutex::new(Vec::new()))),
            statuses.clone(),
        );
        assert_eq!(host.load_token("b1").await.as_deref(), Some("token"));
        host.persist_buf("b1", "buf-7").await.unwrap();
        assert_eq!(host.read_buf("b1").unwrap(), "buf-7");
        host.process_message("b1", &message()).await.unwrap();
        host.set_status(WeixinRuntimeStatus::new("b1", "connected", None));
        assert_eq!(statuses.get("b1").unwrap().status, "connected");
        // 状态文件其它字段不被触碰（只写自己的键）。
        let state = BotsRepo::new(&base).read_state().unwrap();
        assert_eq!(state["bots"]["b1"]["weixinGetUpdatesBuf"], "buf-7");
        std::fs::remove_dir_all(base).ok();
    }
}
