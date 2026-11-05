use cleanshot_core::{
    BorderWidth, Color, Fill, GlyphMask, PointF, Shape, ShapeEntry, TextMasks, arrow_head,
};
use tiny_skia::{BlendMode, LineCap, LineJoin, Path, PathBuilder, Pixmap, Stroke, Transform};

use crate::flatten::{glyphs, paint, polyline, rounded_rect, skia_rect};
use crate::shadow::drawn_patch;
use crate::{Error, Patch};

/// The border `entry` has on an image of `width` by `height`, drawn alone as the export draws
/// it under the shape (D-131), for the editor to show; a text's from its glyphs in `masks`,
/// and none without them. `None` when `entry` has no border or it falls outside the image.
///
/// # Errors
///
/// [`Error::TooLarge`] if the image exceeds the renderer's size limit;
/// [`Error::MissingTextMask`] if `masks` has no entry for a bordered text shape.
pub fn shape_border(
    entry: &ShapeEntry,
    width: u32,
    height: u32,
    masks: Option<&TextMasks>,
) -> Result<Option<Patch>, Error> {
    if entry.style.border().is_none() {
        return Ok(None);
    }
    let mut layer = Pixmap::new(width, height).ok_or(Error::TooLarge { width, height })?;
    draw_border(&mut layer, entry, masks)?;
    drawn_patch(&layer)
}

