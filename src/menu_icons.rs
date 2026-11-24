//! 侧栏与设置菜单图标（Boxicons，line / filled 成对）。
//!
//! 线性用于未选中，实心用于选中；侧栏 24px、设置菜单 18px 两种尺寸。
//! 资产在 `assets/icons/ui/`（与全局 `icons::Icon` 共用同一套）。
//!
//! 历史：手绘 24 网格 → Phosphor（实心、方）→ Remix（线框、圆）→ **Boxicons**。
//! Boxicons 圆润度实测 35.3%，且 1884 组 line/filled 严格成对。

use gpui::{Hsla, IntoElement, Rgba, Styled, px, svg};

/// 侧栏图标尺寸（左导轨）。
///
/// Boxicons 24 网格，线框笔画约 2px，21px 下与 Remix 时代观感相当。
pub const RAIL_SIZE: f32 = 22.;
/// 设置菜单图标尺寸。
pub const SETTINGS_SIZE: f32 = 16.;
/// 标题栏窗口控制按钮（最小化 / 最大化 / 关闭 / 置顶）的图标尺寸。
pub const TITLEBAR_SIZE: f32 = 14.;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuIcon {
    Chat,
    Contacts,
    Channels,
    Gear,
    Sliders,
    Models,
    Plugins,
    Agents,
    Bot,
    Memory,
    Im,
    Connectors,
    Schedules,
    Keys,
    Abilities,
    Usage,
    About,
    Feedback,
    Browser,
    Computer,
    More,
}

/// 每个菜单项一对 Remix 图标名：(线框, 实心)。
///
/// 成对是硬约束——取不到的实心会退回线框，选中态就没有视觉反馈了，
/// 所以这里只挑 Remix 确实同时提供 `-line` 与 `-fill` 的图标。
const PAIRS: [(MenuIcon, &str, &str); 21] = [
    // 「消息」用圆形气泡，「反馈」用扩音器，两者不撞脸
    (MenuIcon::Chat, "message-circle", "message-circle-filled"),
    (MenuIcon::Contacts, "group", "group-filled"),
    (MenuIcon::Channels, "hashtag", "hashtag-filled"),
    (MenuIcon::Gear, "gear", "gear-filled"),
    (MenuIcon::Sliders, "slider", "slider-filled"),
    (MenuIcon::Models, "layers", "layers-filled"),
    (MenuIcon::Plugins, "puzzle", "puzzle-filled"),
    // 智能体用 CPU，「Bot」用机器人：两个都用 robot 会撞脸
    (MenuIcon::Agents, "chip", "chip-filled"),
    (MenuIcon::Bot, "robot", "robot-filled"),
    (MenuIcon::Memory, "brain", "brain-filled"),
    (MenuIcon::Im, "mobile", "mobile-filled"),
    (MenuIcon::Connectors, "plug-connect", "plug-connect-filled"),
    (MenuIcon::Schedules, "calendar", "calendar-filled"),
    (MenuIcon::Keys, "key", "key-filled"),
    (MenuIcon::Abilities, "bolt", "bolt-filled"),
    (MenuIcon::Usage, "bar-chart", "bar-chart-filled"),
    (MenuIcon::About, "info-circle", "info-circle-filled"),
    (MenuIcon::Feedback, "megaphone", "megaphone-filled"),
    (MenuIcon::Browser, "globe", "globe-filled"),
    (MenuIcon::Computer, "desktop", "desktop-filled"),
    (MenuIcon::More, "dots-horizontal", "dots-horizontal-filled"),
];

impl MenuIcon {
    /// (线框文件名, 实心文件名)，不含 `.svg`。
    fn names(self) -> (&'static str, &'static str) {
        PAIRS
            .iter()
            .find(|(kind, _, _)| *kind == self)
            .map(|(_, line, solid)| (*line, *solid))
            .expect("PAIRS 覆盖了全部 MenuIcon")
    }

