//! 本地智能体引擎的官方图标（Pi / Hermes / DeepSeek / ZCode）。
//!
//! 全部换成官方最新矢量标（LobeHub 单色版），按 `currentColor` 染色：
//! - Pi → `pi`（官方三色 logo 的单色版，比原先手拆三层彩色干净）
//! - Hermes → `hermesagent`（原先用 44KB PNG 位图）
//! - DeepSeek → `deepseek`
//! - ZCode → `zai`（智谱 Z.ai 的官方标）
//!
//! 资产在 `assets/icons/agents/`，由 `scripts/gen-icons-lobe.py` 生成。

use gpui::{div, px, svg, AnyElement, IntoElement, ParentElement, Rgba, Styled};

use crate::theme;

fn glyph(data: &'static [u8], size: f32, color: Rgba) -> AnyElement {
    div()
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .child(svg().data(data).size(px(size * 0.66)).text_color(color))
        .into_any_element()
}

/// 引擎 id → 官方标。未知引擎返回 None（调用方回落到通用 Bot 图标）。
pub fn agent(id: &str, size: f32, color: Rgba) -> Option<AnyElement> {
    let data: &'static [u8] = match id {
        "pi" | "pi-acp" => include_bytes!("../assets/icons/agents/pi.svg"),
        "hermes" => include_bytes!("../assets/icons/agents/hermes.svg"),
        "deepseek" => include_bytes!("../assets/icons/agents/deepseek.svg"),
        "zcode" | "zcode-acp-server" => include_bytes!("../assets/icons/agents/zcode.svg"),
        _ => return None,
    };
    Some(glyph(data, size, color))
}

/// 品牌色（深色主题下过暗的标回退到前景色）。
pub fn agent_color(id: &str) -> Rgba {
    let hex = match id {
        "pi" | "pi-acp" => 0xf09082,
        "hermes" => 0xd97757,
        "deepseek" => 0x4d6bfe,
        "zcode" | "zcode-acp-server" => 0x2b8cff,
        _ => return theme::ink_secondary(),
    };
    let r = ((hex >> 16) & 0xff) as f32 / 255.0;
    let g = ((hex >> 8) & 0xff) as f32 / 255.0;
    let b = (hex & 0xff) as f32 / 255.0;
    let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    if !theme::is_light() && luminance < 0.28 {
        theme::ink()
    } else {
        gpui::rgb(hex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_has_an_official_mark() {
        for id in ["pi", "pi-acp", "hermes", "deepseek", "zcode"] {
            // 用不透明色构造，只验证资产存在且是合法 svg
            assert!(agent(id, 26., gpui::rgb(0xffffff)).is_some(), "{id} 缺图标");
        }
        assert!(agent("unknown-engine", 26., gpui::rgb(0xffffff)).is_none());
    }

    #[test]
    fn agent_assets_are_monochrome_vectors() {
        for (name, bytes) in [
            ("pi", &include_bytes!("../assets/icons/agents/pi.svg")[..]),
            ("hermes", &include_bytes!("../assets/icons/agents/hermes.svg")[..]),
            ("deepseek", &include_bytes!("../assets/icons/agents/deepseek.svg")[..]),
            ("zcode", &include_bytes!("../assets/icons/agents/zcode.svg")[..]),
        ] {
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(text.starts_with("<svg"), "{name} 不是 svg");
            assert!(text.contains("currentColor"), "{name} 不是单色");
            assert!(!text.contains("<image"), "{name} 内嵌位图，不是矢量");
        }
    }
}
