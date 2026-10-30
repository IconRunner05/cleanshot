#![forbid(unsafe_code)]

mod document;
mod history;
mod selection;

pub use document::{
    Color, Document, Fill, ImageError, PointF, RectF, RedactMode, RgbaImage, Shape, ShapeEntry,
    ShapeId, Style, arrow_head,
};
pub use history::{Command, CommandError, History};
pub use selection::{BoxHandle, Handle, handle_at, handles, moved, resized, shape_at};
