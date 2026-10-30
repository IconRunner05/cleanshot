use cleanshot_core::{
    Color, Document, Fill, PointF, RectF, RgbaImage, Shape, ShapeEntry, arrow_head,
};
use tiny_skia::{
    BlendMode, ColorU8, FillRule, IntRect, IntSize, LineCap, LineJoin, Paint, Path, PathBuilder,
    Pixmap, Rect, Stroke, Transform,
};

use crate::Error;

/// The document's final pixels: the base image with every shape drawn on it in order, cut to
/// the crop (F-37). Copy and save both export these pixels (F-24).
///
/// # Errors
///
/// [`Error::TooLarge`] if the image exceeds the renderer's size limit. The other variants mean
/// the document broke one of core's guarantees.
pub fn flatten(document: &Document) -> Result<RgbaImage, Error> {
    let base = document.base();
    let too_large = || Error::TooLarge {
        width: base.width(),
        height: base.height(),
    };
    let premultiplied = base
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, a]| {
            let pixel = ColorU8::from_rgba(r, g, b, a).premultiply();
            [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
        })
        .collect();
    let size = IntSize::from_wh(base.width(), base.height()).ok_or_else(too_large)?;
    let mut pixmap = Pixmap::from_vec(premultiplied, size).ok_or_else(too_large)?;
    for entry in document.shapes() {
        draw(&mut pixmap, entry);
    }
    if let Some(crop) = document.crop() {
        pixmap = pixel_bounds(crop)
            .and_then(|bounds| pixmap.clone_rect(bounds))
            .ok_or(Error::CropOutsideImage(crop))?;
    }
    let straight = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let pixel = pixel.demultiply();
            [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
        })
        .collect();
    RgbaImage::new(pixmap.width(), pixmap.height(), straight).map_err(Error::Output)
}

/// Every pixel the crop touches, so a crop on fractional coordinates loses no edge pixel.
#[expect(
    clippy::cast_possible_truncation,
    reason = "core keeps a crop inside the image, whose sides fit in i32"
)]
fn pixel_bounds(crop: RectF) -> Option<IntRect> {
    IntRect::from_ltrb(
        crop.x.floor() as i32,
        crop.y.floor() as i32,
        (crop.x + crop.width).ceil() as i32,
        (crop.y + crop.height).ceil() as i32,
    )
}

