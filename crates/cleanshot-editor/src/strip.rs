use cleanshot_core::{
    Alignment, Command, Fill, Frame, Horizontal, Look, Property, Setting, SpotlightDim, Vertical,
};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Div, ElementId, Entity, InteractiveElement, IntoElement, ParentElement, RenderOnce, Role,
    Stateful, StatefulInteractiveElement, Styled, Toggled, Window, anchored, deferred, div, point,
    px,
};

use crate::canvas::rgba;
use crate::theme::{ACCENT, Theme};
use crate::toolbar::panel;
use crate::tooltip::{tooltip_text, with_tooltip};

/// The property strip under the toolbar (CHARTER 4, D-55): a radio group for each of
/// `settings`, with `look`'s values checked, then the document's spotlight `dim` where there is
/// one to show (D-123), then its frame toggles (F-44). `on_set` is called with the choice picked
/// and `on_document` with the document change, by click, Enter or Space.
pub(crate) fn property_strip(
    settings: &[Setting],
    look: Look,
    frame: Frame,
    dim: Option<SpotlightDim>,
    theme: Theme,
    on_set: impl Fn(Property, &mut Window, &mut App) + Clone + 'static,
    on_document: impl Fn(Command, &mut Window, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let groups = settings
        .iter()
        .enumerate()
        .map(|(index, &setting)| match setting {
            Setting::Shadow => {
                let on_set = on_set.clone();
                let toggled = Property::Shadow(!look.shadow());
                let name = setting.name();
                let check =
                    strip_button(name.to_owned(), Role::CheckBox, name, look.shadow(), theme)
                        .on_click(move |_, window, cx| on_set(toggled, window, cx))
                        .child(div().text_xs().text_color(theme.text).child(name));
                group(name, Role::Group, index > 0, theme).child(check)
            }
            Setting::Alignment | Setting::HorizontalAlignment => {
                group(setting.name(), Role::Group, index > 0, theme).child(AlignmentPicker {
                    setting,
                    look,
                    theme,
                    on_set: on_set.clone(),
                })
            }
            Setting::Color
            | Setting::StrokeWidth
            | Setting::Fill
            | Setting::Redact
            | Setting::RedactStrength
            | Setting::HighlightStrength
            | Setting::TextSize => {
                let choices = setting.choices().iter().map(|&(property, name)| {
                    let on_set = on_set.clone();
                    let id = format!("{}: {name}", setting.name());
                    strip_button(id, Role::RadioButton, name, look.has(property), theme)
                        .on_click(move |_, window, cx| on_set(property, window, cx))
                        .child(swatch(property, name, theme))
                });
                group(setting.name(), Role::RadioGroup, index > 0, theme).children(choices)
            }
        });
    let toggles = [
        (
            "Border",
            frame.border,
            Frame {
                border: !frame.border,
                ..frame
            },
        ),
        (
            "Shadow",
            frame.shadow,
            Frame {
                shadow: !frame.shadow,
                ..frame
            },
        ),
    ]
    .map(|(name, checked, toggled)| {
        let on_document = on_document.clone();
        strip_button(
            format!("{IMAGE_FRAME}: {name}"),
            Role::CheckBox,
            name,
            checked,
            theme,
        )
        .on_click(move |_, window, cx| on_document(Command::SetFrame(toggled), window, cx))
        .child(div().text_xs().text_color(theme.text).child(name))
    });
    panel(theme)
        .id("property-strip")
        .role(Role::Toolbar)
        .aria_label("Properties")
        .gap(px(8.0))
        .children(groups)
        .children(dim.map(|dim| dim_group(dim, !settings.is_empty(), theme, on_document)))
        .child(
            group(
                IMAGE_FRAME,
                Role::Group,
                !settings.is_empty() || dim.is_some(),
                theme,
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text)
                    .pr(px(4.0))
                    .child(IMAGE_FRAME),
            )
            .children(toggles),
        )
}

/// The button showing where text sits in its box, which opens a grid of `setting`'s choices,
/// each drawn as where it puts the lines (D-129). A pick or a press outside the grid closes it.
#[derive(IntoElement)]
struct AlignmentPicker<F: Fn(Property, &mut Window, &mut App) + Clone + 'static> {
    setting: Setting,
    look: Look,
    theme: Theme,
    on_set: F,
}

impl<F: Fn(Property, &mut Window, &mut App) + Clone + 'static> RenderOnce for AlignmentPicker<F> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            setting,
            look,
            theme,
            on_set,
        } = self;
        let open = window.use_keyed_state("alignment-grid", cx, |_, _| false);
        let shown = *open.read(cx);
        let set_open = move |entity: &Entity<bool>, value: bool, cx: &mut App| {
            entity.update(cx, |open, cx| {
                *open = value;
                cx.notify();
            });
        };
        let opener = open.clone();
        let button = strip_button(
            format!("{}: open", setting.name()),
            Role::Button,
            setting.name(),
            shown,
            theme,
        )
        .aria_expanded(shown)
        .on_click(move |_, _, cx| set_open(&opener, true, cx))
        .child(alignment_glyph(look.alignment(), theme));
        let grid = shown.then(|| {
            let closer = open.clone();
            let cells = setting.choices().iter().map(|&(property, name)| {
                let (on_set, open) = (on_set.clone(), open.clone());
                strip_button(
                    format!("{}: {name}", setting.name()),
                    Role::RadioButton,
                    name,
                    look.has(property),
                    theme,
                )
                .on_click(move |_, window, cx| {
                    set_open(&open, false, cx);
                    on_set(property, window, cx);
                })
                .child(swatch(property, name, theme))
            });
            deferred(
                anchored().offset(point(px(-6.0), px(10.0))).child(
                    panel(theme)
                        .id("alignment-grid")
                        .role(Role::RadioGroup)
                        .aria_label(setting.name())
                        .grid()
                        .grid_cols(3)
                        .gap(px(2.0))
                        .on_mouse_down_out(move |_, _, cx| set_open(&closer, false, cx))
                        .children(cells),
                ),
            )
        });
        div().flex().flex_col().child(button).children(grid)
    }
}

