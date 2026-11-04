use std::f32::consts::TAU;
use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{
    Bubble, Color, Command, Document, Fill, Frame, Label, PointF, RectF, RgbaImage, Shape, ShapeId,
    SpotlightDim, Style, areas_outside, arrow_head, box_handles, dimmed_areas, handles,
    step_numbers, text_on,
};
use gpui::{
    App, BorderStyle, Bounds, BoxShadow, Canvas, Corners, DispatchPhase, Hitbox, HitboxBehavior,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point,
    RenderImage, Rgba, ScrollDelta, ScrollEvent, Window, canvas, fill, hsla, point, px, quad, size,
    transparent_black,
};
use smallvec::smallvec;

use crate::placement::{Fit, Placement};
use crate::text::paint_label;
use crate::theme::ACCENT;

mod previews;

pub(crate) use previews::Previews;
use previews::{RedactionPreviews, ShadowPreviews, paint_shadow};

const ELLIPSE_SEGMENTS: u16 = 64;

/// The base image in the BGRA layout GPUI uploads, made once per window.
pub(crate) fn render_image(image: &RgbaImage) -> Arc<RenderImage> {
    let mut buffer = image::RgbaImage::new(image.width(), image.height());
    let bgra_pixels = buffer.as_chunks_mut::<4>().0.iter_mut();
    for (bgra, rgba) in bgra_pixels.zip(image.pixels().as_chunks::<4>().0) {
        *bgra = [rgba[2], rgba[1], rgba[0], rgba[3]];
    }
    Arc::new(RenderImage::new(smallvec![image::Frame::new(buffer)]))
}

/// The pointer on the canvas, in image pixels, clamped to the image.
#[derive(Clone, Copy)]
pub(crate) enum Pointer {
    /// The left button went down over the canvas. `reach` is how far from `at`, in image
    /// pixels, a press still grips a shape or handle.
    /// `position` is on the canvas, and `placement` is where the image was when it was pressed.
    /// `double_click` is a second press within the Windows double-click time and distance.
    Press {
        at: PointF,
        reach: f32,
        position: Point<Pixels>,
        placement: Placement,
        double_click: bool,
    },
    Move {
        at: PointF,
        shift: bool,
        position: Point<Pixels>,
    },
    Release {
        shift: bool,
    },
    /// The right button went down over the canvas at `at`, `position` in the window (D-115).
    Menu {
        at: PointF,
        reach: f32,
        position: Point<Pixels>,
    },
    /// Ctrl+wheel over the canvas: scale `placement` by `factor` about `around` (F-39).
    Zoom {
        factor: f32,
        around: Point<Pixels>,
        placement: Placement,
    },
}

pub(crate) type OnPointer = Rc<dyn Fn(Pointer, &mut Window, &mut App)>;

pub(crate) type PaintOver = Box<dyn FnOnce(&Fit, &mut Window, &mut App)>;

/// What is drawn over the document: the edit a drag or the text box would make, the selected
/// shape's handles, the crop's handles while `crop_handles` (D-118), and last `over`, given
/// where the image is (the text box's caret and input).
pub(crate) struct Overlay {
    pub preview: Option<Command>,
    pub selected: Option<ShapeId>,
    pub crop_handles: bool,
    pub over: Option<PaintOver>,
}

/// How far from a shape or handle, in screen pixels, a press still grips it.
const POINTER_REACH: f32 = 6.0;
const HANDLE_SIZE: f32 = 8.0;
/// D-46: each wheel line zooms by a fifth; a touchpad reports pixels, read as lines of 20.
const ZOOM_PER_WHEEL_LINE: f32 = 1.2;
const PIXELS_PER_WHEEL_LINE: f32 = 20.0;

/// Draws the document's base image scaled to fit the element, its shapes with `overlay`'s
/// preview on top, and reports the pointer to `on_pointer`, following a drag outside the element.
/// `document`'s shapes in drawing order as the canvas shows them: the one `preview` changes in
/// its changed form, and the one it adds last, without an id.
fn shown_shapes<'a>(
    document: &'a Document,
    preview: Option<&'a Command>,
) -> Vec<(Option<ShapeId>, &'a Shape, &'a Style)> {
    let (replaced, added) = match preview {
        Some(Command::Replace { id, shape, style }) => (Some((*id, shape, style)), None),
        Some(Command::Add { shape, style }) => (None, Some((shape, style))),
        Some(
            Command::Remove(_)
            | Command::Restack { .. }
            | Command::SetCrop(_)
            | Command::SetFrame(_)
            | Command::SetDim(_),
        )
        | None => (None, None),
    };
    document
        .shapes()
        .iter()
        .map(|entry| match replaced {
            Some((id, shape, style)) if id == entry.id => (Some(entry.id), shape, style),
            Some(_) | None => (Some(entry.id), &entry.shape, &entry.style),
        })
        .chain(added.map(|(shape, style)| (None, shape, style)))
        .collect()
}

