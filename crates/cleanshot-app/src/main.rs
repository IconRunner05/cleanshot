#![forbid(unsafe_code)]
// A console exe opens a console window beside the tray. Debug builds keep theirs, so
// eprintln stays readable while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cleanshot_core::{
    AfterCapture, Delivered, DeliveryError, Document, Hotkey, MAX_SETTINGS_BYTES, Output,
    RgbaImage, SaveFormat, Settings, SettingsProblem, delivery_notice,
};
use cleanshot_editor::ask_save_path;
use cleanshot_render::{Flattened, Format, encode, flatten};
use cleanshot_win::capture::{
    Desktop, MonitorImage, RECT, capture_desktop, monitor_under_cursor, region,
};
use cleanshot_win::clipboard::copy_image;
use cleanshot_win::instance::Launch;
use cleanshot_win::log::{Log, Usage, since_start, usage};
use cleanshot_win::overlay::{Overlay, OverlayEvent};
use cleanshot_win::save::{self, FolderError, SaveError, Stamp};
use cleanshot_win::settings_file::{read_settings, settings_path};
use cleanshot_win::text;
use cleanshot_win::tray::{
    Request, Tray, ask_running_instance_to_capture, message_box, print_screen_fallback,
    snipping_tool_holds_print_screen,
};

fn main() {
    cleanshot_win::load_dlls_from_system32_only().expect("the DLL search path is restricted");
    let instance = match Launch::claim().expect("the single-instance mutex opens") {
        Launch::First(instance) => instance,
        Launch::AlreadyRunning => {
            // C-14: a second launch asks the running one to capture, or says it is still starting.
            if let Err(error) = ask_running_instance_to_capture() {
                let text = format!(
                    "Cleanshot is already running but did not answer ({error}). Wait a few seconds and start it again. If it still does not answer, end cleanshot.exe in Task Manager and start Cleanshot again."
                );
                if let Err(shown) = message_box("Cleanshot", &text) {
                    eprintln!("{text} ({shown})");
                }
            }
            return;
        }
    };
    let log = std::env::args().skip(1).any(|argument| argument == "--log");
    let (log, log_warning) = match log.then(open_log).transpose() {
        Ok(log) => (log, None),
        Err(error) => (None, Some(format!("The log could not be opened: {error}."))),
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
        log: RefCell::new(log),
    });
    let launched = Rc::clone(&session);
    cleanshot_editor::run(move |cx| {
        let app = cx.to_async();
        let created = Tray::create(&hotkeys, move |request| {
            // P-01 counts from here, inside the tray's handling of the hotkey message.
            let asked = Instant::now();
            let session = Rc::clone(&session);
            let closer = app.clone();
            let defer = move |job: Box<dyn FnOnce()>| closer.spawn(async move |_| job()).detach();
            let opener = app.clone();
            let editing = Rc::clone(&session);
            let pass_on = move |image: RgbaImage, to: Handoff| {
                let session = Rc::clone(&editing);
                opener
                    .spawn(async move |cx| match to {
                        Handoff::Editor => cx.update(|cx| open_editor(cx, image, &session)),
                        Handoff::SaveAs => {
                            let (folder, name) = save_as_start(&session.settings);
                            let chosen = cx.update(|cx| ask_save_path(cx, &folder, &name));
                            save_chosen(&session, image, chosen.await);
                        }
                    })
                    .detach();
            };
            // The tray's window procedure can run while GPUI holds the app, so the request
            // waits for GPUI's executor instead of borrowing the app here.
            app.spawn(async move |cx| {
                cx.update(|cx| {
                    let kind = match request {
                        Request::CaptureRegion => Capture::Region,
                        Request::CaptureScreen => Capture::Screen,
                        Request::CaptureLastRegion => Capture::LastRegion,
                        Request::Exit => return cx.quit(),
                    };
                    capture(&session, kind, asked, defer, pass_on);
                });
            })
            .detach();
        })
        .expect("the tray window and icon are created");
        let text = start_warnings(&created, [warning, log_warning, fallback]);
        launched_tray.replace(Some(created));
        if !text.is_empty() {
            launched.warn(&text);
        }
        launched.perf(|since_start| format!("P-03 tray_ready_ms={}", since_start.as_millis()));
        // P-04, P-05: the working set, and the CPU time spent with the tray idle, logged once.
        if let Some(start) = launched.idle_start() {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(IDLE_SAMPLE).await;
                launched.idle_use(start);
            })
            .detach();
        }
    })
    .expect("the Windows platform starts");
    // The tray's callback holds the session, which holds the tray, so the tray is taken
    // out to remove its icon.
    drop(tray.take());
    drop(instance);
}

