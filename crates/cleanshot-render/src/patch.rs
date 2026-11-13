use cleanshot_core::{
    GlyphMask, PointF, RectF, RgbaImage, Shape, ShapeEntry, TextMasks, arrow_head, moved,
};
use tiny_skia::{IntRect, Pixmap};

use crate::Error;
use crate::flatten::straight;

/// Pixels an export draws at one place on the image: `pixels`, its top left corner at
/// `left`, `top`, for the editor to show as the export will.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Patch {
    pub left: u32,
    pub top: u32,
    pub pixels: RgbaImage,
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
            let area = match entry.style.background() {
                Some(_) => block.padded_extent(),
                None => block.extent(),
            };
            let mut points = corners(area).to_vec();
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
pub(crate) fn covered(pixmap: &Pixmap) -> Option<(usize, usize, usize, usize)> {
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
