//! Bot（IM 机器人）数据层（`botsService` 管理面移植，切片 1）。
//!
//! 真源：`packages/shared/src/bots.ts`（schema）+`packages/services/src/bots/repo.ts`/
//! `config.ts`。文件（数据根 `{data_root}/v2/`）：
//! - `bot-config.v3.json`：`{version:3, bots:[BotConfig]}`；
//! - `bot-bindings.v3.json`：`{version:1|2, bindings: Record<workspaceKey, string|string[]>}`；
//! - `bot-state.v3.json`：`{version:4, bots: Record<botId, BotChannelState>}`——本切片按
//!   **不透明 JSON** 保留（运行时波次再建模），只提供删除/读改写入口。
//!
//! 本切片**不注册频道**：bots 频道仍由 Node Host 服务；运行时（三平台 channel runtime、
//! reply mirror、注册流）迁移完成后才切壳。管理面方法语义尽量与 TS 对齐：
//! 保存做 normalize（trim/去重/默认回复粒度）、删除级联状态与绑定、访问列表去重。

pub mod attachments;
pub mod bot_text;
pub mod command;
pub mod commands;
pub mod conversation;
pub mod gate;
pub mod heartbeat;
pub mod http;
pub mod inbound;
pub mod media;
pub mod memory_store;
pub mod message_queue;
pub mod mirror;
pub mod reply;
pub mod runtime;
pub mod tasks;
pub mod turn_gate;
pub mod workspace;
pub mod wecom;
pub mod wecom_payload;
pub mod wecom_protocol;
pub mod wecom_registration;
pub mod wecom_requests;
pub mod wecom_ws;
pub mod weixin;
pub mod weixin_host;
pub mod weixin_inbound;
pub mod weixin_media;
pub mod weixin_registration;
pub mod weixin_runtime;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use atomic_file::{write_private_text_atomic, WriteOptions};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const BOTS_CONFIG_FILE: &str = "bot-config.v3.json";
pub const BOTS_BINDINGS_FILE: &str = "bot-bindings.v3.json";
pub const BOTS_STATE_FILE: &str = "bot-state.v3.json";
pub const BOTS_STATE_FILE_VERSION: i64 = 4;
pub const BOT_BINDINGS_FILE_VERSION: i64 = 2;

