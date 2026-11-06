use std::rc::Rc;

use cleanshot_core::{Color, ColorValue, Property, Setting};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, Bounds, BoxShadow, Canvas, ClickEvent, DispatchPhase, Div, ElementId, Entity,
    FocusHandle, Global, Hitbox, HitboxBehavior, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    RenderOnce, Role, SharedString, Stateful, StatefulInteractiveElement, Styled, Toggled, Window,
    anchored, black, canvas, deferred, div, hsla, linear_color_stop, linear_gradient, point, px,
    white,
};

use crate::canvas::rgba;
use crate::theme::{ACCENT, Theme};
use crate::toolbar::panel;
use crate::tooltip::{tooltip_text, with_tooltip};

/// How many picked colours the Recent row keeps.
const RECENT: usize = 8;
const SHADE_HEIGHT: f32 = 150.0;
const PICKER_WIDTH: f32 = 248.0;
const HUE_HEIGHT: f32 = 12.0;
const MARKER: f32 = 14.0;
/// The hue bar's stops: red, yellow, green, cyan, blue, magenta and red again.
const HUE_STOPS: u16 = 6;

/// The colours picked in the picker, newest first. D-147: they last only while Cleanshot runs.
#[derive(Default)]
struct RecentColors(Vec<Color>);

impl Global for RecentColors {}

fn remember_recent(color: Color, cx: &mut App) {
    let recent = &mut cx.default_global::<RecentColors>().0;
    recent.retain(|&kept| kept != color);
    recent.insert(0, color);
    recent.truncate(RECENT);
}

fn recent_colors(cx: &App) -> Vec<Color> {
    cx.try_global::<RecentColors>()
        .map(|recent| recent.0.clone())
        .unwrap_or_default()
}

/// A colour as the picker moves it: `hue` in degrees from 0 up to 360, `saturation` and
/// `value` from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Hsv {
    hue: f32,
    saturation: f32,
    value: f32,
}

impl Hsv {
    fn of(color: Color) -> Self {
        let top = color.r.max(color.g).max(color.b);
        let [r, g, b] = [color.r, color.g, color.b].map(|channel| f32::from(channel) / 255.0);
        let most = f32::from(top) / 255.0;
        let range = most - r.min(g).min(b);
        let hue = if range == 0.0 {
            0.0
        } else if top == color.r {
            60.0 * ((g - b) / range).rem_euclid(6.0)
        } else if top == color.g {
            60.0 * ((b - r) / range + 2.0)
        } else {
            60.0 * ((r - g) / range + 4.0)
        };
        Self {
            hue,
            saturation: if most == 0.0 { 0.0 } else { range / most },
            value: most,
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "each channel is rounded from 0 to 255 before the cast"
    )]
    fn color(self) -> Color {
        let chroma = self.value * self.saturation;
        let sector = (self.hue.rem_euclid(360.0)) / 60.0;
        let second = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
        let (r, g, b) = match sector as u8 {
            0 => (chroma, second, 0.0),
            1 => (second, chroma, 0.0),
            2 => (0.0, chroma, second),
            3 => (0.0, second, chroma),
            4 => (second, 0.0, chroma),
            _ => (chroma, 0.0, second),
        };
        let lift = self.value - chroma;
        let channel = |part: f32| ((part + lift).clamp(0.0, 1.0) * 255.0).round() as u8;
        Color {
            r: channel(r),
            g: channel(g),
            b: channel(b),
            a: 255,
        }
    }
}

/// Which page of the colour popup is open.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Popup {
    Palette,
    Picker { draft: Hsv, held: Option<Held> },
}

/// The picker control a press went down on, which drags until the button comes up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Held {
    Shade,
    Hue,
}

