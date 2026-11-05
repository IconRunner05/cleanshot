#![forbid(unsafe_code)]

mod bench;
mod canvas;
mod editor_window;
mod placement;
mod restricted_platform;
mod settings_window;
mod shape_menu;
mod strip;
mod text;
mod text_box;
mod theme;
mod toolbar;
mod tooltip;

use std::num::NonZeroIsize;
use std::rc::Rc;

use cleanshot_core::{CommandError, ImageError};
pub use gpui::{App, AsyncApp, Keystroke};
use gpui::{Application, Entity, QuitMode, Render, Window, WindowHandle, WindowOptions};
use gpui_windows::WindowsPlatform;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub use crate::bench::{BenchReport, bench};
pub use crate::editor_window::{EditorWindow, Outputs, ask_save_path, open};
use crate::restricted_platform::RestrictedPlatform;
pub use crate::settings_window::open_settings;
#[cfg(feature = "drift-test")]
pub use crate::text::em_widths;
use crate::toolbar::Icons;
pub use crate::tooltip::follow_hover_time;

/// Where the editor writes a diagnostic for the --log file: fixed words, never an error's own
/// text, which can name a path or hold what the user typed (C-23).
type Log = Rc<dyn Fn(&'static str)>;

/// The glyph masks of every text in a document as the export draws them, or `None` when they
/// could not be made, which the giver has logged (D-131).
type MasksFor = Rc<dyn Fn(&cleanshot_core::Document) -> Option<cleanshot_core::TextMasks>>;

/// Why the editor could not start, open a window or run the bench.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the Windows platform did not start")]
    PlatformStart(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("the Inter font could not be loaded")]
    FontLoad(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("the editor window did not open")]
    WindowOpen(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("the bench image could not be built")]
    SyntheticImage(#[source] ImageError),
    #[error("the bench shapes could not be added")]
    SyntheticShape(#[source] CommandError),
    #[error("the organisation's policy keeps the settings window closed")]
    SettingsHidden,
}

/// Runs the GPUI event loop on this thread until something calls `quit`, calling `on_launch`
/// once it is ready. Closing the last window does not quit: the tray outlives every window.
///
/// # Errors
///
/// [`Error::PlatformStart`] if the Windows platform cannot be created.
pub fn run(on_launch: impl FnOnce(&mut App) + 'static) -> Result<(), Error> {
    let platform =
        WindowsPlatform::new(false).map_err(|error| Error::PlatformStart(error.into()))?;
    Application::with_platform(Rc::new(RestrictedPlatform(platform)))
        .with_assets(Icons)
        .with_quit_mode(QuitMode::Explicit)
        .run(on_launch);
    Ok(())
}

/// Opens a window with `options` on the view `build` makes, with its Win32 handle for the app to
/// bring it to the front once shown (D-124).
fn open_window<V: Render>(
    cx: &mut App,
    options: WindowOptions,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> Result<(WindowHandle<V>, NonZeroIsize), Error> {
    let mut win32 = None;
    let window = cx
        .open_window(options, |window, cx| {
            win32 = Some(win32_handle(window));
            build(window, cx)
        })
        .map_err(|error| Error::WindowOpen(error.into()))?;
    Ok((
        window,
        win32.expect("open_window builds the view before it returns"),
    ))
}

/// The Win32 handle of `window` (D-124).
fn win32_handle(window: &Window) -> NonZeroIsize {
    let handle = HasWindowHandle::window_handle(window)
        .expect("a GPUI window on Windows always has its handle");
    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => handle.hwnd,
        other => unreachable!("GPUI on Windows gives a Win32 handle, not {other:?}"),
    }
}
