//! 真实名册（只读）：`{data_root}/v2/teammates.json` + `groups.json`。
//!
//! 数据根与 Mode Bot 的 `data_root.rs` 同口径：
//! - `BOT_PILOT_DATA_ROOT` 显式覆盖（试点调试用）；
//! - 否则 `MODE_DATA_BASE_DIR`（基目录）→ `{base}/.mode`；
//! - 否则 `{home}/.mode-bot/.mode`（产品默认）。
//!
//! 这里只解析展示需要的字段，不复制 bots-service 的写路径；文件缺失/损坏时返回空名册
//! 并把错误带给 UI，不让试点启动失败。

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// 模型选择（与 Agent `MessageModel` 同语义）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    pub provider_id: String,
    pub model_id: String,
    pub reasoning_level: Option<String>,
}

impl ModelSpec {
    pub fn label(&self) -> String {
        format!("{}/{}", self.provider_id, self.model_id)
    }
}

#[derive(Debug, Clone)]
pub struct Bot {
    pub id: String,
    pub name: String,
    pub role: String,
    #[allow(dead_code)] pub description: String,
    pub directory: String,
    pub is_chief: bool,
    pub is_main: bool,
    /// 隐私 Bot：会话独立存放、不参与全局搜索，也不能加入群聊。
    pub is_private: bool,
    pub pinned: bool,
    pub model: Option<ModelSpec>,
    #[allow(dead_code)] pub last_session_id: Option<String>,
    /// AGENTS.md 受管块 + persona.json 是否开启（编辑对话框切换）。
    pub identity_sync: bool,
    /// 审批策略（bot-team 矩阵的输入；非法/缺省落回 ask）。
    pub approval_level: String,
    pub approval_allowlist: Vec<String>,
}

impl Bot {
    /// 会话隔离键：与壳里 `allowedWorkspaces` 的 `mode-bot:teammate:<id>` 一致。
    pub fn identity(&self) -> String {
        format!("mode-bot:teammate:{}", self.id)
    }
}

