use std::collections::HashMap;
use std::ops::Range;

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

/// A text shape: lines broken where the user pressed Enter and, in a box of set width, where a
/// line would overflow it (F-34, D-119), each placed in the box by its alignment (D-129).
#[derive(Clone, Debug, PartialEq)]
pub struct TextBlock {
    at: PointF,
    text: String,
    size: f32,
    width: Width,
    /// Each line's width in ems, top to bottom, without the spaces a wrapped line ends in.
    line_widths: Vec<f32>,
    alignment: Alignment,
}

/// How wide a text block is: as wide as its widest line, or a width set for it, in ems, with
/// the byte offsets where [`wrap`] started a new line to fit it, and the height set for its
/// box, if any (D-128).
#[derive(Clone, Debug, PartialEq)]
enum Width {
    Measured,
    Wrapped {
        width: f32,
        height: Option<f32>,
        breaks: Vec<usize>,
    },
}

/// Where a text's lines sit in its box (D-129). A line narrower than the box moves across it;
/// the lines together move down a box set higher than they are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Alignment {
    pub horizontal: Horizontal,
    pub vertical: Vertical,
}

/// Where each line sits across its box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Horizontal {
    #[default]
    Left,
    Centre,
    Right,
}

/// Where the lines together sit down a box set higher than they are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Vertical {
    #[default]
    Top,
    Centre,
    Bottom,
}

impl Horizontal {
    /// The share of the room a line leaves in its box that goes before it.
    fn share_before(self) -> f32 {
        match self {
            Self::Left => 0.0,
            Self::Centre => 0.5,
            Self::Right => 1.0,
        }
    }
}

impl Vertical {
    /// The share of the room the lines leave in their box that goes above them.
    fn share_before(self) -> f32 {
        match self {
            Self::Top => 0.0,
            Self::Centre => 0.5,
            Self::Bottom => 1.0,
        }
    }
}

/// The size of a text's box, in ems of its text, set when the box was dragged out (D-126,
/// D-128).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxSize {
    pub width: f32,
    pub height: f32,
}

/// Why [`TextBlock::new`] or [`TextBlock::with_text`] refused its size or width.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum TextBlockError {
    #[error("a font size must be a positive number of pixels, but it is {0}")]
    SizeNotPositive(f32),
    #[error("a measured width must be a finite number of ems, at least 0, but it is {0}")]
    WidthNotMeasured(f32),
}