/// A colour setting on the property strip (D-147): a button showing the colour, which opens
/// the palette, whose last swatch opens the picker. `on_set` gets the colour picked;
/// `on_sample` asks the editor to take the next colour from the screenshot.
#[derive(IntoElement)]
pub(crate) struct ColorField<F, S>
where
    F: Fn(Property, &mut Window, &mut App) + Clone + 'static,
    S: Fn(ColorValue, &mut Window, &mut App) + Clone + 'static,
{
    pub setting: Setting,
    pub value: ColorValue,
    pub theme: Theme,
    pub on_set: F,
    pub on_sample: S,
}

impl<F, S> RenderOnce for ColorField<F, S>
where
    F: Fn(Property, &mut Window, &mut App) + Clone + 'static,
    S: Fn(ColorValue, &mut Window, &mut App) + Clone + 'static,
{
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let name = self.setting.name();
        let popup = window.use_keyed_state(
            ElementId::Name(format!("{name} popup").into()),
            cx,
            |_, _| None::<Popup>,
        );
        let hex_focus = window
            .use_keyed_state(
                ElementId::Name(format!("{name} hex focus").into()),
                cx,
                |_, cx| cx.focus_handle(),
            )
            .read(cx)
            .clone();
        let hex_draft =
            window.use_keyed_state(ElementId::Name(format!("{name} hex").into()), cx, |_, _| {
                None::<String>
            });
        let open = *popup.read(cx);
        let shown = self.value.shown();
        let toggle = popup.clone();
        let button = div()
            .id(ElementId::Name(format!("{name}: button").into()))
            .focusable()
            .tab_stop(true)
            .role(Role::Button)
            .aria_label(name)
            .aria_value(describe(shown))
            .aria_expanded(open.is_some())
            .map(|element| with_tooltip(element, tooltip_text(name, None), self.theme))
            .focus_visible(|button| button.border_2().border_color(ACCENT))
            .h(px(28.0))
            .w(px(30.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.0))
            .when(open.is_some(), |button| button.bg(self.theme.hover))
            .hover(move |button| button.bg(self.theme.hover))
            .on_click(move |_, _, cx| {
                toggle.update(cx, |popup, cx| {
                    *popup = popup.is_none().then_some(Popup::Palette);
                    cx.notify();
                });
            })
            .child(dot(shown, 16.0).border_1().border_color(self.theme.text));
        let shown_popup = open.map(|page| {
            let field = Field {
                name,
                value: self.value,
                theme: self.theme,
                popup: popup.clone(),
                on_set: Rc::new(self.on_set.clone()),
            };
            let body = match page {
                Popup::Palette => field.palette(cx),
                Popup::Picker { draft, .. } => field.picker(
                    draft,
                    &hex_focus,
                    &hex_draft,
                    self.on_sample.clone(),
                    window,
                    cx,
                ),
            };
            let close = popup.clone();
            let escape = popup.clone();
            let element = panel(self.theme)
                .id(ElementId::Name(format!("{name}: popup").into()))
                .role(Role::Dialog)
                .aria_label(name)
                .flex_col()
                .items_start()
                .gap(px(10.0))
                .p(px(12.0))
                .rounded(px(14.0))
                .on_mouse_down_out(move |_, _, cx| close_popup(&close, cx))
                .on_key_down(move |event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        close_popup(&escape, cx);
                        cx.stop_propagation();
                    }
                })
                .children(body);
            div()
                .absolute()
                .top(px(32.0))
                .left_0()
                .child(deferred(anchored().snap_to_window().child(element)))
        });
        div().relative().child(button).children(shown_popup)
    }
}

type OnColor = Rc<dyn Fn(Property, &mut Window, &mut App)>;

/// What every part of an open popup needs.
struct Field {
    name: &'static str,
    value: ColorValue,
    theme: Theme,
    popup: Entity<Option<Popup>>,
    on_set: OnColor,
}

