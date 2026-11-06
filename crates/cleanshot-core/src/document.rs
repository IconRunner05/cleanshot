use std::f32::consts::{FRAC_PI_6, PI};
use std::sync::Arc;

use crate::bubble::Bubble;
use crate::step::Step;
use crate::text::{Label, TextBlock};

/// A capture with annotations on top. All coordinates are in base-image physical pixels.
/// Changed only through [`crate::History`].
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub(crate) base: Arc<RgbaImage>,
    pub(crate) shapes: Vec<ShapeEntry>,
    pub(crate) crop: Option<RectF>,
    pub(crate) frame: Frame,
    pub(crate) dim: SpotlightDim,
}

impl Document {
    /// A document showing `base` with no shapes, no crop and no frame.
    #[must_use]
    pub fn new(base: Arc<RgbaImage>) -> Self {
        Self {
            base,
            shapes: Vec::new(),
            crop: None,
            frame: Frame::default(),
            dim: SpotlightDim::Medium,
        }
    }

    /// The capture, never changed by an edit.
    #[must_use]
    pub fn base(&self) -> &Arc<RgbaImage> {
        &self.base
    }

    /// In drawing order, so the last shape is on top.
    #[must_use]
    pub fn shapes(&self) -> &[ShapeEntry] {
        &self.shapes
    }

    /// Whether a shape with `id` is in the document.
    #[must_use]
    pub fn contains(&self, id: ShapeId) -> bool {
        self.shapes.iter().any(|entry| entry.id == id)
    }

    /// The shape with `id`, if it is in the document.
    #[must_use]
    pub fn shape(&self, id: ShapeId) -> Option<&ShapeEntry> {
        self.shapes.iter().find(|entry| entry.id == id)
    }

    /// The part of the base image an export keeps, inside the image with a positive size;
    /// `None` keeps all of it.
    #[must_use]
    pub fn crop(&self) -> Option<RectF> {
        self.crop
    }

    /// The part of the base image an export keeps: the crop, or else all of it (F-37).
    #[must_use]
    pub fn kept_area(&self) -> RectF {
        self.crop.unwrap_or(self.image_area())
    }

    /// The whole base image, in image pixels.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "image sides are far below 2^24, where f32 stops being exact"
    )]
    pub fn image_area(&self) -> RectF {
        RectF {
            x: 0.0,
            y: 0.0,
            width: self.base.width() as f32,
            height: self.base.height() as f32,
        }
    }

    /// The border and shadow an export draws around the image (F-44).
    #[must_use]
    pub fn frame(&self) -> Frame {
        self.frame
    }

    /// How dark the image is outside every spotlight (F-43, D-123).
    #[must_use]
    pub fn dim(&self) -> SpotlightDim {
        self.dim
    }

    /// The text each shape shows, in drawing order, with the shape showing it.
    pub fn labels(&self) -> impl Iterator<Item = (ShapeId, Label<'_>)> {
        let shapes = self.shapes.iter().map(|entry| &entry.shape);
        self.shapes
            .iter()
            .zip(step_numbers(shapes))
            .filter_map(|(entry, number)| {
                let label = match &entry.shape {
                    Shape::Text(block) => Label::Block(block),
                    Shape::Step(step) => step.label(number),
                    Shape::Bubble(bubble) => Label::Block(bubble.block()),
                    Shape::Arrow { .. }
                    | Shape::Line { .. }
                    | Shape::Rect { .. }
                    | Shape::Ellipse { .. }
                    | Shape::Pen(_)
                    | Shape::Highlight { .. }
                    | Shape::Redact { .. }
                    | Shape::Spotlight(_) => return None,
                };
                Some((entry.id, label))
            })
    }
}

/// For each of `shapes`, in order, how many of them up to and including it are steps: a
/// step's number, from 1 (F-41).
pub fn step_numbers<'a>(
    shapes: impl IntoIterator<Item = &'a Shape>,
) -> impl Iterator<Item = usize> {
    shapes.into_iter().scan(0, |steps, shape| {
        if let Shape::Step(_) = shape {
            *steps += 1;
        }
        Some(*steps)
    })
}