/// What start-up has to tell the user, in one line: `earlier`, then each hotkey another program
/// holds (F-51: shown, not swallowed).
fn start_warnings(tray: &Tray, earlier: [Option<String>; 3]) -> String {
    let taken = tray
        .hotkeys()
        .filter(|(_, _, registered)| registered.is_err())
        .map(|(_, hotkey, _)| {
            format!("{hotkey} is taken by another program, so it does not capture. Choose another in cleanshot.toml.")
        });
    earlier
        .into_iter()
        .flatten()
        .chain(taken)
        .collect::<Vec<_>>()
        .join(" ")
}

/// What lives as long as the tray: read once at start, or held between captures.
struct Session {
    settings: Settings,
    tray: Rc<RefCell<Option<Tray>>>,
    overlay: RefCell<Option<Overlay>>,
    /// F-06: the last selection copied or saved, kept in memory only (C-20).
    last_selection: Cell<Option<RECT>>,
    /// C-23: open only under `--log`.
    log: RefCell<Option<Log>>,
}

impl Session {
    /// Under --log, the usage to measure P-05 from; None without it, so normal running starts
    /// no timer (P-05).
    fn idle_start(&self) -> Option<Usage> {
        self.log.borrow().as_ref()?;
        usage()
            .inspect_err(|error| self.warn(&format!("Logging could not sample idle use: {error}.")))
            .ok()
    }

    /// Logs the P-04 working set now and the P-05 CPU time spent since `start`.
    fn idle_use(&self, start: Usage) {
        let end = match usage() {
            Ok(end) => end,
            Err(error) => {
                return self.warn(&format!("Logging could not sample idle use: {error}."));
            }
        };
        self.perf(|_| format!("P-04 working_set_bytes={}", end.working_set_bytes));
        self.perf(|_| {
            format!(
                "P-05 cpu_ms={} over_ms={}",
                end.cpu.saturating_sub(start.cpu).as_millis(),
                IDLE_SAMPLE.as_millis()
            )
        });
    }

    /// Logs a `perf` line under `--log`, made from the time since the process started. A log
    /// that fails is closed, and the tray says so once.
    fn perf(&self, line: impl FnOnce(Duration) -> String) {
        let written = match &*self.log.borrow() {
            Some(log) => {
                since_start().and_then(|since| log.write(&format!("perf {}", line(since))))
            }
            None => return,
        };
        if let Err(error) = written {
            drop(self.log.borrow_mut().take());
            self.warn(&format!("Logging stopped: {error}."));
        }
    }

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

/// Where the overlay hands a selection on to GPUI: the editor (F-05), or the Save As dialog
/// once the overlay has closed (F-22).
#[derive(Clone, Copy, Debug)]
enum Handoff {
    Editor,
    SaveAs,
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
fn capture(
    session: &Rc<Session>,
    kind: Capture,
    asked: Instant,
    defer: impl Fn(Box<dyn FnOnce()>) + 'static,
    pass_on: impl Fn(RgbaImage, Handoff) + 'static,
) {
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
    let pass_on = Rc::new(pass_on);
    let shown = Overlay::show(&captures, selected, move |event| {
        // F-13 (D-68): the editor takes over from the overlay with no fade between them.
        let handoff = matches!(event, OverlayEvent::Edit(_));
        let mut saving_as = None;
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
            OverlayEvent::SaveAs(selected) => {
                finishing.last_selection.set(Some(selected));
                match region(&frozen, selected) {
                    Ok(image) => saving_as = Some(image),
                    Err(error) => {
                        finishing.warn(&format!("The capture did not finish: {error}."));
                    }
                }
            }
            OverlayEvent::Edit(selected) => match region(&frozen, selected) {
                Ok(image) => pass_on(image, Handoff::Editor),
                Err(error) => {
                    finishing.warn(&format!("The capture could not be edited: {error}."));
                }
            },
            OverlayEvent::DrawFailed(error) => {
                finishing.warn(&format!("The overlay could not draw: {error}."));
            }
        }
        // The event arrives inside an overlay window's procedure, so the windows close
        // after it returns, and the Save As dialog opens over the desktop, not the overlay.
        let closing = Rc::clone(&finishing);
        let pass_on = Rc::clone(&pass_on);
        defer(Box::new(move || {
            let overlay = closing.overlay.borrow_mut().take();
            if let Some(overlay) = overlay {
                if handoff {
                    drop(overlay);
                } else if let Err(error) = overlay.close() {
                    closing.warn(&format!("The overlay could not fade out: {error}."));
                }
            }
            if let Some(image) = saving_as {
                pass_on(image, Handoff::SaveAs);
            }
        }));
    });
    match shown {
        Ok(shown) => {
            *session.overlay.borrow_mut() = Some(shown);
            session.perf(|_| format!("P-01 overlay_visible_ms={}", asked.elapsed().as_millis()));
        }
        Err(error) => session.warn(&format!("The overlay could not open: {error}.")),
    }
}

// The selection becomes a document with nothing drawn on it, so the overlay delivers through
// the same path as the editor.
fn finish(session: &Session, captures: &[MonitorImage], selected: RECT, output: Output) {
    let started = Instant::now();
    let image = match region(captures, selected) {
        Ok(image) => image,
        Err(error) => return session.warn(&format!("The capture did not finish: {error}.")),
    };
    match deliver(output, &Document::new(Arc::new(image)), &session.settings) {
        Ok(Delivered::Copied) => {
            session.perf(|_| format!("P-02 copied_ms={}", started.elapsed().as_millis()));
        }
        Ok(Delivered::Saved(_)) => {}
        Err(error) => session.warn(&delivery_notice(&Err(error))),
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
    let image = flattened(document)?;
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
            let (format, extension) = encoding(settings.save_format);
            let bytes = encode(&image, format).map_err(|error| from_render(&error))?;
            let folder = save::save_folder(settings.save_folder.as_deref()).map_err(from_folder)?;
            save::save(&folder, FILE_NAME_TEMPLATE, Stamp::now(), extension, &bytes)
                .map(Delivered::Saved)
                .map_err(|error| from_save(error, &folder))
        }
    }
}