#[derive(Debug, Clone)]
pub struct Group {
    pub id: String,
    pub name: String,
    /// 群聊共享目录；空/缺省回落成员目录。
    pub directory: Option<String>,
    pub member_ids: Vec<String>,
    pub admin_id: Option<String>,
    pub instructions: String,
    pub memory: String,
    pub responder_policy: String,
    /// 侧栏置顶（DS `room.pinned`）；只影响排序。
    pub pinned: bool,
    /// 已归档（不在侧栏出现；设置面板可恢复）。
    pub archived: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Roster {
    pub bots: Vec<Bot>,
    pub groups: Vec<Group>,
    /// hidden=true 的条目：不在侧栏出现，设置面板里可恢复显示。
    pub hidden_bots: Vec<Bot>,
    pub hidden_groups: Vec<Group>,
    /// archived=true 的群聊：同样不在侧栏，设置面板可恢复。
    pub archived_groups: Vec<Group>,
    pub data_root: PathBuf,
    #[allow(dead_code)] pub error: Option<String>,
}

/// 隐私 Bot 与 Chief 互斥：私有 Bot 不能进群、不能当管理员（与 `bot_team` 口径一致）。
fn check_chief_private(is_private: bool, is_chief: bool) -> Result<(), String> {
    if is_private && is_chief {
        return Err("隐私 Bot 不能同时是团队管理员（Chief）".to_string());
    }
    Ok(())
}

/// 团队管理员唯一：`keep_id` 当 Chief 时清掉其他人，避免群聊默认管理员歧义。
fn demote_other_chiefs(file: &mut bot_team::teammate::TeammatesFile, keep_id: &str) {
    let keep_is_chief = file
        .teammates
        .iter()
        .find(|record| record.id == keep_id)
        .is_some_and(|record| record.is_chief);
    if !keep_is_chief {
        return;
    }
    for other in &mut file.teammates {
        if other.id != keep_id {
            other.is_chief = false;
        }
    }
}

/// 新建 Bot：追加进 teammates.json（原子写，复用产品的记录模型与校验）。
/// `model_template` 来自名册首个 Bot（缺省则交给 Agent 默认）。
pub fn create_bot(
    name: &str,
    role: &str,
    directory: &str,
    model_template: Option<&ModelSpec>,
    is_private: bool,
    is_chief: bool,
) -> Result<Bot, String> {
    let name = name.trim();
    let directory = directory.trim();
    if name.is_empty() {
        return Err("名字不能为空".to_string());
    }
    if directory.is_empty() {
        return Err("工作目录不能为空".to_string());
    }
    check_chief_private(is_private, is_chief)?;
    let path = data_root().join("v2").join("teammates.json");
    let mut file = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<bot_team::teammate::TeammatesFile>(&bytes)
            .map_err(|error| format!("teammates.json 解析失败（{error}）"))?,
        Err(_) => bot_team::teammate::TeammatesFile::empty(),
    };
    let model_selection = model_template.map(|model| {
        let mut options = serde_json::Map::new();
        if let Some(level) = &model.reasoning_level {
            options.insert(
                "reasoningLevel".to_string(),
                serde_json::Value::String(level.clone()),
            );
        }
        serde_json::json!({
            "providerId": model.provider_id,
            "modelId": model.model_id,
            "options": options,
        })
    });
    let record = bot_team::teammate::TeammateRecord {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.to_string(),
        role: role.trim().to_string(),
        description: String::new(),
        directory: directory.to_string(),
        created_at: iso_now(),
        last_session_id: None,
        pinned: false,
        instructions: String::new(),
        identity_sync: false,
        is_chief,
        is_private,
        model_selection,
        approval_policy: Some(bot_team::teammate::ApprovalPolicy {
            level: "ask".to_string(),
            allowlist: Vec::new(),
        }),
        plugin_ids: Vec::new(),
        hidden: false,
        is_main: false,
    };
    let new_id = record.id.clone();
    file.teammates.push(record);
    demote_other_chiefs(&mut file, &new_id);
    let file = bot_team::teammate::validate_teammates_file(file)?;
    let value = serde_json::to_value(&file)
        .map_err(|error| format!("teammates.json 序列化失败（{error}）"))?;
    atomic_file::write_json_atomic(&path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("teammates.json 写入失败（{error}）"))?;
    let id = value["teammates"]
        .as_array()
        .and_then(|items| items.last())
        .and_then(|item| item.get("id"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let roster = Roster::load();
    roster
        .bot_by_id(&id)
        .cloned()
        .ok_or_else(|| "写入后名册里找不到新 Bot".to_string())
}

/// 官方插件 id（workspace `.mode/config.json` 的 enabledPlugins 键；与主程序同源）。
pub const PLUGIN_BROWSER: &str = "browser-use@mode-plugins-official";
pub const PLUGIN_COMPUTER: &str = "computer-use@mode-plugins-official";

/// 只读读取 workspace 插件开关：`Some((browser, computer))`；文件缺失/解析失败为 None。
pub fn read_plugin_switches(directory: &str) -> Option<(bool, bool)> {
    let path = Path::new(directory.trim())
        .join(".mode")
        .join("config.json");
    let bytes = std::fs::read(&path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let plugins = value.get("plugins")?.get("enabledPlugins")?;
    let flag = |key: &str| {
        plugins
            .get(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    };
    Some((flag(PLUGIN_BROWSER), flag(PLUGIN_COMPUTER)))
}

/// 列出 workspace 已登记的所有插件开关（按插件 id 排序；插件宿主恢复后使用）。
#[allow(dead_code)]
pub fn workspace_plugins(directory: &str) -> Vec<(String, bool)> {
    let path = Path::new(directory.trim())
        .join(".mode")
        .join("config.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    let Some(plugins) = value
        .get("plugins")
        .and_then(|plugins| plugins.get("enabledPlugins"))
        .and_then(serde_json::Value::as_object)
    else {
        return Vec::new();
    };
    let mut rows: Vec<(String, bool)> = plugins
        .iter()
        .map(|(name, enabled)| (name.clone(), enabled.as_bool().unwrap_or(false)))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// 写 workspace 插件开关：只改 `plugins.enabledPlugins` 里对应键，其余字段原样保留。
///
/// 文件不存在时**不新建**——CLI 的 workspace 配置有其它必需字段，凭空造一个可能让它
/// 启动失败；这种情况直接报错，提示先在主程序里初始化 workspace。
pub fn set_plugin_switch(directory: &str, plugin: &str, enabled: bool) -> Result<(), String> {
    let directory = directory.trim();
    if directory.is_empty() {
        return Err("该 Bot 没有工作目录".to_string());
    }
    let path = Path::new(directory).join(".mode").join("config.json");
    let bytes = std::fs::read(&path).map_err(|_| {
        format!(
            "没有读到 {}（workspace 未初始化；请用旧版 Mode Bot 创建一次会话，或手动创建 .mode/config.json）",
            path.display()
        )
    })?;
    let mut value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("config.json 解析失败（{error}）"))?;
    let root = value
        .as_object_mut()
        .ok_or_else(|| "config.json 顶层不是对象".to_string())?;
    let plugins = root
        .entry("plugins")
        .or_insert_with(|| serde_json::json!({}));
    let plugins = plugins
        .as_object_mut()
        .ok_or_else(|| "config.json 的 plugins 不是对象".to_string())?;
    let enabled_plugins = plugins
        .entry("enabledPlugins")
        .or_insert_with(|| serde_json::json!({}));
    let enabled_plugins = enabled_plugins
        .as_object_mut()
        .ok_or_else(|| "config.json 的 plugins.enabledPlugins 不是对象".to_string())?;
    enabled_plugins.insert(plugin.to_string(), serde_json::Value::Bool(enabled));
    atomic_file::write_json_atomic(&path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("config.json 写入失败（{error}）"))?;
    Ok(())
}

/// CLI storage 根：优先 `MODE_STORAGE_DIR`（宿主注入），其次跟随本项目自己的
/// 数据根覆盖（`BOT_PILOT_DATA_ROOT` / `MODE_DATA_BASE_DIR`），最后才是产品默认
/// `~/.mode-bot/.mode/cli`（单独启动试点时也要能读写同一份记忆）。
///
/// 为什么必须跟随 `BOT_PILOT_DATA_ROOT`：验收时用 `BOT_PILOT_DATA_ROOT` 指向临时目录
/// 做隔离，如果这里仍回落到 `~/.mode-bot`，`remember` 写下的记忆会静默落进用户的
/// production 数据根（v0.5.0 验收时真实发生过一次）。隔离指针必须对所有写路径生效。
fn cli_storage_dir() -> Option<String> {
    if let Some(value) = env_path("MODE_STORAGE_DIR") {
        return Some(value.to_string_lossy().into_owned());
    }
    // 数据根被显式覆盖时，记忆也放在覆盖后的根下，别再回落到 home。
    if let Some(root) = env_path("BOT_PILOT_DATA_ROOT") {
        return Some(root.join("cli").to_string_lossy().into_owned());
    }
    if let Some(base) = env_path("MODE_DATA_BASE_DIR") {
        return Some(base.join(".mode").join("cli").to_string_lossy().into_owned());
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    let path = PathBuf::from(home)
        .join(".mode-bot")
        .join(".mode")
        .join("cli");
    path.is_dir().then(|| path.to_string_lossy().into_owned())
}

/// 隐私 Bot 的工作目录集合（全局搜索按 cwd 排除旧会话文件）。
pub fn private_workspace_dirs() -> Vec<String> {
    let path = data_root().join("v2").join("teammates.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    let Ok(file) = serde_json::from_slice::<TeammatesFile>(&bytes) else {
        return Vec::new();
    };
    file.teammates
        .into_iter()
        .filter(|bot| bot.is_private)
        .map(|bot| bot.directory.trim().to_string())
        .filter(|directory| !directory.is_empty())
        .collect()
}

/// 团队记忆根（`mode-bot:team` 身份；与壳 orchestrator 同口径）。
pub fn team_memory_root() -> Option<PathBuf> {
    let storage = cli_storage_dir();
    bots_service::memory_store::resolve_memory_root(storage.as_deref(), "mode-bot:team")
}

/// 本机检测到的 Agent 引擎（本地 Agent 路线：引擎本体不迁 Rust，按 CLI/ACP 调用）。
pub struct LocalAgentInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// 检测用的可执行名（PATH 上找 `name` / `name.exe` / `.cmd` / `.bat`）。
    pub command: &'static str,
    /// 是否已接入回合执行（目前只有内嵌 pi-rust）。
    pub wired: bool,
    pub path: Option<String>,
}

const LOCAL_AGENT_CANDIDATES: &[(&str, &str, &str, bool)] = &[
    ("mode", "内置引擎（Pi-Rust）", "mode-agent", true),
    // 产品只支持这四个本地引擎（全部走 ACP 适配层）；不再探测 Claude/Codex/Gemini 等。
    ("pi-acp", "Pi（ACP）", "pi-acp", false),
    ("deepseek", "DeepSeek（ACP）", "deepseek-acp", false),
    ("hermes", "Hermes（ACP）", "hermes", false),
    ("zcode", "ZCode（ACP）", "zcode-acp-server", false),
];

fn find_command(command: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    let names = [
        command.to_string(),
        format!("{command}.exe"),
        format!("{command}.cmd"),
        format!("{command}.bat"),
    ];
    for dir in std::env::split_paths(&path) {
        for name in &names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// 探测本机 Agent 引擎（只读 PATH；用于设置 → 本地 Agent 页）。
pub fn local_agents() -> Vec<LocalAgentInfo> {
    LOCAL_AGENT_CANDIDATES
        .iter()
        .map(|(id, name, command, wired)| LocalAgentInfo {
            id,
            name,
            command,
            wired: *wired,
            path: find_command(command),
        })
        .collect()
}

/// 团队记忆上下文（与 Bot 每回合注入的是同一份；4000 字符）。
pub fn team_memory() -> String {
    let Some(root) = team_memory_root() else {
        return String::new();
    };
    bots_service::memory_store::memory_context(&root, 4_000)
}

/// 群聊记忆根（`mode-bot:group:<id>` 身份；与 WebView groupWorkspaceIdentity 同口径）。
pub fn group_memory_root(group_id: &str) -> Option<PathBuf> {
    let storage = cli_storage_dir();
    let identity = format!("mode-bot:group:{group_id}");
    bots_service::memory_store::resolve_memory_root(storage.as_deref(), &identity)
}

/// 群聊记忆：groups.json 里手写的 `memory` + 自动回顾写入的文件记忆。
/// 返回可直接塞进群 prompt 的合并文本。
pub fn group_memory_context(group_id: &str, manual: &str) -> String {
    let mut memory = manual.trim().to_string();
    let Some(root) = group_memory_root(group_id) else {
        return memory;
    };
    let file_memory = bots_service::memory_store::memory_context(&root, 4_000);
    if file_memory.trim().is_empty() {
        return memory;
    }
    if !memory.is_empty() {
        memory.push_str("\n\n");
    }
    memory.push_str("群记忆（自动回顾）：\n");
    memory.push_str(file_memory.trim());
    memory
}

/// 团队记忆条目列表（设置页用）。
pub fn team_memory_entries() -> Vec<bots_service::memory_store::MemoryEntry> {
    let Some(root) = team_memory_root() else {
        return Vec::new();
    };
    bots_service::memory_store::list_memory(&root).entries
}

/// 群聊记忆条目列表。
pub fn group_memory_entries(group_id: &str) -> Vec<bots_service::memory_store::MemoryEntry> {
    let Some(root) = group_memory_root(group_id) else {
        return Vec::new();
    };
    bots_service::memory_store::list_memory(&root).entries
}

/// 读一条团队记忆的完整内容（编辑对话框回填）。
pub fn read_team_memory(file: &str) -> Option<bots_service::memory_store::MemoryReadResponse> {
    let root = team_memory_root()?;
    bots_service::memory_store::read_memory(&root, file).ok()
}

/// 读一条群聊记忆的完整内容。
pub fn read_group_memory(
    group_id: &str,
    file: &str,
) -> Option<bots_service::memory_store::MemoryReadResponse> {
    let root = group_memory_root(group_id)?;
    bots_service::memory_store::read_memory(&root, file).ok()
}

fn save_memory_in(
    root: PathBuf,
    identity: &str,
    file: Option<&str>,
    title: &str,
    description: &str,
    body: &str,
) -> Result<(), String> {
    let existing = file.and_then(|file| bots_service::memory_store::read_memory(&root, file).ok());
    let (kind, end, pin) = existing
        .map(|entry| (entry.kind, entry.end, entry.pin))
        .unwrap_or_else(|| ("note".to_string(), None, false));
    bots_service::memory_store::write_memory(
        &root,
        bots_service::memory_store::WriteRequest {
            identity,
            file,
            title,
            description,
            kind: &kind,
            body,
            end: end.as_deref(),
            pin,
        },
    )
    .map(|_| ())
}

/// 新增/编辑一条团队记忆（`file` 为 None 表示新增；编辑时保留 kind/end/pin）。
pub fn save_team_memory(
    file: Option<&str>,
    title: &str,
    description: &str,
    body: &str,
) -> Result<(), String> {
    let root = team_memory_root().ok_or_else(|| {
        "记忆存储不可用（没有 MODE_STORAGE_DIR，默认目录 ~/.mode-bot/.mode/cli 也不存在）"
            .to_string()
    })?;
    save_memory_in(root, "mode-bot:team", file, title, description, body)
}

/// 新增/编辑一条群聊记忆。
pub fn save_group_memory(
    group_id: &str,
    file: Option<&str>,
    title: &str,
    description: &str,
    body: &str,
) -> Result<(), String> {
    let root = group_memory_root(group_id).ok_or_else(|| "记忆存储不可用".to_string())?;
    let identity = format!("mode-bot:group:{group_id}");
    save_memory_in(root, &identity, file, title, description, body)
}

/// 删除一条团队记忆。
pub fn delete_team_memory(file: &str) -> Result<(), String> {
    let root = team_memory_root().ok_or_else(|| "记忆存储不可用".to_string())?;
    bots_service::memory_store::delete_memory(&root, file)
}

/// 删除一条群聊记忆。
pub fn delete_group_memory(group_id: &str, file: &str) -> Result<(), String> {
    let root = group_memory_root(group_id).ok_or_else(|| "记忆存储不可用".to_string())?;
    bots_service::memory_store::delete_memory(&root, file)
}

fn undo_memory_in(root: &Path) -> Result<String, String> {
    let records = bots_service::memory_store::history(root, 10)?;
    let record = records
        .iter()
        .find(|record| !record.undone && record.op != "undo")
        .ok_or_else(|| "没有可撤销的记忆修改".to_string())?;
    let undone = bots_service::memory_store::undo(root, &record.id)?;
    Ok(undone.title)
}

/// 撤销最近一次未被撤销的记忆修改，返回被撤销的标题。
pub fn undo_team_memory() -> Result<String, String> {
    let root = team_memory_root().ok_or_else(|| "记忆存储不可用".to_string())?;
    undo_memory_in(&root)
}

/// 撤销最近一次群聊记忆修改。
pub fn undo_group_memory(group_id: &str) -> Result<String, String> {
    let root = group_memory_root(group_id).ok_or_else(|| "记忆存储不可用".to_string())?;
    undo_memory_in(&root)
}

// ---------------------------------------------------------------------------
// Bot 自己的记忆（v0.5.0 W1）
//
// 身份用 `mode-bot:teammate:<id>`，与壳里 `workspaceIdentity` 完全同口径；
// `resolve_memory_root` 自带 slug 回退，所以老 Bot 目录里的历史记忆**不用迁移**
// 就能被新工具/新面板读到。
// ---------------------------------------------------------------------------

/// 某个 Bot 的记忆根（`mode-bot:teammate:<id>`）。
pub fn bot_memory_root(bot_id: &str) -> Option<PathBuf> {
    let bot_id = bot_id.trim();
    if bot_id.is_empty() {
        return None;
    }
    let storage = cli_storage_dir();
    let identity = format!("mode-bot:teammate:{bot_id}");
    bots_service::memory_store::resolve_memory_root(storage.as_deref(), &identity)
}

/// 某个 Bot 的记忆条目列表（Bot 详情页 / 设置页用；契约 §2.3，UI 待接）。
#[allow(dead_code)]
pub fn bot_memory_entries(bot_id: &str) -> Vec<bots_service::memory_store::MemoryEntry> {
    let Some(root) = bot_memory_root(bot_id) else {
        return Vec::new();
    };
    bots_service::memory_store::list_memory(&root).entries
}

/// 读一条 Bot 记忆的完整内容（编辑对话框回填；契约 §2.3，UI 待接）。
#[allow(dead_code)]
pub fn read_bot_memory(
    bot_id: &str,
    file: &str,
) -> Option<bots_service::memory_store::MemoryReadResponse> {
    let root = bot_memory_root(bot_id)?;
    bots_service::memory_store::read_memory(&root, file).ok()
}

/// 新增/编辑一条 Bot 记忆（契约 §2.3，UI 待接）。
#[allow(dead_code)]
pub fn save_bot_memory(
    bot_id: &str,
    file: Option<&str>,
    title: &str,
    description: &str,
    body: &str,
) -> Result<(), String> {
    let root = bot_memory_root(bot_id).ok_or_else(|| "记忆存储不可用".to_string())?;
    let identity = format!("mode-bot:teammate:{bot_id}");
    save_memory_in(root, &identity, file, title, description, body)
}

/// 删除一条 Bot 记忆（契约 §2.3，UI 待接）。
#[allow(dead_code)]
pub fn delete_bot_memory(bot_id: &str, file: &str) -> Result<(), String> {
    let root = bot_memory_root(bot_id).ok_or_else(|| "记忆存储不可用".to_string())?;
    bots_service::memory_store::delete_memory(&root, file)
}

/// 撤销最近一次 Bot 记忆修改，返回被撤销的标题（契约 §2.3，UI 待接）。
#[allow(dead_code)]
pub fn undo_bot_memory(bot_id: &str) -> Result<String, String> {
    let root = bot_memory_root(bot_id).ok_or_else(|| "记忆存储不可用".to_string())?;
    undo_memory_in(&root)
}

/// Bot 记忆注入 prompt 的文本（预算按模型窗口算，不再写死 4000 字符）。
pub fn bot_memory_context(bot_id: &str, budget: usize) -> String {
    let Some(root) = bot_memory_root(bot_id) else {
        return String::new();
    };
    let context = bots_service::memory_store::memory_context(&root, budget);
    context.trim().to_string()
}

/// 记忆注入预算（字符）：窗口的 6%，夹在 2000–24000 之间。
///
/// 6k 字符 ≈ 1.5–3k tokens，对 128k 窗口是 2% 量级，对 8k 小窗口也不会吃掉
/// reserve（`CompactionSettings.reserve_tokens = 16384` 之下自洽）。
pub fn memory_budget(context_window: i64) -> usize {
    let window = context_window.max(1) as f64;
    ((window * 0.06) as usize).clamp(2_000, 24_000)
}

/// 群聊目标 prompt（`{data_root}/v2/groups/<gid>/goal.json`；无目标/终态返回空串）。
pub fn group_goal_prompt(group_id: &str) -> String {
    if group_id.trim().is_empty() {
        return String::new();
    }
    let path = data_root()
        .join("v2")
        .join("groups")
        .join(group_id.trim())
        .join("goal.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return String::new();
    };
    let Ok(goal) = serde_json::from_slice::<bot_team::goal::GroupGoal>(&bytes) else {
        return String::new();
    };
    bot_team::goal::render_goal_prompt(&goal).unwrap_or_default()
}

/// 编辑 Bot：按 id 改名字/角色/工作目录/身份同步（原子写；校验与新建同口径）。
/// 身份同步在名册写盘**之前**执行（与主程序顺序一致：sync 失败则记录不变）。
pub fn update_bot(
    id: &str,
    name: &str,
    role: &str,
    directory: &str,
    identity_sync: bool,
    is_private: bool,
    is_chief: bool,
) -> Result<Bot, String> {
    let name = name.trim();
    let directory = directory.trim();
    if name.is_empty() {
        return Err("名字不能为空".to_string());
    }
    if directory.is_empty() {
        return Err("工作目录不能为空".to_string());
    }
    check_chief_private(is_private, is_chief)?;
    let path = data_root().join("v2").join("teammates.json");
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("teammates.json 读取失败（{error}）"))?;
    let mut file = serde_json::from_slice::<bot_team::teammate::TeammatesFile>(&bytes)
        .map_err(|error| format!("teammates.json 解析失败（{error}）"))?;
    let Some(record) = file
        .teammates
        .iter_mut()
        .find(|record| record.id == id)
    else {
        return Err("名册里找不到该 Bot".to_string());
    };
    record.name = name.to_string();
    record.role = role.trim().to_string();
    record.directory = directory.to_string();
    record.identity_sync = identity_sync;
    record.is_private = is_private;
    record.is_chief = is_chief;
    demote_other_chiefs(&mut file, id);
    // 身份同步先行：AGENTS.md 受管块 + persona.json（名字压过 system 的「You are Mode」）。
    sync_identity_fields(id, directory, name, role, identity_sync, None)?;
    let file = bot_team::teammate::validate_teammates_file(file)?;
    let value = serde_json::to_value(&file)
        .map_err(|error| format!("teammates.json 序列化失败（{error}）"))?;
    atomic_file::write_json_atomic(&path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("teammates.json 写入失败（{error}）"))?;
    Roster::load()
        .bot_by_id(id)
        .cloned()
        .ok_or_else(|| "写入后名册里找不到该 Bot".to_string())
}

/// 删除 Bot：按 id 从名册移除（原子写；会话库不动，槽位由调用方清理）。
///
/// 隐私 Bot（v0.5.0 W3）：删除前先把 `{id,name,directory}` 登记进
/// `v2/private-archives.json`，目录继续受路径守卫保护；解除保护见
/// `privacy::release_archive`（只删登记，不动目录）。
pub fn delete_bot(id: &str) -> Result<(), String> {
    let path = data_root().join("v2").join("teammates.json");
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("teammates.json 读取失败（{error}）"))?;
    let mut file = serde_json::from_slice::<bot_team::teammate::TeammatesFile>(&bytes)
        .map_err(|error| format!("teammates.json 解析失败（{error}）"))?;
    // 先登记归档，再删名册：登记失败不阻断删除，但要留日志。
    if let Some(record) = file.teammates.iter().find(|record| record.id == id)
        && record.is_private
    {
        let (name, directory) = (record.name.clone(), record.directory.clone());
        if let Err(error) = crate::privacy::archive_private_bot(id, &name, &directory) {
            eprintln!("[privacy] 归档隐私 Bot 目录失败（{id}）：{error}");
        }
    }
    let before = file.teammates.len();
    file.teammates.retain(|record| record.id != id);
    if file.teammates.len() == before {
        return Err("名册里找不到该 Bot".to_string());
    }
    let file = bot_team::teammate::validate_teammates_file(file)?;
    let value = serde_json::to_value(&file)
        .map_err(|error| format!("teammates.json 序列化失败（{error}）"))?;
    atomic_file::write_json_atomic(&path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("teammates.json 写入失败（{error}）"))?;
    Ok(())
}

/// 创建 Bot 副本：复制配置（角色/目录/模型/审批/插件），新 id、名字加「副本」。
/// 身份同步关闭（同一目录下两个 Bot 共用 AGENTS.md 会互相覆盖）。
pub fn duplicate_bot(id: &str) -> Result<Bot, String> {
    let path = data_root().join("v2").join("teammates.json");
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("teammates.json 读取失败（{error}）"))?;
    let mut file = serde_json::from_slice::<bot_team::teammate::TeammatesFile>(&bytes)
        .map_err(|error| format!("teammates.json 解析失败（{error}）"))?;
    let Some(source) = file
        .teammates
        .iter()
        .find(|record| record.id == id)
        .cloned()
    else {
        return Err("名册里找不到该 Bot".to_string());
    };
    let mut name = format!("{} 副本", source.name.trim());
    let mut suffix = 2;
    while file.teammates.iter().any(|record| record.name == name) {
        name = format!("{} 副本 {suffix}", source.name.trim());
        suffix += 1;
    }
    let record = bot_team::teammate::TeammateRecord {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        role: source.role,
        description: source.description,
        directory: source.directory,
        created_at: iso_now(),
        last_session_id: None,
        pinned: false,
        instructions: source.instructions,
        identity_sync: false,
        is_chief: false,
        is_private: source.is_private,
        model_selection: source.model_selection,
        approval_policy: source.approval_policy,
        plugin_ids: source.plugin_ids,
        hidden: false,
        is_main: false,
    };
    let new_id = record.id.clone();
    file.teammates.push(record);
    let file = bot_team::teammate::validate_teammates_file(file)?;
    let value = serde_json::to_value(&file)
        .map_err(|error| format!("teammates.json 序列化失败（{error}）"))?;
    atomic_file::write_json_atomic(&path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("teammates.json 写入失败（{error}）"))?;
    Roster::load()
        .bot_by_id(&new_id)
        .cloned()
        .ok_or_else(|| "写入后名册里找不到副本".to_string())
}

/// 写回审批策略档位（ask/auto/full；allowlist 原样保留）。
///
/// 与 WebView 的 `ApprovalPolicySelect` 同语义：只改 `level`，不动允许清单。
pub fn set_bot_approval_level(id: &str, level: &str) -> Result<(), String> {
    bot_team::approval::parse_level(level)?;
    let path = data_root().join("v2").join("teammates.json");
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("teammates.json 读取失败（{error}）"))?;
    let mut file = serde_json::from_slice::<bot_team::teammate::TeammatesFile>(&bytes)
        .map_err(|error| format!("teammates.json 解析失败（{error}）"))?;
    let Some(record) = file.teammates.iter_mut().find(|record| record.id == id) else {
        return Err("名册里找不到该 Bot".to_string());
    };
    let allowlist = record
        .approval_policy
        .as_ref()
        .map(|policy| policy.allowlist.clone())
        .unwrap_or_default();
    record.approval_policy = Some(bot_team::teammate::ApprovalPolicy {
        level: level.trim().to_string(),
        allowlist,
    });
    let file = bot_team::teammate::validate_teammates_file(file)?;
    let value = serde_json::to_value(&file)
        .map_err(|error| format!("teammates.json 序列化失败（{error}）"))?;
    atomic_file::write_json_atomic(&path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("teammates.json 写入失败（{error}）"))?;
    Ok(())
}

/// 置顶/隐藏开关（teammates.json；原子写，校验同新建口径）。
/// `None` 表示不动该字段；hidden=true 的条目在设置面板可恢复显示。
pub fn set_bot_flags(id: &str, pinned: Option<bool>, hidden: Option<bool>) -> Result<(), String> {
    if pinned.is_none() && hidden.is_none() {
        return Ok(());
    }
    let path = data_root().join("v2").join("teammates.json");
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("teammates.json 读取失败（{error}）"))?;
    let mut file = serde_json::from_slice::<bot_team::teammate::TeammatesFile>(&bytes)
        .map_err(|error| format!("teammates.json 解析失败（{error}）"))?;
    let Some(record) = file
        .teammates
        .iter_mut()
        .find(|record| record.id == id)
    else {
        return Err("名册里找不到该 Bot".to_string());
    };
    if let Some(value) = pinned {
        record.pinned = value;
    }
    if let Some(value) = hidden {
        record.hidden = value;
    }
    let file = bot_team::teammate::validate_teammates_file(file)?;
    let value = serde_json::to_value(&file)
        .map_err(|error| format!("teammates.json 序列化失败（{error}）"))?;
    atomic_file::write_json_atomic(&path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("teammates.json 写入失败（{error}）"))?;
    Ok(())
}

/// 可选模型（来自 `{data_root}/v2/provider_config.json` 的已配置 provider）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    pub provider_id: String,
    pub provider_name: String,
    pub model_id: String,
}

/// 列出已配置 provider 下的模型（按 `providerOrder` / `modelOrder` 排序）。
/// 文件缺失或格式不认识时返回空列表——调用方据此降级成"只显示当前模型"。
pub fn available_models() -> Vec<ModelOption> {
    let path = data_root().join("v2").join("provider_config.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    let Some(config) = value.get("config") else {
        return Vec::new();
    };
    let order: Vec<String> = config
        .get("providerOrder")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let rules = config
        .get("providerConfigRules")
        .and_then(|rules| rules.get("providerRules"))
        .and_then(serde_json::Value::as_array);
    let Some(rules) = rules else {
        return Vec::new();
    };
    let mut options = Vec::new();
    for rule in rules {
        let Some(provider_id) = rule.get("providerId").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let provider_name = rule
            .get("providerName")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(provider_id)
            .to_string();
        let model_ids: Vec<String> = rule
            .get("config")
            .and_then(|config| config.get("modelOrder"))
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        for model_id in model_ids {
            options.push(ModelOption {
                provider_id: provider_id.to_string(),
                provider_name: provider_name.clone(),
                model_id,
            });
        }
    }
    // providerOrder 里没列到的 provider 排在最后（顺序稳定）。
    options.sort_by_key(|option| {
        order
            .iter()
            .position(|id| id == &option.provider_id)
            .unwrap_or(order.len())
    });
    options
}

/// 已配置 provider 摘要（设置页密钥编辑用；值永不回读）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEntry {
    pub provider_id: String,
    pub provider_name: String,
    pub access_type: String,
    pub has_api_key: bool,
    /// provider 自己的 API 地址（模板默认时为 None）。
    pub base_url: Option<String>,
}

