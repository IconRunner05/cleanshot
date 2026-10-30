use cleanshot_core::RgbaImage;
use image::ExtendedColorType;
use image::ImageEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;

use crate::Error;

/// The encodings an export can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    /// Quality 90 (F-21); the alpha channel is dropped.
    Jpeg,
    Bmp,
    /// A packed `CF_DIBV5`: the bitmap a BMP file holds, without the file header (F-20).
    ClipboardBitmap,
}

/// `image` encoded as `format`. Copy and save encode through here (F-24).
///
/// # Errors
///
/// [`Error::Encode`] if the encoder fails, and [`Error::TooLarge`] if the image is too large
/// for a bitmap header.
pub fn encode(image: &RgbaImage, format: Format) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    let (width, height, pixels) = (image.width(), image.height(), image.pixels());
    match format {
        Format::Png => PngEncoder::new(&mut bytes)
            .write_image(pixels, width, height, ExtendedColorType::Rgba8)
            .map_err(Error::Encode)?,
        Format::Jpeg => {
            let rgb: Vec<u8> = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|&[r, g, b, _]| [r, g, b])
                .collect();
            JpegEncoder::new_with_quality(&mut bytes, 90)
                .write_image(&rgb, width, height, ExtendedColorType::Rgb8)
                .map_err(Error::Encode)?;
        }
        Format::Bmp => {
            let bitmap = clipboard_bitmap(image)?;
            let too_large = || Error::TooLarge { width, height };
            let file_size = u32::try_from(bitmap.len())
                .ok()
                .and_then(|length| length.checked_add(FILE_HEADER_SIZE))
                .ok_or_else(too_large)?;
            bytes.extend(b"BM");
            bytes.extend(file_size.to_le_bytes());
            bytes.extend([0; 4]);
            bytes.extend((FILE_HEADER_SIZE + INFO_HEADER_SIZE).to_le_bytes());
            bytes.extend(bitmap);
        }
        Format::ClipboardBitmap => bytes = clipboard_bitmap(image)?,
    }
    Ok(bytes)
}

const FILE_HEADER_SIZE: u32 = 14;
const INFO_HEADER_SIZE: u32 = 124;
const BI_BITFIELDS: u32 = 3;
const LCS_SRGB: u32 = u32::from_be_bytes(*b"sRGB");
const LCS_GM_IMAGES: u32 = 4;

/// A BITMAPV5HEADER, then 32-bit BGRA rows bottom-up with straight alpha. Layout:
/// <https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-bitmapv5header>
fn clipboard_bitmap(image: &RgbaImage) -> Result<Vec<u8>, Error> {
    let too_large = || Error::TooLarge {
        width: image.width(),
        height: image.height(),
    };
    let width = i32::try_from(image.width()).map_err(|_| too_large())?;
    let height = i32::try_from(image.height()).map_err(|_| too_large())?;
    let image_size = u32::try_from(image.pixels().len()).map_err(|_| too_large())?;

    let mut bytes = Vec::with_capacity(INFO_HEADER_SIZE as usize + image.pixels().len());
    bytes.extend(INFO_HEADER_SIZE.to_le_bytes());
    bytes.extend(width.to_le_bytes());
    bytes.extend(height.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(32_u16.to_le_bytes());
    bytes.extend(BI_BITFIELDS.to_le_bytes());
    bytes.extend(image_size.to_le_bytes());
    bytes.extend([0; 16]);
    for mask in [0x00FF_0000_u32, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000] {
        bytes.extend(mask.to_le_bytes());
    }
    bytes.extend(LCS_SRGB.to_le_bytes());
    bytes.extend([0; 36 + 12]);
    bytes.extend(LCS_GM_IMAGES.to_le_bytes());
    bytes.extend([0; 12]);

    let row_length = image.width() as usize * 4;
    for row in image.pixels().chunks_exact(row_length).rev() {
        for &[r, g, b, a] in row.as_chunks::<4>().0 {
            bytes.extend([b, g, r, a]);
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use image::ImageFormat;

    use super::*;

    /// 3 by 2, every pixel different and none fully opaque, so a flip, a channel swap or a
    /// dropped alpha shows.
    fn sample() -> RgbaImage {
        let pixels = (0..6_u8)
            .flat_map(|index| [index * 40, 255 - index * 30, index * 7, 100 + index * 20])
            .collect();
        RgbaImage::new(3, 2, pixels).unwrap()
    }

    fn decoded(bytes: &[u8], format: ImageFormat) -> image::RgbaImage {
        image::load_from_memory_with_format(bytes, format)
            .unwrap()
            .into_rgba8()
    }

    #[test]
    fn f21_png_keeps_every_pixel() {
        let image = sample();
        let png = encode(&image, Format::Png).unwrap();
        assert_eq!(decoded(&png, ImageFormat::Png).as_raw(), image.pixels());
    }

    #[test]
    fn f20_the_clipboard_bitmap_is_a_bmp_file_less_its_file_header() {
        let image = sample();
        let bitmap = encode(&image, Format::ClipboardBitmap).unwrap();
        let file = encode(&image, Format::Bmp).unwrap();
        assert_eq!(&file[..2], b"BM");
        assert_eq!(file[2..6], u32::try_from(file.len()).unwrap().to_le_bytes());
        assert_eq!(file[10..14], (14_u32 + 124).to_le_bytes());
        assert_eq!(file[14..], bitmap);
    }

    #[test]
    fn f20_the_clipboard_bitmap_is_a_v5_header_then_bottom_up_bgra_with_straight_alpha() {
        let top = [200, 10, 20, 128];
        let bottom = [1, 2, 3, 255];
        let image = RgbaImage::new(1, 2, [top, bottom].concat()).unwrap();
        let bitmap = encode(&image, Format::ClipboardBitmap).unwrap();

        let field =
            |offset: usize| u32::from_le_bytes(bitmap[offset..offset + 4].try_into().unwrap());
        assert_eq!(field(0), 124, "bV5Size");
        assert_eq!(field(4), 1, "bV5Width");
        assert_eq!(field(8), 2, "bV5Height, positive: bottom-up");
        assert_eq!(bitmap[12..16], [1, 0, 32, 0], "bV5Planes, bV5BitCount");
        assert_eq!(field(16), 3, "bV5Compression BI_BITFIELDS");
        assert_eq!(field(20), 8, "bV5SizeImage");
        assert_eq!(
            [field(40), field(44), field(48), field(52)],
            [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000],
            "red, green, blue and alpha masks"
        );
        assert_eq!(field(56), 0x7352_4742, "bV5CSType LCS_sRGB");
        assert_eq!(field(108), 4, "bV5Intent LCS_GM_IMAGES");
        assert_eq!(
            bitmap[124..],
            [3, 2, 1, 255, 20, 10, 200, 128],
            "bottom row first, BGRA"
        );
    }

    #[test]
    fn f21_bmp_keeps_every_pixel_and_its_alpha() {
        let image = sample();
        let file = encode(&image, Format::Bmp).unwrap();
        assert_eq!(decoded(&file, ImageFormat::Bmp).as_raw(), image.pixels());
    }

    #[test]
    fn f21_jpeg_keeps_the_size() {
        let jpeg = encode(&sample(), Format::Jpeg).unwrap();
        assert_eq!(decoded(&jpeg, ImageFormat::Jpeg).dimensions(), (3, 2));
    }
}
