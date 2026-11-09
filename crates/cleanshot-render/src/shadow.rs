use cleanshot_core::{Color, ShapeEntry, Style};
use tiny_skia::Pixmap;

use crate::flatten::{draw, source_over};
use crate::patch::{Alone, covered};
use crate::{Error, Patch};

/// The shadow `entry` casts on an image of `width` by `height`, drawn alone as the export
/// draws it under the shape, for the editor to show. Text casts none here, having no
/// glyph masks. `None` when `entry` casts no shadow or it falls outside the image.
///
/// # Errors
///
/// [`Error::TooLarge`] if the image exceeds the renderer's size limit.
pub fn shape_shadow(entry: &ShapeEntry, width: u32, height: u32) -> Result<Option<Patch>, Error> {
    if !entry.style.shadow() {
        return Ok(None);
    }
    let (spread, _) = gaussian(Style::SHADOW_BLUR);
    #[expect(
        clippy::cast_precision_loss,
        reason = "the blur's spread and offset are a few pixels"
    )]
    let falls = (spread + Style::SHADOW_OFFSET) as f32;
    let Some(mut alone) = Alone::new(entry, width, height, falls, None)? else {
        return Ok(None);
    };
    draw(&mut alone.pixmap, &alone.entry, None)?;
    let mut shadow = alone.blank()?;
    cast_shadow(&mut shadow, &alone.pixmap);
    alone.patch(&shadow)
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

