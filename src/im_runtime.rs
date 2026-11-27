//! 微信 / 企业微信 IM 运行时（GPUI 版）：长轮询/长连接、入站闸门、绑定、会话编排与 v4 回合。
//!
//! 复用 `bots-service` 的通道实现（WeixinPollLoop / WeCom WS / 门禁 / 会话编排），
//! 宿主侧只提供四件事：HTTP、凭据、v4 回合操作、回复投递。
//!
//! 与主程序共用 `v2/bot-config.v3.json` 与 `bot-state.v3.json`；跨进程锁
//! （`bots-runtime-locks/weixin-polling` / `wecom-websocket`）保证同一 token/bot
//! 只有一个实例在收消息。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bots_service::command::{parse_bot_command, BotCommand};
use bots_service::commands::{
    handle_new, handle_status, handle_workspace_list, handle_workspace_set, CommandReply,
};
use bots_service::bot_text::split_bot_text;
use bots_service::attachments::{InboundAttachment, materialize_attachments};
use bots_service::conversation::{
    BotReplySink, BotTurnOps, ConversationContext, ConversationRunner, ConversationStore,
    InboundTurn, OutboundMedia, PendingPermissionPrompt, PermissionPromptOption,
};
use bots_service::media::resolve_media_file;
use bots_service::gate::{evaluate_inbound_gate, GateDecision, GateMessage};
use bots_service::heartbeat::{build_heartbeat_prompt, heartbeat_locale};
use bots_service::http::HttpJsonPort;
use bots_service::runtime::{AbortSignal, BotRuntimeLock, acquire_bot_runtime_lock};
use bots_service::wecom::{WeComDedupe, WeComDedupeEntry};
use bots_service::media::{base64_encode, md5_hex, plan_media_chunks};
use bots_service::wecom_payload::{
    WECOM_TYPING_PLACEHOLDER, WeComInboundAttachment, markdown_send_body, media_reply_body,
    media_send_body, media_upload_chunk_body, media_upload_finish_body, media_upload_init_body,
    parse_inbound_message, stream_reply_body,
};
use bots_service::wecom_protocol::{
    CMD_RESPONSE, CMD_SEND_MSG, CMD_UPLOAD_MEDIA_CHUNK, CMD_UPLOAD_MEDIA_FINISH,
    CMD_UPLOAD_MEDIA_INIT, generate_req_id, reply_frame,
};
use bots_service::wecom_requests::{WeComAck, WeComRequestBroker, request_frame};
use bots_service::wecom_ws::{EventSink, WeComWsConfig, WeComWsEvent, run_wecom_ws};
use bots_service::weixin::WeixinClient;
use bots_service::weixin_host::{
    WeixinBotHost, WeixinCredentialSource, WeixinMessageHandler, WeixinStatusRegistry,
};
use bots_service::weixin_inbound::{
    WeixinInboundActor, WeixinInboundMessage,
};
use bots_service::weixin_runtime::{RuntimeFuture, WeixinPollLoop, WeixinRuntimeStatus};
use bots_service::workspace::{create_workspace_ref, WorkspaceRef};
use bots_service::{
    BotConfig, BotGroupActivation, BotGroupChat, BotGroupPermission, BotGroupPermissionStatus,
    BotProvider, BotRegisteredUser, BotsRepo,
};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::UnboundedSender;

use crate::im_service::{BindCodeStore, ReqwestHttpPort, TokenLoader, TokenVault, now_ms};
use bots_service::dingtalk;
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

/// 按数据根读 bots state/config，找桌面任务的 IM 镜像目标。
fn mirror_targets_for(
    data_root: &std::path::Path,
    task_id: &str,
    workspace_path: &str,
    workspace_identity: Option<&str>,
) -> Vec<bots_service::mirror::MirrorTarget> {
    let repo = BotsRepo::new(data_root);
    let Ok(state) = repo.read_state() else {
        return Vec::new();
    };
    let Ok(config) = repo.read_config() else {
        return Vec::new();
    };
    bots_service::mirror::find_mirror_targets(
        &state,
        &config,
        task_id,
        workspace_path,
        workspace_identity,
    )
}

