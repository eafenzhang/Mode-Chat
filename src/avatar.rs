//! Bot / 频道头像（原版色相派生 + 渐变 + 机器人面）。
//!
//! 为什么不用「首字色块」：原版的辨识度来自 id → 稳定色相（360 色）+ 145° 渐变 +
//! 内描边 + 机器人面。手绘圆点会让所有 Bot 看起来一样、浅色下还容易糊字。
//!
//! 机器人面是原版 `ui/avatar.tsx` 的四段 SVG（24 viewBox、stroke 2），与图标同一套
//! `svg().data()` 渲染管线。

use std::sync::{Arc, OnceLock};

use gpui::{
    div, hsla, img, linear_color_stop, linear_gradient, px, svg, AnyElement, BoxShadow, Hsla,
    Image, ImageFormat, IntoElement, ParentElement, Rgba, Styled,
};

use crate::theme;

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

/// 机器人面变体（0..3）。
pub fn avatar_variant(id: &str) -> usize {
    (avatar_hash(id) % 4) as usize
}

fn deg(h: f32, s: f32, l: f32, a: f32) -> Hsla {
    hsla(h / 360.0, s, l, a)
}

/// BOT 机器人面（按变体）。
fn bot_face(variant: usize) -> &'static [u8] {
    match variant % 4 {
        0 => include_bytes!("../assets/icons/bot-face-0.svg"),
        1 => include_bytes!("../assets/icons/bot-face-1.svg"),
        2 => include_bytes!("../assets/icons/bot-face-2.svg"),
        _ => include_bytes!("../assets/icons/bot-face-3.svg"),
    }
}

/// 头像外壳：色相渐变 + 内描边 + 外阴影。
fn shell(hue: f32, width: f32, height: f32, radius: f32) -> gpui::Div {
    let light = theme::is_light();
    let (top, bottom) = if light {
        (deg(hue, 0.72, 0.96, 1.), deg(hue, 0.62, 0.91, 1.))
    } else {
        (deg(hue, 0.44, 0.34, 1.), deg(hue, 0.48, 0.24, 1.))
    };
    let ring = if light {
        deg(hue, 0.55, 0.82, 1.)
    } else {
        deg(hue, 0.55, 0.70, 0.24)
    };
    let outer = if light {
        deg(hue, 0.30, 0.40, 0.10)
    } else {
        hsla(0., 0., 0., 0.24)
    };
    let inset = BoxShadow::new(px(0.), px(0.), ring)
        .spread_radius(px(1.))
        .inset();
    let drop = BoxShadow::new(px(0.), px(1.), outer).blur_radius(px(2.));
    div()
        .w(px(width))
        .h(px(height))
        .flex_shrink_0()
        .rounded(px(radius))
        .bg(linear_gradient(
            145.0,
            linear_color_stop(top, 0.0),
            linear_color_stop(bottom, 1.0),
        ))
        .shadow(vec![inset, drop])
        .flex()
        .items_center()
        .justify_center()
}

/// 自定义图标头像：近黑圆角底 + 染色面罩机器人（与 App 图标同族）。
pub fn bot_icon_tile(icon: &str, color: &str, size: f32, radius: f32) -> AnyElement {
    bot_icon_tile_rect(icon, color, size, size, radius)
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
    // 图标占底板的 78%，四周留呼吸位；设计稿 640×640。
    let glyph_size = size * 0.78;
    let k = glyph_size / 640.0;
    let white = gpui::rgb(0xffffff);
    let dark = gpui::rgb(0x0a0b0c);
    let bg = crate::bot_icon::parse_color(color);
    let mut glyph = div()
        .relative()
        .size(px(glyph_size))
        .child(
            svg()
                .data(crate::bot_icon::head_svg())
                .size(px(glyph_size))
                .text_color(white),
        )
        .child(
            div()
                .absolute()
                .left(px(120. * k))
                .top(px(286. * k))
                .w(px(400. * k))
                .h(px(172. * k))
                .rounded(px(86. * k))
                .bg(dark)
                .flex()
                .items_center()
                .justify_center()
                // 眼睛放进面罩内部，保证在面罩底色之上。
                .child(
                    svg()
                        .data(crate::bot_icon::eyes_svg(icon))
                        .w(px(400. * k))
                        .h(px(172. * k))
                        .text_color(white),
                ),
        );
    if let Some(mouth) = crate::bot_icon::mouth_svg(icon) {
        glyph = glyph.child(
            svg()
                .data(mouth)
                .absolute()
                .left(px(248. * k))
                .top(px(468. * k))
                .w(px(144. * k))
                .h(px(90. * k))
                .text_color(dark),
        );
    }
    div()
        .w(px(width))
        .h(px(height))
        .flex_shrink_0()
        .rounded(px(radius))
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .child(glyph)
        .into_any_element()
}

