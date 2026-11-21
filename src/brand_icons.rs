//! 第三方品牌图标（连接器 / IM 平台）。
//!
//! SVG 是单色 currentColor；GPUI 的 svg 按 alpha 染色，所以只关心形状。
//! 颜色用各家品牌色，深色主题下过暗的品牌色回退到前景色。

use gpui::{rgb, Rgba};

use crate::theme;

pub fn connector_key(id: &str) -> Option<&'static str> {
    match id {
        "github" => Some("github"),
        "gitlab" => Some("gitlab"),
        "notion" => Some("notion"),
        "slack" => Some("slack"),
        "postgres" => Some("postgres"),
        "kubernetes" => Some("kubernetes"),
        "cloudflare" => Some("cloudflare"),
        "linear" => Some("linear"),
        "figma" => Some("figma"),
        "playwright" => Some("playwright"),
        "redis" => Some("redis"),
        "elasticsearch" => Some("elasticsearch"),
        "brave-search" => Some("brave"),
        _ => None,
    }
}

pub fn asset(key: &str) -> Option<&'static [u8]> {
    Some(match key {
        "github" => include_bytes!("../assets/icons/brands/github.svg"),
        "gitlab" => include_bytes!("../assets/icons/brands/gitlab.svg"),
        "notion" => include_bytes!("../assets/icons/brands/notion.svg"),
        "slack" => include_bytes!("../assets/icons/brands/slack.svg"),
        "postgres" => include_bytes!("../assets/icons/brands/postgresql.svg"),
        "kubernetes" => include_bytes!("../assets/icons/brands/kubernetes.svg"),
        "cloudflare" => include_bytes!("../assets/icons/brands/cloudflare.svg"),
        "linear" => include_bytes!("../assets/icons/brands/linear.svg"),
        "figma" => include_bytes!("../assets/icons/brands/figma.svg"),
        "playwright" => include_bytes!("../assets/icons/brands/playwright.svg"),
        "redis" => include_bytes!("../assets/icons/brands/redis.svg"),
        "elasticsearch" => include_bytes!("../assets/icons/brands/elasticsearch.svg"),
        "brave" => include_bytes!("../assets/icons/brands/brave.svg"),
        "wechat" => include_bytes!("../assets/icons/brands/wechat.svg"),
        "wecom" => include_bytes!("../assets/icons/brands/wecom.svg"),
        _ => return None,
    })
}

fn brand_hex(key: &str) -> Option<u32> {
    Some(match key {
        "github" => 0x181717,
        "gitlab" => 0xfc6d26,
        "notion" => 0x000000,
        "slack" => 0x4a154b,
        "postgres" => 0x4169e1,
        "kubernetes" => 0x326ce5,
        "cloudflare" => 0xf38020,
        "linear" => 0x5e6ad2,
        "figma" => 0xf24e1e,
        "playwright" => 0x2ead33,
        "redis" => 0xdc382d,
        "elasticsearch" => 0x005571,
        "brave" => 0xfb542b,
        "wechat" => 0x07c160,
        "wecom" => 0x0082ef,
        _ => return None,
    })
}

/// 品牌原色（头像底色等需要原色的场景）。
pub fn raw_color(key: &str) -> Option<Rgba> {
    brand_hex(key).map(rgb)
}

/// 前景用品牌色；暗色主题下亮度太低的品牌改用主题前景色。
pub fn color(key: &str) -> Option<Rgba> {
    let hex = brand_hex(key)?;
    let r = ((hex >> 16) & 0xff) as f32 / 255.0;
    let g = ((hex >> 8) & 0xff) as f32 / 255.0;
    let b = (hex & 0xff) as f32 / 255.0;
    let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    if !theme::is_light() && luminance < 0.28 {
        Some(theme::ink())
    } else {
        Some(rgb(hex))
    }
}
