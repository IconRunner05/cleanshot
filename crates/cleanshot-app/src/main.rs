#![forbid(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cleanshot_core::{
    AfterCapture, Hotkey, MAX_SETTINGS_BYTES, RgbaImage, SaveFormat, Settings, SettingsProblem,
};
use cleanshot_render::{Format, encode};
use cleanshot_win::capture::{Desktop, RECT, capture_desktop, monitor_under_cursor, region};
use cleanshot_win::clipboard::copy_image;
use cleanshot_win::instance::Launch;
use cleanshot_win::overlay::{Overlay, OverlayEvent};
use cleanshot_win::save::{self, Stamp};
use cleanshot_win::settings_file::{read_settings, settings_path};
use cleanshot_win::tray::{
    Request, Tray, ask_running_instance_to_capture, print_screen_fallback,
    snipping_tool_holds_print_screen,
};

fn main() {
    cleanshot_win::load_dlls_from_system32_only().expect("the DLL search path is restricted");
    let instance = match Launch::claim().expect("the single-instance mutex opens") {
        Launch::First(instance) => instance,
        Launch::AlreadyRunning => {
            // C-14: a second launch asks the running one to capture, then exits. If that
            // one is still starting it has no tray window yet, and this launch is lost.
            if let Err(error) = ask_running_instance_to_capture() {
                eprintln!("Cleanshot is already running and could not be reached: {error}");
            }
            return;
        }
    };
    let (settings, warning) = load_settings();
    let (region, fallback) = region_hotkey(settings.hotkeys.region);
    let hotkeys = [
        (Request::CaptureRegion, region),
        (Request::CaptureScreen, settings.hotkeys.full_screen),
        (Request::CaptureLastRegion, settings.hotkeys.last_region),
    ];
    let tray = Rc::new(RefCell::new(None));
    let launched_tray = Rc::clone(&tray);
    let session = Rc::new(Session {
        settings,
        overlay: RefCell::new(None),
        last_selection: Cell::new(None),
    });
    cleanshot_editor::run(move |cx| {
        let app = cx.to_async();
        let created = Tray::create(&hotkeys, move |request| {
            let session = Rc::clone(&session);
            let closer = app.clone();
            let defer = move |job: Box<dyn FnOnce()>| closer.spawn(async move |_| job()).detach();
            // The tray's window procedure can run while GPUI holds the app, so the request
            // waits for GPUI's executor instead of borrowing the app here.
            app.spawn(async move |cx| {
                cx.update(|cx| match request {
                    Request::CaptureRegion => capture(&session, Capture::Region, defer),
                    Request::CaptureScreen => capture(&session, Capture::Screen, defer),
                    Request::CaptureLastRegion => capture(&session, Capture::LastRegion, defer),
                    Request::Exit => cx.quit(),
                });
            })
            .detach();
        })
        .expect("the tray window and icon are created");
        // F-51: a hotkey another program holds is shown, not swallowed.
        let taken = created
            .hotkeys()
            .filter(|(_, _, registered)| registered.is_err())
            .map(|(_, hotkey, _)| {
                format!("{hotkey} is taken by another program, so it does not capture. Choose another in cleanshot.toml.")
            });
        let warnings: Vec<String> = warning.into_iter().chain(fallback).chain(taken).collect();
        if !warnings.is_empty() {
            let text = warnings.join(" ");
            if let Err(error) = created.warn("Cleanshot", &text) {
                eprintln!("{text} ({error})");
            }
        }
        launched_tray.replace(Some(created));
    })
    .expect("the Windows platform starts");
    drop(tray);
    drop(instance);
}

/// What lives as long as the tray: read once at start, or held between captures.
struct Session {
    settings: Settings,
    overlay: RefCell<Option<Overlay>>,
    /// F-06: the last selection copied or saved, kept in memory only (C-20).
    last_selection: Cell<Option<RECT>>,
}

#[derive(Clone, Copy, Debug)]
enum Capture {
    Region,
    Screen,
    LastRegion,
}

#[derive(Debug, PartialEq)]
enum Plan {
    Copy(RECT),
    Overlay(Option<RECT>),
}

