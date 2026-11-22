//! Bot 图标 + 颜色：面罩机器人 9 表情，全局按 Bot id 存一份外观。
//!
//! 落盘在 `state_dir/bot-appearance.json`，不写 teammates.json——图标只是
//! Mode Chat 的视觉偏好，避免和壳共享的 Bot schema 漂移互相踩。
//! 读取侧对缺失/坏文件一律回落空表（旧 Bot 继续用 id 色相头像）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use gpui::{rgb, Rgba};

pub const DEFAULT_ICON: &str = "app";
pub const DEFAULT_COLOR: &str = "#2ed17f";

/// 头像统一用新应用图标（对话气泡机器人）；底色仍可选。
/// 旧 Bot 存的 k1..k9 仍能读到，但渲染一律用同一套图形。
pub const ICONS: [(&str, &str); 1] = [("app", "气泡机器人")];

pub const COLORS: [(&str, &str); 8] = [
    ("#2ed17f", "绿"),
    ("#00b4a6", "青"),
    ("#4d6bfe", "蓝"),
    ("#7c5cff", "紫"),
    ("#ec2f6b", "粉"),
    ("#ff6900", "橙"),
    ("#e5484d", "红"),
    ("#f5a623", "黄"),
];

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Appearance {
    #[serde(default = "default_icon")]
    pub icon: String,
    #[serde(default = "default_color")]
    pub color: String,
}

fn default_icon() -> String {
    DEFAULT_ICON.to_string()
}

fn default_color() -> String {
    DEFAULT_COLOR.to_string()
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            icon: default_icon(),
            color: default_color(),
        }
    }
}

static STORE: OnceLock<RwLock<HashMap<String, Appearance>>> = OnceLock::new();

fn store() -> &'static RwLock<HashMap<String, Appearance>> {
    STORE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn path() -> PathBuf {
    crate::instance::state_dir().join("bot-appearance.json")
}

/// 启动时读一次；坏文件按空表处理（不影响主流程）。
pub fn load() {
    let Ok(bytes) = std::fs::read(path()) else {
        return;
    };
    let Ok(map) = serde_json::from_slice::<HashMap<String, Appearance>>(&bytes) else {
        return;
    };
    if let Ok(mut guard) = store().write() {
        *guard = map;
    }
}

pub fn get(bot_id: &str) -> Option<Appearance> {
    store().read().ok()?.get(bot_id).cloned()
}

/// 写内存并落盘；落盘失败也保留内存值，返回错误交给调用方提示。
pub fn set(bot_id: &str, icon: &str, color: &str) -> Result<(), String> {
    let appearance = Appearance {
        icon: icon.to_string(),
        color: color.to_string(),
    };
    if let Ok(mut guard) = store().write() {
        guard.insert(bot_id.to_string(), appearance);
    }
    persist()
}

pub fn remove(bot_id: &str) {
    if let Ok(mut guard) = store().write() {
        guard.remove(bot_id);
    }
    let _ = persist();
}

fn persist() -> Result<(), String> {
    let value = {
        let guard = store()
            .read()
            .map_err(|_| "图标偏好锁不可用".to_string())?;
        serde_json::to_value(&*guard).map_err(|error| format!("图标偏好序列化失败：{error}"))?
    };
    let path = path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    atomic_file::write_json_atomic(&path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("图标偏好写入失败：{error}"))
}

pub fn icon_label(key: &str) -> &'static str {
    ICONS
        .iter()
        .find(|(candidate, _)| *candidate == key)
        .map(|(_, label)| *label)
        .unwrap_or(ICONS[0].1)
}

/// `#rrggbb`；非法值回落默认绿。
pub fn parse_color(value: &str) -> Rgba {
    u32::from_str_radix(value.trim().trim_start_matches('#'), 16)
        .ok()
        .filter(|hex| *hex <= 0xffffff)
        .map(rgb)
        .unwrap_or_else(|| rgb(0x2ed17f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_and_falls_back() {
        assert_eq!(parse_color("#2ed17f"), rgb(0x2ed17f));
        assert_eq!(parse_color("nope"), rgb(0x2ed17f));
    }

    #[test]
    fn default_icon_is_selectable_and_legacy_values_fall_back() {
        assert!(ICONS.iter().any(|(key, _)| *key == DEFAULT_ICON));
        assert_eq!(icon_label("k3"), ICONS[0].1);
        assert_eq!(icon_label(DEFAULT_ICON), ICONS[0].1);
    }
}