fn config_fingerprint(bot: &BotConfig) -> u64 {    let bytes = serde_json::to_vec(bot).unwrap_or_default();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

struct RunningLoop {
    signal: AbortSignal,
    task: tokio::task::JoinHandle<()>,
    fingerprint: u64,
    _lock: Option<BotRuntimeLock>,
}

struct RunningWecom {
    signal: AbortSignal,
    task: tokio::task::JoinHandle<()>,
    fingerprint: u64,
    _lock: Option<BotRuntimeLock>,
    outbound: UnboundedSender<Value>,
}

/// 钉钉 Stream 长连接运行句柄（与企微同构：锁 + fingerprint 对齐 + abort 句柄）。
struct RunningDingtalk {
    signal: AbortSignal,
    task: tokio::task::JoinHandle<()>,
    fingerprint: u64,
    _lock: Option<BotRuntimeLock>,
}

/// 外挂适配器运行句柄（bot_id → 子进程 + 回复句柄）。
struct RunningAdapter {
    shared: Arc<std::sync::Mutex<Option<crate::im_adapter::AdapterProcess>>>,
    fingerprint: u64,
    _lock: Option<BotRuntimeLock>,
}

/// 微信轮询 / 企微长连接运行时管理器。
pub struct ImRuntimeManager {
    data_root: std::path::PathBuf,
    http: Arc<ReqwestHttpPort>,
    tokens: Arc<TokenVault>,
    bind_codes: Arc<BindCodeStore>,
    statuses: WeixinStatusRegistry,
    running: Mutex<HashMap<String, RunningLoop>>,
    running_wecom: Mutex<HashMap<String, RunningWecom>>,
    running_dingtalk: Mutex<HashMap<String, RunningDingtalk>>,
    running_adapter: Mutex<HashMap<String, RunningAdapter>>,
    heartbeat_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    stopped: AtomicBool,
}

impl ImRuntimeManager {
    pub fn new(
        data_root: std::path::PathBuf,
        http: Arc<ReqwestHttpPort>,
        tokens: Arc<TokenVault>,
        bind_codes: Arc<BindCodeStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            data_root,
            http,
            tokens,
            bind_codes,
            statuses: WeixinStatusRegistry::new(),
            running: Mutex::new(HashMap::new()),
            running_wecom: Mutex::new(HashMap::new()),
            running_dingtalk: Mutex::new(HashMap::new()),
            running_adapter: Mutex::new(HashMap::new()),
            heartbeat_task: Mutex::new(None),
            stopped: AtomicBool::new(false),
        })
    }

    pub fn statuses(&self) -> &WeixinStatusRegistry {
        &self.statuses
    }

    pub fn is_running(&self, bot_id: &str) -> bool {
        self.running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(bot_id)
            || self
                .running_wecom
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains_key(bot_id)
    }

    /// 找桌面任务在 IM 侧需要回显的会话目标（与壳同一份 mirror 策略）。
    pub fn mirror_targets(
        &self,
        task_id: &str,
        workspace_path: &str,
        workspace_identity: Option<&str>,
    ) -> Vec<bots_service::mirror::MirrorTarget> {
        mirror_targets_for(&self.data_root, task_id, workspace_path, workspace_identity)
    }

    /// 投递一条桌面消息/回复镜像（微信 REST / 企微复用运行中的 WS 出站队列）。
    pub async fn deliver_mirror(
        &self,
        target: &bots_service::mirror::MirrorTarget,
        text: &str,
        attachment_count: usize,
    ) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        if target.provider == BotProvider::Wecom {
            let outbound = self
                .running_wecom
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&target.bot_id)
                .map(|entry| entry.outbound.clone())
                .ok_or_else(|| {
                    format!("企微连接未运行，无法镜像（bot={}）", target.bot_id)
                })?;
            let mut body = bots_service::bot_text::to_plain_text_reply(text);
            if attachment_count > 0 {
                body.push_str(&format!(
                    "\n（另有 {attachment_count} 个附件未同步到 IM频道）"
                ));
            }
            for chunk in split_bot_text(&body, 3_500) {
                outbound
                    .send(reply_frame(
                        CMD_SEND_MSG,
                        &generate_req_id(CMD_SEND_MSG),
                        markdown_send_body(&target.target_id, &chunk),
                    ))
                    .map_err(|_| {
                        format!(
                            "企微连接已关闭，镜像未发送（bot={}）",
                            target.bot_id
                        )
                    })?;
            }
            return Ok(());
        }
        let config = BotsRepo::new(&self.data_root).read_config()?;
        let bot = config
            .bots
            .iter()
            .find(|bot| bot.id == target.bot_id)
            .ok_or_else(|| format!("找不到 IM频道机器人配置：{}", target.bot_id))?;
        let from_user_id = bot
            .provider_self_id
            .clone()
            .or_else(|| bot.provider_user_id.clone())
            .unwrap_or_default();
        let sink = ImReplySink::new(
            Arc::clone(&self.http),
            self.tokens.loader(),
            &target.bot_id,
            &from_user_id,
        );
        let mut body = text.to_string();
        if attachment_count > 0 {
            body.push_str(&format!(
                "\n（另有 {attachment_count} 个附件未同步到 IM频道）"
            ));
        }
        sink.send_reply(&target.bot_id, &target.target_id, None, &body)
            .await
    }

    /// 读配置并对齐循环：启用的微信 bot 启动，删除/停用的停止；配置指纹变化即重建。
    pub fn refresh(self: &Arc<Self>) -> Result<(), String> {
        if self.stopped.load(Ordering::SeqCst) {
            return Ok(());
        }
        let config = BotsRepo::new(&self.data_root).read_config()?;
        let desired: Vec<BotConfig> = config
            .bots
            .into_iter()
            .filter(|bot| bot.enabled && bot.provider == BotProvider::Weixin)
            .collect();
        let desired_ids: Vec<String> = desired.iter().map(|bot| bot.id.clone()).collect();
        let mut to_start = Vec::new();
        {
            let mut running = self
                .running
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let stale: Vec<String> = running
                .keys()
                .filter(|bot_id| !desired_ids.contains(bot_id))
                .cloned()
                .collect();
            for bot_id in stale {
                if let Some(entry) = running.remove(&bot_id) {
                    entry.signal.abort();
                    entry.task.abort();
                }
                self.statuses.remove(&bot_id);
            }
            for bot in desired {
                let fingerprint = config_fingerprint(&bot);
                match running.get(&bot.id) {
                    Some(entry) if entry.fingerprint == fingerprint && !entry.task.is_finished() => {}
                    Some(_) => {
                        if let Some(entry) = running.remove(&bot.id) {
                            entry.signal.abort();
                            entry.task.abort();
                        }
                        self.statuses.remove(&bot.id);
                        to_start.push(bot);
                    }
                    None => to_start.push(bot),
                }
            }
        }
        for bot in to_start {
            if self.stopped.load(Ordering::SeqCst) {
                break;
            }
            self.start_loop(bot);
        }
        self.refresh_wecom()?;
        self.refresh_dingtalk()?;
        self.refresh_adapters()?;
        Ok(())
    }

    /// 对齐企微长连接：启用且配置/secret 未变的保留，其余重启或停止。
    /// 对齐钉钉 Stream 连接：启用且配置/secret 未变的保留，其余重启或停止。
    fn refresh_dingtalk(self: &Arc<Self>) -> Result<(), String> {
        if self.stopped.load(Ordering::SeqCst) {
            return Ok(());
        }
        let config = BotsRepo::new(&self.data_root).read_config()?;
        // 适配器包里有钉钉适配器时优先走适配器（内置 Stream 实现回落）。
        let adapter_managed = crate::im_adapter::load_manifest()
            .ok()
            .is_some_and(|manifest| manifest.covers("dingtalk"));
        let desired: Vec<(BotConfig, Option<String>)> = config
            .bots
            .into_iter()
            .filter(|bot| bot.enabled && bot.provider == BotProvider::Dingtalk)
            .filter(|bot| {
                bot.dingtalk_client_id
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
            })
            .filter(|_bot| !adapter_managed)
            .map(|bot| {
                let secret = self.tokens.load(&bot.id);
                (bot, secret)
            })
            .collect();
        let desired_ids: Vec<String> = desired.iter().map(|(bot, _)| bot.id.clone()).collect();
        let mut to_start = Vec::new();
        {
            let mut running = self
                .running_dingtalk
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let stale: Vec<String> = running
                .keys()
                .filter(|bot_id| !desired_ids.contains(bot_id))
                .cloned()
                .collect();
            for bot_id in stale {
                if let Some(entry) = running.remove(&bot_id) {
                    entry.signal.abort();
                    entry.task.abort();
                }
                self.statuses.remove(&bot_id);
            }
            for (bot, secret) in desired {
                let fingerprint = dingtalk_fingerprint(&bot, secret.as_deref());
                match running.get(&bot.id) {
                    Some(entry) if entry.fingerprint == fingerprint && !entry.task.is_finished() => {}
                    Some(_) => {
                        if let Some(entry) = running.remove(&bot.id) {
                            entry.signal.abort();
                            entry.task.abort();
                        }
                        self.statuses.remove(&bot.id);
                        to_start.push((bot, secret));
                    }
                    None => to_start.push((bot, secret)),
                }
            }
        }
        for (bot, secret) in to_start {
            if self.stopped.load(Ordering::SeqCst) {
                break;
            }
            self.start_dingtalk_loop(bot, secret);
        }
        Ok(())
    }

    /// 对齐外挂适配器：manifest 覆盖的 provider 的启用 bot 全部走适配器进程。
    fn refresh_adapters(self: &Arc<Self>) -> Result<(), String> {
        if self.stopped.load(Ordering::SeqCst) {
            return Ok(());
        }
        let manifest = match crate::im_adapter::load_manifest() {
            Ok(manifest) => manifest,
            Err(error) => {
                im_log!("[im] 适配器清单不可用：{error}");
                return Ok(());
            }
        };
        let config = BotsRepo::new(&self.data_root).read_config()?;
        let desired: Vec<(BotConfig, Option<String>)> = config
            .bots
            .into_iter()
            .filter(|bot| bot.enabled)
            .filter(|bot| {
                manifest.covers(bot.provider.as_str())
                    && matches!(bot.provider, BotProvider::Wecom | BotProvider::Dingtalk | BotProvider::Feishu | BotProvider::Lark)
            })
            .map(|bot| {
                let secret = self.tokens.load(&bot.id);
                (bot, secret)
            })
            .collect();
        let desired_ids: Vec<String> = desired.iter().map(|(bot, _)| bot.id.clone()).collect();
        let mut to_start = Vec::new();
        {
            let mut running = self
                .running_adapter
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let stale: Vec<String> = running
                .keys()
                .filter(|bot_id| !desired_ids.contains(bot_id))
                .cloned()
                .collect();
            for bot_id in stale {
                if let Some(entry) = running.remove(&bot_id)
                    && let Some(process) = entry.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()
                {
                    tokio::spawn(async move { process.shutdown().await });
                }
                self.statuses.remove(&bot_id);
            }
            for (bot, secret) in desired {
                let fingerprint = adapter_fingerprint(&bot, secret.as_deref());
                match running.get(&bot.id) {
                    Some(entry) if entry.fingerprint == fingerprint => {}
                    Some(_) => {
                        if let Some(entry) = running.remove(&bot.id)
                            && let Some(process) = entry.shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()
                        {
                            tokio::spawn(async move { process.shutdown().await });
                        }
                        self.statuses.remove(&bot.id);
                        to_start.push((bot, secret));
                    }
                    None => to_start.push((bot, secret)),
                }
            }
        }
        for (bot, secret) in to_start {
            if self.stopped.load(Ordering::SeqCst) {
                break;
            }
            self.start_adapter_loop(&manifest, bot, secret);
        }
        Ok(())
    }

    fn start_adapter_loop(
        self: &Arc<Self>,
        manifest: &crate::im_adapter::AdapterManifest,
        bot: BotConfig,
        secret: Option<String>,
    ) {
        let bot_id = bot.id.clone();
        let Some(entry) = manifest.entry_for(bot.provider.as_str()) else {
            return;
        };
        let fingerprint = adapter_fingerprint(&bot, secret.as_deref());
        let lock_root = self.data_root.join("v2").join("bots-runtime-locks");
        let lock = match acquire_bot_runtime_lock(&lock_root, "adapter", &bot_id) {
            Ok(Some(lock)) => Some(lock),
            Ok(None) => {
                self.set_wecom_status(&bot_id, "handled_elsewhere", Some("该 Bot 由另一个 Mode 窗口的适配器处理".to_string()));
                return;
            }
            Err(error) => {
                im_log!("[im] 适配器运行时锁失败 bot={bot_id}: {error}");
                None
            }
        };
        self.set_wecom_status(&bot_id, "connecting", None);
        let provider = bot.provider.as_str().to_string();
        let provider_label: &'static str = Box::leak(provider.clone().into_boxed_str());
        let config =
            crate::im_adapter::AdapterCredentialSource.config_for(&provider, &bot, secret);
        let statuses = self.statuses.clone();
        let _log_bot_id = bot_id.clone();
        let _data_root = self.data_root.clone();
        let ops: Arc<dyn BotTurnOps> = turn_ops(&self.data_root);
        let shared: Arc<std::sync::Mutex<Option<crate::im_adapter::AdapterProcess>>> =
            Arc::new(std::sync::Mutex::new(None));
        let (reply_tx, mut reply_rx) =
            tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
        let sink: Arc<dyn BotReplySink> = Arc::new(AdapterReplySink {
            reply_tx,
            supports_media: provider == "wecom",
        });
        let inner: Arc<dyn WeixinMessageHandler> = Arc::new(
            ImWeixinHandler::new(
                self.data_root.clone(),
                bot,
                Arc::clone(&self.bind_codes),
                ops,
                Arc::clone(&sink),
                Arc::clone(&self.http) as Arc<dyn HttpJsonPort>,
            )
            .with_provider_label(provider_label),
        );
        let shared_for_spawn = Arc::clone(&shared);
        let handler = Arc::clone(&inner);
        let message_bot_id = bot_id.clone();
        let status_bot_id = bot_id.clone();
        let log_bot_id = bot_id.clone();
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::try_current().ok().map(|handle| {
                handle.block_on(crate::im_adapter::spawn_adapter(
                    &entry,
                    &bot_id,
                    config,
                    move |value| {
                        let message = weixin_inbound_from_adapter(&value, &message_bot_id);
                        if let Some(message) = message {
                            let handler = Arc::clone(&handler);
                            tokio::spawn(async move {
                                if let Err(error) = handler.handle(&message.bot_id, &message).await {
                                    im_log!("[im] 适配器回合失败 bot={}: {error}", message.bot_id);
                                }
                            });
                        }
                    },
                    move |status, detail| {
                        set_adapter_status(&statuses, &status_bot_id, provider_label, status, detail);
                    },
                    move |line| {
                        im_log!("[im] 适配器 bot={log_bot_id}: {line}");
                    },
                ))
            })
        });
        let process = match result {
            Some(Ok(process)) => process,
            Some(Err(error)) => {
                self.set_wecom_status(&bot_id, "error", Some(error));
                return;
            }
            None => {
                self.set_wecom_status(&bot_id, "error", Some("没有可用的 tokio 运行时".to_string()));
                return;
            }
        };
        // 回复转发：sink → 子进程 stdin（spawn 前积压的也会按序送达）。
        let reply_stdin = Arc::clone(&process.stdin);
        tokio::spawn(async move {
            while let Some(op) = reply_rx.recv().await {
                if AdapterReplySink::write_reply(&reply_stdin, op).await.is_err() {
                    break;
                }
            }
        });
        *shared_for_spawn.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(process);
        let mut running = self
            .running_adapter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        running.insert(
            bot_id,
            RunningAdapter {
                shared,
                fingerprint,
                _lock: lock,
            },
        );
    }

    fn start_dingtalk_loop(self: &Arc<Self>, bot: BotConfig, secret: Option<String>) {
        let bot_id = bot.id.clone();
        let Some(client_id) = bot
            .dingtalk_client_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
        else {
            self.set_wecom_status(&bot_id, "error", Some("钉钉 Client ID 未配置".to_string()));
            return;
        };
        let Some(client_secret) = secret.filter(|value| !value.trim().is_empty()) else {
            self.set_wecom_status(&bot_id, "error", Some("钉钉 Client Secret 未配置".to_string()));
            return;
        };
        let fingerprint = dingtalk_fingerprint(&bot, Some(&client_secret));
        let lock_root = self.data_root.join("v2").join("bots-runtime-locks");
        let lock = match acquire_bot_runtime_lock(&lock_root, "dingtalk-stream", &client_id) {
            Ok(Some(lock)) => Some(lock),
            Ok(None) => {
                self.set_wecom_status(
                    &bot_id,
                    "handled_elsewhere",
                    Some("钉钉连接由另一个 Mode 窗口处理".to_string()),
                );
                return;
            }
            Err(error) => {
                im_log!("[im] 钉钉运行时锁失败 bot={bot_id}: {error}");
                None
            }
        };
        self.set_wecom_status(&bot_id, "connecting", None);
        let signal = AbortSignal::new();
        let statuses = self.statuses.clone();
        let log_bot_id = bot.id.clone();
        let handler_bot = bot.clone();
        let http = Arc::clone(&self.http) as Arc<dyn HttpJsonPort>;
        let ops: Arc<dyn BotTurnOps> = turn_ops(&self.data_root);
        let sink: Arc<dyn BotReplySink> = Arc::new(DingtalkReplySink::new());
        let inner: Arc<dyn WeixinMessageHandler> = Arc::new(
            ImWeixinHandler::new(
                self.data_root.clone(),
                handler_bot,
                Arc::clone(&self.bind_codes),
                ops,
                Arc::clone(&sink),
                Arc::clone(&self.http) as Arc<dyn HttpJsonPort>,
            )
            .with_provider_label("dingtalk"),
        );
        let loop_signal = signal.clone();
        let task = tokio::spawn(async move {
            dingtalk_stream_loop(
                log_bot_id,
                client_id,
                client_secret,
                http,
                inner,
                statuses,
                loop_signal,
            )
            .await;
        });
        let mut running = self
            .running_dingtalk
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        running.insert(
            bot.id.clone(),
            RunningDingtalk {
                signal,
                task,
                fingerprint,
                _lock: lock,
            },
        );
    }

    fn refresh_wecom(self: &Arc<Self>) -> Result<(), String> {
        if self.stopped.load(Ordering::SeqCst) {
            return Ok(());
        }
        let config = BotsRepo::new(&self.data_root).read_config()?;
        // 适配器包里有 wecom 适配器时，企微全部走适配器（内置协议只作回落）。
        let adapter_managed = crate::im_adapter::load_manifest()
            .ok()
            .is_some_and(|manifest| manifest.covers("wecom"));
        let desired: Vec<BotConfig> = config
            .bots
            .into_iter()
            .filter(|bot| bot.enabled && bot.provider == BotProvider::Wecom)
            .filter(|_bot| !adapter_managed)
            .collect();
        let desired_ids: Vec<String> = desired.iter().map(|bot| bot.id.clone()).collect();
        let mut to_start = Vec::new();
        {
            let mut running = self
                .running_wecom
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let stale: Vec<String> = running
                .keys()
                .filter(|bot_id| !desired_ids.contains(bot_id))
                .cloned()
                .collect();
            for bot_id in stale {
                if let Some(entry) = running.remove(&bot_id) {
                    entry.signal.abort();
                    entry.task.abort();
                }
                self.statuses.remove(&bot_id);
            }
            for bot in desired {
                let secret = self.tokens.load(&bot.id);
                let fingerprint = wecom_fingerprint(&bot, secret.as_deref());
                match running.get(&bot.id) {
                    Some(entry) if entry.fingerprint == fingerprint && !entry.task.is_finished() => {}
                    Some(_) => {
                        if let Some(entry) = running.remove(&bot.id) {
                            entry.signal.abort();
                            entry.task.abort();
                        }
                        self.statuses.remove(&bot.id);
                        to_start.push((bot, secret));
                    }
                    None => to_start.push((bot, secret)),
                }
            }
        }
        for (bot, secret) in to_start {
            if self.stopped.load(Ordering::SeqCst) {
                break;
            }
            self.start_wecom_loop(bot, secret);
        }
        Ok(())
    }

    fn start_wecom_loop(self: &Arc<Self>, bot: BotConfig, secret: Option<String>) {
        let bot_id = bot.id.clone();
        let Some(wecom_bot_id) = bot
            .wecom_bot_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
        else {
            self.set_wecom_status(&bot_id, "error", Some("企微 Bot ID 未配置".to_string()));
            return;
        };
        let Some(secret) = secret.filter(|value| !value.trim().is_empty()) else {
            self.set_wecom_status(&bot_id, "error", Some("企微机器人 secret 未配置".to_string()));
            return;
        };
        let fingerprint = wecom_fingerprint(&bot, Some(&secret));
        let lock_root = self.data_root.join("v2").join("bots-runtime-locks");
        let lock = match acquire_bot_runtime_lock(&lock_root, "wecom-websocket", &wecom_bot_id) {
            Ok(Some(lock)) => Some(lock),
            Ok(None) => {
                self.set_wecom_status(
                    &bot_id,
                    "handled_elsewhere",
                    Some("企微连接由另一个 Mode 窗口处理".to_string()),
                );
                return;
            }
            Err(error) => {
                im_log!("[im] 企微运行时锁失败 bot={bot_id}: {error}");
                None
            }
        };
        self.set_wecom_status(&bot_id, "connecting", None);
        let signal = AbortSignal::new();
        let (outbound, inbound) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let streams: WeComStreams = Arc::new(Mutex::new(HashMap::new()));
        let broker = WeComRequestBroker::new();
        let sink: Arc<dyn BotReplySink> = Arc::new(WeComReplySink::new(
            &bot.id,
            outbound.clone(),
            Arc::clone(&streams),
            broker.clone(),
        ));
        let ops: Arc<dyn BotTurnOps> = turn_ops(&self.data_root);
        let inner: Arc<dyn WeixinMessageHandler> = Arc::new(
            ImWeixinHandler::new(
                self.data_root.clone(),
                bot.clone(),
                Arc::clone(&self.bind_codes),
                ops,
                Arc::clone(&sink),
                Arc::clone(&self.http) as Arc<dyn HttpJsonPort>,
            )
            .with_provider_label("wecom"),
        );
        let handler = Arc::new(WeComInboundHandler::new(
            &bot.id,
            inner,
            outbound.clone(),
            streams,
        ));
        let statuses = self.statuses.clone();
        let log_bot_id = bot.id.clone();
        let data_root = self.data_root.clone();
        let broker_for_ack = broker.clone();
        let dedupe = Arc::new(Mutex::new(WeComDedupe::from_entries(load_wecom_dedupe(
            &data_root, &bot.id,
        ))));
        let dedupe_root = data_root.clone();
        let event_sink: EventSink = Arc::new(move |event| match event {
            WeComWsEvent::Connected => {
                set_wecom_status(&statuses, &log_bot_id, "connecting", None);
            }
            WeComWsEvent::Authenticated => {
                im_log!("[im] 企微 WS 已认证 bot={log_bot_id}");
                set_wecom_status(&statuses, &log_bot_id, "connected", None);
            }
            WeComWsEvent::Disconnected { reason } => {
                set_wecom_status(&statuses, &log_bot_id, "reconnecting", Some(reason));
            }
            WeComWsEvent::ServerDisconnect { reason } => {
                set_wecom_status(&statuses, &log_bot_id, "kicked", Some(reason));
            }
            WeComWsEvent::AuthFailed { errcode, errmsg } => {
                set_wecom_status(
                    &statuses,
                    &log_bot_id,
                    "error",
                    Some(format!("认证失败 {errcode}: {errmsg}")),
                );
            }
            WeComWsEvent::Message { req_id, body } => {
                if let Some(message_id) = inbound_message_id(&body) {
                    let snapshot = {
                        let mut guard = dedupe
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if !guard.mark(&message_id, now_ms()) {
                            return;
                        }
                        guard.snapshot()
                    };
                    persist_wecom_dedupe(&dedupe_root, &log_bot_id, snapshot);
                }
                let handler = Arc::clone(&handler);
                let log_bot_id = log_bot_id.clone();
                tokio::spawn(async move {
                    if let Err(error) = handler.handle_body(&req_id, &body).await {
                        im_log!("[im] 企微回合失败 bot={log_bot_id}: {error}");
                    }
                });
            }
            WeComWsEvent::ReplyAck {
                req_id,
                errcode,
                errmsg,
                body,
            } => {
                if errcode != 0 {
                    im_log!(
                        "[im] 企微回复回执错误 bot={log_bot_id} req={req_id} errcode={errcode}: {errmsg}"
                    );
                }
                broker_for_ack.resolve(
                    &req_id,
                    WeComAck {
                        errcode,
                        errmsg,
                        body,
                    },
                );
            }
            WeComWsEvent::Error { message } => {
                im_log!("[im] 企微 WS 错误 bot={log_bot_id}: {message}");
            }
            WeComWsEvent::Event { .. } => {}
        });
        let ws = run_wecom_ws(
            WeComWsConfig::new(&wecom_bot_id, &secret),
            inbound,
            event_sink,
            Arc::new(|_| false),
            signal.clone(),
        );
        let task = tokio::spawn(async move {
            ws.await;
        });
        let mut running = self
            .running_wecom
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.stopped.load(Ordering::SeqCst) || running.contains_key(&bot_id) {
            signal.abort();
            task.abort();
            return;
        }
        running.insert(
            bot_id,
            RunningWecom {
                signal,
                task,
                fingerprint,
                _lock: lock,
                outbound,
            },
        );
    }

    /// 60s 心跳巡检：到点、有私聊目标、任务不在跑，就以 quiet 回合跑一次。
    pub fn start_heartbeat_scheduler(self: &Arc<Self>) {
        if self.stopped.load(Ordering::SeqCst) {
            return;
        }
        let mut slot = self
            .heartbeat_task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_some() {
            return;
        }
        let manager = Arc::clone(self);
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(60));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if manager.stopped.load(Ordering::SeqCst) {
                    break;
                }
                manager.tick_heartbeats().await;
            }
        });
        *slot = Some(handle);
    }

    async fn tick_heartbeats(&self) {
        let repo = BotsRepo::new(&self.data_root);
        let config = match repo.read_config() {
            Ok(config) => config,
            Err(error) => {
                im_log!("[im] 心跳读取配置失败：{error}");
                return;
            }
        };
        let state = match repo.read_state() {
            Ok(state) => state,
            Err(error) => {
                im_log!("[im] 心跳读取状态失败：{error}");
                return;
            }
        };
        let now = crate::im_heartbeat::now_ms();
        let locale = heartbeat_locale(
            crate::im_heartbeat::read_settings_locale(&self.data_root).as_deref(),
        );
        for bot in config.bots.into_iter().filter(|bot| {
            bot.enabled && matches!(bot.provider, BotProvider::Weixin | BotProvider::Wecom)
        }) {
            let Some(plan) = crate::im_heartbeat::plan_heartbeat(
                &bot.id,
                bot.heartbeat.as_ref(),
                &state,
                now,
                self.is_running(&bot.id),
            ) else {
                continue;
            };
            let conversation = plan.conversation;

            if let Err(error) = crate::im_heartbeat::record_heartbeat(&repo, &bot.id, now) {
                im_log!("[im] 心跳时间戳写入失败 bot={}: {error}", bot.id);
            }
            let instructions =
                crate::im_heartbeat::read_heartbeat_instructions(&conversation.workspace_path);
            let prompt = build_heartbeat_prompt(locale, instructions.as_deref());
            let sink: Arc<dyn BotReplySink> = if bot.provider == BotProvider::Wecom {
                let outbound = self
                    .running_wecom
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get(&bot.id)
                    .map(|entry| entry.outbound.clone());
                let Some(outbound) = outbound else {
                    continue;
                };
                Arc::new(WeComReplySink::new(
                    &bot.id,
                    outbound,
                    Arc::new(Mutex::new(HashMap::new())),
                    WeComRequestBroker::new(),
                ))
            } else {
                let from_user_id = bot
                    .provider_self_id
                    .clone()
                    .or_else(|| bot.provider_user_id.clone())
                    .unwrap_or_default();
                Arc::new(ImReplySink::new(
                    Arc::clone(&self.http),
                    self.tokens.loader(),
                    &bot.id,
                    &from_user_id,
                ))
            };
            let ops: Arc<dyn BotTurnOps> = turn_ops(&self.data_root);
            let store = ConversationStore::new(BotsRepo::new(&self.data_root));
            let runner = ConversationRunner::new(&store, &*ops, &*sink)
                .with_polling(Duration::from_secs(1), 240);
            let turn = InboundTurn {
                bot_id: bot.id.clone(),
                conversation_key: conversation.conversation_key,
                conversation_kind: Some(conversation.kind.clone()),
                text: prompt,
                provider_context_token: None,
                default_workspace: conversation.workspace_path,
                default_workspace_identity: conversation.workspace_identity,
                quiet: true,
            };
            match runner.handle(turn).await {
                Ok(outcome) => im_log!(
                    "[im] 心跳完成 bot={} suppressed={} polls={} reply_len={}",
                    bot.id,
                    outcome.suppressed,
                    outcome.polls,
                    outcome.reply.chars().count()
                ),
                Err(error) => im_log!("[im] 心跳失败 bot={}: {error}", bot.id),
            }
        }
    }

    fn set_wecom_status(&self, bot_id: &str, status: &'static str, message: Option<String>) {
        set_wecom_status(&self.statuses, bot_id, status, message);
    }

    fn start_loop(self: &Arc<Self>, bot: BotConfig) {
        let fingerprint = config_fingerprint(&bot);
        let signal = AbortSignal::new();
        let bot_id = bot.id.clone();
        let lock_root = self.data_root.join("v2").join("bots-runtime-locks");
        let lock = match self.tokens.load(&bot.id).filter(|token| !token.trim().is_empty()) {
            Some(token) => match acquire_bot_runtime_lock(&lock_root, "weixin-polling", &token) {
                Ok(Some(lock)) => Some(lock),
                Ok(None) => {
                    self.statuses.set(WeixinRuntimeStatus::new(
                        &bot_id,
                        "handled_elsewhere",
                        Some("微信长轮询由另一个 Mode 窗口处理".to_string()),
                    ));
                    return;
                }
                Err(error) => {
                    im_log!("[im] 微信运行时锁失败 bot={bot_id}: {error}");
                    None
                }
            },
            None => None,
        };
        let from_user_id = bot
            .provider_self_id
            .clone()
            .or_else(|| bot.provider_user_id.clone())
            .unwrap_or_default();
        let sink: Arc<dyn BotReplySink> = Arc::new(ImReplySink::new(
            Arc::clone(&self.http),
            self.tokens.loader(),
            &bot.id,
            &from_user_id,
        ));
        let ops: Arc<dyn BotTurnOps> = turn_ops(&self.data_root);
        let handler: Arc<dyn WeixinMessageHandler> = Arc::new(ImWeixinHandler::new(
            self.data_root.clone(),
            bot.clone(),
            Arc::clone(&self.bind_codes),
            ops,
            Arc::clone(&sink),
            Arc::clone(&self.http) as Arc<dyn HttpJsonPort>,
        ));
        let repo = BotsRepo::new(&self.data_root);
        let buf = repo
            .read_state()
            .ok()
            .and_then(|state| {
                state
                    .get("bots")
                    .and_then(|bots| bots.get(&bot_id))
                    .and_then(|entry| entry.get("weixinGetUpdatesBuf"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let host = Arc::new(WeixinBotHost::new(
            repo,
            Arc::new(MapTokenSource {
                tokens: Arc::clone(&self.tokens),
            }),
            handler,
            self.statuses.clone(),
        ));
        let mut poll = WeixinPollLoop::new(
            bot_id.clone(),
            Arc::clone(&self.http) as Arc<dyn HttpJsonPort>,
            host,
            signal.clone(),
            buf,
        );
        let task = tokio::spawn(async move {
            poll.run().await;
        });
        let mut running = self
            .running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.stopped.load(Ordering::SeqCst) || running.contains_key(&bot_id) {
            signal.abort();
            task.abort();
            return;
        }
        running.insert(
            bot_id,
            RunningLoop {
                signal,
                task,
                fingerprint,
                _lock: lock,
            },
        );
    }
}

struct MapTokenSource {
    tokens: Arc<TokenVault>,
}

impl WeixinCredentialSource for MapTokenSource {
    fn load_token<'a>(&'a self, bot_id: &'a str) -> RuntimeFuture<'a, Option<String>> {
        let token = self.tokens.load(bot_id);
        Box::pin(async move { token })
    }
}

/// 微信回复投递：纯文本降级 + `/sendmessage`。
pub struct ImReplySink {
    http: Arc<ReqwestHttpPort>,
    tokens: TokenLoader,
    from_user_id: String,
    bot_id: String,
}

impl ImReplySink {
    pub fn new(
        http: Arc<ReqwestHttpPort>,
        tokens: TokenLoader,
        bot_id: &str,
        from_user_id: &str,
    ) -> Self {
        Self {
            http,
            tokens,
            from_user_id: from_user_id.to_string(),
            bot_id: bot_id.to_string(),
        }
    }
}

fn plain_weixin_text(text: &str) -> String {
    bots_service::bot_text::to_plain_text_reply(text)
        .replace("\r\n", "\n")
        .replace('\n', "\r\n")
}

impl BotReplySink for ImReplySink {
    fn send_reply<'a>(
        &'a self,
        _bot_id: &'a str,
        conversation_key: &'a str,
        context_token: Option<&'a str>,
        text: &'a str,
    ) -> RuntimeFuture<'a, Result<(), String>> {
        let http = Arc::clone(&self.http);
        let token = (self.tokens)(&self.bot_id);
        let from_user_id = self.from_user_id.clone();
        let to_user_id = conversation_key.to_string();
        let text = plain_weixin_text(text);
        let context_token = context_token.map(str::to_string);
        Box::pin(async move {
            let Some(token) = token.filter(|token| !token.trim().is_empty()) else {
                return Err("微信 bot token 缺失".to_string());
            };
            let client = WeixinClient::new(http);
            client
                .send_text(&token, &from_user_id, &to_user_id, &text, context_token.as_deref())
                .await
                .map(|_| ())
                .map_err(|error| error.message)
        })
    }
}

fn wecom_fingerprint(bot: &BotConfig, secret: Option<&str>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    if let Ok(bytes) = serde_json::to_vec(bot) {
        feed(&bytes);
    }
    feed(secret.unwrap_or_default().as_bytes());
    hash
}

/// 钉钉指纹：配置或 secret 变了就重启连接。
fn dingtalk_fingerprint(bot: &BotConfig, secret: Option<&str>) -> u64 {
    let mut hash = config_fingerprint(bot);
    hash = hash
        .wrapping_mul(1_099_511_628_211)
        .wrapping_add(secret.unwrap_or_default().len() as u64);
    hash
}

/// 适配器指纹：配置或凭据变化即重启适配器进程。
fn adapter_fingerprint(bot: &BotConfig, secret: Option<&str>) -> u64 {
    let mut hash = config_fingerprint(bot);
    hash = hash
        .wrapping_mul(1_099_511_628_211)
        .wrapping_add(secret.unwrap_or_default().len() as u64);
    hash
}

/// 适配器状态条：provider 标回原通道名（UI 不需要知道背后是适配器）。
fn set_adapter_status(
    statuses: &WeixinStatusRegistry,
    bot_id: &str,
    provider_label: &'static str,
    status: &str,
    detail: Option<String>,
) {
    // 状态字面量来自适配器 JSON，映射回 &'static 白名单（new 的签名要求）。
    let status_static: &'static str = match status {
        "connecting" => "connecting",
        "connected" => "connected",
        "error" => "error",
        "handled_elsewhere" => "handled_elsewhere",
        _ => "reconnecting",
    };
    let mut entry = WeixinRuntimeStatus::new(bot_id, status_static, detail);
    entry.provider = provider_label;
    entry.last_update_at = Some(now_ms());
    statuses.set(entry);
}

