//! 微信 4.x 皮肤 token（深色为 2026-11-16 现场截图采样；2026-11-21 增加浅色与跟随系统）。
//!
//! 仅用于 `BOT_PILOT_WECHAT_UI` 三栏外壳；经典三栏仍走 `theme.rs`。
//! 主题色（accent）由设置页选择，默认微信绿；气泡/选中行/图标都跟随。

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{linear_color_stop, linear_gradient, Background, Rgba};

const fn rgb(value: u32) -> Rgba {
    rgba(value, 255)
}

const fn rgba(value: u32, a: u8) -> Rgba {
    Rgba {
        r: ((value >> 16) & 0xff) as f32 / 255.0,
        g: ((value >> 8) & 0xff) as f32 / 255.0,
        b: (value & 0xff) as f32 / 255.0,
        a: a as f32 / 255.0,
    }
}

pub const FONT_FAMILY: &str = "Microsoft YaHei UI";

static ACTIVE: AtomicBool = AtomicBool::new(false);
static LIGHT: AtomicBool = AtomicBool::new(false);

/// 启动时由 main 置位；设置页/对话框 helper 用它切到微信表面。
pub fn set_active(active: bool) {
    ACTIVE.store(active, Ordering::Relaxed);
}

pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// 浅色/深色皮肤（设置页「主题」，跟随系统时由启动逻辑解析）。
pub fn set_light(light: bool) {
    LIGHT.store(light, Ordering::Relaxed);
}

pub fn light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

fn pick(dark: u32, light_value: u32) -> Rgba {
    rgb(if light() { light_value } else { dark })
}

fn pick_alpha(dark: u32, dark_a: u8, light_value: u32, light_a: u8) -> Rgba {
    if light() {
        rgba(light_value, light_a)
    } else {
        rgba(dark, dark_a)
    }
}

/// 用户选择的主题色；默认微信绿（不再固定绿色）。
fn accent_color() -> Rgba {
    crate::theme::accent_custom().unwrap_or_else(|| rgb(0x0da869))
}

/// 在给定底色上选可读的文字色（深色底用白字）。
fn contrast_ink(color: Rgba) -> Rgba {
    let luma = 0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b;
    if luma < 0.55 {
        rgb(0xffffff)
    } else {
        rgb(0x1a1a1a)
    }
}

// 布局（微信 4.x 896×648 实测）
pub const TITLEBAR_HEIGHT: f32 = 28.0;
pub const NAV_WIDTH: f32 = 60.0;
/// 会话/设置列表宽度：对齐微信 4.x 的 232（原 246）。
pub const LIST_WIDTH: f32 = 232.0;
/// 右侧信息栏在本皮肤里默认不显示（微信没有常驻右栏）。
pub const PANEL_WIDTH: f32 = 166.0;
pub const NAV_ITEM_HEIGHT: f32 = 44.0;
/// 会话行高：对齐微信 4.x 的 60（原 56）。
pub const LIST_ROW_HEIGHT: f32 = 60.0;
pub const SEARCH_HEIGHT: f32 = 32.0;
/// 输入框最小高度。原来是 148，占掉窗口 21% 却只放两行字；
/// 收到 110（≈15%）更接近微信 4.x 的比例，也把空间让给消息区。
/// 输入框最小高度：对齐微信 4.x 的约 128（原 110）。
pub const COMPOSER_HEIGHT: f32 = 128.0;

// 窗口 / 标题栏
pub fn window_bg() -> Rgba {
    pick(0x1e1e1f, 0xf3f3f3)
}
pub fn titlebar_bg() -> Rgba {
    pick(0x252526, 0xebebeb)
}

// 左导轨
pub fn nav_item_selected_bg() -> Rgba {
    pick(0x2f2f30, 0xdedede)
}
pub fn nav_item_hover_bg() -> Rgba {
    pick(0x2f2f30, 0xd8d8d8)
}
pub fn nav_text() -> Rgba {
    pick(0xc8c8c8, 0x4a4a4a)
}
pub fn nav_text_active() -> Rgba {
    pick(0xededed, 0x1a1a1a)
}
pub fn nav_icon() -> Rgba {
    pick(0x9a9a9c, 0x666666)
}
pub fn nav_badge_bg() -> Rgba {
    pick(0xe24747, 0xfa5151)
}
// 会话列表
pub fn list_hover() -> Rgba {
    pick(0x3a3a3b, 0xe8e8e8)
}
pub fn list_title() -> Rgba {
    pick(0xe8e8e8, 0x1a1a1a)
}
pub fn list_title_selected() -> Rgba {
    pick(0xffffff, 0xffffff)
}
pub fn list_preview() -> Rgba {
    pick(0x8a8a8c, 0x8a8a8a)
}
pub fn list_preview_selected() -> Rgba {
    pick_alpha(0xffffff, 0xd9, 0xffffff, 0xd9)
}
pub fn list_time() -> Rgba {
    pick(0x7a7a7c, 0xb0b0b0)
}
pub fn list_time_selected() -> Rgba {
    pick_alpha(0xffffff, 0xa6, 0xffffff, 0xa6)
}
pub fn search_bg() -> Rgba {
    pick(0x3a3a3b, 0xffffff)
}
pub fn star() -> Rgba {
    pick(0xfbc224, 0xf5a623)
}