fn provider_config_path() -> PathBuf {
    data_root().join("v2").join("provider_config.json")
}

fn provider_rules(value: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    value
        .get("config")?
        .get("providerConfigRules")?
        .get("providerRules")?
        .as_array()
}

fn provider_rules_mut(value: &mut serde_json::Value) -> Option<&mut Vec<serde_json::Value>> {
    value
        .get_mut("config")?
        .get_mut("providerConfigRules")?
        .get_mut("providerRules")?
        .as_array_mut()
}

/// 读某个 provider 的完整配置（测试连接用）。
///
/// 返回 `(base_url, api_key)`；缺任意一项都返回 None——
/// 没有 key 就没法测，没有地址就只能用模板默认值。
pub fn provider_endpoint(provider_id: &str) -> Option<(String, String)> {
    let bytes = std::fs::read(provider_config_path()).ok()?;
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    let rules = provider_rules(&value)?;
    let rule = rules.iter().find(|rule| {
        rule.get("providerId").and_then(|v| v.as_str()) == Some(provider_id)
    })?;
    let config = rule.get("config")?;
    let access = config.get("access")?;
    let api_key = access.get("apiKey").and_then(|v| v.as_str())?.trim();
    if api_key.is_empty() {
        return None;
    }
    // 地址可能挂在 provider 上，也可能在模板里；取不到就回落到官方默认。
    let base_url = config
        .get("baseUrl")
        .or_else(|| config.get("apiBase"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_base_url(provider_id));
    Some((base_url, api_key.to_string()))
}

/// 各 provider 的默认 API 地址（与官方文档一致）。
fn default_base_url(provider_id: &str) -> String {
    match provider_id {
        "deepseek" => "https://api.deepseek.com".to_string(),
        "openai" => "https://api.openai.com".to_string(),
        "anthropic" => "https://api.anthropic.com".to_string(),
        "moonshot" => "https://api.moonshot.cn".to_string(),
        "zhipu" | "zhipu-coding-plan" => "https://open.bigmodel.cn/api/paas/v4".to_string(),
        "qwen" | "dashscope" => "https://dashscope.aliyuncs.com/compatible-mode".to_string(),
        "siliconflow" => "https://api.siliconflow.cn".to_string(),
        other => format!("https://api.{other}.com"),
    }
}

/// 测试 provider 连通性：发一个最小请求，按状态码判定。
///
/// 为什么用 /chat/completions 而不是 /models：不是所有 provider 都实现
/// `/models`，但都能跑一次补全。用 1 token + 极短 prompt，成本可忽略。
pub async fn probe_provider(provider_id: &str) -> Result<String, String> {
    let Some((base_url, api_key)) = provider_endpoint(provider_id) else {
        return Err("没有配置 API Key，无法测试".to_string());
    };
    let base = base_url.trim_end_matches('/');
    // 有些 provider 的 baseUrl 已经带 /v1，避免拼成 /v1/v1。
    let url = if base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    };
    let body = serde_json::json!({
        "model": "nonexistent-probe-model",
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 1,
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|error| format!("创建 HTTP 客户端失败：{error}"))?;
    let payload = serde_json::to_vec(&body).unwrap_or_default();
    let response = match client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .body(payload)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            let detail = if error.is_timeout() {
                "请求超时（15 秒）".to_string()
            } else if error.is_connect() {
                "连接失败，地址不可达".to_string()
            } else {
                error.to_string()
            };
            return Err(format!("测试失败：{detail}"));
        }
    };
    let status = response.status();
    match status.as_u16() {
        // 2xx：通了
        200..=299 => Ok(format!("连通（HTTP {}）", status.as_u16())),
        // 400/422：key 有效，只是请求体被挑刺（模型名不存在是最常见原因）
        400 | 422 => Ok(format!(
            "连通，认证有效（HTTP {}，模型名按预期被拒）",
            status.as_u16()
        )),
        401 | 403 => Err("API Key 无效或被拒绝（HTTP 401/403）".to_string()),
        404 => Err(format!(
            "地址不存在（HTTP 404）：请检查 base URL 是否正确（当前 {url}）"
        )),
        429 => Ok("连通，但触发限流（HTTP 429）；说明认证有效".to_string()),
        other => Err(format!("服务端返回 HTTP {other}")),
    }
}

