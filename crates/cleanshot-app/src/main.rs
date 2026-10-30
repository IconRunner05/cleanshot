#![forbid(unsafe_code)]
// A console exe opens a console window beside the tray. Debug builds keep theirs, so
// eprintln stays readable while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{
    AfterCapture, Delivered, DeliveryError, Document, Hotkey, MAX_SETTINGS_BYTES, Output,
    SaveFormat, Settings, SettingsProblem, delivery_notice,
};
use cleanshot_render::{Format, encode, flatten};
use cleanshot_win::capture::{
    Desktop, MonitorImage, RECT, capture_desktop, monitor_under_cursor, region,
};
use cleanshot_win::clipboard::copy_image;
use cleanshot_win::instance::Launch;
use cleanshot_win::overlay::{Overlay, OverlayEvent};
use cleanshot_win::save::{self, FolderError, SaveError, Stamp};
use cleanshot_win::settings_file::{read_settings, settings_path};
use cleanshot_win::text;
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
        tray: Rc::clone(&tray),
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
    // The tray's callback holds the session, which holds the tray, so the tray is taken
    // out to remove its icon.
    drop(tray.take());
    drop(instance);
}

/// What lives as long as the tray: read once at start, or held between captures.
struct Session {
    settings: Settings,
    tray: Rc<RefCell<Option<Tray>>>,
    overlay: RefCell<Option<Overlay>>,
    /// F-06: the last selection copied or saved, kept in memory only (C-20).
    last_selection: Cell<Option<RECT>>,
}

impl Session {
    /// Shows `text` from the tray icon. The release exe has no console, so stderr is only
    /// the last resort, for when the tray cannot show it.
    fn warn(&self, text: &str) {
        let shown = match &*self.tray.borrow() {
            Some(tray) => tray.warn("Cleanshot", text),
            None => return eprintln!("{text} (the tray icon is not created yet)"),
        };
        if let Err(error) = shown {
            eprintln!("{text} ({error})");
        }
    }
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
        Err(error) => return session.warn(&format!("The screen could not be captured: {error}.")),
    };
    let pointer_monitor = || match monitor_under_cursor(&captures) {
        Ok(monitor) => Some(monitor),
        Err(error) => {
            session.warn(&format!(
                "Cleanshot could not tell which screen the pointer is on: {error}."
            ));
            None
        }
    };
    let last = session.last_selection.get();
    let selected = match plan(kind, session.settings.after_capture, last, pointer_monitor) {
        None => return,
        Some(Plan::Overlay(selected)) => selected,
        Some(Plan::Copy(selected)) => {
            return finish(session, &captures, selected, Output::Copy);
        }
    };
    let captures = Rc::new(captures);
    let frozen = Rc::clone(&captures);
    let finishing = Rc::clone(session);
    let shown = Overlay::show(&captures, selected, move |event| {
        match event {
            OverlayEvent::Cancelled => {}
            OverlayEvent::Copy(selected) => {
                finishing.last_selection.set(Some(selected));
                finish(&finishing, &frozen, selected, Output::Copy);
            }
            OverlayEvent::Save(selected) => {
                finishing.last_selection.set(Some(selected));
                finish(&finishing, &frozen, selected, Output::Save);
            }
            OverlayEvent::DrawFailed(error) => {
                finishing.warn(&format!("The overlay could not draw: {error}."));
            }
        }
        // The event arrives inside an overlay window's procedure, so the windows close
        // after it returns.
        let closing = Rc::clone(&finishing);
        defer(Box::new(move || drop(closing.overlay.borrow_mut().take())));
    });
    match shown {
        Ok(shown) => *session.overlay.borrow_mut() = Some(shown),
        Err(error) => session.warn(&format!("The overlay could not open: {error}.")),
    }
}

