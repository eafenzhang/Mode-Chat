//! 图标（Lucide，ISC）。
//!
//! 为什么用 SVG 原文而不是手绘 path：GPUI 的 `svg().data()` 直接渲染同源 SVG，
//! 几何/线宽/圆角与设计稿一致，不需要近似重画。
//!
//! 资产在 `assets/icons/ui/`，由 `scripts/gen-icons-lobe.py` 一次性生成
//! （不进构建链）。画布 24×24，stroke-width 2，round cap/join。
//!
//! **纯线框，没有实心变体**：选中态靠颜色和底色区分，不再切换字形。
//!
//! 颜色走 `currentColor`：调用方用 `text_color` 指定，悬停换色不重新栅格化。
//!
//! 选型历程：Phosphor（实心、方）→ Remix（线框、圆）→ Boxicons → **Lucide**。
//! Lucide 圆润度实测 8%（全场最低＝最圆），复杂度 39 命令，ISC 许可，
//! 且整个库只有线框一种风格。

use gpui::{px, svg, Hsla, IntoElement, Rgba, Styled};

/// 图标种类（只收当前界面用到的；新增图标先加资产再加枚举）。
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Search,
    Plus,
    Ellipsis,
    Settings,
    X,
    Check,
    ArrowUp,
    ArrowDown,
    Stop,
    ChevronLeft,
    ChevronRight,
    Paperclip,
    Minus,
    WindowMaximize,
    WindowRestore,
    PanelRight,
    Hash,
    Pin,
    Star,
    Crown,
    Pencil,
    Trash,
    EyeOff,
    Archive,
    Chart,
    Cpu,
    Plug,
    Calendar,
    Refresh,
    Download,
    MessageCircle,
    Bot,
    ListChecks,
    Building,
    Smile,
    Shield,
    Terminal,
    Folder,
    Scissors,
    Mic,
    Copy,
    BellOff,
    Key,
    Info,
    MessageSquare,
    Sparkles,
    Contacts,
    MessageCircleFilled,
    Brain,
    Pushpin,
    Sun,
    Moon,
    Monitor,
    ChevronDown,
    /// 展开右侧栏（侧栏轮廓，面板在右）
    SidebarExpand,
    /// 折叠右侧栏（侧栏轮廓，面板在右，作为收起态的同族图标）
    SidebarCollapse,
    /// 折叠右侧栏（双箭头向右收拢）
    PaneCollapse,
    /// 展开（双箭头向左）
    PaneExpand,
}

/// 图标字重。Remix 只提供 `-line` 与 `-fill` 两种；light 与 regular 同源。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Weight {
    /// 常规（默认）
    #[default]
    Regular,
    /// 细线：比 regular 更轻，用于标题栏/输入框这类需要退后的位置
    Light,
    /// 实心：`MessageCircleFilled` 这类本来就只有 fill 的图标同义
    #[allow(dead_code)] // 菜单图标用各自的 fill 资产，这里只为公共 API 完整
    Fill,
}