pub fn provider_entries() -> Vec<ProviderEntry> {
    let Ok(bytes) = std::fs::read(provider_config_path()) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    let Some(rules) = provider_rules(&value) else {
        return Vec::new();
    };
    let order: Vec<String> = value
        .get("config")
        .and_then(|config| config.get("providerOrder"))
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut entries: Vec<ProviderEntry> = rules
        .iter()
        .filter_map(|rule| {
            let provider_id = rule.get("providerId")?.as_str()?.to_string();
            let provider_name = rule
                .get("providerName")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&provider_id)
                .to_string();
            let access = rule.get("config").and_then(|config| config.get("access"));
            let access_type = access
                .and_then(|access| access.get("type"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let has_api_key = access
                .and_then(|access| access.get("apiKey"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|key| !key.trim().is_empty());
            let base_url = rule
                .get("config")
                .and_then(|config| config.get("api"))
                .and_then(|api| api.get("baseUrl"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            Some(ProviderEntry {
                provider_id,
                provider_name,
                access_type,
                has_api_key,
                base_url,
            })
        })
        .collect();
    entries.sort_by_key(|entry| {
        order
            .iter()
            .position(|id| id == &entry.provider_id)
            .unwrap_or(order.len())
    });
    entries
}

fn set_api_key_in_value(
    value: &mut serde_json::Value,
    provider_id: &str,
    api_key: &str,
) -> Result<(), String> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err("API Key 不能为空".to_string());
    }
    let Some(rules) = provider_rules_mut(value) else {
        return Err("provider_config.json 里没有 providerRules".to_string());
    };
    let Some(rule) = rules.iter_mut().find(|rule| {
        rule.get("providerId")
            .and_then(serde_json::Value::as_str)
            == Some(provider_id)
    }) else {
        return Err(format!("找不到 provider：{provider_id}"));
    };
    let Some(config) = rule.get_mut("config").and_then(|config| config.as_object_mut()) else {
        return Err("provider 缺少 config".to_string());
    };
    let Some(access) = config.get_mut("access").and_then(|access| access.as_object_mut()) else {
        return Err("provider 缺少 access 配置".to_string());
    };
    let access_type = access
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if access_type != "api-key" && access_type != "zhipu-coding-plan-api-key" {
        return Err(format!(
            "该 provider 使用 {access_type} 接入，暂不支持在 GPUI 改密钥"
        ));
    }
    access.insert(
        "apiKey".to_string(),
        serde_json::Value::String(api_key.to_string()),
    );
    Ok(())
}

/// 改一个 provider 的 API Key：只动 `config.access.apiKey`，其余字段原样保留。
pub fn set_provider_api_key(provider_id: &str, api_key: &str) -> Result<(), String> {
    let path = provider_config_path();
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("provider_config.json 读取失败（{error}）"))?;
    let mut value = serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|error| format!("provider_config.json 解析失败（{error}）"))?;
    set_api_key_in_value(&mut value, provider_id, api_key)?;
    atomic_file::write_json_atomic(&path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("provider_config.json 写入失败（{error}）"))
}

/// provider 下的模型（按 modelOrder）与是否启用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderModelRow {
    pub model_id: String,
    pub enabled: bool,
}

fn read_provider_value() -> Result<serde_json::Value, String> {
    let path = provider_config_path();
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("provider_config.json 读取失败（{error}）"))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("provider_config.json 解析失败（{error}）"))
}

fn write_provider_value(value: &serde_json::Value) -> Result<(), String> {
    atomic_file::write_json_atomic(
        &provider_config_path(),
        value,
        &atomic_file::WriteOptions::default(),
    )
    .map_err(|error| format!("provider_config.json 写入失败（{error}）"))
}

fn model_enabled(value: &serde_json::Value, provider_id: &str, model_id: &str) -> bool {
    let Some(rules) = value
        .get("config")
        .and_then(|config| config.get("modelConfigRules"))
        .and_then(|rules| rules.get("providerModelRules"))
        .and_then(serde_json::Value::as_array)
    else {
        return true;
    };
    rules
        .iter()
        .find(|rule| {
            rule.get("providerId").and_then(serde_json::Value::as_str) == Some(provider_id)
                && rule.get("modelId").and_then(serde_json::Value::as_str) == Some(model_id)
        })
        .and_then(|rule| rule.get("config"))
        .and_then(|config| config.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true)
}

pub fn provider_model_rows(provider_id: &str) -> Vec<ProviderModelRow> {
    let Ok(value) = read_provider_value() else {
        return Vec::new();
    };
    let Some(rule) = provider_rules(&value).and_then(|rules| {
        rules.iter().find(|rule| {
            rule.get("providerId").and_then(serde_json::Value::as_str) == Some(provider_id)
        })
    }) else {
        return Vec::new();
    };
    rule.get("config")
        .and_then(|config| config.get("modelOrder"))
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(|model_id| ProviderModelRow {
                    model_id: model_id.to_string(),
                    enabled: model_enabled(&value, provider_id, model_id),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 改 provider 自己的 API 地址（只支持 config.api.baseUrl 已存在的 provider）。
pub fn set_provider_base_url(provider_id: &str, base_url: &str) -> Result<(), String> {
    let base_url = base_url.trim();
    if !(base_url.starts_with("https://") || base_url.starts_with("http://")) {
        return Err("API 地址必须以 http:// 或 https:// 开头".to_string());
    }
    let mut value = read_provider_value()?;
    let Some(rule) = provider_rules_mut(&mut value).and_then(|rules| {
        rules.iter_mut().find(|rule| {
            rule.get("providerId").and_then(serde_json::Value::as_str) == Some(provider_id)
        })
    }) else {
        return Err(format!("找不到 provider：{provider_id}"));
    };
    let Some(api) = rule
        .get_mut("config")
        .and_then(|config| config.get_mut("api"))
        .and_then(serde_json::Value::as_object_mut)
    else {
        return Err("该 provider 使用模板默认地址，不能单独修改".to_string());
    };
    api.insert(
        "baseUrl".to_string(),
        serde_json::Value::String(base_url.to_string()),
    );
    write_provider_value(&value)
}

/// 删除一个 provider（规则 + 排序 + 模型启用规则）。
pub fn delete_provider(provider_id: &str) -> Result<(), String> {
    let mut value = read_provider_value()?;
    let Some(rules) = provider_rules_mut(&mut value) else {
        return Err("provider_config.json 里没有 providerRules".to_string());
    };
    let before = rules.len();
    rules.retain(|rule| {
        rule.get("providerId").and_then(serde_json::Value::as_str) != Some(provider_id)
    });
    if rules.len() == before {
        return Err(format!("找不到 provider：{provider_id}"));
    }
    if let Some(config) = value
        .get_mut("config")
        .and_then(|config| config.as_object_mut())
    {
        if let Some(order) = config
            .get_mut("providerOrder")
            .and_then(|order| order.as_array_mut())
        {
            order.retain(|item| item.as_str() != Some(provider_id));
        }
        if let Some(model_rules) = config
            .get_mut("modelConfigRules")
            .and_then(|rules| rules.get_mut("providerModelRules"))
            .and_then(|rules| rules.as_array_mut())
        {
            model_rules.retain(|rule| {
                rule.get("providerId").and_then(serde_json::Value::as_str) != Some(provider_id)
            });
        }
    }
    write_provider_value(&value)
}

/// 启用/停用模型：写 `modelConfigRules.providerModelRules`（停用必须有显式 false 规则）。
pub fn set_model_enabled(
    provider_id: &str,
    model_id: &str,
    enabled: bool,
) -> Result<(), String> {
    let mut value = read_provider_value()?;
    let config = value
        .get_mut("config")
        .and_then(|config| config.as_object_mut())
        .ok_or_else(|| "provider_config.json 缺少 config".to_string())?;
    let rules = config
        .entry("modelConfigRules")
        .or_insert_with(|| serde_json::json!({ "providerModelRules": [] }))
        .as_object_mut()
        .ok_or_else(|| "modelConfigRules 不是对象".to_string())?;
    let list = rules
        .entry("providerModelRules")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| "providerModelRules 不是数组".to_string())?;
    if let Some(rule) = list.iter_mut().find(|rule| {
        rule.get("providerId").and_then(serde_json::Value::as_str) == Some(provider_id)
            && rule.get("modelId").and_then(serde_json::Value::as_str) == Some(model_id)
    }) {
        let rule_config = rule
            .as_object_mut()
            .ok_or_else(|| "model rule 不是对象".to_string())?
            .entry("config")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .ok_or_else(|| "model rule config 不是对象".to_string())?;
        rule_config.insert("enabled".to_string(), serde_json::Value::Bool(enabled));
    } else if !enabled {
        list.push(serde_json::json!({
            "providerId": provider_id,
            "modelId": model_id,
            "config": { "enabled": false }
        }));
    }
    write_provider_value(&value)
}

/// 给 provider 追加一个模型（写到 modelOrder 末尾）。
pub fn add_model(provider_id: &str, model_id: &str) -> Result<(), String> {
    let mut value = read_provider_value()?;
    add_model_in(&mut value, provider_id, model_id)?;
    write_provider_value(&value)
}

/// 从 provider 的 modelOrder 里删掉一个模型。
pub fn remove_model(provider_id: &str, model_id: &str) -> Result<(), String> {
    let mut value = read_provider_value()?;
    remove_model_in(&mut value, provider_id, model_id)?;
    write_provider_value(&value)
}

/// 纯 JSON 版：把一个模型 ID 追加到 provider 的 modelOrder。
///
/// 拆出来是为了可测——文件版会读全局配置，测试并行时会互相干扰。
fn add_model_in(
    value: &mut serde_json::Value,
    provider_id: &str,
    model_id: &str,
) -> Result<(), String> {
    let model_id = model_id.trim();
    if model_id.is_empty() {
        return Err("模型 ID 不能为空".to_string());
    }
    if !model_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' || ch == ':')
    {
        return Err("模型 ID 只能包含字母、数字、- _ . :".to_string());
    }
    let Some(rule) = provider_rules_mut(value).and_then(|rules| {
        rules.iter_mut().find(|rule| {
            rule.get("providerId").and_then(serde_json::Value::as_str) == Some(provider_id)
        })
    }) else {
        return Err(format!("找不到 provider：{provider_id}"));
    };
    let Some(config) = rule.get_mut("config").and_then(|c| c.as_object_mut()) else {
        return Err("provider 缺少 config".to_string());
    };
    let order = config
        .entry("modelOrder")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| "provider 的 modelOrder 不是数组".to_string())?;
    if order.iter().any(|item| item.as_str() == Some(model_id)) {
        return Err(format!("模型已存在：{model_id}"));
    }
    order.push(serde_json::Value::String(model_id.to_string()));
    Ok(())
}

/// 纯 JSON 版：从 provider 的 modelOrder 里移除一个模型。
fn remove_model_in(
    value: &mut serde_json::Value,
    provider_id: &str,
    model_id: &str,
) -> Result<(), String> {
    let model_id = model_id.trim();
    if model_id.is_empty() {
        return Err("模型 ID 不能为空".to_string());
    }
    let Some(rule) = provider_rules_mut(value).and_then(|rules| {
        rules.iter_mut().find(|rule| {
            rule.get("providerId").and_then(serde_json::Value::as_str) == Some(provider_id)
        })
    }) else {
        return Err(format!("找不到 provider：{provider_id}"));
    };
    let Some(order) = rule
        .get_mut("config")
        .and_then(|config| config.get_mut("modelOrder"))
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Err("provider 缺少 modelOrder".to_string());
    };
    let before = order.len();
    order.retain(|item| item.as_str() != Some(model_id));
    if order.len() == before {
        return Err(format!("modelOrder 里找不到模型：{model_id}"));
    }
    clear_model_rule(value, provider_id, model_id);
    Ok(())
}

