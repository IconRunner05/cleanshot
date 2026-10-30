#![forbid(unsafe_code)]

mod encode;
mod flatten;
mod redact;

use cleanshot_core::{ImageError, RectF};

pub use encode::{Format, encode};
pub use flatten::flatten;

/// Why an export failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("a {width}x{height} image is larger than the renderer can hold")]
    TooLarge { width: u32, height: u32 },
    #[error("the crop {0:?} does not cover any pixel of the image")]
    CropOutsideImage(RectF),
    #[error("the flattened pixels did not form an image")]
    Output(#[source] ImageError),
    #[error("the image could not be encoded")]
    Encode(#[source] image::ImageError),
}
