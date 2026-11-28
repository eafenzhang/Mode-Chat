//! IM 服务层：微信扫码注册、凭据、绑定码与运行态查询。
//!
//! 与主程序共用同一份 `v2/bot-config.v3.json` / `v2/credentials.json`：
//! - 扫码与长轮询复用 `bots-service` 的 Rust 实现（iLink HTTP 协议）；
//! - 凭据键 `bot:<id>:credential` 与 Node 时代完全一致，旧 token 直接可读；
//! - 设置页只通过本模块的同步入口下发异步任务，避免在 GPUI 线程里起网络请求。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bots_service::http::{
    HttpBytesResponse, HttpFuture, HttpJsonPort, HttpJsonResponse, OutboundHttpRequest,
};
use bots_service::weixin::DEFAULT_WEIXIN_ILINK_BASE_URL;
use bots_service::wecom_registration::{
    WECOM_QR_BASE_URL, WecomRegistrationPoll, begin_wecom_registration, poll_wecom_registration,
};
use bots_service::weixin_registration::{
    begin_weixin_registration, poll_weixin_registration, RegistrationPoll,
};
use bots_service::weixin_runtime::WeixinRuntimeStatus;
use bots_service::{BotCredentialStore, BotSaveParams, BotsRepo, BotsService};
use credential_store::CredentialStore;
use tokio::sync::mpsc;

use crate::im_runtime::ImRuntimeManager;

/// 运行时用的 token 读取器（同步、按 botId）。
pub type TokenLoader = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// `v2/credentials.json` 适配：键与主程序一致，值加密落盘。
pub struct TokenVault {
    data_root: PathBuf,
    store: CredentialStore,
}

impl TokenVault {
    pub fn new(data_root: &Path) -> Self {
        Self {
            data_root: data_root.to_path_buf(),
            store: CredentialStore::new(data_root),
        }
    }

    /// 按 botId 读 token：配置里的 `credentialRef` → 凭据库。
    pub fn load(&self, bot_id: &str) -> Option<String> {
        let config = BotsRepo::new(&self.data_root).read_config().ok()?;
        let bot = config.bots.iter().find(|bot| bot.id == bot_id)?;
        let key = bot.credential_ref.as_deref()?;
        self.store.load(key).ok().flatten()
    }

    /// 运行时用的 token 读取器（每次调用重读配置，换 token 不用重启）。
    pub fn loader(self: &Arc<Self>) -> TokenLoader {
        let vault = Arc::clone(self);
        Arc::new(move |bot_id: &str| vault.load(bot_id))
    }
}

impl BotCredentialStore for TokenVault {
    fn save(&self, key: &str, value: &str) -> Result<(), String> {
        self.store.save(key, value).map_err(|error| error.to_string())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.store.delete(key).map_err(|error| error.to_string())
    }
}

/// `bots_service::http::HttpJsonPort` 的 reqwest 实现（rustls，provider 首次调用安装）。
pub struct ReqwestHttpPort {
    client: reqwest::Client,
}

impl ReqwestHttpPort {
    pub fn new() -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client }
    }
}

impl Default for ReqwestHttpPort {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpJsonPort for ReqwestHttpPort {
    fn post_json<'a>(
        &'a self,
        request: &'a OutboundHttpRequest,
    ) -> HttpFuture<'a, Result<HttpJsonResponse, String>> {
        Box::pin(async move {
            let mut builder = self.client.post(&request.url).body(request.body.clone());
            for (key, value) in &request.headers {
                builder = builder.header(key, value);
            }
            let response = builder
                .timeout(request.timeout.max(Duration::from_secs(1)))
                .send()
                .await
                .map_err(|error| error.to_string())?;
            let status = response.status().as_u16();
            let body = response.text().await.map_err(|error| error.to_string())?;
            let payload = if body.trim().is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::from_str(&body).unwrap_or(serde_json::Value::Null)
            };
            Ok(HttpJsonResponse { status, payload })
        })
    }

    fn get_bytes<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        timeout: Duration,
    ) -> HttpFuture<'a, Result<HttpBytesResponse, String>> {
        Box::pin(async move {
            let mut builder = self.client.get(url);
            for (key, value) in headers {
                builder = builder.header(key, value);
            }
            let response = builder
                .timeout(timeout.max(Duration::from_secs(1)))
                .send()
                .await
                .map_err(|error| error.to_string())?;
            let status = response.status().as_u16();
            let body = response
                .bytes()
                .await
                .map_err(|error| error.to_string())?
                .to_vec();
            Ok(HttpBytesResponse { status, body })
        })
    }
}

