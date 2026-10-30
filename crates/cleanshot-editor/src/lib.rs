#![forbid(unsafe_code)]

mod bench;
mod canvas;
mod editor_window;
mod placement;
mod restricted_platform;
mod toolbar;

use std::rc::Rc;

use cleanshot_core::{CommandError, ImageError};
use gpui::{App, Application};
use gpui_windows::WindowsPlatform;

pub use crate::bench::{BenchReport, bench};
pub use crate::editor_window::{EditorWindow, open};
use crate::restricted_platform::RestrictedPlatform;
use crate::toolbar::Icons;

/// Why the editor could not start, open a window or run the bench.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the Windows platform did not start")]
    PlatformStart(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("the editor window did not open")]
    WindowOpen(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("the bench image could not be built")]
    SyntheticImage(#[source] ImageError),
    #[error("the bench shapes could not be added")]
    SyntheticShape(#[source] CommandError),
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
    Application::with_platform(Rc::new(RestrictedPlatform(platform)))
        .with_assets(Icons)
        .run(on_launch);
    Ok(())
}