/// 适配器 message 事件 → 统一入站模型（contextToken 原样透传）。
fn weixin_inbound_from_adapter(value: &Value, bot_id: &str) -> Option<WeixinInboundMessage> {
    let text = value.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
    if text.is_empty() {
        return None;
    }
    let sender_id = value
        .get("senderId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if sender_id.is_empty() {
        return None;
    }
    Some(WeixinInboundMessage {
        bot_id: bot_id.to_string(),
        text,
        attachments: Vec::new(),
        actor: WeixinInboundActor {
            provider_user_id: sender_id,
            display_name: value
                .get("senderNick")
                .and_then(Value::as_str)
                .map(str::to_string),
            chat_type: value
                .get("chatType")
                .and_then(Value::as_str)
                .unwrap_or("private")
                .to_string(),
            chat_id: value
                .get("chatId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            provider_message_id: value
                .get("messageId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            provider_context_token: value
                .get("contextToken")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
    })
}

/// 适配器回复链：`op.reply` 写通道，由转发任务落子进程 stdin
/// （spawn 前就要构造 sink，所以走 channel 解开循环依赖）。
struct AdapterReplySink {
    reply_tx: tokio::sync::mpsc::UnboundedSender<serde_json::Value>,
    /// 钉钉/飞书的 webhook/OpenAPI 回复链只支持文本；企微适配器支持媒体。
    supports_media: bool,
}

impl AdapterReplySink {
    async fn write_reply(
        stdin: &crate::im_adapter::AdapterShared,
        op: serde_json::Value,
    ) -> Result<(), String> {
        let mut writer = stdin.lock().await;
        writer
            .write_all(format!("{op}\n").as_bytes())
            .await
            .map_err(|error| format!("写适配器回复失败：{error}"))?;
        writer.flush().await.map_err(|error| format!("{error}"))
    }
}

impl BotReplySink for AdapterReplySink {
    fn send_reply<'a>(
        &'a self,
        _bot_id: &'a str,
        conversation_key: &'a str,
        context_token: Option<&'a str>,
        text: &'a str,
    ) -> bots_service::conversation::RuntimeFuture<'a, Result<(), String>> {
        let reply_tx = self.reply_tx.clone();
        Box::pin(async move {
            reply_tx
                .send(serde_json::json!({
                    "op": "reply",
                    "context_token": context_token.unwrap_or_default(),
                    "text": text,
                    "media_path": null,
                    "caption": null,
                    "chat_id": conversation_key,
                }))
                .map_err(|_| "适配器连接已关闭，回复未发送".to_string())
        })
    }

    fn send_media<'a>(
        &'a self,
        _bot_id: &'a str,
        conversation_key: &'a str,
        context_token: Option<&'a str>,
        media: bots_service::conversation::OutboundMedia,
    ) -> bots_service::conversation::RuntimeFuture<'a, Result<(), String>> {
        let reply_tx = self.reply_tx.clone();
        Box::pin(async move {
            if !self.supports_media {
                return Err("该通道适配器暂不支持发送文件".to_string());
            }
            // bytes 落临时文件，适配器按 path 读（进程间不传大二进制）。
            let outbox = crate::data::data_root().join("v2").join("im-adapters");
            let _ = std::fs::create_dir_all(&outbox);
            let path = outbox.join(format!(
                "media-{}-{}",
                now_ms(),
                media.filename.replace(['/', '\\', ':'], "_")
            ));
            std::fs::write(&path, &media.bytes)
                .map_err(|error| format!("写临时媒体文件失败：{error}"))?;
            let result = reply_tx
                .send(serde_json::json!({
                    "op": "reply",
                    "context_token": context_token.unwrap_or_default(),
                    "text": "",
                    "media_path": path.to_string_lossy(),
                    "caption": media.caption,
                    "chat_id": conversation_key,
                }))
                .map_err(|_| "适配器连接已关闭，回复未发送".to_string());
            let _ = std::fs::remove_file(&path);
            result
        })
    }
}