/// 扫码渠道：微信（iLink）或企业微信（智能机器人 QR）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ImQrProvider {
    Weixin,
    Wecom,
}

/// 扫码状态：`starting`/`pending`/`scanned`/`success`/`expired`/`error`。
#[derive(Clone)]
pub struct ImQrView {
    pub provider: ImQrProvider,
    pub path: PathBuf,
    pub status: &'static str,
    pub message: Option<String>,
    pub expires_at: i64,
}

#[derive(Clone)]
pub struct ImBindView {
    pub code: String,
}

struct BindEntry {
    bot_id: String,
    workspaces: Vec<String>,
    expires_at: i64,
}

/// 一次性绑定码（进程内；10 分钟过期，消费即失效）。
#[derive(Default)]
pub struct BindCodeStore {
    entries: Mutex<HashMap<String, BindEntry>>,
}

pub const BIND_CODE_TTL_MS: i64 = 10 * 60 * 1000;

impl BindCodeStore {
    fn write(&self) -> std::sync::MutexGuard<'_, HashMap<String, BindEntry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn create(&self, bot_id: &str, workspaces: Vec<String>) -> (String, i64) {
        let expires_at = now_ms() + BIND_CODE_TTL_MS;
        let code = uuid::Uuid::new_v4().simple().to_string()[..8].to_uppercase();
        let mut entries = self.write();
        entries.retain(|_, entry| entry.expires_at > now_ms());
        entries.insert(
            code.clone(),
            BindEntry {
                bot_id: bot_id.to_string(),
                workspaces,
                expires_at,
            },
        );
        (code, expires_at)
    }

    pub fn consume(&self, code: &str) -> Option<(String, Vec<String>)> {
        let mut entries = self.write();
        let entry = entries.remove(&code.trim().to_uppercase())?;
        (entry.expires_at > now_ms()).then_some((entry.bot_id, entry.workspaces))
    }
}

struct ServiceState {
    qr: HashMap<String, ImQrView>,
    binds: HashMap<String, ImBindView>,
}

enum ImTask {
    BeginQr {
        bot_id: String,
        provider: ImQrProvider,
        generation: u64,
    },
    RefreshRuntime,
    Mirror {
        targets: Vec<bots_service::mirror::MirrorTarget>,
        text: String,
        attachment_count: usize,
    },
}

pub struct ImService {
    data_root: PathBuf,
    tx: mpsc::UnboundedSender<ImTask>,
    state: Mutex<ServiceState>,
    generations: Mutex<HashMap<String, u64>>,
    bind_codes: Arc<BindCodeStore>,
    http: Arc<ReqwestHttpPort>,
    manager: Arc<ImRuntimeManager>,
}

static SERVICE: OnceLock<Arc<ImService>> = OnceLock::new();
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// IM 运行日志：同时写 stderr 与 `{data_root}/v2/logs/im.log`（多实例时仍可追溯）。
pub fn log_line(message: impl AsRef<str>) {
    let message = message.as_ref();
    eprintln!("{message}");
    let Some(path) = LOG_PATH.get() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write as _;
        let line = format!("[{}] {}
", now_ms(), message);
        let _ = file.write_all(line.as_bytes());
    }
}

/// 在 Agent 控制面启动后调用：装配 HTTP/凭据/绑定码，并启动配置巡检测试的运行时。
pub fn start(data_root: PathBuf) -> Arc<ImService> {
    if let Some(existing) = SERVICE.get() {
        return Arc::clone(existing);
    }
    let _ = LOG_PATH.set(data_root.join("v2").join("logs").join("im.log"));
    let http = Arc::new(ReqwestHttpPort::new());
    let tokens = Arc::new(TokenVault::new(&data_root));
    let bind_codes = Arc::new(BindCodeStore::default());
    let manager = ImRuntimeManager::new(
        data_root.clone(),
        Arc::clone(&http),
        Arc::clone(&tokens),
        Arc::clone(&bind_codes),
    );
    let (tx, rx) = mpsc::unbounded_channel::<ImTask>();
    let service = Arc::new(ImService {
        data_root,
        tx,
        state: Mutex::new(ServiceState {
            qr: HashMap::new(),
            binds: HashMap::new(),
        }),
        generations: Mutex::new(HashMap::new()),
        bind_codes,
        http,
        manager,
    });
    service.manager.start_heartbeat_scheduler();
    let worker = Arc::clone(&service);
    tokio::spawn(async move {
        run_tasks(worker, rx).await;
    });
    let refresher = Arc::clone(&service.manager);
    tokio::spawn(async move {
        if let Err(error) = refresher.refresh() {
            eprintln!("[im] 运行时就绪失败：{error}");
        }
    });
    let _ = SERVICE.set(Arc::clone(&service));
    service
}

