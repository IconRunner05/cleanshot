#![forbid(unsafe_code)]

mod canvas;
mod editor_window;
mod restricted_platform;

use std::rc::Rc;

use gpui::{App, Application};
use gpui_windows::WindowsPlatform;

pub use crate::editor_window::{EditorWindow, open};
use crate::restricted_platform::RestrictedPlatform;

/// Why the editor could not start or open a window.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the Windows platform did not start")]
    PlatformStart(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("the editor window did not open")]
    WindowOpen(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Runs the GPUI event loop on this thread until the app quits, calling `on_launch` once it
/// is ready.
///
/// # Errors
///
/// [`Error::PlatformStart`] if the Windows platform cannot be created.
pub fn run(on_launch: impl FnOnce(&mut App) + 'static) -> Result<(), Error> {
    let platform =
        WindowsPlatform::new(false).map_err(|error| Error::PlatformStart(error.into()))?;
    Application::with_platform(Rc::new(RestrictedPlatform(platform))).run(on_launch);
    Ok(())
}