impl Icon {
    /// 全部图标（测试与清单用；顺序与 `svg()` 无关）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub const ALL: [Icon; 58] = [
        Icon::Search,
        Icon::Plus,
        Icon::Ellipsis,
        Icon::Settings,
        Icon::X,
        Icon::Check,
        Icon::ArrowUp,
        Icon::ArrowDown,
        Icon::Stop,
        Icon::ChevronLeft,
        Icon::ChevronRight,
        Icon::Paperclip,
        Icon::Minus,
        Icon::WindowMaximize,
        Icon::WindowRestore,
        Icon::PanelRight,
        Icon::Hash,
        Icon::Pin,
        Icon::Star,
        Icon::Crown,
        Icon::Pencil,
        Icon::Trash,
        Icon::EyeOff,
        Icon::Archive,
        Icon::Chart,
        Icon::Cpu,
        Icon::Plug,
        Icon::Calendar,
        Icon::Refresh,
        Icon::Download,
        Icon::MessageCircle,
        Icon::Bot,
        Icon::ListChecks,
        Icon::Building,
        Icon::Smile,
        Icon::Shield,
        Icon::Terminal,
        Icon::Folder,
        Icon::Scissors,
        Icon::Mic,
        Icon::Copy,
        Icon::BellOff,
        Icon::Key,
        Icon::Info,
        Icon::MessageSquare,
        Icon::Sparkles,
        Icon::Contacts,
        Icon::MessageCircleFilled,
        Icon::Brain,
        Icon::Pushpin,
        Icon::Sun,
        Icon::Moon,
        Icon::Monitor,
        Icon::ChevronDown,
        Icon::SidebarExpand,
        Icon::SidebarCollapse,
        Icon::PaneCollapse,
        Icon::PaneExpand,
    ];

    /// 图标对应的 SVG 原文（regular 字重）。
    pub fn svg(self) -> &'static [u8] {
        self.svg_at(Weight::Regular)
    }

    /// 按字重取 SVG。缺少 light 变体的图标（目前只有 `MessageCircleFilled`）
    /// 回落到 regular，保证调用方不用逐图标判断。
    pub fn svg_at(self, weight: Weight) -> &'static [u8] {
        match weight {
            Weight::Regular | Weight::Fill => self.regular_svg(),
            Weight::Light => self.light_svg(),
        }
    }

    fn regular_svg(self) -> &'static [u8] {
        match self {
            Icon::Search => include_bytes!("../assets/icons/ui/search.svg"),
            Icon::Plus => include_bytes!("../assets/icons/ui/plus.svg"),
            Icon::Ellipsis => include_bytes!("../assets/icons/ui/ellipsis.svg"),
            Icon::Settings => include_bytes!("../assets/icons/ui/settings.svg"),
            Icon::X => include_bytes!("../assets/icons/ui/x.svg"),
            Icon::Check => include_bytes!("../assets/icons/ui/check.svg"),
            Icon::ArrowUp => include_bytes!("../assets/icons/ui/arrow-up.svg"),
            Icon::ArrowDown => include_bytes!("../assets/icons/ui/arrow-down.svg"),
            Icon::Stop => include_bytes!("../assets/icons/ui/circle-stop.svg"),
            Icon::ChevronLeft => include_bytes!("../assets/icons/ui/chevron-left.svg"),
            Icon::ChevronRight => include_bytes!("../assets/icons/ui/chevron-right.svg"),
            Icon::ChevronDown => include_bytes!("../assets/icons/ui/chevron-down.svg"),
            Icon::Paperclip => include_bytes!("../assets/icons/ui/paperclip.svg"),
            Icon::Minus => include_bytes!("../assets/icons/ui/minus.svg"),
            Icon::WindowMaximize => include_bytes!("../assets/icons/ui/square.svg"),
            Icon::WindowRestore => include_bytes!("../assets/icons/ui/copy.svg"),
            Icon::PanelRight => include_bytes!("../assets/icons/ui/panel-right.svg"),
            Icon::Hash => include_bytes!("../assets/icons/ui/hash.svg"),
            Icon::Pin => include_bytes!("../assets/icons/ui/pin.svg"),
            Icon::Star => include_bytes!("../assets/icons/ui/star.svg"),
            Icon::Crown => include_bytes!("../assets/icons/ui/crown.svg"),
            Icon::Pencil => include_bytes!("../assets/icons/ui/pencil.svg"),
            Icon::Trash => include_bytes!("../assets/icons/ui/trash.svg"),
            Icon::EyeOff => include_bytes!("../assets/icons/ui/eye-off.svg"),
            Icon::Archive => include_bytes!("../assets/icons/ui/archive.svg"),
            Icon::Chart => include_bytes!("../assets/icons/ui/chart-line.svg"),
            Icon::Cpu => include_bytes!("../assets/icons/ui/cpu.svg"),
            Icon::Plug => include_bytes!("../assets/icons/ui/plug.svg"),
            Icon::Calendar => include_bytes!("../assets/icons/ui/calendar.svg"),
            Icon::Refresh => include_bytes!("../assets/icons/ui/refresh-cw.svg"),
            Icon::Download => include_bytes!("../assets/icons/ui/download.svg"),
            Icon::MessageCircle => include_bytes!("../assets/icons/ui/message-circle.svg"),
            Icon::Bot => include_bytes!("../assets/icons/ui/bot.svg"),
            Icon::ListChecks => include_bytes!("../assets/icons/ui/list-checks.svg"),
            Icon::Building => include_bytes!("../assets/icons/ui/building.svg"),
            Icon::Smile => include_bytes!("../assets/icons/ui/smile.svg"),
            Icon::Shield => include_bytes!("../assets/icons/ui/shield.svg"),
            Icon::Terminal => include_bytes!("../assets/icons/ui/terminal.svg"),
            Icon::Folder => include_bytes!("../assets/icons/ui/folder.svg"),
            Icon::Scissors => include_bytes!("../assets/icons/ui/scissors.svg"),
            Icon::Mic => include_bytes!("../assets/icons/ui/mic.svg"),
            Icon::Copy => include_bytes!("../assets/icons/ui/copy.svg"),
            Icon::BellOff => include_bytes!("../assets/icons/ui/bell-off.svg"),
            Icon::Key => include_bytes!("../assets/icons/ui/key.svg"),
            Icon::Info => include_bytes!("../assets/icons/ui/info.svg"),
            Icon::MessageSquare => include_bytes!("../assets/icons/ui/message-square.svg"),
            Icon::Sparkles => include_bytes!("../assets/icons/ui/sparkles.svg"),
            Icon::Contacts => include_bytes!("../assets/icons/ui/users.svg"),
            Icon::MessageCircleFilled => include_bytes!("../assets/icons/ui/message-circle.svg"),
            Icon::Brain => include_bytes!("../assets/icons/ui/brain.svg"),
            Icon::Pushpin => include_bytes!("../assets/icons/ui/pin.svg"),
            Icon::Sun => include_bytes!("../assets/icons/ui/sun.svg"),
            Icon::Moon => include_bytes!("../assets/icons/ui/moon.svg"),
            Icon::Monitor => include_bytes!("../assets/icons/ui/monitor.svg"),
            Icon::SidebarExpand => include_bytes!("../assets/icons/ui/panel-left.svg"),
            Icon::SidebarCollapse => include_bytes!("../assets/icons/ui/panel-right.svg"),
            Icon::PaneCollapse => include_bytes!("../assets/icons/ui/chevrons-right.svg"),
            Icon::PaneExpand => include_bytes!("../assets/icons/ui/chevrons-left.svg"),
        }
    }

    /// 细线变体。Lucide 只有一档字重，线框本身就是最轻的，
    /// 因此这里与 regular 相同；保留接口是为了不动调用方。
    fn light_svg(self) -> &'static [u8] {
        self.regular_svg()
    }
}




