use std::collections::HashMap;
use std::f32::consts::TAU;
use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{
    Bubble, Color, Command, Document, Fill, Frame, Label, PointF, RectF, RedactMode, RgbaImage,
    SPOTLIGHT_DIM, Shape, ShapeId, Style, arrow_head, dimmed_areas, handles, step_numbers, text_on,
};
use cleanshot_render::{Redacted, redacted};
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

/// What the canvas shows for each redaction: the pixels an export leaves there (F-36, D-58),
/// made once for each rectangle and mode rather than every frame.
#[derive(Clone, Default)]
pub(crate) struct RedactionPreviews(HashMap<RedactionKey, RedactionPreview>);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct RedactionKey {
    rect: [u32; 4],
    mode: RedactMode,
}

impl RedactionKey {
    fn new(rect: RectF, mode: RedactMode) -> Self {
        Self {
            rect: [rect.x, rect.y, rect.width, rect.height].map(f32::to_bits),
            mode,
        }
    }
}

#[derive(Clone)]
enum RedactionPreview {
    Pixels {
        bounds: RectF,
        image: Arc<RenderImage>,
    },
    CoversNoPixel,
    /// The preview could not be made, so the area is painted over to keep what is under it
    /// hidden.
    Failed,
}

impl RedactionPreviews {
    /// Keeps a preview for each redaction in `document`, making the ones it lacks and dropping
    /// the ones no longer there. A redaction still being dragged is not in the document, so it
    /// has none until release and paints as a solid fill (D-58).
    pub(crate) fn update(&mut self, document: &Document) {
        let mut kept = HashMap::new();
        for entry in document.shapes() {
            let Shape::Redact { rect, mode } = entry.shape else {
                continue;
            };
            let key = RedactionKey::new(rect, mode);
            if kept.contains_key(&key) {
                continue;
            }
            let preview = self
                .0
                .remove(&key)
                .unwrap_or_else(|| redaction_preview(document.base(), rect, mode));
            kept.insert(key, preview);
        }
        self.0 = kept;
    }