/// The parts of `base` the spotlights among `shapes` dim: everything outside all of them, as
/// rectangles that do not overlap; none when there is no spotlight (F-43, D-78).
#[must_use]
pub fn dimmed_areas<'a>(
    base: &RgbaImage,
    shapes: impl IntoIterator<Item = &'a Shape>,
) -> Vec<RectF> {
    let spotlights: Vec<RectF> = shapes
        .into_iter()
        .filter_map(|shape| {
            if let Shape::Spotlight(rect) = shape {
                Some(*rect)
            } else {
                None
            }
        })
        .collect();
    if spotlights.is_empty() {
        return Vec::new();
    }
    areas_outside(base, &spotlights)
}

/// Everything of `base` outside all of `kept`, as rectangles that do not overlap, such as
/// what a crop leaves out (D-113).
#[must_use]
pub fn areas_outside(base: &RgbaImage, kept: &[RectF]) -> Vec<RectF> {
    let cuts = |limit: u32, sides: fn(&RectF) -> [f32; 2]| {
        let limit = f64::from(limit);
        let mut cuts: Vec<f64> = kept
            .iter()
            .flat_map(sides)
            .map(|side| f64::from(side).clamp(0.0, limit))
            .chain([0.0, limit])
            .collect();
        cuts.sort_by(f64::total_cmp);
        cuts.dedup();
        cuts
    };
    let columns = cuts(base.width(), |rect| [rect.x, rect.x + rect.width]);
    let rows = cuts(base.height(), |rect| [rect.y, rect.y + rect.height]);
    let lit = |x: f64, y: f64| {
        kept.iter().any(|spot| {
            let (left, top) = (f64::from(spot.x), f64::from(spot.y));
            (left..left + f64::from(spot.width)).contains(&x)
                && (top..top + f64::from(spot.height)).contains(&y)
        })
    };
    let mut dimmed = Vec::new();
    for row in rows.windows(2) {
        for column in columns.windows(2) {
            if !lit(column[0].midpoint(column[1]), row[0].midpoint(row[1])) {
                dimmed.push(cell(column[0], row[0], column[1], row[1]));
            }
        }
    }
    dimmed
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "every cut is an f32 side or an image size, clamped to the image"
)]
fn cell(left: f64, top: f64, right: f64, bottom: f64) -> RectF {
    RectF {
        x: left as f32,
        y: top as f32,
        width: (right - left) as f32,
        height: (bottom - top) as f32,
    }
}

/// Whole-image effects an export draws around the image, outside every capture pixel (F-44).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub border: bool,
    pub shadow: bool,
}

// D-80: CHARTER 4's "0 8 24, 25% black" shadow, blur radius two deviations; a 20% black border.
impl Frame {
    /// The border's opacity: black at 20%, one image pixel wide.
    pub const BORDER_ALPHA: u8 = 51;
    /// How far, in image pixels, the shadow falls below the image.
    pub const SHADOW_OFFSET: u32 = 8;
    /// The shadow's blur radius in image pixels: twice its standard deviation, and how far
    /// past the offset image the export reaches.
    pub const SHADOW_BLUR: u32 = 24;
    /// The shadow's opacity where it is fully covered: black at 25%.
    pub const SHADOW_ALPHA: f32 = 0.25;

    /// The room, in image pixels, this frame adds on each side of the image an export draws.
    #[must_use]
    pub const fn margins(self) -> FrameMargins {
        let border = if self.border { 1 } else { 0 };
        if !self.shadow {
            return FrameMargins {
                left: border,
                top: border,
                right: border,
                bottom: border,
            };
        }
        FrameMargins {
            left: border + Self::SHADOW_BLUR,
            top: border + Self::SHADOW_BLUR - Self::SHADOW_OFFSET,
            right: border + Self::SHADOW_BLUR,
            bottom: border + Self::SHADOW_BLUR + Self::SHADOW_OFFSET,
        }
    }
}

/// The room a [`Frame`] adds on each side of the image, in image pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameMargins {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

