use std::ops::Range;

use cleanshot_core::{
    Color, Document, Fill, GlyphMask, PointF, RectF, RgbaImage, Shape, ShapeEntry, TextMasks,
    arrow_head,
};
use tiny_skia::{
    BlendMode, ColorU8, FillRule, IntRect, IntSize, LineCap, LineJoin, Paint, Path, PathBuilder,
    Pixmap, PremultipliedColorU8, Rect, Stroke, Transform,
};

use crate::Error;
use crate::gamma::coverage_table;
use crate::redact::redact;

/// The document's final pixels: the base image with every shape drawn on it in order, cut to
/// the crop (F-37). Copy and save both export these pixels (F-24). Text shapes are painted
/// from `masks`, their glyph coverage as DirectWrite rasterised it (F-34).
///
/// # Errors
///
/// [`Error::TooLarge`] if the image exceeds the renderer's size limit;
/// [`Error::MissingTextMask`] if `masks` has no entry for one of the document's text shapes.
/// The other variants mean the document broke one of core's guarantees.
pub fn flatten(document: &Document, masks: &TextMasks) -> Result<RgbaImage, Error> {
    let base = document.base();
    let too_large = || Error::TooLarge {
        width: base.width(),
        height: base.height(),
    };
    let premultiplied = premultiplied(base.pixels());
    let size = IntSize::from_wh(base.width(), base.height()).ok_or_else(too_large)?;
    let mut pixmap = Pixmap::from_vec(premultiplied, size).ok_or_else(too_large)?;
    for entry in document.shapes() {
        draw(&mut pixmap, entry, masks)?;
    }
    if let Some(crop) = document.crop() {
        pixmap = covered_pixels(crop, pixmap.width(), pixmap.height())
            .and_then(|(columns, rows)| {
                IntRect::from_xywh(
                    i32::try_from(columns.start).ok()?,
                    i32::try_from(rows.start).ok()?,
                    columns.end - columns.start,
                    rows.end - rows.start,
                )
            })
            .and_then(|bounds| pixmap.clone_rect(bounds))
            .ok_or(Error::CropOutsideImage(crop))?;
    }
    RgbaImage::new(pixmap.width(), pixmap.height(), straight(&pixmap)).map_err(Error::Output)
}

/// Straight RGBA bytes as the premultiplied ones tiny-skia draws on.
pub(crate) fn premultiplied(straight: &[u8]) -> Vec<u8> {
    straight
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, a]| {
            let pixel = ColorU8::from_rgba(r, g, b, a).premultiply();
            [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
        })
        .collect()
}

/// `pixmap`'s pixels as straight RGBA bytes.
pub(crate) fn straight(pixmap: &Pixmap) -> Vec<u8> {
    pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let pixel = pixel.demultiply();
            [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
        })
        .collect()
}

/// The columns and rows of every image pixel `rect` touches, so fractional coordinates lose no
/// edge pixel; `None` if it touches none.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=limit, a u32, first"
)]
pub(crate) fn covered_pixels(
    rect: RectF,
    width: u32,
    height: u32,
) -> Option<(Range<u32>, Range<u32>)> {
    let span = |start: f32, length: f32, limit: u32| {
        let limit = f64::from(limit);
        let first = f64::from(start).floor().clamp(0.0, limit);
        let end = (f64::from(start) + f64::from(length))
            .ceil()
            .clamp(0.0, limit);
        (first < end).then_some(first as u32..end as u32)
    };
    Some((
        span(rect.x, rect.width, width)?,
        span(rect.y, rect.height, height)?,
    ))
}

fn draw(pixmap: &mut Pixmap, entry: &ShapeEntry, masks: &TextMasks) -> Result<(), Error> {
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
        Shape::Redact { rect, mode } => redact(pixmap, *rect, *mode),
        Shape::Text(_) => {
            let layers = masks
                .layers(entry.id)
                .ok_or(Error::MissingTextMask(entry.id))?;
            for layer in layers {
                glyphs(pixmap, layer, layer.color().unwrap_or(entry.style.color));
            }
        }
    }
    Ok(())
}

/// Paints `color` through one glyph layer's coverage, gamma corrected as GPUI draws it on
/// screen, source over, clipped to the image.
fn glyphs(pixmap: &mut Pixmap, mask: &GlyphMask, color: Color) {
    if mask.width() == 0 || mask.height() == 0 {
        return;
    }
    let corrected = coverage_table(color);
    let (width, height) = (pixmap.width(), pixmap.height());
    let pixels = pixmap.pixels_mut();
    let rows = mask.coverage().chunks_exact(mask.width() as usize);
    for (row, line) in rows.enumerate() {
        let Some(y) = image_index(mask.top(), row, height) else {
            continue;
        };
        for (column, &coverage) in line.iter().enumerate() {
            let Some(x) = image_index(mask.left(), column, width) else {
                continue;
            };
            let pixel = &mut pixels[y * width as usize + x];
            *pixel = source_over(*pixel, color, corrected[usize::from(coverage)]);
        }
    }
}

/// The image column or row `offset` past `start`, if it lies inside `0..limit`.
fn image_index(start: i32, offset: usize, limit: u32) -> Option<usize> {
    let position = i64::from(start) + i64::try_from(offset).ok()?;
    usize::try_from(position)
        .ok()
        .filter(|&position| position < limit as usize)
}