/// Paints onto `pixmap` the shadow of what `layer` covers: its coverage blurred by
/// [`Style::SHADOW_BLUR`], moved down by [`Style::SHADOW_OFFSET`], in black at
/// [`Style::SHADOW_ALPHA`]. `layer` is the size of `pixmap`.
pub(crate) fn cast_shadow(pixmap: &mut Pixmap, layer: &Pixmap) {
    let width = layer.width() as usize;
    let height = layer.height() as usize;
    let Some((left, right, top, bottom)) = covered(layer) else {
        return;
    };
    let (reach, weights) = gaussian(Style::SHADOW_BLUR);
    let total: f64 = weights.iter().sum();
    let spread = reach as usize;
    let first_column = left.saturating_sub(spread);
    let last_column = (right + spread).min(width - 1);
    let first_row = top.saturating_sub(spread);
    let last_row = (bottom + spread).min(height - 1);
    let columns = last_column - first_column + 1;
    // A row is finished once the last row of coverage that reaches it is added, so only as many
    // rows as the blur spans are held at a time.
    let mut held = vec![0.0; weights.len() * columns];
    let mut across = vec![0.0; columns];
    // The stretches of `across` a row reaches: most of a hollow shape's row is untouched.
    let mut reached: Vec<(usize, usize)> = Vec::new();
    let offset = Style::SHADOW_OFFSET as usize;
    let pixels = pixmap.pixels_mut();
    for y in top..=last_row + spread {
        if y <= bottom {
            reached.clear();
            let row = &layer.pixels()[y * width..(y + 1) * width];
            for x in (left..=right).filter(|&x| row[x].alpha() > 0) {
                let coverage = f64::from(row[x].alpha()) / 255.0;
                let first = x.saturating_sub(spread).max(first_column);
                let last = (x + spread).min(last_column);
                for to in first..=last {
                    across[to - first_column] += weights[x + spread - to] * coverage;
                }
                let (first, last) = (first - first_column, last - first_column);
                match reached.last_mut() {
                    Some((_, end)) if first <= *end + 1 => *end = last,
                    _ => reached.push((first, last)),
                }
            }
            for to in y.saturating_sub(spread).max(first_row)..=(y + spread).min(last_row) {
                let weight = weights[y + spread - to];
                let target = &mut held[to % weights.len() * columns..][..columns];
                for &(first, last) in &reached {
                    for (shade, blurred) in target[first..=last].iter_mut().zip(&across[first..]) {
                        *shade += weight * (blurred / total);
                    }
                }
            }
            for &(first, last) in &reached {
                across[first..=last].fill(0.0);
            }
        }
        let Some(done) = y.checked_sub(spread).filter(|done| *done >= first_row) else {
            continue;
        };
        let finished = &mut held[done % weights.len() * columns..][..columns];
        let below = done + offset;
        if below < height {
            let start = below * width + first_column;
            for (shade, pixel) in finished.iter().zip(&mut pixels[start..start + columns]) {
                if *shade == 0.0 {
                    continue;
                }
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "shade is in 0..=1, so the product is in 0..=255"
                )]
                let alpha =
                    (f64::from(Style::SHADOW_ALPHA) * (shade / total) * 255.0).round() as u8;
                *pixel = source_over(*pixel, BLACK, alpha);
            }
        }
        finished.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use cleanshot_core::{GlyphMask, PointF, RectF, RgbaImage, Shape, TextMasks};
    use tiny_skia::{Color as SkiaColor, Paint, Rect, Transform};

    use crate::patch::drawn_patch;

    use super::*;

    fn red(pixmap: &Pixmap, x: u32, y: u32) -> u8 {
        pixmap.pixel(x, y).unwrap().red()
    }

    fn byte(image: &RgbaImage, x: u32, y: u32, channel: usize) -> u8 {
        image.pixels()[(y * image.width() + x) as usize * 4 + channel]
    }

    #[test]
    fn a_shadow_or_border_patch_is_what_drawing_on_the_whole_image_gives() {
        use std::sync::Arc;

        use cleanshot_core::{
            BorderWidth, Bubble, Command, Document, Fill, HighlightStrength, History, Look,
            Property, TextBlock, Tool,
        };

        use crate::border::{draw_border, shape_border};

        let (width, height) = (120, 90);
        let point = |x, y| PointF { x, y };
        let rect = |x, y, width, height| RectF {
            x,
            y,
            width,
            height,
        };
        let block = |at| TextBlock::new(at, "hi".into(), 12.0, 4.0).unwrap();
        let shapes = [
            Shape::Arrow {
                from: point(10.3, 12.7),
                to: point(70.6, 40.2),
            },
            Shape::Arrow {
                from: point(90.5, 20.5),
                to: point(140.0, 60.0),
            },
            Shape::Line {
                from: point(-15.0, -8.0),
                to: point(30.4, 25.9),
            },
            Shape::Rect {
                rect: rect(20.5, 50.25, 30.0, 20.0),
                fill: Fill::Hollow,
            },
            Shape::Ellipse {
                rect: rect(60.1, 55.7, 40.0, 30.0),
                fill: Fill::Solid,
            },
            Shape::Pen(vec![point(5.5, 80.0), point(25.0, 60.3), point(45.8, 88.0)]),
            Shape::Text(block(point(70.0, 5.0))),
            Shape::Bubble(Bubble::new(block(point(30.0, 30.0)), point(8.0, 70.0))),
            Shape::Highlight {
                rect: rect(40.4, 10.6, 25.0, 9.0),
                strength: HighlightStrength::Strong,
            },
            Shape::Spotlight(rect(85.2, 40.8, 30.0, 49.5)),
        ];
        for shape in shapes {
            let tool = Tool::drawing(&shape);
            let (shape, style) = Look::default_for(tool)
                .with(Property::Shadow(true))
                .and_then(|look| look.with(Property::Border(true)))
                .and_then(|look| look.with(Property::BorderWidth(BorderWidth::Thick)))
                .unwrap()
                .dress(shape);
            let mut history = History::new(Document::new(Arc::new(
                RgbaImage::new(width, height, vec![255; (width * height * 4) as usize]).unwrap(),
            )));
            history.apply(Command::Add { shape, style }).unwrap();
            let entry = &history.document().shapes()[0];
            let mut masks = TextMasks::default();
            let glyph = GlyphMask::new(71, 6, 9, 11, vec![180; 99], None).unwrap();
            masks.insert(entry.id, vec![glyph]);

            let mut layer = Pixmap::new(width, height).unwrap();
            draw_border(&mut layer, entry, Some(&masks)).unwrap();
            assert_eq!(
                shape_border(entry, width, height, Some(&masks)).unwrap(),
                drawn_patch(&layer)
                    .unwrap()
                    .filter(|_| entry.style.border().is_some()),
                "{tool:?} border"
            );

            let mut layer = Pixmap::new(width, height).unwrap();
            draw(&mut layer, entry, None).unwrap();
            let mut shadow = Pixmap::new(width, height).unwrap();
            cast_shadow(&mut shadow, &layer);
            assert_eq!(
                shape_shadow(entry, width, height).unwrap(),
                drawn_patch(&shadow)
                    .unwrap()
                    .filter(|_| entry.style.shadow()),
                "{tool:?} shadow"
            );
        }
    }

    #[test]
    fn a_line_drawn_or_selected_with_shadow_ticked_casts_one_dark_enough_to_see() {
        use std::sync::Arc;

        use cleanshot_core::{
            Command, Document, History, Looks, PointF, Property, Shape, TextMasks, Tool,
        };

        let line = Shape::Line {
            from: PointF { x: 20.0, y: 40.0 },
            to: PointF { x: 180.0, y: 40.0 },
        };
        let white = RgbaImage::new(200, 80, vec![255; 200 * 80 * 4]).unwrap();
        let mut history = History::new(Document::new(Arc::new(white)));
        let mut looks = Looks::default();
        let (shape, style) = looks.look(Tool::Line).dress(line.clone());
        history.apply(Command::Add { shape, style }).unwrap();
        let unshadowed = history.document().shapes()[0].clone();
        let restyle = looks
            .set(Property::Shadow(true), Tool::Select, Some(&unshadowed))
            .unwrap()
            .expect("ticking Shadow on a selected line restyles it");
        history.apply(restyle).unwrap();
        let (shape, style) = looks.look(Tool::Line).dress(line);
        history.apply(Command::Add { shape, style }).unwrap();
        let export = crate::flatten(history.document(), &TextMasks::default()).unwrap();
        let below = 40 + Style::SHADOW_OFFSET + 2;
        for entry in history.document().shapes() {
            let patch = shape_shadow(entry, 200, 80).unwrap().expect("a shadow");
            let darkest = patch
                .pixels
                .pixels()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|[.., a]| *a)
                .max();
            assert!(
                darkest >= Some(64),
                "the editor's shadow is {darkest:?} of 255 at most"
            );
        }
        let shaded = 255 - byte(export.image(), 100, below, 0);
        assert!(
            shaded >= 48,
            "the export darkens white by {shaded} below the line"
        );
    }

    #[test]
    fn a_shadow_falls_below_the_shape_fades_out_and_leaves_the_rest_alone() {
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
    fn the_editor_gets_the_shadow_the_export_draws_and_text_gets_none_without_masks() {
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
        let block = TextBlock::new(PointF { x: 4.0, y: 4.0 }, "a".into(), 12.0, 1.0).unwrap();
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
