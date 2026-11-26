//! Bot / 群聊头像（原版色相派生 + 渐变 + 机器人面）。
//!
//! 为什么不用「首字色块」：原版的辨识度来自 id → 稳定色相（360 色）+ 145° 渐变 +
//! 内描边 + 机器人面。手绘圆点会让所有 Bot 看起来一样、浅色下还容易糊字。
//!
//! 机器人面是原版 `ui/avatar.tsx` 的四段 SVG（24 viewBox、stroke 2），与图标同一套
//! `svg().data()` 渲染管线。

use gpui::{
    div, px, svg, AnyElement, IntoElement, ParentElement, Rgba, Styled,
};

/// 头像色相哈希：照原版 `avatarHash`（`hash = hash*31 + charCode`，取绝对值）。
/// 用 UTF-16 码元与 JS 的 `charCodeAt` 对齐。
pub fn avatar_hash(id: &str) -> u32 {
    let mut hash: i32 = 0;
    for unit in id.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(unit as i32);
    }
    hash.unsigned_abs()
}

/// 头像色相（0..360）。
pub fn avatar_hue(id: &str) -> f32 {
    (avatar_hash(id) % 360) as f32
}

/// 机器人面变体（0..3；旧色相头像测试用）。
#[cfg(test)]
pub fn avatar_variant(id: &str) -> usize {
    (avatar_hash(id) % 4) as usize
}

/// 自定义图标头像：近黑圆角底 + 染色面罩机器人（与 App 图标同族）。
pub fn bot_icon_tile(icon: &str, color: &str, size: f32, radius: f32) -> AnyElement {
    bot_icon_tile_rect(icon, color, size, size, radius)
}

/// 只有机器人分层的版本（选择器格子自己画底色/描边）。
pub fn bot_icon_glyph(icon: &str, tile_size: f32, background: Rgba) -> AnyElement {
    icon_layers(icon, gpui::rgb(0xffffff), background, tile_size).into_any_element()
}

/// 应用图标的 Bot 版分层：白气泡 + 面罩（用底色）+ 白眼睛。
/// 图标 id 仍保留在存储里以兼容旧 Bot，但所有 id 都渲染同一套新图形。
fn icon_layers(
    _icon: &str,
    tint: Rgba,
    dark: Rgba,
    glyph_size: f32,
) -> gpui::Div {
    let side = px(glyph_size);
    // 面罩比底色暗一档，小尺寸下白色眼睛才有对比。
    let visor = Rgba {
        r: dark.r * 0.76,
        g: dark.g * 0.76,
        b: dark.b * 0.76,
        a: dark.a,
    };
    let layer = |bytes: &'static [u8], color: Rgba| {
        svg()
            .data(bytes)
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size(side)
            .text_color(color)
    };
    div()
        .relative()
        .size(side)
        // 头像单独用居中、无尾巴、无倾斜的图层：小尺寸下不会被尾巴带偏。
        .child(layer(
            include_bytes!("../assets/icons/bot-avatar-bubble.svg"),
            tint,
        ))
        .child(layer(
            include_bytes!("../assets/icons/bot-avatar-visor.svg"),
            visor,
        ))
        .child(layer(
            include_bytes!("../assets/icons/bot-avatar-eyes.svg"),
            tint,
        ))
}

/// App 图标（矢量版）：微信绿圆角底 + 白色气泡，任意尺寸都清晰。
pub fn app_icon(size: f32, radius: f32) -> AnyElement {
    // 矢量分层：绿底 + 白气泡/尾巴 + 绿内窗 + 白眼睛（与 app.png / app.ico 同源）。
    // 不缩放位图，36/26/18px 下也保持锐利。
    let side = px(size);
    let layer = |bytes: &'static [u8], color: Rgba| {
        svg()
            .data(bytes)
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size(side)
            .text_color(color)
    };
    div()
        .relative()
        .size(side)
        .flex_shrink_0()
        .rounded(px(radius))
        .overflow_hidden()
        // 微信绿底白标：与 app.png / app.ico 完全一致（#07C160）。
        .bg(gpui::rgb(0x07c160))
        .child(layer(
            include_bytes!("../assets/icons/app-layer-bubble.svg"),
            gpui::rgb(0xffffff),
        ))
        .child(layer(
            include_bytes!("../assets/icons/app-layer-visor.svg"),
            gpui::rgb(0x07c160),
        ))
        .child(layer(
            include_bytes!("../assets/icons/app-layer-eyes.svg"),
            gpui::rgb(0xffffff),
        ))
        .into_any_element()
}

/// 任意宽高版本（群头像单元格）：近黑底板 + 头部/面罩/眼睛/嘴分层拼装。
pub fn bot_icon_tile_rect(
    icon: &str,
    color: &str,
    width: f32,
    height: f32,
    radius: f32,
) -> AnyElement {
    let size = width.min(height);
    let white = gpui::rgb(0xffffff);
    let bg = crate::bot_icon::parse_color(color);
    div()
        .w(px(width))
        .h(px(height))
        .flex_shrink_0()
        .rounded(px(radius))
        .bg(bg)
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        // 与应用图标同一比例；图形在画布里略偏右下，这里补偿回视觉中心。
        .child(icon_layers(icon, white, bg, size))
        .into_any_element()
}

/// 本机用户头像：与 App 图标同构的矢量版，任意尺寸都清晰。
pub fn app_avatar(size: f32, radius: f32) -> AnyElement {
    app_icon(size, radius)
}

/// 任意宽高的 Bot 头像（群头像拼图单元格）。
pub fn bot_avatar_rect(id: &str, width: f32, height: f32, radius: f32) -> AnyElement {
    if let Some(appearance) = crate::bot_icon::get(id) {
        return bot_icon_tile_rect(
            &appearance.icon,
            &appearance.color,
            width,
            height,
            radius,
        );
    }
    bot_icon_tile_rect(
        crate::bot_icon::DEFAULT_ICON,
        crate::bot_icon::DEFAULT_COLOR,
        width,
        height,
        radius,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_web() {
        assert_eq!(avatar_hash("bot-1"), 93924779);
        assert_eq!(avatar_hash("mode-bot"), 620911971);
        assert_eq!(avatar_hash("a"), 97);
        assert_eq!(avatar_hash("娜娜"), 738176);
        assert_eq!(avatar_hue("bot-1"), 59.0);
        assert_eq!(avatar_variant("bot-1"), 3);
        assert_eq!(avatar_variant("娜娜"), 0);
    }
}