/// F-05: opens `image` in the editor, or says in the tray why it could not.
fn open_editor(cx: &mut cleanshot_editor::App, image: RgbaImage, session: &Rc<Session>) {
    if let Err(error) = cleanshot_editor::open(
        cx,
        image,
        editor_deliver(session),
        editor_save_as_start(session),
        editor_save_to(session),
        editor_first_frame(session),
    ) {
        session.warn(&format!("The editor could not open: {error}."));
    }
}

/// F-05: the editor's Copy and Save deliver as the overlay does (D-48).
fn editor_deliver(
    session: &Rc<Session>,
) -> impl Fn(Output, &Document) -> Result<Delivered, DeliveryError> + 'static {
    let session = Rc::clone(session);
    move |output, document| deliver(output, document, &session.settings)
}

fn editor_save_as_start(session: &Rc<Session>) -> impl Fn() -> (PathBuf, String) + 'static {
    let session = Rc::clone(session);
    move || save_as_start(&session.settings)
}

fn editor_save_to(
    session: &Rc<Session>,
) -> impl Fn(&Path, &Document) -> Result<Delivered, DeliveryError> + 'static {
    let session = Rc::clone(session);
    move |path, document| save_to(path, document, &session.settings)
}

/// P-07: logs the time from now, as the editor opens, to the frame after its first draw.
fn editor_first_frame(session: &Rc<Session>) -> impl FnOnce(Instant) + 'static {
    let session = Rc::clone(session);
    let opened = Instant::now();
    move |shown| {
        session.perf(|_| format!("P-07 first_frame_ms={}", (shown - opened).as_millis()));
    }
}

fn flattened(document: &Document) -> Result<Flattened, DeliveryError> {
    let masks = text::masks(document).map_err(|cause| {
        eprintln!("DirectWrite could not draw the text: {cause}");
        DeliveryError::TextNotDrawn
    })?;
    flatten(document, &masks).map_err(|error| from_render(&error))
}

fn encoding(format: SaveFormat) -> (Format, &'static str) {
    match format {
        SaveFormat::Png => (Format::Png, "png"),
        SaveFormat::Jpg => (Format::Jpeg, "jpg"),
        SaveFormat::Bmp => (Format::Bmp, "bmp"),
    }
}

/// F-22 (D-54): the folder Save As opens on and the name it offers, Save's own at this moment.
fn save_as_start(settings: &Settings) -> (PathBuf, String) {
    // A folder that cannot be named leaves the dialog on its own default; the user picks one
    // there, and a save into it reports its own failure.
    let folder = save::save_folder(settings.save_folder.as_deref()).unwrap_or_default();
    let (_, extension) = encoding(settings.save_format);
    let name = save::offered_name(FILE_NAME_TEMPLATE, Stamp::now(), extension)
        .unwrap_or_else(|error| panic!("the file-name template is a safe constant: {error}"));
    (folder, name)
}

/// F-22: saves the overlay's `image` to the path chosen in Save As. A cancel and a save show
/// nothing, as the overlay's Save does; a failure is shown from the tray.
fn save_chosen(
    session: &Session,
    image: RgbaImage,
    chosen: Option<Result<PathBuf, DeliveryError>>,
) {
    let Some(chosen) = chosen else {
        return;
    };
    let document = Document::new(Arc::new(image));
    if let Err(error) = chosen.and_then(|path| save_to(&path, &document, &session.settings)) {
        session.warn(&delivery_notice(&Err(error)));
    }
}

