#![forbid(unsafe_code)]

mod document;
mod history;

pub use document::{
    Color, Document, Fill, ImageError, PointF, RectF, RedactMode, RgbaImage, Shape, ShapeEntry,
    ShapeId, Style,
};
pub use history::{Command, CommandError, History};
