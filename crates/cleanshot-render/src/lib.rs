//! A document's output pixels: flatten, redaction and encoding, drawn with tiny-skia
//! (CHARTER 5).

#![forbid(unsafe_code)]

mod encode;
mod flatten;
mod frame;
mod gamma;
mod highlight;
mod redact;
mod shadow;
mod thumbnail;

use cleanshot_core::{ImageError, RectF, RgbaImage, ShapeId};

pub use encode::{Format, encode};
pub use flatten::{Flattened, flatten};
pub use gamma::TEXT_GAMMA_RATIOS;
pub use highlight::highlighted;
pub use redact::redacted;
pub use shadow::shape_shadow;
pub use thumbnail::thumbnail;

/// Pixels an export draws at one place on the image: `pixels`, its top left corner at
/// `left`, `top`, for the editor to show as the export will.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Patch {
    pub left: u32,
    pub top: u32,
    pub pixels: RgbaImage,
}

/// Why an export failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("a {width}x{height} image is larger than the renderer can hold")]
    TooLarge { width: u32, height: u32 },
    #[error("the crop {0:?} does not cover any pixel of the image")]
    CropOutsideImage(RectF),
    #[error("shape {0:?} shows text but has no glyph masks")]
    MissingTextMask(ShapeId),
    #[error("the flattened pixels did not form an image")]
    Output(#[source] ImageError),
    #[error("the image could not be encoded")]
    Encode(#[source] image::ImageError),
}