/// 已下线通道：解析阶段直接丢弃（与 TS `stripRetiredBotProviders` 同义）。
pub const RETIRED_BOT_PROVIDERS: &[&str] = &["telegram", "webhook", "discord", "astrbot"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BotProvider {
    Telegram,
    Webhook,
    Discord,
    Astrbot,
    Feishu,
    Lark,
    Weixin,
    Wecom,
    Dingtalk,
}

impl BotProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Webhook => "webhook",
            Self::Discord => "discord",
            Self::Astrbot => "astrbot",
            Self::Feishu => "feishu",
            Self::Lark => "lark",
            Self::Weixin => "weixin",
            Self::Wecom => "wecom",
            Self::Dingtalk => "dingtalk",
        }
    }

    pub fn is_retired(self) -> bool {
        RETIRED_BOT_PROVIDERS.contains(&self.as_str())
    }

    pub fn is_feishu_family(self) -> bool {
        matches!(self, Self::Feishu | Self::Lark)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotPrivateChatMode {
    BoundUsers,
    AllUsers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BotAccessMode {
    Whitelist,
    Free,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotReplyMode {
    AssistantChanges,
    AssistantToolcallsChanges,
    SummaryChanges,
    StreamingCard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BotDeliveryMode {
    Auto,
    ToolOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BotGroupActivation {
    Disabled,
    Mention,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BotGroupPermissionStatus {
    Pending,
    Approved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotBlockedGroup {
    pub group_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
    pub platform: BotProvider,
    pub blocked_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotRegisteredUser {
    pub provider_user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub discovered_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotGroupPermission {
    pub group_id: String,
    pub group_name: String,
    pub platform: BotProvider,
    pub status: BotGroupPermissionStatus,
    pub discovered_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotGroupChat {
    pub activation: BotGroupActivation,
    /// 旧配置遗留字段：群白名单已下线，保留以便老配置继续解析。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_groups: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<BotGroupPermission>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotHeartbeatActiveHours {
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotHeartbeat {
    pub enabled: bool,
    pub interval_minutes: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_hours: Option<BotHeartbeatActiveHours>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotAllowedCommands {
    // 这些字段在 TS `DEFAULT_BOT_COMMANDS` 里恒为 true；老配置缺字段时按默认放行，
    // 不能因为一次 schema 收紧让整份 bot-config 读不出来。
    #[serde(default = "default_true")]
    pub status: bool,
    #[serde(default = "default_true")]
    pub new: bool,
    #[serde(default = "default_true")]
    pub workspace: bool,
    #[serde(default = "default_true")]
    pub model: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<bool>,
    #[serde(default = "default_true")]
    pub thought_level: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<bool>,
    /// 兼容旧 bot-config.json（`/cli` 命令已移除）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli: Option<bool>,
    /// `/reply` 命令开关（TS 必填、默认 true；漏掉字段曾让 saveBot 直接 unknown field 失败）。
    #[serde(default = "default_true")]
    pub reply: bool,
}

fn default_true() -> bool {
    true
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

impl Default for BotAllowedCommands {
    fn default() -> Self {
        Self {
            status: true,
            new: true,
            workspace: true,
            model: true,
            mode: Some(true),
            thought_level: true,
            sandbox_mode: None,
            approval_policy: None,
            cli: None,
            reply: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotCurrentOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<model_selection::ModelSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    /// 兼容旧 bot-config.json 的 CLI provider 标记（TS schema 仍接受）；保存时剥掉。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotConfig {
    pub id: String,
    pub name: String,
    pub provider: BotProvider,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_secret_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_auth_header_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feishu_app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_self_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_chat_mode: Option<BotPrivateChatMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_mode: Option<BotAccessMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_users: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_groups: Option<Vec<BotBlockedGroup>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_registry: Option<Vec<BotRegisteredUser>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wecom_bot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dingtalk_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dingtalk_use_ai_card: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dingtalk_card_template_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_users: Option<Vec<String>>,
    pub allowed_workspaces: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_chat: Option<BotGroupChat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_mode: Option<BotDeliveryMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat: Option<BotHeartbeat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_commands: Option<BotAllowedCommands>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_options: Option<BotCurrentOptions>,
    pub reply_mode: BotReplyMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotsConfigFile {
    pub version: i64,
    pub bots: Vec<BotConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotsBindingsFile {
    pub version: i64,
    pub bindings: serde_json::Map<String, Value>,
}

/// 归一化绑定表（v1 单 bot 字符串 → 数组；trim/去重/丢空）。
pub fn normalize_bindings(file: &BotsBindingsFile) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut result = std::collections::BTreeMap::new();
    for (key, value) in &file.bindings {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let mut ids: Vec<String> = Vec::new();
        let raw: Vec<&Value> = match value {
            Value::String(_) => vec![value],
            Value::Array(items) => items.iter().collect(),
            _ => Vec::new(),
        };
        for item in raw {
            if let Some(text) = item.as_str() {
                let text = text.trim();
                if !text.is_empty() && !ids.iter().any(|id| id == text) {
                    ids.push(text.to_string());
                }
            }
        }
        if !ids.is_empty() {
            result.insert(key.to_string(), ids);
        }
    }
    result
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotSaveParams {
    pub bot: BotConfig,
    #[serde(default)]
    pub credential_value: Option<String>,
    #[serde(default)]
    pub webhook_secret_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotWorkspaceBindingParams {
    pub bot_id: String,
    pub workspace_path: String,
    #[serde(default)]
    pub workspace_identity: Option<String>,
}

#[derive(Clone)]
pub struct BotsRepo {
    dir: PathBuf,
    /// 进程内**按数据根共享**的锁：壳里每处都 `new` 一个 repo，实例级锁挡不住并发 RMW。
    lock: Arc<Mutex<()>>,
}

fn shared_repo_lock(dir: &Path) -> Arc<Mutex<()>> {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Arc::clone(
        locks
            .entry(dir.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(()))),
    )
}

impl BotsRepo {
    pub fn new(data_root: &Path) -> Self {
        let dir = data_root.join("v2");
        Self {
            lock: shared_repo_lock(&dir),
            dir,
        }
    }

    fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    /// 数据根（`{data_root}`）：调用方需要读同目录的 teammates.json 等文件时用。
    pub fn data_root(&self) -> PathBuf {
        self.dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.dir.clone())
    }

    fn read_json<T: serde::de::DeserializeOwned>(&self, file: &str) -> Result<Option<T>, String> {
        let path = self.path(file);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("读取 {} 失败: {error}", path.display())),
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| format!("解析 {} 失败: {error}", path.display()))
    }

    fn write_json<T: Serialize>(&self, file: &str, value: &T) -> Result<(), String> {
        let path = self.path(file);
        let text = serde_json::to_string_pretty(value)
            .map_err(|error| format!("序列化 {file} 失败: {error}"))?;
        write_private_text_atomic(&path, &format!("{text}
"), &WriteOptions::default())
        .map_err(|error| format!("写入 {} 失败: {error}", path.display()))
    }

    pub fn read_config(&self) -> Result<BotsConfigFile, String> {
        let _guard = self.lock.lock().map_err(|_| "bots repo lock poisoned")?;
        self.read_config_locked()
    }

    fn read_config_locked(&self) -> Result<BotsConfigFile, String> {
        let Some(raw) = self.read_json::<Value>(BOTS_CONFIG_FILE)? else {
            let config = BotsConfigFile {
                version: 3,
                bots: Vec::new(),
            };
            self.write_json(BOTS_CONFIG_FILE, &config)?;
            return Ok(config);
        };
        let mut config: BotsConfigFile = serde_json::from_value(raw.clone())
            .map_err(|error| format!("解析 {BOTS_CONFIG_FILE} 失败: {error}"))?;
        let before = config.bots.len();
        config.bots.retain(|bot| !bot.provider.is_retired());
        // 已下线通道记录被丢弃时立即固化，避免磁盘上留下再也读不到的记录。
        if config.bots.len() != before {
            self.write_json(BOTS_CONFIG_FILE, &config)?;
        }
        Ok(config)
    }

    pub fn write_config(&self, config: &BotsConfigFile) -> Result<BotsConfigFile, String> {
        let _guard = self.lock.lock().map_err(|_| "bots repo lock poisoned")?;
        let mut config = config.clone();
        config.bots.retain(|bot| !bot.provider.is_retired());
        self.write_json(BOTS_CONFIG_FILE, &config)?;
        Ok(config)
    }

    pub fn mutate_config<T>(
        &self,
        mutate: impl FnOnce(&mut BotsConfigFile) -> Result<T, String>,
    ) -> Result<T, String> {
        let _guard = self.lock.lock().map_err(|_| "bots repo lock poisoned")?;
        let mut config = self.read_config_locked()?;
        let result = mutate(&mut config)?;
        config.bots.retain(|bot| !bot.provider.is_retired());
        self.write_json(BOTS_CONFIG_FILE, &config)?;
        Ok(result)
    }

    pub fn read_bindings(&self) -> Result<BotsBindingsFile, String> {
        let _guard = self.lock.lock().map_err(|_| "bots repo lock poisoned")?;
        Ok(self
            .read_json::<BotsBindingsFile>(BOTS_BINDINGS_FILE)?
            .unwrap_or(BotsBindingsFile {
                version: BOT_BINDINGS_FILE_VERSION,
                bindings: serde_json::Map::new(),
            }))
    }

    pub fn write_bindings(&self, file: &BotsBindingsFile) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|_| "bots repo lock poisoned")?;
        self.write_json(BOTS_BINDINGS_FILE, file)
    }

    /// 状态文件按不透明 JSON 保留：运行时波次再建模，本切片只支持整体读改写与删 bot。
    pub fn read_state(&self) -> Result<Value, String> {
        Ok(self
            .read_json::<Value>(BOTS_STATE_FILE)?
            .unwrap_or_else(|| serde_json::json!({ "version": BOTS_STATE_FILE_VERSION, "bots": {} })))
    }

    pub fn mutate_state<T>(
        &self,
        mutate: impl FnOnce(&mut Value) -> Result<T, String>,
    ) -> Result<T, String> {
        let _guard = self.lock.lock().map_err(|_| "bots repo lock poisoned")?;
        let mut state = self
            .read_json::<Value>(BOTS_STATE_FILE)?
            .unwrap_or_else(|| serde_json::json!({ "version": BOTS_STATE_FILE_VERSION, "bots": {} }));
        let result = mutate(&mut state)?;
        self.write_json(BOTS_STATE_FILE, &state)?;
        Ok(result)
    }
}

fn normalize_trimmed_list(values: &[String]) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() && !result.iter().any(|item| item == trimmed) {
            result.push(trimmed.to_string());
        }
    }
    result
}

fn conversation_key_parts(key: &str) -> (&'static str, String) {
    if let Some(id) = key.strip_prefix("group:") {
        ("group", id.to_string())
    } else if let Some(id) = key.strip_prefix("private:") {
        ("private", id.to_string())
    } else {
        ("private", key.to_string())
    }
}

/// TS `BotConversationSummary`：字段必须与 renderer 的读取口径一致。
fn conversation_summary(bot_id: &str, key: &str, context: Option<&Value>) -> Value {
    let (fallback_kind, fallback_id) = conversation_key_parts(key);
    let kind = context
        .and_then(|value| value.get("conversationKind"))
        .and_then(Value::as_str)
        .unwrap_or(fallback_kind);
    let id = context
        .and_then(|value| value.get("conversationId"))
        .and_then(Value::as_str)
        .unwrap_or(&fallback_id)
        .to_string();
    let task_id = context
        .filter(|value| value.get("mode").and_then(Value::as_str) == Some("task"))
        .and_then(|value| value.get("activeTaskId"))
        .and_then(Value::as_str)
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null);
    let mut summary = serde_json::json!({
        "botId": bot_id,
        "conversationKey": key,
        "conversationKind": kind,
        "conversationId": id,
        "taskId": task_id,
    });
    if let Some(label) = context
        .and_then(|value| value.get("conversationLabel"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        summary["conversationLabel"] = Value::String(label.to_string());
    }
    if let Some(workspace) = context
        .and_then(|value| value.get("workspacePath"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        summary["workspacePath"] = Value::String(workspace.to_string());
    }
    summary
}

fn normalize_reply_mode(provider: BotProvider, mode: BotReplyMode) -> BotReplyMode {
    match provider {
        // 飞书回复依赖 Card JSON 2.0 单卡更新；旧普通消息模式不能保留。
        BotProvider::Feishu | BotProvider::Lark => BotReplyMode::StreamingCard,
        // 微信 iLink 没有卡片流式；别的值原样保留。
        BotProvider::Weixin => match mode {
            BotReplyMode::StreamingCard => BotReplyMode::AssistantChanges,
            other => other,
        },
        _ => mode,
    }
}

/// 与 TS `normalizeBotConfig` 同口径：补齐所有 zod 必填字段，避免 Rust 写出 Node 读不了的配置。
fn normalize_bot(mut bot: BotConfig) -> BotConfig {
    bot.id = bot.id.trim().to_string();
    bot.name = bot.name.trim().to_string();
    bot.allowed_workspaces = normalize_trimmed_list(&bot.allowed_workspaces);
    // 空数组统一落成 "*"（bot 配置的 workspace 权限边界，spec bot-access-mode）。
    if bot.allowed_workspaces.is_empty() {
        bot.allowed_workspaces = vec!["*".to_string()];
    }
    if let Some(users) = &bot.allowed_users {
        bot.allowed_users = Some(normalize_trimmed_list(users));
    }
    if let Some(users) = &bot.blocked_users {
        bot.blocked_users = Some(normalize_trimmed_list(users));
    }
    // 与 TS `normalizeBotCommandPolicy` 同口径：mode 缺省视为开启，`cli` 是旧字段不再回写。
    let mut commands = bot.allowed_commands.take().unwrap_or_default();
    commands.mode = Some(commands.mode.unwrap_or(true));
    commands.cli = None;
    bot.allowed_commands = Some(commands);
    let mut options = bot.current_options.take().unwrap_or_default();
    options.cli = None;
    bot.current_options = Some(options);
    if bot.access_mode.is_none() {
        bot.access_mode = Some(if bot.private_chat_mode == Some(BotPrivateChatMode::AllUsers) {
            BotAccessMode::Free
        } else {
            BotAccessMode::Whitelist
        });
    }
    bot.reply_mode = normalize_reply_mode(bot.provider, bot.reply_mode);
    if bot.provider == BotProvider::Weixin {
        // 微信使用内置 iLink 地址；清掉其他 provider 的出站字段。
        bot.webhook_url = None;
    }
    bot
}

/// 工作区绑定键：identity 优先（remote workspace），否则目录。
pub fn workspace_binding_key(params: &BotWorkspaceBindingParams) -> String {
    params
        .workspace_identity
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(params.workspace_path.trim())
        .to_string()
}

/// Bot 密钥存储（宿主注入：bot-tauri 的 `credential_store`）。
///
/// 与 TS `ICredentialService` 的 save/delete 同语义：值为加密落盘，`saveBot` 只在
/// `credentialValue` 非空时写，并把 `credentialRef` 指到固定键。
pub trait BotCredentialStore: Send + Sync {
    fn save(&self, key: &str, value: &str) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

/// `buildBotCredentialKey`（TS `bot:<id>:credential`）。
pub fn bot_credential_key(bot_id: &str) -> String {
    format!("bot:{bot_id}:credential")
}

/// `buildBotWebhookSecretKey`（TS `bot:<id>:webhook-secret`）。
pub fn bot_webhook_secret_key(bot_id: &str) -> String {
    format!("bot:{bot_id}:webhook-secret")
}

pub struct BotsService {
    repo: BotsRepo,
    credentials: Option<Arc<dyn BotCredentialStore>>,
}

impl BotsService {
    pub fn new(data_root: &Path) -> Self {
        Self {
            repo: BotsRepo::new(data_root),
            credentials: None,
        }
    }

    /// 注入密钥存储；不注入时 `saveBot` 带 `credentialValue` 会显式报错（不静默丢密钥）。
    pub fn with_credentials(mut self, credentials: Arc<dyn BotCredentialStore>) -> Self {
        self.credentials = Some(credentials);
        self
    }

    fn credential_store(&self) -> Result<&Arc<dyn BotCredentialStore>, String> {
        self.credentials
            .as_ref()
            .ok_or_else(|| "Bot 密钥存储未装配（壳内凭据服务缺失）".to_string())
    }

    pub fn repo(&self) -> &BotsRepo {
        &self.repo
    }

    pub fn get_config(&self) -> Result<BotsConfigFile, String> {
        self.repo.read_config()
    }

    pub fn save_bot(&self, params: &BotSaveParams) -> Result<BotConfig, String> {
        let mut bot = normalize_bot(params.bot.clone());
        if bot.id.is_empty() {
            return Err("Bot id 不能为空".to_string());
        }
        if bot.provider.is_retired() {
            return Err(format!("已下线的 Bot 通道: {}", bot.provider.as_str()));
        }
        // 密钥先落盘再写配置：写入失败时不产生「配置指向不存在的密钥」的半截状态。
        if let Some(value) = params
            .credential_value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let key = bot_credential_key(&bot.id);
            self.credential_store()?.save(&key, value)?;
            bot.credential_ref = Some(key);
        }
        if let Some(value) = params
            .webhook_secret_value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let key = bot_webhook_secret_key(&bot.id);
            self.credential_store()?.save(&key, value)?;
            bot.webhook_secret_ref = Some(key);
        }
        self.repo.mutate_config(|config| {
            let mut bots: Vec<BotConfig> = config
                .bots
                .iter()
                .filter(|item| item.id != bot.id)
                .cloned()
                .collect();
            let saved = normalize_bot(bot.clone());
            bots.push(saved.clone());
            config.bots = bots;
            Ok(saved)
        })
    }

    /// 删除 bot：配置/状态/绑定级联 + 两类密钥清理（TS `deleteBot` 同序）。
    /// `saveConfig(config)`：逐条 normalize 后全量写（TS `normalizeConfigBots` 同口径）。
    pub fn save_config(
        &self,
        config: &BotsConfigFile,
    ) -> Result<BotsConfigFile, String> {
        let mut next = config.clone();
        next.bots = config.bots.iter().cloned().map(normalize_bot).collect();
        self.repo.write_config(&next)
    }

    pub fn list_bots(&self) -> Result<Vec<BotConfig>, String> {
        Ok(self.get_config()?.bots)
    }

    /// `getBotStates()`：所有 bot 的会话上下文摊平（按通道顺序）。
    pub fn bot_states(&self) -> Result<Vec<Value>, String> {
        let state = self.repo.read_state()?;
        let mut result = Vec::new();
        if let Some(bots) = state.get("bots").and_then(Value::as_object) {
            for channel in bots.values() {
                if let Some(conversations) = channel.get("conversations").and_then(Value::as_object) {
                    result.extend(conversations.values().cloned());
                }
            }
        }
        Ok(result)
    }

    /// `listBotConversations(botId)`：已存上下文（按 updatedAt 倒序）+ 绑定用户的私聊兜底。
    pub fn list_conversations(&self, bot_id: &str) -> Result<Vec<Value>, String> {
        let config = self.get_config()?;
        let Some(bot) = config.bots.iter().find(|bot| bot.id == bot_id) else {
            return Ok(Vec::new());
        };
        let state = self.repo.read_state()?;
        let mut conversations: Vec<(i64, String, Value)> = Vec::new();
        if let Some(map) = state
            .get("bots")
            .and_then(|bots| bots.get(bot_id))
            .and_then(|channel| channel.get("conversations"))
            .and_then(Value::as_object)
        {
            for (key, context) in map {
                let updated = context
                    .get("updatedAt")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                conversations.push((updated, key.clone(), context.clone()));
            }
        }
        conversations.sort_by_key(|(updated, _, _)| std::cmp::Reverse(*updated));
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (_, key, context) in conversations {
            if !seen.insert(key.clone()) {
                continue;
            }
            result.push(conversation_summary(bot_id, &key, Some(&context)));
        }
        if let Some(bound_user) = bot.provider_user_id.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            let key = format!("private:{bound_user}");
            if seen.insert(key.clone()) {
                result.push(conversation_summary(bot_id, &key, None));
            }
        }
        Ok(result)
    }

    pub fn delete_bot(&self, bot_id: &str) -> Result<(), String> {
        let bot_id = bot_id.trim();
        let removed = self
            .repo
            .read_config()
            .ok()
            .and_then(|config| config.bots.into_iter().find(|bot| bot.id == bot_id));
        self.repo.mutate_config(|config| {
            config.bots.retain(|bot| bot.id != bot_id);
            Ok(())
        })?;
        let bot_id_owned = bot_id.to_string();
        self.repo.mutate_state(|state| {
            if let Some(bots) = state.get_mut("bots").and_then(|value| value.as_object_mut()) {
                bots.remove(&bot_id_owned);
            }
            Ok(())
        })?;
        self.prune_bot_from_bindings(bot_id)?;
        if let Some(bot) = removed {
            self.delete_bot_secrets(&bot)?;
        }
        Ok(())
    }

    /// `removeBotSecret`：只摘密钥与绑定身份，保留 bot 条目与工作区/权限配置。
    ///
    /// 与 TS 一致：清 `credentialRef`/`webhookSecretRef`/`providerUserId`/`displayName`
    ///（`feishuAppId` 在非飞书通道保留），清运行态槽位，再删两类密钥。
    pub fn remove_bot_secret(&self, bot_id: &str) -> Result<(), String> {
        let bot_id = bot_id.trim();
        let bot = self
            .repo
            .read_config()?
            .bots
            .into_iter()
            .find(|bot| bot.id == bot_id)
            .ok_or_else(|| format!("Bot not found: {bot_id}"))?;
        let mut next = bot.clone();
        next.credential_ref = None;
        next.webhook_secret_ref = None;
        next.provider_user_id = None;
        next.display_name = None;
        if next.provider == BotProvider::Feishu {
            next.feishu_app_id = None;
        }
        next = normalize_bot(next);
        self.repo.mutate_config(|config| {
            for item in &mut config.bots {
                if item.id == bot_id {
                    *item = next.clone();
                }
            }
            Ok(())
        })?;
        self.repo.mutate_state(|state| {
            if let Some(bots) = state.get_mut("bots").and_then(|value| value.as_object_mut()) {
                bots.remove(bot_id);
            }
            Ok(())
        })?;
        self.delete_bot_secrets(&bot)
    }

    fn delete_bot_secrets(&self, bot: &BotConfig) -> Result<(), String> {
        let Ok(store) = self.credential_store() else {
            // 没装凭据服务（纯数据层用例）：没有密钥可删，不算失败。
            return Ok(());
        };
        if let Some(key) = bot.credential_ref.as_deref() {
            store.delete(key)?;
        }
        if let Some(key) = bot.webhook_secret_ref.as_deref() {
            store.delete(key)?;
        }
        Ok(())
    }

    fn prune_bot_from_bindings(&self, bot_id: &str) -> Result<(), String> {
        let mut file = self.repo.read_bindings()?;
        let mut changed = false;
        let keys: Vec<String> = file.bindings.keys().cloned().collect();
        for key in keys {
            let Some(value) = file.bindings.get_mut(&key) else {
                continue;
            };
            let kept: Vec<String> = match value {
                Value::String(text) => text
                    .split(',')
                    .map(str::trim)
                    .filter(|id| !id.is_empty() && *id != bot_id)
                    .map(str::to_string)
                    .collect(),
                Value::Array(items) => items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .map(str::trim)
                    .filter(|id| !id.is_empty() && *id != bot_id)
                    .map(str::to_string)
                    .collect(),
                _ => continue,
            };
            if kept.is_empty() {
                file.bindings.remove(&key);
                changed = true;
            } else if Value::Array(kept.iter().cloned().map(Value::String).collect::<Vec<_>>())
                != *value
            {
                *value = Value::Array(kept.into_iter().map(Value::String).collect());
                changed = true;
            }
        }
        if changed {
            file.version = BOT_BINDINGS_FILE_VERSION;
            self.repo.write_bindings(&file)?;
        }
        Ok(())
    }

    pub fn bind_bot_to_workspace(&self, params: &BotWorkspaceBindingParams) -> Result<(), String> {
        let key = workspace_binding_key(params);
        if key.is_empty() {
            return Err("workspacePath 不能为空".to_string());
        }
        let mut file = self.repo.read_bindings()?;
        let mut normalized = normalize_bindings(&file);
        let entry = normalized.entry(key.clone()).or_default();
        if !entry.iter().any(|id| id == params.bot_id.trim()) {
            entry.push(params.bot_id.trim().to_string());
        }
        file.bindings = normalized
            .into_iter()
            .map(|(key, ids)| {
                (
                    key,
                    Value::Array(ids.into_iter().map(Value::String).collect()),
                )
            })
            .collect();
        file.version = BOT_BINDINGS_FILE_VERSION;
        self.repo.write_bindings(&file)
    }

    pub fn unbind_bot_from_workspace(
        &self,
        params: &BotWorkspaceBindingParams,
    ) -> Result<(), String> {
        let key = workspace_binding_key(params);
        let mut file = self.repo.read_bindings()?;
        let mut normalized = normalize_bindings(&file);
        if let Some(ids) = normalized.get_mut(&key) {
            ids.retain(|id| id != params.bot_id.trim());
            if ids.is_empty() {
                normalized.remove(&key);
            }
        }
        file.bindings = normalized
            .into_iter()
            .map(|(key, ids)| {
                (
                    key,
                    Value::Array(ids.into_iter().map(Value::String).collect()),
                )
            })
            .collect();
        file.version = BOT_BINDINGS_FILE_VERSION;
        self.repo.write_bindings(&file)
    }

    /// 工作区 → botId[]。
    pub fn workspace_binding(
        &self,
        workspace_path: &str,
        workspace_identity: Option<&str>,
    ) -> Result<Vec<String>, String> {
        let key = workspace_identity
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(workspace_path.trim());
        let file = self.repo.read_bindings()?;
        Ok(normalize_bindings(&file).remove(key).unwrap_or_default())
    }

    /// bot → 绑定的工作区键列表。
    pub fn bot_workspace_bindings(&self, bot_id: &str) -> Result<Vec<String>, String> {
        let file = self.repo.read_bindings()?;
        Ok(normalize_bindings(&file)
            .into_iter()
            .filter(|(_, ids)| ids.iter().any(|id| id == bot_id.trim()))
            .map(|(key, _)| key)
            .collect())
    }

    pub fn set_bot_access_mode(
        &self,
        bot_id: &str,
        access_mode: BotAccessMode,
    ) -> Result<(), String> {
        self.repo.mutate_config(|config| {
            let bot = find_bot_mut(config, bot_id)?;
            bot.access_mode = Some(access_mode);
            Ok(())
        })
    }

    pub fn block_bot_user(&self, bot_id: &str, provider_user_id: &str) -> Result<(), String> {
        let user = provider_user_id.trim();
        if user.is_empty() {
            return Err("providerUserId 不能为空".to_string());
        }
        self.repo.mutate_config(|config| {
            let bot = find_bot_mut(config, bot_id)?;
            let mut blocked = bot.blocked_users.clone().unwrap_or_default();
            blocked.push(user.to_string());
            bot.blocked_users = Some(normalize_trimmed_list(&blocked));
            Ok(())
        })
    }

    pub fn unblock_bot_user(&self, bot_id: &str, provider_user_id: &str) -> Result<(), String> {
        self.repo.mutate_config(|config| {
            let bot = find_bot_mut(config, bot_id)?;
            if let Some(blocked) = &mut bot.blocked_users {
                blocked.retain(|candidate| candidate != provider_user_id.trim());
            }
            Ok(())
        })
    }

    pub fn block_bot_group(
        &self,
        bot_id: &str,
        group_id: &str,
        group_name: Option<&str>,
        platform: BotProvider,
        blocked_at: i64,
    ) -> Result<(), String> {
        let group_id = group_id.trim();
        if group_id.is_empty() {
            return Err("groupId 不能为空".to_string());
        }
        self.repo.mutate_config(|config| {
            let bot = find_bot_mut(config, bot_id)?;
            let mut groups = bot.blocked_groups.clone().unwrap_or_default();
            if groups.iter().any(|group| group.group_id == group_id) {
                // TS 幂等：重复拉黑不刷新 groupName/blockedAt。
                return Ok(());
            }
            groups.push(BotBlockedGroup {
                group_id: group_id.to_string(),
                group_name: group_name.map(str::trim).filter(|v| !v.is_empty()).map(str::to_string),
                platform,
                blocked_at,
            });
            bot.blocked_groups = Some(groups);
            Ok(())
        })
    }

    pub fn unblock_bot_group(&self, bot_id: &str, group_id: &str) -> Result<(), String> {
        self.repo.mutate_config(|config| {
            let bot = find_bot_mut(config, bot_id)?;
            if let Some(groups) = &mut bot.blocked_groups {
                groups.retain(|group| group.group_id != group_id.trim());
            }
            Ok(())
        })
    }

    pub fn approve_bot_group(&self, bot_id: &str, group_id: &str) -> Result<(), String> {
        self.set_group_status(bot_id, group_id, BotGroupPermissionStatus::Approved)
    }

    pub fn reject_bot_group(&self, bot_id: &str, group_id: &str) -> Result<(), String> {
        // 拒绝 = 从权限清单移除（与 TS reject 语义一致：不进入 approved 列表）。
        self.repo.mutate_config(|config| {
            let bot = find_bot_mut(config, bot_id)?;
            if let Some(group_chat) = &mut bot.group_chat {
                if let Some(permissions) = &mut group_chat.permissions {
                    permissions.retain(|permission| permission.group_id != group_id.trim());
                }
            }
            Ok(())
        })
    }

    pub fn remove_bot_group(&self, bot_id: &str, group_id: &str) -> Result<(), String> {
        self.reject_bot_group(bot_id, group_id)
    }

    fn set_group_status(
        &self,
        bot_id: &str,
        group_id: &str,
        status: BotGroupPermissionStatus,
    ) -> Result<(), String> {
        self.repo.mutate_config(|config| {
            let bot = find_bot_mut(config, bot_id)?;
            let group_chat = bot
                .group_chat
                .as_mut()
                .ok_or_else(|| format!("Bot 未启用群聊: {bot_id}"))?;
            let permissions = group_chat
                .permissions
                .as_mut()
                .ok_or_else(|| format!("群组不存在: {group_id}"))?;
            let permission = permissions
                .iter_mut()
                .find(|permission| permission.group_id == group_id.trim())
                .ok_or_else(|| format!("群组不存在: {group_id}"))?;
            permission.status = status;
            Ok(())
        })
    }

    pub fn remove_bot_registered_user(
        &self,
        bot_id: &str,
        provider_user_id: &str,
    ) -> Result<(), String> {
        self.repo.mutate_config(|config| {
            let bot = find_bot_mut(config, bot_id)?;
            if let Some(users) = &mut bot.user_registry {
                users.retain(|user| user.provider_user_id != provider_user_id.trim());
            }
            Ok(())
        })
    }

    /// `resetBotState(contextKey)`：key 是对话键时只删该对话；是 botId 时只清 conversations，
    /// **保留通道级游标/激活标记/去重表**（TS 语义：避免重置后重放历史消息）。
    pub fn reset_bot_state(&self, context_key: &str) -> Result<(), String> {
        let key = context_key.trim();
        if key.is_empty() {
            return Err("resetBotState 需要 botId 或 conversationKey".to_string());
        }
        let key = key.to_string();
        self.repo.mutate_state(|state| {
            let Some(bots) = state.get_mut("bots").and_then(|value| value.as_object_mut()) else {
                return Ok(());
            };
            let now = now_millis();
            if let Some(channel) = bots.get_mut(&key).and_then(|value| value.as_object_mut()) {
                channel.insert("conversations".into(), serde_json::json!({}));
                channel.insert("updatedAt".into(), serde_json::json!(now));
                return Ok(());
            }
            for channel in bots.values_mut() {
                let Some(object) = channel.as_object_mut() else { continue };
                let Some(conversations) = object
                    .get_mut("conversations")
                    .and_then(|value| value.as_object_mut())
                else {
                    continue;
                };
                if conversations.remove(&key).is_some() {
                    object.insert("updatedAt".into(), serde_json::json!(now));
                    break;
                }
            }
            Ok(())
        })
    }
}

fn find_bot_mut<'a>(
    config: &'a mut BotsConfigFile,
    bot_id: &str,
) -> Result<&'a mut BotConfig, String> {
    config
        .bots
        .iter_mut()
        .find(|bot| bot.id == bot_id.trim())
        .ok_or_else(|| format!("Bot not found: {bot_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bots-service-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn bot(id: &str, provider: BotProvider) -> BotConfig {
        BotConfig {
            id: id.to_string(),
            name: format!("bot-{id}"),
            provider,
            enabled: true,
            credential_ref: None,
            webhook_secret_ref: None,
            webhook_url: None,
            webhook_auth_header_name: None,
            feishu_app_id: None,
            provider_user_id: None,
            provider_self_id: None,
            private_chat_mode: None,
            access_mode: None,
            blocked_users: None,
            blocked_groups: None,
            user_registry: None,
            display_name: None,
            wecom_bot_id: None,
            dingtalk_client_id: None,
            dingtalk_use_ai_card: None,
            dingtalk_card_template_id: None,
            allowed_users: None,
            allowed_workspaces: vec!["  D:/ws  ".to_string(), "D:/ws".to_string()],
            group_chat: None,
            delivery_mode: None,
            heartbeat: None,
            allowed_commands: None,
            current_options: None,
            reply_mode: BotReplyMode::AssistantChanges,
        }
    }

    #[test]
    fn save_normalizes_and_upserts() {
        let base = sandbox("save");
        let service = BotsService::new(&base);
        let saved = service
            .save_bot(&BotSaveParams {
                bot: bot(" b1 ", BotProvider::Feishu),
                credential_value: None,
                webhook_secret_value: None,
            })
            .unwrap();
        assert_eq!(saved.id, "b1");
        assert_eq!(saved.allowed_workspaces, vec!["D:/ws".to_string()]);
        // 再存同名：替换而不是追加。
        let mut updated = saved.clone();
        updated.name = " 新名字 ".to_string();
        service
            .save_bot(&BotSaveParams {
                bot: updated,
                credential_value: None,
                webhook_secret_value: None,
            })
            .unwrap();
        let config = service.get_config().unwrap();
        assert_eq!(config.bots.len(), 1);
        assert_eq!(config.bots[0].name, "新名字");
        // 落盘文件：version 3 + 2 空格缩进。
        let text = std::fs::read_to_string(base.join("v2").join(BOTS_CONFIG_FILE)).unwrap();
        assert!(text.starts_with("{\n  \"version\": 3,"), "{text}");
        assert!(text.ends_with('\n'));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn retired_provider_is_dropped_on_read() {
        let base = sandbox("retired");
        let dir = base.join("v2");
        std::fs::create_dir_all(&dir).unwrap();
        let mut retired = serde_json::to_value(bot("old", BotProvider::Telegram)).unwrap();
        retired["provider"] = serde_json::json!("telegram");
        let file = serde_json::json!({
            "version": 3,
            "bots": [retired, serde_json::to_value(bot("keep", BotProvider::Weixin)).unwrap()],
        });
        std::fs::write(
            dir.join(BOTS_CONFIG_FILE),
            serde_json::to_string(&file).unwrap(),
        )
        .unwrap();
        let service = BotsService::new(&base);
        let config = service.get_config().unwrap();
        assert_eq!(config.bots.len(), 1);
        assert_eq!(config.bots[0].id, "keep");
        // 清理结果已固化到磁盘。
        let text = std::fs::read_to_string(dir.join(BOTS_CONFIG_FILE)).unwrap();
        assert!(!text.contains("telegram"), "{text}");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn delete_cascades_state_and_bindings() {
        let base = sandbox("delete");
        let service = BotsService::new(&base);
        service
            .save_bot(&BotSaveParams {
                bot: bot("b1", BotProvider::Feishu),
                credential_value: None,
                webhook_secret_value: None,
            })
            .unwrap();
        service
            .bind_bot_to_workspace(&BotWorkspaceBindingParams {
                bot_id: "b1".to_string(),
                workspace_path: "D:/ws".to_string(),
                workspace_identity: None,
            })
            .unwrap();
        service.repo.mutate_state(|state| {
            state["bots"]["b1"] = serde_json::json!({ "botId": "b1" });
            Ok(())
        }).unwrap();
        service.delete_bot("b1").unwrap();
        assert!(service.get_config().unwrap().bots.is_empty());
        assert!(service.workspace_binding("D:/ws", None).unwrap().is_empty());
        let state = service.repo.read_state().unwrap();
        assert!(state["bots"].get("b1").is_none(), "{state}");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn workspace_bindings_support_multiple_bots_and_identity_key() {
        let base = sandbox("bindings");
        let service = BotsService::new(&base);
        for bot_id in ["b1", "b2"] {
            service
                .bind_bot_to_workspace(&BotWorkspaceBindingParams {
                    bot_id: bot_id.to_string(),
                    workspace_path: "D:/ws".to_string(),
                    workspace_identity: Some("ws-identity".to_string()),
                })
                .unwrap();
        }
        assert_eq!(
            service.workspace_binding("D:/ws", Some("ws-identity")).unwrap(),
            vec!["b1".to_string(), "b2".to_string()]
        );
        assert_eq!(
            service.bot_workspace_bindings("b2").unwrap(),
            vec!["ws-identity".to_string()]
        );
        service
            .unbind_bot_from_workspace(&BotWorkspaceBindingParams {
                bot_id: "b1".to_string(),
                workspace_path: "D:/ws".to_string(),
                workspace_identity: Some("ws-identity".to_string()),
            })
            .unwrap();
        assert_eq!(
            service.workspace_binding("D:/ws", Some("ws-identity")).unwrap(),
            vec!["b2".to_string()]
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn access_lists_and_group_approval_persist() {
        let base = sandbox("access");
        let service = BotsService::new(&base);
        let mut config_bot = bot("b1", BotProvider::Feishu);
        config_bot.group_chat = Some(BotGroupChat {
            activation: BotGroupActivation::Mention,
            allowed_groups: None,
            permissions: Some(vec![BotGroupPermission {
                group_id: "g1".to_string(),
                group_name: "群一".to_string(),
                platform: BotProvider::Feishu,
                status: BotGroupPermissionStatus::Pending,
                discovered_at: 1,
                added_by: None,
            }]),
        });
        service
            .save_bot(&BotSaveParams {
                bot: config_bot,
                credential_value: None,
                webhook_secret_value: None,
            })
            .unwrap();
        service.set_bot_access_mode("b1", BotAccessMode::Whitelist).unwrap();
        service.block_bot_user("b1", " u1 ").unwrap();
        service.block_bot_user("b1", "u1").unwrap();
        service.approve_bot_group("b1", "g1").unwrap();
        service
            .block_bot_group("b1", "g2", Some("群二"), BotProvider::Feishu, 42)
            .unwrap();
        let config = service.get_config().unwrap();
        let saved = &config.bots[0];
        assert_eq!(saved.access_mode, Some(BotAccessMode::Whitelist));
        assert_eq!(saved.blocked_users.clone().unwrap(), vec!["u1".to_string()]);
        assert_eq!(
            saved.group_chat.as_ref().unwrap().permissions.as_ref().unwrap()[0].status,
            BotGroupPermissionStatus::Approved
        );
        assert_eq!(saved.blocked_groups.as_ref().unwrap()[0].blocked_at, 42);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn credential_save_requires_store_and_sets_ref() {
        let base = sandbox("credential");
        // 没有凭据服务：显式报错，不静默丢密钥。
        let service = BotsService::new(&base);
        let error = service
            .save_bot(&BotSaveParams {
                bot: bot("b1", BotProvider::Weixin),
                credential_value: Some("secret".to_string()),
                webhook_secret_value: None,
            })
            .unwrap_err();
        assert!(error.contains("凭据服务"), "{error}");

        let store = Arc::new(FakeCredentialStore::default());
        let service = service.with_credentials(store.clone());
        let saved = service
            .save_bot(&BotSaveParams {
                bot: bot("b1", BotProvider::Weixin),
                credential_value: Some("secret".to_string()),
                webhook_secret_value: Some("hook".to_string()),
            })
            .unwrap();
        assert_eq!(saved.credential_ref.as_deref(), Some("bot:b1:credential"));
        assert_eq!(
            saved.webhook_secret_ref.as_deref(),
            Some("bot:b1:webhook-secret")
        );
        assert_eq!(
            store.get("bot:b1:credential").as_deref(),
            Some("secret")
        );

        // removeBotSecret：清引用 + 删密钥，条目保留。
        service.remove_bot_secret("b1").unwrap();
        let config = service.get_config().unwrap();
        assert!(config.bots[0].credential_ref.is_none());
        assert!(config.bots[0].webhook_secret_ref.is_none());
        assert!(store.get("bot:b1:credential").is_none());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn normalize_bot_fills_ts_required_fields() {
        let base = sandbox("normalize-defaults");
        let service = BotsService::new(&base);
        let params: BotSaveParams = serde_json::from_value(serde_json::json!({
            "bot": {
                "id": "b1",
                "name": "bot",
                "provider": "weixin",
                "enabled": true,
                "allowedWorkspaces": [],
                "replyMode": "streaming_card"
            }
        }))
        .unwrap();
        let saved = service.save_bot(&params).unwrap();
        assert_eq!(saved.allowed_workspaces, vec!["*".to_string()]);
        let commands = saved.allowed_commands.expect("allowedCommands 必填");
        assert!(commands.status && commands.new && commands.workspace && commands.model);
        assert!(commands.thought_level && commands.reply);
        assert_eq!(commands.mode, Some(true));
        assert!(saved.current_options.is_some(), "currentOptions 必填");
        assert_eq!(saved.access_mode, Some(BotAccessMode::Whitelist));
        assert_eq!(
            saved.reply_mode,
            BotReplyMode::AssistantChanges,
            "微信不支持 streaming_card"
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn allowed_commands_accept_reply_and_normalize_like_ts() {
        let base = sandbox("commands-reply");
        let service = BotsService::new(&base);
        // UI 的完整载荷：`reply` 必须被接受（此前缺这个字段导致 saveBot unknown field）。
        let params: BotSaveParams = serde_json::from_value(serde_json::json!({
            "bot": {
                "id": "b1",
                "name": "bot",
                "provider": "weixin",
                "enabled": true,
                "allowedWorkspaces": [],
                "replyMode": "assistant_changes",
                "allowedCommands": {
                    "status": true,
                    "new": true,
                    "workspace": true,
                    "model": true,
                    "thoughtLevel": true,
                    "reply": false,
                    "cli": true
                }
            },
            "credentialValue": null,
            "webhookSecretValue": null
        }))
        .expect("载荷必须可解析");
        let saved = service.save_bot(&params).unwrap();
        let commands = saved.allowed_commands.expect("allowedCommands");
        assert!(!commands.reply, "reply=false 必须保留");
        assert_eq!(commands.mode, Some(true), "mode 缺省按 TS 默认开启");
        assert_eq!(commands.cli, None, "旧 cli 字段不再回写");

        // 老配置缺 `reply`：按默认 true 读出来，而不是整份配置解析失败。
        let commands: BotAllowedCommands = serde_json::from_value(serde_json::json!({
            "status": true,
            "new": true,
            "workspace": true,
            "model": true,
            "thoughtLevel": true
        }))
        .expect("缺 reply 的老配置必须可读");
        assert!(commands.reply);

        // 旧 currentOptions.cli 同样必须能读、且保存时被剥掉。
        let options: BotCurrentOptions = serde_json::from_value(serde_json::json!({
            "cli": "mode"
        }))
        .expect("旧 currentOptions.cli 必须可读");
        assert_eq!(options.cli.as_deref(), Some("mode"));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn two_repo_instances_share_one_lock() {
        let base = sandbox("shared-lock");
        let repo_a = BotsRepo::new(&base);
        let repo_b = BotsRepo::new(&base);
        repo_a
            .mutate_state(|state| {
                state["counter"] = serde_json::json!(0);
                Ok(())
            })
            .unwrap();
        let mut handles = Vec::new();
        for repo in [repo_a.clone(), repo_b.clone()] {
            handles.push(std::thread::spawn(move || {
                for _ in 0..50 {
                    repo.mutate_state(|state| {
                        let next = state.get("counter").and_then(Value::as_i64).unwrap_or(0) + 1;
                        state["counter"] = serde_json::json!(next);
                        Ok(())
                    })
                    .unwrap();
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let state = repo_a.read_state().unwrap();
        assert_eq!(state["counter"], serde_json::json!(100), "并发 RMW 不得丢更新");
        std::fs::remove_dir_all(base).ok();
    }

    #[derive(Default)]
    struct FakeCredentialStore {
        values: Mutex<std::collections::BTreeMap<String, String>>,
    }

    impl FakeCredentialStore {
        fn get(&self, key: &str) -> Option<String> {
            self.values.lock().unwrap().get(key).cloned()
        }
    }

    impl BotCredentialStore for FakeCredentialStore {
        fn save(&self, key: &str, value: &str) -> Result<(), String> {
            self.values
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), String> {
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
    }
}

#[cfg(test)]
mod reset_state_tests {
    use super::*;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bots-reset-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reset_keeps_channel_cursor_and_removes_only_target_conversation() {
        let base = sandbox("cursor");
        let service = BotsService::new(&base);
        let repo = service.repo();
        repo.mutate_state(|state| {
            state["bots"]["b1"] = serde_json::json!({
                "botId": "b1",
                "weixinGetUpdatesBuf": "cursor-1",
                "weixinActivatedAt": 123,
                "conversations": {
                    "private:u1": { "mode": "task", "activeTaskId": "t1" },
                    "private:u2": { "mode": "draft" }
                }
            });
            Ok(())
        })
        .unwrap();
        // 按对话键：只删该对话，通道游标保留。
        service.reset_bot_state("private:u1").unwrap();
        let state = repo.read_state().unwrap();
        assert_eq!(state["bots"]["b1"]["weixinGetUpdatesBuf"], "cursor-1");
        assert!(state["bots"]["b1"]["conversations"]["private:u1"].is_null());
        assert_eq!(state["bots"]["b1"]["conversations"]["private:u2"]["mode"], "draft");
        // 按 botId：清 conversations，保留游标与激活标记。
        service.reset_bot_state("b1").unwrap();
        let state = repo.read_state().unwrap();
        assert_eq!(state["bots"]["b1"]["conversations"], serde_json::json!({}));
        assert_eq!(state["bots"]["b1"]["weixinGetUpdatesBuf"], "cursor-1");
        assert_eq!(state["bots"]["b1"]["weixinActivatedAt"], 123);
        std::fs::remove_dir_all(base).ok();
    }
}

#[cfg(test)]
mod conversation_listing_tests {
    use super::*;

    #[test]
    fn list_conversations_matches_summary_contract() {
        let base = std::env::temp_dir().join(format!(
            "bots-conv-list-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let service = BotsService::new(&base);
        let params: BotSaveParams = serde_json::from_value(serde_json::json!({
            "bot": {
                "id": "b1", "name": "bot", "provider": "weixin", "enabled": true,
                "allowedWorkspaces": [], "replyMode": "assistant_changes",
                "providerUserId": "u1"
            }
        }))
        .unwrap();
        service.save_bot(&params).unwrap();
        service.repo().mutate_state(|state| {
            state["bots"]["b1"]["conversations"]["private:u1"] = serde_json::json!({
                "botId": "b1",
                "conversationKey": "private:u1",
                "conversationKind": "private",
                "conversationId": "u1",
                "workspacePath": "D:/ws",
                "mode": "task",
                "activeTaskId": "task-9",
                "updatedAt": 10
            });
            state["bots"]["b1"]["conversations"]["group:g1"] = serde_json::json!({
                "botId": "b1",
                "conversationKey": "group:g1",
                "conversationKind": "group",
                "conversationId": "g1",
                "workspacePath": "D:/ws",
                "mode": "draft",
                "activeTaskId": null,
                "updatedAt": 5
            });
            Ok(())
        })
        .unwrap();
        let list = service.list_conversations("b1").unwrap();
        assert_eq!(list.len(), 2, "{list:?}");
        assert_eq!(list[0]["conversationKey"], "private:u1");
        assert_eq!(list[0]["taskId"], "task-9");
        assert_eq!(list[0]["workspacePath"], "D:/ws");
        assert_eq!(list[1]["conversationKind"], "group");
        assert!(list[1]["taskId"].is_null());
        assert_eq!(service.bot_states().unwrap().len(), 2);
        // saveConfig 会 normalize：空 workspace → "*"。
        let config = service.get_config().unwrap();
        let mut next = config.clone();
        next.bots[0].allowed_workspaces = Vec::new();
        next.bots[0].allowed_commands = None;
        let saved = service.save_config(&next).unwrap();
        assert_eq!(saved.bots[0].allowed_workspaces, vec!["*".to_string()]);
        assert!(saved.bots[0].allowed_commands.is_some());
        std::fs::remove_dir_all(base).ok();
    }
}