// The selection becomes a document with nothing drawn on it, so the overlay delivers through
// the same path as the editor.
fn finish(session: &Session, captures: &[MonitorImage], selected: RECT, output: Output) {
    let image = match region(captures, selected) {
        Ok(image) => image,
        Err(error) => return session.warn(&format!("The capture did not finish: {error}.")),
    };
    if let Err(error) = deliver(output, &Document::new(Arc::new(image)), &session.settings) {
        session.warn(&delivery_notice(&Err(error)));
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

const FILE_NAME_TEMPLATE: &str = "Screenshot {date} {time}";

/// Flattens `document`, redaction included (C-22), and copies it (F-20) or saves it in the
/// configured folder and format (F-21). The overlay and the editor both finish here (D-48).
fn deliver(
    output: Output,
    document: &Document,
    settings: &Settings,
) -> Result<Delivered, DeliveryError> {
    let masks = text::masks(document).map_err(|cause| {
        eprintln!("DirectWrite could not draw the text: {cause}");
        DeliveryError::TextNotDrawn
    })?;
    let image = flatten(document, &masks).map_err(|error| from_render(&error))?;
    match output {
        Output::Copy => {
            let bitmap =
                encode(&image, Format::ClipboardBitmap).map_err(|error| from_render(&error))?;
            let png = encode(&image, Format::Png).map_err(|error| from_render(&error))?;
            copy_image(&bitmap, &png).map_err(|cause| {
                eprintln!("the clipboard refused the image: {cause}");
                DeliveryError::ClipboardBusy
            })?;
            Ok(Delivered::Copied)
        }
        Output::Save => {
            let (format, extension) = match settings.save_format {
                SaveFormat::Png => (Format::Png, "png"),
                SaveFormat::Jpg => (Format::Jpeg, "jpg"),
                SaveFormat::Bmp => (Format::Bmp, "bmp"),
            };
            let bytes = encode(&image, format).map_err(|error| from_render(&error))?;
            let folder = save::save_folder(settings.save_folder.as_deref()).map_err(from_folder)?;
            save::save(&folder, FILE_NAME_TEMPLATE, Stamp::now(), extension, &bytes)
                .map(Delivered::Saved)
                .map_err(|error| from_save(error, &folder))
        }
    }
}

fn from_render(error: &cleanshot_render::Error) -> DeliveryError {
    match error {
        &cleanshot_render::Error::TooLarge { width, height } => {
            DeliveryError::TooLarge { width, height }
        }
        cleanshot_render::Error::MissingTextMask(_) => DeliveryError::TextNotDrawn,
        cleanshot_render::Error::CropOutsideImage(_)
        | cleanshot_render::Error::Output(_)
        | cleanshot_render::Error::Encode(_) => {
            panic!("a document from core flattens and encodes: {error}")
        }
    }
}

fn from_folder(error: FolderError) -> DeliveryError {
    match error {
        FolderError::NoFolder(cause) => {
            eprintln!("the screenshots folder is unknown: {cause}");
            DeliveryError::NoSaveFolder
        }
        FolderError::RelativeFolder(configured) => DeliveryError::SaveFolderNotFull { configured },
    }
}

fn from_save(error: SaveError, folder: &Path) -> DeliveryError {
    match error {
        SaveError::NoFreeName(name) => DeliveryError::NoFreeName { name },
        SaveError::Write(cause) => {
            eprintln!("{} could not be written: {cause}", folder.display());
            DeliveryError::NotWritten {
                folder: folder.to_owned(),
            }
        }
        SaveError::UnknownField(_) | SaveError::UnsafeName(_) => {
            panic!("the file-name template is a safe constant: {error}")
        }
    }
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
    use cleanshot_core::{
        Color, Command, History, PointF, RectF, RedactMode, RgbaImage, Shape, Style, TextBlock,
    };

    use super::*;

    fn redacted_document() -> Document {
        let pixels = (0..=255u8)
            .flat_map(|i| [i.wrapping_mul(37), 0, 255, 255])
            .collect();
        let image = RgbaImage::new(16, 16, pixels).expect("16x16 RGBA");
        let mut history = History::new(Document::new(Arc::new(image)));
        history
            .apply(Command::Add {
                shape: Shape::Redact {
                    rect: RectF {
                        x: 0.0,
                        y: 0.0,
                        width: 16.0,
                        height: 16.0,
                    },
                    mode: RedactMode::Pixelate,
                },
                style: Style {
                    color: Color {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 255,
                    },
                    stroke_width: 1.0,
                },
            })
            .expect("the redaction is added");
        history.document().clone()
    }

    fn settings_saving_to(folder: &str) -> Settings {
        Settings {
            save_folder: Some(folder.to_owned()),
            save_format: SaveFormat::Bmp,
            ..Settings::default()
        }
    }

    #[test]
    fn c22_a_saved_capture_holds_the_redacted_pixels_not_the_capture() {
        let folder = std::env::temp_dir().join(format!("cleanshot-deliver-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("the test folder is created");
        let document = redacted_document();
        let settings = settings_saving_to(folder.to_str().expect("a UTF-8 temp path"));
        let Ok(Delivered::Saved(path)) = deliver(Output::Save, &document, &settings) else {
            panic!("the save did not deliver a file");
        };
        let written = std::fs::read(&path).expect("the saved file is readable");
        std::fs::remove_dir_all(&folder).expect("the test folder is removable");
        let masks = text::masks(&document).expect("the document has no text to draw");
        let flattened = flatten(&document, &masks).expect("the document flattens");
        assert_eq!(written, encode(&flattened, Format::Bmp).expect("encodes"));
        assert_ne!(
            written,
            encode(document.base(), Format::Bmp).expect("encodes")
        );
    }

    #[test]
    fn f34_a_saved_document_holds_its_text_drawn_by_directwrite() {
        let folder = std::env::temp_dir().join(format!("cleanshot-text-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("the test folder is created");
        let image = RgbaImage::new(64, 40, vec![255; 64 * 40 * 4]).expect("64x40 RGBA");
        let mut history = History::new(Document::new(Arc::new(image)));
        let block = TextBlock::new(PointF { x: 4.0, y: 4.0 }, "Hg".to_owned(), 24.0, 1.0)
            .expect("a positive size and a measured width");
        history
            .apply(Command::Add {
                shape: Shape::Text(block),
                style: Style {
                    color: Color {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 255,
                    },
                    stroke_width: 1.0,
                },
            })
            .expect("the text is added");
        let document = history.document().clone();
        let settings = settings_saving_to(folder.to_str().expect("a UTF-8 temp path"));
        let saved = deliver(Output::Save, &document, &settings);
        let written = match &saved {
            Ok(Delivered::Saved(path)) => std::fs::read(path).ok(),
            _ => None,
        };
        std::fs::remove_dir_all(&folder).expect("the test folder is removable");
        let masks = text::masks(&document).expect("DirectWrite draws Hg");
        let flattened = flatten(&document, &masks).expect("the document flattens");
        assert_eq!(
            written,
            Some(encode(&flattened, Format::Bmp).expect("encodes")),
            "{saved:?}"
        );
        assert_ne!(
            flattened,
            *document.base().as_ref(),
            "the text left the image white"
        );
    }

    #[test]
    fn f21_each_save_failure_names_what_the_user_can_fix() {
        let document = redacted_document();
        assert_eq!(
            deliver(Output::Save, &document, &settings_saving_to("Shots")),
            Err(DeliveryError::SaveFolderNotFull {
                configured: "Shots".to_owned()
            })
        );
        let missing =
            std::env::temp_dir().join(format!("cleanshot-missing-{}", std::process::id()));
        let settings = settings_saving_to(missing.to_str().expect("a UTF-8 temp path"));
        assert_eq!(
            deliver(Output::Save, &document, &settings),
            Err(DeliveryError::NotWritten { folder: missing })
        );
    }

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
