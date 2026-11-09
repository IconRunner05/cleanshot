use std::time::Duration;

use gpui::{
    App, AppContext, Context, Global, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::theme::{CORNER, Theme};

/// `element` showing `text` once the pointer has rested on it for `theme`'s tooltip delay, or
/// GPUI's own where it has none, and hiding it on leave or press.
pub(crate) fn with_tooltip<E: StatefulInteractiveElement>(
    element: E,
    text: impl AsRef<str>,
    theme: Theme,
) -> E {
    let text = SharedString::from(text.as_ref().to_owned());
    let element = element.tooltip(move |_, cx| {
        let text = text.clone();
        cx.new(|_| Tooltip { text, theme }).into()
    });
    match theme.tooltip_delay {
        Some(delay) => element.tooltip_show_delay(delay),
        None => element,
    }
}

/// Makes every tooltip wait `hover`, the Windows mouse hover time, before it shows.
/// Call once, before any window opens; until then a tooltip waits GPUI's own delay.
pub fn follow_hover_time(cx: &mut App, hover: Duration) {
    cx.set_global(HoverTime(hover));
}

/// How long a tooltip waits before it shows, once the app has said.
pub(crate) fn hover_time(cx: &App) -> Option<Duration> {
    cx.try_global::<HoverTime>().map(|HoverTime(hover)| *hover)
}

struct HoverTime(Duration);

impl Global for HoverTime {}

struct Tooltip {
    text: SharedString,
    theme: Theme,
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(CORNER)
            .bg(self.theme.material)
            .border_1()
            .border_color(self.theme.hairline)
            .text_color(self.theme.text)
            .text_size(px(12.0))
            .child(self.text.clone())
    }
}