/// 钉钉状态条（与企微同一张注册表，provider 标 dingtalk 供 UI 区分）。
fn set_dingtalk_status(
    statuses: &WeixinStatusRegistry,
    bot_id: &str,
    status: &'static str,
    message: Option<String>,
) {
    let mut entry = WeixinRuntimeStatus::new(bot_id, status, message);
    entry.provider = "dingtalk";
    entry.last_update_at = Some(now_ms());
    statuses.set(entry);
}

/// 钉钉回复链：sessionWebhook 直发（入站时把 webhook 存进 context_token）。
struct DingtalkReplySink;

impl DingtalkReplySink {
    fn new() -> Self {
        Self
    }
}

impl BotReplySink for DingtalkReplySink {
    fn send_reply<'a>(
        &'a self,
        _bot_id: &'a str,
        _conversation_key: &'a str,
        context_token: Option<&'a str>,
        text: &'a str,
    ) -> bots_service::conversation::RuntimeFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let Some(webhook) = context_token.filter(|value| !value.is_empty()) else {
                return Err("没有回复通道（sessionWebhook 缺失）".to_string());
            };
            let request = bots_service::http::OutboundHttpRequest {
                url: webhook.to_string(),
                headers: vec![("Content-Type".to_string(), "application/json".to_string())],
                body: dingtalk::webhook_reply_body(text).to_string(),
                timeout: std::time::Duration::from_secs(10),
            };
            let port = ReqwestHttpPort::default();
            let response = port.post_json(&request).await?;
            if response.status >= 400 {
                return Err(format!("钉钉 webhook 回复失败（HTTP {}）", response.status));
            }
            Ok(())
        })
    }
}

/// gateway/connections/open：拿 WS endpoint 与注册 ticket。
async fn dingtalk_gateway_open(
    http: &dyn HttpJsonPort,
    client_id: &str,
    client_secret: &str,
) -> Result<(String, String), String> {
    let request = bots_service::http::OutboundHttpRequest {
        url: dingtalk::GATEWAY_OPEN_URL.to_string(),
        headers: vec![("Content-Type".to_string(), "application/json".to_string())],
        body: dingtalk::gateway_open_body(client_id, client_secret).to_string(),
        timeout: std::time::Duration::from_secs(10),
    };
    let response = http.post_json(&request).await?;
    let endpoint = response
        .payload
        .get("endpoint")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let ticket = response
        .payload
        .get("ticket")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if endpoint.is_empty() || ticket.is_empty() {
        return Err(format!(
            "钉钉 gateway 响应缺 endpoint/ticket（HTTP {}）",
            response.status
        ));
    }
    Ok((endpoint, ticket))
}

/// 钉钉 Stream 长连接：断线退避重连，EVENT → 统一入站 handler。
async fn dingtalk_stream_loop(
    bot_id: String,
    client_id: String,
    client_secret: String,
    http: Arc<dyn HttpJsonPort>,
    handler: Arc<dyn WeixinMessageHandler>,
    statuses: WeixinStatusRegistry,
    signal: AbortSignal,
) {
    let ping_interval = std::time::Duration::from_millis(dingtalk::PING_INTERVAL_MS);
    let mut dedupe: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        if signal.is_aborted() {
            return;
        }
        set_dingtalk_status(&statuses, &bot_id, "connecting", None);
        let (endpoint, ticket) =
            match dingtalk_gateway_open(http.as_ref(), &client_id, &client_secret).await {
                Ok(pair) => pair,
                Err(error) => {
                    set_dingtalk_status(&statuses, &bot_id, "reconnecting", Some(error));
                    signal.wait_for(5_000).await;
                    continue;
                }
            };
        let ws = match tokio_tungstenite::connect_async(&endpoint).await {
            Ok((ws, _)) => ws,
            Err(error) => {
                set_dingtalk_status(
                    &statuses,
                    &bot_id,
                    "reconnecting",
                    Some(format!("WS 连接失败：{error}")),
                );
                signal.wait_for(5_000).await;
                continue;
            }
        };
        let (mut writer, mut reader) = ws.split();
        let register = dingtalk::register_frame(&ticket, &client_id, &req_id());
        if let Err(error) = writer.send(Message::Text(register.to_string())).await {
            set_dingtalk_status(
                &statuses,
                &bot_id,
                "reconnecting",
                Some(format!("注册帧发送失败：{error}")),
            );
            signal.wait_for(3_000).await;
            continue;
        }
        // 等 REGISTER:ACK（最长 10s）。
        let mut registered = false;
        for _ in 0..3 {
            let frame = match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                reader.next(),
            )
            .await
            {
                Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<Value>(&text).ok(),
                _ => None,
            };
            if matches!(frame.as_ref().map(dingtalk::frame_type), Some("REGISTER:ACK")) {
                registered = true;
                break;
            }
        }
        if !registered {
            set_dingtalk_status(
                &statuses,
                &bot_id,
                "reconnecting",
                Some("REGISTER:ACK 超时".to_string()),
            );
            signal.wait_for(3_000).await;
            continue;
        }
        im_log!("[im] 钉钉 Stream 已注册 bot={bot_id}");
        set_dingtalk_status(&statuses, &bot_id, "connected", None);
        // 消息循环：窗口内没有下行就主动 PING。
        loop {
            if signal.is_aborted() {
                return;
            }
            let frame = match tokio::time::timeout(ping_interval, reader.next()).await {
                Err(_elapsed) => {
                    let _ = writer
                        .send(Message::Text(dingtalk::ping_frame(&req_id()).to_string()))
                        .await;
                    continue;
                }
                Ok(None) => break,
                Ok(Some(Err(error))) => {
                    set_dingtalk_status(
                        &statuses,
                        &bot_id,
                        "reconnecting",
                        Some(format!("WS 断开：{error}")),
                    );
                    break;
                }
                Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<Value>(&text).ok(),
                Ok(Some(Ok(_))) => continue,
            };
            let Some(frame) = frame else { continue };
            match dingtalk::frame_type(&frame) {
                "PING" => {
                    let _ = writer
                        .send(Message::Text(
                            serde_json::json!({
                                "frameType": "system",
                                "type": "PONG",
                                "contentType": "application/json"
                            })
                            .to_string(),
                        ))
                        .await;
                }
                "PING:ACK" | "REGISTER:ACK" => {}
                "EVENT" => {
                    let req_id_value = frame
                        .get("reqId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let _ = writer
                        .send(Message::Text(
                            dingtalk::event_ack_frame(&req_id_value).to_string(),
                        ))
                        .await;
                    if let Some(event) = dingtalk::parse_message_event(&frame) {
                        if !event.message_id.is_empty()
                            && !dedupe.insert(event.message_id.clone())
                        {
                            continue;
                        }
                        let chat_type = if event.conversation_type == "1" {
                            "private"
                        } else {
                            "group"
                        };
                        let message = WeixinInboundMessage {
                            bot_id: bot_id.clone(),
                            text: event.text,
                            attachments: Vec::new(),
                            actor: WeixinInboundActor {
                                provider_user_id: event.sender_id,
                                display_name: event.sender_nick,
                                chat_type: chat_type.to_string(),
                                chat_id: Some(event.conversation_id),
                                provider_message_id: Some(event.message_id),
                                provider_context_token: Some(event.session_webhook),
                            },
                        };
                        if let Err(error) = handler.handle(&bot_id, &message).await {
                            im_log!("[im] 钉钉回合失败 bot={bot_id}: {error}");
                        }
                    }
                }
                _ => {}
            }
        }
        if signal.is_aborted() {
            return;
        }
        signal.wait_for(3_000).await;
    }
}

/// 轻量 reqId（时间戳 + 计数，无需 uuid 依赖）。
fn req_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!(
        "req-{}-{}",
        now_ms(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

fn set_wecom_status(
    statuses: &WeixinStatusRegistry,
    bot_id: &str,
    status: &'static str,
    message: Option<String>,
) {
    let mut entry = WeixinRuntimeStatus::new(bot_id, status, message);
    entry.provider = "wecom";
    entry.last_update_at = Some(now_ms());
    statuses.set(entry);
}

const WECOM_DEDUPE_STATE_KEY: &str = "wecomDedupe";

fn load_wecom_dedupe(data_root: &std::path::Path, bot_id: &str) -> Vec<WeComDedupeEntry> {
    BotsRepo::new(data_root)
        .read_state()
        .ok()
        .and_then(|state| {
            state
                .get("bots")
                .and_then(|bots| bots.get(bot_id))
                .and_then(|entry| entry.get(WECOM_DEDUPE_STATE_KEY))
                .cloned()
        })
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

fn persist_wecom_dedupe(
    data_root: &std::path::Path,
    bot_id: &str,
    entries: Vec<WeComDedupeEntry>,
) {
    let Ok(value) = serde_json::to_value(entries) else {
        return;
    };
    let _ = BotsRepo::new(data_root).mutate_state(|state| {
        let bots = state
            .as_object_mut()
            .ok_or_else(|| "bot-state 不是对象".to_string())?
            .entry("bots")
            .or_insert_with(|| json!({}));
        let entry = bots
            .as_object_mut()
            .ok_or_else(|| "bot-state.bots 不是对象".to_string())?
            .entry(bot_id.to_string())
            .or_insert_with(|| json!({}));
        entry[WECOM_DEDUPE_STATE_KEY] = value;
        Ok(())
    });
}

/// 企微流式回复正文上限（与主程序同口径）。
const WECOM_STREAM_TEXT_LIMIT: usize = 3_500;
/// 企微请求回执超时（SDK `replyAckTimeout` 为 5s；上传分片留一点余量）。
const WECOM_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

type WeComStreams = Arc<Mutex<HashMap<String, String>>>;

fn take_stream(streams: &WeComStreams, req_id: &str) -> Option<String> {
    streams
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(req_id)
}

/// 企微回复投递：入站回合以一条 `finish=true` 的流式消息回原 req_id；
/// 主动触达（无 req_id）走 markdown 消息。
pub struct WeComReplySink {
    bot_id: String,
    outbound: UnboundedSender<Value>,
    streams: WeComStreams,
    broker: WeComRequestBroker,
}

impl WeComReplySink {
    pub fn new(
        bot_id: &str,
        outbound: UnboundedSender<Value>,
        streams: WeComStreams,
        broker: WeComRequestBroker,
    ) -> Self {
        Self {
            bot_id: bot_id.to_string(),
            outbound,
            streams,
            broker,
        }
    }
}

impl BotReplySink for WeComReplySink {
    fn send_reply<'a>(
        &'a self,
        _bot_id: &'a str,
        conversation_key: &'a str,
        context_token: Option<&'a str>,
        text: &'a str,
    ) -> RuntimeFuture<'a, Result<(), String>> {
        let outbound = self.outbound.clone();
        let req_id = context_token.map(str::to_string);
        let text = text.to_string();
        let bot_id = self.bot_id.clone();
        let streams = Arc::clone(&self.streams);
        let conversation_key = conversation_key.to_string();
        Box::pin(async move {
            if text.trim().is_empty() {
                return Ok(());
            }
            let chunks = split_bot_text(&text, WECOM_STREAM_TEXT_LIMIT);
            let req_id = req_id.filter(|value| !value.trim().is_empty());
            if let Some(req_id) = req_id {
                let stream_id =
                    take_stream(&streams, &req_id).unwrap_or_else(|| generate_req_id("stream"));
                let head = chunks.first().map(String::as_str).unwrap_or_default();
                outbound
                    .send(reply_frame(
                        CMD_RESPONSE,
                        &req_id,
                        stream_reply_body(&stream_id, head, true),
                    ))
                    .map_err(|_| format!("企微连接已关闭，回复未发送（bot={bot_id}）"))?;
                for chunk in chunks.iter().skip(1) {
                    outbound
                        .send(reply_frame(
                            CMD_SEND_MSG,
                            &generate_req_id(CMD_SEND_MSG),
                            markdown_send_body(&conversation_key, chunk),
                        ))
                        .map_err(|_| format!("企微连接已关闭，回复未发送（bot={bot_id}）"))?;
                }
                return Ok(());
            }
            for chunk in &chunks {
                outbound
                    .send(reply_frame(
                        CMD_SEND_MSG,
                        &generate_req_id(CMD_SEND_MSG),
                        markdown_send_body(&conversation_key, chunk),
                    ))
                    .map_err(|_| format!("企微连接已关闭，回复未发送（bot={bot_id}）"))?;
            }
            Ok(())
        })
    }

    fn send_media<'a>(
        &'a self,
        _bot_id: &'a str,
        conversation_key: &'a str,
        context_token: Option<&'a str>,
        media: OutboundMedia,
    ) -> RuntimeFuture<'a, Result<(), String>> {
        let outbound = self.outbound.clone();
        let broker = self.broker.clone();
        let streams = Arc::clone(&self.streams);
        let bot_id = self.bot_id.clone();
        let conversation_key = conversation_key.to_string();
        let req_id = context_token
            .map(str::to_string)
            .filter(|value| !value.trim().is_empty());
        Box::pin(async move {
            let media_id = upload_wecom_media(&broker, &outbound, &media).await?;
            let mut caption_handled = false;
            if let Some(req_id) = req_id.as_deref()
                && let Some(stream_id) = take_stream(&streams, req_id)
            {
                let placeholder_text = media
                    .caption
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or(&media.filename);
                if request_frame(
                    &broker,
                    &outbound,
                    CMD_RESPONSE,
                    req_id,
                    stream_reply_body(&stream_id, placeholder_text, true),
                    WECOM_REQUEST_TIMEOUT,
                )
                .await
                .is_ok()
                {
                    caption_handled = media
                        .caption
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty());
                }
            }
            if let Some(caption) = media
                .caption
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                && !caption_handled
            {
                if let Some(req_id) = req_id.as_deref() {
                    let _ = request_frame(
                        &broker,
                        &outbound,
                        CMD_RESPONSE,
                        req_id,
                        stream_reply_body(&generate_req_id("stream"), caption, true),
                        WECOM_REQUEST_TIMEOUT,
                    )
                    .await;
                } else {
                    let frame = reply_frame(
                        CMD_SEND_MSG,
                        &generate_req_id(CMD_SEND_MSG),
                        markdown_send_body(&conversation_key, caption),
                    );
                    outbound.send(frame).map_err(|_| {
                        format!("企微连接已关闭，媒体说明未发送（bot={bot_id}）")
                    })?;
                }
            }
            let media_type = media.kind.as_wire();
            if let Some(req_id) = req_id.as_deref() {
                request_frame(
                    &broker,
                    &outbound,
                    CMD_RESPONSE,
                    req_id,
                    media_reply_body(media_type, &media_id),
                    WECOM_REQUEST_TIMEOUT,
                )
                .await?;
            } else {
                request_frame(
                    &broker,
                    &outbound,
                    CMD_SEND_MSG,
                    &generate_req_id(CMD_SEND_MSG),
                    media_send_body(&conversation_key, media_type, &media_id),
                    WECOM_REQUEST_TIMEOUT,
                )
                .await?;
            }
            Ok(())
        })
    }
}

