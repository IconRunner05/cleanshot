use std::ops::Range;

use cleanshot_core::{
    Color, Document, Fill, GlyphMask, HighlightStrength, PointF, RectF, RgbaImage, Shape,
    ShapeEntry, ShapeId, TextMasks, arrow_head, dimmed_areas, text_on,
};
use tiny_skia::{
    BlendMode, ColorU8, FillRule, IntRect, IntSize, LineCap, LineJoin, Paint, Path, PathBuilder,
    Pixmap, PremultipliedColorU8, Rect, Stroke, Transform,
};

use crate::border::draw_border;
use crate::gamma::coverage_table;
use crate::redact::redact;
use crate::shadow::cast_shadow;
use crate::{Error, Patch};

/// A document's final pixels, which only [`flatten`] makes, so [`crate::encode`] can export
/// nothing else. Nothing outside this crate can make one:
///
/// ```compile_fail
/// # use cleanshot_render::Flattened;
/// # let capture = cleanshot_core::RgbaImage::new(1, 1, vec![0; 4]).unwrap();
/// let raw = Flattened(capture);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Flattened(RgbaImage);

impl Flattened {
    #[must_use]
    pub fn image(&self) -> &RgbaImage {
        &self.0
    }
}

#[cfg(test)]
impl Flattened {
    /// Encode's tests: `image` as if flatten had made it, alpha and all.
    pub(crate) fn fixture(image: RgbaImage) -> Self {
        Self(image)
    }
}

/// The document's final pixels: the base image with every shape drawn on it in order, cut to
/// the crop, outside any spotlight dimmed first. Copy and save both export these
/// pixels. Text shapes are painted from `masks`, their glyph coverage as DirectWrite
/// rasterised it.
///
/// # Errors
///
/// [`Error::TooLarge`] if the image exceeds the renderer's size limit;
/// [`Error::MissingTextMask`] if `masks` has no entry for one of the document's text shapes.
/// The other variants mean the document broke one of core's guarantees.
pub fn flatten(document: &Document, masks: &TextMasks) -> Result<Flattened, Error> {
    let base = document.base();
    let too_large = || Error::TooLarge {
        width: base.width(),
        height: base.height(),
    };
    let premultiplied = premultiplied(base.pixels());
    let size = IntSize::from_wh(base.width(), base.height()).ok_or_else(too_large)?;
    let mut pixmap = Pixmap::from_vec(premultiplied, size).ok_or_else(too_large)?;
    let dim = Paint {
        anti_alias: false,
        ..paint(document.dim().color(), BlendMode::SourceOver)
    };
    for area in dimmed_areas(base, document.shapes().iter().map(|entry| &entry.shape)) {
        fill(
            &mut pixmap,
            skia_rect(area).map(PathBuilder::from_rect),
            &dim,
        );
    }
    for entry in document.shapes() {
        if entry.style.shadow() {
            let mut layer = Pixmap::new(pixmap.width(), pixmap.height()).ok_or_else(too_large)?;
            draw(&mut layer, entry, Some(masks))?;
            cast_shadow(&mut pixmap, &layer);
        }
        draw(&mut pixmap, entry, Some(masks))?;
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
    RgbaImage::new(pixmap.width(), pixmap.height(), straight(&pixmap))
        .map(Flattened)
        .map_err(Error::Output)
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
    let mut bytes = vec![0; pixmap.pixels().len() * 4];
    for (bytes, pixel) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(pixmap.pixels()) {
        // Most of a patch is clear, and a clear pixel is already all zeros here.
        if pixel.alpha() != 0 {
            let pixel = pixel.demultiply();
            *bytes = [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()];
        }
    }
    bytes
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

/// The pixels of an image that a rectangle touches, premultiplied, with their top left corner
/// on the image, for drawing one shape alone as the export draws it there.
pub(crate) struct Covered {
    left: u32,
    top: u32,
    pub(crate) pixmap: Pixmap,
}

impl Covered {
    /// The pixels of `image` that `rect` touches; `None` if it touches none.
    ///
    /// # Errors
    ///
    /// [`Error::TooLarge`] if the covered area exceeds the renderer's size limit.
    pub(crate) fn of(image: &RgbaImage, rect: RectF) -> Result<Option<Self>, Error> {
        let Some((columns, rows)) = covered_pixels(rect, image.width(), image.height()) else {
            return Ok(None);
        };
        let (width, height) = (columns.end - columns.start, rows.end - rows.start);
        let too_large = || Error::TooLarge { width, height };
        let stride = image.width() as usize * 4;
        let (start, length) = (columns.start as usize * 4, width as usize * 4);
        let covered = rows
            .clone()
            .map(|y| {
                let row = y as usize * stride + start;
                &image.pixels()[row..row + length]
            })
            .collect::<Vec<_>>()
            .concat();
        let size = IntSize::from_wh(width, height).ok_or_else(too_large)?;
        let pixmap = Pixmap::from_vec(premultiplied(&covered), size).ok_or_else(too_large)?;
        Ok(Some(Self {
            left: columns.start,
            top: rows.start,
            pixmap,
        }))
    }

    /// `rect` moved so that the covered pixels' corner is the origin.
    #[expect(
        clippy::cast_precision_loss,
        reason = "an image side is far below 2^24, where f32 stops being exact"
    )]
    pub(crate) fn local(&self, rect: RectF) -> RectF {
        RectF {
            x: rect.x - self.left as f32,
            y: rect.y - self.top as f32,
            ..rect
        }
    }

    /// The covered pixels as drawn, back at their place on the image.
    ///
    /// # Errors
    ///
    /// [`Error::Output`] if the pixels do not make an image, which tiny-skia rules out.
    pub(crate) fn into_patch(self) -> Result<Patch, Error> {
        let pixels = RgbaImage::new(
            self.pixmap.width(),
            self.pixmap.height(),
            straight(&self.pixmap),
        )
        .map_err(Error::Output)?;
        Ok(Patch {
            left: self.left,
            top: self.top,
            pixels,
        })
    }
}

