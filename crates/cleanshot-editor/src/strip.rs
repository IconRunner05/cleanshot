use cleanshot_core::{
    Alignment, ColorValue, Document, Fill, Horizontal, KeyAction, Look, Opacity, Property, Setting,
    Tool, TypedValue, Vertical, Warning, tooltip_text,
};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, Bounds, DispatchPhase, Div, ElementId, Entity, Hitbox, HitboxBehavior,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, RenderOnce, Role, Stateful, StatefulInteractiveElement,
    Styled, Toggled, Window, anchored, canvas, deferred, div, point, px,
};

use crate::canvas::rgba;
use crate::color_picker::ColorField;
use crate::menu::expandable;
use crate::theme::{CORNER, ControlFill, Theme, filled, panel};
use crate::tooltip::with_tooltip;

/// The property strip under the toolbar: a group for each setting `tool`
/// shows in `look`, with `look`'s values checked and the document's own spotlight dim, then
/// the shape's shadow and border where the tool has them.
/// `on_set` is called with the choice picked, by click, Enter or Space, and `on_sample` when
/// the colour picker's "From image" asks for a colour from the screenshot. The caption row
/// lays the groups out.
pub(crate) fn property_strip(
    tool: Tool,
    look: Look,
    document: &Document,
    theme: Theme,
    on_set: impl Fn(Property, &mut Window, &mut App) + Clone + 'static,
    on_sample: impl Fn(ColorValue, &mut Window, &mut App) + Clone + 'static,
) -> Vec<(StripGroup, AnyElement)> {
    // Undo can take the image's dim away from the one its tool remembers.
    let look = look
        .with(Property::Dim(document.dim()))
        .expect("a dim is never refused");
    let (shape, own): (Vec<Setting>, Vec<Setting>) = Setting::shown_for(tool, &look)
        .into_iter()
        .partition(|&setting| in_shape_group(setting));
    let mut groups: Vec<(StripGroup, AnyElement)> = own
        .iter()
        .map(|&setting| {
            let group = setting_group(setting, look, theme, on_set.clone(), on_sample.clone());
            (StripGroup::Setting(setting), group.into_any_element())
        })
        .collect();
    if !shape.is_empty() {
        let group = group(SHAPE, Role::Group, theme)
            .child(group_label(SHAPE, theme))
            .children(shape.iter().map(|&setting| {
                setting_group(setting, look, theme, on_set.clone(), on_sample.clone())
            }));
        groups.push((StripGroup::Shape, group.into_any_element()));
    }
    groups
}

/// A group of the property strip, as the caption row tells them apart to fit them.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum StripGroup {
    Setting(Setting),
    Shape,
    Crop,
}

/// The Crop tool's strip: Apply, then Cancel, each calling `on_action` with its action.
pub(crate) fn crop_strip(
    theme: Theme,
    on_action: impl Fn(KeyAction, &mut Window, &mut App) + Clone + 'static,
) -> Vec<(StripGroup, AnyElement)> {
    let buttons = [
        (KeyAction::ApplyCrop, "Apply", "Enter"),
        (KeyAction::CancelCrop, "Cancel", "Esc"),
    ]
    .map(|(action, name, key)| {
        let on_action = on_action.clone();
        div()
            .id(name)
            .focusable()
            .tab_stop(true)
            .role(Role::Button)
            .aria_label(name)
            .aria_keyshortcuts(key)
            .map(|button| with_tooltip(button, tooltip_text(name, Some(key)), theme))
            .map(|button| filled(button, ControlFill::Neutral, theme))
            .h(px(28.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .rounded(CORNER)
            .hover(move |button| button.bg(theme.hover))
            .on_click(move |_, window, cx| on_action(action, window, cx))
            .child(div().text_xs().text_color(theme.text).child(name))
    });
    let crop = group(CROP, Role::Group, theme)
        .child(group_label(CROP, theme))
        .children(buttons);
    vec![(StripGroup::Crop, crop.into_any_element())]
}

const CROP: &str = "Crop";

/// Whether `setting` sits in the Shape group.
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
        | Setting::HorizontalAlignment
        | Setting::Background
        | Setting::BackgroundColor
        | Setting::Opacity
        | Setting::Dim => false,
    }
}