/// 三步分片上传（init → chunk × N → finish），返回 `media_id`。
async fn upload_wecom_media(
    broker: &WeComRequestBroker,
    outbound: &UnboundedSender<Value>,
    media: &OutboundMedia,
) -> Result<String, String> {
    let total_size = media.bytes.len() as u64;
    let chunks = plan_media_chunks(total_size)?;
    let md5 = md5_hex(&media.bytes);
    let req_id = generate_req_id(CMD_UPLOAD_MEDIA_INIT);
    let body = request_frame(
        broker,
        outbound,
        CMD_UPLOAD_MEDIA_INIT,
        &req_id,
        media_upload_init_body(
            media.kind.as_wire(),
            &media.filename,
            total_size,
            chunks.len() as u64,
            &md5,
        ),
        WECOM_REQUEST_TIMEOUT,
    )
    .await?;
    let upload_id = body
        .get("upload_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "企微上传初始化失败：缺少 upload_id。".to_string())?
        .to_string();
    for (index, (start, end)) in chunks.iter().enumerate() {
        let data = base64_encode(&media.bytes[*start as usize..*end as usize]);
        let req_id = generate_req_id(CMD_UPLOAD_MEDIA_CHUNK);
        request_frame(
            broker,
            outbound,
            CMD_UPLOAD_MEDIA_CHUNK,
            &req_id,
            media_upload_chunk_body(&upload_id, index as u64, &data),
            WECOM_REQUEST_TIMEOUT,
        )
        .await?;
    }
    let req_id = generate_req_id(CMD_UPLOAD_MEDIA_FINISH);
    let body = request_frame(
        broker,
        outbound,
        CMD_UPLOAD_MEDIA_FINISH,
        &req_id,
        media_upload_finish_body(&upload_id),
        WECOM_REQUEST_TIMEOUT,
    )
    .await?;
    body.get("media_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| "企微上传完成失败：缺少 media_id。".to_string())
}

/// 企微入站处理：WS `aibot_msg_callback` → 共用编排 → 占位流收口。
pub struct WeComInboundHandler {
    bot_id: String,
    inner: Arc<dyn WeixinMessageHandler>,
    outbound: UnboundedSender<Value>,
    streams: WeComStreams,
}

impl WeComInboundHandler {
    pub fn new(
        bot_id: &str,
        inner: Arc<dyn WeixinMessageHandler>,
        outbound: UnboundedSender<Value>,
        streams: WeComStreams,
    ) -> Self {
        Self {
            bot_id: bot_id.to_string(),
            inner,
            outbound,
            streams,
        }
    }

    fn open_placeholder(&self, req_id: &str) {
        if req_id.trim().is_empty() {
            return;
        }
        let stream_id = generate_req_id("stream");
        let frame = reply_frame(
            CMD_RESPONSE,
            req_id,
            stream_reply_body(&stream_id, WECOM_TYPING_PLACEHOLDER, false),
        );
        if self.outbound.send(frame).is_ok() {
            self.streams
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(req_id.to_string(), stream_id);
        }
    }

    fn finish_placeholder(&self, req_id: &str, text: &str) {
        let Some(stream_id) = take_stream(&self.streams, req_id) else {
            return;
        };
        let frame = reply_frame(
            CMD_RESPONSE,
            req_id,
            stream_reply_body(&stream_id, text, true),
        );
        let _ = self.outbound.send(frame);
    }

    pub async fn handle_body(&self, req_id: &str, body: &Value) -> Result<(), String> {
        let Some(message) = parse_inbound_message(&self.bot_id, body) else {
            return Ok(());
        };
        im_log!(
            "[im] wecom inbound bot={} chat={} user={} msg={:?} text_len={}",
            message.bot_id,
            bots_service::wecom_payload::chat_key(&message),
            message.provider_user_id,
            message.message_id,
            message.text.chars().count()
        );
        let converted = to_weixin_message(&message, req_id);
        self.open_placeholder(req_id);
        let result = self.inner.handle(&self.bot_id, &converted).await;
        if let Err(error) = &result {
            self.finish_placeholder(req_id, &format!("处理失败：{error}"));
        } else {
            self.finish_placeholder(req_id, "");
        }
        result
    }
}

fn inbound_message_id(body: &Value) -> Option<String> {
    let value = body.get("msgid").and_then(Value::as_str)?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// 企微媒体解密：`aeskey` 为 base64（常缺 `=` 填充），AES-256-CBC，IV = key 前 16 字节。
fn decrypt_wecom_media(bytes: &[u8], raw_key: &str) -> Result<Vec<u8>, String> {
    use aes::cipher::{BlockDecryptMut, KeyIvInit};
    use base64::Engine as _;
    type Decryptor = cbc::Decryptor<aes::Aes256>;
    let raw = raw_key.trim();
    if raw.is_empty() {
        return Ok(bytes.to_vec());
    }
    let padded = match raw.len() % 4 {
        0 => raw.to_string(),
        remainder => format!("{raw}{}", "=".repeat(4 - remainder)),
    };
    let key = base64::engine::general_purpose::STANDARD
        .decode(padded)
        .map_err(|error| format!("企微附件 aeskey 解析失败：{error}"))?;
    if key.len() != 32 {
        return Err(format!("企微附件 aeskey 长度异常：{}", key.len()));
    }
    let mut buffer = bytes.to_vec();
    let plain = Decryptor::new_from_slices(&key, &key[..16])
        .map_err(|error| format!("企微附件解密初始化失败：{error}"))?
        .decrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buffer)
        .map_err(|error| format!("企微附件解密失败：{error}"))?;
    Ok(plain.to_vec())
}

fn to_weixin_attachment(attachment: &WeComInboundAttachment) -> bots_service::weixin_inbound::WeixinInboundAttachment {
    bots_service::weixin_inbound::WeixinInboundAttachment {
        id: attachment.id.clone(),
        kind: attachment.kind.clone(),
        filename: attachment.filename.clone(),
        mime_type: attachment.mime_type.clone(),
        size_bytes: None,
        provider_file_id: None,
        download_url: Some(attachment.download_url.clone()),
        data_base64: None,
        aes_key: attachment.aes_key.clone(),
    }
}

fn to_weixin_message(
    message: &bots_service::wecom_payload::WeComInboundMessage,
    req_id: &str,
) -> WeixinInboundMessage {
    WeixinInboundMessage {
        bot_id: message.bot_id.clone(),
        text: message.text.clone(),
        attachments: message.attachments.iter().map(to_weixin_attachment).collect(),
        actor: WeixinInboundActor {
            provider_user_id: message.provider_user_id.clone(),
            display_name: message.display_name.clone(),
            // 企微用 `single` 表示私聊；门禁（与命令面）只认 `private` / `group`。
            chat_type: if message.chat_type == "single" {
                "private".to_string()
            } else {
                message.chat_type.clone()
            },
            chat_id: message.chat_id.clone(),
            provider_message_id: message.message_id.clone(),
            provider_context_token: Some(req_id.to_string()),
        },
    }
}

fn turn_ops(data_root: &std::path::Path) -> Arc<dyn BotTurnOps> {
    Arc::new(RpiTurnOps::new(data_root.to_path_buf()))
}

/// 内嵌 pi-rust 的 IM 回合操作：每个 IM 会话一个持久 JSONL 会话。
pub struct RpiTurnOps {
    data_root: PathBuf,
    sessions: tokio::sync::Mutex<HashMap<String, Arc<RpiEntry>>>,
    open_lock: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct RpiTurnState {
    running: bool,
    text: String,
    error: Option<String>,
    /// 待发出去的提问卡提示（`take_notice` 取走即清空）。
    notice: Option<String>,
}

struct RpiEntry {
    session: Arc<crate::rpi_engine::RpiSession>,
    state: Arc<Mutex<RpiTurnState>>,
    approvals: Arc<Mutex<Vec<PendingPermissionPrompt>>>,
    /// 待作答的提问卡（`ask_user_question`）。
    questions: Arc<Mutex<Vec<PendingQuestionPrompt>>>,
}

/// IM 里的提问卡：`request_id` + 问题 + 选项（类型在 `bots-service` 里，通道共用）。
type PendingQuestionPrompt = bots_service::conversation::PendingQuestionPrompt;

impl RpiTurnOps {
    pub fn new(data_root: PathBuf) -> Self {
        Self {
            data_root,
            sessions: tokio::sync::Mutex::new(HashMap::new()),
            open_lock: tokio::sync::Mutex::new(()),
        }
    }

    async fn open_entry(
        &self,
        resume: Option<String>,
        workspace: &str,
        identity: Option<&str>,
    ) -> Result<Arc<RpiEntry>, String> {
        let identity = identity.unwrap_or("");
        let spec = crate::rpi_engine::default_model_spec(identity, workspace)
            .ok_or_else(|| "未选择模型：请先在桌面端为这个 Bot 选一个模型".to_string())?;
        let engine_model = crate::rpi_engine::resolve_model(&spec)?;
        let (approval_level, allowlist) =
            crate::rpi_engine::approval_policy(identity, workspace);
        let im_bot_id = crate::rpi_engine::bot_id_from_identity(identity);
        let im_limits = crate::model_limits::read(&spec.provider_id, &spec.model_id);
        let (im_window, _) = crate::model_limits::resolve(&im_limits);
        let system_prompt = crate::rpi_engine::system_prompt_for_bot(
            std::path::Path::new(workspace),
            im_bot_id.as_deref(),
            im_window,
        );
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::backend::BackendEvent>();
        let mut extra_tools = crate::mcp_host::for_workspace(workspace)
            .await
            .harness_tools();
        extra_tools.extend(crate::automation_tools::tools(
            &self.data_root,
            workspace,
            identity,
        ));
        if crate::tool_scope::allowed(crate::tool_scope::Family::Browser, identity) {
            extra_tools.extend(crate::browser_tools::tools(identity));
        }
        if crate::tool_scope::allowed(crate::tool_scope::Family::Computer, identity) {
            extra_tools.extend(crate::computer_tools::tools(identity));
            extra_tools.extend(crate::cua_tree::tools(identity));
        }
        let options = crate::rpi_engine::OpenOptions {
            workspace: PathBuf::from(workspace),
            resume_path: resume.filter(|path| Path::new(path).is_file()),
            model: engine_model,
            thinking: crate::rpi_engine::thinking_level_for(&spec),
            system_prompt,
            events: tx,
            key: "im".to_string(),
            member: None,
            approval_level,
            allowlist,
            bot_id: im_bot_id.clone(),
            bot_name: im_bot_id.as_deref().and_then(crate::backend::bot_name_of),
            extra_tools,
            private_bot: crate::rpi_engine::private_bot_id(identity),
        };
        let session = Arc::new(crate::rpi_engine::RpiSession::open(options).await?);
        let state = Arc::new(Mutex::new(RpiTurnState::default()));
        let approvals = Arc::new(Mutex::new(Vec::new()));
        let questions = Arc::new(Mutex::new(Vec::new()));
        let state_for_reader = Arc::clone(&state);
        let approvals_for_reader = Arc::clone(&approvals);
        let questions_for_reader = Arc::clone(&questions);
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                let crate::backend::BackendEvent::Session { event, .. } = event else {
                    continue;
                };
                match event {
                    crate::backend::SessionEvent::Delta { text, .. } => {
                        state_for_reader
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .text
                            .push_str(&text);
                    }
                    crate::backend::SessionEvent::Done { .. } => {
                        state_for_reader
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .running = false;
                    }
                    crate::backend::SessionEvent::Error { message, .. } => {
                        let mut state = state_for_reader
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        state.error = Some(message);
                        state.running = false;
                    }
                    crate::backend::SessionEvent::Permission(prompt) => {
                        approvals_for_reader
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(PendingPermissionPrompt {
                                request_id: prompt.request_id,
                                tool_name: prompt.tool_name,
                                reason: prompt.reason,
                                risk_level: prompt.risk,
                                options: prompt
                                    .options
                                    .into_iter()
                                    .map(|option| PermissionPromptOption {
                                        option_id: option.option_id,
                                        kind: option.kind,
                                        name: option.label,
                                    })
                                    .collect(),
                            });
                    }
                    crate::backend::SessionEvent::PermissionResolved { request_id } => {
                        approvals_for_reader
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .retain(|prompt| prompt.request_id != request_id);
                    }
                    // 提问卡（W11）：原来落进 `_ => {}`，用户在 IM 里永远看不到问题。
                    // 现在记进 questions 并把问题文本挂到 notice，由 runner 发出去。
                    crate::backend::SessionEvent::Question(card) => {
                        let prompt = PendingQuestionPrompt {
                            request_id: card.request_id,
                            question: card.question,
                            options: card.options.into_iter().map(|(_, label)| label).collect(),
                        };
                        let rendered = prompt.render();
                        questions_for_reader
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(prompt);
                        state_for_reader
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .notice = Some(rendered);
                    }
                    crate::backend::SessionEvent::QuestionResolved { request_id } => {
                        questions_for_reader
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .retain(|prompt| prompt.request_id != request_id);
                    }
                    _ => {}
                }
            }
        });
        let entry = Arc::new(RpiEntry {
            session,
            state,
            approvals,
            questions,
        });
        Ok(entry)
    }

    async fn entry_for(&self, context: &ConversationContext) -> Result<Arc<RpiEntry>, String> {
        if let Some(session_id) = context
            .session_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if let Some(entry) = self.sessions.lock().await.get(session_id).cloned() {
                return Ok(entry);
            }
            let _guard = self.open_lock.lock().await;
            if let Some(entry) = self.sessions.lock().await.get(session_id).cloned() {
                return Ok(entry);
            }
            let entry = self
                .open_entry(
                    Some(session_id.to_string()),
                    &context.workspace_path,
                    context.workspace_identity.as_deref(),
                )
                .await?;
            self.sessions
                .lock()
                .await
                .insert(entry.session.path.clone(), Arc::clone(&entry));
            return Ok(entry);
        }
        let entry = self
            .open_entry(
                None,
                &context.workspace_path,
                context.workspace_identity.as_deref(),
            )
            .await?;
        self.sessions
            .lock()
            .await
            .insert(entry.session.path.clone(), Arc::clone(&entry));
        Ok(entry)
    }
}

