use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{PointF, RectF};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, BorderStyle, Bounds, Canvas, ContentMask, DispatchPhase, Div, Hitbox, HitboxBehavior,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, RenderImage, Role, SharedString, Size, Stateful,
    StatefulInteractiveElement, Styled, Window, canvas, div, fill, hsla, point, px, quad, size,
    transparent_black,
};

use crate::Log;
use crate::placement::Placement;
use crate::theme::{ACCENT, Theme};
use crate::toolbar::panel;
use crate::tooltip::{tooltip_text, with_tooltip};

/// The most room the minimap takes: the mock-up's 184 px width, and a height that keeps a tall
/// capture's panel clear of the caption row.
const MINIMAP_ROOM: Size<f32> = Size {
    width: 184.0,
    height: 120.0,
};
/// What one press of minus or plus zooms by.
const ZOOM_STEP: f32 = 1.25;
const SCROLLBAR: Pixels = px(6.0);
const SCROLLBAR_INSET: Pixels = px(3.0);

/// What the zoom panel asks of the view (D-145).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum PanelInput {
    /// Zoom by this factor about the middle of the canvas.
    ZoomBy(f32),
    ActualSize,
    Fit,
    /// A press on the minimap over this image point: the view centres there and follows the
    /// pointer until it is let go.
    Grab(PointF),
    /// The pointer moved over this image point with its button down.
    Drag(PointF),
    Release,
}

pub(crate) type OnPanelInput = Rc<dyn Fn(PanelInput, &mut Window, &mut App)>;

/// What the canvas shows: the `area` of the image it can show (the whole image or its crop),
/// the part of it in view at `placement` on a canvas of `room`, and the display's
/// `scale_factor`.
#[derive(Clone, Copy)]
pub(crate) struct Viewport {
    pub area: RectF,
    pub placement: Placement,
    pub room: Size<Pixels>,
    pub scale_factor: f32,
}

impl Viewport {
    /// The part of the area in view, in image pixels.
    pub fn in_view(&self) -> RectF {
        self.placement.shows(self.room, self.area)
    }

    /// The zoom as the panel shows it: 100 is one image pixel on each device pixel.
    fn percent(&self) -> f32 {
        (self.placement.scale * self.scale_factor * 100.0).round()
    }
}

/// Image pixels to minimap pixels for `area`: as large as fits in [`MINIMAP_ROOM`].
fn minimap_scale(area: RectF) -> f32 {
    MINIMAP_ROOM.width / area.width
}

/// The image point of `area` under `offset` from the minimap's top-left corner, kept inside
/// the area.
pub(crate) fn minimap_point(area: RectF, offset: Point<Pixels>) -> PointF {
    let scale = minimap_scale(area);
    PointF {
        x: (area.x + f32::from(offset.x) / scale).clamp(area.x, area.x + area.width),
        y: (area.y + f32::from(offset.y) / scale).clamp(area.y, area.y + area.height),
    }
}

/// `rect`, in image pixels of `area`, on a minimap whose top-left corner is `origin`.
pub(crate) fn on_minimap(area: RectF, rect: RectF, origin: Point<Pixels>) -> Bounds<Pixels> {
    let scale = minimap_scale(area);
    Bounds::new(
        origin + point(px((rect.x - area.x) * scale), px((rect.y - area.y) * scale)),
        size(px(rect.width * scale), px(rect.height * scale)),
    )
}

