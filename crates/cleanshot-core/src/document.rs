use std::f32::consts::{FRAC_PI_6, PI};
use std::sync::Arc;

/// A capture with annotations on top. All coordinates are in base-image physical pixels.
/// Changed only through [`crate::History`].
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub(crate) base: Arc<RgbaImage>,
    pub(crate) shapes: Vec<ShapeEntry>,
    pub(crate) crop: Option<RectF>,
}

impl Document {
    /// A document showing `base` with no shapes and no crop.
    #[must_use]
    pub fn new(base: Arc<RgbaImage>) -> Self {
        Self {
            base,
            shapes: Vec::new(),
            crop: None,
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

    /// The part of the base image an export keeps, inside the image with a positive size;
    /// `None` keeps all of it.
    #[must_use]
    pub fn crop(&self) -> Option<RectF> {
        self.crop
    }
}

/// Names one shape for the life of its [`crate::History`]; never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShapeId(pub(crate) u64);

#[derive(Clone, Debug, PartialEq)]
pub struct ShapeEntry {
    pub id: ShapeId,
    pub shape: Shape,
    pub style: Style,
}

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
    /// `size` is the font size in pixels.
    Text {
        at: PointF,
        text: String,
        size: f32,
    },
    /// Multiplies the image under it by the style colour.
    Highlight(RectF),
    /// Replaces the pixels under it on export (C-22).
    Redact {
        rect: RectF,
        mode: RedactMode,
    },
}

/// Whether a rectangle or ellipse is drawn as an outline or filled with the style colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    Hollow,
    Solid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedactMode {
    Pixelate,
    Blur,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub color: Color,
    pub stroke_width: f32,
}

/// Straight (not premultiplied) alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

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
}

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
}