impl BotTurnOps for RpiTurnOps {
    fn create_session<'a>(
        &'a self,
        workspace_path: &'a str,
        workspace_identity: Option<&'a str>,
    ) -> bots_service::conversation::RuntimeFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let entry = self
                .open_entry(None, workspace_path, workspace_identity)
                .await?;
            let path = entry.session.path.clone();
            self.sessions
                .lock()
                .await
                .insert(path.clone(), entry);
            Ok(path)
        })
    }

    fn send_text<'a>(
        &'a self,
        context: &'a ConversationContext,
        text: &'a str,
    ) -> bots_service::conversation::RuntimeFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let entry = self.entry_for(context).await?;
            {
                let mut state = entry
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.running = true;
                state.text.clear();
                state.error = None;
            }
            let session = Arc::clone(&entry.session);
            let text = text.to_string();
            tokio::spawn(async move {
                session.send(text, Vec::new()).await;
            });
            Ok(())
        })
    }

    fn read_reply<'a>(
        &'a self,
        context: &'a ConversationContext,
    ) -> bots_service::conversation::RuntimeFuture<'a, Result<Option<String>, String>> {
        Box::pin(async move {
            let entry = self.entry_for(context).await?;
            let state = entry
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone_state();
            if state.running {
                return Ok(None);
            }
            let mut text = state.text;
            if let Some(error) = state.error {
                if text.trim().is_empty() {
                    text = format!("回合失败：{error}");
                } else {
                    text.push_str("\n\n（回合失败：");
                    text.push_str(&error);
                    text.push('）');
                }
            }
            Ok(Some(text))
        })
    }

    fn pending_permissions<'a>(
        &'a self,
        context: &'a ConversationContext,
    ) -> bots_service::conversation::RuntimeFuture<'a, Result<Vec<PendingPermissionPrompt>, String>> {
        Box::pin(async move {
            let entry = self.entry_for(context).await?;
            let prompts = entry
                .approvals
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            Ok(prompts)
        })
    }

    /// 待作答的提问卡（`/question` 用）。
    fn pending_questions<'a>(
        &'a self,
        context: &'a ConversationContext,
    ) -> bots_service::conversation::RuntimeFuture<'a, Result<Vec<PendingQuestionPrompt>, String>>
    {
        Box::pin(async move {
            let entry = self.entry_for(context).await?;
            let prompts = entry
                .questions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            Ok(prompts)
        })
    }

    /// 手动压缩会话上下文（IM 的 `/compact`）：对当前会话调引擎压缩。
    /// 返回文案与 `RpiSession::compact` 同口径——`Ok` 也可能是"上下文还很短"。
    fn compact_session<'a>(
        &'a self,
        context: &'a ConversationContext,
    ) -> bots_service::conversation::RuntimeFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let entry = self.entry_for(context).await?;
            entry.session.compact().await
        })
    }

    fn resolve_interaction<'a>(
        &'a self,
        context: &'a ConversationContext,
        request_id: &'a str,
        option_id: &'a str,
    ) -> bots_service::conversation::RuntimeFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let entry = self.entry_for(context).await?;
            if entry.session.gate.resolve(request_id, option_id) {
                Ok(())
            } else {
                Err("审批请求已失效".to_string())
            }
        })
    }

    /// 取一条待发的提问卡提示（W11）；取走即清空，避免重复发。
    fn take_notice<'a>(
        &'a self,
        context: &'a ConversationContext,
    ) -> bots_service::conversation::RuntimeFuture<'a, Result<Option<String>, String>> {
        Box::pin(async move {
            let entry = self.entry_for(context).await?;
            let mut state = entry
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Ok(state.notice.take())
        })
    }

    /// 作答一条提问卡：`answer=None` = 取消（agent 侧会拿到「用户取消了提问」）。
    fn resolve_question<'a>(
        &'a self,
        context: &'a ConversationContext,
        request_id: &'a str,
        answer: Option<&'a str>,
    ) -> bots_service::conversation::RuntimeFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let entry = self.entry_for(context).await?;
            if entry
                .session
                .questions
                .answer(request_id, answer.map(str::to_string))
            {
                entry
                    .questions
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .retain(|prompt| prompt.request_id != request_id);
                Ok(())
            } else {
                Err("提问已失效或已被回答".to_string())
            }
        })
    }

    fn stop<'a>(
        &'a self,
        context: &'a ConversationContext,
    ) -> bots_service::conversation::RuntimeFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            let entry = self.entry_for(context).await?;
            entry.session.abort().await;
            entry
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .running = false;
            Ok(true)
        })
    }
}

impl RpiTurnState {
    fn clone_state(&self) -> RpiTurnState {
        RpiTurnState {
            running: self.running,
            text: self.text.clone(),
            error: self.error.clone(),
            notice: None,
        }
    }
}


/// 微信入站处理器：绑定 → 闸门 → 命令 → 会话编排。
pub struct ImWeixinHandler {
    data_root: std::path::PathBuf,
    bot: BotConfig,
    repo: BotsRepo,
    ops: Arc<dyn BotTurnOps>,
    sink: Arc<dyn BotReplySink>,
    bind_codes: Arc<BindCodeStore>,
    http: Arc<dyn HttpJsonPort>,
    active_turns: Mutex<HashSet<String>>,
    provider_label: &'static str,
}

impl ImWeixinHandler {
    pub fn new(
        data_root: std::path::PathBuf,
        bot: BotConfig,
        bind_codes: Arc<BindCodeStore>,
        ops: Arc<dyn BotTurnOps>,
        sink: Arc<dyn BotReplySink>,
        http: Arc<dyn HttpJsonPort>,
    ) -> Self {
        Self {
            repo: BotsRepo::new(&data_root),
            data_root,
            bot,
            ops,
            sink,
            bind_codes,
            http,
            active_turns: Mutex::new(HashSet::new()),
            provider_label: "weixin",
        }
    }

    /// 日志里的通道名（微信 / 企微共用同一套编排）。
    pub fn with_provider_label(mut self, label: &'static str) -> Self {
        self.provider_label = label;
        self
    }

    fn read_v2_json(&self, file: &str) -> Option<Value> {
        let text = std::fs::read_to_string(self.data_root.join("v2").join(file)).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn teammate_directory(&self, id: &str) -> Option<String> {
        let value = self.read_v2_json("teammates.json")?;
        value
            .get("teammates")?
            .as_array()?
            .iter()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(id))?
            .get("directory")?
            .as_str()
            .map(str::to_string)
    }

    fn resolve_workspace_key(&self, key: &str) -> Option<(String, Option<String>)> {
        let key = key.trim();
        if key.is_empty() || key == "*" {
            return None;
        }
        if let Some(id) = key.strip_prefix("mode-bot:teammate:") {
            let directory = self.teammate_directory(id)?;
            if std::path::Path::new(&directory).is_dir() {
                return Some((directory, Some(key.to_string())));
            }
            return None;
        }
        if std::path::Path::new(key).is_dir() {
            return Some((key.to_string(), None));
        }
        None
    }

    fn resolve_workspace(&self) -> Option<(String, Option<String>)> {
        for key in &self.bot.allowed_workspaces {
            if let Some(resolved) = self.resolve_workspace_key(key) {
                return Some(resolved);
            }
        }
        if let Some(bindings) = self.read_v2_json("bot-bindings.v3.json")
            && let Some(map) = bindings.get("bindings").and_then(Value::as_object)
        {
            for (key, value) in map {
                let references_bot = match value {
                    Value::String(text) => text.split(',').any(|id| id.trim() == self.bot.id),
                    Value::Array(items) => items
                        .iter()
                        .filter_map(Value::as_str)
                        .any(|id| id.trim() == self.bot.id),
                    _ => false,
                };
                if references_bot
                    && let Some(resolved) = self.resolve_workspace_key(key)
                {
                    return Some(resolved);
                }
            }
        }
        let value = self.read_v2_json("teammates.json")?;
        for teammate in value.get("teammates")?.as_array()? {
            if let Some(directory) = teammate.get("directory").and_then(Value::as_str)
                && std::path::Path::new(directory).is_dir()
            {
                let identity = teammate
                    .get("id")
                    .and_then(Value::as_str)
                    .map(|id| format!("mode-bot:teammate:{id}"));
                return Some((directory.to_string(), identity));
            }
        }
        None
    }

    fn workspace_refs(&self) -> Vec<WorkspaceRef> {
        let mut refs: Vec<WorkspaceRef> = Vec::new();
        let mut push = |path: String, identity: Option<String>| {
            let reference = create_workspace_ref(&path, identity.as_deref());
            if !refs.iter().any(|item| item.id == reference.id) {
                refs.push(reference);
            }
        };
        for key in &self.bot.allowed_workspaces {
            if let Some((path, identity)) = self.resolve_workspace_key(key) {
                push(path, identity);
            }
        }
        if let Some(value) = self.read_v2_json("teammates.json")
            && let Some(teammates) = value.get("teammates").and_then(Value::as_array)
        {
            for teammate in teammates {
                if let Some(directory) = teammate.get("directory").and_then(Value::as_str)
                    && std::path::Path::new(directory).is_dir()
                {
                    let identity = teammate
                        .get("id")
                        .and_then(Value::as_str)
                        .map(|id| format!("mode-bot:teammate:{id}"));
                    push(directory.to_string(), identity);
                }
            }
        }
        refs
    }

    fn conversation_key(message: &WeixinInboundMessage) -> String {
        bots_service::wecom::resolve_chat_key(
            message.actor.chat_id.as_deref(),
            &message.actor.provider_user_id,
        )
    }

