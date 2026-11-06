use gpui::{
    InteractiveElement, IntoElement, ParentElement, Rgba, Role, Stateful,
    StatefulInteractiveElement, Styled, Window, WindowControlArea, div, px, rgba,
};

use crate::placement::CAPTION_ROW;
use crate::theme::Theme;

/// CHARTER 4: Windows 11's close-button red, under the pointer.
const CLOSE_HOVER: Rgba = Rgba {
    r: 0.769,
    g: 0.169,
    b: 0.11,
    a: 1.0,
};

/// The window's own caption row, in place of a title bar (D-141): `controls` from the left,
/// clipped if the window is too narrow for them, then empty space that moves the window, then
/// `actions`, then the caption buttons.
pub(crate) fn caption_row(
    controls: impl IntoElement,
    actions: impl IntoElement,
    window: &Window,
    theme: Theme,
) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .h(CAPTION_ROW)
        .items_center()
        .child(
            div()
                .flex()
                .items_center()
                .min_w_0()
                .overflow_hidden()
                .pl(px(12.0))
                .child(controls),
        )
        .child(drag_space())
        .child(actions)
        .child(caption_buttons(window, theme))
}

/// The row's empty space, which moves the window as a title bar does: a drag moves it, a
/// double-click maximises it, and Windows snaps it (D-141).
fn drag_space() -> impl IntoElement {
    div()
        .id("caption-drag")
        .flex_1()
        .min_w(px(40.0))
        .h_full()
        .window_control_area(WindowControlArea::Drag)
}

/// Minimise, maximise or restore, and close, drawn in Windows' own caption glyphs. Windows
/// works them, as it works a title bar's, with the snap layouts on maximise (D-141).
fn caption_buttons(window: &Window, theme: Theme) -> impl IntoElement {
    let (maximise, maximise_glyph) = if window.is_maximized() {
        ("Restore", "\u{e923}")
    } else {
        ("Maximize", "\u{e922}")
    };
    div()
        .flex()
        .h(CAPTION_ROW)
        .flex_none()
        .font_family("Segoe Fluent Icons")
        .text_size(px(10.0))
        .text_color(theme.text)
        .child(
            caption_button("Minimize", "\u{e921}", WindowControlArea::Min)
                .hover(move |button| button.bg(theme.hover)),
        )
        .child(
            caption_button(maximise, maximise_glyph, WindowControlArea::Max)
                .hover(move |button| button.bg(theme.hover)),
        )
        .child(
            caption_button("Close", "\u{e8bb}", WindowControlArea::Close)
                .hover(|button| button.bg(CLOSE_HOVER).text_color(rgba(0xffff_ffff))),
        )
}

fn caption_button(
    name: &'static str,
    glyph: &'static str,
    area: WindowControlArea,
) -> Stateful<gpui::Div> {
    div()
        .id(name)
        .role(Role::Button)
        .aria_label(name)
        .occlude()
        .w(px(46.0))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .window_control_area(area)
        .child(glyph)
}
