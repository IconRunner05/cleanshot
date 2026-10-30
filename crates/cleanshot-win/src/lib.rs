pub mod capture;
#[cfg(feature = "field-test")]
pub mod field_test;
pub mod tray;

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