/// 删掉 `modelConfigRules.providerModelRules` 里针对该模型的规则。
fn clear_model_rule(value: &mut serde_json::Value, provider_id: &str, model_id: &str) {
    let Some(rules) = value
        .get_mut("config")
        .and_then(|config| config.get_mut("modelConfigRules"))
        .and_then(|rules| rules.get_mut("providerModelRules"))
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    rules.retain(|rule| {
        !(rule.get("providerId").and_then(serde_json::Value::as_str) == Some(provider_id)
            && rule.get("modelId").and_then(serde_json::Value::as_str) == Some(model_id))
    });
}

/// 在 provider 的 modelOrder 里上/下移一格（delta = -1 / +1）。
pub fn move_model(provider_id: &str, model_id: &str, delta: isize) -> Result<(), String> {
    let mut value = read_provider_value()?;
    let Some(rule) = provider_rules_mut(&mut value).and_then(|rules| {
        rules.iter_mut().find(|rule| {
            rule.get("providerId").and_then(serde_json::Value::as_str) == Some(provider_id)
        })
    }) else {
        return Err(format!("找不到 provider：{provider_id}"));
    };
    let order = rule
        .get_mut("config")
        .and_then(|config| config.get_mut("modelOrder"))
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| "provider 缺少 modelOrder".to_string())?;
    let Some(index) = order
        .iter()
        .position(|item| item.as_str() == Some(model_id))
    else {
        return Err(format!("modelOrder 里找不到模型：{model_id}"));
    };
    let target = index as isize + delta;
    if target < 0 || target as usize >= order.len() {
        return Ok(());
    }
    order.swap(index, target as usize);
    write_provider_value(&value)
}

/// 在 providerOrder 里上/下移一格。
pub fn move_provider(provider_id: &str, delta: isize) -> Result<(), String> {
    let mut value = read_provider_value()?;
    let order = value
        .get_mut("config")
        .and_then(|config| config.get_mut("providerOrder"))
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| "provider_config.json 缺少 providerOrder".to_string())?;
    let Some(index) = order
        .iter()
        .position(|item| item.as_str() == Some(provider_id))
    else {
        return Err(format!("providerOrder 里找不到 provider：{provider_id}"));
    };
    let target = index as isize + delta;
    if target < 0 || target as usize >= order.len() {
        return Ok(());
    }
    order.swap(index, target as usize);
    write_provider_value(&value)
}

/// 内置 provider 模板（从 mode-builtin.json 的 templateRules 读取）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTemplate {
    pub template_id: String,
    pub name: String,
    pub access_type: String,
    pub api_key_management_url: Option<String>,
    pub model_ids: Vec<String>,
    pub logo: Option<serde_json::Value>,
}

fn builtin_provider_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("MODE_BUILTIN_PROVIDER_CONFIG_FILE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let path = exe
        .parent()?
        .join("config")
        .join("provider")
        .join("mode-builtin.json");
    path.is_file().then_some(path)
}

/// 编译进二进制的 provider 模板（`assets/provider/mode-builtin.json`）。
///
/// 为什么要有内置兜底：模板文件原本只从 exe 同级的 `config/provider/`
/// 读，但 build.rs 和打包脚本都没拷它，于是 `provider_templates()` 返回空，
/// 设置页「+ 添加服务商」点了毫无反应。编译进来之后，无论开发构建、
/// 本地打包还是 CI 发版都不会再缺这个文件。
const BUILTIN_PROVIDER_JSON: &str =
    include_str!("../assets/provider/mode-builtin.json");

pub fn provider_templates() -> Vec<ProviderTemplate> {
    // 外部文件优先（用户/发行版可以覆盖模板），缺失则用内置的那份。
    let value = builtin_provider_path()
        .and_then(|path| std::fs::read(&path).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .or_else(|| serde_json::from_str::<serde_json::Value>(BUILTIN_PROVIDER_JSON).ok());
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(rules) = value
        .get("config")
        .and_then(|config| config.get("providerConfigRules"))
        .and_then(|rules| rules.get("templateRules"))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    rules
        .iter()
        .filter_map(|rule| {
            let template_id = rule.get("templateId")?.as_str()?.to_string();
            let name = rule
                .get("templateNameMap")
                .and_then(|map| map.get("zh-CN"))
                .or_else(|| rule.get("templateNameMap").and_then(|map| map.get("en-US")))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&template_id)
                .to_string();
            let config = rule.get("config")?;
            let access = config.get("access");
            let access_type = access
                .and_then(|access| access.get("type"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if access_type != "api-key" && access_type != "zhipu-coding-plan-api-key" {
                return None;
            }
            let api_key_management_url = access
                .and_then(|access| access.get("apiKeyManagementUrl"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let model_ids: Vec<String> = config
                .get("builtinModelIds")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let logo = config.get("logo").cloned();
            Some(ProviderTemplate {
                template_id,
                name,
                access_type,
                api_key_management_url,
                model_ids,
                logo,
            })
        })
        .collect()
}

/// 从内置模板新增一个 personal provider（写 providerRules + providerOrder）。
pub fn add_provider_from_template(
    template_id: &str,
    provider_id: &str,
    provider_name: &str,
    api_key: &str,
) -> Result<(), String> {
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        return Err("provider id 不能为空".to_string());
    }
    if !provider_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err("provider id 只能包含字母、数字、- 和 _".to_string());
    }
    // API Key 允许留空：面板里本来就写着「可留空」。
    // 添加 provider 和填 key 是两个独立动作——用户可能先建骨架、
    // 之后再补 key（团队协作时 key 常由别人提供）。
    // 空 key 的 provider 只是「未设置 key」，UI 会显示成待配置状态。
    let api_key = api_key.trim();
    let template = provider_templates()
        .into_iter()
        .find(|template| template.template_id == template_id)
        .ok_or_else(|| format!("找不到 provider 模板：{template_id}"))?;
    let mut value = read_provider_value()?;
    let config = value
        .get_mut("config")
        .and_then(|config| config.as_object_mut())
        .ok_or_else(|| "provider_config.json 缺少 config".to_string())?;
    let rules = config
        .entry("providerConfigRules")
        .or_insert_with(|| serde_json::json!({ "providerRules": [] }))
        .as_object_mut()
        .ok_or_else(|| "providerConfigRules 不是对象".to_string())?;
    let list = rules
        .entry("providerRules")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| "providerRules 不是数组".to_string())?;
    if list.iter().any(|rule| {
        rule.get("providerId").and_then(serde_json::Value::as_str) == Some(provider_id)
    }) {
        return Err(format!("provider id 已存在：{provider_id}"));
    }
    let mut access = serde_json::Map::new();
    access.insert(
        "type".to_string(),
        serde_json::Value::String(template.access_type.clone()),
    );
    access.insert(
        "apiKey".to_string(),
        serde_json::Value::String(api_key.to_string()),
    );
    if let Some(url) = &template.api_key_management_url {
        access.insert(
            "apiKeyManagementUrl".to_string(),
            serde_json::Value::String(url.clone()),
        );
    }
    let mut provider_config = serde_json::Map::new();
    provider_config.insert(
        "group".to_string(),
        serde_json::Value::String("standard-personal".to_string()),
    );
    provider_config.insert("access".to_string(), serde_json::Value::Object(access));
    provider_config.insert(
        "modelOrder".to_string(),
        serde_json::json!(template.model_ids),
    );
    if let Some(logo) = template.logo.clone() {
        provider_config.insert("logo".to_string(), logo);
    }
    list.push(serde_json::json!({
        "providerId": provider_id,
        "templateId": template_id,
        "providerName": provider_name.trim(),
        "config": provider_config,
    }));
    let order = config
        .entry("providerOrder")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| "providerOrder 不是数组".to_string())?;
    order.push(serde_json::Value::String(provider_id.to_string()));
    write_provider_value(&value)
}

/// 写回模型选择：`None` = 清掉 `modelSelection`（跟随 Agent 默认）。
pub fn set_bot_model(id: &str, model: Option<&ModelSpec>) -> Result<(), String> {
    let path = data_root().join("v2").join("teammates.json");
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("teammates.json 读取失败（{error}）"))?;
    let mut file = serde_json::from_slice::<bot_team::teammate::TeammatesFile>(&bytes)
        .map_err(|error| format!("teammates.json 解析失败（{error}）"))?;
    let Some(record) = file.teammates.iter_mut().find(|record| record.id == id) else {
        return Err("名册里找不到该 Bot".to_string());
    };
    record.model_selection = model.map(|model| {
        let mut options = serde_json::Map::new();
        if let Some(level) = &model.reasoning_level {
            options.insert(
                "reasoningLevel".to_string(),
                serde_json::Value::String(level.clone()),
            );
        }
        serde_json::json!({
            "providerId": model.provider_id,
            "modelId": model.model_id,
            "options": options,
        })
    });
    let file = bot_team::teammate::validate_teammates_file(file)?;
    let value = serde_json::to_value(&file)
        .map_err(|error| format!("teammates.json 序列化失败（{error}）"))?;
    atomic_file::write_json_atomic(&path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("teammates.json 写入失败（{error}）"))?;
    Ok(())
}

