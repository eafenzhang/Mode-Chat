//! 本地智能体引擎的官方图标（Pi / Hermes / ZCode）。
//!
//! Pi 是官方 logo 的三块彩色路径（GPUI 的 svg 只能单色，所以拆三层渲染）；
//! Hermes / ZCode 是官方位图，用 `img` 按 26px 显示（避免整块染色）。

use std::sync::{Arc, OnceLock};

use gpui::{
    div, img, px, svg, AnyElement, Image, ImageFormat, IntoElement, ParentElement, Styled,
};

static HERMES: OnceLock<Arc<Image>> = OnceLock::new();
static ZCODE: OnceLock<Arc<Image>> = OnceLock::new();

fn image(cell: &'static OnceLock<Arc<Image>>, bytes: &'static [u8], size: f32) -> AnyElement {
    let image = cell
        .get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Png, bytes.to_vec())))
        .clone();
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded(px(size * 0.23))
        .overflow_hidden()
        .child(img(image).size(px(size)))
        .into_any_element()
}

/// Pi 官方 logo：三块颜色分层，保持矢量清晰。
pub fn pi(size: f32) -> AnyElement {
    let side = px(size);
    div()
        .relative()
        .size(side)
        .child(
            svg()
                .data(include_bytes!("../assets/icons/agents/pi-salmon.svg"))
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .size(side)
                .text_color(gpui::rgb(0xf09082)),
        )
        .child(
            svg()
                .data(include_bytes!("../assets/icons/agents/pi-blue.svg"))
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .size(side)
                .text_color(gpui::rgb(0x4d9abf)),
        )
        .child(
            svg()
                .data(include_bytes!("../assets/icons/agents/pi-yellow.svg"))
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .size(side)
                .text_color(gpui::rgb(0xf1be58)),
        )
        .into_any_element()
}

pub fn hermes(size: f32) -> AnyElement {
    image(
        &HERMES,
        include_bytes!("../assets/icons/agents/hermes.png"),
        size,
    )
}

pub fn zcode(size: f32) -> AnyElement {
    image(
        &ZCODE,
        include_bytes!("../assets/icons/agents/zcode.png"),
        size,
    )
}
