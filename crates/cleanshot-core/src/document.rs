use std::sync::Arc;

/// A capture with annotations on top. All coordinates are in base-image physical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub base: Arc<RgbaImage>,
    /// Drawn in order, so the last shape is on top.
    pub shapes: Vec<ShapeEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShapeEntry {
    pub shape: Shape,
    pub style: Style,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Arrow { from: PointF, to: PointF },
    Line { from: PointF, to: PointF },
    Rect(RectF),
    Ellipse(RectF),
    Pen(Vec<PointF>),
    Highlight(RectF),
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
    fn rgba_image_rejects_a_zero_width_or_height() {
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
}
