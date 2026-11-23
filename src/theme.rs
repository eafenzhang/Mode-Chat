//! 主题 token：与已退役 Web 版 `tokens.css` 同值、同语义，深浅两套运行期切换。
//!
//! 为什么是函数不是常量：浅色/深色要在运行期切换，常量做不到。调用点统一写
//! `theme::bg()` / `theme::ink()`，取值只看当前模式。
//!
//! 真源优先级：Web 源码 (`apps/bot-tauri/frontend/src/ui/tokens.css`) > DS 标准 >
//! 设计稿表格。新增颜色先在这里加，组件里不许写死十六进制。

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use gpui::{px, BoxShadow, FontFallbacks, Hsla, Rgba};

pub const FONT_FAMILY: &str = "Geist";
pub const FONT_MONO: &str = "Consolas";

const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Rgba {
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: a as f32 / 255.0,
    }
}

/// 带浮点透明度的颜色（`color-mix` / `rgb(... / .12)` 的口径）。
const fn rgba_a(r: u8, g: u8, b: u8, a: f32) -> Rgba {
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}

const fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

// ---- 深色（Bot tokens.css 口径）----
const DARK_APP: Rgba = rgba(0x14, 0x14, 0x14, 255);
const DARK_PANEL: Rgba = rgba(0x1b, 0x1b, 0x1b, 255);
const DARK_RAISED: Rgba = rgba(0x26, 0x26, 0x26, 255);
const DARK_RAISED_HOVER: Rgba = rgba(0x30, 0x30, 0x30, 255);
const DARK_COMPOSER: Rgba = rgba(0x1f, 0x1f, 0x1f, 255);
const DARK_MENU: Rgba = rgba(0x1f, 0x1f, 0x1f, 255);
const DARK_CARD: Rgba = rgba(0x1f, 0x1f, 0x1f, 255);
const DARK_INSET: Rgba = rgba(0x16, 0x16, 0x16, 255);
const DARK_CONTROL: Rgba = rgba(0x2b, 0x2b, 0x2b, 255);
const DARK_HAIRLINE: Rgba = rgba(0x3a, 0x3a, 0x3a, 255);
const DARK_INK: Rgba = rgba(0xf5, 0xf5, 0xf5, 255);
const DARK_INK_SECONDARY: Rgba = rgba(0xf5, 0xf5, 0xf5, 0xa8);
const DARK_INK_TERTIARY: Rgba = rgba(0xf5, 0xf5, 0xf5, 0x91);
const DARK_ACCENT: Rgba = rgba(0x10, 0x84, 0xfe, 255);
const DARK_ACCENT_BORDER: Rgba = rgba(0x45, 0x9f, 0xfe, 255);
const DARK_ACCENT_TEXT: Rgba = rgba(0x7f, 0xc0, 0xff, 255);
const DARK_SUCCESS: Rgba = rgba(0x38, 0xd5, 0x91, 255);
const DARK_DANGER: Rgba = rgba(0xff, 0x56, 0x67, 255);
const DARK_WARNING: Rgba = rgba(0xff, 0x98, 0x00, 255);
const DARK_CHIEF: Rgba = rgba(0xe8, 0x99, 0x3a, 255);
const DARK_BUBBLE_USER: Rgba = rgba(0x30, 0x30, 0x30, 255);
const DARK_BUBBLE_USER_INK: Rgba = rgba(0xf5, 0xf5, 0xf5, 255);
const DARK_BUBBLE_ASSISTANT: Rgba = rgba(0x1f, 0x1f, 0x1f, 255);
/// 骨架占位块（比气泡再深一档，呼吸时才看得出来）。
const DARK_SKELETON: Rgba = rgba(0x2a, 0x2a, 0x2a, 255);