/// Three lines in a box, set where `alignment` puts text (D-129).
fn alignment_glyph(alignment: Alignment, theme: Theme) -> Div {
    let lines = [10.0, 6.0, 8.0].map(|width| div().w(px(width)).h(px(2.0)).bg(theme.text));
    let frame = div()
        .size(px(18.0))
        .p(px(2.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(theme.hairline)
        .flex()
        .flex_col()
        .gap(px(1.0));
    let frame = match alignment.horizontal {
        Horizontal::Left => frame.items_start(),
        Horizontal::Centre => frame.items_center(),
        Horizontal::Right => frame.items_end(),
    };
    let frame = match alignment.vertical {
        Vertical::Top => frame.justify_start(),
        Vertical::Centre => frame.justify_center(),
        Vertical::Bottom => frame.justify_end(),
    };
    frame.children(lines)
}

/// What the strip calls the border and shadow round the whole image, apart from a shape's own
/// shadow (F-44, D-114).
const IMAGE_FRAME: &str = "Image frame";

/// The spotlight dim's three steps, with `dim` checked (F-43, D-123).
fn dim_group(
    dim: SpotlightDim,
    after: bool,
    theme: Theme,
    on_document: impl Fn(Command, &mut Window, &mut App) + Clone + 'static,
) -> Stateful<Div> {
    let choices = [
        (SpotlightDim::Light, "Light"),
        (SpotlightDim::Medium, "Medium"),
        (SpotlightDim::Dark, "Dark"),
    ]
    .map(|(choice, name)| {
        let on_document = on_document.clone();
        strip_button(
            format!("{SPOTLIGHT_DIM}: {name}"),
            Role::RadioButton,
            name,
            choice == dim,
            theme,
        )
        .on_click(move |_, window, cx| on_document(Command::SetDim(choice), window, cx))
        .child(div().text_xs().text_color(theme.text).child(name))
    });
    group(SPOTLIGHT_DIM, Role::RadioGroup, after, theme).children(choices)
}

const SPOTLIGHT_DIM: &str = "Dim";

/// A labelled row of buttons, `after` another: then set off from it by a hairline.
fn group(name: &'static str, role: Role, after: bool, theme: Theme) -> Stateful<Div> {
    div()
        .id(name)
        .role(role)
        .aria_label(name)
        .map(|element| with_tooltip(element, tooltip_text(name, None), theme))
        .flex()
        .items_center()
        .gap(px(2.0))
        .when(after, |group| {
            group.pl(px(8.0)).border_l_1().border_color(theme.hairline)
        })
}

/// A button in the strip, shown pressed while `checked`, reachable by Tab.
fn strip_button(
    id: String,
    role: Role,
    name: &'static str,
    checked: bool,
    theme: Theme,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .focusable()
        .tab_stop(true)
        .role(role)
        .aria_label(name)
        .aria_toggled(if checked {
            Toggled::True
        } else {
            Toggled::False
        })
        .focus_visible(|button| button.border_2().border_color(ACCENT))
        .h(px(28.0))
        .min_w(px(28.0))
        .px(px(6.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.0))
        .when(checked, |button| button.bg(theme.hover))
        .when(!checked, |button| {
            button.hover(move |button| button.bg(theme.hover))
        })
}

/// How a choice looks on its button: the colour, the line width, the fill, or its name.
fn swatch(property: Property, name: &'static str, theme: Theme) -> impl IntoElement {
    match property {
        Property::Color(color) => div()
            .size(px(16.0))
            .rounded_full()
            .bg(rgba(color))
            .border_1()
            .border_color(theme.hairline),
        Property::StrokeWidth(width) => {
            div().w(px(16.0)).h(px(width)).rounded_full().bg(theme.text)
        }
        Property::Fill(Fill::Hollow) => div()
            .size(px(14.0))
            .rounded(px(3.0))
            .border_2()
            .border_color(theme.text),
        Property::Fill(Fill::Solid) => div().size(px(14.0)).rounded(px(3.0)).bg(theme.text),
        Property::Redact(_) | Property::HighlightStrength(_) | Property::Shadow(_) => {
            div().text_xs().text_color(theme.text).child(name)
        }
        Property::TextSize(size) => div()
            .text_xs()
            .text_color(theme.text)
            .child(format!("{size}")),
        Property::Alignment(alignment) => alignment_glyph(alignment, theme),
        Property::RedactStrength(strength) => div()
            .text_xs()
            .text_color(theme.text)
            .child(format!("{}", strength.cell())),
    }
}
