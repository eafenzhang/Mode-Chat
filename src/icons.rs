//! 图标（Phosphor Icons，MIT）。
//!
//! 为什么用 SVG 原文而不是手绘 path：GPUI 的 `svg().data()` 直接渲染同源 SVG，
//! 几何/线宽/圆角与设计稿一致，不需要近似重画。
//!
//! 资产在 `assets/icons/phosphor/`，由 `scripts/gen-icons-lobe.py` 一次性生成
//! （不进构建链）。画布是 256×256（Phosphor 的原始网格），不是 24×24。
//!
//! 颜色走 `currentColor`：调用方用 `text_color` 指定，悬停换色不重新栅格化。
//!
//! **粗细由字重决定，不是 stroke-width**：Phosphor 是纯填充路径，没有描边属性。
//! 需要更细的画面用 [`icon_light`]，想按数值微调的地方请改用 `Icon` 的 light 变体。

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
    /// 展开右侧栏（侧栏轮廓）
    SidebarExpand,
    /// 折叠右侧栏（双箭头向右收拢）
    PaneCollapse,
    /// 展开（双箭头向左）
    PaneExpand,
}

/// 图标字重。Phosphor 每个图标提供 regular / light / bold / fill 等变体，
/// 这里的取值决定读哪个 SVG 文件。
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
    pub const ALL: [Icon; 55] = [
        Icon::Search,
        Icon::Plus,
        Icon::Ellipsis,
        Icon::Settings,
        Icon::X,
        Icon::Check,
        Icon::ArrowUp,
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
            Icon::Search => include_bytes!("../assets/icons/phosphor/magnifying-glass.svg"),
            Icon::Plus => include_bytes!("../assets/icons/phosphor/plus.svg"),
            Icon::Ellipsis => include_bytes!("../assets/icons/phosphor/dots-three.svg"),
            Icon::Settings => include_bytes!("../assets/icons/phosphor/gear.svg"),
            Icon::X => include_bytes!("../assets/icons/phosphor/x.svg"),
            Icon::Check => include_bytes!("../assets/icons/phosphor/check.svg"),
            Icon::ArrowUp => include_bytes!("../assets/icons/phosphor/arrow-up.svg"),
            Icon::Stop => include_bytes!("../assets/icons/phosphor/square.svg"),
            Icon::ChevronLeft => include_bytes!("../assets/icons/phosphor/caret-left.svg"),
            Icon::ChevronRight => include_bytes!("../assets/icons/phosphor/caret-right.svg"),
            Icon::Paperclip => include_bytes!("../assets/icons/phosphor/paperclip.svg"),
            Icon::Minus => include_bytes!("../assets/icons/phosphor/minus.svg"),
            Icon::WindowMaximize => include_bytes!("../assets/icons/phosphor/square.svg"),
            Icon::WindowRestore => include_bytes!("../assets/icons/phosphor/copy.svg"),
            Icon::PanelRight => include_bytes!("../assets/icons/phosphor/sidebar-simple.svg"),
            Icon::Hash => include_bytes!("../assets/icons/phosphor/hash.svg"),
            Icon::Pin => include_bytes!("../assets/icons/phosphor/push-pin.svg"),
            Icon::Star => include_bytes!("../assets/icons/phosphor/star.svg"),
            Icon::Crown => include_bytes!("../assets/icons/phosphor/crown.svg"),
            Icon::Pencil => include_bytes!("../assets/icons/phosphor/pencil-simple.svg"),
            Icon::Trash => include_bytes!("../assets/icons/phosphor/trash.svg"),
            Icon::EyeOff => include_bytes!("../assets/icons/phosphor/eye-slash.svg"),
            Icon::Archive => include_bytes!("../assets/icons/phosphor/archive.svg"),
            Icon::Chart => include_bytes!("../assets/icons/phosphor/chart-line.svg"),
            Icon::Cpu => include_bytes!("../assets/icons/phosphor/cpu.svg"),
            Icon::Plug => include_bytes!("../assets/icons/phosphor/plug.svg"),
            Icon::Calendar => include_bytes!("../assets/icons/phosphor/calendar-blank.svg"),
            Icon::Refresh => include_bytes!("../assets/icons/phosphor/arrows-clockwise.svg"),
            Icon::Download => include_bytes!("../assets/icons/phosphor/download-simple.svg"),
            Icon::MessageCircle => include_bytes!("../assets/icons/phosphor/chat-circle.svg"),
            Icon::Bot => include_bytes!("../assets/icons/phosphor/robot.svg"),
            Icon::ListChecks => include_bytes!("../assets/icons/phosphor/list-checks.svg"),
            Icon::Building => include_bytes!("../assets/icons/phosphor/buildings.svg"),
            Icon::Smile => include_bytes!("../assets/icons/phosphor/smiley.svg"),
            Icon::Shield => include_bytes!("../assets/icons/phosphor/shield.svg"),
            Icon::Terminal => include_bytes!("../assets/icons/phosphor/terminal-window.svg"),
            Icon::Folder => include_bytes!("../assets/icons/phosphor/folder.svg"),
            Icon::Scissors => include_bytes!("../assets/icons/phosphor/scissors.svg"),
            Icon::Mic => include_bytes!("../assets/icons/phosphor/microphone.svg"),
            Icon::BellOff => include_bytes!("../assets/icons/phosphor/bell-slash.svg"),
            Icon::Key => include_bytes!("../assets/icons/phosphor/key.svg"),
            Icon::Info => include_bytes!("../assets/icons/phosphor/info.svg"),
            Icon::MessageSquare => include_bytes!("../assets/icons/phosphor/chat.svg"),
            Icon::Sparkles => include_bytes!("../assets/icons/phosphor/sparkle.svg"),
            Icon::Contacts => include_bytes!("../assets/icons/phosphor/users.svg"),
            Icon::MessageCircleFilled => {
                include_bytes!("../assets/icons/phosphor/chat-circle-fill.svg")
            }
            Icon::Brain => include_bytes!("../assets/icons/phosphor/brain.svg"),
            Icon::Pushpin => include_bytes!("../assets/icons/phosphor/push-pin.svg"),
            Icon::Sun => include_bytes!("../assets/icons/phosphor/sun.svg"),
            Icon::Moon => include_bytes!("../assets/icons/phosphor/moon.svg"),
            Icon::Monitor => include_bytes!("../assets/icons/phosphor/desktop.svg"),
            Icon::ChevronDown => include_bytes!("../assets/icons/phosphor/caret-down.svg"),
            Icon::SidebarExpand => {
                include_bytes!("../assets/icons/phosphor/sidebar-simple.svg")
            }
            Icon::PaneCollapse => {
                include_bytes!("../assets/icons/phosphor/caret-double-right.svg")
            }
            Icon::PaneExpand => {
                include_bytes!("../assets/icons/phosphor/caret-double-left.svg")
            }
        }
    }

    fn light_svg(self) -> &'static [u8] {
        match self {
            Icon::Search => include_bytes!("../assets/icons/phosphor/magnifying-glass-light.svg"),
            Icon::Plus => include_bytes!("../assets/icons/phosphor/plus-light.svg"),
            Icon::Ellipsis => include_bytes!("../assets/icons/phosphor/dots-three-light.svg"),
            Icon::Settings => include_bytes!("../assets/icons/phosphor/gear-light.svg"),
            Icon::X => include_bytes!("../assets/icons/phosphor/x-light.svg"),
            Icon::Check => include_bytes!("../assets/icons/phosphor/check-light.svg"),
            Icon::ArrowUp => include_bytes!("../assets/icons/phosphor/arrow-up-light.svg"),
            Icon::Stop => include_bytes!("../assets/icons/phosphor/square-light.svg"),
            Icon::ChevronLeft => include_bytes!("../assets/icons/phosphor/caret-left-light.svg"),
            Icon::ChevronRight => include_bytes!("../assets/icons/phosphor/caret-right-light.svg"),
            Icon::Paperclip => include_bytes!("../assets/icons/phosphor/paperclip-light.svg"),
            Icon::Minus => include_bytes!("../assets/icons/phosphor/minus-light.svg"),
            Icon::WindowMaximize => include_bytes!("../assets/icons/phosphor/square-light.svg"),
            Icon::WindowRestore => include_bytes!("../assets/icons/phosphor/copy-light.svg"),
            Icon::PanelRight => {
                include_bytes!("../assets/icons/phosphor/sidebar-simple-light.svg")
            }
            Icon::Hash => include_bytes!("../assets/icons/phosphor/hash-light.svg"),
            Icon::Pin => include_bytes!("../assets/icons/phosphor/push-pin-light.svg"),
            Icon::Star => include_bytes!("../assets/icons/phosphor/star-light.svg"),
            Icon::Crown => include_bytes!("../assets/icons/phosphor/crown-light.svg"),
            Icon::Pencil => include_bytes!("../assets/icons/phosphor/pencil-simple-light.svg"),
            Icon::Trash => include_bytes!("../assets/icons/phosphor/trash-light.svg"),
            Icon::EyeOff => include_bytes!("../assets/icons/phosphor/eye-slash-light.svg"),
            Icon::Archive => include_bytes!("../assets/icons/phosphor/archive-light.svg"),
            Icon::Chart => include_bytes!("../assets/icons/phosphor/chart-line-light.svg"),
            Icon::Cpu => include_bytes!("../assets/icons/phosphor/cpu-light.svg"),
            Icon::Plug => include_bytes!("../assets/icons/phosphor/plug-light.svg"),
            Icon::Calendar => include_bytes!("../assets/icons/phosphor/calendar-blank-light.svg"),
            Icon::Refresh => {
                include_bytes!("../assets/icons/phosphor/arrows-clockwise-light.svg")
            }
            Icon::Download => include_bytes!("../assets/icons/phosphor/download-simple-light.svg"),
            Icon::MessageCircle => {
                include_bytes!("../assets/icons/phosphor/chat-circle-light.svg")
            }
            Icon::Bot => include_bytes!("../assets/icons/phosphor/robot-light.svg"),
            Icon::ListChecks => include_bytes!("../assets/icons/phosphor/list-checks-light.svg"),
            Icon::Building => include_bytes!("../assets/icons/phosphor/buildings-light.svg"),
            Icon::Smile => include_bytes!("../assets/icons/phosphor/smiley-light.svg"),
            Icon::Shield => include_bytes!("../assets/icons/phosphor/shield-light.svg"),
            Icon::Terminal => {
                include_bytes!("../assets/icons/phosphor/terminal-window-light.svg")
            }
            Icon::Folder => include_bytes!("../assets/icons/phosphor/folder-light.svg"),
            Icon::Scissors => include_bytes!("../assets/icons/phosphor/scissors-light.svg"),
            Icon::Mic => include_bytes!("../assets/icons/phosphor/microphone-light.svg"),
            Icon::BellOff => include_bytes!("../assets/icons/phosphor/bell-slash-light.svg"),
            Icon::Key => include_bytes!("../assets/icons/phosphor/key-light.svg"),
            Icon::Info => include_bytes!("../assets/icons/phosphor/info-light.svg"),
            Icon::MessageSquare => include_bytes!("../assets/icons/phosphor/chat-light.svg"),
            Icon::Sparkles => include_bytes!("../assets/icons/phosphor/sparkle-light.svg"),
            Icon::Contacts => include_bytes!("../assets/icons/phosphor/users-light.svg"),
            // 只有 fill 变体，light 回落 regular
            Icon::MessageCircleFilled => {
                include_bytes!("../assets/icons/phosphor/chat-circle-fill.svg")
            }
            Icon::Brain => include_bytes!("../assets/icons/phosphor/brain-light.svg"),
            Icon::Pushpin => include_bytes!("../assets/icons/phosphor/push-pin-light.svg"),
            Icon::Sun => include_bytes!("../assets/icons/phosphor/sun-light.svg"),
            Icon::Moon => include_bytes!("../assets/icons/phosphor/moon-light.svg"),
            Icon::Monitor => include_bytes!("../assets/icons/phosphor/desktop-light.svg"),
            Icon::ChevronDown => include_bytes!("../assets/icons/phosphor/caret-down-light.svg"),
            Icon::SidebarExpand => {
                include_bytes!("../assets/icons/phosphor/sidebar-simple-light.svg")
            }
            Icon::PaneCollapse => {
                include_bytes!("../assets/icons/phosphor/caret-double-right-light.svg")
            }
            Icon::PaneExpand => {
                include_bytes!("../assets/icons/phosphor/caret-double-left-light.svg")
            }
        }
    }
}

/// 画一个图标（regular 字重）。`size` 是边长，颜色由调用方给（`currentColor`）。
pub fn icon(kind: Icon, size: f32, color: Rgba) -> impl IntoElement {
    svg()
        .data(kind.svg())
        .size(px(size))
        .text_color(Hsla::from(color))
}

/// 细线版（Phosphor light 字重）。
///
/// 旧接口 `icon_with_stroke(.., stroke_width)` 是替换 SVG 里的 `stroke-width` 字符串；
/// Phosphor 是纯填充路径、没有这个属性，那种做法会静默失效。要更细就用这个函数。
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
                    text.contains("viewBox=\"0 0 256 256\""),
                    "{icon:?}/{weight:?} 缺 256 画布"
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
        assert_eq!(Icon::ALL.len(), 55);
    }

    #[test]
    fn light_weight_differs_from_regular_where_available() {
        // 除了只有 fill 变体的那个，light 与 regular 必须是不同资产
        for icon in Icon::ALL {
            if icon == Icon::MessageCircleFilled {
                continue;
            }
            assert_ne!(
                icon.svg(),
                icon.svg_at(Weight::Light),
                "{icon:?} 的 light 变体与 regular 相同"
            );
        }
    }
}