    /// Paints the redaction at `rect` and returns where it painted, `None` if nowhere.
    fn paint(
        &self,
        rect: RectF,
        mode: RedactMode,
        fit: &Fit,
        window: &mut Window,
    ) -> Option<Bounds<Pixels>> {
        let hide = |window: &mut Window| {
            let bounds = fit.bounds(rect);
            window.paint_quad(fill(bounds, gpui::black()));
            Some(bounds)
        };
        match self.0.get(&RedactionKey::new(rect, mode)) {
            Some(RedactionPreview::Pixels { bounds, image }) => {
                let bounds = fit.bounds(*bounds);
                match window.paint_image(bounds, bounds, 0.0.into(), Arc::clone(image), 0, false) {
                    Ok(()) => Some(bounds),
                    Err(error) => {
                        eprintln!("the redaction preview could not be drawn: {error:#}");
                        hide(window)
                    }
                }
            }
            Some(RedactionPreview::CoversNoPixel) => None,
            Some(RedactionPreview::Failed) | None => hide(window),
        }
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "image sides are far below 2^24, where f32 stops being exact"
)]
fn redaction_preview(image: &RgbaImage, rect: RectF, mode: RedactMode) -> RedactionPreview {
    match redacted(image, rect, mode) {
        Ok(Some(Redacted { left, top, pixels })) => RedactionPreview::Pixels {
            bounds: RectF {
                x: left as f32,
                y: top as f32,
                width: pixels.width() as f32,
                height: pixels.height() as f32,
            },
            image: render_image(&pixels),
        },
        Ok(None) => RedactionPreview::CoversNoPixel,
        Err(error) => {
            eprintln!("the redaction preview could not be made: {error}");
            RedactionPreview::Failed
        }
    }
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

pub(crate) type PaintOver = Box<dyn FnOnce(&Fit, &mut Window, &mut App)>;

/// What is drawn over the document: the edit a drag or the text box would make, the selected
/// shape's handles, and last `over`, given where the image is (the text box's caret and input).
pub(crate) struct Overlay {
    pub preview: Option<Command>,
    pub selected: Option<ShapeId>,
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
pub(crate) fn document_canvas(
    document: Rc<Document>,
    base: Arc<RenderImage>,
    placement: Option<Placement>,
    redactions: Rc<RedactionPreviews>,
    overlay: Overlay,
    on_pointer: OnPointer,
) -> Canvas<Hitbox> {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, cx| {
            let image = document.base();
            let scale_factor = window.scale_factor();
            let placement =
                placement.unwrap_or_else(|| Placement::fitted(bounds.size, image, scale_factor));
            let fit = Fit::new(bounds, image, placement, scale_factor);
            paint_frame(document.frame(), fit.image_bounds, fit.scale, window);
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
                Some(Command::Remove(_) | Command::SetCrop(_) | Command::SetFrame(_)) | None => {
                    (None, None)
                }
            };
            let shown: Vec<_> = document
                .shapes()
                .iter()
                .map(|entry| match replaced {
                    Some((id, shape, style)) if id == entry.id => (Some(entry.id), shape, style),
                    Some(_) | None => (Some(entry.id), &entry.shape, &entry.style),
                })
                .chain(added.map(|(shape, style)| (None, shape, style)))
                .collect();
            let dimmed = dimmed_areas(image, shown.iter().map(|(_, shape, _)| *shape));
            paint_dim(&dimmed, fit.image_bounds, &fit, window);
            let mut selected_shape = None;
            let numbers =
                step_numbers(shown.iter().map(|(_, shape, _)| *shape)).collect::<Vec<_>>();
            for ((id, shape, style), number) in shown.into_iter().zip(numbers) {
                let painting = Painting {
                    fit: &fit,
                    redactions: &redactions,
                    dimmed: &dimmed,
                    number,
                };
                paint_shape(shape, *style, &painting, window, cx);
                if id.is_some() && overlay.selected == id {
                    selected_shape = Some(shape);
                }
            }
            if let Some(Command::SetCrop(Some(crop))) = &overlay.preview {
                paint_outline(fit.bounds(*crop), window);
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
    dimmed: &'a [RectF],
    number: usize,
}

fn paint_shape(
    shape: &Shape,
    style: Style,
    painting: &Painting,
    window: &mut Window,
    cx: &mut App,
) {
    let Painting {
        fit,
        redactions,
        dimmed,
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
        Shape::Text(block) => paint_label(&Label::Block(block), color, fit, window, cx),
        Shape::Redact { rect, mode } => {
            // D-78: a redaction is the capture with its pixels destroyed, so it dims like the capture.
            if let Some(painted) = redactions.paint(*rect, *mode, fit, window) {
                paint_dim(dimmed, painted, fit, window);
            }
        }
        // F-43: the canvas dims around every spotlight before it paints any shape.
        Shape::Spotlight(_) => {}
        Shape::Step(step) => {
            paint_polygon(
                window,
                PathBuilder::fill(),
                &ellipse(fit.bounds(step.circle())),
                true,
                color,
            );
            paint_label(
                &step.label(number),
                rgba(text_on(style.color)),
                fit,
                window,
                cx,
            );
        }
        Shape::Bubble(bubble) => paint_bubble(bubble, style, fit, window, cx),
    }
}

/// F-42: the rounded box and tail in the style colour, the text on them in the colour that reads.
fn paint_bubble(bubble: &Bubble, style: Style, fit: &Fit, window: &mut Window, cx: &mut App) {
    let color = rgba(style.color);
    window.paint_quad(quad(
        fit.bounds(bubble.outline()),
        px(bubble.corner_radius() * fit.scale),
        color,
        px(0.0),
        transparent_black(),
        BorderStyle::Solid,
    ));
    if let Some(tail) = bubble.tail_triangle() {
        let tail = tail.map(|point| fit.point(point));
        paint_polygon(window, PathBuilder::fill(), &tail, true, color);
    }
    paint_label(
        &Label::Block(bubble.block()),
        rgba(text_on(style.color)),
        fit,
        window,
        cx,
    );
}

/// Lays the spotlight dim over each of `areas`, cut to `within` (F-43, D-78).
fn paint_dim(areas: &[RectF], within: Bounds<Pixels>, fit: &Fit, window: &mut Window) {
    for area in areas {
        let bounds = fit.bounds(*area).intersect(&within);
        if !bounds.is_empty() {
            window.paint_quad(fill(bounds, rgba(SPOTLIGHT_DIM)));
        }
    }
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

pub(crate) fn rgba(color: Color) -> Rgba {
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
    use cleanshot_core::History;

    fn image_of(previews: &RedactionPreviews, rect: RectF, mode: RedactMode) -> Arc<RenderImage> {
        match previews.0.get(&RedactionKey::new(rect, mode)) {
            Some(RedactionPreview::Pixels { image, .. }) => Arc::clone(image),
            Some(RedactionPreview::CoversNoPixel | RedactionPreview::Failed) | None => {
                panic!("a preview of {rect:?}")
            }
        }
    }

    #[test]
    fn f36_a_redaction_preview_is_made_once_and_dropped_with_its_redaction() {
        let image = RgbaImage::new(64, 48, vec![200; 64 * 48 * 4]).unwrap();
        let rect = RectF {
            x: 3.0,
            y: 2.0,
            width: 20.0,
            height: 10.0,
        };
        let mode = RedactMode::Blur;
        let add = || Command::Add {
            shape: Shape::Redact { rect, mode },
            style: Style {
                color: Color {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                },
                stroke_width: 1.0,
            },
        };
        let mut history = History::new(Document::new(Arc::new(image)));
        history.apply(add()).unwrap();
        history.apply(add()).unwrap();
        let mut previews = RedactionPreviews::default();
        previews.update(history.document());
        let first = image_of(&previews, rect, mode);
        history.undo().unwrap();
        previews.update(history.document());
        assert!(Arc::ptr_eq(&first, &image_of(&previews, rect, mode)));
        history.undo().unwrap();
        previews.update(history.document());
        assert!(previews.0.is_empty());
    }
}