/// What a capture of `kind` does once the screen is copied, or None when the pointer's
/// monitor is unknown. F-07 follows `after_capture` (D-34); F-06 copies the last selection
/// with no overlay, or opens one when there is none yet (D-35).
fn plan(
    kind: Capture,
    after_capture: AfterCapture,
    last_selection: Option<RECT>,
    pointer_monitor: impl FnOnce() -> Option<RECT>,
) -> Option<Plan> {
    Some(match (kind, after_capture) {
        (Capture::Region, _) => Plan::Overlay(None),
        (Capture::Screen, AfterCapture::Copy) => Plan::Copy(pointer_monitor()?),
        (Capture::Screen, AfterCapture::ShowToolbar) => Plan::Overlay(Some(pointer_monitor()?)),
        (Capture::LastRegion, _) => last_selection.map_or(Plan::Overlay(None), Plan::Copy),
    })
}

// F-01, F-02: the screen is copied before any window of ours appears, then frozen on every
// monitor until Esc. F-07: a screen capture selects the pointer's monitor, then runs the
// after-capture action. F-06: the last region is copied with no overlay; with none yet, the
// overlay opens as for a region.
fn capture(session: &Rc<Session>, kind: Capture, defer: impl Fn(Box<dyn FnOnce()>) + 'static) {
    if session.overlay.borrow().is_some() {
        return;
    }
    let captures = match capture_desktop() {
        Ok(Desktop::Duplicated(monitors)) => monitors,
        Ok(Desktop::CopiedWithGdi { monitors, because }) => {
            eprintln!("Desktop Duplication was refused, so GDI copied the screen: {because}");
            monitors
        }
        Err(error) => return eprintln!("the screen could not be captured: {error}"),
    };
    let pointer_monitor = || match monitor_under_cursor(&captures) {
        Ok(monitor) => Some(monitor),
        Err(error) => {
            eprintln!("the screen to capture is unknown: {error}");
            None
        }
    };
    let last = session.last_selection.get();
    let selected = match plan(kind, session.settings.after_capture, last, pointer_monitor) {
        None => return,
        Some(Plan::Overlay(selected)) => selected,
        Some(Plan::Copy(selected)) => {
            let copied = region(&captures, selected)
                .map_err(Box::from)
                .and_then(|image| copy(&image));
            if let Err(error) = copied {
                eprintln!("the capture did not finish: {error}");
            }
            return;
        }
    };
    let captures = Rc::new(captures);
    let frozen = Rc::clone(&captures);
    let finishing = Rc::clone(session);
    let shown = Overlay::show(&captures, selected, move |event| {
        let finished = match event {
            OverlayEvent::Cancelled => Ok(()),
            OverlayEvent::Copy(selected) => {
                finishing.last_selection.set(Some(selected));
                region(&frozen, selected)
                    .map_err(Box::from)
                    .and_then(|image| copy(&image))
            }
            OverlayEvent::Save(selected) => {
                finishing.last_selection.set(Some(selected));
                region(&frozen, selected)
                    .map_err(Box::from)
                    .and_then(|image| save(&image, &finishing.settings))
            }
            OverlayEvent::DrawFailed(error) => Err(error.into()),
        };
        if let Err(error) = finished {
            eprintln!("the capture did not finish: {error}");
        }
        // The event arrives inside an overlay window's procedure, so the windows close
        // after it returns.
        let closing = Rc::clone(&finishing);
        defer(Box::new(move || drop(closing.overlay.borrow_mut().take())));
    });
    match shown {
        Ok(shown) => *session.overlay.borrow_mut() = Some(shown),
        Err(error) => eprintln!("the overlay could not open: {error}"),
    }
}

// F-52: Cleanshot never opens Settings for the user (C-11), so it says where the switch is.
fn region_hotkey(configured: Hotkey) -> (Hotkey, Option<String>) {
    let holds = match snipping_tool_holds_print_screen() {
        Ok(holds) => holds,
        Err(error) => {
            let warning =
                format!("Cleanshot could not tell whether Snipping Tool uses PrtSc: {error}.");
            return (configured, Some(warning));
        }
    };
    match print_screen_fallback(configured, holds) {
        None => (configured, None),
        Some(fallback) => (
            fallback,
            Some(format!(
                "Snipping Tool uses PrtSc, so {fallback} captures a region instead. To use PrtSc, turn off \"Use the Print screen key to open screen capture\" in Settings > Accessibility > Keyboard, then restart Cleanshot."
            )),
        ),
    }
}