// 会话区
pub fn chat_bg() -> Rgba {
    pick(0x1e1e1f, 0xffffff)
}
pub fn chat_title() -> Rgba {
    pick(0xededed, 0x1a1a1a)
}
pub fn chat_subtitle() -> Rgba {
    pick(0x8a8a8c, 0x8a8a8a)
}
pub fn bubble_in() -> Rgba {
    pick(0x2f2f30, 0xf2f2f2)
}
pub fn bubble_out() -> Rgba {
    // 默认微信气泡（浅色是经典浅绿）；用户选了主题色则直接使用主题色。
    crate::theme::accent_custom().unwrap_or_else(|| pick(0x35d28d, 0x95ec69))
}
pub fn bubble_text() -> Rgba {
    pick(0xd8d8d8, 0x1a1a1a)
}
/// 发送气泡文字：默认绿底黑字；自定义主题色按底色明暗取黑/白。
pub fn bubble_out_text() -> Rgba {
    crate::theme::accent_custom()
        .map(contrast_ink)
        .unwrap_or_else(|| rgb(0x1a1a1a))
}
pub fn sender_label() -> Rgba {
    pick(0x8a8a8c, 0x8a8a8a)
}
pub fn time_label() -> Rgba {
    pick(0x7a7a7c, 0x9a9a9a)
}
pub fn chip_bg() -> Rgba {
    pick(0x2f2f30, 0xe8e8e8)
}
pub fn chip_text() -> Rgba {
    pick(0xa0a0a2, 0x555555)
}

// 输入区
pub fn composer_bg() -> Rgba {
    pick(0x1e1e1f, 0xffffff)
}
pub fn composer_border() -> Rgba {
    pick(0x2f2f30, 0xededed)
}
pub fn send_disabled_bg() -> Rgba {
    pick(0x3a3a3b, 0xe5e5e5)
}
pub fn send_disabled_text() -> Rgba {
    pick(0x7a7a7c, 0x999999)
}
pub fn send_enabled() -> Rgba {
    accent_color()
}

// 通用
pub fn icon() -> Rgba {
    pick(0xa8a8aa, 0x666666)
}
pub fn icon_strong() -> Rgba {
    pick(0xd8d8da, 0x333333)
}
pub fn accent() -> Rgba {
    accent_color()
}
pub fn divider() -> Rgba {
    pick(0x2f2f30, 0xededed)
}

// 信息栏（本皮肤默认不显示；保留 token 供以后做聊天信息面板）
pub fn panel_bg() -> Rgba {
    pick(0x252526, 0xfafafa)
}
pub fn panel_text() -> Rgba {
    pick(0xa9a9ab, 0x555555)
}
pub fn panel_card_bg() -> Rgba {
    pick(0x2f2f30, 0xffffff)
}
pub fn panel_divider() -> Rgba {
    pick(0x3a3a3b, 0xededed)
}
pub fn online_dot() -> Rgba {
    accent_color()
}
/// 群头像默认底色（微信灰白）。
pub fn group_avatar_bg() -> Rgba {
    rgb(0xf2f2f2)
}

/// 导轨背景：微信是纯色，这里仍返回 Background 方便调用点统一。
pub fn nav_gradient() -> Background {
    let color = pick(0x252526, 0xe7e7e7);
    linear_gradient(
        180.0,
        linear_color_stop(color, 0.0),
        linear_color_stop(color, 1.0),
    )
}

pub fn list_gradient() -> Background {
    let color = pick(0x2f2f30, 0xf7f7f7);
    linear_gradient(
        180.0,
        linear_color_stop(color, 0.0),
        linear_color_stop(color, 1.0),
    )
}

/// 选中会话行：直接用主题色实色（深浅色一致），不再做半透明叠加。
pub fn selected_gradient() -> Background {
    let color = accent_color();
    linear_gradient(
        90.0,
        linear_color_stop(color, 0.0),
        linear_color_stop(color, 1.0),
    )
}

/// 微信彩色首字头像色板（深色下低饱和）。
pub const AVATAR_COLORS: [u32; 8] = [
    0x8a5a5a, 0x4a7a5a, 0x4a6a8a, 0x6a5a8a, 0x8a7a4a, 0x5a6a6a, 0x7a4a6a, 0x4a5a7a,
];

pub fn avatar_color(id: &str) -> Rgba {
    let mut hash: u32 = 0;
    for unit in id.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(unit as u32);
    }
    rgb(AVATAR_COLORS[(hash as usize) % AVATAR_COLORS.len()])
}