/// Names one shape for the life of its [`crate::History`]; never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShapeId(pub(crate) u64);

/// A shape in the document, with its id and the style it is drawn in.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeEntry {
    pub id: ShapeId,
    pub shape: Shape,
    pub style: Style,
}

/// What a shape draws, in image pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Arrow {
        from: PointF,
        to: PointF,
    },
    Line {
        from: PointF,
        to: PointF,
    },
    Rect {
        rect: RectF,
        fill: Fill,
    },
    Ellipse {
        rect: RectF,
        fill: Fill,
    },
    Pen(Vec<PointF>),
    Text(TextBlock),
    /// Multiplies the image under it by the style colour, at its strength (D-123).
    Highlight {
        rect: RectF,
        strength: HighlightStrength,
    },
    /// Replaces the pixels under it on export (C-22).
    Redact {
        rect: RectF,
        mode: RedactMode,
        strength: RedactStrength,
    },
    /// Dims the image outside it (F-43).
    Spotlight(RectF),
    Step(Step),
    Bubble(Bubble),
}

/// Whether a rectangle or ellipse is drawn as an outline or filled with the style colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    Hollow,
    Solid,
}

/// How a redaction replaces the pixels under it (F-36).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RedactMode {
    Pixelate,
    Blur,
}

/// How strongly a highlight tints what is under it (D-123).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HighlightStrength {
    Light,
    Medium,
    Strong,
}

impl HighlightStrength {
    /// The opacity the highlight multiplies at, out of 255.
    #[must_use]
    pub const fn alpha(self) -> u8 {
        match self {
            Self::Light => 89,
            Self::Medium => 153,
            Self::Strong => 255,
        }
    }
}

/// How coarse a redaction is, as the side of its cells in image pixels (D-123, D-130, D-132):
/// from [`Self::WEAKEST`] to [`Self::STRONGEST`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RedactStrength(u16);

impl RedactStrength {
    /// The finest a typed strength can be (D-132).
    pub const WEAKEST: Self = Self(8);
    /// The default and finest preset step, the finest strength C-22's legibility check is
    /// proved at.
    pub const FINE: Self = Self(16);
    /// A preset step on the strip (D-123).
    pub const MEDIUM: Self = Self(24);
    /// A preset step on the strip (D-123).
    pub const COARSE: Self = Self(32);
    /// The strongest preset step on the strip (D-123).
    pub const COARSEST: Self = Self(48);
    /// The strongest a typed strength can be (D-130, D-132).
    pub const STRONGEST: Self = Self(512);

    /// The strength with cells of `pixels`, or the nearest one a redaction takes.
    #[must_use]
    pub fn clamped(pixels: u16) -> Self {
        Self(pixels.clamp(Self::WEAKEST.0, Self::STRONGEST.0))
    }

    /// Whether cells this small can leave text readable: finer than [`Self::FINE`] (D-132).
    #[must_use]
    pub(crate) const fn may_be_readable(self) -> bool {
        self.0 < Self::FINE.0
    }

    /// The side of a pixelation cell in image pixels, which is also the blur radius.
    #[must_use]
    pub const fn pixels(self) -> u16 {
        self.0
    }
}

/// How dark the image is outside every spotlight, one setting for the document (F-43, D-123).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpotlightDim {
    Light,
    Medium,
    Dark,
}

impl SpotlightDim {
    /// What the dim lays over the image outside the spotlights: black at 40%, 60% or 80%.
    #[must_use]
    pub const fn color(self) -> Color {
        let a = match self {
            Self::Light => 102,
            Self::Medium => 153,
            Self::Dark => 204,
        };
        Color {
            r: 0,
            g: 0,
            b: 0,
            a,
        }
    }
}

/// The colour a shape is drawn in, its stroke width in image pixels, whether it casts a
/// shadow (D-114), and its border (D-131).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub color: Color,
    pub stroke_width: f32,
    // D-114, D-131: only Look::style_for sets them, and only for a shape that takes them.
    shadow: bool,
    border: Option<Border>,
}

