use std::borrow::Cow;
use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{Document, PointF, RectF, tooltip_text};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Axis, BorderStyle, Bounds, Canvas, ContentMask, DispatchPhase, Div, Hitbox,
    HitboxBehavior, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, RenderImage, Role, SharedString, Size, Stateful,
    StatefulInteractiveElement, Styled, Window, canvas, div, fill, hsla, point, px, quad, size,
    transparent_black,
};

use crate::Log;
use crate::canvas::Previews;
use crate::menu::RowState;
use crate::placement::{Fit, Placement};
use crate::theme::{CORNER, ControlFill, Theme, filled, panel};
use crate::tooltip::with_tooltip;

/// The most room the minimap takes: 184 px wide, and a height that keeps a tall
/// capture's panel clear of the caption row.
const MINIMAP_ROOM: Size<f32> = Size {
    width: 184.0,
    height: 120.0,
};
/// What one press of minus or plus zooms by.
const ZOOM_STEP: f32 = 1.25;
const SCROLLBAR: Pixels = px(6.0);
const SCROLLBAR_INSET: Pixels = px(3.0);

/// What the zoom panel asks of the view.
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
    /// A press on the scrollbar along `axis`, at `at` in the window.
    GrabBar {
        axis: Axis,
        at: Point<Pixels>,
    },
    /// The pointer moved to this window point with its button down.
    DragBar(Point<Pixels>),
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
    pub(crate) fn in_view(&self) -> RectF {
        self.placement.shows(self.room, self.area)
    }

    /// The canvas offset that pans the view as far as a scrollbar's thumb `moved` along
    /// `axis`, so the thumb stays under the pointer.
    pub(crate) fn bar_pan(&self, axis: Axis, moved: Point<Pixels>) -> Point<Pixels> {
        let scale = self.placement.scale;
        match axis {
            Axis::Horizontal => point(
                -moved.x * (self.area.width * scale / f32::from(self.room.width)),
                px(0.0),
            ),
            Axis::Vertical => point(
                px(0.0),
                -moved.y * (self.area.height * scale / f32::from(self.room.height)),
            ),
        }
    }

    /// Whether fitting would show the area at actual size, so Fit and Actual size zoom to the
    /// same scale.
    pub(crate) fn fit_is_actual_size(&self) -> bool {
        let fitted = Placement::fitted(self.room, self.area, self.scale_factor);
        fitted.scale.to_bits() == (1.0 / self.scale_factor).to_bits()
    }

    /// The zoom as the panel shows it: 100 is one image pixel on each device pixel.
    fn percent(&self) -> f32 {
        (self.placement.scale * self.scale_factor * 100.0).round()
    }
}