/// 写回审批白名单（`approvalPolicy.allowlist`；档位保持不变）。
pub fn set_bot_allowlist(id: &str, allowlist: Vec<String>) -> Result<(), String> {
    let path = data_root().join("v2").join("teammates.json");
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("teammates.json 读取失败（{error}）"))?;
    let mut file = serde_json::from_slice::<bot_team::teammate::TeammatesFile>(&bytes)
        .map_err(|error| format!("teammates.json 解析失败（{error}）"))?;
    let Some(record) = file.teammates.iter_mut().find(|record| record.id == id) else {
        return Err("名册里找不到该 Bot".to_string());
    };
    let level = record
        .approval_policy
        .as_ref()
        .map(|policy| policy.level.clone())
        .unwrap_or_else(|| "ask".to_string());
    let allowlist: Vec<String> = allowlist
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();
    record.approval_policy = Some(bot_team::teammate::ApprovalPolicy { level, allowlist });
    let file = bot_team::teammate::validate_teammates_file(file)?;
    let value = serde_json::to_value(&file)
        .map_err(|error| format!("teammates.json 序列化失败（{error}）"))?;
    atomic_file::write_json_atomic(&path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("teammates.json 写入失败（{error}）"))?;
    Ok(())
}

/// 新建/编辑群聊用的入参（校验与写盘由本模块统一做）。
#[derive(Debug, Clone)]
pub struct GroupDraft {
    pub name: String,
    pub admin_id: String,
    pub member_ids: Vec<String>,
    pub instructions: String,
    pub memory: String,
    /// `mentions-first`（默认）或 `all`（全部回应；校验层会归一成产品允许的两档之外
    /// 直传的 "all" 会被拒，故这里只允许两档 + 由调用方决定）。
    pub responder_policy: String,
}

fn groups_path() -> PathBuf {
    data_root().join("v2").join("groups.json")
}

fn read_groups() -> Result<bot_team::group::GroupsFile, String> {
    let path = groups_path();
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("groups.json 解析失败（{error}）")),
        Err(_) => Ok(bot_team::group::GroupsFile::empty()),
    }
}

fn write_groups(file: bot_team::group::GroupsFile) -> Result<(), String> {
    let file = bot_team::group::validate_groups_file(file)?;
    let value = serde_json::to_value(&file)
        .map_err(|error| format!("groups.json 序列化失败（{error}）"))?;
    atomic_file::write_json_atomic(
        &groups_path(),
        &value,
        &atomic_file::WriteOptions::default(),
    )
    .map_err(|error| format!("groups.json 写入失败（{error}）"))
}

fn draft_to_record(id: String, draft: &GroupDraft, created_at: String) -> bot_team::group::GroupRecord {
    bot_team::group::GroupRecord {
        id,
        name: draft.name.trim().to_string(),
        admin_id: draft.admin_id.trim().to_string(),
        member_ids: draft
            .member_ids
            .iter()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect(),
        instructions: draft.instructions.trim().to_string(),
        memory: draft.memory.trim().to_string(),
        directory: None,
        responder_policy: Some(draft.responder_policy.trim().to_string())
            .filter(|value| !value.is_empty()),
        approval_override: None,
        archived: false,
        pinned: false,
        hidden: false,
        color: None,
        created_at,
    }
}

/// 新建群聊（原子写；管理员必须在成员里、成员非空由校验层保证）。
pub fn create_group(draft: GroupDraft) -> Result<Group, String> {
    if draft.name.trim().is_empty() {
        return Err("群聊名不能为空".to_string());
    }
    if draft.member_ids.is_empty() {
        return Err("至少选一位成员".to_string());
    }
    let mut file = read_groups()?;
    let id = uuid::Uuid::new_v4().to_string();
    file.groups.push(draft_to_record(id.clone(), &draft, iso_now()));
    write_groups(file)?;
    Roster::load()
        .groups
        .into_iter()
        .find(|group| group.id == id)
        .ok_or_else(|| "写入后名册里找不到新群聊".to_string())
}

/// 编辑群聊：只改名字/管理员/成员/规则/记忆/回应策略，置顶/归档/隐藏保持不变。
pub fn update_group(id: &str, draft: GroupDraft) -> Result<Group, String> {
    if draft.name.trim().is_empty() {
        return Err("群聊名不能为空".to_string());
    }
    if draft.member_ids.is_empty() {
        return Err("至少选一位成员".to_string());
    }
    let mut file = read_groups()?;
    let Some(record) = file.groups.iter_mut().find(|record| record.id == id) else {
        return Err("名册里找不到该群聊".to_string());
    };
    let created_at = record.created_at.clone();
    let pinned = record.pinned;
    let archived = record.archived;
    let hidden = record.hidden;
    let approval_override = record.approval_override.clone();
    let directory = record.directory.clone();
    let mut next = draft_to_record(id.to_string(), &draft, created_at);
    next.pinned = pinned;
    next.archived = archived;
    next.hidden = hidden;
    next.approval_override = approval_override;
    next.directory = directory;
    *record = next;
    write_groups(file)?;
    Roster::load()
        .groups
        .into_iter()
        .find(|group| group.id == id)
        .ok_or_else(|| "写入后名册里找不到该群聊".to_string())
}

/// 删除群聊（groups.json 移除；成员会话与 transcript 不动）。
pub fn delete_group(id: &str) -> Result<(), String> {
    let mut file = read_groups()?;
    let before = file.groups.len();
    file.groups.retain(|record| record.id != id);
    if file.groups.len() == before {
        return Err("名册里找不到该群聊".to_string());
    }
    write_groups(file)
}

/// 归档/取消归档（只影响侧栏可见性，不动成员会话）。
pub fn set_group_archived(id: &str, archived: bool) -> Result<(), String> {
    let mut file = read_groups()?;
    let Some(record) = file.groups.iter_mut().find(|record| record.id == id) else {
        return Err("名册里找不到该群聊".to_string());
    };
    record.archived = archived;
    write_groups(file)
}

/// 置顶/隐藏开关（groups.json；语义与产品一致：置顶只影响排序，隐藏可恢复）。
pub fn set_group_flags(id: &str, pinned: Option<bool>, hidden: Option<bool>) -> Result<(), String> {
    if pinned.is_none() && hidden.is_none() {
        return Ok(());
    }
    let path = data_root().join("v2").join("groups.json");
    let bytes = std::fs::read(&path).map_err(|error| format!("groups.json 读取失败（{error}）"))?;
    let mut file = serde_json::from_slice::<bot_team::group::GroupsFile>(&bytes)
        .map_err(|error| format!("groups.json 解析失败（{error}）"))?;
    let Some(record) = file.groups.iter_mut().find(|record| record.id == id) else {
        return Err("名册里找不到该群聊".to_string());
    };
    if let Some(value) = pinned {
        record.pinned = value;
    }
    if let Some(value) = hidden {
        record.hidden = value;
    }
    let file = bot_team::group::validate_groups_file(file)?;
    let value = serde_json::to_value(&file)
        .map_err(|error| format!("groups.json 序列化失败（{error}）"))?;
    atomic_file::write_json_atomic(&path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("groups.json 写入失败（{error}）"))?;
    Ok(())
}

// ---- 身份同步（逻辑移植自壳 `src-tauri/src/identity.rs`；marker 来自 bot_team）----

use bot_team::identity::{IDENTITY_END_MARKER, IDENTITY_START_MARKER};

const MAX_AGENTS_BYTES: u64 = 256 * 1024;

/// 纯函数：把文件内容里的受管块替换成 `block`（None = 移除）。
/// 未闭合的 START marker 视为损坏：吞到文件尾再按新块重建；块外内容逐字节保留。
fn splice_block(existing: &str, block: Option<&str>) -> String {
    let mut base = String::new();
    let mut cursor = 0usize;
    while let Some(start_rel) = existing[cursor..].find(IDENTITY_START_MARKER) {
        let start = cursor + start_rel;
        let after_start = start + IDENTITY_START_MARKER.len();
        let end = match existing[after_start..].find(IDENTITY_END_MARKER) {
            Some(rel) => after_start + rel + IDENTITY_END_MARKER.len(),
            None => existing.len(),
        };
        base.push_str(&existing[cursor..start]);
        cursor = end;
    }
    base.push_str(&existing[cursor..]);

    let mut out = base.trim_end().to_string();
    if let Some(block) = block {
        let block = block.trim();
        if !block.is_empty() {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(block);
            out.push('\n');
        }
    } else if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// zh-CN 身份块文案（移植自前端 identity.ts buildIdentityBlock）。
fn build_identity_block(
    name: &str,
    role: &str,
    instructions: &str,
    team_memory: &str,
) -> String {
    let role_suffix = if role.trim().is_empty() {
        String::new()
    } else {
        format!("（{}）", role.trim())
    };
    let mut parts = vec![
        IDENTITY_START_MARKER.to_string(),
        "# 你是谁".to_string(),
        String::new(),
        format!("你是 **{name}**{role_suffix}——用户在这个工作区里长期协作的智能体队友。"),
    ];
    let description_part = String::new();
    let _ = description_part;
    parts.push(String::new());
    parts.push("## 工作方式".to_string());
    parts.push(String::new());
        parts.push(format!("- 始终以「{name}」的身份与用户协作；不要每次回复都重新自我介绍。"));
        parts.push(format!("- 被问到你是谁时，直接回答「我是{name}」。系统提示里的「Mode」是产品名，不要拿它当自己的名字。"));
        parts.push("- 用用户的语言回答。".to_string());
        parts.push("- 除非用户另有要求，工作范围限定在你绑定的工作目录内。".to_string());
    let instructions = instructions.trim();
    if !instructions.is_empty() {
        parts.push(String::new());
        parts.push("## 指令".to_string());
        parts.push(String::new());
        parts.push(instructions.to_string());
    }
    let team = team_memory.trim();
    if !team.is_empty() {
        parts.push(String::new());
        parts.push("## 团队记忆".to_string());
        parts.push(String::new());
        parts.push(
            "以下是这个团队所有 Bot 共享的事实，由用户或团队管理员维护；与用户当次要求冲突时，以当次要求为准。"
                .to_string(),
        );
        parts.push(String::new());
        parts.push(team.to_string());
    }
    parts.push(IDENTITY_END_MARKER.to_string());
    parts.join("\n")
}

/// 读 → splice → 原子写 AGENTS.md。`block=None` 只移除受管块，不删文件。
fn sync_agents_block(directory: &str, block: Option<&str>) -> Result<(), String> {
    let directory_path = Path::new(directory);
    if !directory_path.is_dir() {
        return Err(format!("工作目录不存在或不是目录：{directory}"));
    }
    let path = directory_path.join("AGENTS.md");
    let existing = match std::fs::read(&path) {
        Ok(bytes) => {
            if bytes.len() as u64 > MAX_AGENTS_BYTES {
                return Err(format!(
                    "AGENTS.md 超过 {} KB，拒绝改写：{}",
                    MAX_AGENTS_BYTES / 1024,
                    path.display()
                ));
            }
            String::from_utf8(bytes)
                .map_err(|_| format!("AGENTS.md 不是 UTF-8 文本，拒绝改写：{}", path.display()))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(format!("读取 AGENTS.md 失败（{}）：{error}", path.display()));
        }
    };
    let next = splice_block(&existing, block);
    if next == existing {
        return Ok(());
    }
    atomic_file::write_text_atomic(&path, &next, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("写入 AGENTS.md 失败（{}）：{error}", path.display()))
}

/// 写/删 persona.json（按 Bot id 分开存，同一目录多 Bot 不互相覆盖）。
fn sync_persona(directory: &str, bot_id: &str, persona: Option<(&str, &str)>) -> Result<(), String> {
    let path = Path::new(directory)
        .join(".mode")
        .join("personas")
        .join(format!("{bot_id}.json"));
    match persona {
        Some((name, role)) => {
            let name = name.trim();
            if name.is_empty() {
                return Err("身份名称为空".to_string());
            }
            let mut payload = serde_json::Map::new();
            payload.insert("name".to_string(), serde_json::Value::String(name.to_string()));
            if !role.trim().is_empty() {
                payload.insert(
                    "role".to_string(),
                    serde_json::Value::String(role.trim().to_string()),
                );
            }
            let text = format!(
                "{}\n",
                serde_json::to_string_pretty(&serde_json::Value::Object(payload))
                    .map_err(|error| format!("序列化身份失败：{error}"))?
            );
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    format!("创建 .mode 目录失败（{}）：{error}", parent.display())
                })?;
            }
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            if existing == text {
                return Ok(());
            }
            atomic_file::write_text_atomic(&path, &text, &atomic_file::WriteOptions::default())
                .map_err(|error| format!("写入 persona.json 失败（{}）：{error}", path.display()))
        }
        None => {
            if path.exists() {
                std::fs::remove_file(&path)
                    .map_err(|error| format!("删除 persona.json 失败：{error}"))?;
            }
            Ok(())
        }
    }
}

