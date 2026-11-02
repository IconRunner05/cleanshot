use std::collections::HashMap;

use crate::{Color, PointF, RectF, ShapeId};

/// A line's height as a multiple of the font size.
pub const LINE_HEIGHT: f32 = 1.25;

/// The font size of a new text box, in image pixels.
pub const DEFAULT_TEXT_SIZE: f32 = 24.0;

/// Inter 4.1 in its semibold weight, the one font text shapes are drawn in, on screen and in the export.
pub static INTER: &[u8] = include_bytes!("../fonts/Inter-SemiBold.ttf");

/// Inter's design units per em.
pub const INTER_UNITS_PER_EM: u16 = 2048;
/// Inter's ascent above the baseline, in design units; its hhea, typo and win metrics agree.
pub const INTER_ASCENT: u16 = 1984;
/// Inter's descent below the baseline, in design units; its hhea, typo and win metrics agree.
pub const INTER_DESCENT: u16 = 494;

/// A text shape: lines broken only where the user pressed Enter, with the width DirectWrite or
/// GPUI measured for exactly this text (F-34).
#[derive(Clone, Debug, PartialEq)]
pub struct TextBlock {
    at: PointF,
    text: String,
    size: f32,
    em_width: f32,
}

/// Why [`TextBlock::new`] refused its size or width.
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

    /// The top-left corner of the first line box, in image pixels.
    #[must_use]
    pub fn at(&self) -> PointF {
        self.at
    }

    /// The text, with `\n` where the user broke a line.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The font size in image pixels.
    #[must_use]
    pub fn size(&self) -> f32 {
        self.size
    }

    /// This block at `size`, which the caller has checked is positive; its width in ems
    /// holds at any size.
    pub(crate) fn with_size(&self, size: f32) -> Self {
        Self {
            size,
            ..self.clone()
        }
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

    /// The line, from 0, whose line box holds the height `y` in image pixels; the first line above
    /// the block, the last below it.
    #[must_use]
    pub fn line_at(&self, y: f32) -> usize {
        let step = LINE_HEIGHT * self.size;
        self.text
            .split('\n')
            .skip(1)
            .scan(self.at.y + step, |top, _| {
                let line_top = *top;
                *top += step;
                Some(line_top)
            })
            .take_while(|line_top| *line_top <= y)
            .count()
    }

    /// Each line's text and where its baseline starts, in image pixels: the line box's top,
    /// plus half the leading Inter leaves in it, plus Inter's ascent.
    pub fn lines(&self) -> impl Iterator<Item = (&str, PointF)> {
        let first = baseline(self.at.y, self.size);
        let step = LINE_HEIGHT * self.size;
        let x = self.at.x;
        self.text.split('\n').scan(first, move |y, line| {
            let baseline = PointF { x, y: *y };
            *y += step;
            Some((line, baseline))
        })
    }

    pub(crate) fn moved_to(&self, at: PointF) -> Self {
        Self { at, ..self.clone() }
    }
}

/// The baseline of a line whose line box has its top at `top`, in Inter at `size` pixels per
/// em: the top, plus half the leading Inter leaves in the box, plus Inter's ascent.
#[must_use]
pub fn baseline(top: f32, size: f32) -> f32 {
    let em = f32::from(INTER_UNITS_PER_EM);
    let (ascent, descent) = (f32::from(INTER_ASCENT) / em, f32::from(INTER_DESCENT) / em);
    top + ((LINE_HEIGHT - ascent - descent) / 2.0 + ascent) * size
}

/// Text a shape shows, as the screen and the export both lay it out (F-34, F-41).
#[derive(Clone, Debug, PartialEq)]
pub enum Label<'a> {
    /// Laid out from its top-left corner, as a text shape is.
    Block(&'a TextBlock),
    /// One line, `size` pixels per em, horizontally centred on `centre_x`, its line box's top
    /// at `top`.
    Centred {
        text: String,
        size: f32,
        centre_x: f32,
        top: f32,
    },
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

/// Why [`GlyphMask::new`] refused its coverage.
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
    fn f34_each_line_has_its_baseline_half_the_leading_plus_the_ascent_below_its_top() {
        let block =
            TextBlock::new(PointF { x: 5.0, y: 0.0 }, "a\n\nb".into(), 2048.0, 1.0).unwrap();
        let leading = 2560.0 - 1984.0 - 494.0;
        let first = leading / 2.0 + 1984.0;
        assert_eq!(
            block.lines().collect::<Vec<_>>(),
            [
                ("a", PointF { x: 5.0, y: first }),
                (
                    "",
                    PointF {
                        x: 5.0,
                        y: first + 2560.0
                    }
                ),
                (
                    "b",
                    PointF {
                        x: 5.0,
                        y: first + 5120.0
                    }
                ),
            ]
        );
    }

    fn table(font: &[u8], tag: [u8; 4]) -> usize {
        let be32 = |at: usize| u32::from_be_bytes(font[at..at + 4].try_into().unwrap());
        let count = usize::from(u16::from_be_bytes([font[4], font[5]]));
        (0..count)
            .map(|index| 12 + 16 * index)
            .find(|&record| &font[record..record + 4] == tag.as_slice())
            .map(|record| usize::try_from(be32(record + 8)).unwrap())
            .unwrap()
    }

    #[test]
    fn f34_the_metric_constants_are_the_embedded_fonts_own() {
        let be16 = |at: usize| u16::from_be_bytes([INTER[at], INTER[at + 1]]);
        let (head, hhea, os2) = (
            table(INTER, *b"head"),
            table(INTER, *b"hhea"),
            table(INTER, *b"OS/2"),
        );
        assert_eq!(be16(head + 18), INTER_UNITS_PER_EM);
        assert_eq!(be16(hhea + 4), INTER_ASCENT);
        assert_eq!(be16(hhea + 6).wrapping_neg(), INTER_DESCENT);
        assert_eq!(be16(os2 + 68), INTER_ASCENT);
        assert_eq!(be16(os2 + 70).wrapping_neg(), INTER_DESCENT);
        assert_eq!(
            (be16(os2 + 74), be16(os2 + 76)),
            (INTER_ASCENT, INTER_DESCENT)
        );
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

    #[test]
    fn f34_a_height_falls_on_the_line_whose_box_holds_it_or_the_nearest_line_outside() {
        let block =
            TextBlock::new(PointF { x: 0.0, y: 100.0 }, "a\nb\nc".into(), 20.0, 1.0).unwrap();
        let top = |line: f32| 100.0 + line * LINE_HEIGHT * 20.0;
        let heights = [0.0, top(0.5), top(1.0), top(2.0) - 0.5, top(2.5), 500.0];
        assert_eq!(heights.map(|y| block.line_at(y)), [0, 0, 1, 1, 2, 2]);
    }
}