pub(crate) fn document_canvas(
    document: Rc<Document>,
    base: Arc<RenderImage>,
    placement: Option<Placement>,
    previews: Rc<Previews>,
    overlay: Overlay,
    on_pointer: OnPointer,
) -> Canvas<Hitbox> {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, cx| {
            let image = document.base();
            let scale_factor = window.scale_factor();
            // D-118: while the crop is being changed, the view shows what it can grow into.
            let shown = if overlay.crop_handles {
                document.image_area()
            } else {
                document.kept_area()
            };
            let placement =
                placement.unwrap_or_else(|| Placement::fitted(bounds.size, shown, scale_factor));
            let fit = Fit::new(bounds, image, placement, scale_factor);
            paint_frame(document.frame(), fit.image_bounds, fit.scale, window);
            let painted = window.paint_image(
                fit.image_bounds,
                fit.image_bounds,
                0.0.into(),
                base,
                0,
                false,
            );
            if painted.is_err() {
                (previews.log)("base_image not_painted");
            }
            let shown = shown_shapes(&document, overlay.preview.as_ref());
            previews.paint_highlights(
                shown.iter().map(|(_, shape, style)| (*shape, *style)),
                &fit,
                window,
            );
            let dimmed = dimmed_areas(image, shown.iter().map(|(_, shape, _)| *shape));
            let dim = document.dim().color();
            paint_dim(&dimmed, dim, fit.image_bounds, &fit, window);
            let mut selected_shape = None;
            let numbers =
                step_numbers(shown.iter().map(|(_, shape, _)| *shape)).collect::<Vec<_>>();
            for ((id, shape, style), number) in shown.into_iter().zip(numbers) {
                let painting = Painting {
                    fit: &fit,
                    redactions: &previews.redactions,
                    shadows: &previews.shadows,
                    dimmed: &dimmed,
                    dim,
                    number,
                };
                let shadow = paint_shadow(id, shape, *style, &painting, window, cx);
                let painted = paint_shape(shape, *style, &painting, window, cx);
                for unpainted in [shadow, painted].into_iter().filter_map(Result::err) {
                    (previews.log)(unpainted.event());
                }
                if id.is_some() && overlay.selected == id {
                    selected_shape = Some(shape);
                }
            }
            if let Some(crop) = document.crop() {
                paint_dim(
                    &areas_outside(image, &[crop]),
                    OUTSIDE_CROP,
                    fit.image_bounds,
                    &fit,
                    window,
                );
            }
            let new_crop = if let Some(Command::SetCrop(Some(crop))) = &overlay.preview {
                paint_outline(fit.bounds(*crop), window);
                Some(*crop)
            } else {
                None
            };
            if overlay.crop_handles
                && let Some(crop) = new_crop.or(document.crop())
            {
                for (_, at) in box_handles(crop) {
                    paint_handle(fit.point(at), window);
                }
            }
            if let Some(shape) = selected_shape {
                for (_, at) in handles(shape) {
                    paint_handle(fit.point(at), window);
                }
            }
            if let Some(over) = overlay.over {
                over(&fit, window, cx);
            }
            listen_for_pointer(&fit, placement, bounds.origin, hitbox, &on_pointer, window);
        },
    )
}

fn listen_for_pointer(
    fit: &Fit,
    placement: Placement,
    canvas_origin: Point<Pixels>,
    hitbox: Hitbox,
    on_pointer: &OnPointer,
    window: &mut Window,
) {
    let fit = *fit;
    let image_point = move |position| fit.image_point(position);
    let reach = POINTER_REACH / fit.scale;
    let zoom_hitbox = hitbox.clone();
    let on_press = Rc::clone(on_pointer);
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble
            && event.button == MouseButton::Left
            && hitbox.is_hovered(window)
        {
            let at = image_point(event.position);
            let position = event.position - canvas_origin;
            let press = Pointer::Press {
                at,
                reach,
                position,
                placement,
                double_click: event.click_count > 1,
            };
            on_press(press, window, cx);
        }
    });
    let on_menu = Rc::clone(on_pointer);
    let menu_hitbox = zoom_hitbox.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble
            && event.button == MouseButton::Right
            && menu_hitbox.is_hovered(window)
        {
            let menu = Pointer::Menu {
                at: image_point(event.position),
                reach,
                position: event.position,
            };
            on_menu(menu, window, cx);
        }
    });
    let on_move = Rc::clone(on_pointer);
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && event.pressed_button == Some(MouseButton::Left) {
            let at = image_point(event.position);
            let shift = event.modifiers.shift;
            let position = event.position - canvas_origin;
            on_move(
                Pointer::Move {
                    at,
                    shift,
                    position,
                },
                window,
                cx,
            );
        }
    });
    let on_release = Rc::clone(on_pointer);
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
            let shift = event.modifiers.shift;
            on_release(Pointer::Release { shift }, window, cx);
        }
    });
    let on_zoom = Rc::clone(on_pointer);
    window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble
            && event.modifiers.control
            && zoom_hitbox.is_hovered(window)
        {
            let lines = match event.delta {
                ScrollDelta::Lines(lines) => lines.y,
                ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / PIXELS_PER_WHEEL_LINE,
            };
            let zoom = Pointer::Zoom {
                factor: ZOOM_PER_WHEEL_LINE.powf(lines),
                around: event.position - canvas_origin,
                placement,
            };
            on_zoom(zoom, window, cx);
        }
    });
}

