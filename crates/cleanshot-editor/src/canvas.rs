use std::borrow::Cow;
use std::f32::consts::TAU;
use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{
    Color, Command, Document, Fill, INTER, LINE_HEIGHT, PointF, RgbaImage, Shape, ShapeId, Style,
    TextBlock, arrow_head, handles,
};
use gpui::{
    App, BorderStyle, Bounds, Canvas, DispatchPhase, Font, FontWeight, Global, Hitbox,
    HitboxBehavior, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels,
    Point, RenderImage, Rgba, ScrollDelta, ScrollEvent, TextAlign, TextRun, Window, canvas, fill,
    font, point, px, quad, size, transparent_black,
};
use smallvec::smallvec;

use crate::Error;
use crate::placement::{Fit, Placement};
use crate::theme::ACCENT;

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
    Press {
        at: PointF,
        reach: f32,
        position: Point<Pixels>,
        placement: Placement,
    },
    Move {
        at: PointF,
        shift: bool,
        position: Point<Pixels>,
    },
    Release {
        shift: bool,
    },
    /// Ctrl+wheel over the canvas: scale `placement` by `factor` about `around` (F-39).
    Zoom {
        factor: f32,
        around: Point<Pixels>,
        placement: Placement,
    },
}

pub(crate) type OnPointer = Rc<dyn Fn(Pointer, &mut Window, &mut App)>;

/// What is drawn over the document: the edit a drag would make, and the selected shape's
/// handles.
pub(crate) struct Overlay {
    pub preview: Option<Command>,
    pub selected: Option<ShapeId>,
}

/// How far from a shape or handle, in screen pixels, a press still grips it.
const POINTER_REACH: f32 = 6.0;
const HANDLE_SIZE: f32 = 8.0;
/// D-46: each wheel line zooms by a fifth; a touchpad reports pixels, read as lines of 20.
const ZOOM_PER_WHEEL_LINE: f32 = 1.2;
const PIXELS_PER_WHEEL_LINE: f32 = 20.0;

/// Draws the document's base image scaled to fit the element, its shapes with `overlay`'s
/// preview on top, and reports the pointer to `on_pointer`, following a drag outside the element.
pub(crate) fn document_canvas(
    document: Rc<Document>,
    base: Arc<RenderImage>,
    placement: Option<Placement>,
    overlay: Overlay,
    on_pointer: OnPointer,
) -> Canvas<Hitbox> {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, cx| {
            let image = document.base();
            let placement = placement
                .unwrap_or_else(|| Placement::fitted(bounds.size, image, window.scale_factor()));
            let fit = Fit::new(bounds, image, placement);
            // Fails only if the GPU cannot allocate the atlas texture; the shapes still draw.
            let _ = window.paint_image(
                fit.image_bounds,
                fit.image_bounds,
                0.0.into(),
                base,
                0,
                false,
            );
            let (replaced, added) = match &overlay.preview {
                Some(Command::Replace { id, shape, style }) => (Some((*id, shape, style)), None),
                Some(Command::Add { shape, style }) => (None, Some((shape, style))),
                Some(Command::Remove(_) | Command::SetCrop(_)) | None => (None, None),
            };
            let mut selected_shape = None;
            for entry in document.shapes() {
                let (shape, style) = match replaced {
                    Some((id, shape, style)) if id == entry.id => (shape, style),
                    Some(_) | None => (&entry.shape, &entry.style),
                };
                paint_shape(shape, *style, &fit, window, cx);
                if overlay.selected == Some(entry.id) {
                    selected_shape = Some(shape);
                }
            }
            if let Some((shape, style)) = added {
                paint_shape(shape, *style, &fit, window, cx);
            }
            if let Some(Command::SetCrop(Some(crop))) = &overlay.preview {
                paint_outline(fit.bounds(*crop), window);
            }
            if let Some(shape) = selected_shape {
                for (_, at) in handles(shape) {
                    paint_handle(fit.point(at), window);
                }
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
            };
            on_press(press, window, cx);
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

fn paint_shape(shape: &Shape, style: Style, fit: &Fit, window: &mut Window, cx: &mut App) {
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
            paint_polygon(
                window,
                PathBuilder::stroke(width),
                &[fit.point(*from), fit.point(*to)],
                false,
                color,
            );
            let head = arrow_head(*from, *to, style.stroke_width).map(|p| fit.point(p));
            paint_polygon(window, stroke, &head, false, color);
        }
        Shape::Rect {
            rect,
            fill: Fill::Hollow,
        } => window.paint_quad(quad(
            fit.bounds(*rect),
            px(0.0),
            transparent_black(),
            width,
            color,
            BorderStyle::Solid,
        )),
        Shape::Rect {
            rect,
            fill: Fill::Solid,
        }
        | Shape::Highlight(rect) => window.paint_quad(fill(fit.bounds(*rect), color)),
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
            paint_polygon(window, stroke, &points, false, color);
        }
        Shape::Text(block) => paint_text(block, color, fit, window, cx),
        // The editor's redaction preview comes with its tool, after the M0 gate.
        Shape::Redact { .. } => {}
    }
}