/// The buttons that set `setting`: a check box for an on-or-off one,
/// which names itself, or a group named for the setting holding the alignment picker or its
/// choices, with the box to type a value where it takes one. Each is named once, so a screen
/// reader says it once.
fn setting_group(
    setting: Setting,
    look: Look,
    theme: Theme,
    on_set: impl Fn(Property, &mut Window, &mut App) + Clone + 'static,
    on_sample: impl Fn(ColorValue, &mut Window, &mut App) + Clone + 'static,
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
        Setting::Background => check(look.background(), Property::Background(!look.background())),
        Setting::Alignment | Setting::HorizontalAlignment => {
            group(name, Role::Group, theme).child(AlignmentPicker {
                setting,
                look,
                theme,
                on_set,
            })
        }
        Setting::Color | Setting::BorderColor | Setting::BackgroundColor => {
            group(name, Role::Group, theme).child(ColorField {
                setting,
                value: setting
                    .colored(&look)
                    .expect("a colour setting holds a colour"),
                theme,
                on_set,
                on_sample,
            })
        }
        Setting::Opacity => group(name, Role::Group, theme).child(OpacitySlider {
            opacity: look.opacity(),
            theme,
            on_set,
        }),
        Setting::StrokeWidth
        | Setting::Fill
        | Setting::Redact
        | Setting::RedactStrength
        | Setting::HighlightStrength
        | Setting::TextSize
        | Setting::BorderWidth
        | Setting::Dim => {
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
            group(name, Role::RadioGroup, theme)
                .children(choices)
                .children(typed)
                .children(warning)
        }
    }
}

/// A group's name, shown at its start.
fn group_label(name: &'static str, theme: Theme) -> Div {
    div()
        .text_xs()
        .text_color(theme.text)
        .pr(px(4.0))
        .child(name)
}

/// What the strip calls the group of the shape's own shadow and border.
const SHAPE: &str = "Shape";

/// The button showing where text sits in its box, which opens a grid of `setting`'s choices,
/// each drawn as where it puts the lines. A pick or a press outside the grid closes it.
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
        let expand = open.clone();
        let button = strip_button(
            format!("{}: open", setting.name()),
            Role::Button,
            setting.name(),
            shown,
            theme,
        )
        .map(|button| {
            expandable(button, shown, move |value, _, cx| {
                set_open(&expand, value, cx);
            })
        })
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
/// description a screen reader gives.
fn warning_label(setting: Setting, warning: Warning, theme: Theme) -> impl IntoElement {
    div()
        .id(ElementId::Name(
            format!("{}: warning", setting.name()).into(),
        ))
        .role(Role::Label)
        .aria_label(warning.label)
        .aria_description(warning.reason)
        .map(|label| with_tooltip(label, warning.reason, theme))
        .px(px(4.0))
        .text_xs()
        .text_color(theme.text)
        .child(warning.label)
}

/// A box to type `setting`'s value in, showing what the look holds until digits are typed.
/// Enter, Tab or a press outside sets the nearest value the setting takes, which the box then
/// shows; Esc leaves the value as it was.
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
            .rounded(CORNER)
            .border_1()
            .border_color(theme.hairline)
            .focus(move |field| field.border_color(theme.accent))
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

/// The fill opacity as a slider naming its value: a press or a drag moves it to the nearest
/// tenth and the release sets it, and Left and Right step it a tenth.
#[derive(IntoElement)]
struct OpacitySlider<F: Fn(Property, &mut Window, &mut App) + Clone + 'static> {
    opacity: Opacity,
    theme: Theme,
    on_set: F,
}

const SLIDER_WIDTH: f32 = 96.0;

