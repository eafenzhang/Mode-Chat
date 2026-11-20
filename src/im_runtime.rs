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
use tokio::sync::mpsc::UnboundedSender;

use crate::im_service::{BindCodeStore, ReqwestHttpPort, TokenLoader, TokenVault, now_ms};

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

/// 微信轮询 / 企微长连接运行时管理器。
pub struct ImRuntimeManager {
    data_root: std::path::PathBuf,
    http: Arc<ReqwestHttpPort>,
    tokens: Arc<TokenVault>,
    bind_codes: Arc<BindCodeStore>,
    statuses: WeixinStatusRegistry,
    running: Mutex<HashMap<String, RunningLoop>>,
    running_wecom: Mutex<HashMap<String, RunningWecom>>,
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
                    "\n（另有 {attachment_count} 个附件未同步到 IM）"
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
            .ok_or_else(|| format!("找不到 IM 机器人配置：{}", target.bot_id))?;
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
                "\n（另有 {attachment_count} 个附件未同步到 IM）"
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
        Ok(())
    }

    /// 对齐企微长连接：启用且配置/secret 未变的保留，其余重启或停止。
    fn refresh_wecom(self: &Arc<Self>) -> Result<(), String> {
        if self.stopped.load(Ordering::SeqCst) {
            return Ok(());
        }
        let config = BotsRepo::new(&self.data_root).read_config()?;
        let desired: Vec<BotConfig> = config
            .bots
            .into_iter()
            .filter(|bot| bot.enabled && bot.provider == BotProvider::Wecom)
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
}

struct RpiEntry {
    session: Arc<crate::rpi_engine::RpiSession>,
    state: Arc<Mutex<RpiTurnState>>,
    approvals: Arc<Mutex<Vec<PendingPermissionPrompt>>>,
}

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
        let system_prompt =
            crate::rpi_engine::system_prompt_for(std::path::Path::new(workspace));
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
            extra_tools,
        };
        let session = Arc::new(crate::rpi_engine::RpiSession::open(options).await?);
        let state = Arc::new(Mutex::new(RpiTurnState::default()));
        let approvals = Arc::new(Mutex::new(Vec::new()));
        let state_for_reader = Arc::clone(&state);
        let approvals_for_reader = Arc::clone(&approvals);
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
                    _ => {}
                }
            }
        });
        let entry = Arc::new(RpiEntry {
            session,
            state,
            approvals,
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

    fn resolve_interaction<'a>(
        &'a self,
        context: &'a ConversationContext,
        request_id: &'a str,
        option_id: &'a str,
    ) -> bots_service::conversation::RuntimeFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let entry = self.entry_for(context).await?;
            let allow = matches!(option_id, "allow_once" | "allow_always");
            if entry.session.gate.resolve(request_id, allow) {
                Ok(())
            } else {
                Err("审批请求已失效".to_string())
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
        "可用命令：\n/status 查看会话状态\n/new [工作区] 开始新会话\n/workspace 列出工作区\n/workspace <序号|id|路径> 切换\n/stop 停止当前生成\n/permission <序号> 作答审批\n/deny 拒绝当前审批\n/bind <绑定码> 绑定账号\n\n文件发送与模型/模式命令暂未接入 GPUI，请在桌面端处理。".to_string()
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
            BotCommand::Send { value } => self.handle_send_media(message, &value).await,
            BotCommand::Unknown { .. } => self.help_text(),
            _ => "该命令暂未接入 GPUI 版，请在桌面端处理。".to_string(),
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
                    "绑定码无效或已过期，请在设置 → IM 里重新生成。".to_string()
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
                        "你还没有绑定这个 Bot。请让管理员在设置 → IM 里生成绑定码，然后发送 /bind <绑定码>。"
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
                        "已登记这个群聊；管理员在设置 → IM 批准后我才会在群里回复。".to_string(),
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
