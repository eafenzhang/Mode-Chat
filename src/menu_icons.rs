//! 侧栏与设置菜单图标（自绘 24 网格，线性/面性成对）。
//!
//! 线性用于未选中，面性用于选中；侧栏 24px、设置菜单 16px 两种尺寸。
//! 资产在 `assets/icons/menu/`，与全局 lucide 图标互不影响。

use gpui::{Hsla, IntoElement, Rgba, Styled, px, svg};

/// 侧栏图标尺寸（左导轨）。
pub const RAIL_SIZE: f32 = 24.;
/// 设置菜单图标尺寸。
pub const SETTINGS_SIZE: f32 = 22.;

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

impl MenuIcon {
    fn svg(self, solid: bool) -> &'static [u8] {
        match (self, solid) {
            (MenuIcon::Chat, false) => include_bytes!("../assets/icons/menu/chat.svg"),
            (MenuIcon::Chat, true) => include_bytes!("../assets/icons/menu/chat-solid.svg"),
            (MenuIcon::Contacts, false) => include_bytes!("../assets/icons/menu/contacts.svg"),
            (MenuIcon::Contacts, true) => include_bytes!("../assets/icons/menu/contacts-solid.svg"),
            (MenuIcon::Channels, false) => include_bytes!("../assets/icons/menu/channels.svg"),
            (MenuIcon::Channels, true) => include_bytes!("../assets/icons/menu/channels-solid.svg"),
            (MenuIcon::Gear, false) => include_bytes!("../assets/icons/menu/gear.svg"),
            (MenuIcon::Gear, true) => include_bytes!("../assets/icons/menu/gear-solid.svg"),
            (MenuIcon::Sliders, false) => include_bytes!("../assets/icons/menu/sliders.svg"),
            (MenuIcon::Sliders, true) => include_bytes!("../assets/icons/menu/sliders-solid.svg"),
            (MenuIcon::Models, false) => include_bytes!("../assets/icons/menu/models.svg"),
            (MenuIcon::Models, true) => include_bytes!("../assets/icons/menu/models-solid.svg"),
            (MenuIcon::Plugins, false) => include_bytes!("../assets/icons/menu/plugins.svg"),
            (MenuIcon::Plugins, true) => include_bytes!("../assets/icons/menu/plugins-solid.svg"),
            (MenuIcon::Agents, false) => include_bytes!("../assets/icons/menu/agents.svg"),
            (MenuIcon::Agents, true) => include_bytes!("../assets/icons/menu/agents-solid.svg"),
            (MenuIcon::Bot, false) => include_bytes!("../assets/icons/menu/bot.svg"),
            (MenuIcon::Bot, true) => include_bytes!("../assets/icons/menu/bot-solid.svg"),
            (MenuIcon::Memory, false) => include_bytes!("../assets/icons/menu/memory.svg"),
            (MenuIcon::Memory, true) => include_bytes!("../assets/icons/menu/memory-solid.svg"),
            (MenuIcon::Connectors, false) => include_bytes!("../assets/icons/menu/connectors.svg"),
            (MenuIcon::Connectors, true) => include_bytes!("../assets/icons/menu/connectors-solid.svg"),
            (MenuIcon::Schedules, false) => include_bytes!("../assets/icons/menu/schedules.svg"),
            (MenuIcon::Schedules, true) => include_bytes!("../assets/icons/menu/schedules-solid.svg"),
            (MenuIcon::Keys, false) => include_bytes!("../assets/icons/menu/keys.svg"),
            (MenuIcon::Keys, true) => include_bytes!("../assets/icons/menu/keys-solid.svg"),
            (MenuIcon::Abilities, false) => include_bytes!("../assets/icons/menu/abilities.svg"),
            (MenuIcon::Abilities, true) => include_bytes!("../assets/icons/menu/abilities-solid.svg"),
            (MenuIcon::Usage, false) => include_bytes!("../assets/icons/menu/usage.svg"),
            (MenuIcon::Usage, true) => include_bytes!("../assets/icons/menu/usage-solid.svg"),
            (MenuIcon::About, false) => include_bytes!("../assets/icons/menu/about.svg"),
            (MenuIcon::About, true) => include_bytes!("../assets/icons/menu/about-solid.svg"),
            (MenuIcon::Feedback, false) => include_bytes!("../assets/icons/menu/feedback.svg"),
            (MenuIcon::Feedback, true) => include_bytes!("../assets/icons/menu/feedback-solid.svg"),
            (MenuIcon::Browser, false) => include_bytes!("../assets/icons/menu/browser.svg"),
            (MenuIcon::Browser, true) => include_bytes!("../assets/icons/menu/browser-solid.svg"),
            (MenuIcon::Computer, false) => include_bytes!("../assets/icons/menu/computer.svg"),
            (MenuIcon::Computer, true) => include_bytes!("../assets/icons/menu/computer-solid.svg"),
            (MenuIcon::More, false) => include_bytes!("../assets/icons/menu/more.svg"),
            (MenuIcon::More, true) => include_bytes!("../assets/icons/menu/more-solid.svg"),
        }
    }
}

/// 按状态渲染：`solid = true` 取面性图标，否则线性。
pub fn menu_icon(kind: MenuIcon, solid: bool, size: f32, color: Rgba) -> impl IntoElement {
    svg()
        .data(kind.svg(solid))
        .size(px(size))
        .text_color(Hsla::from(color))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_has_both_weights() {
        let all = [
            MenuIcon::Chat,
            MenuIcon::Contacts,
            MenuIcon::Channels,
            MenuIcon::Gear,
            MenuIcon::Sliders,
            MenuIcon::Models,
            MenuIcon::Plugins,
            MenuIcon::Agents,
            MenuIcon::Bot,
            MenuIcon::Memory,
            MenuIcon::Connectors,
            MenuIcon::Schedules,
            MenuIcon::Keys,
            MenuIcon::Abilities,
            MenuIcon::Usage,
            MenuIcon::About,
            MenuIcon::Feedback,
            MenuIcon::Browser,
            MenuIcon::Computer,
            MenuIcon::More,
        ];
        for icon in all {
            assert!(!icon.svg(false).is_empty());
            assert!(!icon.svg(true).is_empty());
        }
    }
}
