//! A document's output pixels: flatten, redaction and encoding, drawn with tiny-skia
//! (CHARTER 5).

#![forbid(unsafe_code)]

mod encode;
mod flatten;
mod gamma;
mod redact;

use cleanshot_core::{ImageError, RectF, ShapeId};

pub use encode::{Format, encode};
pub use flatten::flatten;
pub use gamma::TEXT_GAMMA_RATIOS;

/// Why an export failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("a {width}x{height} image is larger than the renderer can hold")]
    TooLarge { width: u32, height: u32 },
    #[error("the crop {0:?} does not cover any pixel of the image")]
    CropOutsideImage(RectF),
    #[error("text shape {0:?} has no glyph masks")]
    MissingTextMask(ShapeId),
    #[error("the flattened pixels did not form an image")]
    Output(#[source] ImageError),
    #[error("the image could not be encoded")]
    Encode(#[source] image::ImageError),
}
