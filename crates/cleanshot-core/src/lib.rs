//! The document a capture is marked up in: shapes, commands with undo, text and settings
//! types. Pure Rust, with no OS calls or I/O (CHARTER 5).

#![forbid(unsafe_code)]

pub mod thumbnail_layout;

mod bubble;
mod capture_mode;
mod capture_panel;
mod delivery;
mod document;
mod drag;
mod drawing;
mod history;
mod keys;
mod look;
mod options_menu;
mod policy;
mod screen;
mod selection;
mod settings;
mod settings_change;
mod startup;
mod step;
mod strip;
mod text;
mod text_edit;
mod timer;
mod tool;
mod toolbar;
mod window_pick;

pub use bubble::{BUBBLE_PADDING, Bubble};
pub use capture_mode::CaptureMode;
pub use capture_panel::{CapturePanel, PanelOutcome};
pub use delivery::{
    Delivered, DeliveryError, Output, SET_BY_ORGANISATION, delivery_notice, save_as_notice,
    saving_off_notice, saving_off_reason,
};
pub use document::{
    Color, Document, Fill, Frame, HighlightStrength, ImageError, PointF, RectF, RedactMode,
    RedactStrength, RgbaImage, Shape, ShapeEntry, ShapeId, SpotlightDim, Style, areas_outside,
    arrow_head, dimmed_areas, step_numbers,
};
pub use drag::{Drag, Press, Start, text_reopened_at};
pub use drawing::{smoothed_pen, snapped_to_45_degrees, squared};
pub use history::{Change, Command, CommandError, History, Restack};
pub use keys::{Activity, KeyAction, LineStep, TextKey, action_for_key};
pub use look::{Look, Looks, Property, PropertyError};
pub use options_menu::{OptionsEntry, OptionsItem, Picked, options_menu};
pub use policy::{
    POLICY_KEY, Policy, PolicyProblem, PolicySetting, PolicyValue, Saving, SavingOff,
};
pub use screen::{Monitor, Point, Rect, first_holding};
pub use selection::{
    BoxHandle, Handle, box_handle_at, box_handles, handle_at, handles, hits, moved, resized,
    resized_box, shape_at,
};
pub use settings::{
    AfterCapture, Hotkey, HotkeyAction, HotkeyRefusal, Hotkeys, Key, MAX_SETTINGS_BYTES, Modifiers,
    SaveFormat, Settings, SettingsProblem,
};
pub use settings_change::{Applied, SettingsNotice, SettingsSaveError, settings_notice};
pub use startup::{StartupEntryError, run_command, starts};
pub use step::{STEP_DIAMETER, Step};
pub use strip::{Setting, TypedValue};
pub use text::{
    Alignment, BoxSize, DEFAULT_TEXT_SIZE, GlyphMask, GlyphMaskError, Horizontal, INTER,
    INTER_ASCENT, INTER_DESCENT, INTER_UNITS_PER_EM, LINE_HEIGHT, Label, TextBlock, TextBlockError,
    TextMasks, Vertical, baseline, text_on,
};
pub use text_edit::{
    BoxPress, CaretMove, LineMove, LinePosition, TextEdit, TextFrame, TextOrigin, commit_text,
};
pub use timer::Timer;
pub use tool::Tool;
pub use toolbar::{TabDirection, Toolbar, ToolbarControl};
pub use window_pick::{Pick, Pickable, next_window, picked};
