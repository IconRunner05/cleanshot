use std::f32::consts::TAU;
use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{
    Color, Document, Fill, PointF, RectF, RgbaImage, Shape, ShapeEntry, arrow_head,
};
use gpui::{
    BorderStyle, Bounds, Canvas, PathBuilder, Pixels, Point, RenderImage, Rgba, Window, canvas,
    fill, point, px, quad, size, transparent_black,
};
use smallvec::smallvec;

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

/// Draws the document's base image scaled to fit the element, with its shapes on top.
pub(crate) fn document_canvas(document: Rc<Document>, base: Arc<RenderImage>) -> Canvas<()> {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let fit = Fit::new(bounds, &document);
            // Fails only if the GPU cannot allocate the atlas texture; the shapes still draw.
            let _ = window.paint_image(
                fit.image_bounds,
                fit.image_bounds,
                0.0.into(),
                base,
                0,
                false,
            );
            for entry in document.shapes() {
                paint_shape(entry, &fit, window);
            }
        },
    )
}

/// Maps document pixels onto the element: uniform scale, centred.
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
    fn new(bounds: Bounds<Pixels>, document: &Document) -> Self {
        let width = document.base().width() as f32;
        let height = document.base().height() as f32;
        let scale =
            (f32::from(bounds.size.width) / width).min(f32::from(bounds.size.height) / height);
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

fn paint_shape(entry: &ShapeEntry, fit: &Fit, window: &mut Window) {
    let color = rgba(entry.style.color);
    let width = px(entry.style.stroke_width * fit.scale);
    let stroke = PathBuilder::stroke(width);
    match &entry.shape {
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
            let head = arrow_head(*from, *to, entry.style.stroke_width).map(|p| fit.point(p));
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
