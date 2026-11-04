use std::time::Duration;

use gpui::{
    App, AppContext, Context, Global, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::theme::Theme;

/// What a control shows while the pointer rests on it (D-117): its accessible name, then its
/// shortcut in brackets when it has one, as in "Highlighter (H)".
pub(crate) fn tooltip_text(name: &str, keys: Option<&str>) -> SharedString {
    match keys {
        Some(keys) => format!("{name} ({keys})").into(),
        None => SharedString::from(name.to_owned()),
    }
}

/// `element` showing `text` once the pointer has rested on it for `theme`'s tooltip delay, or
/// GPUI's own where it has none, and hiding it on leave or press.
pub(crate) fn with_tooltip<E: StatefulInteractiveElement>(
    element: E,
    text: SharedString,
    theme: Theme,
) -> E {
    let element = element.tooltip(move |_, cx| {
        let text = text.clone();
        cx.new(|_| Tooltip { text, theme }).into()
    });
    match theme.tooltip_delay {
        Some(delay) => element.tooltip_show_delay(delay),
        None => element,
    }
}

/// Makes every tooltip wait `hover`, the Windows mouse hover time, before it shows (D-117).
/// Call once, before any window opens; until then a tooltip waits GPUI's own delay.
pub fn follow_hover_time(cx: &mut App, hover: Duration) {
    cx.set_global(HoverTime(hover));
}

/// How long a tooltip waits before it shows, once the app has said (D-117).
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
            .rounded(px(7.0))
            .bg(self.theme.material)
            .border_1()
            .border_color(self.theme.hairline)
            .text_color(self.theme.text)
            .text_size(px(12.0))
            .child(self.text.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d117_a_tooltip_is_the_name_then_the_shortcut_if_there_is_one() {
        assert_eq!(tooltip_text("Highlighter", Some("H")), "Highlighter (H)");
        assert_eq!(
            tooltip_text("Undo arrow", Some("Ctrl+Z")),
            "Undo arrow (Ctrl+Z)"
        );
        assert_eq!(tooltip_text("Red", None), "Red");
    }
}