pub fn handle() -> Option<Arc<ImService>> {
    SERVICE.get().map(Arc::clone)
}

async fn run_tasks(service: Arc<ImService>, mut rx: mpsc::UnboundedReceiver<ImTask>) {
    while let Some(task) = rx.recv().await {
        match task {
            ImTask::BeginQr {
                bot_id,
                provider,
                generation,
            } => {
                let worker = Arc::clone(&service);
                tokio::spawn(async move {
                    run_qr(worker, bot_id, provider, generation).await;
                });
            }
            ImTask::RefreshRuntime => {
                if let Err(error) = service.manager.refresh() {
                    eprintln!("[im] 运行时刷新失败：{error}");
                }
            }
            ImTask::Mirror {
                targets,
                text,
                attachment_count,
            } => {
                let manager = Arc::clone(&service.manager);
                tokio::spawn(async move {
                    for target in targets {
                        if let Err(error) = manager
                            .deliver_mirror(&target, &text, attachment_count)
                            .await
                        {
                            log_line(format!(
                                "[im] 镜像投递失败 bot={} target={}：{error}",
                                target.bot_id, target.target_id
                            ));
                        }
                    }
                });
            }
        }
    }
}

impl ImService {
    fn state(&self) -> std::sync::MutexGuard<'_, ServiceState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn next_generation(&self, bot_id: &str) -> u64 {
        let mut generations = self
            .generations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let generation = generations.entry(bot_id.to_string()).or_insert(0);
        *generation += 1;
        *generation
    }

    fn is_current(&self, bot_id: &str, generation: u64) -> bool {
        self.generations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(bot_id)
            .is_some_and(|current| *current == generation)
    }

    /// 微信扫码登录：立即进入 `starting`，二维码在后台任务里生成。
    pub fn start_weixin_qr(&self, bot_id: &str) {
        self.start_qr(bot_id, ImQrProvider::Weixin);
    }

    /// 企业微信扫码创建机器人：二维码内容为企业微信 `auth_url`。
    pub fn start_wecom_qr(&self, bot_id: &str) {
        self.start_qr(bot_id, ImQrProvider::Wecom);
    }

    fn start_qr(&self, bot_id: &str, provider: ImQrProvider) {
        let generation = self.next_generation(bot_id);
        {
            let mut state = self.state();
            state.qr.insert(
                bot_id.to_string(),
                ImQrView {
                    provider,
                    path: PathBuf::new(),
                    status: "starting",
                    message: None,
                    expires_at: 0,
                },
            );
        }
        let _ = self.tx.send(ImTask::BeginQr {
            bot_id: bot_id.to_string(),
            provider,
            generation,
        });
    }

    pub fn cancel_qr(&self, bot_id: &str) {
        self.next_generation(bot_id);
        let removed = self.state().qr.remove(bot_id);
        if let Some(view) = removed
            && !view.path.as_os_str().is_empty()
        {
            let _ = std::fs::remove_file(view.path);
        }
    }

    pub fn qr_for(&self, bot_id: &str) -> Option<ImQrView> {
        self.state().qr.get(bot_id).cloned()
    }

    /// 生成绑定码（白名单模式下列表页展示，微信里发 `/bind <code>`）。
    pub fn create_bind_code(&self, bot_id: &str) -> Result<ImBindView, String> {
        let bot = BotsRepo::new(&self.data_root)
            .read_config()?
            .bots
            .into_iter()
            .find(|bot| bot.id == bot_id)
            .ok_or_else(|| format!("Bot 不存在：{bot_id}"))?;
        let (code, _expires_at) = self.bind_codes.create(bot_id, bot.allowed_workspaces);
        let view = ImBindView { code };
        self.state().binds.insert(bot_id.to_string(), view.clone());
        Ok(view)
    }

    pub fn bind_for(&self, bot_id: &str) -> Option<ImBindView> {
        self.state().binds.get(bot_id).cloned()
    }

    /// 配置/账号变更后让运行时按新配置重建轮询循环。
    /// 后台任务终态 → 给绑定 IM 用户推一条通知（后台线程跑，不阻塞调用方）。
    pub fn notify_task_finished(&self, bot_id: String, title: String, ok: bool) {
        let manager = Arc::clone(&self.manager);
        tokio::spawn(async move {
            manager.notify_task_finished(&bot_id, &title, ok).await;
        });
    }

    pub fn refresh_runtime(&self) {
        let _ = self.tx.send(ImTask::RefreshRuntime);
    }

    pub fn runtime_status(&self, bot_id: &str) -> Option<WeixinRuntimeStatus> {
        self.manager.statuses().get(bot_id)
    }

    pub fn is_running(&self, bot_id: &str) -> bool {
        self.manager.is_running(bot_id)
    }

    /// 桌面会话发给 IM 的回显：返回命中的 IM 目标数（0 = 该任务没有绑定 IM 会话）。
    pub fn mirror_desktop_message(
        &self,
        task_id: &str,
        workspace_path: &str,
        workspace_identity: Option<&str>,
        text: &str,
        attachment_count: usize,
    ) -> usize {
        if text.trim().is_empty() {
            return 0;
        }
        let targets = self.manager.mirror_targets(
            task_id,
            workspace_path,
            workspace_identity,
        );
        if targets.is_empty() {
            return 0;
        }
        let count = targets.len();
        let _ = self.tx.send(ImTask::Mirror {
            targets,
            text: text.to_string(),
            attachment_count,
        });
        count
    }
}

