use cleanshot_core::{Color, RgbaImage, ShapeEntry, Style};
use tiny_skia::{IntRect, Pixmap};

use crate::flatten::{draw, source_over, straight};
use crate::{Error, Patch};

/// The shadow `entry` casts on an image of `width` by `height`, drawn alone as the export
/// draws it under the shape (D-114), for the editor to show. Text casts none here, having no
/// glyph masks. `None` when `entry` casts no shadow or it falls outside the image.
///
/// # Errors
///
/// [`Error::TooLarge`] if the image exceeds the renderer's size limit.
pub fn shape_shadow(entry: &ShapeEntry, width: u32, height: u32) -> Result<Option<Patch>, Error> {
    if !entry.style.shadow() {
        return Ok(None);
    }
    let too_large = || Error::TooLarge { width, height };
    let mut layer = Pixmap::new(width, height).ok_or_else(too_large)?;
    draw(&mut layer, entry, None)?;
    let mut shadow = Pixmap::new(width, height).ok_or_else(too_large)?;
    cast_shadow(&mut shadow, &layer);
    drawn_patch(&shadow)
}

/// What is drawn on `pixmap`, cut to the smallest rectangle holding it; `None` if nothing is.
///
/// # Errors
///
/// [`Error::Output`] if the cut pixels do not form an image.
pub(crate) fn drawn_patch(pixmap: &Pixmap) -> Result<Option<Patch>, Error> {
    let Some(bounds) = covered_bounds(pixmap) else {
        return Ok(None);
    };
    let Some(patch) = pixmap.clone_rect(bounds) else {
        return Ok(None);
    };
    let pixels =
        RgbaImage::new(patch.width(), patch.height(), straight(&patch)).map_err(Error::Output)?;
    Ok(Some(Patch {
        left: bounds.x().unsigned_abs(),
        top: bounds.y().unsigned_abs(),
        pixels,
    }))
}

/// The smallest rectangle holding every pixel of `pixmap` that is not transparent.
fn covered_bounds(pixmap: &Pixmap) -> Option<IntRect> {
    let (left, right, top, bottom) = covered(pixmap)?;
    IntRect::from_ltrb(
        i32::try_from(left).ok()?,
        i32::try_from(top).ok()?,
        i32::try_from(right + 1).ok()?,
        i32::try_from(bottom + 1).ok()?,
    )
}

/// The first and last column and row of `pixmap` with any coverage.
fn covered(pixmap: &Pixmap) -> Option<(usize, usize, usize, usize)> {
    let width = pixmap.width() as usize;
    let mut bounds: Option<(usize, usize, usize, usize)> = None;
    for (y, row) in pixmap.pixels().chunks_exact(width).enumerate() {
        for x in (0..width).filter(|&x| row[x].alpha() > 0) {
            let (left, right, top, _) = bounds.unwrap_or((x, x, y, y));
            bounds = Some((left.min(x), right.max(x), top, y));
        }
    }
    bounds
}

const BLACK: Color = Color {
    r: 0,
    g: 0,
    b: 0,
    a: 255,
};