// ---- 浅色（DS-Bot 标准 §2.1 DeepSeek 浅色 + 原版 theme-zai-light 口径）----
const LIGHT_APP: Rgba = rgba(0xfc, 0xfc, 0xfc, 255);
const LIGHT_PANEL: Rgba = rgba(0xf7, 0xf7, 0xf7, 255);
const LIGHT_RAISED: Rgba = rgba(0xff, 0xff, 0xff, 255);
const LIGHT_RAISED_HOVER: Rgba = rgba(0xed, 0xed, 0xed, 255);
const LIGHT_COMPOSER: Rgba = rgba(0xff, 0xff, 0xff, 255);
const LIGHT_MENU: Rgba = rgba(0xff, 0xff, 0xff, 255);
const LIGHT_CARD: Rgba = rgba(0xff, 0xff, 0xff, 255);
const LIGHT_INSET: Rgba = rgba(0xf0, 0xf0, 0xf0, 255);
const LIGHT_CONTROL: Rgba = rgba(0xff, 0xff, 0xff, 255);
const LIGHT_HAIRLINE: Rgba = rgba(0xec, 0xec, 0xec, 255);
const LIGHT_INK: Rgba = rgba(0x14, 0x14, 0x14, 255);
const LIGHT_INK_SECONDARY: Rgba = rgba(0x5a, 0x5a, 0x5a, 255);
const LIGHT_INK_TERTIARY: Rgba = rgba(0x6b, 0x6b, 0x6b, 255);
const LIGHT_ACCENT: Rgba = rgba(0x4d, 0x6b, 0xfe, 255);
const LIGHT_ACCENT_BORDER: Rgba = rgba(0x6b, 0x84, 0xff, 255);
const LIGHT_ACCENT_TEXT: Rgba = rgba(0x3b, 0x4f, 0xd8, 255);
const LIGHT_SUCCESS: Rgba = rgba(0x0d, 0x7a, 0x54, 255);
const LIGHT_DANGER: Rgba = rgba(0xc0, 0x2c, 0x3c, 255);
const LIGHT_WARNING: Rgba = rgba(0x9a, 0x5b, 0x00, 255);
const LIGHT_CHIEF: Rgba = rgba(0xc7, 0x7f, 0x1f, 255);
/// 浅色用户气泡是近黑底白字（原版口径，不是浅灰泡）。
const LIGHT_BUBBLE_USER: Rgba = rgba(0x07, 0x07, 0x07, 255);
const LIGHT_BUBBLE_USER_INK: Rgba = rgba(0xfc, 0xfc, 0xfc, 255);
const LIGHT_BUBBLE_ASSISTANT: Rgba = rgba(0xee, 0xee, 0xee, 255);
const LIGHT_SKELETON: Rgba = rgba(0xe8, 0xe8, 0xe8, 255);

// ---- 成员色（12 色，深/浅各一套）----
const MEMBER_DARK: [Rgba; 12] = [
    rgba(0xff, 0x8a, 0x80, 255),
    rgba(0xff, 0xb2, 0x6b, 255),
    rgba(0xff, 0xd5, 0x4f, 255),
    rgba(0x7e, 0xe7, 0x87, 255),
    rgba(0x56, 0xd4, 0xdd, 255),
    rgba(0x79, 0xb8, 0xff, 255),
    rgba(0xb3, 0x92, 0xf0, 255),
    rgba(0xf6, 0x92, 0xce, 255),
    rgba(0xc9, 0xd1, 0xd9, 255),
    rgba(0xff, 0xc0, 0xa8, 255),
    rgba(0x9f, 0xf0, 0xc6, 255),
    rgba(0xd0, 0xbf, 0xff, 255),
];
const MEMBER_LIGHT: [Rgba; 12] = [
    rgba(0xc6, 0x28, 0x28, 255),
    rgba(0xb2, 0x6a, 0x00, 255),
    rgba(0x8a, 0x6d, 0x00, 255),
    rgba(0x1a, 0x7f, 0x37, 255),
    rgba(0x0e, 0x7a, 0x8a, 255),
    rgba(0x13, 0x5f, 0xb8, 255),
    rgba(0x6c, 0x3f, 0xc9, 255),
    rgba(0xb0, 0x32, 0x7e, 255),
    rgba(0x57, 0x60, 0x6a, 255),
    rgba(0xa3, 0x4a, 0x1f, 255),
    rgba(0x0f, 0x7b, 0x5f, 255),
    rgba(0x5b, 0x4b, 0xb5, 255),
];

