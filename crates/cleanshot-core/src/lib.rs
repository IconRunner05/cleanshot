#![forbid(unsafe_code)]

mod document;
mod history;
mod selection;
mod settings;

pub use document::{
    Color, Document, Fill, ImageError, PointF, RectF, RedactMode, RgbaImage, Shape, ShapeEntry,
    ShapeId, Style, arrow_head,
};
pub use history::{Command, CommandError, History};
pub use selection::{
    BoxHandle, Handle, box_handle_at, box_handles, handle_at, handles, moved, resized, resized_box,
    shape_at,
};
pub use settings::{
    AfterCapture, Hotkey, Hotkeys, Key, MAX_SETTINGS_BYTES, Modifiers, SaveFormat, Settings,
    SettingsProblem,
};
