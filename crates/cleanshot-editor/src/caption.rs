use std::collections::HashMap;

use cleanshot_core::groups_that_fit;
use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, AnyElement, App, Div, Entity, InteractiveElement, IntoElement, ParentElement, Pixels,
    RenderOnce, Rgba, Role, Stateful, StatefulInteractiveElement, Styled, Window,
    WindowControlArea, anchored, deferred, div, px, rgba,
};

use crate::placement::CAPTION_ROW;
use crate::strip::StripGroup;
use crate::theme::{ACCENT, Theme};
use crate::toolbar::{DIVIDER_WIDTH, actions_width, divider, icon_of, panel, toolbar_width};
use crate::tooltip::{tooltip_text, with_tooltip};

/// CHARTER 4: Windows 11's close-button red, under the pointer.
const CLOSE_HOVER: Rgba = Rgba {
    r: 0.769,
    g: 0.169,
    b: 0.11,
    a: 1.0,
};

/// The row's sizes, which the window's smallest width adds up (D-161).
const ROW_START: Pixels = px(12.0);
const TOOLS_GAP: Pixels = px(8.0);
const STRIP_GAP: Pixels = px(8.0);
/// A strip group after another is set off from it by a hairline 8 px to its left.
const SEPARATOR: Pixels = px(8.0 + 1.0);
const MORE_PROPERTIES_SIZE: Pixels = px(28.0);
const DRAG_SPACE_LEAST: Pixels = px(40.0);
const CAPTION_BUTTON: Pixels = px(46.0);
const MORE_PROPERTIES: &str = "More properties";

/// The editor's caption row: `toolbar` and the property strip's groups, as [`caption_row`]'s
/// controls. The strip's groups that do not fit go behind a button that opens them (D-161).
#[derive(IntoElement)]
pub(crate) struct CaptionRow {
    pub toolbar: AnyElement,
    pub strip: Vec<(StripGroup, AnyElement)>,
    pub actions: AnyElement,
    pub theme: Theme,
}

/// The editor window's smallest width: the row with every strip group behind its button
/// (D-161).
pub(crate) fn min_width(cx: &App) -> Pixels {
    ROW_START
        + toolbar_width()
        + TOOLS_GAP * 2.0
        + DIVIDER_WIDTH
        + MORE_PROPERTIES_SIZE
        + DRAG_SPACE_LEAST
        + actions_width(cx)
        + CAPTION_BUTTON * 3.0
}

/// What the row measured on the frame before, in window pixels, so that this frame fits the
/// strip between the tools and the drag space (D-161).
#[derive(Default)]
struct StripFit {
    scale: f32,
    /// The strip's left edge.
    left: Option<Pixels>,
    /// What the row needs right of the strip: the drag space's least, `actions` and the caption
    /// buttons.
    right: Option<Pixels>,
    /// Each group's width as last laid out, without its separator.
    widths: HashMap<StripGroup, Pixels>,
}

impl StripFit {
    /// How many of `groups` lead the strip in a window `width` wide: all of them until every
    /// one has been measured.
    fn shown(&self, groups: &[StripGroup], width: Pixels) -> usize {
        let (Some(left), Some(right)) = (self.left, self.right) else {
            return groups.len();
        };
        let placed: Option<Vec<f32>> = groups
            .iter()
            .enumerate()
            .map(|(index, group)| {
                let width = *self.widths.get(group)?;
                Some(f32::from(if index > 0 { width + SEPARATOR } else { width }))
            })
            .collect();
        let Some(placed) = placed else {
            return groups.len();
        };
        groups_that_fit(
            &placed,
            f32::from(width - left - right),
            f32::from(STRIP_GAP),
            f32::from(MORE_PROPERTIES_SIZE),
        )
    }

    /// Takes the strip's left edge and its shown groups' widths; whether any changed.
    fn measured_strip(&mut self, left: Pixels, widths: Vec<(StripGroup, Pixels)>) -> bool {
        let changed = self.left != Some(left)
            || widths
                .iter()
                .any(|(group, width)| self.widths.get(group) != Some(width));
        self.left = Some(left);
        self.widths.extend(widths);
        changed
    }
}

/// Stores what `change` measured once this frame is drawn, and draws again if it differs.
fn remeasure(
    fit: &Entity<StripFit>,
    cx: &mut App,
    change: impl FnOnce(&mut StripFit) -> bool + 'static,
) {
    let fit = fit.clone();
    cx.defer(move |cx| {
        fit.update(cx, |fit, cx| {
            if change(fit) {
                cx.notify();
            }
        });
    });
}