fn copy(image: &RgbaImage) -> Result<(), Box<dyn std::error::Error>> {
    // C-22: the raw capture goes straight to encode until flatten carries destructive
    // redaction.
    let bitmap = encode(image, Format::ClipboardBitmap)?;
    let png = encode(image, Format::Png)?;
    copy_image(&bitmap, &png)?;
    Ok(())
}

// F-21: the configured folder and format arrive with F-54's settings file.
fn save(image: &RgbaImage, settings: &Settings) -> Result<(), Box<dyn std::error::Error>> {
    let (format, extension) = match settings.save_format {
        SaveFormat::Png => (Format::Png, "png"),
        SaveFormat::Jpg => (Format::Jpeg, "jpg"),
        SaveFormat::Bmp => (Format::Bmp, "bmp"),
    };
    // C-22: as in copy.
    let bytes = encode(image, format)?;
    let folder = save::save_folder(settings.save_folder.as_deref())?;
    save::save(
        &folder,
        "Screenshot {date} {time}",
        Stamp::now(),
        extension,
        &bytes,
    )?;
    Ok(())
}

// F-54: reading never stops Cleanshot; whatever is wrong becomes one warning and the
// defaults stand in.
fn load_settings() -> (Settings, Option<String>) {
    let read = settings_path().and_then(|path| read_settings(&path, MAX_SETTINGS_BYTES));
    let bytes = match read {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return (Settings::default(), None),
        Err(error) => {
            let warning =
                format!("The settings could not be read, so the defaults apply: {error}.");
            return (Settings::default(), Some(warning));
        }
    };
    let (settings, problems) = Settings::from_toml(&bytes);
    if problems.is_empty() {
        return (settings, None);
    }
    let described: Vec<String> = problems.iter().map(describe).collect();
    let warning = format!("Some settings were not used: {}.", described.join("; "));
    (settings, Some(warning))
}

fn describe(problem: &SettingsProblem) -> String {
    match problem {
        SettingsProblem::TooLarge { bytes } => {
            format!(
                "the file is {bytes} bytes, over the {MAX_SETTINGS_BYTES}-byte limit, so every default applies"
            )
        }
        SettingsProblem::Unreadable => {
            "the file is not TOML text, so every default applies".to_owned()
        }
        SettingsProblem::InvalidValue { key } => {
            format!("{key} has a value Cleanshot cannot use, so its default applies")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: RECT = RECT {
        left: -1920,
        top: 0,
        right: 0,
        bottom: 1080,
    };
    const LAST: RECT = RECT {
        left: 10,
        top: 20,
        right: 110,
        bottom: 220,
    };

    #[test]
    fn f07_full_screen_copies_or_preselects_the_pointer_monitor_by_after_capture() {
        let monitor = || Some(MONITOR);
        assert_eq!(
            plan(Capture::Screen, AfterCapture::Copy, None, monitor),
            Some(Plan::Copy(MONITOR))
        );
        assert_eq!(
            plan(Capture::Screen, AfterCapture::ShowToolbar, None, monitor),
            Some(Plan::Overlay(Some(MONITOR)))
        );
        assert_eq!(
            plan(Capture::Screen, AfterCapture::Copy, None, || None),
            None
        );
    }

    #[test]
    fn f06_last_region_copies_the_last_selection_or_opens_the_overlay_without_one() {
        let monitor = || Some(MONITOR);
        for after in [AfterCapture::Copy, AfterCapture::ShowToolbar] {
            assert_eq!(
                plan(Capture::LastRegion, after, Some(LAST), monitor),
                Some(Plan::Copy(LAST))
            );
            assert_eq!(
                plan(Capture::LastRegion, after, None, monitor),
                Some(Plan::Overlay(None))
            );
        }
        assert_eq!(
            plan(Capture::Region, AfterCapture::Copy, Some(LAST), monitor),
            Some(Plan::Overlay(None))
        );
    }
}
