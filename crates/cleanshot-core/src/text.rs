use std::collections::HashMap;

use crate::{Color, ShapeId};

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
