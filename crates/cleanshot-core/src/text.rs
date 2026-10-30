use std::collections::HashMap;

use crate::{Color, PointF, RectF, ShapeId};

/// A line's height as a multiple of the font size.
pub const LINE_HEIGHT: f32 = 1.25;

/// A text shape: lines broken only where the user pressed Enter, with the width DirectWrite or
/// GPUI measured for exactly this text (F-34).
#[derive(Clone, Debug, PartialEq)]
pub struct TextBlock {
    at: PointF,
    text: String,
    size: f32,
    em_width: f32,
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum TextBlockError {
    #[error("a font size must be a positive number of pixels, but it is {0}")]
    SizeNotPositive(f32),
    #[error("a measured width must be a finite number of ems, at least 0, but it is {0}")]
    WidthNotMeasured(f32),
}

impl TextBlock {
    /// Text whose first line box has its top-left corner at `at`, `size` pixels high per em,
    /// with `em_width` the widest line's layout width at size 1, measured for this `text`.
    ///
    /// # Errors
    ///
    /// [`TextBlockError::SizeNotPositive`] unless `size` is finite and above 0;
    /// [`TextBlockError::WidthNotMeasured`] unless `em_width` is finite and at least 0.
    pub fn new(at: PointF, text: String, size: f32, em_width: f32) -> Result<Self, TextBlockError> {
        if !(size.is_finite() && size > 0.0) {
            return Err(TextBlockError::SizeNotPositive(size));
        }
        if !(em_width.is_finite() && em_width >= 0.0) {
            return Err(TextBlockError::WidthNotMeasured(em_width));
        }
        Ok(Self {
            at,
            text,
            size,
            em_width,
        })
    }

    #[must_use]
    pub fn at(&self) -> PointF {
        self.at
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The font size in image pixels.
    #[must_use]
    pub fn size(&self) -> f32 {
        self.size
    }

    /// The layout box of every line, in image pixels.
    #[must_use]
    pub fn extent(&self) -> RectF {
        let lines = self.text.split('\n').fold(0.0, |lines, _| lines + 1.0);
        RectF {
            x: self.at.x,
            y: self.at.y,
            width: self.em_width * self.size,
            height: lines * LINE_HEIGHT * self.size,
        }
    }

    pub(crate) fn moved_to(&self, at: PointF) -> Self {
        Self { at, ..self.clone() }
    }
}

/// The 8-bit coverage of one glyph layer of a text shape, in image pixels, as DirectWrite
/// rasterised it for export (F-34).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphMask {
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    coverage: Vec<u8>,
    color: Option<Color>,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum GlyphMaskError {
    #[error("a {width}x{height} mask needs {width}*{height} bytes, but {len} were given")]
    CoverageCountMismatch { width: u32, height: u32, len: usize },
}

impl GlyphMask {
    /// A mask whose top-left pixel is at (`left`, `top`) in the image, with one coverage byte per
    /// pixel, row by row. `color` is `None` for the shape's own colour, or a colour glyph
    /// layer's colour.
    ///
    /// # Errors
    ///
    /// [`GlyphMaskError::CoverageCountMismatch`] unless `coverage` holds `width * height` bytes.
    pub fn new(
        left: i32,
        top: i32,
        width: u32,
        height: u32,
        coverage: Vec<u8>,
        color: Option<Color>,
    ) -> Result<Self, GlyphMaskError> {
        if coverage.len() as u128 != u128::from(width) * u128::from(height) {
            return Err(GlyphMaskError::CoverageCountMismatch {
                width,
                height,
                len: coverage.len(),
            });
        }
        Ok(Self {
            left,
            top,
            width,
            height,
            coverage,
            color,
        })
    }

    #[must_use]
    pub fn left(&self) -> i32 {
        self.left
    }

    #[must_use]
    pub fn top(&self) -> i32 {
        self.top
    }

    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// One byte per pixel, row by row, 0 for none and 255 for full.
    #[must_use]
    pub fn coverage(&self) -> &[u8] {
        &self.coverage
    }

    /// `None` paints in the shape's colour; `Some` is a colour glyph layer's own colour.
    #[must_use]
    pub fn color(&self) -> Option<Color> {
        self.color
    }
}

/// Each text shape's glyph layers, in painting order, handed from DirectWrite to the export.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextMasks(HashMap<ShapeId, Vec<GlyphMask>>);

impl TextMasks {
    /// Records `layers`, in painting order, as all the glyph layers of the text shape `id`; an
    /// empty list records a shape with no ink, such as one of only spaces.
    pub fn insert(&mut self, id: ShapeId, layers: Vec<GlyphMask>) {
        self.0.insert(id, layers);
    }

    /// The layers recorded for the text shape `id`, or `None` if none were.
    #[must_use]
    pub fn layers(&self, id: ShapeId) -> Option<&[GlyphMask]> {
        self.0.get(&id).map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f34_a_glyph_mask_needs_one_coverage_byte_per_pixel() {
        assert!(GlyphMask::new(-3, 4, 2, 3, vec![0; 6], None).is_ok());
        assert!(GlyphMask::new(0, 0, 0, 0, Vec::new(), None).is_ok());
        assert_eq!(
            GlyphMask::new(0, 0, 2, 3, vec![0; 5], None),
            Err(GlyphMaskError::CoverageCountMismatch {
                width: 2,
                height: 3,
                len: 5
            })
        );
        assert!(GlyphMask::new(0, 0, u32::MAX, u32::MAX, vec![0; 1], None).is_err());
    }

    #[test]
    fn f34_a_text_block_needs_a_positive_size_and_a_measured_width() {
        let at = PointF { x: 0.0, y: 0.0 };
        let block = |size, em_width| TextBlock::new(at, "Hi".into(), size, em_width);
        assert!(block(24.0, 0.0).is_ok());
        assert_eq!(block(0.0, 1.0), Err(TextBlockError::SizeNotPositive(0.0)));
        assert!(block(f32::NAN, 1.0).is_err());
        assert!(block(f32::INFINITY, 1.0).is_err());
        assert_eq!(
            block(24.0, -0.5),
            Err(TextBlockError::WidthNotMeasured(-0.5))
        );
        assert!(block(24.0, f32::NAN).is_err());
    }

    #[test]
    fn f34_a_text_extent_is_its_measured_width_by_one_line_height_per_line() {
        let at = PointF { x: 10.0, y: 20.0 };
        let block = |text: &str| TextBlock::new(at, text.into(), 16.0, 2.5).unwrap();
        let extent = |width, height| RectF {
            x: 10.0,
            y: 20.0,
            width,
            height,
        };
        assert_eq!(block("one").extent(), extent(40.0, 20.0));
        assert_eq!(block("one\ntwo\n").extent(), extent(40.0, 60.0));
        assert_eq!(block("").extent(), extent(40.0, 20.0));
    }

    #[test]
    fn f34_text_masks_keep_each_shapes_layers_in_order_and_record_a_shape_with_none() {
        let red = Color {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        };
        let mask = |left, color| GlyphMask::new(left, 0, 1, 1, vec![255], color).unwrap();
        let (first, second) = (ShapeId(1), ShapeId(2));
        let mut masks = TextMasks::default();
        masks.insert(first, vec![mask(0, None), mask(1, Some(red))]);
        masks.insert(second, vec![mask(5, None)]);
        masks.insert(second, Vec::new());
        let layers = masks.layers(first).unwrap();
        assert_eq!(
            layers.iter().map(GlyphMask::left).collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(layers[1].color(), Some(red));
        assert_eq!(masks.layers(second), Some([].as_slice()));
        assert_eq!(masks.layers(ShapeId(3)), None);
    }
}