async fn run_qr(
    service: Arc<ImService>,
    bot_id: String,
    provider: ImQrProvider,
    generation: u64,
) {
    match provider {
        ImQrProvider::Weixin => run_weixin_qr(service, bot_id, generation).await,
        ImQrProvider::Wecom => run_wecom_qr(service, bot_id, generation).await,
    }
}

async fn run_weixin_qr(service: Arc<ImService>, bot_id: String, generation: u64) {
    let http: Arc<dyn HttpJsonPort> = Arc::clone(&service.http) as Arc<dyn HttpJsonPort>;
    let begin = match begin_weixin_registration(Arc::clone(&http), DEFAULT_WEIXIN_ILINK_BASE_URL).await
    {
        Ok(begin) => begin,
        Err(error) => {
            update_qr(&service, &bot_id, generation, |view| {
                view.status = "error";
                view.message = Some(error);
            });
            return;
        }
    };
    if !service.is_current(&bot_id, generation) {
        return;
    }
    let path = qr_file_path(&service.data_root, &bot_id, generation);
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        update_qr(&service, &bot_id, generation, |view| {
            view.status = "error";
            view.message = Some(format!("创建二维码目录失败：{error}"));
        });
        return;
    }
    if let Err(error) = render_qr_png(&begin.qr_url, &path) {
        update_qr(&service, &bot_id, generation, |view| {
            view.status = "error";
            view.message = Some(format!("二维码生成失败：{error}"));
        });
        return;
    }
    update_qr(&service, &bot_id, generation, |view| {
        view.path = path.clone();
        view.status = "pending";
        view.expires_at = begin.expires_at;
        view.message = None;
    });

    let interval = Duration::from_millis(begin.interval_ms.max(1_000));
    loop {
        tokio::time::sleep(interval).await;
        if !service.is_current(&bot_id, generation) {
            return;
        }
        if now_ms() > begin.expires_at {
            update_qr(&service, &bot_id, generation, |view| {
                view.status = "expired";
                view.message = Some("二维码已过期，点「重新扫码」刷新。".to_string());
            });
            return;
        }
        match poll_weixin_registration(Arc::clone(&http), DEFAULT_WEIXIN_ILINK_BASE_URL, &begin.qr_code)
            .await
        {
            Ok(RegistrationPoll::Pending { .. }) => {
                update_qr(&service, &bot_id, generation, |view| {
                    view.status = "pending";
                });
            }
            Ok(RegistrationPoll::Scanned { .. }) => {
                update_qr(&service, &bot_id, generation, |view| {
                    view.status = "scanned";
                    view.message = Some("已扫码，请在手机上确认登录。".to_string());
                });
            }
            Ok(RegistrationPoll::Success { bot_token, bot_id: self_id }) => {
                match save_scanned_bot(&service, &bot_id, &bot_token, self_id.as_deref()) {
                    Ok(()) => {
                        update_qr(&service, &bot_id, generation, |view| {
                            view.status = "success";
                            view.message = Some("登录成功，凭据已保存。".to_string());
                            view.path = PathBuf::new();
                        });
                        let _ = std::fs::remove_file(&path);
                        service.refresh_runtime();
                    }
                    Err(error) => {
                        update_qr(&service, &bot_id, generation, |view| {
                            view.status = "error";
                            view.message = Some(format!("保存凭据失败：{error}"));
                        });
                    }
                }
                return;
            }
            Ok(RegistrationPoll::Expired) => {
                update_qr(&service, &bot_id, generation, |view| {
                    view.status = "expired";
                    view.message = Some("二维码已过期，点「重新扫码」刷新。".to_string());
                });
                return;
            }
            Ok(RegistrationPoll::Error { message }) => {
                update_qr(&service, &bot_id, generation, |view| {
                    view.status = "error";
                    view.message = Some(message);
                });
                return;
            }
            Err(error) => {
                update_qr(&service, &bot_id, generation, |view| {
                    view.status = "error";
                    view.message = Some(error);
                });
                return;
            }
        }
    }
}

