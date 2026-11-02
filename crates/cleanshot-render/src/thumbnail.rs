use cleanshot_core::RgbaImage;
use image::{ImageBuffer, Rgba, imageops};

use crate::Error;

/// F-09: `image` scaled to fit within `max_width` by `max_height` pixels with its aspect
/// kept, each pixel the average of the area it covers, so text stays legible. An image that
/// already fits is returned at its own size, never enlarged.
///
/// # Errors
///
/// [`Error::Output`] if the scaled pixels do not form an image.
pub fn thumbnail(image: &RgbaImage, max_width: u32, max_height: u32) -> Result<RgbaImage, Error> {
    let (width, height) = fitted(image.width(), image.height(), max_width, max_height);
    let scaled = imageops::thumbnail(&buffer(image), width, height);
    RgbaImage::new(width, height, scaled.into_raw()).map_err(Error::Output)
}

/// `image`'s pixels as the `image` crate reads them, without a copy.
fn buffer(image: &RgbaImage) -> ImageBuffer<Rgba<u8>, &[u8]> {
    ImageBuffer::from_raw(image.width(), image.height(), image.pixels())
        .expect("an RgbaImage holds width * height * 4 bytes")
}

/// The largest size within `max_width` by `max_height`, and no larger than `width` by `height`,
/// that keeps their aspect, each side at least 1.
fn fitted(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let scale = (f64::from(max_width) / f64::from(width))
        .min(f64::from(max_height) / f64::from(height))
        .min(1.0);
    let side = |length: u32| {
        // The product is at most `length`, a u32, and rounds to at least 1.
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let scaled = (f64::from(length) * scale).round() as u32;
        scaled.max(1)
    };
    (side(width), side(height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkerboard(width: u32, height: u32) -> RgbaImage {
        let pixels = (0..height)
            .flat_map(|y| (0..width).map(move |x| if (x + y) % 2 == 0 { 0 } else { 255 }))
            .flat_map(|level| [level, level, level, 255])
            .collect();
        RgbaImage::new(width, height, pixels).unwrap()
    }

    #[test]
    fn f09_a_thumbnail_fits_its_box_keeping_the_aspect_and_never_grows() {
        let wide = thumbnail(&checkerboard(400, 100), 200, 200).unwrap();
        assert_eq!((wide.width(), wide.height()), (200, 50));
        let tall = thumbnail(&checkerboard(100, 400), 200, 200).unwrap();
        assert_eq!((tall.width(), tall.height()), (50, 200));
        let small = thumbnail(&checkerboard(30, 20), 200, 200).unwrap();
        assert_eq!((small.width(), small.height()), (30, 20));
        let sliver = thumbnail(&checkerboard(4000, 1), 200, 200).unwrap();
        assert_eq!((sliver.width(), sliver.height()), (200, 1));
    }

    #[test]
    fn f09_each_thumbnail_pixel_averages_the_area_it_covers() {
        let halved = thumbnail(&checkerboard(64, 64), 32, 32).unwrap();
        for pixel in halved.pixels().as_chunks::<4>().0 {
            assert!(
                (126..=129).contains(&pixel[0]) && pixel[3] == 255,
                "a 2x2 checker cell averages to mid grey, not {pixel:?}"
            );
        }
    }
}