static LIGHT: AtomicBool = AtomicBool::new(false);

/// 当前是否浅色模式。
pub fn is_light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

/// 只切运行期模式、不落盘（企微皮肤启动时强制深色，退出后不影响用户偏好）。
pub fn force_dark() {
    LIGHT.store(false, Ordering::Relaxed);
}

/// 只切运行期模式、不落盘（模式偏好由 main 的 prefs `theme` 统一管理）。
pub fn set_light_runtime(light: bool) {
    LIGHT.store(light, Ordering::Relaxed);
}

/// 切换主题（同时落盘，下次启动沿用）。返回切换后的模式。
pub fn set_light(light: bool) -> bool {
    LIGHT.store(light, Ordering::Relaxed);
    persist(light);
    light
}

/// 启动时读偏好（文件缺失/损坏按深色）。
///
/// 偏好统一存 `prefs.json`（`{"theme":"light"}`，以后加字段直接加键、不清旧值）；
/// 旧的 `theme.txt` 只做一次迁移：读到就转写进 prefs.json 并删掉旧文件。
pub fn load_preference() {
    if let Some(prefs) = read_prefs() {
        if prefs.get("theme").and_then(serde_json::Value::as_str) == Some("light") {
            LIGHT.store(true, Ordering::Relaxed);
        }
        return;
    }
    let legacy = legacy_path();
    let Ok(text) = std::fs::read_to_string(&legacy) else {
        return;
    };
    let light = text.trim() == "light";
    LIGHT.store(light, Ordering::Relaxed);
    if persist(light) {
        let _ = std::fs::remove_file(legacy);
    }
}

fn prefs_path() -> std::path::PathBuf {
    crate::instance::state_dir().join("prefs.json")
}

fn legacy_path() -> std::path::PathBuf {
    crate::instance::state_dir().join("theme.txt")
}