async fn run_wecom_qr(service: Arc<ImService>, bot_id: String, generation: u64) {
    let http: Arc<dyn HttpJsonPort> = Arc::clone(&service.http) as Arc<dyn HttpJsonPort>;
    let begin = match begin_wecom_registration(Arc::clone(&http), WECOM_QR_BASE_URL).await {
        Ok(begin) => begin,
        Err(error) => {
            update_qr(&service, &bot_id, generation, |view| {
                view.status = "error";
                view.message = Some(error);
            });
            return;
        }
    };
    if !service.is_current(&bot_id, generation) {
        return;
    }
    let path = qr_file_path(&service.data_root, &bot_id, generation);
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        update_qr(&service, &bot_id, generation, |view| {
            view.status = "error";
            view.message = Some(format!("创建二维码目录失败：{error}"));
        });
        return;
    }
    if let Err(error) = render_qr_png(&begin.auth_url, &path) {
        update_qr(&service, &bot_id, generation, |view| {
            view.status = "error";
            view.message = Some(format!("二维码生成失败：{error}"));
        });
        return;
    }
    update_qr(&service, &bot_id, generation, |view| {
        view.path = path.clone();
        view.status = "pending";
        view.expires_at = begin.expires_at;
        view.message = None;
    });

    let interval = Duration::from_millis(begin.interval_ms.max(1_000));
    loop {
        tokio::time::sleep(interval).await;
        if !service.is_current(&bot_id, generation) {
            return;
        }
        if now_ms() > begin.expires_at {
            update_qr(&service, &bot_id, generation, |view| {
                view.status = "expired";
                view.message = Some("二维码已过期，点「重新扫码」刷新。".to_string());
            });
            return;
        }
        match poll_wecom_registration(Arc::clone(&http), WECOM_QR_BASE_URL, &begin.scode).await {
            Ok(WecomRegistrationPoll::Pending { .. }) => {
                update_qr(&service, &bot_id, generation, |view| {
                    view.status = "pending";
                });
            }
            Ok(WecomRegistrationPoll::Scanned { .. }) => {
                update_qr(&service, &bot_id, generation, |view| {
                    view.status = "scanned";
                    view.message = Some("已扫码，请在手机上确认创建。".to_string());
                });
            }
            Ok(WecomRegistrationPoll::Success {
                bot_id: wecom_bot_id,
                secret,
            }) => {
                match save_scanned_wecom_bot(&service, &bot_id, &wecom_bot_id, &secret) {
                    Ok(()) => {
                        update_qr(&service, &bot_id, generation, |view| {
                            view.status = "success";
                            view.message = Some("创建成功，凭据已保存。".to_string());
                            view.path = PathBuf::new();
                        });
                        let _ = std::fs::remove_file(&path);
                        service.refresh_runtime();
                    }
                    Err(error) => {
                        update_qr(&service, &bot_id, generation, |view| {
                            view.status = "error";
                            view.message = Some(format!("保存凭据失败：{error}"));
                        });
                    }
                }
                return;
            }
            Ok(WecomRegistrationPoll::Expired) => {
                update_qr(&service, &bot_id, generation, |view| {
                    view.status = "expired";
                    view.message = Some("二维码已过期，点「重新扫码」刷新。".to_string());
                });
                return;
            }
            Ok(WecomRegistrationPoll::Cancelled) => {
                update_qr(&service, &bot_id, generation, |view| {
                    view.status = "expired";
                    view.message = Some("扫码已取消，可重新扫码。".to_string());
                });
                return;
            }
            Ok(WecomRegistrationPoll::Denied) => {
                update_qr(&service, &bot_id, generation, |view| {
                    view.status = "error";
                    view.message = Some("企业微信拒绝了本次授权。".to_string());
                });
                return;
            }
            Ok(WecomRegistrationPoll::Error { message }) => {
                update_qr(&service, &bot_id, generation, |view| {
                    view.status = "error";
                    view.message = Some(message);
                });
                return;
            }
            Err(error) => {
                update_qr(&service, &bot_id, generation, |view| {
                    view.status = "error";
                    view.message = Some(error);
                });
                return;
            }
        }
    }
}