/// F-22 (D-66): flattens `document` and writes it to `path`, the name the user confirmed in
/// Save As, replacing a file there. The extension picks the format; any other name is written
/// in the configured format, under exactly that name.
fn save_to(
    path: &Path,
    document: &Document,
    settings: &Settings,
) -> Result<Delivered, DeliveryError> {
    let image = flattened(document)?;
    let (format, _) = encoding(typed_format(path).unwrap_or(settings.save_format));
    let bytes = encode(&image, format).map_err(|error| from_render(&error))?;
    let folder = path.parent().unwrap_or(path);
    match save::replace(path, &bytes) {
        Ok(()) => Ok(Delivered::Saved(path.to_owned())),
        Err(SaveError::Write(cause)) => {
            eprintln!("{} could not be written: {cause}", folder.display());
            Err(DeliveryError::NotWritten {
                folder: folder.to_owned(),
            })
        }
        Err(error @ (SaveError::UnsafeName(_) | SaveError::NoFreeName(_))) => {
            eprintln!("{} could not be written: {error}", path.display());
            Err(DeliveryError::NotWritten {
                folder: folder.to_owned(),
            })
        }
        Err(error @ SaveError::UnknownField(_)) => {
            panic!("save::replace expands no file-name template: {error}")
        }
    }
}

fn typed_format(path: &Path) -> Option<SaveFormat> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some(SaveFormat::Png),
        "jpg" | "jpeg" => Some(SaveFormat::Jpg),
        "bmp" => Some(SaveFormat::Bmp),
        _ => None,
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

const IDLE_SAMPLE: Duration = Duration::from_secs(10);

fn open_log() -> Result<Log, String> {
    let settings = settings_path().map_err(|error| error.to_string())?;
    Log::open(&settings).map_err(|error| error.to_string())
}

// F-54: reading never stops Cleanshot; what is wrong becomes one warning, and defaults stand in.
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
    #[cfg_attr(
        debug_assertions,
        ignore = "P-02 budgets the release build: cargo test --release -p cleanshot-app p02_"
    )]
    fn p02_a_4k_region_reaches_the_clipboard_within_150_ms() {
        let (width, height) = (3840, 2160);
        let pixels = (0..height)
            .flat_map(|y: u32| {
                (0..width).flat_map(move |x: u32| {
                    let [x0, y0] = [x.to_le_bytes()[0], y.to_le_bytes()[0]];
                    [x0, y0, x0 ^ y0, 255]
                })
            })
            .collect();
        let image = RgbaImage::new(width, height, pixels).expect("3840x2160 RGBA");
        let document = Document::new(Arc::new(image));
        let mut copies: Vec<Duration> = (0..5)
            .map(|_| {
                let started = Instant::now();
                let copied = deliver(Output::Copy, &document, &Settings::default());
                assert_eq!(copied, Ok(Delivered::Copied));
                started.elapsed()
            })
            .collect();
        copies.sort();
        println!(
            "P-02 median copy of a 4K region {:?}, all {copies:?}",
            copies[2]
        );
        assert!(
            copies[2] < Duration::from_millis(150),
            "median copy of a 4K region {:?}, all {copies:?}",
            copies[2]
        );
    }

    #[test]
    fn f22_save_as_picks_the_format_from_the_typed_extension_and_replaces_the_file() {
        let folder = std::env::temp_dir().join(format!("cleanshot-save-as-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("the test folder is created");
        let document = redacted_document();
        let settings = settings_saving_to(folder.to_str().expect("a UTF-8 temp path"));
        let typed = folder.join("Shot.JPG");
        std::fs::write(&typed, b"old").expect("the file to replace is written");
        let bare = folder.join("Shot");
        let saved = [&typed, &bare].map(|path| {
            assert_eq!(
                save_to(path, &document, &settings),
                Ok(Delivered::Saved(path.clone()))
            );
            std::fs::read(path).expect("the saved file is readable")
        });
        std::fs::remove_dir_all(&folder).expect("the test folder is removable");
        assert!(
            saved[0].starts_with(&[0xFF, 0xD8]),
            "Shot.JPG is not a JPEG"
        );
        assert!(
            saved[1].starts_with(b"BM"),
            "Shot is not the configured BMP"
        );
    }

    #[test]
    fn f22_a_save_as_path_with_no_file_name_is_reported_not_fatal() {
        let folder = std::env::temp_dir();
        let settings = settings_saving_to(folder.to_str().expect("a UTF-8 temp path"));
        assert_eq!(
            save_to(&folder.join(".."), &redacted_document(), &settings),
            Err(DeliveryError::NotWritten { folder })
        );
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
        assert_ne!(flattened.image(), document.base().as_ref());
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
            flattened.image(),
            document.base().as_ref(),
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