/// 开启/关闭身份同步：AGENTS.md 受管块 + persona.json 一起收口。
pub fn sync_identity_fields(
    bot_id: &str,
    directory: &str,
    name: &str,
    role: &str,
    enabled: bool,
    instructions: Option<&str>,
) -> Result<(), String> {
    let block = enabled.then(|| {
        build_identity_block(
            name,
            role,
            instructions.unwrap_or(""),
            &team_memory(),
        )
    });
    sync_agents_block(directory, block.as_deref())?;
    let persona = enabled.then(|| (name.to_string(), role.to_string()));
    sync_persona(
        directory,
        bot_id,
        persona.as_ref().map(|(name, role)| (name.as_str(), role.as_str())),
    )?;
    Ok(())
}


fn iso_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
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

impl Roster {
    pub fn load() -> Self {
        let data_root = data_root();
        let v2 = data_root.join("v2");
        let mut error = None;

        let mut bots: Vec<Bot> = Vec::new();
        let mut hidden_bots: Vec<Bot> = Vec::new();
        match read_json::<TeammatesFile>(&v2.join("teammates.json")) {
            Ok(file) => {
                for (index, item) in file.teammates.into_iter().enumerate() {
                    let hidden = item.hidden;
                    let bot = item.into_bot(index);
                    if hidden {
                        hidden_bots.push(bot);
                    } else {
                        bots.push(bot);
                    }
                }
            }
            Err(message) => {
                error = Some(format!("读取 teammates.json 失败：{message}"));
            }
        }
        // 置顶优先，其次主 Bot；稳定排序保留文件内顺序。
        bots.sort_by_key(|bot| (!bot.pinned, !bot.is_main));

        let mut groups: Vec<Group> = Vec::new();
        let mut hidden_groups: Vec<Group> = Vec::new();
        let mut archived_groups: Vec<Group> = Vec::new();
        match read_json::<GroupsFile>(&v2.join("groups.json")) {
            Ok(file) => {
                for item in file.groups {
                    let (hidden, archived) = (item.hidden, item.archived);
                    let group = item.into_group();
                    if hidden {
                        hidden_groups.push(group);
                    } else if archived {
                        archived_groups.push(group);
                    } else {
                        groups.push(group);
                    }
                }
            }
            Err(message) => {
                error.get_or_insert(format!("读取 groups.json 失败：{message}"));
            }
        }
        // 置顶优先；稳定排序保留文件内顺序。
        groups.sort_by_key(|group| !group.pinned);

        Self {
            bots,
            groups,
            hidden_bots,
            hidden_groups,
            archived_groups,
            data_root,
            error,
        }
    }

    pub fn bot_by_id(&self, id: &str) -> Option<&Bot> {
        self.bots.iter().find(|bot| bot.id == id)
    }
}

pub fn data_root() -> PathBuf {
    if let Some(dir) = env_path("BOT_PILOT_DATA_ROOT") {
        return dir;
    }
    if let Some(base) = env_path("MODE_DATA_BASE_DIR") {
        return base.join(".mode");
    }
    home_dir().join(".mode-bot").join(".mode")
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn home_dir() -> PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}（{error}）", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{}（{error}）", path.display()))
}