/// F-44: the frame the export draws around the image, at the image's scale, so the canvas shows
/// how the export will look. It frames the whole image, as the canvas shows it uncropped.
#[expect(
    clippy::cast_precision_loss,
    reason = "the frame's sizes are a few pixels, exact in f32"
)]
fn paint_frame(frame: Frame, image: Bounds<Pixels>, scale: f32, window: &mut Window) {
    let border = if frame.border { scale } else { 0.0 };
    let block = image.dilate(px(border));
    if frame.shadow {
        window.paint_drop_shadows(
            block,
            Corners::default(),
            &[BoxShadow {
                color: hsla(0.0, 0.0, 0.0, Frame::SHADOW_ALPHA),
                offset: point(px(0.0), px(Frame::SHADOW_OFFSET as f32 * scale)),
                blur_radius: px(Frame::SHADOW_BLUR as f32 * scale),
                spread_radius: px(0.0),
                inset: false,
            }],
        );
    }
    if frame.border {
        window.paint_quad(quad(
            block,
            px(0.0),
            transparent_black(),
            px(border),
            hsla(0.0, 0.0, 0.0, f32::from(Frame::BORDER_ALPHA) / 255.0),
            BorderStyle::Solid,
        ));
    }
}

fn paint_outline(bounds: Bounds<Pixels>, window: &mut Window) {
    window.paint_quad(quad(
        bounds,
        px(0.0),
        transparent_black(),
        px(1.0),
        ACCENT,
        BorderStyle::Dashed,
    ));
}

fn paint_handle(centre: Point<Pixels>, window: &mut Window) {
    let side = px(HANDLE_SIZE);
    let bounds = Bounds::centered_at(centre, size(side, side));
    window.paint_quad(quad(
        bounds,
        px(2.0),
        gpui::white(),
        px(1.0),
        ACCENT,
        BorderStyle::Solid,
    ));
}

/// What a shape is painted against: the fit, the redaction previews, the spotlights' dimmed
/// areas, and the number a step at this place in the drawing order shows.
struct Painting<'a> {
    fit: &'a Fit,
    redactions: &'a RedactionPreviews,
    shadows: &'a ShadowPreviews,
    dimmed: &'a [RectF],
    dim: Color,
    number: usize,
}