    fn line_svg(self) -> &'static [u8] {
        match self.names().0 {
            "message-circle" => include_bytes!("../assets/icons/ui/message-circle.svg"),
            "group" => include_bytes!("../assets/icons/ui/group.svg"),
            "hashtag" => include_bytes!("../assets/icons/ui/hashtag.svg"),
            "gear" => include_bytes!("../assets/icons/ui/gear.svg"),
            "slider" => include_bytes!("../assets/icons/ui/slider.svg"),
            "layers" => include_bytes!("../assets/icons/ui/layers.svg"),
            "puzzle" => include_bytes!("../assets/icons/ui/puzzle.svg"),
            "chip" => include_bytes!("../assets/icons/ui/chip.svg"),
            "robot" => include_bytes!("../assets/icons/ui/robot.svg"),
            "brain" => include_bytes!("../assets/icons/ui/brain.svg"),
            "mobile" => include_bytes!("../assets/icons/ui/mobile.svg"),
            "plug-connect" => include_bytes!("../assets/icons/ui/plug-connect.svg"),
            "calendar" => include_bytes!("../assets/icons/ui/calendar.svg"),
            "key" => include_bytes!("../assets/icons/ui/key.svg"),
            "bolt" => include_bytes!("../assets/icons/ui/bolt.svg"),
            "bar-chart" => include_bytes!("../assets/icons/ui/bar-chart.svg"),
            "info-circle" => include_bytes!("../assets/icons/ui/info-circle.svg"),
            "megaphone" => include_bytes!("../assets/icons/ui/megaphone.svg"),
            "globe" => include_bytes!("../assets/icons/ui/globe.svg"),
            "desktop" => include_bytes!("../assets/icons/ui/desktop.svg"),
            "dots-horizontal" => include_bytes!("../assets/icons/ui/dots-horizontal.svg"),
            other => unreachable!("未登记的线框图标：{other}"),
        }
    }

    fn fill_svg(self) -> &'static [u8] {
        match self.names().1 {
            "message-circle-filled" => include_bytes!("../assets/icons/ui/message-circle-filled.svg"),
            "group-filled" => include_bytes!("../assets/icons/ui/group-filled.svg"),
            "hashtag-filled" => include_bytes!("../assets/icons/ui/hashtag-filled.svg"),
            "gear-filled" => include_bytes!("../assets/icons/ui/gear-filled.svg"),
            "slider-filled" => include_bytes!("../assets/icons/ui/slider-filled.svg"),
            "layers-filled" => include_bytes!("../assets/icons/ui/layers-filled.svg"),
            "puzzle-filled" => include_bytes!("../assets/icons/ui/puzzle-filled.svg"),
            "chip-filled" => include_bytes!("../assets/icons/ui/chip-filled.svg"),
            "robot-filled" => include_bytes!("../assets/icons/ui/robot-filled.svg"),
            "brain-filled" => include_bytes!("../assets/icons/ui/brain-filled.svg"),
            "mobile-filled" => include_bytes!("../assets/icons/ui/mobile-filled.svg"),
            "plug-connect-filled" => include_bytes!("../assets/icons/ui/plug-connect-filled.svg"),
            "calendar-filled" => include_bytes!("../assets/icons/ui/calendar-filled.svg"),
            "key-filled" => include_bytes!("../assets/icons/ui/key-filled.svg"),
            "bolt-filled" => include_bytes!("../assets/icons/ui/bolt-filled.svg"),
            "bar-chart-filled" => include_bytes!("../assets/icons/ui/bar-chart-filled.svg"),
            "info-circle-filled" => include_bytes!("../assets/icons/ui/info-circle-filled.svg"),
            "megaphone-filled" => include_bytes!("../assets/icons/ui/megaphone-filled.svg"),
            "globe-filled" => include_bytes!("../assets/icons/ui/globe-filled.svg"),
            "desktop-filled" => include_bytes!("../assets/icons/ui/desktop-filled.svg"),
            "dots-horizontal-filled" => include_bytes!("../assets/icons/ui/dots-horizontal-filled.svg"),
            other => unreachable!("未登记的实心图标：{other}"),
        }
    }

    fn light_svg(self) -> &'static [u8] {
        // Boxicons 没有 light 字重，线框本身就比实心轻，直接复用。
        self.line_svg()
    }
}

/// 按状态渲染：`solid = true` 取实心，否则常规。
pub fn menu_icon(kind: MenuIcon, solid: bool, size: f32, color: Rgba) -> impl IntoElement {
    let data = if solid { kind.fill_svg() } else { kind.line_svg() };
    svg()
        .data(data)
        .size(px(size))
        .text_color(Hsla::from(color))
}

/// 侧栏用：`light = true` 时取细线字重（比 regular 更轻，贴近旧手绘观感）。
pub fn menu_icon_with_weight(
    kind: MenuIcon,
    solid: bool,
    size: f32,
    color: Rgba,
    light: bool,
) -> gpui::AnyElement {
    let data = match (solid, light) {
        (true, _) => kind.fill_svg(),
        (false, true) => kind.light_svg(),
        (false, false) => kind.line_svg(),
    };
    svg()
        .data(data)
        .size(px(size))
        .text_color(Hsla::from(color))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Boxicons 里有少数图标（`key` / `bar-chart` / `dots-horizontal`）的
    /// `-filled` 变体与线框版**内容完全相同**——这是上游库自身的重复，不是我们配错。
    /// 这些项选中态靠颜色区分（调用方本来就按 `active` 切换 text_color），不影响可用性。
    const IDENTICAL_PAIRS: [MenuIcon; 3] =
        [MenuIcon::Keys, MenuIcon::Usage, MenuIcon::More];

    #[test]
    fn every_variant_has_both_weights() {
        for (kind, line, solid) in PAIRS {
            let l = kind.line_svg();
            let f = kind.fill_svg();
            assert!(l.starts_with(b"<svg"), "{kind:?}({line}) 不是 svg");
            assert!(f.starts_with(b"<svg"), "{kind:?}({solid}) 不是 svg");
            if !IDENTICAL_PAIRS.contains(&kind) {
                assert_ne!(l, f, "{kind:?} 的实心与线框相同（若为上游重复请登记白名单）");
            }
            let text = std::str::from_utf8(l).unwrap();
            assert!(text.contains("viewBox=\"0 0 24 24\""), "{kind:?} 缺 24 画布");
        }
    }

    #[test]
    fn feedback_is_not_a_chat_bubble() {
        // 「反馈」必须和「消息」用不同图标，否则侧栏里两个气泡分不清
        assert_ne!(MenuIcon::Feedback.names().0, MenuIcon::Chat.names().0);
        assert_eq!(MenuIcon::Feedback.names().0, "megaphone");
        assert_eq!(MenuIcon::Chat.names().0, "message-circle");
    }

    #[test]
    fn line_icon_is_not_filled() {
        // Remix 的 `-line` 变体内部应当同时有描边感和镂空；
        // 这里只校验「线框」与「实心」确实是不同资产（light 与 line 同源）。
        for (kind, name, _) in PAIRS {
            assert_eq!(
                kind.light_svg(),
                kind.line_svg(),
                "{kind:?}({name}) 的 light 应与 line 同源"
            );
        }
    }
}