/// The bottom-right zoom panel for `viewport` (D-145): a minimap of `base` with the part in
/// view framed, which a press or drag pans, then minus, the zoom, plus and Fit.
pub(crate) fn zoom_panel(
    base: Arc<RenderImage>,
    image: RectF,
    viewport: Viewport,
    theme: Theme,
    log: Log,
    on_input: &OnPanelInput,
) -> impl IntoElement {
    let area = viewport.area;
    let minimap = on_minimap(area, area, point(px(0.0), px(0.0))).size;
    let zoom = |id: &'static str, label: String, name: SharedString, tip: SharedString, input| {
        let on_input = Rc::clone(on_input);
        panel_button(id, name, tip, theme)
            .child(label)
            .on_click(move |_, window, cx| on_input(input, window, cx))
    };
    let percent = viewport.percent();
    panel(theme)
        .absolute()
        .right(px(16.0))
        .bottom(px(16.0))
        .w(px(MINIMAP_ROOM.width + 12.0))
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .id("minimap")
                .role(Role::Image)
                .aria_label("Minimap")
                .w_full()
                .flex()
                .justify_center()
                .child(
                    minimap_canvas(base, image, viewport, log, Rc::clone(on_input))
                        .w(minimap.width)
                        .h(minimap.height),
                ),
        )
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(2.0))
                .text_size(px(12.0))
                .text_color(theme.text)
                .child(zoom(
                    "zoom-out",
                    "\u{2212}".to_owned(),
                    "Zoom out".into(),
                    "Zoom out".into(),
                    PanelInput::ZoomBy(1.0 / ZOOM_STEP),
                ))
                .child(
                    zoom(
                        "zoom-level",
                        format!("{percent}%"),
                        format!("Zoom {percent}%").into(),
                        tooltip_text("Actual size", Some("Ctrl+1")),
                        PanelInput::ActualSize,
                    )
                    .flex_1(),
                )
                .child(zoom(
                    "zoom-in",
                    "+".to_owned(),
                    "Zoom in".into(),
                    "Zoom in".into(),
                    PanelInput::ZoomBy(ZOOM_STEP),
                ))
                .child(
                    zoom(
                        "fit",
                        "Fit".to_owned(),
                        "Fit to window".into(),
                        tooltip_text("Fit to window", Some("Ctrl+0")),
                        PanelInput::Fit,
                    )
                    .px(px(10.0))
                    .bg(theme.hover),
                ),
        )
}

fn panel_button(
    id: &'static str,
    name: SharedString,
    tip: SharedString,
    theme: Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(name)
        .focusable()
        .tab_stop(true)
        .map(|element| with_tooltip(element, tip, theme))
        .h(px(24.0))
        .min_w(px(26.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.0))
        .focus_visible(|button| button.border_2().border_color(ACCENT))
        .hover(move |button| button.bg(theme.hover))
}

/// The capture drawn small from `base`, the texture of `image`, with the part in view framed
/// and the rest shaded, reporting presses and drags on it to `on_input`.
fn minimap_canvas(
    base: Arc<RenderImage>,
    image: RectF,
    viewport: Viewport,
    log: Log,
    on_input: OnPanelInput,
) -> Canvas<Hitbox> {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, _| {
            let area = viewport.area;
            let mask = ContentMask { bounds };
            window.with_content_mask(Some(mask), |window| {
                let painted = window.paint_image(
                    bounds,
                    on_minimap(area, image, bounds.origin),
                    px(7.0).into(),
                    base,
                    0,
                    false,
                );
                if painted.is_err() {
                    log("minimap not_painted");
                }
            });
            let frame = on_minimap(area, viewport.in_view(), bounds.origin);
            paint_shade_around(bounds, frame, window);
            window.paint_quad(quad(
                frame,
                px(0.0),
                transparent_black(),
                px(2.0),
                hsla(0.0, 0.0, 1.0, 1.0),
                BorderStyle::Solid,
            ));
            listen_on_minimap(area, bounds.origin, hitbox, &on_input, window);
        },
    )
}

/// The minimap outside `frame` shaded, as the mock-up's 28 % black.
fn paint_shade_around(minimap: Bounds<Pixels>, frame: Bounds<Pixels>, window: &mut Window) {
    let shade = hsla(0.0, 0.0, 0.0, 0.28);
    let (left, top) = (frame.left(), frame.top());
    let (right, bottom) = (frame.right(), frame.bottom());
    for strip in [
        Bounds::from_corners(minimap.origin, point(minimap.right(), top)),
        Bounds::from_corners(point(minimap.left(), bottom), minimap.bottom_right()),
        Bounds::from_corners(point(minimap.left(), top), point(left, bottom)),
        Bounds::from_corners(point(right, top), point(minimap.right(), bottom)),
    ] {
        if strip.size.width > px(0.0) && strip.size.height > px(0.0) {
            window.paint_quad(fill(strip, shade));
        }
    }
}

fn listen_on_minimap(
    area: RectF,
    origin: Point<Pixels>,
    hitbox: Hitbox,
    on_input: &OnPanelInput,
    window: &mut Window,
) {
    let on_press = Rc::clone(on_input);
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble
            && event.button == MouseButton::Left
            && hitbox.is_hovered(window)
        {
            let at = minimap_point(area, event.position - origin);
            on_press(PanelInput::Grab(at), window, cx);
            cx.stop_propagation();
        }
    });
    let on_move = Rc::clone(on_input);
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && event.pressed_button == Some(MouseButton::Left) {
            let at = minimap_point(area, event.position - origin);
            on_move(PanelInput::Drag(at), window, cx);
        }
    });
    let on_release = Rc::clone(on_input);
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
            on_release(PanelInput::Release, window, cx);
        }
    });
}