fn paint_shape(
    shape: &Shape,
    style: Style,
    painting: &Painting,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), Unpainted> {
    let Painting {
        fit,
        redactions,
        shadows: _,
        dimmed,
        dim,
        number,
    } = *painting;
    let color = rgba(style.color);
    let width = px(style.stroke_width * fit.scale);
    let stroke = PathBuilder::stroke(width);
    match shape {
        Shape::Line { from, to } => paint_polygon(
            window,
            stroke,
            &[fit.point(*from), fit.point(*to)],
            false,
            color,
        ),
        Shape::Arrow { from, to } => {
            let line = paint_polygon(
                window,
                PathBuilder::stroke(width),
                &[fit.point(*from), fit.point(*to)],
                false,
                color,
            );
            let head = arrow_head(*from, *to, style.stroke_width).map(|p| fit.point(p));
            line.and(paint_polygon(window, stroke, &head, false, color))
        }
        Shape::Rect {
            rect,
            fill: Fill::Hollow,
        } => {
            window.paint_quad(quad(
                fit.bounds(*rect),
                px(0.0),
                transparent_black(),
                width,
                color,
                BorderStyle::Solid,
            ));
            Ok(())
        }
        Shape::Rect {
            rect,
            fill: Fill::Solid,
        } => {
            window.paint_quad(fill(fit.bounds(*rect), color));
            Ok(())
        }
        Shape::Ellipse {
            rect,
            fill: Fill::Hollow,
        } => paint_polygon(window, stroke, &ellipse(fit.bounds(*rect)), true, color),
        Shape::Ellipse {
            rect,
            fill: Fill::Solid,
        } => paint_polygon(
            window,
            PathBuilder::fill(),
            &ellipse(fit.bounds(*rect)),
            true,
            color,
        ),
        Shape::Pen(points) => {
            let points: Vec<_> = points.iter().map(|p| fit.point(*p)).collect();
            paint_polygon(window, stroke, &points, false, color)
        }
        Shape::Text(block) => paint_label(&Label::Block(block), color, fit, window, cx),
        Shape::Redact {
            rect,
            mode,
            strength,
        } => {
            // D-78: a redaction is the capture with its pixels destroyed, so it dims like the capture.
            if let Some(painted) = redactions.paint(*rect, *mode, *strength, fit, window) {
                paint_dim(dimmed, dim, painted, fit, window);
            }
            Ok(())
        }
        // F-43, D-125: the canvas dims around every spotlight, and paints every highlight, before
        // it paints any shape.
        Shape::Spotlight(_) | Shape::Highlight { .. } => Ok(()),
        Shape::Step(step) => {
            let circle = paint_polygon(
                window,
                PathBuilder::fill(),
                &ellipse(fit.bounds(step.circle())),
                true,
                color,
            );
            circle.and(paint_label(
                &step.label(number),
                rgba(text_on(style.color)),
                fit,
                window,
                cx,
            ))
        }
        Shape::Bubble(bubble) => paint_bubble(bubble, style, fit, window, cx),
    }
}

/// F-42: the rounded box and tail in the style colour, the text on them in the colour that reads.
fn paint_bubble(
    bubble: &Bubble,
    style: Style,
    fit: &Fit,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), Unpainted> {
    let color = rgba(style.color);
    window.paint_quad(quad(
        fit.bounds(bubble.outline()),
        px(bubble.corner_radius() * fit.scale),
        color,
        px(0.0),
        transparent_black(),
        BorderStyle::Solid,
    ));
    let tail = bubble.tail_triangle().map_or(Ok(()), |tail| {
        let tail = tail.map(|point| fit.point(point));
        paint_polygon(window, PathBuilder::fill(), &tail, true, color)
    });
    let text = paint_label(
        &Label::Block(bubble.block()),
        rgba(text_on(style.color)),
        fit,
        window,
        cx,
    );
    tail.and(text)
}

/// Lays the spotlight dim over each of `areas`, cut to `within` (F-43, D-78).
/// What the canvas lays over the image outside the crop while it is being changed (F-37).
const OUTSIDE_CROP: Color = SpotlightDim::Medium.color();

fn paint_dim(
    areas: &[RectF],
    color: Color,
    within: Bounds<Pixels>,
    fit: &Fit,
    window: &mut Window,
) {
    for area in areas {
        let bounds = fit.bounds(*area).intersect(&within);
        if !bounds.is_empty() {
            window.paint_quad(fill(bounds, rgba(color)));
        }
    }
}

/// What the canvas could not paint this frame (D-120). The document keeps the shape, so the
/// export still has it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Unpainted {
    /// lyon rejected the path: it has no length or too many vertices.
    Path,
    /// A line of text could not be rasterised.
    Glyphs,
}

impl Unpainted {
    fn event(self) -> &'static str {
        match self {
            Self::Path => "path not_painted",
            Self::Glyphs => "glyphs not_painted",
        }
    }
}

fn paint_polygon(
    window: &mut Window,
    mut builder: PathBuilder,
    points: &[Point<Pixels>],
    closed: bool,
    color: Rgba,
) -> Result<(), Unpainted> {
    builder.add_polygon(points, closed);
    let path = builder.build().map_err(|_| Unpainted::Path)?;
    window.paint_path(path, color);
    Ok(())
}

fn ellipse(bounds: Bounds<Pixels>) -> Vec<Point<Pixels>> {
    let centre = bounds.center();
    let radius_x = f32::from(bounds.size.width) / 2.0;
    let radius_y = f32::from(bounds.size.height) / 2.0;
    (0..ELLIPSE_SEGMENTS)
        .map(|segment| {
            let angle = TAU * f32::from(segment) / f32::from(ELLIPSE_SEGMENTS);
            centre + point(px(radius_x * angle.cos()), px(radius_y * angle.sin()))
        })
        .collect()
}

pub(crate) fn rgba(color: Color) -> Rgba {
    Rgba {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: f32::from(color.a) / 255.0,
    }
}