/// Image pixels to minimap pixels for `area`: as large as fits in [`MINIMAP_ROOM`].
fn minimap_scale(area: RectF) -> f32 {
    (MINIMAP_ROOM.width / area.width).min(MINIMAP_ROOM.height / area.height)
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

/// Image pixels to the screen for a minimap of `area` whose top-left corner is `origin`.
fn minimap_fit(area: RectF, image: RectF, origin: Point<Pixels>) -> Fit {
    let image_bounds = on_minimap(area, image, origin);
    Fit {
        origin: image_bounds.origin,
        scale: minimap_scale(area),
        image_bounds,
        shown: area,
    }
}

/// The bottom-right zoom panel for `viewport`: a minimap of `base` with the part in
/// view framed, which a press or drag pans, then minus, the zoom, plus and Fit.
pub(crate) fn zoom_panel(
    base: Arc<RenderImage>,
    document: Rc<Document>,
    previews: Rc<Previews>,
    viewport: Viewport,
    theme: Theme,
    log: Log,
    on_input: &OnPanelInput,
) -> impl IntoElement {
    let area = viewport.area;
    let minimap = on_minimap(area, area, point(px(0.0), px(0.0))).size;
    let zoom =
        |id: &'static str, label: String, name: SharedString, tip: Cow<'static, str>, input| {
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
                    minimap_canvas(
                        base,
                        document,
                        previews,
                        viewport,
                        theme,
                        log,
                        Rc::clone(on_input),
                    )
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
                .child(fit_button(viewport, theme, on_input)),
        )
}

/// Fit, or while fitting would only show the image at actual size, Fit dimmed with a tooltip
/// saying why.
fn fit_button(viewport: Viewport, theme: Theme, on_input: &OnPanelInput) -> Stateful<Div> {
    let fits = viewport.fit_is_actual_size();
    let tip = if fits {
        "Already fits".into()
    } else {
        tooltip_text("Fit to window", Some("Ctrl+0"))
    };
    let on_input = Rc::clone(on_input);
    panel_button("fit", "Fit to window".into(), tip, theme)
        .child("Fit")
        .px(px(10.0))
        .bg(theme.hover)
        .map(|button| {
            if fits {
                button.text_color(RowState::Dimmed.ink(theme))
            } else {
                button.on_click(move |_, window, cx| on_input(PanelInput::Fit, window, cx))
            }
        })
}

fn panel_button(
    id: &'static str,
    name: SharedString,
    tip: Cow<'static, str>,
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
        .rounded(CORNER)
        .map(|button| filled(button, ControlFill::Neutral, theme))
        .hover(move |button| button.bg(theme.hover))
}

/// The capture drawn small from `base`, the texture of `image`, with the part in view framed
/// and the rest shaded, reporting presses and drags on it to `on_input`.
fn minimap_canvas(
    base: Arc<RenderImage>,
    document: Rc<Document>,
    previews: Rc<Previews>,
    viewport: Viewport,
    theme: Theme,
    log: Log,
    on_input: OnPanelInput,
) -> Canvas<Hitbox> {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, _| {
            let area = viewport.area;
            let image = document.image_area();
            let mask = ContentMask { bounds };
            window.with_content_mask(Some(mask), |window| {
                let painted = window.paint_image(
                    bounds,
                    on_minimap(area, image, bounds.origin),
                    CORNER.into(),
                    base,
                    0,
                    false,
                );
                if painted.is_err() {
                    log("minimap not_painted");
                }
                previews.paint_redactions(
                    &document,
                    &minimap_fit(area, image, bounds.origin),
                    window,
                );
            });
            let frame = on_minimap(area, viewport.in_view(), bounds.origin);
            paint_shade_around(bounds, frame, window);
            window.paint_quad(quad(
                frame,
                px(0.0),
                transparent_black(),
                px(2.0),
                theme.frame,
                BorderStyle::Solid,
            ));
            listen_on_minimap(area, bounds.origin, hitbox, &on_input, window);
        },
    )
}

/// The minimap outside `frame` shaded.
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
    let grab = move |position| PanelInput::Grab(minimap_point(area, position - origin));
    let drag = move |position| PanelInput::Drag(minimap_point(area, position - origin));
    listen_for_drag(hitbox, on_input, window, grab, drag);
}

/// Thin bars on the card's right and bottom edges marking the part of the image in view, one
/// for each direction the image runs past the canvas; a drag on one pans that way.
pub(crate) fn scrollbars(viewport: Viewport, theme: Theme, on_input: &OnPanelInput) -> Vec<Div> {
    let (area, seen, room) = (viewport.area, viewport.in_view(), viewport.room);
    let thumb = |start: f32, seen: f32, whole: f32, length: Pixels| {
        (seen < whole).then(|| (length * (start / whole), length * (seen / whole)))
    };
    let bar = |axis: Axis| {
        let on_input = Rc::clone(on_input);
        div().rounded(px(3.0)).bg(theme.scroll_thumb).child(
            canvas(
                |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                move |_, hitbox, window, _| listen_on_bar(axis, hitbox, &on_input, window),
            )
            .size_full(),
        )
    };
    let vertical =
        thumb(seen.y - area.y, seen.height, area.height, room.height).map(|(top, height)| {
            bar(Axis::Vertical)
                .absolute()
                .right(SCROLLBAR_INSET)
                .top(top)
                .w(SCROLLBAR)
                .h(height)
        });
    let horizontal =
        thumb(seen.x - area.x, seen.width, area.width, room.width).map(|(left, width)| {
            bar(Axis::Horizontal)
                .absolute()
                .bottom(SCROLLBAR_INSET)
                .left(left)
                .h(SCROLLBAR)
                .w(width)
        });
    vertical.into_iter().chain(horizontal).collect()
}

fn listen_on_bar(axis: Axis, hitbox: Hitbox, on_input: &OnPanelInput, window: &mut Window) {
    let grab = move |at| PanelInput::GrabBar { axis, at };
    listen_for_drag(hitbox, on_input, window, grab, PanelInput::DragBar);
}