/// Draws `entry` on `pixmap` over its border, its text from `masks`, or without its
/// text when there are none.
pub(crate) fn draw(
    pixmap: &mut Pixmap,
    entry: &ShapeEntry,
    masks: Option<&TextMasks>,
) -> Result<(), Error> {
    draw_border(pixmap, entry, masks)?;
    if let Some((area, color)) = filled(entry) {
        let filling = paint(entry.style.filled(color), BlendMode::SourceOver);
        pixmap.fill_path(
            &area,
            &filling,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
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
        Shape::Highlight { rect, strength } => {
            highlight(pixmap, *rect, entry.style.color, *strength);
        }
        Shape::Redact {
            rect,
            mode,
            strength,
        } => redact(pixmap, *rect, *mode, *strength),
        // Flatten dims around every spotlight before it draws any shape, and a solid shape is
        // all fill, painted above.
        Shape::Spotlight(_)
        | Shape::Rect {
            fill: Fill::Solid, ..
        }
        | Shape::Ellipse {
            fill: Fill::Solid, ..
        } => {}
        Shape::Text(_) => text(pixmap, entry.id, masks, entry.style.color)?,
        Shape::Step(_) | Shape::Bubble(_) => {
            text(pixmap, entry.id, masks, text_on(entry.style.color))?;
        }
    }
    Ok(())
}

/// `rect` with each corner a quarter circle of `radius`, drawn as cubic Beziers.
pub(crate) fn rounded_rect(rect: RectF, radius: f32) -> Option<Path> {
    // The control-point distance that makes a cubic Bezier follow a quarter circle.
    const KAPPA: f32 = 0.552_284_8;
    let radius = radius.min(rect.width / 2.0).min(rect.height / 2.0);
    let control = radius * (1.0 - KAPPA);
    let (left, top) = (rect.x, rect.y);
    let (right, bottom) = (left + rect.width, top + rect.height);
    let mut builder = PathBuilder::new();
    builder.move_to(left + radius, top);
    builder.line_to(right - radius, top);
    builder.cubic_to(
        right - control,
        top,
        right,
        top + control,
        right,
        top + radius,
    );
    builder.line_to(right, bottom - radius);
    builder.cubic_to(
        right,
        bottom - control,
        right - control,
        bottom,
        right - radius,
        bottom,
    );
    builder.line_to(left + radius, bottom);
    builder.cubic_to(
        left + control,
        bottom,
        left,
        bottom - control,
        left,
        bottom - radius,
    );
    builder.line_to(left, top + radius);
    builder.cubic_to(left, top + control, left + control, top, left + radius, top);
    builder.close();
    builder.finish()
}

/// The area `entry` fills, as one path, and the colour it fills it with before its fill
/// opacity: a solid shape's, a step's circle, a bubble's box and tail, a text's background.
pub(crate) fn filled(entry: &ShapeEntry) -> Option<(Path, Color)> {
    let color = entry.style.color;
    match &entry.shape {
        Shape::Rect {
            rect,
            fill: Fill::Solid,
        } => Some((PathBuilder::from_rect(skia_rect(*rect)?), color)),
        Shape::Ellipse {
            rect,
            fill: Fill::Solid,
        } => Some((PathBuilder::from_oval(skia_rect(*rect)?)?, color)),
        Shape::Step(step) => Some((PathBuilder::from_oval(skia_rect(step.circle())?)?, color)),
        Shape::Bubble(bubble) => {
            let mut builder = PathBuilder::new();
            builder.push_path(&rounded_rect(bubble.outline(), bubble.corner_radius())?);
            if let Some([start, tip, end]) = bubble.tail_triangle() {
                builder.move_to(start.x, start.y);
                builder.line_to(tip.x, tip.y);
                builder.line_to(end.x, end.y);
                builder.close();
            }
            Some((builder.finish()?, color))
        }
        Shape::Text(block) => Some((
            PathBuilder::from_rect(skia_rect(block.padded_extent())?),
            entry.style.background()?,
        )),
        Shape::Rect {
            fill: Fill::Hollow, ..
        }
        | Shape::Ellipse {
            fill: Fill::Hollow, ..
        }
        | Shape::Line { .. }
        | Shape::Arrow { .. }
        | Shape::Pen(_)
        | Shape::Highlight { .. }
        | Shape::Redact { .. }
        | Shape::Spotlight(_) => None,
    }
}

/// Paints the glyph layers of the shape `id` in `color`, or in a colour glyph's own colour.
fn text(
    pixmap: &mut Pixmap,
    id: ShapeId,
    masks: Option<&TextMasks>,
    color: Color,
) -> Result<(), Error> {
    let Some(masks) = masks else {
        return Ok(());
    };
    let layers = masks.layers(id).ok_or(Error::MissingTextMask(id))?;
    for layer in layers {
        glyphs(pixmap, layer, layer.color().unwrap_or(color));
    }
    Ok(())
}

/// Paints `color` through one glyph layer's coverage, gamma corrected as GPUI draws it on
/// screen, source over, clipped to the image.
pub(crate) fn glyphs(pixmap: &mut Pixmap, mask: &GlyphMask, color: Color) {
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
pub(crate) fn source_over(
    under: PremultipliedColorU8,
    color: Color,
    coverage: u8,
) -> PremultipliedColorU8 {
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

/// The highlighter darkens what is under it, like a marker on paper, at `strength`'s
/// opacity.
pub(crate) fn highlight(
    pixmap: &mut Pixmap,
    rect: RectF,
    color: Color,
    strength: HighlightStrength,
) {
    let color = Color {
        a: strength.alpha(),
        ..color
    };
    fill(
        pixmap,
        skia_rect(rect).map(PathBuilder::from_rect),
        &paint(color, BlendMode::Multiply),
    );
}

fn fill(pixmap: &mut Pixmap, path: Option<Path>, paint: &Paint) {
    if let Some(path) = path {
        pixmap.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
    }
}

pub(crate) fn paint(color: Color, blend_mode: BlendMode) -> Paint<'static> {
    let mut paint = Paint {
        blend_mode,
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint
}

/// `None` for fewer than two points: a path with no length draws nothing.
pub(crate) fn polyline(points: &[PointF]) -> Option<Path> {
    let (first, rest) = points.split_first()?;
    let mut builder = PathBuilder::new();
    builder.move_to(first.x, first.y);
    for point in rest {
        builder.line_to(point.x, point.y);
    }
    builder.finish()
}

/// `None` for an empty or non-finite rectangle, which draws nothing.
pub(crate) fn skia_rect(rect: RectF) -> Option<Rect> {
    Rect::from_xywh(rect.x, rect.y, rect.width, rect.height)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cleanshot_core::{Command, History, SpotlightDim, Style};

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
            &flatten(history.document(), &TextMasks::default())
                .unwrap()
                .image()
                .clone(),
            &**history.document().base()
        );
    }

    #[test]
    fn highlight_multiplies_the_image_under_it() {
        let mut history = history(uniform([200, 100, 50, 255]));
        history
            .apply(Command::Add {
                shape: Shape::Highlight {
                    rect: whole_image(),
                    strength: HighlightStrength::Strong,
                },
                style: Style::new(
                    Color {
                        r: 255,
                        g: 255,
                        b: 0,
                        a: 255,
                    },
                    1.0,
                ),
            })
            .unwrap();

        let flat = flatten(history.document(), &TextMasks::default())
            .unwrap()
            .image()
            .clone();
        assert_eq!(pixel(&flat, 1, 1), [200, 100, 0, 255]);
    }

    #[test]
    fn export_keeps_only_the_cropped_pixels() {
        let mut history = history(numbered());
        let crop = RectF {
            x: 1.0,
            y: 1.0,
            width: 2.0,
            height: 2.0,
        };
        history.apply(Command::SetCrop(Some(crop))).unwrap();

        let flat = flatten(history.document(), &TextMasks::default())
            .unwrap()
            .image()
            .clone();
        assert_eq!((flat.width(), flat.height()), (2, 2));
        assert_eq!(pixel(&flat, 0, 0), pixel(&numbered(), 1, 1));
        assert_eq!(pixel(&flat, 1, 1), pixel(&numbered(), 2, 2));
    }

    #[test]
    fn a_fractional_crop_keeps_every_pixel_it_touches() {
        let mut history = history(numbered());
        let crop = RectF {
            x: 0.5,
            y: 0.5,
            width: 1.0,
            height: 1.0,
        };
        history.apply(Command::SetCrop(Some(crop))).unwrap();

        let flat = flatten(history.document(), &TextMasks::default())
            .unwrap()
            .image()
            .clone();
        assert_eq!((flat.width(), flat.height()), (2, 2));
        assert_eq!(pixel(&flat, 0, 0), pixel(&numbered(), 0, 0));
    }

    const BLUE: Color = Color {
        r: 0,
        g: 0,
        b: 255,
        a: 255,
    };

    fn with_text(base: RgbaImage) -> (History, ShapeId) {
        let mut history = history(base);
        let block = cleanshot_core::TextBlock::new(PointF { x: 0.0, y: 0.0 }, "a".into(), 2.0, 1.0)
            .unwrap();
        history
            .apply(Command::Add {
                shape: Shape::Text(block),
                style: Style::new(BLUE, 1.0),
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
    fn a_text_shape_without_glyph_masks_is_refused() {
        let (history, id) = with_text(numbered());
        assert!(matches!(
            flatten(history.document(), &TextMasks::default()),
            Err(Error::MissingTextMask(missing)) if missing == id
        ));
    }

    #[test]
    fn a_step_fills_its_circle_and_draws_its_number_in_the_contrasting_colour() {
        let mut history = history(RgbaImage::new(40, 40, [0, 0, 0, 255].repeat(1600)).unwrap());
        let look = cleanshot_core::Look::default_for(cleanshot_core::Tool::Step);
        let centre = PointF { x: 20.0, y: 20.0 };
        let step = cleanshot_core::Tool::Step.command(&[centre], false, &look);
        history.apply(step.unwrap()).unwrap();
        let id = history.document().shapes()[0].id;
        assert!(matches!(
            flatten(history.document(), &TextMasks::default()),
            Err(Error::MissingTextMask(missing)) if missing == id
        ));
        let mut masks = TextMasks::default();
        masks.insert(id, vec![mask(20, 20, 1, &[255], None)]);
        let flat = flatten(history.document(), &masks).unwrap().image().clone();
        let red = look.style().color;
        assert_eq!(pixel(&flat, 20, 20), [255, 255, 255, 255]);
        assert_eq!(pixel(&flat, 20, 10), [red.r, red.g, red.b, 255]);
        assert_eq!(pixel(&flat, 1, 1), [0, 0, 0, 255]);
    }

    #[test]
    fn a_bubble_fills_its_rounded_box_and_tail_and_draws_its_text_in_the_contrasting_colour() {
        let mut history = history(RgbaImage::new(40, 40, [0, 0, 0, 255].repeat(1600)).unwrap());
        let block =
            cleanshot_core::TextBlock::new(PointF { x: 10.0, y: 10.0 }, "a".into(), 8.0, 1.0)
                .unwrap();
        let bubble = cleanshot_core::Bubble::new(block, PointF { x: 14.0, y: 38.0 });
        let red = Color {
            r: 255,
            g: 59,
            b: 48,
            a: 255,
        };
        history
            .apply(Command::Add {
                shape: Shape::Bubble(bubble),
                style: Style::new(red, 3.0),
            })
            .unwrap();
        let id = history.document().shapes()[0].id;
        assert!(matches!(
            flatten(history.document(), &TextMasks::default()),
            Err(Error::MissingTextMask(missing)) if missing == id
        ));
        let mut masks = TextMasks::default();
        masks.insert(id, vec![mask(12, 12, 1, &[255], None)]);
        let flat = flatten(history.document(), &masks).unwrap().image().clone();
        let red = [red.r, red.g, red.b, 255];
        assert_eq!(pixel(&flat, 12, 12), [255, 255, 255, 255]);
        assert_eq!(pixel(&flat, 16, 20), red);
        assert_eq!(pixel(&flat, 14, 33), red);
        assert_eq!(pixel(&flat, 6, 6), [0, 0, 0, 255]);
        assert_eq!(pixel(&flat, 20, 33), [0, 0, 0, 255]);
    }

    #[test]
    fn text_with_no_ink_leaves_the_image_as_it_was() {
        let (history, id) = with_text(numbered());
        let mut masks = TextMasks::default();
        masks.insert(id, vec![mask(0, 0, 1, &[], None)]);
        assert_eq!(
            &flatten(history.document(), &masks).unwrap().image().clone(),
            &**history.document().base()
        );
    }

    #[test]
    fn glyph_coverage_paints_the_shapes_colour_where_the_mask_sits() {
        let (history, id) = with_text(uniform([200, 100, 50, 255]));
        let mut masks = TextMasks::default();
        masks.insert(id, vec![mask(1, 2, 3, &[255, 128, 0], None)]);
        let flat = flatten(history.document(), &masks).unwrap().image().clone();
        assert_eq!(pixel(&flat, 1, 2), [0, 0, 255, 255]);
        assert_eq!(pixel(&flat, 2, 2), [100, 50, 153, 255]);
        assert_eq!(pixel(&flat, 3, 2), [200, 100, 50, 255]);
        assert_eq!(pixel(&flat, 1, 1), [200, 100, 50, 255]);
    }

    #[test]
    fn a_colour_glyph_layer_keeps_its_own_colour_and_layers_paint_in_order() {
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
        let flat = flatten(history.document(), &masks).unwrap().image().clone();
        assert_eq!(pixel(&flat, 0, 0), [0, 0, 255, 255]);
        assert_eq!(pixel(&flat, 1, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn a_glyph_mask_partly_outside_the_image_paints_only_the_part_inside() {
        let (history, id) = with_text(uniform([200, 100, 50, 255]));
        let mut masks = TextMasks::default();
        masks.insert(id, vec![mask(-1, 2, 2, &[255, 255, 255, 255], None)]);
        let flat = flatten(history.document(), &masks).unwrap().image().clone();
        assert_eq!(pixel(&flat, 0, 2), [0, 0, 255, 255]);
        assert_eq!(pixel(&flat, 1, 2), [200, 100, 50, 255]);
        assert_eq!(pixel(&flat, 3, 2), [200, 100, 50, 255]);
        assert_eq!(pixel(&flat, 0, 1), [200, 100, 50, 255]);
    }

    #[test]
    fn a_spotlight_dims_the_capture_outside_it_and_no_annotation() {
        let mut history = history(uniform([250, 250, 250, 255]));
        let red = Style::new(
            Color {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            },
            1.0,
        );
        let corner = RectF {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        };
        let spotlight = RectF {
            x: 2.0,
            y: 1.0,
            width: 2.0,
            height: 2.0,
        };
        for shape in [
            Shape::Rect {
                rect: corner,
                fill: Fill::Solid,
            },
            Shape::Spotlight(spotlight),
        ] {
            history.apply(Command::Add { shape, style: red }).unwrap();
        }

        let flat = flatten(history.document(), &TextMasks::default())
            .unwrap()
            .image()
            .clone();
        assert_eq!(pixel(&flat, 0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&flat, 3, 2), [250, 250, 250, 255]);
        assert_eq!(pixel(&flat, 1, 1), [100, 100, 100, 255]);
    }

    #[test]
    fn the_export_dims_outside_a_spotlight_as_dark_as_the_document_says() {
        let mut history = history(uniform([250, 250, 250, 255]));
        let spotlight = Shape::Spotlight(RectF {
            x: 2.0,
            y: 1.0,
            width: 2.0,
            height: 2.0,
        });
        let style = Style::new(
            Color {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            1.0,
        );
        history
            .apply(Command::Add {
                shape: spotlight,
                style,
            })
            .unwrap();
        let outside = |history: &History| {
            let flat = flatten(history.document(), &TextMasks::default()).unwrap();
            pixel(flat.image(), 0, 0)[0]
        };
        let mut shades = Vec::new();
        for dim in [
            SpotlightDim::Light,
            SpotlightDim::Medium,
            SpotlightDim::Dark,
        ] {
            history.apply(Command::SetDim(dim)).unwrap();
            shades.push(outside(&history));
        }
        assert_eq!(shades, [150, 100, 50]);
    }

    #[test]
    fn a_redaction_outside_the_spotlight_exports_dimmed_like_the_capture() {
        let mut history = history(uniform([250, 250, 250, 255]));
        let style = Style::new(
            Color {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            },
            1.0,
        );
        let spotlight = Shape::Spotlight(RectF {
            x: 3.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        });
        let redaction = Shape::Redact {
            rect: RectF {
                x: 0.0,
                y: 1.0,
                width: 2.0,
                height: 2.0,
            },
            mode: cleanshot_core::RedactMode::Pixelate,
            strength: cleanshot_core::RedactStrength::FINE,
        };
        for shape in [spotlight, redaction] {
            history.apply(Command::Add { shape, style }).unwrap();
        }

        let flat = flatten(history.document(), &TextMasks::default())
            .unwrap()
            .image()
            .clone();
        assert_eq!(pixel(&flat, 0, 1), [100, 100, 100, 255]);
        assert_eq!(pixel(&flat, 1, 2), [100, 100, 100, 255]);
    }
}