/// The embedded Inter at the one weight F-34 draws text in.
fn inter() -> Font {
    Font {
        weight: FontWeight::SEMIBOLD,
        ..font("Inter")
    }
}

struct InterRegistered;

impl Global for InterRegistered {}

/// Hands core's Inter bytes to GPUI, once per app, so `inter()` finds them before any installed
/// font.
pub(crate) fn register_inter(cx: &mut App) -> Result<(), Error> {
    if cx.has_global::<InterRegistered>() {
        return Ok(());
    }
    cx.text_system()
        .add_fonts(vec![Cow::Borrowed(INTER)])
        .map_err(|error| Error::FontLoad(error.into()))?;
    cx.set_global(InterRegistered);
    Ok(())
}

fn paint_text(block: &TextBlock, color: Rgba, fit: &Fit, window: &mut Window, cx: &mut App) {
    let font_size = px(block.size() * fit.scale);
    let line_height = font_size * LINE_HEIGHT;
    for (line, baseline) in block.lines() {
        let run = TextRun {
            len: line.len(),
            font: inter(),
            color: color.into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped =
            window
                .text_system()
                .shape_line(line.to_owned().into(), font_size, &[run], None);
        let origin = line_origin(
            fit.point(baseline),
            line_height,
            shaped.ascent,
            shaped.descent,
        );
        // Fails only if a glyph cannot be rasterised; the document keeps the text.
        let _ = shaped.paint(origin, line_height, TextAlign::Left, None, window, cx);
    }
}

/// F-34: core places each baseline. GPUI paints a line's baseline
/// `(line_height - ascent - descent) / 2 + ascent` below the origin it is given, so this is the
/// origin that puts it on `baseline`.
fn line_origin(
    baseline: Point<Pixels>,
    line_height: Pixels,
    ascent: Pixels,
    descent: Pixels,
) -> Point<Pixels> {
    point(
        baseline.x,
        baseline.y - (line_height + ascent - descent) / 2.0,
    )
}

fn paint_polygon(
    window: &mut Window,
    mut builder: PathBuilder,
    points: &[Point<Pixels>],
    closed: bool,
    color: Rgba,
) {
    builder.add_polygon(points, closed);
    // lyon rejects a path with no length or too many vertices; the document keeps the shape.
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
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

fn rgba(color: Color) -> Rgba {
    Rgba {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: f32::from(color.a) / 255.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f34_a_line_painted_from_its_origin_has_its_baseline_where_core_put_it() {
        let baseline = point(px(40.0), px(100.0));
        let (line_height, ascent, descent) = (px(30.0), px(19.0), px(6.0));
        let origin = line_origin(baseline, line_height, ascent, descent);
        let gpui_baseline = origin.y + (line_height - ascent - descent) / 2.0 + ascent;
        assert_eq!(origin.x, baseline.x);
        assert_eq!(gpui_baseline, baseline.y);
        assert_eq!(origin.y, px(78.5));
    }

    #[test]
    fn f34_the_exports_gamma_correction_is_gpuis_for_the_runners_gamma() {
        assert_eq!(
            cleanshot_render::TEXT_GAMMA_RATIOS,
            gpui::get_gamma_correction_ratios(1.8)
        );
    }
}
