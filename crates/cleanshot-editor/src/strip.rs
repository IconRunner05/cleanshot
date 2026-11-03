use cleanshot_core::{Fill, Frame, Look, Property, Setting};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Div, ElementId, InteractiveElement, IntoElement, ParentElement, Role, Stateful,
    StatefulInteractiveElement, Styled, Toggled, Window, div, px,
};

use crate::canvas::rgba;
use crate::theme::{ACCENT, Theme};
use crate::toolbar::panel;
use crate::tooltip::{tooltip_text, with_tooltip};

/// The property strip under the toolbar (CHARTER 4, D-55): a radio group for each of
/// `settings`, with `look`'s values checked, then the document's frame toggles (F-44). `on_set`
/// is called with the choice picked and `on_frame` with the frame toggled, by click, Enter or
/// Space.
pub(crate) fn property_strip(
    settings: &[Setting],
    look: Look,
    frame: Frame,
    theme: Theme,
    on_set: impl Fn(Property, &mut Window, &mut App) + Clone + 'static,
    on_frame: impl Fn(Frame, &mut Window, &mut App) + Clone + 'static,
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
        let on_frame = on_frame.clone();
        strip_button(
            format!("{IMAGE_FRAME}: {name}"),
            Role::CheckBox,
            name,
            checked,
            theme,
        )
        .on_click(move |_, window, cx| on_frame(toggled, window, cx))
        .child(div().text_xs().text_color(theme.text).child(name))
    });
    panel(theme)
        .id("property-strip")
        .role(Role::Toolbar)
        .aria_label("Properties")
        .gap(px(8.0))
        .children(groups)
        .child(
            group(IMAGE_FRAME, Role::Group, !settings.is_empty(), theme)
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

/// What the strip calls the border and shadow round the whole image, apart from a shape's own
/// shadow (F-44, D-114).
const IMAGE_FRAME: &str = "Image frame";

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
        Property::RedactStrength(strength) => div()
            .text_xs()
            .text_color(theme.text)
            .child(format!("{}", strength.cell())),
    }
}