/// Thin bars on the card's right and bottom edges marking the part of the image in view, one
/// for each direction the image runs past the canvas (D-145).
pub(crate) fn scrollbars(viewport: Viewport) -> Vec<Div> {
    let (area, seen, room) = (viewport.area, viewport.in_view(), viewport.room);
    let thumb = |start: f32, seen: f32, whole: f32, length: Pixels| {
        (seen < whole).then(|| (length * (start / whole), length * (seen / whole)))
    };
    let bar = || {
        div()
            .absolute()
            .rounded(px(3.0))
            .bg(hsla(0.0, 0.0, 0.0, 0.3))
    };
    let vertical = thumb(seen.y - area.y, seen.height, area.height, room.height)
        .map(|(top, height)| bar().right(SCROLLBAR_INSET).top(top).w(SCROLLBAR).h(height));
    let horizontal =
        thumb(seen.x - area.x, seen.width, area.width, room.width).map(|(left, width)| {
            bar()
                .bottom(SCROLLBAR_INSET)
                .left(left)
                .h(SCROLLBAR)
                .w(width)
        });
    vertical.into_iter().chain(horizontal).collect()
}

#[cfg(test)]
mod tests {
    use gpui::{point, px, size};

    use super::*;

    const ROOM: Size<Pixels> = Size {
        width: px(800.0),
        height: px(600.0),
    };

    fn area() -> RectF {
        RectF {
            x: 0.0,
            y: 0.0,
            width: 4000.0,
            height: 3000.0,
        }
    }

    fn zoomed_in() -> Viewport {
        Viewport {
            area: area(),
            placement: Placement {
                corner: point(px(-1000.0), px(-500.0)),
                scale: 1.0,
            },
            room: ROOM,
            scale_factor: 1.0,
        }
    }

    #[test]
    fn d145_the_minimap_frames_the_part_of_the_image_in_view() {
        let viewport = zoomed_in();
        let seen = viewport.in_view();
        assert_eq!(
            (seen.x, seen.y, seen.width, seen.height),
            (1000.0, 500.0, 800.0, 600.0)
        );
        let frame = on_minimap(area(), seen, point(px(10.0), px(20.0)));
        assert_eq!(frame.origin, point(px(50.0), px(40.0)));
        assert_eq!(frame.size, size(px(32.0), px(24.0)));
        assert!((viewport.percent() - 100.0).abs() < f32::EPSILON);
    }

    #[test]
    fn d145_a_press_on_the_minimap_pans_the_view_to_centre_on_that_point() {
        let viewport = zoomed_in();
        let scale = minimap_scale(area());
        let pressed = point(px(3000.0 * scale), px(2000.0 * scale));
        let at = minimap_point(area(), pressed);
        assert!(
            (at.x - 3000.0).abs() < 0.5 && (at.y - 2000.0).abs() < 0.5,
            "{at:?}"
        );
        let panned = Viewport {
            placement: viewport.placement.centred_on(at, ROOM),
            ..viewport
        };
        let seen = panned.in_view();
        let middle = (seen.x + seen.width / 2.0, seen.y + seen.height / 2.0);
        assert!(
            (middle.0 - at.x).abs() < 0.5 && (middle.1 - at.y).abs() < 0.5,
            "{middle:?} is not {at:?}"
        );
        assert!((panned.placement.scale - 1.0).abs() < f32::EPSILON);
        let outside = minimap_point(area(), point(px(-50.0), px(1000.0)));
        assert_eq!((outside.x, outside.y), (0.0, 3000.0));
    }

    #[test]
    fn d145_scrollbars_show_only_where_the_image_runs_past_the_canvas() {
        assert_eq!(scrollbars(zoomed_in()).len(), 2);
        let wide = Viewport {
            placement: Placement {
                corner: point(px(-100.0), px(0.0)),
                scale: 0.2,
            },
            ..zoomed_in()
        };
        assert_eq!(scrollbars(wide).len(), 1);
    }
}
