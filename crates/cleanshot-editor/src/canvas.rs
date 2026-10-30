use std::f32::consts::TAU;
use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{
    Color, Command, Document, Fill, PointF, RectF, RgbaImage, Shape, ShapeId, Style, arrow_head,
    handles,
};
use gpui::{
    App, BorderStyle, Bounds, Canvas, DispatchPhase, Hitbox, HitboxBehavior, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point, RenderImage, Rgba,
    Window, canvas, fill, point, px, quad, size, transparent_black,
};
use smallvec::smallvec;

use crate::toolbar::ACCENT;

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
    Press {
        at: PointF,
        reach: f32,
    },
    Move {
        at: PointF,
        shift: bool,
    },
    Release {
        shift: bool,
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

/// Draws the document's base image scaled to fit the element, its shapes with `overlay`'s
/// preview on top, and reports the pointer to `on_pointer`, following a drag outside the element.
pub(crate) fn document_canvas(
    document: Rc<Document>,
    base: Arc<RenderImage>,
    overlay: Overlay,
    on_pointer: OnPointer,
) -> Canvas<Hitbox> {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, _| {
            let fit = Fit::new(bounds, &document, window.scale_factor());
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
                paint_shape(shape, *style, &fit, window);
                if overlay.selected == Some(entry.id) {
                    selected_shape = Some(shape);
                }
            }
            if let Some((shape, style)) = added {
                paint_shape(shape, *style, &fit, window);
            }
            if let Some(Command::SetCrop(Some(crop))) = &overlay.preview {
                paint_outline(fit.bounds(*crop), window);
            }
            if let Some(shape) = selected_shape {
                for (_, at) in handles(shape) {
                    paint_handle(fit.point(at), window);
                }
            }
            listen_for_pointer(&fit, hitbox, &on_pointer, window);
        },
    )
}

fn listen_for_pointer(fit: &Fit, hitbox: Hitbox, on_pointer: &OnPointer, window: &mut Window) {
    let fit = *fit;
    let image_point = move |position| fit.image_point(position);
    let reach = POINTER_REACH / fit.scale;
    let on_press = Rc::clone(on_pointer);
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble
            && event.button == MouseButton::Left
            && hitbox.is_hovered(window)
        {
            let at = image_point(event.position);
            on_press(Pointer::Press { at, reach }, window, cx);
        }
    });
    let on_move = Rc::clone(on_pointer);
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && event.pressed_button == Some(MouseButton::Left) {
            let at = image_point(event.position);
            let shift = event.modifiers.shift;
            on_move(Pointer::Move { at, shift }, window, cx);
        }
    });
    let on_release = Rc::clone(on_pointer);
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
            let shift = event.modifiers.shift;
            on_release(Pointer::Release { shift }, window, cx);
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

/// Maps document pixels onto the element: uniform scale, centred.
#[derive(Clone, Copy)]
struct Fit {
    origin: Point<Pixels>,
    scale: f32,
    image_bounds: Bounds<Pixels>,
}

impl Fit {
    #[expect(
        clippy::cast_precision_loss,
        reason = "image sides are far below 2^24, where f32 stops being exact"
    )]
    fn new(bounds: Bounds<Pixels>, document: &Document, scale_factor: f32) -> Self {
        let width = document.base().width() as f32;
        let height = document.base().height() as f32;
        // D-41: one image pixel never covers more than one device pixel, so a small capture stays sharp.
        let scale = (f32::from(bounds.size.width) / width)
            .min(f32::from(bounds.size.height) / height)
            .min(1.0 / scale_factor);
        let fitted = size(px(width * scale), px(height * scale));
        let origin = bounds.origin
            + point(
                (bounds.size.width - fitted.width) / 2.0,
                (bounds.size.height - fitted.height) / 2.0,
            );
        Self {
            origin,
            scale,
            image_bounds: Bounds::new(origin, fitted),
        }
    }

    /// The image pixel under `position`, clamped to the image.
    fn image_point(&self, position: Point<Pixels>) -> PointF {
        let offset = position - self.origin;
        let image =
            |screen: Pixels, side: Pixels| f32::from(screen.clamp(px(0.0), side)) / self.scale;
        PointF {
            x: image(offset.x, self.image_bounds.size.width),
            y: image(offset.y, self.image_bounds.size.height),
        }
    }

    fn point(&self, point_f: PointF) -> Point<Pixels> {
        self.origin + point(px(point_f.x * self.scale), px(point_f.y * self.scale))
    }

    fn bounds(&self, rect: RectF) -> Bounds<Pixels> {
        Bounds::new(
            self.point(PointF {
                x: rect.x,
                y: rect.y,
            }),
            size(px(rect.width * self.scale), px(rect.height * self.scale)),
        )
    }
}

fn paint_shape(shape: &Shape, style: Style, fit: &Fit, window: &mut Window) {
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
        // The editor's text and redaction previews come with their tools, after the M0 gate.
        Shape::Text { .. } | Shape::Redact { .. } => {}
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

    fn fit(image_width: u32, image_height: u32, scale_factor: f32) -> Fit {
        let pixels = vec![0; image_width as usize * image_height as usize * 4];
        let image = RgbaImage::new(image_width, image_height, pixels).unwrap();
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(1000.0), px(600.0)));
        Fit::new(bounds, &Document::new(Arc::new(image)), scale_factor)
    }

    #[test]
    fn f23_a_small_capture_is_drawn_one_image_pixel_per_device_pixel() {
        let small = fit(100, 50, 2.0);
        assert_eq!(small.scale, 0.5);
        assert_eq!(small.image_bounds.size, size(px(50.0), px(25.0)));
        assert_eq!(small.image_bounds.center(), point(px(500.0), px(300.0)));
    }

    #[test]
    fn f30_a_pointer_outside_the_image_lands_on_its_nearest_edge() {
        let small = fit(100, 50, 2.0);
        let image_point = |x, y| small.image_point(point(px(x), px(y)));
        assert_eq!(image_point(475.0, 287.5), PointF { x: 0.0, y: 0.0 });
        assert_eq!(image_point(500.0, 300.0), PointF { x: 50.0, y: 25.0 });
        assert_eq!(image_point(0.0, 1000.0), PointF { x: 0.0, y: 50.0 });
        assert_eq!(image_point(900.0, -5.0), PointF { x: 100.0, y: 0.0 });
    }

    #[test]
    fn f23_a_large_capture_shrinks_to_fit_the_canvas() {
        let large = fit(4000, 1200, 1.0);
        assert_eq!(large.image_bounds.size, size(px(1000.0), px(300.0)));
    }
}