fn update_qr(
    service: &ImService,
    bot_id: &str,
    generation: u64,
    apply: impl FnOnce(&mut ImQrView),
) {
    if !service.is_current(bot_id, generation) {
        return;
    }
    let mut state = service.state();
    if let Some(view) = state.qr.get_mut(bot_id) {
        apply(view);
    }
}

fn qr_file_path(data_root: &Path, bot_id: &str, generation: u64) -> PathBuf {
    let safe: String = bot_id
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' { ch } else { '_' })
        .collect();
    data_root
        .join("v2")
        .join("im-qr")
        .join(format!("{safe}-{generation}.png"))
}

fn render_qr_png(text: &str, path: &Path) -> Result<(), String> {
    const SCALE: u32 = 8;
    const QUIET: u32 = 4;
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|error| error.to_string())?;
    let width = code.width() as u32;
    let size = (width + QUIET * 2) * SCALE;
    let colors = code.to_colors();
    let mut canvas = image::RgbaImage::from_pixel(size, size, image::Rgba([255, 255, 255, 255]));
    for (index, color) in colors.iter().enumerate() {
        if *color != qrcode::Color::Dark {
            continue;
        }
        let x = index as u32 % width;
        let y = index as u32 / width;
        let left = (x + QUIET) * SCALE;
        let top = (y + QUIET) * SCALE;
        for dy in 0..SCALE {
            for dx in 0..SCALE {
                canvas.put_pixel(left + dx, top + dy, image::Rgba([17, 17, 17, 255]));
            }
        }
    }
    canvas.save(path).map_err(|error| error.to_string())
}

/// 扫码成功：把 bot_token 写进凭据库，并把自身 id 落成 `providerSelfId`。
fn save_scanned_bot(
    service: &ImService,
    bot_id: &str,
    bot_token: &str,
    self_id: Option<&str>,
) -> Result<(), String> {
    let repo = BotsRepo::new(&service.data_root);
    let config = repo.read_config()?;
    let mut bot = config
        .bots
        .into_iter()
        .find(|bot| bot.id == bot_id)
        .ok_or_else(|| format!("Bot 不存在：{bot_id}"))?;
    if let Some(self_id) = self_id.map(str::trim).filter(|value| !value.is_empty()) {
        bot.provider_self_id = Some(self_id.to_string());
        bot.display_name = Some(self_id.to_string());
    }
    if bot.name.trim().is_empty() {
        bot.name = "微信 Bot".to_string();
    }
    let service_handle = BotsService::new(&service.data_root)
        .with_credentials(Arc::new(TokenVault::new(&service.data_root)));
    service_handle
        .save_bot(&BotSaveParams {
            bot,
            credential_value: Some(bot_token.to_string()),
            webhook_secret_value: None,
        })
        .map(|_| ())
}

/// 企微扫码成功：`bot_info` 的 botid 写 `wecomBotId`，secret 进凭据库。
fn save_scanned_wecom_bot(
    service: &ImService,
    bot_id: &str,
    wecom_bot_id: &str,
    secret: &str,
) -> Result<(), String> {
    let repo = BotsRepo::new(&service.data_root);
    let config = repo.read_config()?;
    let mut bot = config
        .bots
        .into_iter()
        .find(|bot| bot.id == bot_id)
        .ok_or_else(|| format!("Bot 不存在：{bot_id}"))?;
    let wecom_bot_id = wecom_bot_id.trim();
    if wecom_bot_id.is_empty() {
        return Err("企业微信没有返回 botid".to_string());
    }
    bot.wecom_bot_id = Some(wecom_bot_id.to_string());
    bot.display_name = Some(wecom_bot_id.to_string());
    if bot.name.trim().is_empty() {
        bot.name = "企微 Bot".to_string();
    }
    let service_handle = BotsService::new(&service.data_root)
        .with_credentials(Arc::new(TokenVault::new(&service.data_root)));
    service_handle
        .save_bot(&BotSaveParams {
            bot,
            credential_value: Some(secret.to_string()),
            webhook_secret_value: None,
        })
        .map(|_| ())
}