impl Field {
    /// The 11 colours, the rainbow swatch that opens the picker, and the Recent row.
    fn palette(&self, cx: &App) -> Vec<AnyElement> {
        let mut swatches: Vec<AnyElement> = ColorValue::palette()
            .iter()
            .map(|&(color, choice)| {
                let checked = color == self.value.shown();
                let pick = self.picks(color);
                swatch_button(format!("{}: {choice}", self.name), choice, self.theme)
                    .role(Role::RadioButton)
                    .aria_toggled(if checked {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .size(px(24.0))
                    .bg(rgba(color))
                    .when(checked, |swatch| {
                        swatch.border_2().border_color(self.theme.text)
                    })
                    .when(!checked, |swatch| {
                        swatch.border_1().border_color(self.theme.hairline)
                    })
                    .on_click(move |_, window, cx| pick(window, cx))
                    .into_any_element()
            })
            .collect();
        let popup = self.popup.clone();
        let shown = self.value.shown();
        swatches.push(
            swatch_button(
                format!("{}: custom", self.name),
                "Custom colour",
                self.theme,
            )
            .role(Role::Button)
            .size(px(24.0))
            .overflow_hidden()
            .border_1()
            .border_color(self.theme.hairline)
            .child(hue_strip().size_full())
            .on_click(move |_, _, cx| {
                popup.update(cx, |popup, cx| {
                    *popup = Some(Popup::Picker {
                        draft: Hsv::of(shown),
                        held: None,
                    });
                    cx.notify();
                });
            })
            .into_any_element(),
        );
        let mut rows = Vec::new();
        let mut swatches = swatches.into_iter();
        loop {
            let row: Vec<AnyElement> = swatches.by_ref().take(6).collect();
            if row.is_empty() {
                break;
            }
            rows.push(div().flex().gap(px(10.0)).children(row));
        }
        let palette = div()
            .id(ElementId::Name(format!("{}: palette", self.name).into()))
            .role(Role::RadioGroup)
            .aria_label(self.name)
            .flex()
            .flex_col()
            .gap(px(10.0))
            .children(rows)
            .into_any_element();
        let mut parts = vec![palette];
        parts.extend(self.recent_row(cx, None));
        parts
    }

    /// Sets `color` and closes the popup.
    fn picks(&self, color: Color) -> impl Fn(&mut Window, &mut App) + 'static {
        let (on_set, popup, property) = (
            Rc::clone(&self.on_set),
            self.popup.clone(),
            self.value.property(color),
        );
        move |window, cx| {
            on_set(property, window, cx);
            close_popup(&popup, cx);
        }
    }

    fn recent_row(&self, cx: &App, back: Option<AnyElement>) -> Option<AnyElement> {
        let recent = recent_colors(cx);
        if recent.is_empty() && back.is_none() {
            return None;
        }
        let dots = recent.into_iter().map(|color| {
            let pick = self.picks(color);
            let hex = color.hex();
            let name = format!("Recent {}", describe(color));
            swatch_button(format!("{}: recent {hex}", self.name), name, self.theme)
                .role(Role::Button)
                .size(px(16.0))
                .bg(rgba(color))
                .border_1()
                .border_color(self.theme.hairline)
                .on_click(move |_, window, cx| pick(window, cx))
        });
        Some(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_xs()
                .text_color(self.theme.text)
                .child("Recent")
                .children(dots)
                .child(div().flex_1())
                .children(back)
                .into_any_element(),
        )
    }

    /// The shade square, the hue bar, the hex box, "From image", and the Recent row.
    fn picker<S>(
        &self,
        draft: Hsv,
        hex_focus: &FocusHandle,
        hex_draft: &Entity<Option<String>>,
        on_sample: S,
        window: &Window,
        cx: &App,
    ) -> Vec<AnyElement>
    where
        S: Fn(ColorValue, &mut Window, &mut App) + 'static,
    {
        let pure_hue = Hsv {
            saturation: 1.0,
            value: 1.0,
            ..draft
        }
        .color();
        let shade = div()
            .relative()
            .w(px(PICKER_WIDTH))
            .h(px(SHADE_HEIGHT))
            .rounded(px(8.0))
            .overflow_hidden()
            .bg(rgba(pure_hue))
            .child(div().absolute().size_full().bg(linear_gradient(
                90.0,
                linear_color_stop(white(), 0.0),
                linear_color_stop(white().opacity(0.0), 1.0),
            )))
            .child(div().absolute().size_full().bg(linear_gradient(
                180.0,
                linear_color_stop(black().opacity(0.0), 0.0),
                linear_color_stop(black(), 1.0),
            )))
            .child(marker(
                draft.saturation * PICKER_WIDTH,
                (1.0 - draft.value) * SHADE_HEIGHT,
                None,
            ))
            .child(self.drag_area(Held::Shade).absolute().size_full());
        let hue = div()
            .relative()
            .w(px(PICKER_WIDTH))
            .h(px(HUE_HEIGHT))
            .child(hue_strip().size_full().rounded_full().overflow_hidden())
            .child(marker(
                draft.hue / 360.0 * PICKER_WIDTH,
                HUE_HEIGHT / 2.0,
                Some(pure_hue),
            ))
            .child(self.drag_area(Held::Hue).absolute().size_full());
        let color = draft.color();
        let sample = {
            let (popup, value) = (self.popup.clone(), self.value);
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                close_popup(&popup, cx);
                on_sample(value, window, cx);
            }
        };
        let row = div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                dot(color, 28.0)
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(self.theme.hairline),
            )
            .child(div().text_xs().text_color(self.theme.text).child("Hex"))
            .child(self.hex_box(color, hex_focus, hex_draft, window, cx))
            .child(div().flex_1())
            .child(
                text_button(
                    format!("{}: from image", self.name),
                    "From image",
                    self.theme,
                )
                .aria_description("Pick a colour from the screenshot")
                .on_click(sample),
            );
        let popup = self.popup.clone();
        let back = text_button(
            format!("{}: palette", self.name),
            "\u{2190} Palette",
            self.theme,
        )
        .aria_label("Back to the palette")
        .on_click(move |_, _, cx| {
            popup.update(cx, |popup, cx| {
                *popup = Some(Popup::Palette);
                cx.notify();
            });
        })
        .into_any_element();
        let divider = div().w_full().h(px(1.0)).bg(self.theme.hairline);
        let mut parts = vec![
            shade.into_any_element(),
            hue.into_any_element(),
            row.into_any_element(),
            divider.into_any_element(),
        ];
        parts.extend(self.recent_row(cx, Some(back)));
        parts
    }

    /// The hex box: the draft's colour as `#RRGGBB`, and up to six hexadecimal digits typed
    /// over it, set by Enter.
    fn hex_box(
        &self,
        color: Color,
        focus: &FocusHandle,
        draft: &Entity<Option<String>>,
        window: &Window,
        cx: &App,
    ) -> impl IntoElement {
        let shown = draft
            .read(cx)
            .clone()
            .filter(|_| focus.is_focused(window))
            .map_or_else(|| color.hex(), |digits| format!("#{digits}"));
        let (on_key, on_out) = (draft.clone(), draft.clone());
        let (popup, on_set, value) = (self.popup.clone(), Rc::clone(&self.on_set), self.value);
        div()
            .id(ElementId::Name(format!("{}: hex", self.name).into()))
            .track_focus(focus)
            .tab_stop(true)
            .role(Role::TextInput)
            .aria_label("Hex colour")
            .aria_value(shown.clone())
            .h(px(26.0))
            .w(px(84.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .rounded(px(7.0))
            .border_1()
            .border_color(self.theme.hairline)
            .focus(|field| field.border_color(ACCENT))
            .text_xs()
            .text_color(self.theme.text)
            .on_mouse_down_out(move |_, _, cx| on_out.update(cx, |draft, _| *draft = None))
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                if event.keystroke.modifiers.modified() {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "enter" => {
                        let typed = on_key.update(cx, |draft, cx| {
                            cx.notify();
                            draft.take()
                        });
                        if let Some(color) = typed.as_deref().and_then(Color::from_hex_digits) {
                            popup.update(cx, |popup, cx| {
                                *popup = Some(Popup::Picker {
                                    draft: Hsv::of(color),
                                    held: None,
                                });
                                cx.notify();
                            });
                            remember_recent(color, cx);
                            on_set(value.property(color), window, cx);
                        }
                    }
                    "escape" if on_key.read(cx).is_some() => {
                        on_key.update(cx, |draft, cx| {
                            *draft = None;
                            cx.notify();
                        });
                    }
                    "backspace" => on_key.update(cx, |draft, cx| {
                        draft.get_or_insert_default().pop();
                        cx.notify();
                    }),
                    digit if digit.len() == 1 && digit.bytes().all(|b| b.is_ascii_hexdigit()) => {
                        on_key.update(cx, |draft, cx| {
                            let digits = draft.get_or_insert_default();
                            if digits.len() < 6 {
                                digits.push_str(&digit.to_ascii_uppercase());
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

    /// A press on `held` and the drag after it move the draft; the release sets its colour.
    fn drag_area(&self, held: Held) -> Canvas<Hitbox> {
        let (popup, on_set, value) = (self.popup.clone(), Rc::clone(&self.on_set), self.value);
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, _| {
                listen_on_drag(held, bounds, hitbox, &popup, &on_set, value, window);
            },
        )
    }
}

fn listen_on_drag(
    held: Held,
    bounds: Bounds<Pixels>,
    hitbox: Hitbox,
    popup: &Entity<Option<Popup>>,
    on_set: &OnColor,
    value: ColorValue,
    window: &mut Window,
) {
    let press = popup.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble
            && event.button == MouseButton::Left
            && hitbox.is_hovered(window)
        {
            press.update(cx, |popup, cx| {
                if let Some(Popup::Picker { draft, .. }) = *popup {
                    *popup = Some(Popup::Picker {
                        draft: moved(draft, held, bounds, event.position),
                        held: Some(held),
                    });
                    cx.notify();
                }
            });
            cx.stop_propagation();
        }
    });
    let drag = popup.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble && event.pressed_button == Some(MouseButton::Left) {
            drag.update(cx, |popup, cx| {
                if let Some(Popup::Picker {
                    draft,
                    held: Some(dragging),
                }) = *popup
                    && dragging == held
                {
                    *popup = Some(Popup::Picker {
                        draft: moved(draft, held, bounds, event.position),
                        held: Some(held),
                    });
                    cx.notify();
                }
            });
        }
    });
    let release = popup.clone();
    let on_set = Rc::clone(on_set);
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
            return;
        }
        let Some(Popup::Picker {
            draft,
            held: Some(dragging),
        }) = *release.read(cx)
        else {
            return;
        };
        if dragging != held {
            return;
        }
        release.update(cx, |popup, cx| {
            *popup = Some(Popup::Picker { draft, held: None });
            cx.notify();
        });
        let color = draft.color();
        remember_recent(color, cx);
        on_set(value.property(color), window, cx);
    });
}