/// `color` at `coverage` over `under`, in premultiplied bytes rounded to nearest.
fn source_over(under: PremultipliedColorU8, color: Color, coverage: u8) -> PremultipliedColorU8 {
    let times = |a: u8, b: u8| {
        u8::try_from((u32::from(a) * u32::from(b) + 127) / 255)
            .expect("a byte times a byte over 255 is a byte")
    };
    let over =
        ColorU8::from_rgba(color.r, color.g, color.b, times(color.a, coverage)).premultiply();
    let keep = 255 - over.alpha();
    PremultipliedColorU8::from_rgba(
        over.red() + times(under.red(), keep),
        over.green() + times(under.green(), keep),
        over.blue() + times(under.blue(), keep),
        over.alpha() + times(under.alpha(), keep),
    )
    .expect("source over two premultiplied colours is premultiplied")
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
            &flatten(history.document(), &TextMasks::default()).unwrap(),
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

        let flat = flatten(history.document(), &TextMasks::default()).unwrap();
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

        let flat = flatten(history.document(), &TextMasks::default()).unwrap();
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

        let flat = flatten(history.document(), &TextMasks::default()).unwrap();
        assert_eq!((flat.width(), flat.height()), (2, 2));
        assert_eq!(pixel(&flat, 0, 0), pixel(&numbered(), 0, 0));
    }

    const BLUE: Color = Color {
        r: 0,
        g: 0,
        b: 255,
        a: 255,
    };

    fn with_text(base: RgbaImage) -> (History, cleanshot_core::ShapeId) {
        let mut history = history(base);
        let block = cleanshot_core::TextBlock::new(PointF { x: 0.0, y: 0.0 }, "a".into(), 2.0, 1.0)
            .unwrap();
        history
            .apply(Command::Add {
                shape: Shape::Text(block),
                style: Style {
                    color: BLUE,
                    stroke_width: 1.0,
                },
            })
            .unwrap();
        let id = history.document().shapes()[0].id;
        (history, id)
    }

    fn mask(left: i32, top: i32, width: u32, coverage: &[u8], color: Option<Color>) -> GlyphMask {
        let height = u32::try_from(coverage.len()).unwrap() / width;
        GlyphMask::new(left, top, width, height, coverage.to_vec(), color).unwrap()
    }

    #[test]
    fn f34_a_text_shape_without_glyph_masks_is_refused() {
        let (history, id) = with_text(numbered());
        assert!(matches!(
            flatten(history.document(), &TextMasks::default()),
            Err(Error::MissingTextMask(missing)) if missing == id
        ));
    }

    #[test]
    fn f34_text_with_no_ink_leaves_the_image_as_it_was() {
        let (history, id) = with_text(numbered());
        let mut masks = TextMasks::default();
        masks.insert(id, vec![mask(0, 0, 1, &[], None)]);
        assert_eq!(
            &flatten(history.document(), &masks).unwrap(),
            &**history.document().base()
        );
    }

    #[test]
    fn f34_glyph_coverage_paints_the_shapes_colour_where_the_mask_sits() {
        let (history, id) = with_text(uniform([200, 100, 50, 255]));
        let mut masks = TextMasks::default();
        masks.insert(id, vec![mask(1, 2, 3, &[255, 128, 0], None)]);
        let flat = flatten(history.document(), &masks).unwrap();
        assert_eq!(pixel(&flat, 1, 2), [0, 0, 255, 255]);
        assert_eq!(pixel(&flat, 2, 2), [100, 50, 153, 255]);
        assert_eq!(pixel(&flat, 3, 2), [200, 100, 50, 255]);
        assert_eq!(pixel(&flat, 1, 1), [200, 100, 50, 255]);
    }

    #[test]
    fn f34_a_colour_glyph_layer_keeps_its_own_colour_and_layers_paint_in_order() {
        let (history, id) = with_text(uniform([200, 100, 50, 255]));
        let white = Color {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        };
        let mut masks = TextMasks::default();
        masks.insert(
            id,
            vec![
                mask(0, 0, 2, &[255, 255], None),
                mask(1, 0, 1, &[255], Some(white)),
            ],
        );
        let flat = flatten(history.document(), &masks).unwrap();
        assert_eq!(pixel(&flat, 0, 0), [0, 0, 255, 255]);
        assert_eq!(pixel(&flat, 1, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn f34_a_glyph_mask_partly_outside_the_image_paints_only_the_part_inside() {
        let (history, id) = with_text(uniform([200, 100, 50, 255]));
        let mut masks = TextMasks::default();
        masks.insert(id, vec![mask(-1, 2, 2, &[255, 255, 255, 255], None)]);
        let flat = flatten(history.document(), &masks).unwrap();
        assert_eq!(pixel(&flat, 0, 2), [0, 0, 255, 255]);
        assert_eq!(pixel(&flat, 1, 2), [200, 100, 50, 255]);
        assert_eq!(pixel(&flat, 3, 2), [200, 100, 50, 255]);
        assert_eq!(pixel(&flat, 0, 1), [200, 100, 50, 255]);
    }
}
