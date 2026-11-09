use cleanshot_core::{TabDirection, ToolbarControl};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey,
    VIRTUAL_KEY, VK_DOWN, VK_E, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_S, VK_SPACE, VK_TAB,
    VK_UP, VK_W,
};

use crate::Error;

const SHIFT_NUDGE: i32 = 10;

/// A key the overlay holds as a hotkey while it is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Key {
    /// Chooses this toolbar control, wherever the focus is.
    Choose(ToolbarControl),
    /// Chooses the focused toolbar control.
    Press,
    /// Moves the toolbar's focus one control along.
    Focus(TabDirection),
    Nudge(i32, i32),
    SaveAs,
    /// Switches between drawing a region and picking a window.
    SwitchMode,
}

pub(super) const KEYS: [(HOT_KEY_MODIFIERS, VIRTUAL_KEY, Key); 17] = [
    (MOD_NOREPEAT, VK_ESCAPE, Key::Choose(ToolbarControl::Cancel)),
    (MOD_NOREPEAT, VK_RETURN, Key::Press),
    (MOD_NOREPEAT, VK_SPACE, Key::Press),
    (
        HOT_KEY_MODIFIERS(MOD_NOREPEAT.0 | MOD_CONTROL.0),
        VK_S,
        Key::Choose(ToolbarControl::Save),
    ),
    (
        HOT_KEY_MODIFIERS(MOD_NOREPEAT.0 | MOD_CONTROL.0 | MOD_SHIFT.0),
        VK_S,
        Key::SaveAs,
    ),
    (MOD_NOREPEAT, VK_E, Key::Choose(ToolbarControl::Edit)),
    (MOD_NOREPEAT, VK_W, Key::SwitchMode),
    (
        HOT_KEY_MODIFIERS(0),
        VK_TAB,
        Key::Focus(TabDirection::Forward),
    ),
    (MOD_SHIFT, VK_TAB, Key::Focus(TabDirection::Back)),
    (HOT_KEY_MODIFIERS(0), VK_LEFT, Key::Nudge(-1, 0)),
    (HOT_KEY_MODIFIERS(0), VK_RIGHT, Key::Nudge(1, 0)),
    (HOT_KEY_MODIFIERS(0), VK_UP, Key::Nudge(0, -1)),
    (HOT_KEY_MODIFIERS(0), VK_DOWN, Key::Nudge(0, 1)),
    (MOD_SHIFT, VK_LEFT, Key::Nudge(-SHIFT_NUDGE, 0)),
    (MOD_SHIFT, VK_RIGHT, Key::Nudge(SHIFT_NUDGE, 0)),
    (MOD_SHIFT, VK_UP, Key::Nudge(0, -SHIFT_NUDGE)),
    (MOD_SHIFT, VK_DOWN, Key::Nudge(0, SHIFT_NUDGE)),
];

/// Holds every overlay key as a hotkey of `first`.
pub(super) fn take_keys(first: HWND) -> Result<(), Error> {
    for (id, (modifiers, key, _)) in (0..).zip(KEYS) {
        // SAFETY: `first` is an overlay window of this thread.
        unsafe { RegisterHotKey(Some(first), id, modifiers, u32::from(key.0)) }
            .map_err(Error::HotkeyTaken)?;
    }
    Ok(())
}

pub(super) fn free_keys(first: HWND) {
    for (id, _) in (0..).zip(KEYS) {
        // SAFETY: unregistering an id this window may not hold only returns an error.
        let _ = unsafe { UnregisterHotKey(Some(first), id) };
    }
}