/// An outline in a second colour round a shape, drawn under it (D-131).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Border {
    pub color: Color,
    pub width: BorderWidth,
}

/// How far a border reaches out from its shape (D-131).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BorderWidth {
    Thin,
    Medium,
    Thick,
}

impl BorderWidth {
    /// The width in image pixels.
    #[must_use]
    pub const fn pixels(self) -> u8 {
        match self {
            Self::Thin => 2,
            Self::Medium => 4,
            Self::Thick => 6,
        }
    }
}

// D-114: a shape's shadow is smaller than the image frame's (D-80).
impl Style {
    /// How far, in image pixels, a shape's shadow falls below it.
    pub const SHADOW_OFFSET: u32 = 3;
    /// The shadow's blur radius in image pixels: twice its standard deviation. Small enough
    /// that a thin stroke's shadow stays dark enough to see.
    pub const SHADOW_BLUR: u32 = 4;
    /// The shadow's opacity where the shape fully covers it: black at 50%.
    pub const SHADOW_ALPHA: f32 = 0.5;

    /// A style casting no shadow, with no border.
    #[must_use]
    pub const fn new(color: Color, stroke_width: f32) -> Self {
        Self {
            color,
            stroke_width,
            shadow: false,
            border: None,
        }
    }

    /// Whether the shape casts a shadow.
    #[must_use]
    pub fn shadow(&self) -> bool {
        self.shadow
    }

    pub(crate) fn with_shadow(self, shadow: bool) -> Self {
        Self { shadow, ..self }
    }

    /// The shape's border, if it has one.
    #[must_use]
    pub fn border(&self) -> Option<Border> {
        self.border
    }

    pub(crate) fn with_border(self, border: Option<Border>) -> Self {
        Self { border, ..self }
    }
}

/// Straight (not premultiplied) alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    /// The opaque colour written `#RRGGBB` in hexadecimal digits of either case, as the
    /// settings file holds it (F-59).
    #[must_use]
    pub(crate) fn from_hex(text: &str) -> Option<Self> {
        text.strip_prefix('#')
            .filter(|digits| digits.len() == 6)
            .and_then(Self::from_hex_digits)
    }

    /// The opaque colour typed as `RRGGBB` or the short `RGB`, in hexadecimal digits of either
    /// case, as the colour picker's hex box takes it (D-147).
    #[must_use]
    pub fn from_hex_digits(digits: &str) -> Option<Self> {
        if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |text: &str| u8::from_str_radix(text, 16).ok();
        let (r, g, b) = match digits.len() {
            6 => (
                channel(&digits[0..2])?,
                channel(&digits[2..4])?,
                channel(&digits[4..6])?,
            ),
            3 => (
                channel(&digits[0..1])? * 17,
                channel(&digits[1..2])? * 17,
                channel(&digits[2..3])? * 17,
            ),
            _ => return None,
        };
        Some(Self { r, g, b, a: 255 })
    }

    /// This colour written `#RRGGBB`, without its alpha, which [`Self::from_hex`] reads back
    /// opaque.
    #[must_use]
    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

/// A point in image pixels, from the top-left corner of the image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointF {
    pub x: f32,
    pub y: f32,
}

/// The head of an arrow from `from` to `to`, as one barb, the tip and the other barb. Each barb
/// is 30 degrees off the shaft and four stroke widths long, at least 10 pixels.
#[must_use]
pub fn arrow_head(from: PointF, to: PointF, stroke_width: f32) -> [PointF; 3] {
    let length = (stroke_width * 4.0).max(10.0);
    let angle = (to.y - from.y).atan2(to.x - from.x);
    let barb = |side: f32| {
        let barb_angle = angle + PI + side * FRAC_PI_6;
        PointF {
            x: to.x + length * barb_angle.cos(),
            y: to.y + length * barb_angle.sin(),
        }
    };
    [barb(1.0), to, barb(-1.0)]
}

/// A rectangle in image pixels: its top-left corner, width and height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Row-major pixels, four bytes each in R, G, B, A order, straight alpha.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

