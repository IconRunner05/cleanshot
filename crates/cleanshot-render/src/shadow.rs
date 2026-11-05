use cleanshot_core::{
    Color, GlyphMask, PointF, RectF, RgbaImage, Shape, ShapeEntry, Style, TextMasks, arrow_head,
    moved,
};
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

/// One shape moved onto a transparent pixmap over the part of the image it can paint, so a
/// patch costs what the shape covers rather than what the image does.
pub(crate) struct Alone {
    pub(crate) pixmap: Pixmap,
    pub(crate) entry: ShapeEntry,
    pub(crate) masks: Option<TextMasks>,
    left: u32,
    top: u32,
}

impl Alone {
    /// `entry` and its text `masks` moved onto a pixmap over the part of a `width` by `height`
    /// image within its stroke, border and `beyond` more pixels of its shape; `None` when that
    /// part misses the image. The move is by whole pixels, so what is drawn is what the whole
    /// image would show there.
    ///
    /// # Errors
    ///
    /// [`Error::TooLarge`] if that part exceeds the renderer's size limit.
    pub(crate) fn new(
        entry: &ShapeEntry,
        width: u32,
        height: u32,
        beyond: f32,
        masks: Option<&TextMasks>,
    ) -> Result<Option<Self>, Error> {
        let Some(area) = painted_box(entry, masks) else {
            return Ok(None);
        };
        let border = entry
            .style
            .border()
            .map_or(0.0, |border| f32::from(border.width.pixels()));
        let margin = entry.style.stroke_width + 2.0 * border + beyond + ANTIALIASING;
        let Some((left, top, right, bottom)) = pixel_span(area, margin, width, height) else {
            return Ok(None);
        };
        let (across, down) = (right - left, bottom - top);
        let pixmap = Pixmap::new(across, down).ok_or(Error::TooLarge {
            width: across,
            height: down,
        })?;
        #[expect(
            clippy::cast_precision_loss,
            reason = "image coordinates stay below 2^24, where f32 is exact"
        )]
        let offset = PointF {
            x: -(left as f32),
            y: -(top as f32),
        };
        let shift = |at: u32| i32::try_from(at).expect("a pixmap's side fits an i32");
        let masks = masks.map(|masks| {
            let mut moved_masks = TextMasks::default();
            if let Some(layers) = masks.layers(entry.id) {
                let layers = layers
                    .iter()
                    .map(|layer| {
                        GlyphMask::new(
                            layer.left() - shift(left),
                            layer.top() - shift(top),
                            layer.width(),
                            layer.height(),
                            layer.coverage().to_vec(),
                            layer.color(),
                        )
                        .expect("a moved copy keeps its coverage count")
                    })
                    .collect();
                moved_masks.insert(entry.id, layers);
            }
            moved_masks
        });
        Ok(Some(Self {
            pixmap,
            entry: ShapeEntry {
                id: entry.id,
                shape: moved(&entry.shape, offset),
                style: entry.style,
            },
            masks,
            left,
            top,
        }))
    }

    /// A transparent pixmap the size of this one.
    ///
    /// # Errors
    ///
    /// [`Error::TooLarge`] if this one is past the renderer's size limit.
    pub(crate) fn blank(&self) -> Result<Pixmap, Error> {
        let (width, height) = (self.pixmap.width(), self.pixmap.height());
        Pixmap::new(width, height).ok_or(Error::TooLarge { width, height })
    }

    /// What is drawn on `pixmap`, the size of this one, as a patch of the whole image.
    ///
    /// # Errors
    ///
    /// [`Error::Output`] if the cut pixels do not form an image.
    pub(crate) fn patch(&self, pixmap: &Pixmap) -> Result<Option<Patch>, Error> {
        Ok(drawn_patch(pixmap)?.map(|patch| Patch {
            left: patch.left + self.left,
            top: patch.top + self.top,
            pixels: patch.pixels,
        }))
    }
}

/// How far past its geometry antialiasing can touch a pixel.
const ANTIALIASING: f32 = 2.0;

/// The smallest box holding every point that places `entry`'s shape, its arrow head, its
/// bubble's tail and its text's glyph `masks`; `None` for a pen stroke with no points.
fn painted_box(entry: &ShapeEntry, masks: Option<&TextMasks>) -> Option<RectF> {
    let corners = |rect: RectF| {
        [
            PointF {
                x: rect.x,
                y: rect.y,
            },
            PointF {
                x: rect.x + rect.width,
                y: rect.y + rect.height,
            },
        ]
    };
    let points: Vec<PointF> = match &entry.shape {
        Shape::Line { from, to } => vec![*from, *to],
        Shape::Arrow { from, to } => {
            let mut points = vec![*from, *to];
            points.extend(arrow_head(*from, *to, entry.style.stroke_width));
            points
        }
        Shape::Rect { rect, .. }
        | Shape::Ellipse { rect, .. }
        | Shape::Highlight { rect, .. }
        | Shape::Redact { rect, .. }
        | Shape::Spotlight(rect) => corners(*rect).to_vec(),
        Shape::Pen(points) => points.clone(),
        Shape::Text(block) => {
            let mut points = corners(block.extent()).to_vec();
            for layer in masks
                .and_then(|masks| masks.layers(entry.id))
                .unwrap_or(&[])
            {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "glyph mask coordinates stay below 2^24, where f32 is exact"
                )]
                let rect = RectF {
                    x: layer.left() as f32,
                    y: layer.top() as f32,
                    width: layer.width() as f32,
                    height: layer.height() as f32,
                };
                points.extend(corners(rect));
            }
            points
        }
        Shape::Step(step) => corners(step.circle()).to_vec(),
        Shape::Bubble(bubble) => {
            let mut points = corners(bubble.outline()).to_vec();
            points.extend(bubble.tail_triangle().into_iter().flatten());
            points
        }
    };
    let (first, rest) = points.split_first()?;
    let (low, high) = rest.iter().fold((*first, *first), |(low, high), point| {
        (
            PointF {
                x: low.x.min(point.x),
                y: low.y.min(point.y),
            },
            PointF {
                x: high.x.max(point.x),
                y: high.y.max(point.y),
            },
        )
    });
    Some(RectF {
        x: low.x,
        y: low.y,
        width: high.x - low.x,
        height: high.y - low.y,
    })
}

/// The whole pixels of a `width` by `height` image within `margin` of `area`, as left, top,
/// right and bottom edges; `None` if there are none.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "each edge is clamped into the image before it becomes a pixel index"
)]
fn pixel_span(area: RectF, margin: f32, width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
    let edge = |at: f32, limit: u32| at.clamp(0.0, limit as f32) as u32;
    let left = edge((area.x - margin).floor(), width);
    let top = edge((area.y - margin).floor(), height);
    let right = edge((area.x + area.width + margin).ceil(), width);
    let bottom = edge((area.y + area.height + margin).ceil(), height);
    (right > left && bottom > top).then_some((left, top, right, bottom))
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
    fn p07_a_shadow_or_border_patch_is_what_drawing_on_the_whole_image_gives() {
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
    fn d114_a_line_drawn_or_selected_with_shadow_ticked_casts_one_dark_enough_to_see() {
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