/// `draft` with the part `held` controls set from `position` over `bounds`.
fn moved(draft: Hsv, held: Held, bounds: Bounds<Pixels>, position: Point<Pixels>) -> Hsv {
    let across = |offset: Pixels, extent: Pixels| (offset / extent).clamp(0.0, 1.0);
    let x = across(position.x - bounds.origin.x, bounds.size.width);
    let y = across(position.y - bounds.origin.y, bounds.size.height);
    match held {
        Held::Shade => Hsv {
            saturation: x,
            value: 1.0 - y,
            ..draft
        },
        Held::Hue => Hsv {
            hue: (x * 360.0).min(359.9),
            ..draft
        },
    }
}

fn close_popup(popup: &Entity<Option<Popup>>, cx: &mut App) {
    popup.update(cx, |popup, cx| {
        *popup = None;
        cx.notify();
    });
}

/// What a screen reader and the tooltip call `color`: its palette name, or its hex.
fn describe(color: Color) -> String {
    ColorValue::palette_name(color).map_or_else(|| color.hex(), str::to_owned)
}

fn dot(color: Color, side: f32) -> Div {
    div().size(px(side)).rounded_full().bg(rgba(color))
}

fn swatch_button(id: String, name: impl Into<SharedString>, theme: Theme) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .focusable()
        .tab_stop(true)
        .aria_label(name)
        .rounded_full()
        .focus_visible(move |swatch| swatch.border_2().border_color(ACCENT))
        .hover(move |swatch| swatch.border_color(theme.text))
}