/// Why [`RgbaImage::new`] refused its pixels.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    #[error("an image needs at least one pixel, but it is {width}x{height}")]
    ZeroSize { width: u32, height: u32 },
    #[error("a {width}x{height} RGBA image needs {width}*{height}*4 bytes, but {len} were given")]
    PixelCountMismatch { width: u32, height: u32, len: usize },
}

impl RgbaImage {
    /// Takes `pixels` as the image's RGBA bytes.
    ///
    /// # Errors
    ///
    /// [`ImageError::ZeroSize`] if `width` or `height` is 0;
    /// [`ImageError::PixelCountMismatch`] unless `pixels` holds exactly `width * height * 4` bytes.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, ImageError> {
        if width == 0 || height == 0 {
            return Err(ImageError::ZeroSize { width, height });
        }
        let expected = u128::from(width) * u128::from(height) * 4;
        if pixels.len() as u128 != expected {
            return Err(ImageError::PixelCountMismatch {
                width,
                height,
                len: pixels.len(),
            });
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    /// Width in physical pixels, at least 1.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in physical pixels, at least 1.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixels, `width * height * 4` bytes.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// The colour of the pixel under `at`, opaque, or `None` where `at` is off the image
    /// (D-147: the picker's "From image").
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "image sides are far below 2^24, and only a value from 0 up to a side is cast"
    )]
    pub fn color_at(&self, at: PointF) -> Option<Color> {
        let inside = |value: f32, extent: u32| {
            (value >= 0.0 && value < extent as f32).then(|| value.floor() as usize)
        };
        let column = inside(at.x, self.width)?;
        let row = inside(at.y, self.height)?;
        let start = (row * self.width as usize + column) * 4;
        let pixel = &self.pixels[start..start + 3];
        Some(Color {
            r: pixel[0],
            g: pixel[1],
            b: pixel[2],
            a: 255,
        })
    }

    /// The average colour of a grid of 64 by 64 pixels spread evenly over the image, opaque, so
    /// its cost does not grow with the image (D-146).
    ///
    /// # Panics
    ///
    /// Never: an average of bytes is a byte.
    #[must_use]
    pub fn average_color(&self) -> Color {
        let grid = AVERAGE_GRID as usize;
        let at = |cell: usize, side: u32| (2 * cell + 1) * side as usize / (2 * grid);
        let width = self.width as usize;
        let mut sums = [0u64; 3];
        for row in 0..grid {
            let y = at(row, self.height);
            for column in 0..grid {
                let start = (y * width + at(column, self.width)) * 4;
                for (sum, &channel) in sums.iter_mut().zip(&self.pixels[start..start + 3]) {
                    *sum += u64::from(channel);
                }
            }
        }
        let samples = u64::from(AVERAGE_GRID * AVERAGE_GRID);
        let [r, g, b] = sums.map(|sum| {
            u8::try_from((sum + samples / 2) / samples).expect("an average of bytes is a byte")
        });
        Color { r, g, b, a: 255 }
    }
}