/// Bot 头像：有自定义表情/颜色时用图标卡，否则沿用 id 色相头像。
pub fn bot_avatar(id: &str, size: f32) -> AnyElement {
    if let Some(appearance) = crate::bot_icon::get(id) {
        return bot_icon_tile(
            &appearance.icon,
            &appearance.color,
            size,
            (size * 0.24).max(4.),
        );
    }
    let hue = avatar_hue(id);
    let fg = if theme::is_light() {
        deg(hue, 0.52, 0.30, 1.)
    } else {
        deg(hue, 0.65, 0.88, 1.)
    };
    let glyph = (size * 0.52).max(10.0);
    shell(hue, size, size, size / 2.0)
        .child(
            svg()
                .data(bot_face(avatar_variant(id)))
                .size(px(glyph))
                .text_color(fg),
        )
        .into_any_element()
}

/// 本机用户头像：默认应用图标，圆角与 Bot 方形头像一致。
pub fn app_avatar(size: f32, radius: f32) -> impl IntoElement {
    static APP_IMAGE: OnceLock<Arc<Image>> = OnceLock::new();
    let image = APP_IMAGE
        .get_or_init(|| {
            Arc::new(Image::from_bytes(
                ImageFormat::Png,
                include_bytes!("../assets/icons/app.png").to_vec(),
            ))
        })
        .clone();
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded(px(radius))
        .overflow_hidden()
        .child(img(image).size(px(size)))
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
    let hue = avatar_hue(id);
    let fg = if theme::is_light() {
        deg(hue, 0.52, 0.30, 1.)
    } else {
        deg(hue, 0.65, 0.88, 1.)
    };
    let glyph = (width.min(height) * 0.52).max(8.0);
    shell(hue, width, height, radius)
        .child(
            svg()
                .data(bot_face(avatar_variant(id)))
                .size(px(glyph))
                .text_color(fg),
        )
        .into_any_element()
}

/// 频道头像（同一套色相，前景 `#`）。
pub fn channel_avatar(id: &str, size: f32) -> impl IntoElement {
    let hue = avatar_hue(id);
    let fg = if theme::is_light() {
        deg(hue, 0.52, 0.30, 1.)
    } else {
        deg(hue, 0.65, 0.88, 1.)
    };
    shell(hue, size, size, size / 2.0).child(
        svg()
            .data(include_bytes!("../assets/icons/hash.svg"))
            .size(px((size * 0.48).max(10.0)))
            .text_color(fg),
    )
}

/// 未读圆点（8px，原版 `--bot-accent`；`ring` 用外扩 1.5px 的阴影模拟）。
pub fn unread_dot(size: f32) -> impl IntoElement {
    let ring = BoxShadow::new(px(0.), px(0.), Hsla::from(theme::panel())).spread_radius(px(1.5));
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded_full()
        .bg(theme::accent())
        .shadow(vec![ring])
}

/// 状态徽标（12px / 工作中 8px；颜色由调用方按状态给）。
// TODO(visual): 名册状态数据接入后启用（当前 D8 不显示）。
#[allow(dead_code)]
pub fn status_dot(size: f32, color: Rgba) -> impl IntoElement {
    let ring = BoxShadow::new(px(0.), px(0.), Hsla::from(theme::panel())).spread_radius(px(1.5));
    div()
        .size(px(size))
        .rounded_full()
        .bg(color)
        .shadow(vec![ring])
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