#[derive(Debug, Deserialize)]
struct TeammatesFile {
    #[serde(default)]
    teammates: Vec<TeammateJson>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeammateJson {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    directory: String,
    #[serde(default)]
    is_chief: bool,
    #[serde(default)]
    is_private: bool,
    #[serde(default)]
    is_main: bool,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    identity_sync: bool,
    #[serde(default)]
    model_selection: Option<ModelSelectionJson>,
    #[serde(default)]
    last_session_id: Option<String>,
    #[serde(default)]
    approval_policy: Option<ApprovalPolicyJson>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalPolicyJson {
    #[serde(default)]
    level: String,
    #[serde(default)]
    allowlist: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelSelectionJson {
    #[serde(default)]
    provider_id: String,
    #[serde(default)]
    model_id: String,
    #[serde(default)]
    options: Option<ModelOptionsJson>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelOptionsJson {
    #[serde(default)]
    reasoning_level: Option<String>,
}

impl TeammateJson {
    fn into_bot(self, index: usize) -> Bot {
        let model = self.model_selection.and_then(|selection| {
            if selection.provider_id.is_empty() || selection.model_id.is_empty() {
                return None;
            }
            Some(ModelSpec {
                provider_id: selection.provider_id,
                model_id: selection.model_id,
                reasoning_level: selection
                    .options
                    .and_then(|options| options.reasoning_level)
                    .filter(|level| !level.trim().is_empty()),
            })
        });
        let name = if self.name.trim().is_empty() {
            format!("Bot {}", index + 1)
        } else {
            self.name.trim().to_string()
        };
        Bot {
            id: self.id,
            name,
            role: self.role.trim().to_string(),
            description: self.description.trim().to_string(),
            directory: self.directory.trim().to_string(),
            is_chief: self.is_chief,
            is_private: self.is_private,
            is_main: self.is_main,
            pinned: self.pinned,
            identity_sync: self.identity_sync,
            model,
            last_session_id: self
                .last_session_id
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            approval_level: self
                .approval_policy
                .as_ref()
                .map(|policy| policy.level.trim().to_string())
                .filter(|level| !level.is_empty())
                .unwrap_or_else(|| "ask".to_string()),
            approval_allowlist: self
                .approval_policy
                .map(|policy| policy.allowlist)
                .unwrap_or_default(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct GroupsFile {
    #[serde(default)]
    groups: Vec<GroupJson>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupJson {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    directory: Option<String>,
    #[serde(default)]
    member_ids: Vec<String>,
    #[serde(default)]
    admin_id: Option<String>,
    #[serde(default)]
    instructions: String,
    #[serde(default)]
    memory: String,
    #[serde(default)]
    responder_policy: String,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    archived: bool,
}

impl GroupJson {
    fn into_group(self) -> Group {
        Group {
            id: self.id,
            directory: self
                .directory
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            name: if self.name.trim().is_empty() {
                "未命名群聊".to_string()
            } else {
                self.name.trim().to_string()
            },
            member_ids: self.member_ids,
            admin_id: self.admin_id,
            instructions: self.instructions.trim().to_string(),
            memory: self.memory.trim().to_string(),
            responder_policy: self.responder_policy,
            pinned: self.pinned,
            archived: self.archived,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归：provider 模板必须始终可用。
    ///
    /// 曾经模板只从 exe 同级的 `config/provider/` 读，但 build.rs 和打包
    /// 脚本都没拷这个文件 -> 模板列表为空 -> 「+ 添加服务商」点了没反应。
    /// 现在模板编译进二进制做兜底，这条测试保证它不会再次变空。
    #[test]
    fn provider_templates_are_always_available() {
        let templates = provider_templates();
        assert!(
            !templates.is_empty(),
            "模板列表为空：内置 provider 模板丢了，添加服务商会失效"
        );
        assert!(
            templates.len() >= 10,
            "只剩 {} 个模板，明显不完整",
            templates.len()
        );
        for template in &templates {
            assert!(!template.template_id.is_empty(), "模板缺 templateId");
            assert!(!template.name.is_empty(), "模板缺显示名");
            assert!(
                template.access_type == "api-key"
                    || template.access_type == "zhipu-coding-plan-api-key",
                "模板 {} 的 access 类型不可用：{}",
                template.template_id,
                template.access_type
            );
        }
    }

    /// 模型增删：直接测纯 JSON 版本，不碰文件、不设环境变量，
    /// 因此可以和其它测试并行跑。
    #[test]
    fn add_and_remove_model_roundtrip() {
        let mut value = serde_json::json!({
            "config": {
                "providerConfigRules": {
                    "providerRules": [{
                        "providerId": "probe",
                        "providerName": "Probe",
                        "config": {
                            "access": { "type": "api-key", "apiKey": "k" },
                            "modelOrder": ["existing"]
                        }
                    }]
                },
                "modelConfigRules": {
                    "providerModelRules": [{
                        "providerId": "probe",
                        "modelId": "existing",
                        "config": { "enabled": false }
                    }]
                }
            }
        });

        // 新增两个，追加到末尾
        add_model_in(&mut value, "probe", "new-a").expect("add a");
        add_model_in(&mut value, "probe", "new-b").expect("add b");
        let order: Vec<String> = value["config"]["providerConfigRules"]["providerRules"][0]
            ["config"]["modelOrder"]
            .as_array()
            .expect("array")
            .iter()
            .map(|item| item.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(order, vec!["existing", "new-a", "new-b"]);

        // 重复 ID 与非法字符都要拒绝
        assert!(add_model_in(&mut value, "probe", "new-a").is_err());
        assert!(add_model_in(&mut value, "probe", "bad id").is_err());
        assert!(add_model_in(&mut value, "probe", "").is_err());
        // 不存在的 provider
        assert!(add_model_in(&mut value, "nope", "x").is_err());

        // 删除：连启用规则一起清掉
        remove_model_in(&mut value, "probe", "existing").expect("remove existing");
        let order: Vec<String> = value["config"]["providerConfigRules"]["providerRules"][0]
            ["config"]["modelOrder"]
            .as_array()
            .expect("array")
            .iter()
            .map(|item| item.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(order, vec!["new-a", "new-b"]);
        let rules = value["config"]["modelConfigRules"]["providerModelRules"]
            .as_array()
            .expect("array");
        assert!(
            rules.iter().all(|rule| rule["modelId"] != "existing"),
            "删除模型时启用/停用规则也要一起清掉"
        );

        // 再删同一个应报错
        assert!(remove_model_in(&mut value, "probe", "existing").is_err());
    }

    /// 群聊写路径端到端：新建 → 编辑 → 归档 → 恢复 → 删除（隔离 data root）。
    ///
    /// 只设一次 `BOT_PILOT_DATA_ROOT`：这是进程级环境变量，本模块只有这一个用例读它，
    /// 其它测试不碰 data_root()，因此不会互相干扰。
    #[test]
    fn group_crud_roundtrip() {
        let root = std::env::temp_dir().join(format!("bot-pilot-data-{}", std::process::id()));
        let v2 = root.join("v2");
        std::fs::create_dir_all(&v2).expect("建临时目录");
        std::fs::write(
            v2.join("teammates.json"),
            r#"{"version":1,"teammates":[
                {"id":"bot-a","name":"甲","directory":"C:/tmp","createdAt":"0"},
                {"id":"bot-b","name":"乙","directory":"C:/tmp","createdAt":"0"}
            ]}"#,
        )
        .expect("写 teammates");
        std::fs::write(v2.join("groups.json"), r#"{"version":1,"groups":[]}"#)
            .expect("写 groups");
        // SAFETY: 测试进程内单点设置；本模块只有此用例读取该变量。
        unsafe { std::env::set_var("BOT_PILOT_DATA_ROOT", &root) };

        let created = create_group(GroupDraft {
            name: "演练群聊".to_string(),
            admin_id: "bot-a".to_string(),
            member_ids: vec!["bot-a".to_string(), "bot-b".to_string()],
            instructions: "先对齐再动手".to_string(),
            memory: "窗口固定周四".to_string(),
            responder_policy: "mentions-first".to_string(),
        })
        .expect("新建群聊");
        assert_eq!(created.name, "演练群聊");
        assert_eq!(created.member_ids, vec!["bot-a", "bot-b"]);
        assert_eq!(Roster::load().groups.len(), 1, "名册必须看到新群聊");

        // 管理员不在成员里：校验层拒绝
        let rejected = create_group(GroupDraft {
            name: "坏群聊".to_string(),
            admin_id: "bot-z".to_string(),
            member_ids: vec!["bot-a".to_string()],
            instructions: String::new(),
            memory: String::new(),
            responder_policy: "mentions-first".to_string(),
        });
        assert!(rejected.is_err(), "管理员不在成员里必须被拒");

        let updated = update_group(
            &created.id,
            GroupDraft {
                name: "改名群聊".to_string(),
                admin_id: "bot-b".to_string(),
                member_ids: vec!["bot-b".to_string()],
                instructions: String::new(),
                memory: "新记忆".to_string(),
                responder_policy: "admin-only".to_string(),
            },
        )
        .expect("编辑群聊");
        assert_eq!(updated.name, "改名群聊");
        assert_eq!(updated.admin_id.as_deref(), Some("bot-b"));
        assert_eq!(updated.member_ids, vec!["bot-b"]);
        assert_eq!(updated.responder_policy, "admin-only");

        set_group_archived(&created.id, true).expect("归档");
        let roster = Roster::load();
        assert!(roster.groups.is_empty(), "归档后侧栏不应再有该群聊");
        assert_eq!(roster.archived_groups.len(), 1, "归档区必须能看到它");

        set_group_archived(&created.id, false).expect("取消归档");
        assert_eq!(Roster::load().groups.len(), 1);

        delete_group(&created.id).expect("删除群聊");
        assert!(Roster::load().groups.is_empty(), "删除后名册里不应再有");
        assert!(
            Roster::load().archived_groups.is_empty(),
            "删除后归档区也不应残留"
        );

        // 模型与白名单写回（同一个隔离 data root）。
        set_bot_model(
            "bot-a",
            Some(&ModelSpec {
                provider_id: "deepseek".to_string(),
                model_id: "deepseek-flash".to_string(),
                reasoning_level: Some("max".to_string()),
            }),
        )
        .expect("写模型");
        let with_model = Roster::load().bot_by_id("bot-a").cloned().expect("名册有 bot-a");
        assert_eq!(
            with_model.model.as_ref().map(|model| model.model_id.as_str()),
            Some("deepseek-flash")
        );
        assert_eq!(
            with_model
                .model
                .as_ref()
                .and_then(|model| model.reasoning_level.as_deref()),
            Some("max")
        );
        set_bot_model("bot-a", None).expect("清模型");
        assert!(Roster::load().bot_by_id("bot-a").expect("bot-a").model.is_none());

        set_bot_allowlist("bot-a", vec!["Write".to_string(), " Edit ".to_string(), "".to_string()])
            .expect("写白名单");
        let allowlisted = Roster::load().bot_by_id("bot-a").cloned().expect("bot-a");
        assert_eq!(
            allowlisted.approval_allowlist,
            vec!["Write".to_string(), "Edit".to_string()],
            "空白项要丢掉、条目要 trim"
        );
        assert_eq!(allowlisted.approval_level, "ask", "写白名单不应改档位");

        // SAFETY: 收尾，避免影响后续用例。
        unsafe { std::env::remove_var("BOT_PILOT_DATA_ROOT") };
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 团队记忆写路径：新增 → 编辑 → 撤销 → 删除（隔离 storage 目录）。
    #[test]
    fn team_memory_roundtrip() {
        let root = std::env::temp_dir().join(format!("bot-pilot-memory-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("建临时目录");
        // SAFETY: 本用例独占 MODE_STORAGE_DIR（其它用例不读它）。
        unsafe { std::env::set_var("MODE_STORAGE_DIR", &root) };

        save_team_memory(None, "部署约定", "发布前跑门禁", "先 clippy 再 test").expect("新增");
        let entries = team_memory_entries();
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].title, "部署约定");
        assert!(!entries[0].expired);
        let file = entries[0].file.clone();

        let read = read_team_memory(&file).expect("读回");
        assert_eq!(read.body.trim(), "先 clippy 再 test");

        save_team_memory(Some(&file), "部署约定 v2", "改过的描述", "正文也改了").expect("编辑");
        let entries = team_memory_entries();
        assert_eq!(entries.len(), 1, "编辑不能新增条目");
        assert_eq!(entries[0].title, "部署约定 v2");

        let undone = undo_team_memory().expect("撤销");
        assert!(!undone.is_empty(), "撤销要带标题");
        let entries = team_memory_entries();
        assert_eq!(entries[0].title, "部署约定", "撤销回到修改前");

        delete_team_memory(&file).expect("删除");
        assert!(team_memory_entries().is_empty(), "删除后列表为空");

        // SAFETY: 收尾。
        unsafe { std::env::remove_var("MODE_STORAGE_DIR") };
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 插件开关写路径：只改目标键，其它字段原样保留；文件不存在时不凭空造。
    #[test]
    fn plugin_switch_preserves_other_keys() {
        let root = std::env::temp_dir().join(format!("bot-pilot-plugin-{}", std::process::id()));
        let mode = root.join(".mode");
        std::fs::create_dir_all(&mode).expect("建 workspace 目录");
        std::fs::write(
            mode.join("config.json"),
            r#"{"version":1,"theme":"dark","plugins":{"enabledPlugins":{"other@x":true,"browser-use@mode-plugins-official":true}}}"#,
        )
        .expect("写 config");

        set_plugin_switch(
            root.to_str().expect("路径是 UTF-8"),
            PLUGIN_BROWSER,
            false,
        )
        .expect("关浏览器插件");
        let (browser, computer) = read_plugin_switches(root.to_str().unwrap()).expect("能读回");
        assert!(!browser, "browser 要被关掉");
        assert!(!computer, "computer 从未开启，保持 false");

        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(mode.join("config.json")).expect("读回"))
                .expect("合法 JSON");
        assert_eq!(value["theme"], "dark", "其它顶层字段必须原样保留");
        assert_eq!(
            value["plugins"]["enabledPlugins"]["other@x"], true,
            "其它插件键必须原样保留"
        );
        assert_eq!(
            value["plugins"]["enabledPlugins"]["browser-use@mode-plugins-official"],
            false
        );

        // 目录里没有 config.json：不新建，直接报错。
        let missing = root.join("missing");
        std::fs::create_dir_all(&missing).expect("建空目录");
        assert!(
            set_plugin_switch(missing.to_str().unwrap(), PLUGIN_COMPUTER, true).is_err(),
            "文件不存在时必须报错而不是凭空造配置"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod chief_private_tests {
    use super::*;

    #[test]
    fn private_bot_cannot_be_chief() {
        assert!(check_chief_private(true, true).is_err());
        assert!(check_chief_private(true, false).is_ok());
        assert!(check_chief_private(false, true).is_ok());
    }

    #[test]
    fn only_one_chief_survives() {
        let mut file = bot_team::teammate::TeammatesFile::empty();
        for (id, is_chief) in [("a", true), ("b", true), ("c", false)] {
            let mut record = base_record();
            record.id = id.to_string();
            record.name = id.to_string();
            record.directory = format!("D:\\ws\\{id}");
            record.is_chief = is_chief;
            file.teammates.push(record);
        }
        demote_other_chiefs(&mut file, "b");
        let chiefs: Vec<&str> = file
            .teammates
            .iter()
            .filter(|record| record.is_chief)
            .map(|record| record.id.as_str())
            .collect();
        assert_eq!(chiefs, vec!["b"]);
    }

    fn base_record() -> bot_team::teammate::TeammateRecord {
        bot_team::teammate::TeammateRecord {
            id: String::new(),
            name: String::new(),
            role: String::new(),
            description: String::new(),
            directory: String::new(),
            created_at: String::new(),
            last_session_id: None,
            pinned: false,
            instructions: String::new(),
            identity_sync: false,
            is_chief: false,
            is_private: false,
            model_selection: None,
            approval_policy: None,
            plugin_ids: Vec::new(),
            hidden: false,
            is_main: false,
        }
    }
}

#[cfg(test)]
mod provider_config_tests {
    use super::*;

    #[test]
    fn api_key_update_preserves_other_fields() {
        let mut value = serde_json::json!({
            "schemaVersion": 1,
            "config": {
                "providerConfigRules": {
                    "providerRules": [{
                        "providerId": "deepseek",
                        "providerName": "DeepSeek",
                        "config": {
                            "group": "standard-personal",
                            "access": { "type": "api-key", "apiKey": "old" },
                            "modelOrder": ["deepseek-flash"]
                        }
                    }]
                }
            }
        });
        set_api_key_in_value(&mut value, "deepseek", " new-key ").unwrap();
        assert_eq!(
            value["config"]["providerConfigRules"]["providerRules"][0]["config"]["access"]["apiKey"],
            "new-key"
        );
        assert_eq!(
            value["config"]["providerConfigRules"]["providerRules"][0]["config"]["modelOrder"][0],
            "deepseek-flash"
        );
        assert!(set_api_key_in_value(&mut value, "missing", "x").is_err());
        value["config"]["providerConfigRules"]["providerRules"][0]["config"]["access"]["type"] =
            serde_json::json!("oauth");
        assert!(set_api_key_in_value(&mut value, "deepseek", "x").is_err());
    }
}

#[cfg(test)]
mod agent_list_tests {
    use super::*;

    #[test]
    fn only_supported_local_agents_are_listed() {
        let commands: Vec<&str> = LOCAL_AGENT_CANDIDATES
            .iter()
            .map(|(_, _, command, _)| *command)
            .collect();
        assert_eq!(
            commands,
            vec!["mode-agent", "pi-acp", "deepseek-acp", "hermes", "zcode-acp-server"]
        );
    }

    /// 隔离指针必须对所有写路径生效：只设 `BOT_PILOT_DATA_ROOT` 时，
    /// 记忆根不能回落到 `~/.mode-bot`（v0.5.0 验收时真的写进过 production）。
    #[test]
    fn cli_storage_follows_data_root_override() {
        let root = std::env::temp_dir().join(format!("bot-pilot-clidir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("建临时目录");
        // SAFETY: 本用例独占这两个变量（见下方收尾），其它用例不读。
        unsafe {
            std::env::remove_var("MODE_STORAGE_DIR");
            std::env::set_var("BOT_PILOT_DATA_ROOT", &root);
        }

        let storage = cli_storage_dir().expect("有数据根覆盖时必须有 storage 根");
        assert_eq!(
            PathBuf::from(&storage),
            root.join("cli"),
            "storage 必须跟随 BOT_PILOT_DATA_ROOT，不能回落 home"
        );

        unsafe { std::env::remove_var("BOT_PILOT_DATA_ROOT") };
        let _ = std::fs::remove_dir_all(&root);
    }
}
