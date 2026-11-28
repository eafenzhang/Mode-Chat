//! 浏览器 / 电脑控制的「使用范围」。
//!
//! 存在 prefs.json：
//! - `browser_enabled` / `computer_enabled`：总开关（默认开）；
//! - `browser_disabled_bots` / `computer_disabled_bots`：按 Bot 关闭名单。
//!
//! 所有入口（私聊、群成员、IM）都按 identity 求 Bot id 后判断；外部 ACP
//! 引擎使用自带工具，不受这里限制。运行中的会话在工具执行时再查一次，
//! 因此设置页关掉后立即生效，不必重开会话。

use std::collections::HashSet;

use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Browser,
    Computer,
}

impl Family {
    fn enabled_key(self) -> &'static str {
        match self {
            Family::Browser => "browser_enabled",
            Family::Computer => "computer_enabled",
        }
    }

    fn disabled_key(self) -> &'static str {
        match self {
            Family::Browser => "browser_disabled_bots",
            Family::Computer => "computer_disabled_bots",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Family::Browser => "浏览器控制",
            Family::Computer => "电脑操控",
        }
    }
}

fn prefs_path() -> std::path::PathBuf {
    crate::instance::state_dir().join("prefs.json")
}

fn prefs() -> Value {
    std::fs::read(prefs_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}))
}

fn write_prefs(value: &Value) -> Result<(), String> {
    atomic_file::write_json_atomic(
        &prefs_path(),
        value,
        &atomic_file::WriteOptions::default(),
    )
    .map_err(|error| format!("写入设置失败：{error}"))
}

/// 总开关（默认开）。
pub fn enabled(family: Family) -> bool {
    prefs()
        .get(family.enabled_key())
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

pub fn set_enabled(family: Family, enabled: bool) -> Result<(), String> {
    let mut value = prefs();
    value[family.enabled_key()] = serde_json::json!(enabled);
    write_prefs(&value)
}

/// 从 identity 提取 Bot id（teammate 或群成员）；其它身份返回 None。
pub fn teammate_id(identity: &str) -> Option<String> {
    let identity = identity.trim();
    if let Some(id) = identity.strip_prefix("mode-bot:teammate:") {
        return Some(id.to_string()).filter(|id| !id.is_empty());
    }
    if let Some((_, member)) = identity.rsplit_once(":member:") {
        return Some(member.to_string()).filter(|id| !id.is_empty());
    }
    None
}

/// 该身份当前是否可用（总开关 + 按 Bot 名单）。
pub fn allowed(family: Family, identity: &str) -> bool {
    if !enabled(family) {
        return false;
    }
    match teammate_id(identity) {
        Some(id) => !disabled_bots(family).contains(&id),
        None => true,
    }
}

pub fn disabled_bots(family: Family) -> HashSet<String> {
    prefs()
        .get(family.disabled_key())
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 切换某个 Bot 的允许状态；返回切换后的「是否允许」。
pub fn toggle_bot(family: Family, bot_id: &str) -> Result<bool, String> {
    let mut value = prefs();
    let mut disabled = disabled_bots(family);
    let allowed = if disabled.remove(bot_id) {
        true
    } else {
        disabled.insert(bot_id.to_string());
        false
    };
    let mut list: Vec<String> = disabled.into_iter().collect();
    list.sort();
    value[family.disabled_key()] = serde_json::json!(list);
    write_prefs(&value)?;
    Ok(allowed)
}

/// 工具被关掉时统一的错误文案。
pub fn denial(family: Family) -> String {
    format!(
        "{}已在「设置 → {}」中关闭（可在设置里按 Bot 开启）",
        family.label(),
        family.label()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_teammate_and_group_member_ids() {
        assert_eq!(
            teammate_id("mode-bot:teammate:alpha").as_deref(),
            Some("alpha")
        );
        assert_eq!(
            teammate_id("mode-bot:group:g1:member:beta").as_deref(),
            Some("beta")
        );
        assert_eq!(teammate_id("mode-bot:group:g1"), None);
        assert_eq!(teammate_id(""), None);
    }
}

// ---------- 电脑操控分级（H03/H06）：每 Bot 档位 + 永久禁止清单 ----------

/// 电脑操控档位（prefs `computer_levels`；缺省 = 完全）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputerLevel {
    /// 只看：仅截屏/读窗口，禁止任何输入。
    Look,
    /// 后台：可操作，但不可把窗口切到前台。
    Background,
    /// 完全：允许切换前台（默认）。
    Foreground,
}

impl ComputerLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            ComputerLevel::Look => "look",
            ComputerLevel::Background => "background",
            ComputerLevel::Foreground => "foreground",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "look" => Some(ComputerLevel::Look),
            "background" => Some(ComputerLevel::Background),
            "foreground" => Some(ComputerLevel::Foreground),
            _ => None,
        }
    }
}

/// 一次电脑操控的类别（用于档位判定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputerAction {
    /// 只读：截屏、窗口列表、光标位置、剪贴板读。
    Inspect,
    /// 输入：鼠标、键盘、滚轮、拖拽、剪贴板写。
    Input,
    /// 把窗口切到前台。
    Focus,
}

