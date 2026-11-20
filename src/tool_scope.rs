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