impl TextBlock {
    /// Left-aligned text whose first line box has its top-left corner at `at`, `size` pixels
    /// high per em, with `em_width` the layout width at size 1 of each of its lines, measured
    /// for this `text`; [`TextBlock::with_text`] measures each line on its own.
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
        let line_widths = text.split('\n').map(|_| em_width).collect();
        Ok(Self {
            at,
            text,
            size,
            width: Width::Measured,
            line_widths,
            alignment: Alignment::default(),
        })
    }

    /// This block holding `text` instead, in the same place, size and box, with `measure` giving
    /// a line's width in ems as GPUI lays it out.
    ///
    /// # Errors
    ///
    /// [`TextBlockError::WidthNotMeasured`] if, outside a box, `measure` gives a line a width
    /// that is not a finite number of ems, at least 0.
    pub fn with_text(
        &self,
        text: String,
        measure: impl Fn(&str) -> f32,
    ) -> Result<Self, TextBlockError> {
        match self.width {
            Width::Measured => {
                let mut line_widths = Vec::new();
                for line in text.split('\n') {
                    let width = measure(line);
                    if !(width.is_finite() && width >= 0.0) {
                        return Err(TextBlockError::WidthNotMeasured(width));
                    }
                    line_widths.push(width);
                }
                Ok(Self {
                    at: self.at,
                    text,
                    size: self.size,
                    width: Width::Measured,
                    line_widths,
                    alignment: self.alignment,
                })
            }
            Width::Wrapped { width, height, .. } => Ok(self.wrapped(text, width, height, measure)),
        }
    }

    /// This block with its left edge at `x`, wrapped in a box `width` ems wide, which the caller
    /// has checked is positive (D-119), and at least `height` ems high if set (D-128).
    pub(crate) fn in_box(
        &self,
        x: f32,
        width: f32,
        height: Option<f32>,
        measure: impl Fn(&str) -> f32,
    ) -> Self {
        Self {
            at: PointF { x, y: self.at.y },
            ..self.wrapped(self.text.clone(), width, height, measure)
        }
    }

    /// `text` in this block's place, size and alignment, wrapped in a box `width` ems wide and at
    /// least `height` ems high if set.
    fn wrapped(
        &self,
        text: String,
        width: f32,
        height: Option<f32>,
        measure: impl Fn(&str) -> f32,
    ) -> Self {
        let breaks = wrap(&text, width, &measure);
        let line_widths = line_ranges(&text, &breaks)
            .map(|line| measure(text[line].trim_end_matches(' ')))
            .collect();
        Self {
            at: self.at,
            text,
            size: self.size,
            width: Width::Wrapped {
                width,
                height,
                breaks,
            },
            line_widths,
            alignment: self.alignment,
        }
    }

    /// Where this block's lines sit in its box (D-129).
    #[must_use]
    pub fn alignment(&self) -> Alignment {
        self.alignment
    }

    pub(crate) fn with_alignment(&self, alignment: Alignment) -> Self {
        Self {
            alignment,
            ..self.clone()
        }
    }

    /// The width in ems of the box this block wraps in, or `None` if only Enter breaks its lines.
    #[must_use]
    pub fn wrap_width(&self) -> Option<f32> {
        match self.width {
            Width::Measured => None,
            Width::Wrapped { width, .. } => Some(width),
        }
    }

    /// The height in ems set for the box this block wraps in, if any (D-128).
    pub(crate) fn box_height(&self) -> Option<f32> {
        match self.width {
            Width::Measured => None,
            Width::Wrapped { height, .. } => height,
        }
    }

    /// The byte range of each line in [`TextBlock::text`], top to bottom, without the `\n` that
    /// ends a line the user broke.
    pub fn line_ranges(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        let breaks: &[usize] = match &self.width {
            Width::Measured => &[],
            Width::Wrapped { breaks, .. } => breaks,
        };
        line_ranges(&self.text, breaks)
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

    /// The layout box of every line, in image pixels, at least as high as the box's set height
    /// (D-128).
    #[must_use]
    pub fn extent(&self) -> RectF {
        let lines_height = self.lines_height();
        let height = match self.width {
            Width::Measured => lines_height,
            Width::Wrapped { height, .. } => {
                height.map_or(lines_height, |set| set.max(lines_height))
            }
        };
        RectF {
            x: self.at.x,
            y: self.at.y,
            width: self.box_width() * self.size,
            height: height * self.size,
        }
    }

    /// The width of the box in ems: the set width, or the widest line's.
    fn box_width(&self) -> f32 {
        match self.width {
            Width::Measured => self.line_widths.iter().copied().fold(0.0, f32::max),
            Width::Wrapped { width, .. } => width,
        }
    }

    /// The height of the lines together, in ems.
    fn lines_height(&self) -> f32 {
        self.line_widths
            .iter()
            .fold(0.0, |height, _| height + LINE_HEIGHT)
    }

    /// The top of the first line box, in image pixels: the box's top, or lower in a box set
    /// higher than the lines, as the vertical alignment puts them (D-129).
    fn lines_top(&self) -> f32 {
        let spare = self.extent().height - self.lines_height() * self.size;
        self.at.y + spare * self.alignment.vertical.share_before()
    }

    /// The line, from 0, whose line box holds the height `y` in image pixels; the first line above
    /// the block, the last below it.
    #[must_use]
    pub fn line_at(&self, y: f32) -> usize {
        let step = LINE_HEIGHT * self.size;
        self.line_ranges()
            .skip(1)
            .scan(self.lines_top() + step, |top, _| {
                let line_top = *top;
                *top += step;
                Some(line_top)
            })
            .take_while(|line_top| *line_top <= y)
            .count()
    }

    /// Each line's text and where its baseline starts, in image pixels: the line box's top,
    /// plus half the leading Inter leaves in it, plus Inter's ascent; across, the line moves
    /// into the room it leaves in the box as the alignment puts it (D-129).
    pub fn lines(&self) -> impl Iterator<Item = (&str, PointF)> {
        let first = baseline(self.lines_top(), self.size);
        let step = LINE_HEIGHT * self.size;
        let box_width = self.box_width();
        let share = self.alignment.horizontal.share_before();
        self.line_ranges()
            .zip(&self.line_widths)
            .scan(first, move |y, (line, width)| {
                let room = (box_width - width).max(0.0) * self.size;
                let baseline = PointF {
                    x: self.at.x + room * share,
                    y: *y,
                };
                *y += step;
                Some((&self.text[line], baseline))
            })
    }

    pub(crate) fn moved_to(&self, at: PointF) -> Self {
        Self { at, ..self.clone() }
    }
}