/// The weights of a Gaussian whose blur radius is `blur` pixels, twice its standard deviation,
/// for offsets `-reach..=reach`, with `reach`. The weights are not normalised.
pub(crate) fn gaussian(blur: u32) -> (u32, Vec<f64>) {
    let sigma = f64::from(blur) / 2.0;
    let reach = blur * 3 / 2;
    let weights = (-i64::from(reach)..=i64::from(reach))
        .map(|offset| {
            #[expect(clippy::cast_precision_loss, reason = "offsets are under 2^52")]
            let offset = offset as f64;
            (-(offset * offset) / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    (reach, weights)
}

/// Paints onto `pixmap` the shadow of what `layer` covers (D-114): its coverage blurred by
/// [`Style::SHADOW_BLUR`], moved down by [`Style::SHADOW_OFFSET`], in black at
/// [`Style::SHADOW_ALPHA`]. `layer` is the size of `pixmap`.
pub(crate) fn cast_shadow(pixmap: &mut Pixmap, layer: &Pixmap) {
    let width = layer.width() as usize;
    let height = layer.height() as usize;
    let coverage = |x: usize, y: usize| f64::from(layer.pixels()[y * width + x].alpha()) / 255.0;
    let Some((left, right, top, bottom)) = covered(layer) else {
        return;
    };
    let (reach, weights) = gaussian(Style::SHADOW_BLUR);
    let total: f64 = weights.iter().sum();
    let spread = reach as usize;
    let columns = left.saturating_sub(spread)..(right + spread + 1).min(width);
    let rows = top.saturating_sub(spread)..(bottom + spread + 1).min(height);
    let reach = i64::from(reach);
    let blur = |at: usize, sample: &dyn Fn(usize) -> f64, within: (usize, usize)| {
        (-reach..=reach)
            .zip(&weights)
            .filter_map(|(offset, weight)| {
                let index = usize::try_from(i64::try_from(at).ok()? + offset).ok()?;
                (within.0..=within.1)
                    .contains(&index)
                    .then(|| weight * sample(index))
            })
            .sum::<f64>()
            / total
    };
    let across: Vec<Vec<f64>> = (top..=bottom)
        .map(|y| {
            columns
                .clone()
                .map(|x| blur(x, &|column| coverage(column, y), (left, right)))
                .collect()
        })
        .collect();
    let offset = Style::SHADOW_OFFSET as usize;
    let pixels = pixmap.pixels_mut();
    for y in rows {
        let below = y + offset;
        if below >= height {
            continue;
        }
        for (column, x) in columns.clone().enumerate() {
            let shade = blur(y, &|row| across[row - top][column], (top, bottom));
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "shade is in 0..=1, so the product is in 0..=255"
            )]
            let alpha = (f64::from(Style::SHADOW_ALPHA) * shade * 255.0).round() as u8;
            let pixel = &mut pixels[below * width + x];
            *pixel = source_over(*pixel, BLACK, alpha);
        }
    }
}

#[cfg(test)]
mod tests {
    use tiny_skia::{Color as SkiaColor, Paint, Rect, Transform};

    use super::*;

    fn red(pixmap: &Pixmap, x: u32, y: u32) -> u8 {
        pixmap.pixel(x, y).unwrap().red()
    }

    fn byte(image: &RgbaImage, x: u32, y: u32, channel: usize) -> u8 {
        image.pixels()[(y * image.width() + x) as usize * 4 + channel]
    }

    #[test]
    fn d114_a_shadow_falls_below_the_shape_fades_out_and_leaves_the_rest_alone() {
        let mut layer = Pixmap::new(80, 60).unwrap();
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 0, 0, 255);
        layer.fill_rect(
            Rect::from_xywh(20.0, 20.0, 20.0, 10.0).unwrap(),
            &paint,
            Transform::identity(),
            None,
        );
        let mut pixmap = Pixmap::new(80, 60).unwrap();
        pixmap.fill(SkiaColor::WHITE);
        cast_shadow(&mut pixmap, &layer);
        let (below, above) = (red(&pixmap, 30, 31), red(&pixmap, 30, 18));
        assert!(below < above, "below {below}, above {above}");
        let darkest = (0..60).map(|y| red(&pixmap, 30, y)).min().unwrap();
        assert!(
            f32::from(255 - darkest) <= Style::SHADOW_ALPHA * 255.0 + 1.0,
            "{darkest}"
        );
        assert_eq!(red(&pixmap, 2, 2), 255, "far from the shape");
        assert_eq!(red(&pixmap, 30, 50), 255, "past the blur");
        assert_eq!(
            red(&pixmap, 16, 25),
            red(&pixmap, 43, 25),
            "left and right alike"
        );
    }

    #[test]
    fn d114_the_editor_gets_the_shadow_the_export_draws_and_text_gets_none_without_masks() {
        use std::sync::Arc;

        use cleanshot_core::{
            Command, Document, Fill, History, Look, Property, RectF, Shape, TextBlock, TextMasks,
            Tool,
        };

        let dressed = |shape: Shape, shadow| {
            let tool = Tool::drawing(&shape);
            Look::default_for(tool)
                .with(Property::Fill(Fill::Solid))
                .and_then(|look| look.with(Property::Shadow(shadow)))
                .unwrap()
                .dress(shape)
        };
        let box_at = RectF {
            x: 20.0,
            y: 10.0,
            width: 30.0,
            height: 20.0,
        };
        let (shape, style) = dressed(
            Shape::Rect {
                rect: box_at,
                fill: Fill::Solid,
            },
            true,
        );
        let white = RgbaImage::new(80, 60, vec![255; 80 * 60 * 4]).unwrap();
        let mut history = History::new(Document::new(Arc::new(white)));
        history.apply(Command::Add { shape, style }).unwrap();
        let entry = history.document().shapes()[0].clone();
        let flat = crate::flatten(history.document(), &TextMasks::default()).unwrap();
        let patch = shape_shadow(&entry, 80, 60).unwrap().unwrap();
        for (x, y) in [(35, 31), (35, 34), (52, 20), (18, 30)] {
            let alpha = byte(&patch.pixels, x - patch.left, y - patch.top, 3);
            let exported = byte(flat.image(), x, y, 0);
            assert!(
                (i32::from(255 - alpha) - i32::from(exported)).abs() <= 1,
                "({x}, {y}): shadow alpha {alpha}, export {exported}"
            );
        }
        let (shape, style) = dressed(
            Shape::Rect {
                rect: box_at,
                fill: Fill::Solid,
            },
            false,
        );
        let plain = ShapeEntry {
            style,
            shape,
            ..entry.clone()
        };
        assert_eq!(shape_shadow(&plain, 80, 60).unwrap(), None);
        let block = TextBlock::new(
            cleanshot_core::PointF { x: 4.0, y: 4.0 },
            "a".into(),
            12.0,
            1.0,
        )
        .unwrap();
        let (shape, style) = dressed(Shape::Text(block), true);
        assert!(style.shadow());
        let text = ShapeEntry {
            style,
            shape,
            ..entry
        };
        assert_eq!(shape_shadow(&text, 80, 60).unwrap(), None);
    }
}
