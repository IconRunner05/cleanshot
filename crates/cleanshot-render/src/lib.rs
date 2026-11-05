//! A document's output pixels: flatten, redaction and encoding, drawn with tiny-skia
//! (CHARTER 5).

#![forbid(unsafe_code)]

mod border;
mod encode;
mod flatten;
mod frame;
mod gamma;
mod highlight;
mod patch;
mod redact;
mod shadow;
mod thumbnail;

use cleanshot_core::{ImageError, RectF, ShapeId};

pub use border::shape_border;
pub use encode::{Format, encode};
pub use flatten::{Flattened, flatten};
pub use gamma::TEXT_GAMMA_RATIOS;
pub use highlight::highlighted;
pub use patch::Patch;
pub use redact::redacted;
pub use shadow::shape_shadow;
pub use thumbnail::thumbnail;

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