impl<F: Fn(Property, &mut Window, &mut App) + Clone + 'static> RenderOnce for OpacitySlider<F> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            opacity,
            theme,
            on_set,
        } = self;
        let name = Setting::Opacity.name();
        let draft = window.use_keyed_state(
            ElementId::Name(format!("{name}: draft").into()),
            cx,
            |_, _| None::<Opacity>,
        );
        let percent = draft.read(cx).unwrap_or(opacity).percent();
        let on_key = on_set.clone();
        let slide = canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, _| {
                listen_on_slide(bounds, hitbox, &draft, &on_set, window);
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        div()
            .id(ElementId::Name(format!("{name}: slider").into()))
            .focusable()
            .tab_stop(true)
            .role(Role::Slider)
            .aria_label(name)
            .aria_value(format!("{percent}%"))
            .aria_numeric_value(f64::from(percent))
            .aria_min_numeric_value(f64::from(Opacity::LEAST.percent()))
            .aria_max_numeric_value(f64::from(Opacity::FULL.percent()))
            .map(|slider| filled(slider, ControlFill::Neutral, theme))
            .relative()
            .h(px(28.0))
            .w(px(SLIDER_WIDTH))
            .rounded(CORNER)
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .h_full()
                    .w(px(SLIDER_WIDTH * f32::from(percent) / 100.0))
                    .bg(theme.hover),
            )
            .child(
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(theme.text)
                    .child(format!("{percent}%")),
            )
            .child(slide)
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                let Some(next) = opacity.stepped(&event.keystroke.key) else {
                    return;
                };
                on_key(Property::Opacity(next), window, cx);
                cx.stop_propagation();
            })
    }
}

/// A press on the slider at `bounds` and the drag after it move `draft`; the release sets it.
fn listen_on_slide(
    bounds: Bounds<Pixels>,
    hitbox: Hitbox,
    draft: &Entity<Option<Opacity>>,
    on_set: &(impl Fn(Property, &mut Window, &mut App) + Clone + 'static),
    window: &mut Window,
) {
    let at = move |x: Pixels| Opacity::at((x - bounds.origin.x) / bounds.size.width);
    let press = draft.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble
            && event.button == MouseButton::Left
            && hitbox.is_hovered(window)
        {
            press.update(cx, |draft, cx| {
                *draft = Some(at(event.position.x));
                cx.notify();
            });
            cx.stop_propagation();
        }
    });
    let drag = draft.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble && event.pressed_button == Some(MouseButton::Left) {
            drag.update(cx, |draft, cx| {
                if draft.is_some() {
                    *draft = Some(at(event.position.x));
                    cx.notify();
                }
            });
        }
    });
    let (release, on_set) = (draft.clone(), on_set.clone());
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
            return;
        }
        let set = release.update(cx, |draft, cx| {
            cx.notify();
            draft.take()
        });
        if let Some(opacity) = set {
            on_set(Property::Opacity(opacity), window, cx);
        }
    });
}

/// Three lines in a box, set where `alignment` puts text.
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

/// A labelled row of buttons.
fn group(name: &'static str, role: Role, theme: Theme) -> Stateful<Div> {
    div()
        .id(name)
        .role(role)
        .aria_label(name)
        .map(|element| with_tooltip(element, tooltip_text(name, None), theme))
        .flex()
        .items_center()
        .gap(px(2.0))
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
        .map(|button| filled(button, ControlFill::Neutral, theme))
        .h(px(28.0))
        .min_w(px(28.0))
        .px(px(6.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(CORNER)
        .when(checked, |button| button.bg(theme.hover))
        .when(!checked, |button| {
            button.hover(move |button| button.bg(theme.hover))
        })
}

/// How a choice looks on its button: the colour, the line width, the fill, or its name.
fn swatch(property: Property, name: &'static str, theme: Theme) -> impl IntoElement {
    match property {
        Property::Color(color)
        | Property::BorderColor(color)
        | Property::BackgroundColor(color) => div()
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
        | Property::Border(_)
        | Property::Background(_)
        | Property::Opacity(_)
        | Property::Dim(_) => div().text_xs().text_color(theme.text).child(name),
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
