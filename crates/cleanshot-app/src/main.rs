#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::rc::Rc;

use cleanshot_core::{MAX_SETTINGS_BYTES, RgbaImage, SaveFormat, Settings, SettingsProblem};
use cleanshot_render::{Format, encode};
use cleanshot_win::capture::{Desktop, capture_desktop, region};
use cleanshot_win::clipboard::copy_image;
use cleanshot_win::instance::Launch;
use cleanshot_win::overlay::{Overlay, OverlayEvent};
use cleanshot_win::save::{self, Stamp};
use cleanshot_win::settings_file::{read_settings, settings_path};
use cleanshot_win::tray::{Request, Tray, ask_running_instance_to_capture};

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
    let settings = Rc::new(settings);
    let tray = Rc::new(RefCell::new(None));
    let launched_tray = Rc::clone(&tray);
    let overlay: Rc<RefCell<Option<Overlay>>> = Rc::new(RefCell::new(None));
    cleanshot_editor::run(move |cx| {
        let app = cx.to_async();
        let overlay = Rc::clone(&overlay);
        let created = Tray::create(move |request| {
            let overlay = Rc::clone(&overlay);
            let settings = Rc::clone(&settings);
            let closer = app.clone();
            // The tray's window procedure can run while GPUI holds the app, so the request
            // waits for GPUI's executor instead of borrowing the app here.
            app.spawn(async move |cx| {
                cx.update(|cx| match request {
                    Request::CaptureRegion => open_overlay(&overlay, settings, move |job| {
                        closer.spawn(async move |_| job()).detach();
                    }),
                    Request::Exit => cx.quit(),
                });
            })
            .detach();
        })
        .expect("the tray window and icon are created");
        if let Some(warning) = warning
            && let Err(error) = created.warn("Cleanshot settings", &warning)
        {
            eprintln!("{warning} ({error})");
        }
        launched_tray.replace(Some(created));
    })
    .expect("the Windows platform starts");
    drop(tray);
    drop(instance);
}

// F-01, F-02: the screen is copied before any window of ours appears, then frozen on every
// monitor until Esc.
fn open_overlay(
    overlay: &Rc<RefCell<Option<Overlay>>>,
    settings: Rc<Settings>,
    defer: impl Fn(Box<dyn FnOnce()>) + 'static,
) {
    if overlay.borrow().is_some() {
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
    let captures = Rc::new(captures);
    let frozen = Rc::clone(&captures);
    let closing = Rc::clone(overlay);
    let shown = Overlay::show(&captures, move |event| {
        let finished = match event {
            OverlayEvent::Cancelled => Ok(()),
            OverlayEvent::Copy(selected) => region(&frozen, selected)
                .map_err(Box::from)
                .and_then(|image| copy(&image)),
            OverlayEvent::Save(selected) => region(&frozen, selected)
                .map_err(Box::from)
                .and_then(|image| save(&image, &settings)),
            OverlayEvent::DrawFailed(error) => Err(error.into()),
        };
        if let Err(error) = finished {
            eprintln!("the capture did not finish: {error}");
        }
        // The event arrives inside an overlay window's procedure, so the windows close
        // after it returns.
        let closing = Rc::clone(&closing);
        defer(Box::new(move || drop(closing.borrow_mut().take())));
    });
    match shown {
        Ok(shown) => *overlay.borrow_mut() = Some(shown),
        Err(error) => eprintln!("the overlay could not open: {error}"),
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