fn text_button(id: String, text: &'static str, theme: Theme) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .focusable()
        .tab_stop(true)
        .role(Role::Button)
        .aria_label(text)
        .h(px(28.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .rounded(px(8.0))
        .bg(theme.hover)
        .text_xs()
        .text_color(theme.text)
        .focus_visible(|button| button.border_2().border_color(ACCENT))
        .hover(|button| button.opacity(0.85))
        .child(text)
}

/// The hues from red round to red, left to right.
fn hue_strip() -> Div {
    let at = |stop: u16| {
        Hsv {
            hue: 360.0 * f32::from(stop) / f32::from(HUE_STOPS),
            saturation: 1.0,
            value: 1.0,
        }
        .color()
    };
    div().flex().children((0..HUE_STOPS).map(|stop| {
        div().flex_1().h_full().bg(linear_gradient(
            90.0,
            linear_color_stop(rgba(at(stop)), 0.0),
            linear_color_stop(rgba(at(stop + 1)), 1.0),
        ))
    }))
}

/// The ring marking the picked point, `x` and `y` from the control's top-left corner.
fn marker(x: f32, y: f32, fill: Option<Color>) -> Div {
    div()
        .absolute()
        .left(px(x - MARKER / 2.0))
        .top(px(y - MARKER / 2.0))
        .size(px(MARKER))
        .rounded_full()
        .border_2()
        .border_color(white())
        .when_some(fill, |ring, fill| ring.bg(rgba(fill)))
        .shadow(vec![BoxShadow {
            color: hsla(0.0, 0.0, 0.0, 0.4),
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: px(1.0),
            inset: false,
        }])
}

#[cfg(test)]
mod tests {
    use gpui::size;

    use super::*;

    #[test]
    fn d147_a_colour_comes_back_from_the_picker_as_it_went_in() {
        for hex in [
            "17A2A8", "FF3B30", "000000", "FFFFFF", "8E8E93", "AF52DE", "123456",
        ] {
            let color = Color::from_hex_digits(hex).unwrap();
            assert_eq!(Hsv::of(color).color(), color, "{hex}");
        }
        let teal = Hsv::of(Color::from_hex_digits("17A2A8").unwrap());
        assert!((teal.hue - 182.5).abs() < 0.1, "{teal:?}");
    }

    #[test]
    fn d147_the_shade_square_sets_saturation_across_and_brightness_down_and_the_bar_sets_hue() {
        let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(200.0), px(100.0)));
        let draft = Hsv {
            hue: 120.0,
            saturation: 0.5,
            value: 0.5,
        };
        let shaded = moved(draft, Held::Shade, bounds, point(px(160.0), px(45.0)));
        assert_eq!(
            shaded,
            Hsv {
                hue: 120.0,
                saturation: 0.75,
                value: 0.75,
            }
        );
        let outside = moved(draft, Held::Shade, bounds, point(px(-5.0), px(500.0)));
        assert_eq!((outside.saturation, outside.value), (0.0, 0.0));
        let hued = moved(draft, Held::Hue, bounds, point(px(60.0), px(0.0)));
        assert_eq!(hued, Hsv { hue: 90.0, ..draft });
        assert!(moved(draft, Held::Hue, bounds, point(px(900.0), px(0.0))).hue < 360.0);
    }
}