fn read_prefs() -> Option<serde_json::Value> {
    let bytes = std::fs::read(prefs_path()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// 落盘主题（保留 prefs.json 里的其它键）。返回是否写成功。
fn persist(light: bool) -> bool {
    let path = prefs_path();
    let mut value = read_prefs().unwrap_or_else(|| serde_json::json!({}));
    if !value.is_object() {
        value = serde_json::json!({});
    }
    value["theme"] = serde_json::Value::String(if light { "light" } else { "dark" }.to_string());
    atomic_file::write_json_atomic(&path, &value, &atomic_file::WriteOptions::default()).is_ok()
}

fn pick(dark: Rgba, light: Rgba) -> Rgba {
    if is_light() { light } else { dark }
}

// ---- 表面 ----
pub fn app() -> Rgba {
    pick(DARK_APP, LIGHT_APP)
}
pub fn panel() -> Rgba {
    pick(DARK_PANEL, LIGHT_PANEL)
}
pub fn raised() -> Rgba {
    pick(DARK_RAISED, LIGHT_RAISED)
}
pub fn raised_hover() -> Rgba {
    pick(DARK_RAISED_HOVER, LIGHT_RAISED_HOVER)
}
pub fn composer() -> Rgba {
    pick(DARK_COMPOSER, LIGHT_COMPOSER)
}
pub fn menu() -> Rgba {
    pick(DARK_MENU, LIGHT_MENU)
}
pub fn card() -> Rgba {
    pick(DARK_CARD, LIGHT_CARD)
}
pub fn inset() -> Rgba {
    pick(DARK_INSET, LIGHT_INSET)
}
pub fn control() -> Rgba {
    pick(DARK_CONTROL, LIGHT_CONTROL)
}
pub fn hairline() -> Rgba {
    pick(DARK_HAIRLINE, LIGHT_HAIRLINE)
}
/// 输入卡底：原版 `panel 80% + blur` 的实色近似（D1）。
pub fn composer_card() -> Rgba {
    mix(panel(), app(), 0.2)
}
pub fn bg() -> Rgba {
    app()
}

// ---- 文字 ----
pub fn ink() -> Rgba {
    pick(DARK_INK, LIGHT_INK)
}
pub fn ink_secondary() -> Rgba {
    pick(DARK_INK_SECONDARY, LIGHT_INK_SECONDARY)
}
pub fn ink_tertiary() -> Rgba {
    pick(DARK_INK_TERTIARY, LIGHT_INK_TERTIARY)
}
/// 1px 描边（原版 `--bot-edge`：ink 8%；浅色再收一档，避免白底上线条发重）。
pub fn edge() -> Rgba {
    let ink = ink();
    rgba_a(
        (ink.r * 255.0) as u8,
        (ink.g * 255.0) as u8,
        (ink.b * 255.0) as u8,
        if is_light() { 0.06 } else { 0.08 },
    )
}
/// 输入卡聚焦/悬停描边（DS `[data-composer-card]:focus-within`：ink 30%）。
pub fn ink_line() -> Rgba {
    let ink = ink();
    rgba_a(
        (ink.r * 255.0) as u8,
        (ink.g * 255.0) as u8,
        (ink.b * 255.0) as u8,
        0.30,
    )
}
/// 悬停底色（原版 `--bot-hover`）。
pub fn hover() -> Rgba {
    if is_light() {
        rgba_a(0x14, 0x14, 0x14, 0.06)
    } else {
        rgba_a(0xa0, 0xa0, 0xa0, 0.12)
    }
}
/// ink 指定透明度的叠色（原版大量使用 `color-mix(... ink N%)`）。
fn ink_alpha(alpha: f32) -> Rgba {
    let ink = ink();
    rgba_a(
        (ink.r * 255.0) as u8,
        (ink.g * 255.0) as u8,
        (ink.b * 255.0) as u8,
        alpha,
    )
}

/// 圆形小按钮的静态底色（原版 composer 的 ink 5%）。
pub fn subtle() -> Rgba {
    ink_alpha(0.05)
}

/// 圆形小按钮悬停底色（原版 ink 10%）。
pub fn subtle_hover() -> Rgba {
    ink_alpha(0.10)
}

/// 近黑品牌标的浅色底板：深色主题下给 GitHub / Linear 这类 logo 垫底，
/// 否则它们和 `#262626` 图标槽几乎同色（对比度 < 1.2:1）。
pub fn avatar_light_plate() -> Rgba {
    if is_light() {
        rgba(0xec, 0xec, 0xec, 255)
    } else {
        rgba(0xf0, 0xf0, 0xf0, 255)
    }
}

/// 占位文字（原版 `ink 36%`）。
pub fn ink_placeholder() -> Rgba {
    ink_alpha(0.36)
}

/// 选中底色（原版 `--bot-selected`：ink 15%）。
pub fn selected() -> Rgba {
    let ink = ink();
    rgba_a(
        (ink.r * 255.0) as u8,
        (ink.g * 255.0) as u8,
        (ink.b * 255.0) as u8,
        0.15,
    )
}

// ---- 强调与状态 ----
static ACCENT_OVERRIDE: OnceLock<RwLock<Option<Rgba>>> = OnceLock::new();

fn accent_override() -> Option<Rgba> {
    *ACCENT_OVERRIDE
        .get_or_init(|| RwLock::new(None))
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 当前强调色覆盖（供微信皮肤优先使用用户选的颜色；None = 微信绿）。
pub fn accent_custom() -> Option<Rgba> {
    accent_override()
}

/// 运行时强调色覆盖（None = 跟随深浅主题默认值）。
pub fn set_accent(value: Option<Rgba>) {
    *ACCENT_OVERRIDE
        .get_or_init(|| RwLock::new(None))
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = value;
}

pub fn accent() -> Rgba {
    accent_override().unwrap_or_else(|| pick(DARK_ACCENT, LIGHT_ACCENT))
}
pub fn accent_border() -> Rgba {
    accent_override().unwrap_or_else(|| pick(DARK_ACCENT_BORDER, LIGHT_ACCENT_BORDER))
}
pub fn accent_text() -> Rgba {
    if let Some(accent) = accent_override() {
        // 自定义强调色直接当文字色会踩到暗色系：按背景亮度把过暗/过亮的色拉回可读区间。
        let luma = 0.2126 * accent.r + 0.7152 * accent.g + 0.0722 * accent.b;
        let lift = |amount: f32| Rgba {
            r: accent.r + (1.0 - accent.r) * amount,
            g: accent.g + (1.0 - accent.g) * amount,
            b: accent.b + (1.0 - accent.b) * amount,
            a: accent.a,
        };
        let sink = |amount: f32| Rgba {
            r: accent.r * (1.0 - amount),
            g: accent.g * (1.0 - amount),
            b: accent.b * (1.0 - amount),
            a: accent.a,
        };
        if is_light() {
            return if luma > 0.72 { sink(0.45) } else { accent };
        }
        return if luma < 0.35 { lift(0.55) } else { accent };
    }
    pick(DARK_ACCENT_TEXT, LIGHT_ACCENT_TEXT)
}
/// 强调色上的文字/图标色（DS `--bt-accent-ink`：两种主题下都用近白）。
pub fn accent_ink() -> Rgba {
    rgba(0xff, 0xff, 0xff, 255)
}
pub fn accent_soft() -> Rgba {
    let accent = accent();
    rgba_a(
        (accent.r * 255.0) as u8,
        (accent.g * 255.0) as u8,
        (accent.b * 255.0) as u8,
        if is_light() { 0.12 } else { 0.14 },
    )
}
pub fn success() -> Rgba {
    pick(DARK_SUCCESS, LIGHT_SUCCESS)
}
pub fn success_soft() -> Rgba {
    let color = success();
    rgba_a(
        (color.r * 255.0) as u8,
        (color.g * 255.0) as u8,
        (color.b * 255.0) as u8,
        if is_light() { 0.14 } else { 0.16 },
    )
}
pub fn danger() -> Rgba {
    pick(DARK_DANGER, LIGHT_DANGER)
}
pub fn danger_soft() -> Rgba {
    let color = danger();
    rgba_a(
        (color.r * 255.0) as u8,
        (color.g * 255.0) as u8,
        (color.b * 255.0) as u8,
        if is_light() { 0.12 } else { 0.15 },
    )
}
pub fn warning() -> Rgba {
    pick(DARK_WARNING, LIGHT_WARNING)
}
pub fn warning_soft() -> Rgba {
    let color = warning();
    rgba_a(
        (color.r * 255.0) as u8,
        (color.g * 255.0) as u8,
        (color.b * 255.0) as u8,
        if is_light() { 0.14 } else { 0.16 },
    )
}
pub fn chief() -> Rgba {
    pick(DARK_CHIEF, LIGHT_CHIEF)
}

// ---- 代码高亮 ----
/// 语法高亮「类型」色（沿用试点原有紫，深浅两套）。
pub fn code_type() -> Rgba {
    pick(rgba(0xa7, 0x8b, 0xfa, 255), rgba(0x6d, 0x4b, 0xd8, 255))
}

// ---- 气泡 ----
pub fn bubble_user() -> Rgba {
    pick(DARK_BUBBLE_USER, LIGHT_BUBBLE_USER)
}
pub fn bubble_user_ink() -> Rgba {
    pick(DARK_BUBBLE_USER_INK, LIGHT_BUBBLE_USER_INK)
}
pub fn bubble_assistant() -> Rgba {
    pick(DARK_BUBBLE_ASSISTANT, LIGHT_BUBBLE_ASSISTANT)
}
/// 骨架占位块底色（加载态专用）。
pub fn skeleton() -> Rgba {
    pick(DARK_SKELETON, LIGHT_SKELETON)
}

// ---- 成员色 ----
/// 成员 id → 色号（0..11）。算法照原版 `memberColors.ts`：64 位 FNV-1a + 低 16 位折叠。
pub fn member_color_index(id: &str) -> usize {
    let key = id.trim();
    if key.is_empty() {
        return 0;
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for unit in key.encode_utf16() {
        hash ^= unit as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let low = (hash & 0xff) + ((hash >> 8) & 0xff) * 256;
    (low % 12) as usize
}

pub fn member_color(id: &str) -> Rgba {
    let index = member_color_index(id);
    pick(MEMBER_DARK[index], MEMBER_LIGHT[index])
}

// ---- 阴影 ----
fn shadow(x: f32, y: f32, blur: f32, spread: f32, color: Rgba) -> BoxShadow {
    BoxShadow::new(px(x), px(y), Hsla::from(color))
        .blur_radius(px(blur))
        .spread_radius(px(spread))
}

const SHADOW_DARK: Rgba = rgba(0x00, 0x00, 0x00, 255);
const SHADOW_LIGHT: Rgba = rgba(0x1e, 0x18, 0x10, 255);

fn shadow_color(alpha: f32) -> Rgba {
    let base = if is_light() { SHADOW_LIGHT } else { SHADOW_DARK };
    rgba_a(
        (base.r * 255.0) as u8,
        (base.g * 255.0) as u8,
        (base.b * 255.0) as u8,
        alpha,
    )
}

/// 1 档：浮起（顶部胶囊、输入卡）。
pub fn shadow_1() -> Vec<BoxShadow> {
    vec![
        shadow(0., 2., 8., -1., shadow_color(if is_light() { 0.08 } else { 0.06 })),
        shadow(0., 1., 2., 0., shadow_color(if is_light() { 0.05 } else { 0.04 })),
    ]
}

/// 2 档：弹出（备用，当前无消费方）。
pub fn shadow_2() -> Vec<BoxShadow> {
    vec![
        shadow(0., 10., 20., -3., shadow_color(if is_light() { 0.12 } else { 0.10 })),
        shadow(0., 4., 6., -4., shadow_color(0.10)),
    ]
}

/// 3 档：提示（菜单、Toast）。
pub fn shadow_3() -> Vec<BoxShadow> {
    vec![shadow(
        0.,
        8.,
        28.,
        0.,
        shadow_color(if is_light() { 0.16 } else { 0.14 }),
    )]
}

/// 4 档：对话框。
pub fn shadow_4() -> Vec<BoxShadow> {
    vec![shadow(
        0.,
        24.,
        80.,
        0.,
        shadow_color(0.24),
    )]
}

/// 浮起表面外圈 1px ink 4%（原版 `0 0 0 1px color-mix(... 4%)`）。
pub fn ring_4() -> BoxShadow {
    let ink = ink();
    shadow(
        0.,
        0.,
        0.,
        1.,
        rgba_a(
            (ink.r * 255.0) as u8,
            (ink.g * 255.0) as u8,
            (ink.b * 255.0) as u8,
            0.04,
        ),
    )
}

// ---- 字体 ----
/// 界面字体回退链（Geist 只覆盖拉丁；中文交回系统）。
pub fn font_fallbacks() -> FontFallbacks {
    FontFallbacks::from_fonts(vec![
        "Segoe UI".to_string(),
        "Microsoft YaHei UI".to_string(),
        "Microsoft YaHei".to_string(),
        "PingFang SC".to_string(),
        "Noto Sans CJK SC".to_string(),
    ])
}

// ---- 圆角 ----
/// 行内标签（DS：4）。
pub const RADIUS_TAG: f32 = 4.0;
/// 菜单项/芯片（DS：6）。
pub const RADIUS_CHIP: f32 = 6.0;
/// 输入框/下拉（DS：8）。
pub const RADIUS_FIELD: f32 = 8.0;
/// 分段控件/文字按钮（DS：10）。
pub const RADIUS_CONTROL: f32 = 10.0;
/// 菜单/Toast（DS：12）。
pub const RADIUS_MENU: f32 = 12.0;
/// 卡片/设置卡/侧栏行（原版：14）。
pub const RADIUS_CARD: f32 = 14.0;
/// 对话框（DS：16）。
pub const RADIUS_DIALOG: f32 = 16.0;
/// 气泡（DS：18）。
pub const RADIUS_BUBBLE: f32 = 18.0;
/// 输入卡（DS：22；多行/带附件 18）。
pub const RADIUS_COMPOSER: f32 = 22.0;
pub const RADIUS_COMPOSER_MULTI: f32 = 18.0;
/// 分段控件里的项（DS：8；保留旧名）。
pub const RADIUS_SEGMENT: f32 = 8.0;

/// 全圆角（胶囊 / 头像 / 开关）。999 足够大，等价于 50%。
pub const RADIUS_PILL: f32 = 999.0;

// ---- 动效 ----
pub const DUR_FAST: Duration = Duration::from_millis(120);
pub const DUR_BASE: Duration = Duration::from_millis(160);
pub const DUR_PANEL: Duration = Duration::from_millis(240);

/// 0=quiet、1=normal、2=lively；只影响统一入场动效。
static MOTION_LEVEL: AtomicU8 = AtomicU8::new(1);

pub fn set_motion(level: &str) {
    MOTION_LEVEL.store(
        match level {
            "quiet" => 0,
            "lively" => 2,
            _ => 1,
        },
        Ordering::Relaxed,
    );
}

/// 统一动效时长（quiet 近似瞬时，lively 拉长）。
pub fn motion_base() -> Duration {
    match MOTION_LEVEL.load(Ordering::Relaxed) {
        0 => Duration::from_millis(1),
        2 => Duration::from_millis(220),
        _ => DUR_BASE,
    }
}

/// 轻度过冲的弹簧曲线（DS 原则「形状只用弹簧」的最小实现）：
/// 0→1，最后约 5% 时间轻微过冲再收回。用于入场位移与透明度。
pub fn spring_ease(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    let omega = 12.0f32;
    let damping = 7.0f32;
    let value = 1.0
        - (-damping * t).exp() * ((omega * t).cos() + (damping / omega) * (omega * t).sin());
    value.clamp(0., 1.1)
}

/// 标准 ease-out（设置页切换用）。
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    1.0 - (1.0 - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_tokens_match_web() {
        assert_eq!(DARK_APP, rgba(0x14, 0x14, 0x14, 255));
        assert_eq!(DARK_PANEL, rgba(0x1b, 0x1b, 0x1b, 255));
        assert_eq!(DARK_BUBBLE_USER, rgba(0x30, 0x30, 0x30, 255));
        assert_eq!(DARK_INK_SECONDARY, rgba(0xf5, 0xf5, 0xf5, 0xa8));
    }

    #[test]
    fn light_tokens_match_web() {
        assert_eq!(LIGHT_APP, rgba(0xfc, 0xfc, 0xfc, 255));
        assert_eq!(LIGHT_PANEL, rgba(0xf7, 0xf7, 0xf7, 255));
        assert_eq!(LIGHT_BUBBLE_USER, rgba(0x07, 0x07, 0x07, 255));
        assert_eq!(LIGHT_INK_SECONDARY, rgba(0x5a, 0x5a, 0x5a, 255));
    }

    #[test]
    fn member_colors_match_web() {
        assert_eq!(member_color_index("bot-1"), 8);
        assert_eq!(member_color_index("mode-bot"), 10);
        assert_eq!(member_color_index("a"), 4);
        assert_eq!(member_color_index("娜娜"), 5);
        assert_eq!(member_color_index("  bot-1  "), 8);
        assert_eq!(member_color_index(""), 0);
    }

    #[test]
    fn shadow_ladders_have_expected_stops() {
        assert_eq!(shadow_1().len(), 2);
        assert_eq!(shadow_2().len(), 2);
        assert_eq!(shadow_3().len(), 1);
        assert_eq!(shadow_4().len(), 1);
        assert_eq!(ring_4().spread_radius, px(1.));
        assert!(!ring_4().inset);
    }

    #[test]
    fn easings_land_on_endpoints() {
        assert!(spring_ease(0.).abs() < 0.001);
        assert!((spring_ease(1.) - 1.0).abs() < 0.001);
        assert_eq!(ease_out(0.), 0.);
        assert_eq!(ease_out(1.), 1.);
    }
}