/// 画一个图标（regular 字重）。`size` 是边长，颜色由调用方给（`currentColor`）。
pub fn icon(kind: Icon, size: f32, color: Rgba) -> impl IntoElement {
    svg()
        .data(kind.svg())
        .size(px(size))
        .text_color(Hsla::from(color))
}

/// 细线版。
///
/// 旧接口 `icon_with_stroke(.., stroke_width)` 靠替换 SVG 里的 `stroke-width` 字符串；
/// Remix 的 `-line` 是填充路径、同样没有这个属性，那种做法会静默失效。
/// 现在该函数与 [`icon`] 等价（Remix 没有 light 字重），保留只为不动调用方。
pub fn icon_light(kind: Icon, size: f32, color: Rgba) -> impl IntoElement {
    svg()
        .data(kind.svg_at(Weight::Light))
        .size(px(size))
        .text_color(Hsla::from(color))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_has_well_formed_svg() {
        for icon in Icon::ALL {
            for weight in [Weight::Regular, Weight::Light] {
                let bytes = icon.svg_at(weight);
                assert!(bytes.starts_with(b"<svg"), "{icon:?}/{weight:?} 不是 svg");
                let text = std::str::from_utf8(bytes).unwrap();
                assert!(
                    text.contains("viewBox=\"0 0 24 24\""),
                    "{icon:?}/{weight:?} 缺 24 画布"
                );
                assert!(
                    text.contains("currentColor"),
                    "{icon:?}/{weight:?} 缺 currentColor"
                );
                assert!(text.trim_end().ends_with("</svg>"), "{icon:?}/{weight:?} 未闭合");
            }
        }
    }

    #[test]
    fn icon_set_matches_design_inventory() {
        assert_eq!(Icon::ALL.len(), 58);
    }

    #[test]
    fn light_weight_falls_back_to_regular() {
        // Remix 没有 light 字重：`-line` 本身就是线框，light 与 regular 同源。
        for icon in Icon::ALL {
            assert_eq!(
                icon.svg_at(Weight::Regular),
                icon.svg_at(Weight::Light),
                "{icon:?} 的 light 应与 regular 同源"
            );
        }
    }
}