    fn send_text<'a>(
        &'a self,
        message: &'a WeixinInboundMessage,
        text: String,
    ) -> RuntimeFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.sink
                .send_reply(
                    &self.bot.id,
                    &Self::conversation_key(message),
                    message.actor.provider_context_token.as_deref(),
                    &text,
                )
                .await
        })
    }

    fn gate(&self, message: &WeixinInboundMessage) -> GateDecision {
        evaluate_inbound_gate(
            &self.bot,
            &GateMessage {
                chat_id: message.actor.chat_id.clone(),
                chat_type: message.actor.chat_type.clone(),
                provider_user_id: message.actor.provider_user_id.clone(),
                // iLink 没有平台级 @ 元数据：群消息按定向消息处理（与主程序同口径）。
                is_mention: true,
            },
        )
    }

    fn handle_bind(&self, message: &WeixinInboundMessage, code: &str) -> bool {
        let Some((bot_id, workspaces)) = self.bind_codes.consume(code) else {
            return false;
        };
        if bot_id != self.bot.id {
            return false;
        }
        let user_id = message.actor.provider_user_id.trim().to_string();
        if user_id.is_empty() {
            return false;
        }
        let display_name = message.actor.display_name.clone();
        let discovered_at = now_ms();
        let bot_id = self.bot.id.clone();
        let result = self.repo.mutate_config(move |config| {
            let Some(target) = config.bots.iter_mut().find(|bot| bot.id == bot_id) else {
                return Err("Bot 不存在".to_string());
            };
            target.provider_user_id = Some(user_id.clone());
            target.allowed_workspaces = if workspaces.is_empty() {
                vec!["*".to_string()]
            } else {
                workspaces.clone()
            };
            let registry = target.user_registry.get_or_insert_with(Vec::new);
            if !registry.iter().any(|user| user.provider_user_id == user_id) {
                registry.push(BotRegisteredUser {
                    provider_user_id: user_id.clone(),
                    display_name: display_name.clone(),
                    discovered_at,
                });
            }
            Ok(())
        });
        result.is_ok()
    }

    fn register_free_user(&self, message: &WeixinInboundMessage) {
        if message.actor.chat_type != "private" {
            return;
        }
        let free = self.bot.access_mode == Some(bots_service::BotAccessMode::Free)
            || self.bot.private_chat_mode == Some(bots_service::BotPrivateChatMode::AllUsers);
        if !free {
            return;
        }
        let user_id = message.actor.provider_user_id.trim().to_string();
        if user_id.is_empty() {
            return;
        }
        let display_name = message.actor.display_name.clone();
        let bot_id = self.bot.id.clone();
        let discovered_at = now_ms();
        let _ = self.repo.mutate_config(move |config| {
            let Some(target) = config.bots.iter_mut().find(|bot| bot.id == bot_id) else {
                return Ok(());
            };
            let registry = target.user_registry.get_or_insert_with(Vec::new);
            if !registry.iter().any(|user| user.provider_user_id == user_id) {
                registry.push(BotRegisteredUser {
                    provider_user_id: user_id,
                    display_name,
                    discovered_at,
                });
            }
            Ok(())
        });
    }

    /// 群发现/批准：写 `groupChat.permissions`（pending/approved），保留激活策略。
    fn register_group(&self, chat_id: &str, approved: bool) {
        let group_id = chat_id.trim().to_string();
        if group_id.is_empty() {
            return;
        }
        let bot_id = self.bot.id.clone();
        let provider = self.bot.provider;
        let fallback_activation = self
            .bot
            .group_chat
            .as_ref()
            .map(|group| group.activation);
        let _ = self.repo.mutate_config(move |config| {
            let Some(target) = config.bots.iter_mut().find(|bot| bot.id == bot_id) else {
                return Ok(());
            };
            let activation = target
                .group_chat
                .as_ref()
                .map(|group| group.activation)
                .or(fallback_activation)
                .unwrap_or(BotGroupActivation::Mention);
            let current = target
                .group_chat
                .as_ref()
                .and_then(|group| group.permissions.clone())
                .unwrap_or_default();
            let status = if approved {
                BotGroupPermissionStatus::Approved
            } else {
                BotGroupPermissionStatus::Pending
            };
            let permissions = match current.iter().find(|item| item.group_id == group_id) {
                Some(_) => current
                    .into_iter()
                    .map(|mut item| {
                        if item.group_id == group_id && approved {
                            item.status = status;
                            item.added_by = Some("auto".to_string());
                        }
                        item
                    })
                    .collect(),
                None => {
                    let mut next = current;
                    next.push(BotGroupPermission {
                        group_id: group_id.clone(),
                        group_name: group_id.clone(),
                        platform: provider,
                        status,
                        discovered_at: now_ms(),
                        added_by: approved.then(|| "auto".to_string()),
                    });
                    next
                }
            };
            target.group_chat = Some(BotGroupChat {
                activation,
                allowed_groups: target
                    .group_chat
                    .as_ref()
                    .and_then(|group| group.allowed_groups.clone()),
                permissions: Some(permissions),
            });
            Ok(())
        });
    }

    /// 附件物化：下载/解密到数据根内，把本机路径写进提示（模型可直接读取）。
    async fn prompt_text(&self, message: &WeixinInboundMessage) -> String {
        let mut text = message.text.clone();
        if message.attachments.is_empty() {
            return text;
        }
        if self.bot.provider == BotProvider::Wecom {
            return self.prompt_text_wecom(text, message).await;
        }
        let inbound: Vec<InboundAttachment> = message
            .attachments
            .iter()
            .filter_map(Option::<InboundAttachment>::from)
            .collect();
        let unsupported = message.attachments.len() - inbound.len();
        let directory = self
            .data_root
            .join("v2")
            .join("bot-attachments")
            .join(&self.bot.id);
        let result = materialize_attachments(
            Arc::clone(&self.http),
            &inbound,
            &directory,
            20 * 1024 * 1024,
        )
        .await;
        if !result.files.is_empty() {
            text.push_str("\n\n附件已保存到本机（可直接读取）：");
            for file in &result.files {
                text.push_str(&format!(
                    "\n- {}（{}，{} 字节）",
                    file.path.display(),
                    file.mime_type,
                    file.bytes
                ));
            }
        }
        if unsupported > 0 {
            text.push_str(&format!("\n（另有 {unsupported} 个附件没有可下载地址，未同步）"));
        }
        if !result.errors.is_empty() {
            text.push_str(&format!(
                "\n（{} 个附件下载失败：{}）",
                result.errors.len(),
                result.errors.join("; ")
            ));
        }
        text
    }

    /// 企微入站附件：下载后按 `aeskey` 做 AES-256-CBC 解密再落盘。
    async fn prompt_text_wecom(
        &self,
        mut text: String,
        message: &WeixinInboundMessage,
    ) -> String {
        let directory = self
            .data_root
            .join("v2")
            .join("bot-attachments")
            .join(&self.bot.id);
        if let Err(error) = std::fs::create_dir_all(&directory) {
            text.push_str(&format!("
（附件目录创建失败：{error}）"));
            return text;
        }
        let mut saved = Vec::new();
        let mut errors = Vec::new();
        for attachment in &message.attachments {
            let Some(url) = attachment.download_url.as_deref() else {
                continue;
            };
            match self.materialize_wecom_attachment(url, attachment, &directory).await {
                Ok(path) => saved.push(path),
                Err(error) => errors.push(error),
            }
        }
        if !saved.is_empty() {
            text.push_str("

附件已保存到本机（可直接读取）：");
            for path in &saved {
                text.push_str(&format!("
- {}", path.display()));
            }
        }
        if !errors.is_empty() {
            text.push_str(&format!("
（{} 个附件下载失败：{}）", errors.len(), errors.join("; ")));
        }
        text
    }

    async fn materialize_wecom_attachment(
        &self,
        url: &str,
        attachment: &bots_service::weixin_inbound::WeixinInboundAttachment,
        directory: &std::path::Path,
    ) -> Result<std::path::PathBuf, String> {
        let response = self
            .http
            .get_bytes(url, &[], Duration::from_secs(30))
            .await?;
        if !(200..300).contains(&response.status) {
            return Err(format!("附件下载失败：HTTP {}", response.status));
        }
        let bytes = match attachment.aes_key.as_deref() {
            Some(key) => decrypt_wecom_media(&response.body, key)?,
            None => response.body,
        };
        let filename = bots_service::attachments::sanitize_attachment_filename(&attachment.filename);
        let path = directory.join(format!("{}-{filename}", attachment.id));
        std::fs::write(&path, &bytes).map_err(|error| format!("附件写入失败：{error}"))?;
        Ok(path)
    }

    /// `/send <路径> [| 说明]`：企微走媒体上传；微信 iLink 通道与主程序一致，暂不支持。
    async fn handle_send_media(&self, message: &WeixinInboundMessage, value: &str) -> String {
        if self.bot.provider != BotProvider::Wecom {
            return "微信通道暂不支持发送文件（与主程序一致）；企微通道可用 /send。".to_string();
        }
        let (file_path, caption) = match value.split_once('|') {
            Some((path, caption)) => (
                path.trim(),
                Some(caption.trim().to_string()).filter(|value| !value.is_empty()),
            ),
            None => (value.trim(), None),
        };
        if file_path.is_empty() {
            return "用法：/send <文件路径> [| 说明]".to_string();
        }
        let conversation_key = Self::conversation_key(message);
        let workspace_path = self
            .conversation_context(&conversation_key)
            .map(|context| context.workspace_path)
            .or_else(|| self.resolve_workspace().map(|(path, _)| path));
        let Some(workspace_path) = workspace_path else {
            return "没有可用工作区，无法解析文件路径。".to_string();
        };
        let roots = vec![
            std::path::PathBuf::from(workspace_path),
            self.data_root.join("v2"),
            std::env::temp_dir(),
        ];
        let resolved = match resolve_media_file(file_path, &roots) {
            Ok(resolved) => resolved,
            Err(error) => return format!("发送文件失败：{error}"),
        };
        let bytes = match tokio::fs::read(&resolved.real).await {
            Ok(bytes) => bytes,
            Err(error) => return format!("读取文件失败：{error}"),
        };
        let filename = resolved.filename.clone();
        let media = OutboundMedia {
            filename: filename.clone(),
            kind: resolved.kind,
            mime_type: resolved.mime_type,
            bytes,
            caption,
        };
        match self
            .sink
            .send_media(
                &self.bot.id,
                &conversation_key,
                message.actor.provider_context_token.as_deref(),
                media,
            )
            .await
        {
            Ok(()) => format!("已发送文件：{filename}"),
            Err(error) => format!("发送文件失败：{error}"),
        }
    }

    fn conversation_context(&self, conversation_key: &str) -> Option<ConversationContext> {
        let state = self.repo.read_state().ok()?;
        let value = state
            .get("bots")?
            .get(&self.bot.id)?
            .get("conversations")?
            .get(conversation_key)?
            .clone();
        serde_json::from_value(value).ok()
    }

    fn help_text(&self) -> String {
        "可用命令：\n/status 查看会话状态\n/new [工作区] 开始新会话\n/workspace 列出工作区\n/workspace <序号|id|路径> 切换\n/stop 停止当前生成\n/compact 压缩会话上下文\n/permission <序号> 作答审批\n/deny 拒绝当前审批\n/question <序号或答案> 作答提问（单独 /question = 取消）\n/bind <绑定码> 绑定账号\n/model 查看模型、/model set <provider>/<模型> 切换\n/mode 查看审批策略、/mode set ask|auto|full 切换\n\n文件发送仅企微通道支持；其余命令以桌面端为准。".to_string()
    }

    fn render_command_reply(&self, reply: CommandReply) -> String {
        match reply {
            CommandReply::NewDraft { workspace, switched } => {
                if switched {
                    format!("已开始新会话，工作区：{}（{}）", workspace.label, workspace.workspace_path)
                } else {
                    format!("已开始新会话：{}", workspace.label)
                }
            }
            CommandReply::WorkspaceList { workspaces, current } => {
                let mut lines = vec!["可用工作区：".to_string()];
                for (index, workspace) in workspaces.iter().enumerate() {
                    let marker = if current.as_deref() == Some(workspace.id.as_str()) {
                        " ←"
                    } else {
                        ""
                    };
                    lines.push(format!(
                        "{}. {}（{}）{}",
                        index + 1,
                        workspace.label,
                        workspace.workspace_path,
                        marker
                    ));
                }
                if lines.len() == 1 {
                    lines.push("（没有可用工作区）".to_string());
                }
                lines.join("\n")
            }
            CommandReply::WorkspaceSelected { workspace } => {
                format!("已切换工作区：{}", workspace.label)
            }
            CommandReply::WorkspaceNotFound { value, workspaces } => {
                let mut lines = vec![format!("找不到工作区：{value}")];
                for (index, workspace) in workspaces.iter().enumerate() {
                    lines.push(format!(
                        "{}. {}（{}）",
                        index + 1,
                        workspace.label,
                        workspace.workspace_path
                    ));
                }
                lines.join("\n")
            }
            CommandReply::Status {
                workspace_path,
                workspace_identity,
                mode,
                active_task_id,
                queued_messages,
                pending_permission,
                pending_elicitation,
            } => [
                "会话状态：".to_string(),
                format!("- 工作区: {workspace_path}"),
                format!(
                    "- 身份: {}",
                    workspace_identity.unwrap_or_else(|| "-".to_string())
                ),
                format!("- 模式: {mode}"),
                format!(
                    "- 任务: {}",
                    active_task_id.unwrap_or_else(|| "-".to_string())
                ),
                format!("- 队列: {queued_messages}"),
                format!(
                    "- 待审批: {}",
                    if pending_permission { "是" } else { "否" }
                ),
                format!(
                    "- 待回答: {}",
                    if pending_elicitation { "是" } else { "否" }
                ),
            ]
            .join("\n"),
        }
    }

    async fn execute_command(&self, command: BotCommand, message: &WeixinInboundMessage) -> String {
        let conversation_key = Self::conversation_key(message);
        match command {
            BotCommand::Help => self.help_text(),
            BotCommand::Status => match handle_status(&self.repo, &self.bot.id, &conversation_key) {
                Ok(reply) => self.render_command_reply(reply),
                Err(_) => "还没有会话上下文：先发一条消息建立会话，再发送 /status。".to_string(),
            },
            BotCommand::New => {
                let workspaces = self.workspace_refs();
                match handle_new(
                    &self.repo,
                    &self.bot.id,
                    &conversation_key,
                    &workspaces,
                    &self.bot.allowed_workspaces,
                    None,
                    None,
                ) {
                    Ok(reply) => self.render_command_reply(reply),
                    Err(error) => error,
                }
            }
            BotCommand::WorkspaceList => {
                let workspaces = self.workspace_refs();
                match handle_workspace_list(
                    &self.repo,
                    &self.bot.id,
                    &conversation_key,
                    &workspaces,
                    &self.bot.allowed_workspaces,
                ) {
                    Ok(reply) => self.render_command_reply(reply),
                    Err(error) => error,
                }
            }
            BotCommand::WorkspaceSet { value } => {
                let workspaces = self.workspace_refs();
                match handle_workspace_set(
                    &self.repo,
                    &self.bot.id,
                    &conversation_key,
                    &workspaces,
                    &self.bot.allowed_workspaces,
                    &value,
                    None,
                ) {
                    Ok(reply) => self.render_command_reply(reply),
                    Err(error) => error,
                }
            }
            BotCommand::Stop => {
                let Some(context) = self.conversation_context(&conversation_key) else {
                    return "还没有会话，无法停止。".to_string();
                };
                match self.ops.stop(&context).await {
                    Ok(true) => "已停止当前生成。".to_string(),
                    Ok(false) => "当前没有运行中的生成。".to_string(),
                    Err(error) => format!("停止失败：{error}"),
                }
            }
            BotCommand::PermissionRespond { value } => {
                let Some(context) = self.conversation_context(&conversation_key) else {
                    return "还没有会话，无法作答审批。".to_string();
                };
                let prompts = match self.ops.pending_permissions(&context).await {
                    Ok(prompts) => prompts,
                    Err(error) => return format!("读取审批失败：{error}"),
                };
                let options: Vec<(String, String, String)> = prompts
                    .iter()
                    .flat_map(|prompt| {
                        prompt.options.iter().map(move |option| {
                            (
                                prompt.request_id.clone(),
                                option.option_id.clone(),
                                option.name.clone(),
                            )
                        })
                    })
                    .collect();
                if options.is_empty() {
                    return "当前没有待审批请求。".to_string();
                }
                let selection = value.trim();
                let picked = selection
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| index.checked_sub(1))
                    .and_then(|index| options.get(index))
                    .cloned()
                    .or_else(|| {
                        options
                            .iter()
                            .find(|(_, option_id, _)| option_id == selection)
                            .cloned()
                    });
                let Some((request_id, option_id, name)) = picked else {
                    return "选项无效：请回复 /permission <序号>（序号见审批提示）。".to_string();
                };
                match self
                    .ops
                    .resolve_interaction(&context, &request_id, &option_id)
                    .await
                {
                    Ok(()) => format!("已提交审批：{name}"),
                    Err(error) => format!("提交审批失败：{error}"),
                }
            }
            BotCommand::Deny { request_id } => {
                let Some(context) = self.conversation_context(&conversation_key) else {
                    return "还没有会话，无法作答审批。".to_string();
                };
                let prompts = match self.ops.pending_permissions(&context).await {
                    Ok(prompts) => prompts,
                    Err(error) => return format!("读取审批失败：{error}"),
                };
                let option_id = prompts
                    .iter()
                    .find(|prompt| prompt.request_id == request_id)
                    .and_then(|prompt| {
                        prompt
                            .options
                            .iter()
                            .find(|option| option.kind.starts_with("deny"))
                            .map(|option| option.option_id.clone())
                    });
                let Some(option_id) = option_id else {
                    return "没有可用的拒绝选项。".to_string();
                };
                match self
                    .ops
                    .resolve_interaction(&context, &request_id, &option_id)
                    .await
                {
                    Ok(()) => "已拒绝当前审批。".to_string(),
                    Err(error) => format!("提交审批失败：{error}"),
                }
            }
            // 提问卡作答（W11）：原来这条命令会被 `_ =>` 吞掉当成「未接入」。
            BotCommand::QuestionRespond { value } => {
                let Some(context) = self.conversation_context(&conversation_key) else {
                    return "还没有会话，无法作答提问。".to_string();
                };
                let prompts = match self.ops.pending_questions(&context).await {
                    Ok(prompts) => prompts,
                    Err(error) => return format!("读取提问失败：{error}"),
                };
                if prompts.is_empty() {
                    return "当前没有待回答的提问。".to_string();
                }
                // 只有一条提问时允许直接回答案（自然语言），多条时要求带序号。
                let picked = if prompts.len() == 1 {
                    prompts[0].clone()
                } else {
                    let index = value
                        .trim()
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| index.checked_sub(1))
                        .and_then(|index| prompts.get(index).cloned());
                    match index {
                        Some(prompt) => prompt,
                        None => {
                            return "有多条提问：请回复 /question <序号>（序号见提问提示）"
                                .to_string()
                        }
                    }
                };
                // 有序号就按序号取选项，否则把原文当作自由答案。
                let answer = picked
                    .options
                    .iter()
                    .position(|option| option == value.trim())
                    .map(|index| picked.options[index].clone())
                    .or_else(|| {
                        value
                            .trim()
                            .parse::<usize>()
                            .ok()
                            .and_then(|index| index.checked_sub(1))
                            .and_then(|index| picked.options.get(index))
                            .cloned()
                    })
                    .unwrap_or_else(|| value.trim().to_string());
                match self
                    .ops
                    .resolve_question(&context, &picked.request_id, Some(&answer))
                    .await
                {
                    Ok(()) => format!("已作答：{answer}"),
                    Err(error) => format!("提交回答失败：{error}"),
                }
            }
            BotCommand::QuestionCancel => {
                let Some(context) = self.conversation_context(&conversation_key) else {
                    return "还没有会话，无法作答提问。".to_string();
                };
                let prompts = match self.ops.pending_questions(&context).await {
                    Ok(prompts) => prompts,
                    Err(error) => return format!("读取提问失败：{error}"),
                };
                if prompts.is_empty() {
                    return "当前没有待回答的提问。".to_string();
                }
                match self
                    .ops
                    .resolve_question(&context, &prompts[0].request_id, None)
                    .await
                {
                    Ok(()) => "已取消提问。".to_string(),
                    Err(error) => format!("取消提问失败：{error}"),
                }
            }
            BotCommand::Send { value } => self.handle_send_media(message, &value).await,
            BotCommand::Compact => {
                let Some(context) = self.conversation_context(&conversation_key) else {
                    return "还没有会话上下文：先发一条消息建立会话，再发送 /compact。".to_string();
                };
                match self.ops.compact_session(&context).await {
                    Ok(reply) => reply,
                    Err(error) => error,
                }
            }
            BotCommand::ModelList => self.handle_model_list(message),
            BotCommand::ModelSet { value } => self.handle_model_set(message, &value),
            BotCommand::ModeList => self.handle_mode_list(message),
            BotCommand::ModeSet { value } => self.handle_mode_set(message, &value),
            BotCommand::Unknown { .. } => self.help_text(),
            // ThoughtLevel / Task / Reply / Elicitation 提交等命令仍是桌面端独占。
            _ => "该命令暂未接入 GPUI 版，请在桌面端处理。".to_string(),
        }
    }

    /// `/model`：当前 Bot 的模型 + 可选清单（读名册与 provider 配置）。
    fn handle_model_list(&self, message: &WeixinInboundMessage) -> String {
        let roster = crate::data::Roster::load();
        let current = roster
            .bots
            .iter()
            .find(|bot| bot.id == message.bot_id)
            .and_then(|bot| bot.model.clone());
        let current_line = current
            .as_ref()
            .map(|model| format!("当前模型：{}", model.label()))
            .unwrap_or_else(|| "当前模型：跟随默认模型".to_string());
        let models = crate::data::available_models();
        if models.is_empty() {
            return format!("{current_line}\n（还没有可用模型；先在桌面端「模型」页添加服务商）");
        }
        let mut lines = vec![current_line, String::new(), "可用模型：".to_string()];
        for option in models {
            lines.push(format!("- {}/{}", option.provider_id, option.model_id));
        }
        lines.push(String::new());
        lines.push("切换：/model set <provider>/<模型>".to_string());
        lines.join("\n")
    }

    /// `/model set <provider>/<模型>`：写名册并立即生效（IM 每回合按 Bot 配置开新会话）。
    fn handle_model_set(&self, message: &WeixinInboundMessage, value: &str) -> String {
        let (provider_id, model_id) = match value.split_once('/') {
            Some((provider, model))
                if !provider.trim().is_empty() && !model.trim().is_empty() =>
            {
                (provider.trim().to_string(), model.trim().to_string())
            }
            _ => {
                return "格式：/model set <provider>/<模型>，例如 /model set deepseek/deepseek-flash"
                    .to_string();
            }
        };
        let spec = crate::data::ModelSpec {
            provider_id: provider_id.clone(),
            model_id: model_id.clone(),
            reasoning_level: None,
        };
        match crate::data::set_bot_model(&message.bot_id, Some(&spec)) {
            Ok(()) => format!("模型已切换：{}/{}（下一条消息生效）", provider_id, model_id),
            Err(error) => format!("切换失败：{error}"),
        }
    }

    /// `/mode`：当前 Bot 的审批策略（ask / auto / full）。
    fn handle_mode_list(&self, message: &WeixinInboundMessage) -> String {
        let roster = crate::data::Roster::load();
        let level = roster
            .bots
            .iter()
            .find(|bot| bot.id == message.bot_id)
            .map(|bot| bot.approval_level.clone())
            .unwrap_or_default();
        let current = bot_team::approval::parse_level(&level)
            .map(|level| match level {
                bot_team::approval::ApprovalLevel::Ask => "ask（每次询问）",
                bot_team::approval::ApprovalLevel::Auto => "auto（只读自动放行）",
                bot_team::approval::ApprovalLevel::Full => "full（完全自主）",
            })
            .unwrap_or(level.as_str())
            .to_string();
        format!("当前审批策略：{current}\n切换：/mode set ask|auto|full")
    }

    /// `/mode set ask|auto|full`：写名册并立即生效（审批决策每个回合读取）。
    fn handle_mode_set(&self, message: &WeixinInboundMessage, value: &str) -> String {
        let level = value.trim().to_lowercase();
        match level.as_str() {
            "ask" | "auto" | "full" => {}
            other => return format!("未知策略：{other}；可选 ask / auto / full"),
        }
        match crate::data::set_bot_approval_level(&message.bot_id, &level) {
            Ok(()) => format!("审批策略已切换：{level}"),
            Err(error) => format!("切换失败：{error}"),
        }
    }
}

fn command_name(command: &BotCommand) -> &'static str {
    match command {
        BotCommand::Message { .. } => "message",
        BotCommand::Bind { .. } => "bind",
        BotCommand::Help => "help",
        BotCommand::Status => "status",
        BotCommand::New => "new",
        BotCommand::Stop => "stop",
        BotCommand::WorkspaceSet { .. } | BotCommand::WorkspaceList => "workspace",
        BotCommand::PermissionRespond { .. } => "permission",
        BotCommand::QuestionRespond { .. } | BotCommand::QuestionCancel => "question",
        BotCommand::Deny { .. } => "deny",
        BotCommand::SelectionCancel => "selection-cancel",
        _ => "other",
    }
}

impl WeixinMessageHandler for ImWeixinHandler {
    fn handle<'a>(
        &'a self,
        _bot_id: &'a str,
        message: &'a WeixinInboundMessage,
    ) -> RuntimeFuture<'a, Result<(), String>> {
        Box::pin(async move {
            im_log!(
                "[im] {} inbound bot={} type={} user={} msg={:?} text_len={}",
                self.provider_label,
                self.bot.id,
                message.actor.chat_type,
                message.actor.provider_user_id,
                message.actor.provider_message_id,
                message.text.chars().count()
            );
            if let BotCommand::Bind { code } = parse_bot_command(&message.text) {
                let ok = self.handle_bind(message, &code);
                let reply = if ok {
                    "绑定成功：你现在可以私聊这个 Bot 了。".to_string()
                } else {
                    "绑定码无效或已过期，请在设置 → IM频道里重新生成。".to_string()
                };
                self.send_text(message, reply).await?;
                return Ok(());
            }
            let gate = self.gate(message);
            im_log!(
                "[im] {} gate bot={} decision={gate:?}",
                self.provider_label, self.bot.id
            );
            match gate {
                GateDecision::Allow => {}
                GateDecision::RequiresBinding => {
                    self.send_text(
                        message,
                        "你还没有绑定这个 Bot。请让管理员在设置 → IM频道里生成绑定码，然后发送 /bind <绑定码>。"
                            .to_string(),
                    )
                    .await?;
                    return Ok(());
                }
                GateDecision::PromoteGroup => {
                    if let Some(chat_id) = message.actor.chat_id.as_deref() {
                        self.register_group(chat_id, true);
                    }
                }
                GateDecision::RegisterPendingGroup => {
                    if let Some(chat_id) = message.actor.chat_id.as_deref() {
                        self.register_group(chat_id, false);
                    }
                    self.send_text(
                        message,
                        "已登记这个群聊；管理员在设置 → IM频道批准后我才会在群里回复。".to_string(),
                    )
                    .await?;
                    return Ok(());
                }
                GateDecision::Ignore => return Ok(()),
            }
            match parse_bot_command(&message.text) {
                BotCommand::Message { .. } | BotCommand::SelectionCancel => {}
                command => {
                    im_log!(
                        "[im] command bot={} name={}",
                        self.bot.id,
                        command_name(&command)
                    );
                    let reply = self.execute_command(command, message).await;
                    self.send_text(message, reply).await?;
                    return Ok(());
                }
            }
            if message.text.trim().is_empty() && message.attachments.is_empty() {
                return Ok(());
            }
            self.register_free_user(message);
            let Some((workspace_path, workspace_identity)) = self.resolve_workspace() else {
                self.send_text(
                    message,
                    "这个 Bot 没有可用工作区：请先新建一个 Bot 或绑定工作区。".to_string(),
                )
                .await?;
                return Ok(());
            };
            let store = ConversationStore::new(self.repo.clone());
            let runner = ConversationRunner::new(&store, &*self.ops, &*self.sink)
                .with_polling(Duration::from_secs(1), 240);
            let conversation_key = Self::conversation_key(message);
            let already_running = {
                let mut active = self
                    .active_turns
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                !active.insert(conversation_key.clone())
            };
            if already_running {
                self.send_text(
                    message,
                    "上一轮还在运行；可以发送 /stop 停止，或等它结束。".to_string(),
                )
                .await?;
                return Ok(());
            }
            let turn = InboundTurn {
                bot_id: self.bot.id.clone(),
                conversation_key: conversation_key.clone(),
                conversation_kind: Some(message.actor.chat_type.clone()),
                text: self.prompt_text(message).await,
                provider_context_token: message.actor.provider_context_token.clone(),
                default_workspace: workspace_path,
                default_workspace_identity: workspace_identity,
                quiet: false,
            };
            let result = runner.handle(turn).await;
            if let Ok(mut active) = self.active_turns.lock() {
                active.remove(&conversation_key);
            }
            match result {
                Ok(outcome) => {
                    im_log!(
                        "[im] turn ok bot={} created={} polls={} reply_len={}",
                        self.bot.id,
                        outcome.created,
                        outcome.polls,
                        outcome.reply.chars().count()
                    );
                }
                Err(error) => {
                    im_log!("[im] turn failed bot={}: {error}", self.bot.id);
                    self.send_text(message, format!("处理失败：{error}")).await?;
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_strips_markdown_and_uses_crlf() {
        let text = plain_weixin_text("**你好**\n第二行");
        assert!(text.contains("\r\n第二行"));
        assert!(!text.contains("**"));
    }

    #[test]
    fn wecom_media_decrypt_roundtrip() {
        use aes::cipher::{BlockEncryptMut, KeyIvInit};
        use base64::Engine as _;
        type Encryptor = cbc::Encryptor<aes::Aes256>;
        let key = [7u8; 32];
        let plain = b"hello wecom media";
        let buffer = plain.to_vec();
        let cipher = Encryptor::new_from_slices(&key, &key[..16])
            .expect("cipher")
            .encrypt_padded_vec_mut::<aes::cipher::block_padding::Pkcs7>(&buffer);
        let encoded = base64::engine::general_purpose::STANDARD.encode(key);
        let decoded = decrypt_wecom_media(&cipher, &encoded).expect("decrypt");
        assert_eq!(decoded, plain);
    }

    #[test]
    fn wecom_single_chat_normalizes_to_private() {
        let body = json!({
            "msgid": "m1",
            "msgtype": "text",
            "from": { "userid": "u1", "name": "小明" },
            "chattype": "single",
            "text": { "content": "你好" }
        });
        let message = parse_inbound_message("b1", &body).expect("parse");
        let converted = to_weixin_message(&message, "req-1");
        assert_eq!(converted.actor.chat_type, "private");
        assert_eq!(converted.actor.provider_context_token.as_deref(), Some("req-1"));
    }

    #[test]
    fn command_name_covers_core_commands() {
        assert_eq!(command_name(&BotCommand::Help), "help");
        assert_eq!(command_name(&BotCommand::Stop), "stop");
    }

    #[test]
    fn mirror_targets_find_task_conversation() {
        let dir = std::env::temp_dir().join(format!(
            "mode-bot-gpui-mirror-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("v2")).unwrap();
        std::fs::write(
            dir.join("v2").join("bot-config.v3.json"),
            serde_json::json!({
                "version": 3,
                "bots": [{
                    "id": "bot-1",
                    "name": "Bot",
                    "provider": "weixin",
                    "enabled": true,
                    "allowedWorkspaces": ["id-1"],
                    "replyMode": "streaming_card"
                }]
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            dir.join("v2").join("bot-state.v3.json"),
            serde_json::json!({
                "version": 4,
                "bots": {
                    "bot-1": {
                        "conversations": {
                            "c1": {
                                "mode": "task",
                                "activeTaskId": "sess-1",
                                "workspacePath": "C:/ws",
                                "workspaceIdentity": "id-1",
                                "conversationId": "user-1"
                            }
                        }
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        let targets = mirror_targets_for(&dir, "sess-1", "C:/ws", Some("id-1"));
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].target_id, "user-1");
        assert_eq!(targets[0].provider, BotProvider::Weixin);
        assert!(mirror_targets_for(&dir, "other", "C:/ws", Some("id-1")).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
