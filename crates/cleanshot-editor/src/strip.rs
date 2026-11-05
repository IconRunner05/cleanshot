use cleanshot_core::{
    Alignment, Command, Fill, Frame, Horizontal, Look, Property, Setting, SpotlightDim, TypedValue,
    Vertical, Warning,
};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Div, ElementId, Entity, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    RenderOnce, Role, Stateful, StatefulInteractiveElement, Styled, Toggled, Window, anchored,
    deferred, div, point, px,
};

use crate::canvas::rgba;
use crate::theme::{ACCENT, Theme};
use crate::toolbar::panel;
use crate::tooltip::{tooltip_text, with_tooltip};

/// The property strip under the toolbar (CHARTER 4, D-55): a group for each of `settings`, with
/// `look`'s values checked, then the document's spotlight `dim` where there is one to show
/// (D-123), then the shape's shadow and border where `settings` has them, or else the image's
/// frame toggles (F-44, D-131). `on_set` is called with the choice picked and `on_document`
/// with the document change, by click, Enter or Space.
pub(crate) fn property_strip(
    settings: Vec<Setting>,
    look: Look,
    frame: Frame,
    dim: Option<SpotlightDim>,
    theme: Theme,
    on_set: impl Fn(Property, &mut Window, &mut App) + Clone + 'static,
    on_document: impl Fn(Command, &mut Window, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let (shape, own): (Vec<Setting>, Vec<Setting>) = settings
        .into_iter()
        .partition(|&setting| in_shape_group(setting));
    let groups = own
        .iter()
        .enumerate()
        .map(|(index, &setting)| setting_group(setting, index > 0, look, theme, on_set.clone()));
    let after = !own.is_empty() || dim.is_some();
    let last = if shape.is_empty() {
        image_frame_group(frame, after, theme, on_document.clone())
    } else {
        group(SHAPE, Role::Group, after, theme)
            .child(group_label(SHAPE, theme))
            .children(
                shape
                    .iter()
                    .map(|&setting| setting_group(setting, false, look, theme, on_set.clone())),
            )
    };
    panel(theme)
        .id("property-strip")
        .role(Role::Toolbar)
        .aria_label("Properties")
        .gap(px(8.0))
        .children(groups)
        .children(dim.map(|dim| dim_group(dim, !own.is_empty(), theme, on_document)))
        .child(last)
}

/// Whether `setting` sits in the Shape group, which takes the image frame's place (D-131).
fn in_shape_group(setting: Setting) -> bool {
    match setting {
        Setting::Shadow | Setting::Border | Setting::BorderColor | Setting::BorderWidth => true,
        Setting::Color
        | Setting::StrokeWidth
        | Setting::Fill
        | Setting::Redact
        | Setting::RedactStrength
        | Setting::HighlightStrength
        | Setting::TextSize
        | Setting::Alignment
        | Setting::HorizontalAlignment => false,
    }
}

/// The buttons that set `setting`, `after` another group: a check box for an on-or-off one,
/// which names itself, or a group named for the setting holding the alignment picker or its
/// choices, with the box to type a value where it takes one. Each is named once, so a screen
/// reader says it once.
fn setting_group(
    setting: Setting,
    after: bool,
    look: Look,
    theme: Theme,
    on_set: impl Fn(Property, &mut Window, &mut App) + Clone + 'static,
) -> Stateful<Div> {
    let name = setting.name();
    let check = |checked: bool, toggled: Property| {
        let on_set = on_set.clone();
        strip_button(name.to_owned(), Role::CheckBox, name, checked, theme)
            .on_click(move |_, window, cx| on_set(toggled, window, cx))
            .child(div().text_xs().text_color(theme.text).child(name))
    };
    match setting {
        Setting::Shadow => check(look.shadow(), Property::Shadow(!look.shadow())),
        Setting::Border => check(look.border(), Property::Border(!look.border())),
        Setting::Alignment | Setting::HorizontalAlignment => group(name, Role::Group, after, theme)
            .child(AlignmentPicker {
                setting,
                look,
                theme,
                on_set,
            }),
        Setting::Color
        | Setting::StrokeWidth
        | Setting::Fill
        | Setting::Redact
        | Setting::RedactStrength
        | Setting::HighlightStrength
        | Setting::TextSize
        | Setting::BorderColor
        | Setting::BorderWidth => {
            let choices = setting.choices().iter().map(|&(property, choice)| {
                let on_set = on_set.clone();
                let id = format!("{name}: {choice}");
                strip_button(id, Role::RadioButton, choice, look.has(property), theme)
                    .on_click(move |_, window, cx| on_set(property, window, cx))
                    .child(swatch(property, choice, theme))
            });
            let typed = setting.typed(&look);
            let warning = typed
                .and_then(TypedValue::warning)
                .map(|warning| warning_label(setting, warning, theme));
            let typed = typed.map(|typed| TypedField {
                setting,
                typed,
                theme,
                on_set: on_set.clone(),
            });
            group(name, Role::RadioGroup, after, theme)
                .children(choices)
                .children(typed)
                .children(warning)
        }
    }
}

/// The image frame's Border and Shadow, shown when the strip has no shape to act on (F-44).
fn image_frame_group(
    frame: Frame,
    after: bool,
    theme: Theme,
    on_document: impl Fn(Command, &mut Window, &mut App) + Clone + 'static,
) -> Stateful<Div> {
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
    group(IMAGE_FRAME, Role::Group, after, theme)
        .child(group_label(IMAGE_FRAME, theme))
        .children(toggles)
}

/// A group's name, shown at its start.
fn group_label(name: &'static str, theme: Theme) -> Div {
    div()
        .text_xs()
        .text_color(theme.text)
        .pr(px(4.0))
        .child(name)
}

/// What the strip calls the group of the shape's own shadow and border (D-131).
const SHAPE: &str = "Shape";

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

/// The warning beside `setting`'s typed value, with its reason as the tooltip and the
/// description a screen reader gives (D-132).
fn warning_label(setting: Setting, warning: Warning, theme: Theme) -> impl IntoElement {
    div()
        .id(ElementId::Name(
            format!("{}: warning", setting.name()).into(),
        ))
        .role(Role::Label)
        .aria_label(warning.label)
        .aria_description(warning.reason)
        .map(|label| with_tooltip(label, warning.reason.into(), theme))
        .px(px(4.0))
        .text_xs()
        .text_color(theme.text)
        .child(warning.label)
}

/// A box to type `setting`'s value in, showing what the look holds until digits are typed.
/// Enter, Tab or a press outside sets the nearest value the setting takes, which the box then
/// shows; Esc leaves the value as it was (D-130).
#[derive(IntoElement)]
struct TypedField<F: Fn(Property, &mut Window, &mut App) + Clone + 'static> {
    setting: Setting,
    typed: TypedValue,
    theme: Theme,
    on_set: F,
}