/// How many rows and columns of pixels [`RgbaImage::average_color`] samples.
const AVERAGE_GRID: u32 = 64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_image_keeps_pixels_that_match_its_size() {
        let image = RgbaImage::new(2, 3, vec![7; 24]).unwrap();
        assert_eq!((image.width(), image.height()), (2, 3));
        assert_eq!(image.pixels(), &[7; 24]);
    }

    #[test]
    fn rgba_image_rejects_pixels_that_do_not_match_its_size() {
        assert_eq!(
            RgbaImage::new(2, 3, vec![0; 23]),
            Err(ImageError::PixelCountMismatch {
                width: 2,
                height: 3,
                len: 23
            })
        );
    }

    #[test]
    fn d146_the_average_colour_weighs_every_part_of_the_image_alike() {
        let pixel = |r, g, b| [r, g, b, 255];
        let halves = |width: u32, height: u32| {
            let pixels = (0..height)
                .flat_map(|_| {
                    (0..width).flat_map(move |x| {
                        if x < width / 2 {
                            pixel(200, 0, 40)
                        } else {
                            pixel(0, 100, 240)
                        }
                    })
                })
                .collect();
            RgbaImage::new(width, height, pixels).unwrap()
        };
        let purple = Color {
            r: 100,
            g: 50,
            b: 140,
            a: 255,
        };
        assert_eq!(halves(3840, 2160).average_color(), purple);
        assert_eq!(halves(2, 1).average_color(), purple);
        let one = RgbaImage::new(1, 1, vec![9, 8, 7, 0]).unwrap();
        assert_eq!(
            one.average_color(),
            Color {
                r: 9,
                g: 8,
                b: 7,
                a: 255
            }
        );
    }

    #[test]
    fn d147_the_hex_box_reads_six_or_three_hex_digits_of_either_case_and_nothing_else() {
        let teal = Color {
            r: 0x17,
            g: 0xA2,
            b: 0xA8,
            a: 255,
        };
        assert_eq!(Color::from_hex_digits("17a2a8"), Some(teal));
        assert_eq!(Color::from_hex_digits("17A2A8"), Some(teal));
        assert_eq!(
            Color::from_hex_digits("1aB"),
            Some(Color {
                r: 0x11,
                g: 0xAA,
                b: 0xBB,
                a: 255,
            })
        );
        for refused in ["", "17a2a", "17a2a8f", "17a2ag", "+1a2b3", "#17a2a8"] {
            assert_eq!(Color::from_hex_digits(refused), None, "{refused}");
        }
        assert_eq!(Color::from_hex_digits(&teal.hex()[1..]), Some(teal));
        assert_eq!(Color::from_hex("#1aB"), None);
    }

    #[test]
    fn d147_from_image_reads_the_pixel_under_the_point_and_nothing_off_the_image() {
        let pixels = vec![1, 2, 3, 0, 4, 5, 6, 0, 7, 8, 9, 0, 10, 11, 12, 0];
        let image = RgbaImage::new(2, 2, pixels).unwrap();
        let at = |x, y| image.color_at(PointF { x, y });
        let color = |r, g, b| Some(Color { r, g, b, a: 255 });
        assert_eq!(at(0.0, 0.0), color(1, 2, 3));
        assert_eq!(at(1.9, 0.2), color(4, 5, 6));
        assert_eq!(at(0.5, 1.5), color(7, 8, 9));
        assert_eq!(at(1.0, 1.0), color(10, 11, 12));
        for (x, y) in [(-0.1, 0.0), (0.0, -0.1), (2.0, 0.0), (0.0, 2.0)] {
            assert_eq!(at(x, y), None, "{x},{y}");
        }
    }

    #[test]
    fn f01_rgba_image_rejects_a_zero_width_or_height() {
        for (width, height) in [(0, 0), (0, 3), (2, 0)] {
            assert_eq!(
                RgbaImage::new(width, height, Vec::new()),
                Err(ImageError::ZeroSize { width, height })
            );
        }
    }

    #[test]
    fn rgba_image_rejects_a_size_whose_byte_count_overflows() {
        assert!(RgbaImage::new(1 << 31, 1 << 31, Vec::new()).is_err());
    }

    #[test]
    fn f31_arrow_head_barbs_sit_30_degrees_off_the_shaft_four_widths_long() {
        let [left, tip, right] =
            arrow_head(PointF { x: 0.0, y: 0.0 }, PointF { x: 100.0, y: 0.0 }, 4.0);
        let near = |point: PointF, x: f32, y: f32| {
            (point.x - x).abs() < 1e-4 && (point.y - y).abs() < 1e-4
        };
        assert!(near(tip, 100.0, 0.0));
        let back = 16.0 * FRAC_PI_6.cos();
        assert!(near(left, 100.0 - back, -8.0), "{left:?}");
        assert!(near(right, 100.0 - back, 8.0), "{right:?}");
    }

    #[test]
    fn f31_arrow_head_barbs_are_at_least_ten_pixels_for_thin_strokes() {
        let [left, tip, _] = arrow_head(PointF { x: 0.0, y: 0.0 }, PointF { x: 0.0, y: 50.0 }, 1.0);
        assert!(((left.x - tip.x).hypot(left.y - tip.y) - 10.0).abs() < 1e-4);
    }

    fn dimmed(shapes: &[Shape]) -> Vec<RectF> {
        dimmed_areas(
            &RgbaImage::new(40, 30, vec![0; 40 * 30 * 4]).unwrap(),
            shapes,
        )
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> RectF {
        RectF {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn f43_every_pixel_outside_the_spotlights_is_dimmed_once_and_none_inside() {
        let spots = [
            rect(5.0, 4.0, 10.0, 8.0),
            rect(12.0, 10.0, 10.0, 6.0),
            rect(30.0, -5.0, 20.0, 10.0),
        ];
        let areas = dimmed(&spots.map(Shape::Spotlight));
        let inside = |area: &RectF, x: f32, y: f32| {
            (area.x..area.x + area.width).contains(&x)
                && (area.y..area.y + area.height).contains(&y)
        };
        for y in 0..30_u8 {
            for x in 0..40_u8 {
                let (x, y) = (f32::from(x) + 0.5, f32::from(y) + 0.5);
                let lit = spots.iter().any(|spot| inside(spot, x, y));
                let dimmed = areas.iter().filter(|area| inside(area, x, y)).count();
                assert_eq!(dimmed, usize::from(!lit), "pixel at ({x}, {y})");
            }
        }
    }

    #[test]
    fn f41_removing_a_step_renumbers_the_ones_after_it_and_undo_numbers_them_back() {
        use crate::{Command, History, Look, Tool};
        let mut history = History::new(Document::new(Arc::new(
            RgbaImage::new(1, 1, vec![0; 4]).unwrap(),
        )));
        let look = Look::default_for(Tool::Step);
        let add = |history: &mut History, command| history.apply(command).unwrap();
        for x in [10.0, 20.0, 30.0] {
            let step = Tool::Step.command(&[PointF { x, y: 5.0 }], false, &look);
            add(&mut history, step.unwrap());
            let arrow =
                Tool::Arrow.command(&[PointF { x, y: 0.0 }, PointF { x, y: 9.0 }], false, &look);
            add(&mut history, arrow.unwrap());
        }
        let numbers = |document: &Document| {
            document
                .labels()
                .map(|(_, label)| match label {
                    Label::Centred { text, centre_x, .. } => format!("{text}@{centre_x}"),
                    Label::Block(block) => panic!("no text was added: {block:?}"),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(numbers(history.document()), ["1@10", "2@20", "3@30"]);
        let first = history.document().shapes()[0].id;
        add(&mut history, Command::Remove(first));
        assert_eq!(numbers(history.document()), ["1@20", "2@30"]);
        history.undo().unwrap();
        assert_eq!(numbers(history.document()), ["1@10", "2@20", "3@30"]);
    }

    #[test]
    fn d113_the_kept_area_is_the_crop_or_the_whole_image_and_the_rest_is_outside_it() {
        use crate::{Command, History};
        let image = RgbaImage::new(40, 30, vec![0; 40 * 30 * 4]).unwrap();
        let mut history = History::new(Document::new(Arc::new(image.clone())));
        assert_eq!(history.document().kept_area(), rect(0.0, 0.0, 40.0, 30.0));
        let crop = rect(5.0, 4.0, 10.0, 8.0);
        history.apply(Command::SetCrop(Some(crop))).unwrap();
        assert_eq!(history.document().kept_area(), crop);
        let outside = areas_outside(&image, &[crop]);
        let area: f32 = outside.iter().map(|area| area.width * area.height).sum();
        assert_eq!(area, 40.0 * 30.0 - 10.0 * 8.0);
    }

    #[test]
    fn f43_only_spotlights_dim() {
        let highlight = Shape::Highlight {
            rect: rect(5.0, 4.0, 10.0, 8.0),
            strength: HighlightStrength::Strong,
        };
        assert_eq!(dimmed(&[highlight]), Vec::new());
    }
}