/// Draws `entry`'s border, if it has one, where the shape is about to be drawn over it
/// (D-131): a stroke is drawn again wider, an outline round a filled shape is a stroke along
/// its edge that the fill then covers on the inside, and text is its glyphs grown by the
/// border's width. Pen strokes, highlights, redactions and spotlights draw none.
///
/// # Errors
///
/// [`Error::MissingTextMask`] if `masks` has no entry for a bordered text shape.
pub(crate) fn draw_border(
    pixmap: &mut Pixmap,
    entry: &ShapeEntry,
    masks: Option<&TextMasks>,
) -> Result<(), Error> {
    let Some(border) = entry.style.border() else {
        return Ok(());
    };
    let reach = f32::from(border.width.pixels());
    let paint = paint(border.color, BlendMode::SourceOver);
    let mut stroke_path = |path: Option<Path>, width: f32| {
        let stroke = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        if let Some(path) = path {
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    };
    let casing = entry.style.stroke_width + 2.0 * reach;
    let outline = 2.0 * reach;
    match &entry.shape {
        Shape::Line { from, to } => stroke_path(polyline(&[*from, *to]), casing),
        Shape::Arrow { from, to } => {
            stroke_path(polyline(&[*from, *to]), casing);
            let head = arrow_head(*from, *to, entry.style.stroke_width);
            stroke_path(polyline(&head), casing);
        }
        Shape::Rect { rect, fill } => {
            let width = match fill {
                Fill::Hollow => casing,
                Fill::Solid => outline,
            };
            stroke_path(skia_rect(*rect).map(PathBuilder::from_rect), width);
        }
        Shape::Ellipse { rect, fill } => {
            let width = match fill {
                Fill::Hollow => casing,
                Fill::Solid => outline,
            };
            stroke_path(skia_rect(*rect).and_then(PathBuilder::from_oval), width);
        }
        Shape::Step(step) => stroke_path(
            skia_rect(step.circle()).and_then(PathBuilder::from_oval),
            outline,
        ),
        Shape::Bubble(bubble) => {
            stroke_path(
                rounded_rect(bubble.outline(), bubble.corner_radius()),
                outline,
            );
            if let Some(tail) = bubble.tail_triangle() {
                stroke_path(closed(&tail), outline);
            }
        }
        Shape::Text(_) => {
            let Some(masks) = masks else {
                return Ok(());
            };
            let layers = masks
                .layers(entry.id)
                .ok_or(Error::MissingTextMask(entry.id))?;
            for layer in layers {
                glyphs(pixmap, &grown(layer, border.width), border.color);
            }
        }
        Shape::Pen(_) | Shape::Highlight { .. } | Shape::Redact { .. } | Shape::Spotlight(_) => {}
    }
    Ok(())
}

/// The triangle through `corners`, closed so a stroke goes all the way round.
fn closed(corners: &[PointF; 3]) -> Option<Path> {
    let [first, second, third] = corners;
    let mut builder = PathBuilder::new();
    builder.move_to(first.x, first.y);
    builder.line_to(second.x, second.y);
    builder.line_to(third.x, third.y);
    builder.close();
    builder.finish()
}

/// `mask` grown by `width`: each pixel takes the most coverage within that many pixels of it,
/// in the shape's own colour whatever the layer's was.
fn grown(mask: &GlyphMask, width: BorderWidth) -> GlyphMask {
    let reach = usize::from(width.pixels());
    let (columns, rows) = (mask.width() as usize, mask.height() as usize);
    let grown_columns = columns + 2 * reach;
    let mut coverage = vec![0_u8; grown_columns * (rows + 2 * reach)];
    let steps = steps(width);
    for (row, line) in mask.coverage().chunks_exact(columns).enumerate() {
        for (column, &value) in line.iter().enumerate().filter(|&(_, &value)| value > 0) {
            for &(across, down) in &steps {
                let at = |start: usize, step: i8| {
                    (start + reach)
                        .checked_add_signed(isize::from(step))
                        .expect("a step reaches no further than the growth")
                };
                let pixel = &mut coverage[at(row, down) * grown_columns + at(column, across)];
                *pixel = (*pixel).max(value);
            }
        }
    }
    let grow = i32::from(width.pixels());
    let grown_size = |side: u32| side + 2 * u32::from(width.pixels());
    GlyphMask::new(
        mask.left() - grow,
        mask.top() - grow,
        grown_size(mask.width()),
        grown_size(mask.height()),
        coverage,
        None::<Color>,
    )
    .expect("the grown coverage holds one byte for each grown pixel")
}

/// The whole-pixel steps no further than `width` from a glyph pixel that its growth covers.
fn steps(width: BorderWidth) -> Vec<(i8, i8)> {
    let reach = width.pixels().cast_signed();
    (-reach..=reach)
        .flat_map(|down| (-reach..=reach).map(move |across| (across, down)))
        .filter(|&(across, down)| {
            i16::from(across).pow(2) + i16::from(down).pow(2) <= i16::from(reach).pow(2)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use cleanshot_core::{
        Command, Document, History, Look, PointF, Property, RgbaImage, TextBlock, Tool,
    };
    use std::sync::Arc;

    use super::*;

    #[test]
    fn d131_a_text_border_is_its_glyphs_grown_round_by_the_width_and_none_without_masks() {
        let block = TextBlock::new(PointF { x: 0.0, y: 0.0 }, "hi".into(), 12.0, 1.0).unwrap();
        let (shape, style) = Look::default_for(Tool::Text)
            .with(Property::Border(true))
            .unwrap()
            .dress(Shape::Text(block));
        let mut history = History::new(Document::new(Arc::new(
            RgbaImage::new(32, 32, vec![0; 32 * 32 * 4]).unwrap(),
        )));
        history.apply(Command::Add { shape, style }).unwrap();
        let entry = &history.document().shapes()[0];
        let mut masks = TextMasks::default();
        let glyph = GlyphMask::new(10, 10, 4, 4, vec![255; 16], None).unwrap();
        masks.insert(entry.id, vec![glyph]);
        let patch = shape_border(entry, 32, 32, Some(&masks)).unwrap().unwrap();
        assert_eq!(
            (
                patch.left,
                patch.top,
                patch.pixels.width(),
                patch.pixels.height()
            ),
            (8, 8, 8, 8),
            "a 4 pixel square grown by 2 on every side"
        );
        let corner = &patch.pixels.pixels()[..4];
        assert_eq!(corner[3], 0, "growth is round, so the corner stays clear");
        assert_eq!(shape_border(entry, 32, 32, None).unwrap(), None);
    }
}