/// The most digits the box takes: every value a setting takes has three or fewer.
const TYPED_DIGITS: usize = 3;

impl<F: Fn(Property, &mut Window, &mut App) + Clone + 'static> RenderOnce for TypedField<F> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            setting,
            typed,
            theme,
            on_set,
        } = self;
        let id = format!("{}: typed", setting.name());
        let draft =
            window.use_keyed_state(ElementId::Name(format!("{id} draft").into()), cx, |_, _| {
                None::<String>
            });
        let focus = window
            .use_keyed_state(
                ElementId::Name(format!("{id} focus").into()),
                cx,
                |_, cx| cx.focus_handle(),
            )
            .read(cx)
            .clone();
        let set = move |draft: &Entity<Option<String>>, window: &mut Window, cx: &mut App| {
            let digits = draft.update(cx, |draft, cx| {
                cx.notify();
                draft.take()
            });
            if let Some(digits) = digits.filter(|digits| !digits.is_empty()) {
                let value = digits
                    .parse()
                    .expect("at most three ASCII digits make a u16");
                on_set(typed.property(value), window, cx);
            }
        };
        let shown = draft
            .read(cx)
            .clone()
            .filter(|_| focus.is_focused(window))
            .unwrap_or_else(|| typed.shown().to_string());
        let range = typed.range();
        let (on_key, on_out) = (draft.clone(), draft);
        let set_on_out = set.clone();
        let focus_out = focus.clone();
        div()
            .id(ElementId::Name(id.into()))
            .track_focus(&focus)
            .tab_stop(true)
            .role(Role::SpinButton)
            .aria_label(setting.name())
            .aria_numeric_value(f64::from(typed.shown()))
            .aria_min_numeric_value(f64::from(*range.start()))
            .aria_max_numeric_value(f64::from(*range.end()))
            .h(px(28.0))
            .w(px(40.0))
            .px(px(6.0))
            .flex()
            .items_center()
            .justify_end()
            .rounded(px(7.0))
            .border_1()
            .border_color(theme.hairline)
            .focus(|field| field.border_color(ACCENT))
            .text_xs()
            .text_color(theme.text)
            .on_mouse_down_out(move |_, window, cx| {
                if focus_out.is_focused(window) {
                    set_on_out(&on_out, window, cx);
                } else {
                    on_out.update(cx, |draft, _| *draft = None);
                }
            })
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                if event.keystroke.modifiers.modified() {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "enter" => set(&on_key, window, cx),
                    "tab" => {
                        set(&on_key, window, cx);
                        return;
                    }
                    "escape" => on_key.update(cx, |draft, cx| {
                        *draft = None;
                        cx.notify();
                    }),
                    "backspace" => on_key.update(cx, |draft, cx| {
                        draft.get_or_insert_default().pop();
                        cx.notify();
                    }),
                    digit if digit.len() == 1 && digit.chars().all(|c| c.is_ascii_digit()) => {
                        on_key.update(cx, |draft, cx| {
                            let digits = draft.get_or_insert_default();
                            if digits.len() < TYPED_DIGITS {
                                digits.push_str(digit);
                            }
                            cx.notify();
                        });
                    }
                    _ => return,
                }
                cx.stop_propagation();
            })
            .child(shown)
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
        Property::Color(color) | Property::BorderColor(color) => div()
            .size(px(16.0))
            .rounded_full()
            .bg(rgba(color))
            .border_1()
            .border_color(theme.hairline),
        Property::StrokeWidth(width) => {
            div().w(px(16.0)).h(px(width)).rounded_full().bg(theme.text)
        }
        Property::BorderWidth(width) => div()
            .w(px(16.0))
            .h(px(f32::from(width.pixels())))
            .rounded_full()
            .bg(theme.text),
        Property::Fill(Fill::Hollow) => div()
            .size(px(14.0))
            .rounded(px(3.0))
            .border_2()
            .border_color(theme.text),
        Property::Fill(Fill::Solid) => div().size(px(14.0)).rounded(px(3.0)).bg(theme.text),
        Property::Redact(_)
        | Property::HighlightStrength(_)
        | Property::Shadow(_)
        | Property::Border(_) => div().text_xs().text_color(theme.text).child(name),
        Property::TextSize(size) => div()
            .text_xs()
            .text_color(theme.text)
            .child(format!("{size}")),
        Property::Alignment(alignment) => alignment_glyph(alignment, theme),
        Property::RedactStrength(strength) => div()
            .text_xs()
            .text_color(theme.text)
            .child(format!("{}", strength.pixels())),
    }
}
