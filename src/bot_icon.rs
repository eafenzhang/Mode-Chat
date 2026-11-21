//! Bot 图标 + 颜色：面罩机器人 9 表情，全局按 Bot id 存一份外观。
//!
//! 落盘在 `state_dir/bot-appearance.json`，不写 teammates.json——图标只是
//! Mode Chat 的视觉偏好，避免和壳共享的 Bot schema 漂移互相踩。
//! 读取侧对缺失/坏文件一律回落空表（旧 Bot 继续用 id 色相头像）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use gpui::{rgb, Rgba};

pub const DEFAULT_ICON: &str = "k1";
pub const DEFAULT_COLOR: &str = "#2ed17f";

/// 选择器顺序：中性、微笑、开心、眨眼、困倦、星星眼、爱心眼、惊讶、生气。
pub const ICONS: [(&str, &str); 9] = [
    ("k1", "中性"),
    ("k9", "微笑"),
    ("k2", "开心"),
    ("k3", "眨眼"),
    ("k4", "困倦"),
    ("k5", "星星眼"),
    ("k6", "爱心眼"),
    ("k7", "惊讶"),
    ("k8", "生气"),
];

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

/// 头部（天线/耳朵/脸）单色剪影：GPUI 的 svg 按 alpha 整体染色。
pub fn head_svg() -> &'static [u8] {
    include_bytes!("../assets/icons/bot-head.svg")
}

/// 表情眼睛（面罩内的形状，单色剪影）。
pub fn eyes_svg(key: &str) -> &'static [u8] {
    match key {
        "k9" => include_bytes!("../assets/icons/bot-eyes-k9.svg"),
        "k2" => include_bytes!("../assets/icons/bot-eyes-k2.svg"),
        "k3" => include_bytes!("../assets/icons/bot-eyes-k3.svg"),
        "k4" => include_bytes!("../assets/icons/bot-eyes-k4.svg"),
        "k5" => include_bytes!("../assets/icons/bot-eyes-k5.svg"),
        "k6" => include_bytes!("../assets/icons/bot-eyes-k6.svg"),
        "k7" => include_bytes!("../assets/icons/bot-eyes-k7.svg"),
        "k8" => include_bytes!("../assets/icons/bot-eyes-k8.svg"),
        _ => include_bytes!("../assets/icons/bot-eyes-k1.svg"),
    }
}

/// 嘴（没有嘴的表情返回 None）。
pub fn mouth_svg(key: &str) -> Option<&'static [u8]> {
    match key {
        "k9" => Some(include_bytes!("../assets/icons/bot-mouth-k9.svg")),
        "k2" => Some(include_bytes!("../assets/icons/bot-mouth-k2.svg")),
        "k3" => Some(include_bytes!("../assets/icons/bot-mouth-k3.svg")),
        "k4" => Some(include_bytes!("../assets/icons/bot-mouth-k4.svg")),
        "k5" => Some(include_bytes!("../assets/icons/bot-mouth-k5.svg")),
        "k6" => Some(include_bytes!("../assets/icons/bot-mouth-k6.svg")),
        "k7" => Some(include_bytes!("../assets/icons/bot-mouth-k7.svg")),
        "k8" => Some(include_bytes!("../assets/icons/bot-mouth-k8.svg")),
        _ => None,
    }
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
    fn every_icon_has_asset() {
        for (key, _) in ICONS {
            assert!(!eyes_svg(key).is_empty());
        }
        assert!(!head_svg().is_empty());
        assert!(mouth_svg("k1").is_none());
        assert!(mouth_svg("k2").is_some());
    }
}
