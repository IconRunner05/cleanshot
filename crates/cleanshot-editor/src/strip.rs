use cleanshot_core::{Fill, Look, Property, Setting};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, Role,
    StatefulInteractiveElement, Styled, Toggled, Window, div, px,
};

use crate::canvas::rgba;
use crate::theme::{ACCENT, Theme};
use crate::toolbar::panel;

/// The property strip under the toolbar (CHARTER 4, D-55): a radio group for each of
/// `settings`, with `look`'s values checked. `on_set` is called with the choice picked by click,
/// Enter or Space.
pub(crate) fn property_strip(
    settings: &[Setting],
    look: Look,
    theme: Theme,
    on_set: impl Fn(Property, &mut Window, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let groups = settings.iter().enumerate().map(|(index, setting)| {
        let choices = setting.choices().iter().map(|&(property, name)| {
            let on_set = on_set.clone();
            let checked = look.has(property);
            div()
                .id(ElementId::Name(
                    format!("{}: {name}", setting.name()).into(),
                ))
                .focusable()
                .tab_stop(true)
                .role(Role::RadioButton)
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
                .on_click(move |_, window, cx| on_set(property, window, cx))
                .child(swatch(property, name, theme))
        });
        div()
            .id(setting.name())
            .role(Role::RadioGroup)
            .aria_label(setting.name())
            .flex()
            .items_center()
            .gap(px(2.0))
            .when(index > 0, |group| {
                group.pl(px(8.0)).border_l_1().border_color(theme.hairline)
            })
            .children(choices)
    });
    panel(theme)
        .id("property-strip")
        .role(Role::Toolbar)
        .aria_label("Properties")
        .gap(px(8.0))
        .children(groups)
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
        Property::Redact(_) => div().text_xs().text_color(theme.text).child(name),
        Property::TextSize(size) => div()
            .text_xs()
            .text_color(theme.text)
            .child(format!("{size}")),
    }
}
