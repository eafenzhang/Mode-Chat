//! 单行输入框：gpui-kit `Input` 的薄适配层。
//!
//! 保留原 `TextInput` 的对外 API（`content/set_content/clear/focus_handle`），
//! 内部改用 `gpui_kit::component::input::{Input, InputState}`；composer 仍用
//! 自绘多行 `TextInput`（@ 提及/草稿/软折行依赖它的内部状态）。

use gpui::component::input::{Input, InputEvent, InputState};
use gpui::prelude::*;
use gpui::{
    App, Entity, FocusHandle, Focusable, IntoElement, MouseButton, Render, SharedString,
    Subscription, Window, div, px,
};

use crate::{theme, wechat_theme};

pub struct SimpleInput {
    state: Option<Entity<InputState>>,
    /// 内容镜像：`content()` 没有 `App` 参数，靠 Change 订阅同步。
    content: SharedString,
    /// 首帧前/无 window 时暂存待写入值，render 时补写。
    pending: Option<SharedString>,
    placeholder: SharedString,
    /// 状态创建前的临时焦点句柄；创建后转交给 InputState。
    local_focus: FocusHandle,
    _subscription: Option<Subscription>,
}

impl SimpleInput {
    pub fn new(
        placeholder: impl Into<SharedString>,
        _placeholder_color: fn() -> gpui::Rgba,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        Self {
            state: None,
            content: "".into(),
            pending: None,
            placeholder: placeholder.into(),
            local_focus: cx.focus_handle(),
            _subscription: None,
        }
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn set_content(&mut self, text: impl Into<SharedString>, cx: &mut gpui::Context<Self>) {
        let text = text.into();
        self.content = text.clone();
        self.pending = Some(text);
        cx.notify();
    }

    pub fn clear(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.content = "".into();
        self.pending = None;
        if let Some(state) = self.state.clone() {
            state.update(cx, |state, cx| state.set_value("", window, cx));
        }
        cx.notify();
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state
            .as_ref()
            .map(|state| state.read(cx).focus_handle(cx))
            .unwrap_or_else(|| self.local_focus.clone())
    }
}

impl Focusable for SimpleInput {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        SimpleInput::focus_handle(self, cx)
    }
}

impl Render for SimpleInput {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        if self.state.is_none() {
            let had_focus = self.local_focus.is_focused(window);
            let placeholder = self.placeholder.clone();
            let state = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            let subscription = cx.subscribe_in(
                &state,
                window,
                |this, state, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.content = state.read(cx).value();
                        cx.notify();
                    }
                    // 单行输入框的 Enter 仍按旧行为提交当前对话框；动作会冒泡到
                    // `Dialog` 上下文里的 on_submit_dialog。
                    InputEvent::PressEnter { secondary: false, .. } => {
                        window.dispatch_action(Box::new(crate::SubmitDialog), cx);
                    }
                    _ => {}
                },
            );
            if had_focus {
                state.read(cx).focus_handle(cx).focus(window, cx);
            }
            self.state = Some(state);
            self._subscription = Some(subscription);
        }
        let state = self.state.clone().expect("input state just initialized");
        // 对话框在首帧前就对 local_focus 调了 focus（例如自动聚焦名称框）：
        // 状态创建后的下一帧把焦点转交给 kit 输入。
        if self.local_focus.is_focused(window) {
            state.read(cx).focus_handle(cx).focus(window, cx);
        }
        if let Some(text) = self.pending.take() {
            state.update(cx, |state, cx| state.set_value(text, window, cx));
        }
        let wechat = wechat_theme::active();
        div()
            .size_full()
            // gpui-kit Input 在 appearance(false) 下不一定接管点击焦点，这里兜底。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, window, cx| {
                    let handle = this
                        .state
                        .as_ref()
                        .map(|state| state.read(cx).focus_handle(cx))
                        .unwrap_or_else(|| this.local_focus.clone());
                    handle.focus(window, cx);
                }),
            )
            .child(
            Input::new(&state)
                .appearance(false)
                .bordered(false)
                .focus_bordered(false)
                .px(px(0.))
                .py(px(0.))
                .h_full()
                .font_family(if wechat {
                    wechat_theme::FONT_FAMILY
                } else {
                    theme::FONT_FAMILY
                })
                .text_size(px(13.))
                .line_height(px(18.)),
        )
    }
}