fn draw(pixmap: &mut Pixmap, entry: &ShapeEntry) {
    let paint = paint(entry.style.color, BlendMode::SourceOver);
    let stroke = Stroke {
        width: entry.style.stroke_width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let identity = Transform::identity();
    let mut stroke_path = |path: Option<Path>| {
        if let Some(path) = path {
            pixmap.stroke_path(&path, &paint, &stroke, identity, None);
        }
    };
    match &entry.shape {
        Shape::Line { from, to } => stroke_path(polyline(&[*from, *to])),
        Shape::Arrow { from, to } => {
            stroke_path(polyline(&[*from, *to]));
            stroke_path(polyline(&arrow_head(*from, *to, entry.style.stroke_width)));
        }
        Shape::Pen(points) => stroke_path(polyline(points)),
        Shape::Rect {
            rect,
            fill: Fill::Hollow,
        } => stroke_path(skia_rect(*rect).map(PathBuilder::from_rect)),
        Shape::Ellipse {
            rect,
            fill: Fill::Hollow,
        } => stroke_path(skia_rect(*rect).and_then(PathBuilder::from_oval)),
        Shape::Rect {
            rect,
            fill: Fill::Solid,
        } => fill(pixmap, skia_rect(*rect).map(PathBuilder::from_rect), &paint),
        Shape::Ellipse {
            rect,
            fill: Fill::Solid,
        } => fill(
            pixmap,
            skia_rect(*rect).and_then(PathBuilder::from_oval),
            &paint,
        ),
        // F-35: the highlighter darkens what is under it, like a marker on paper.
        Shape::Highlight(rect) => fill(
            pixmap,
            skia_rect(*rect).map(PathBuilder::from_rect),
            &self::paint(entry.style.color, BlendMode::Multiply),
        ),
        // Text waits for DirectWrite glyph masks from the caller; redaction is the next commit.
        Shape::Text { .. } | Shape::Redact { .. } => {}
    }
}

fn fill(pixmap: &mut Pixmap, path: Option<Path>, paint: &Paint) {
    if let Some(path) = path {
        pixmap.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
    }
}

fn paint(color: Color, blend_mode: BlendMode) -> Paint<'static> {
    let mut paint = Paint {
        blend_mode,
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint
}

/// `None` for fewer than two points: a path with no length draws nothing.
fn polyline(points: &[PointF]) -> Option<Path> {
    let (first, rest) = points.split_first()?;
    let mut builder = PathBuilder::new();
    builder.move_to(first.x, first.y);
    for point in rest {
        builder.line_to(point.x, point.y);
    }
    builder.finish()
}

/// `None` for an empty or non-finite rectangle, which draws nothing.
fn skia_rect(rect: RectF) -> Option<Rect> {
    Rect::from_xywh(rect.x, rect.y, rect.width, rect.height)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cleanshot_core::{Command, History, Style};

    use super::*;

    const WIDTH: u32 = 4;
    const HEIGHT: u32 = 3;

    /// Each pixel encodes its own position, so a test can tell which pixels survived.
    fn numbered() -> RgbaImage {
        let mut pixels = Vec::new();
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                pixels.extend([
                    u8::try_from(x * 10).unwrap(),
                    u8::try_from(y * 10).unwrap(),
                    7,
                    255,
                ]);
            }
        }
        RgbaImage::new(WIDTH, HEIGHT, pixels).unwrap()
    }

    fn uniform(rgba: [u8; 4]) -> RgbaImage {
        RgbaImage::new(WIDTH, HEIGHT, rgba.repeat((WIDTH * HEIGHT) as usize)).unwrap()
    }

    fn history(base: RgbaImage) -> History {
        History::new(Document::new(Arc::new(base)))
    }

    fn whole_image() -> RectF {
        RectF {
            x: 0.0,
            y: 0.0,
            width: 4.0,
            height: 3.0,
        }
    }

    fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let start = ((y * image.width() + x) * 4) as usize;
        image.pixels()[start..start + 4].try_into().unwrap()
    }

    #[test]
    fn a_document_with_no_shapes_flattens_to_its_base() {
        let history = history(numbered());
        assert_eq!(
            &flatten(history.document()).unwrap(),
            &**history.document().base()
        );
    }

    #[test]
    fn f35_highlight_multiplies_the_image_under_it() {
        let mut history = history(uniform([200, 100, 50, 255]));
        history
            .apply(Command::Add {
                shape: Shape::Highlight(whole_image()),
                style: Style {
                    color: Color {
                        r: 255,
                        g: 255,
                        b: 0,
                        a: 255,
                    },
                    stroke_width: 1.0,
                },
            })
            .unwrap();

        let flat = flatten(history.document()).unwrap();
        assert_eq!(pixel(&flat, 1, 1), [200, 100, 0, 255]);
    }

    #[test]
    fn f37_export_keeps_only_the_cropped_pixels() {
        let mut history = history(numbered());
        let crop = RectF {
            x: 1.0,
            y: 1.0,
            width: 2.0,
            height: 2.0,
        };
        history.apply(Command::SetCrop(Some(crop))).unwrap();

        let flat = flatten(history.document()).unwrap();
        assert_eq!((flat.width(), flat.height()), (2, 2));
        assert_eq!(pixel(&flat, 0, 0), pixel(&numbered(), 1, 1));
        assert_eq!(pixel(&flat, 1, 1), pixel(&numbered(), 2, 2));
    }

    #[test]
    fn f37_a_fractional_crop_keeps_every_pixel_it_touches() {
        let mut history = history(numbered());
        let crop = RectF {
            x: 0.5,
            y: 0.5,
            width: 1.0,
            height: 1.0,
        };
        history.apply(Command::SetCrop(Some(crop))).unwrap();

        let flat = flatten(history.document()).unwrap();
        assert_eq!((flat.width(), flat.height()), (2, 2));
        assert_eq!(pixel(&flat, 0, 0), pixel(&numbered(), 0, 0));
    }
}
