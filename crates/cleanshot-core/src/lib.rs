//! The document a capture is marked up in: shapes, commands with undo, text and settings
//! types. Pure Rust, with no OS calls or I/O (CHARTER 5).

#![forbid(unsafe_code)]

mod delivery;
mod document;
mod drag;
mod drawing;
mod history;
mod keys;
mod look;
mod policy;
mod selection;
mod settings;
mod settings_change;
mod step;
mod text;
mod text_edit;
mod tool;

pub use delivery::{Delivered, DeliveryError, Output, delivery_notice, save_as_notice};
pub use document::{
    Color, Document, Fill, Frame, ImageError, PointF, RectF, RedactMode, RgbaImage, SPOTLIGHT_DIM,
    Shape, ShapeEntry, ShapeId, Style, arrow_head, dimmed_areas, step_numbers,
};
pub use drag::{Drag, Press, Start};
pub use drawing::{smoothed_pen, snapped_to_45_degrees, squared};
pub use history::{Command, CommandError, History};
pub use keys::{Activity, KeyAction, LineStep, TextKey, action_for_key};
pub use look::{Look, Looks, Property, PropertyError, Setting};
pub use policy::{Policy, PolicyProblem, PolicySetting, PolicyValue, Saving, SavingOff};
pub use selection::{
    BoxHandle, Handle, box_handle_at, box_handles, handle_at, handles, hits, moved, resized,
    resized_box, shape_at,
};
pub use settings::{
    AfterCapture, Hotkey, HotkeyAction, HotkeyRefusal, Hotkeys, Key, MAX_SETTINGS_BYTES, Modifiers,
    SaveFormat, Settings, SettingsProblem,
};
pub use settings_change::{Applied, SettingsNotice, SettingsSaveError, settings_notice};
pub use step::{STEP_DIAMETER, Step, number_color};
pub use text::{
    DEFAULT_TEXT_SIZE, GlyphMask, GlyphMaskError, INTER, INTER_ASCENT, INTER_DESCENT,
    INTER_UNITS_PER_EM, LINE_HEIGHT, Label, TextBlock, TextBlockError, TextMasks, baseline,
};
pub use text_edit::{
    BoxPress, CaretMove, LineMove, LinePosition, TextEdit, TextOrigin, commit_text,
};
pub use tool::Tool;
