pub mod capture;
#[cfg(feature = "field-test")]
pub mod field_test;
pub mod tray;

use windows::Win32::System::LibraryLoader::{
    LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
};

/// C-12: every later DLL load in this process searches System32 only, never the exe's folder
/// or the current directory. Call it first in `main`.
///
/// # Errors
///
/// The Windows error if the search path cannot be restricted.
pub fn load_dlls_from_system32_only() -> windows::core::Result<()> {
    // SAFETY: SetDefaultDllDirectories takes a flag value and touches no memory of ours.
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) }
}

/// What went wrong in a call to Windows.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Windows refused to create the tray window")]
    Window(#[source] windows::core::Error),
    #[error("another program has taken the hotkey")]
    HotkeyTaken(#[source] windows::core::Error),
    #[error("Windows refused to copy the screen")]
    Capture(#[source] windows::core::Error),
    #[error("a monitor's capture has an impossible size")]
    CaptureShape(#[from] cleanshot_core::ImageError),
}
