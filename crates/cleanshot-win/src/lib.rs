#[cfg(feature = "ui-automation-test")]
pub mod accessibility;
pub mod capture;
pub mod clipboard;
mod fade;
#[cfg(feature = "field-test")]
pub mod field_test;
pub mod instance;
pub mod log;
mod loupe;
pub mod overlay;
mod palette;
pub mod policy;
mod registry;
pub mod save;
mod selection;
pub mod settings_file;
mod size_pill;
pub mod startup;
pub mod text;
pub mod thumbnail;
mod toolbar;
pub mod tray;
mod tray_icon;

use windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS;
use windows::Win32::System::LibraryLoader::{
    LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
};
use windows::Win32::UI::WindowsAndMessaging::{RegisterClassExW, WNDCLASSEXW};

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

/// Registers `class` for this process. A class registered already, by an earlier window of
/// the same kind, counts as registered.
pub(crate) fn register_window_class(class: &WNDCLASSEXW) -> Result<(), Error> {
    // SAFETY: the caller's `class` is fully initialised and its strings are 'static.
    if unsafe { RegisterClassExW(class) } != 0 {
        return Ok(());
    }
    let error = windows::core::Error::from_thread();
    if error.code() == ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
        return Ok(());
    }
    Err(Error::Window(error))
}

/// The size of the Win32 struct `T`, as its `cbSize` field takes it.
pub(crate) fn struct_size<T>() -> u32 {
    u32::try_from(size_of::<T>()).expect("a Win32 struct is smaller than 4 GiB")
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
    #[error("Windows refused the single-instance mutex")]
    SingleInstance(#[source] windows::core::Error),
    #[error("the running Cleanshot has no tray window yet")]
    NoRunningTray(#[source] windows::core::Error),
    #[error("Windows' Print screen key setting could not be read")]
    PrintScreenSetting(#[source] windows::core::Error),
    #[error("the tray has no icon to show a warning from")]
    WarningNotShown,
    #[error("the shell had no tray icon to remove")]
    IconNotRemoved,
    #[cfg(feature = "ui-automation-test")]
    #[error("UI Automation could not read this process's windows")]
    Accessibility(#[source] windows::core::Error),
    #[error("the pointer is on no captured monitor")]
    NoMonitorAtPointer,
    #[error("no display output could be duplicated")]
    NothingDuplicated,
    #[error(
        "duplication gave no desktop image in time, after {pointer_only_frames} frames that held only the pointer"
    )]
    NoDesktopImage { pointer_only_frames: u32 },
    #[error(
        "duplication gave a {width} x {height} frame in DXGI format {format} at rotation {rotation}, which only GDI can copy"
    )]
    DuplicationNotCopyable {
        format: i32,
        rotation: i32,
        width: u32,
        height: u32,
    },
    #[error("DirectWrite could not lay out or draw the text")]
    Text(#[source] windows::core::Error),
    #[error("a glyph mask has an impossible size")]
    TextMaskShape(#[from] cleanshot_core::GlyphMaskError),
}