/// The byte range of each line in `text`, broken at each `\n` and at each of `breaks`.
fn line_ranges<'a>(text: &'a str, breaks: &'a [usize]) -> impl Iterator<Item = Range<usize>> + 'a {
    let mut breaks = breaks.iter().copied().peekable();
    let mut start = Some(0);
    std::iter::from_fn(move || {
        let from = start?;
        let hard_end = text[from..].find('\n').map(|at| from + at);
        if let Some(soft) = breaks.next_if(|soft| hard_end.is_none_or(|end| *soft < end)) {
            start = Some(soft);
            return Some(from..soft);
        }
        start = hard_end.map(|end| end + 1);
        Some(from..hard_end.unwrap_or(text.len()))
    })
}

/// The byte offsets in `text` where a line starts because the line before it would overflow a
/// box `width` ems wide, with `measure` giving a line's width in ems. Each is the start of a
/// word; a word wider than the box gets a line of its own.
fn wrap(text: &str, width: f32, measure: impl Fn(&str) -> f32) -> Vec<usize> {
    let fits = |line: &str| measure(line.trim_end_matches(' ')) <= width;
    let mut breaks = Vec::new();
    let mut line_start = 0;
    for line in text.split('\n') {
        let end = line_start + line.len();
        let word_starts = line
            .char_indices()
            .filter(|&(at, letter)| {
                letter != ' '
                    && line[..at].ends_with(' ')
                    && !line[..at].trim_start_matches(' ').is_empty()
            })
            .map(|(at, _)| line_start + at);
        let mut start = line_start;
        let mut last_fit = None;
        for at in word_starts.chain([end]) {
            if fits(&text[start..at]) {
                last_fit = Some(at);
                continue;
            }
            if let Some(fit) = last_fit {
                breaks.push(fit);
                start = fit;
                if fits(&text[start..at]) {
                    last_fit = Some(at);
                    continue;
                }
            }
            last_fit = None;
            if at < end {
                breaks.push(at);
                start = at;
            }
        }
        line_start = end + 1;
    }
    breaks
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

/// The colour text is drawn in on `fill`, as a step's number and a bubble's text are: black on
/// a light colour, white on a dark one, by the colour's luma (BT.601).
#[must_use]
pub fn text_on(fill: Color) -> Color {
    let luma = 299 * u32::from(fill.r) + 587 * u32::from(fill.g) + 114 * u32::from(fill.b);
    let level = if luma > 150_000 { 0 } else { 255 };
    Color {
        r: level,
        g: level,
        b: level,
        a: 255,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    #[test]
    fn f41_a_step_number_is_white_on_dark_colours_and_black_on_light_ones() {
        let white = rgb(255, 255, 255);
        let black = rgb(0, 0, 0);
        for dark in [rgb(255, 59, 48), rgb(0, 122, 255), rgb(175, 82, 222), black] {
            assert_eq!(text_on(dark), white, "{dark:?}");
        }
        for light in [rgb(255, 230, 0), rgb(255, 149, 0), white] {
            assert_eq!(text_on(light), black, "{light:?}");
        }
    }

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

    fn half_an_em_a_letter(line: &str) -> f32 {
        line.chars().fold(0.0, |width, _| width + 0.5)
    }

    fn wrapped(text: &str, width: f32) -> TextBlock {
        let at = PointF { x: 10.0, y: 20.0 };
        TextBlock::new(at, text.into(), 16.0, 0.0).unwrap().in_box(
            10.0,
            width,
            None,
            half_an_em_a_letter,
        )
    }

    fn lines(block: &TextBlock) -> Vec<&str> {
        block.lines().map(|(line, _)| line).collect()
    }

    #[test]
    fn d129_each_line_sits_across_its_box_and_the_lines_down_a_taller_box_as_aligned() {
        let at = PointF { x: 10.0, y: 20.0 };
        let measured = TextBlock::new(at, String::new(), 20.0, 0.0)
            .unwrap()
            .with_text("a\nbbb".into(), half_an_em_a_letter)
            .unwrap();
        let boxed = measured.in_box(10.0, 4.0, Some(5.0), half_an_em_a_letter);
        let placed = |block: &TextBlock, horizontal, vertical| {
            let aligned = block.with_alignment(Alignment {
                horizontal,
                vertical,
            });
            let starts: Vec<_> = aligned.lines().map(|(_, start)| start.x).collect();
            let first_top = aligned.lines().next().unwrap().1.y - baseline(0.0, 20.0);
            (starts, first_top)
        };
        assert_eq!(
            placed(&boxed, Horizontal::Left, Vertical::Top),
            (vec![10.0, 10.0], 20.0)
        );
        assert_eq!(
            placed(&boxed, Horizontal::Centre, Vertical::Centre),
            (vec![45.0, 35.0], 45.0),
            "a 4 em box round lines of 0.5 and 1.5 ems, 100 px high round 50 px of lines"
        );
        assert_eq!(
            placed(&boxed, Horizontal::Right, Vertical::Bottom),
            (vec![80.0, 60.0], 70.0)
        );
        assert_eq!(
            placed(&measured, Horizontal::Right, Vertical::Bottom),
            (vec![30.0, 10.0], 20.0),
            "unboxed text aligns to its widest line and is as high as its lines"
        );
        let low = boxed.with_alignment(Alignment {
            horizontal: Horizontal::Left,
            vertical: Vertical::Bottom,
        });
        assert_eq!(low.line_at(75.0), 0, "the first line's box starts at 70");
        assert_eq!(low.line_at(96.0), 1);
        assert_eq!(low.extent(), boxed.extent(), "alignment moves no box");
    }

    #[test]
    fn d119_a_line_wraps_after_the_last_word_that_fits_the_box_and_enter_still_breaks() {
        let block = wrapped("one two three\nfour five", 3.5);
        assert_eq!(lines(&block), ["one two ", "three", "four ", "five"]);
        assert_eq!(
            block.line_ranges().collect::<Vec<_>>(),
            [0..8, 8..13, 14..19, 19..23]
        );
        assert_eq!(block.line_at(20.0 + 2.5 * 20.0), 2);
        assert_eq!(block.wrap_width(), Some(3.5));
        assert_eq!(
            block.extent(),
            RectF {
                x: 10.0,
                y: 20.0,
                width: 56.0,
                height: 80.0,
            }
        );
        assert_eq!(lines(&block.with_size(32.0)), lines(&block));
    }

    #[test]
    fn d119_a_word_wider_than_the_box_overflows_on_a_line_of_its_own() {
        assert_eq!(
            lines(&wrapped("a verylongword b", 3.0)),
            ["a ", "verylongword ", "b"]
        );
        assert_eq!(lines(&wrapped("   hi there", 2.5)), ["   hi ", "there"]);
        assert_eq!(lines(&wrapped("", 1.0)), [""]);
        assert_eq!(lines(&wrapped("fits", 9.0)), ["fits"]);
    }

    #[test]
    fn d119_text_retyped_in_a_box_wraps_in_it_and_unboxed_text_is_measured_afresh() {
        let boxed = wrapped("one", 3.5)
            .with_text("one two three".into(), half_an_em_a_letter)
            .unwrap();
        assert_eq!(lines(&boxed), ["one two ", "three"]);
        let at = PointF { x: 0.0, y: 0.0 };
        let measured = TextBlock::new(at, "Hi".into(), 24.0, 1.0).unwrap();
        assert_eq!(measured.wrap_width(), None);
        let retyped = measured
            .with_text("abc\nabcdef".into(), half_an_em_a_letter)
            .unwrap();
        assert_eq!(retyped.extent().width, 3.0 * 24.0);
        assert!(matches!(
            measured.with_text("x".into(), |_| f32::NAN),
            Err(TextBlockError::WidthNotMeasured(width)) if width.is_nan()
        ));
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