impl RenderOnce for CaptionRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            toolbar,
            strip,
            actions,
            theme,
        } = self;
        let fit = window.use_keyed_state("strip-fit", cx, |_, _| StripFit::default());
        let scale = window.scale_factor();
        if fit.read(cx).scale.to_bits() != scale.to_bits() {
            fit.update(cx, |fit, _| {
                *fit = StripFit {
                    scale,
                    ..StripFit::default()
                };
            });
        }
        let keys: Vec<StripGroup> = strip.iter().map(|&(group, _)| group).collect();
        let shown = fit.read(cx).shown(&keys, window.viewport_size().width);
        let mut groups = strip.into_iter().map(|(_, group)| group);
        let placed: Vec<AnyElement> = groups
            .by_ref()
            .take(shown)
            .enumerate()
            .map(|(index, group)| {
                if index == 0 {
                    return group;
                }
                div()
                    .pl(SEPARATOR - px(1.0))
                    .border_l_1()
                    .border_color(theme.hairline)
                    .child(group)
                    .into_any_element()
            })
            .collect();
        let hidden: Vec<AnyElement> = groups.collect();
        let open = window.use_keyed_state("more-properties", cx, |_, _| false);
        if hidden.is_empty() && *open.read(cx) {
            open.update(cx, |open, _| *open = false);
        }
        let more = (!hidden.is_empty()).then(|| more_properties(hidden, &open, theme, cx));
        let measured = keys[..shown].to_vec();
        let strip_fit = fit.clone();
        let strip = div()
            .on_children_prepainted(move |bounds, _, cx| {
                let Some(first) = bounds.first() else {
                    return;
                };
                let left = first.origin.x;
                let widths: Vec<(StripGroup, Pixels)> = measured
                    .iter()
                    .zip(&bounds)
                    .enumerate()
                    .map(|(index, (&group, bounds))| {
                        let separator = if index > 0 { SEPARATOR } else { px(0.0) };
                        (group, bounds.size.width - separator)
                    })
                    .collect();
                remeasure(&strip_fit, cx, move |fit| fit.measured_strip(left, widths));
            })
            .flex()
            .items_center()
            .flex_none()
            .gap(STRIP_GAP)
            .id("property-strip")
            .role(Role::Toolbar)
            .aria_label("Properties")
            .children(placed)
            .children(more);
        let controls = div()
            .flex()
            .items_center()
            .gap(TOOLS_GAP)
            .child(toolbar)
            .child(divider(theme))
            .child(strip);
        caption_row(controls, actions, window, theme).on_children_prepainted(
            move |bounds, window, cx| {
                let actions = bounds.get(2).expect("the row's third child is the actions");
                let right = window.viewport_size().width - actions.origin.x + DRAG_SPACE_LEAST;
                remeasure(&fit, cx, move |fit| {
                    let changed = fit.right != Some(right);
                    fit.right = Some(right);
                    changed
                });
            },
        )
    }
}

/// The window's own caption row, in place of a title bar (D-141): `controls` from the left,
/// clipped if the window is too narrow for them, then empty space that moves the window, then
/// `actions`, then the caption buttons.
pub(crate) fn caption_row(
    controls: impl IntoElement,
    actions: impl IntoElement,
    window: &Window,
    theme: Theme,
) -> Div {
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
                .pl(ROW_START)
                .child(controls),
        )
        .child(drag_space())
        .child(actions)
        .child(caption_buttons(window, theme))
}

/// The button the strip's `hidden` groups go behind, with them in a panel under it while
/// `open` (D-161).
fn more_properties(
    hidden: Vec<AnyElement>,
    open: &Entity<bool>,
    theme: Theme,
    cx: &App,
) -> impl IntoElement + use<> {
    let shown = *open.read(cx);
    let toggle = open.clone();
    let button = div()
        .id(MORE_PROPERTIES)
        .focusable()
        .tab_stop(true)
        .role(Role::Button)
        .aria_label(MORE_PROPERTIES)
        .aria_expanded(shown)
        .map(|button| with_tooltip(button, tooltip_text(MORE_PROPERTIES, None), theme))
        .focus_visible(|button| button.border_2().border_color(ACCENT))
        .size(MORE_PROPERTIES_SIZE)
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.0))
        .hover(move |button| button.bg(theme.hover))
        .on_click(move |_, _, cx| {
            toggle.update(cx, |open, cx| {
                *open = !*open;
                cx.notify();
            });
        })
        .child(icon_of("chevron-down", theme.text));
    let groups = shown.then(|| {
        div().absolute().top(px(32.0)).right_0().child(deferred(
            anchored().anchor(Anchor::TopRight).snap_to_window().child(
                panel(theme)
                    .flex_col()
                    .items_start()
                    .gap(STRIP_GAP)
                    .id("more-properties-panel")
                    .role(Role::Group)
                    .aria_label(MORE_PROPERTIES)
                    .children(hidden),
            ),
        ))
    });
    div().relative().child(button).children(groups)
}

/// The row's empty space, which moves the window as a title bar does: a drag moves it, a
/// double-click maximises it, and Windows snaps it (D-141).
fn drag_space() -> impl IntoElement {
    div()
        .id("caption-drag")
        .flex_1()
        .min_w(DRAG_SPACE_LEAST)
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
        .w(CAPTION_BUTTON)
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .window_control_area(area)
        .child(glyph)
}