/// 某身份对应的电脑操控档位（prefs `computer_levels.<bot_id>`，缺省完全）。
pub fn computer_level(identity: &str) -> ComputerLevel {
    let Some(id) = teammate_id(identity) else {
        return ComputerLevel::Foreground;
    };
    prefs()
        .get("computer_levels")
        .and_then(|levels| levels.get(&id))
        .and_then(Value::as_str)
        .and_then(ComputerLevel::parse)
        .unwrap_or(ComputerLevel::Foreground)
}

pub fn set_computer_level(bot_id: &str, level: ComputerLevel) -> Result<(), String> {
    let mut value = prefs();
    let mut levels = value
        .get("computer_levels")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    levels[bot_id] = serde_json::json!(level.as_str());
    value["computer_levels"] = levels;
    write_prefs(&value)
}

/// 档位判定：总开关/名单之外，再看「只看/后台」的粒度限制。
pub fn ensure_computer_action(identity: &str, action: ComputerAction) -> Result<(), String> {
    if computer_paused() {
        return Err("电脑操控已暂停（设置 → 电脑操控 可恢复）。".to_string());
    }
    if !allowed(Family::Computer, identity) {
        return Err(denial(Family::Computer));
    }
    match computer_level(identity) {
        ComputerLevel::Foreground => Ok(()),
        ComputerLevel::Look if action == ComputerAction::Inspect => Ok(()),
        ComputerLevel::Look => Err(
            "该 Bot 的电脑操控档位是「只看」：只能截屏/读窗口，不能输入或点击。".to_string(),
        ),
        ComputerLevel::Background if action == ComputerAction::Focus => Err(
            "该 Bot 的电脑操控档位是「后台」：不能把窗口切到前台。".to_string(),
        ),
        ComputerLevel::Background => Ok(()),
    }
}

/// 永久禁止操控的窗口（标题/进程名关键词，大小写不敏感）。内置 + prefs `computer_blocklist`。
const BUILTIN_BLOCKED: &[&str] = &[
    // 系统设置与凭据界面
    "systemsettings",
    "系统设置",
    "windows 设置",
    "windows settings",
    "凭据管理器",
    "credential manager",
    // 常见密码管理器
    "1password",
    "bitwarden",
    "keepass",
    "lastpass",
    "dashlane",
    "nordpass",
    "enpass",
    // 锁屏/登录安全界面
    "lockapp",
    "logonui",
    "安全中心",
];

/// 命中禁止清单的说明；None = 允许。
pub fn blocked_window(title: &str, process: &str) -> Option<String> {
    let mut keywords: Vec<String> = BUILTIN_BLOCKED.iter().map(|item| item.to_string()).collect();
    if let Some(items) = prefs()
        .get("computer_blocklist")
        .and_then(Value::as_array)
    {
        for item in items {
            if let Some(text) = item.as_str().map(str::trim).filter(|text| !text.is_empty()) {
                keywords.push(text.to_ascii_lowercase());
            }
        }
    }
    let window = title.to_ascii_lowercase();
    let process = process.to_ascii_lowercase();
    keywords.iter().find_map(|keyword| {
        (window.contains(keyword.as_str()) || process.contains(keyword.as_str()))
            .then(|| keyword.clone())
    })
}

/// 全局暂停（H05「暂停」）：prefs `computer_paused`，暂停时所有电脑操控被拒绝。
pub fn computer_paused() -> bool {
    prefs()
        .get("computer_paused")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub fn set_computer_paused(paused: bool) -> Result<(), String> {
    let mut value = prefs();
    value["computer_paused"] = serde_json::json!(paused);
    write_prefs(&value)
}

/// 每 Bot 应用授权（H04）：prefs `computer_app_grants.<bot_id>`，空 = 不限制。
pub fn computer_app_grants(identity: &str) -> Vec<String> {
    let Some(id) = teammate_id(identity) else {
        return Vec::new();
    };
    prefs()
        .get("computer_app_grants")
        .and_then(|grants| grants.get(&id))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|item| item.trim().to_ascii_lowercase())
                .filter(|item| !item.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// 写授权列表（设置 UI 化之前的入口；prefs `computer_app_grants`）。
#[allow(dead_code)]
pub fn set_computer_app_grants(bot_id: &str, grants: &[String]) -> Result<(), String> {
    let mut value = prefs();
    let mut all = value
        .get("computer_app_grants")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let list: Vec<String> = grants
        .iter()
        .map(|item| item.trim().to_ascii_lowercase())
        .filter(|item| !item.is_empty())
        .collect();
    all[bot_id] = serde_json::json!(list);
    value["computer_app_grants"] = all;
    write_prefs(&value)
}

/// 应用授权判定：配置了白名单而目标应用不在其中 → 拒绝（空名单 = 不限制）。
pub fn ensure_app_granted(identity: &str, process: &str) -> Result<(), String> {
    let grants = computer_app_grants(identity);
    if grants.is_empty() {
        return Ok(());
    }
    let process = process.to_ascii_lowercase();
    if grants.iter().any(|grant| process.contains(grant.as_str())) {
        return Ok(());
    }
    Err(format!(
        "该 Bot 只能操控已授权的应用（当前窗口属于 {process}，授权列表：{}）。",
        grants.join(" / ")
    ))
}