/// Tells `on_input` of a left-button drag begun on `hitbox`: `grab` makes the press's input and
/// `drag` each move's, from the pointer's position in the window.
fn listen_for_drag(
    hitbox: Hitbox,
    on_input: &OnPanelInput,
    window: &mut Window,
    grab: impl Fn(Point<Pixels>) -> PanelInput + 'static,
    drag: impl Fn(Point<Pixels>) -> PanelInput + 'static,
) {
    let on_press = Rc::clone(on_input);
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble
            && event.button == MouseButton::Left
            && hitbox.is_hovered(window)
        {
            on_press(grab(event.position), window, cx);
            cx.stop_propagation();
        }
    });
    let on_move = Rc::clone(on_input);
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && event.pressed_button == Some(MouseButton::Left) {
            on_move(drag(event.position), window, cx);
        }
    });
    let on_release = Rc::clone(on_input);
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
            on_release(PanelInput::Release, window, cx);
        }
    });
}

#[cfg(test)]
mod tests {
    use gpui::{WindowAppearance, point, px, size};

    use super::*;
    use crate::theme::WindowSurface;

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

    #[test]
    fn fit_is_actual_size_only_while_the_area_fits_at_one_image_pixel_per_device_pixel() {
        let viewport = |width, height, scale_factor| Viewport {
            area: RectF {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            placement: Placement {
                corner: point(px(0.0), px(0.0)),
                scale: 2.0,
            },
            room: ROOM,
            scale_factor,
        };
        assert!(viewport(800.0, 600.0, 1.0).fit_is_actual_size());
        assert!(viewport(320.0, 240.0, 1.0).fit_is_actual_size());
        assert!(!viewport(801.0, 600.0, 1.0).fit_is_actual_size());
        assert!(viewport(1100.0, 800.0, 1.5).fit_is_actual_size());
        assert!(!viewport(1300.0, 900.0, 1.5).fit_is_actual_size());
        assert!(!viewport(4000.0, 3000.0, 1.0).fit_is_actual_size());
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
    fn a_redaction_lands_on_the_minimap_where_its_image_pixels_do() {
        let area = RectF {
            x: -800.0,
            y: -400.0,
            width: 1472.0,
            height: 368.0,
        };
        let image = RectF {
            x: 0.0,
            y: 0.0,
            width: 400.0,
            height: 200.0,
        };
        let redaction = RectF {
            x: 100.0,
            y: 40.0,
            width: 64.0,
            height: 32.0,
        };
        let origin = point(px(10.0), px(20.0));
        assert_eq!(
            minimap_fit(area, image, origin).bounds(redaction),
            on_minimap(area, redaction, origin)
        );
    }

    #[test]
    fn the_minimap_frames_the_part_of_the_image_in_view() {
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
    fn a_press_on_the_minimap_pans_the_view_to_centre_on_that_point() {
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

    fn bars(viewport: Viewport) -> Vec<Div> {
        let ignored: OnPanelInput = Rc::new(|_, _, _| {});
        scrollbars(
            viewport,
            Theme::for_appearance(WindowAppearance::Dark, WindowSurface::Solid, None),
            &ignored,
        )
    }

    #[test]
    fn a_scrollbar_drag_pans_so_the_thumb_stays_under_the_pointer() {
        let viewport = zoomed_in();
        let thumb_left = |viewport: &Viewport| {
            let seen = viewport.in_view();
            f32::from(ROOM.width) * (seen.x - viewport.area.x) / viewport.area.width
        };
        let before = thumb_left(&viewport);
        let by = viewport.bar_pan(Axis::Horizontal, point(px(80.0), px(35.0)));
        assert_eq!(by.y, px(0.0));
        let dragged = Viewport {
            placement: viewport.placement.scrolled(by, ROOM, viewport.area),
            ..viewport
        };
        assert!((thumb_left(&dragged) - before - 80.0).abs() < 0.01);
        let down = viewport.bar_pan(Axis::Vertical, point(px(9.0), px(-60.0)));
        assert_eq!((down.x, down.y), (px(0.0), px(300.0)));
    }

    #[test]
    fn scrollbars_show_only_where_the_image_runs_past_the_canvas() {
        assert_eq!(bars(zoomed_in()).len(), 2);
        let wide = Viewport {
            placement: Placement {
                corner: point(px(-100.0), px(0.0)),
                scale: 0.2,
            },
            ..zoomed_in()
        };
        assert_eq!(bars(wide).len(), 1);
    }
}
