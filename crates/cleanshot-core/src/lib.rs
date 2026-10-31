//! The document a capture is marked up in: shapes, commands with undo, text and settings
//! types. Pure Rust, with no OS calls or I/O (CHARTER 5).

#![forbid(unsafe_code)]

mod delivery;
mod document;
mod drag;
mod drawing;
mod history;
mod keys;
mod selection;
mod settings;
mod text;
mod text_edit;
mod tool;

pub use delivery::{Delivered, DeliveryError, Output};
pub use document::{
    Color, Document, Fill, ImageError, PointF, RectF, RedactMode, RgbaImage, Shape, ShapeEntry,
    ShapeId, Style, arrow_head,
};
pub use drag::{Drag, Press};
pub use drawing::{smoothed_pen, snapped_to_45_degrees, squared};
pub use history::{Command, CommandError, History};
pub use keys::{Activity, KeyAction, LineStep, TextKey, action_for_key};
pub use selection::{
    BoxHandle, Handle, box_handle_at, box_handles, handle_at, handles, moved, resized, resized_box,
    shape_at,
};
pub use settings::{
    AfterCapture, Hotkey, Hotkeys, Key, MAX_SETTINGS_BYTES, Modifiers, SaveFormat, Settings,
    SettingsProblem,
};
pub use text::{
    GlyphMask, GlyphMaskError, INTER, INTER_ASCENT, INTER_DESCENT, INTER_UNITS_PER_EM, LINE_HEIGHT,
    TextBlock, TextBlockError, TextMasks,
};
pub use text_edit::{CaretMove, TextEdit, TextOrigin, commit_text};
pub use tool::Tool;
