#![forbid(unsafe_code)]
// A console exe opens a console window beside the tray. Debug builds keep theirs, so
// eprintln stays readable while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::num::NonZeroIsize;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cleanshot_core::{
    CaptureMode, CapturePanel, DeliveryError, Document, Hotkey, HotkeyAction, MAX_SETTINGS_BYTES,
    Policy, PolicyProblem, RgbaImage, SavingOff, Settings, SettingsProblem, delivery_notice,
};
use cleanshot_editor::{AsyncApp, WindowSurface, ask_save_path};
use cleanshot_win::capture::{
    AdapterReport, Copied, Devices, MonitorImage, OutputReport, RECT, black_share,
};
use cleanshot_win::instance::Launch;
use cleanshot_win::log::memory::Counted;
use cleanshot_win::log::{Log, LogError, os_code, since_start};
use cleanshot_win::overlay::Overlay;
use cleanshot_win::panel::{Countdown, Panel};
use cleanshot_win::policy::read_policy;
use cleanshot_win::settings_file::{PortableIgnored, read_settings, settings_location};
use cleanshot_win::tray::{
    Request, Tray, ask_running_instance_to_capture, message_box, print_screen_fallback,
    snipping_tool_holds_print_screen,
};

mod capture;
mod delivery;
mod editor;
mod memory;
mod panel;
mod settings_window;
mod thumbnail;

use capture::{Handoff, Shot, capture};
use delivery::{save_as_start, save_to};
use editor::open_editor;
use memory::IDLE_SAMPLE;
use settings_window::{open_settings, remember_looks};

fn main() {
    let entered = since_start();
    cleanshot_win::load_dlls_from_system32_only().expect("the DLL search path is restricted");
    let instance = match Launch::claim() {
        Ok(Launch::First(instance)) => instance,
        // Another process in the session can hold the name as another kind of object.
        Err(error) => {
            tell_user(&format!(
                "Cleanshot could not start ({error}). Restart Windows, then start Cleanshot again."
            ));
            return;
        }
        Ok(Launch::AlreadyRunning) => {
            // A second launch asks the running one to capture, or says it is still starting.
            if let Err(error) = ask_running_instance_to_capture() {
                tell_user(&format!(
                    "Cleanshot is already running but did not answer ({error}). Wait a few seconds and start it again. If it still does not answer, end cleanshot.exe in Task Manager and start Cleanshot again."
                ));
            }
            return;
        }
    };
    let log = std::env::args().skip(1).any(|argument| argument == "--log");
    let (log, log_warning) = match log.then(open_log).transpose() {
        Ok(log) => (log, None),
        Err(error) => (None, Some(format!("The log could not be opened: {error}."))),
    };
    cleanshot_win::crash::install(log.clone());
    std::sync::LazyLock::force(&delivery::FILE_NAME_TEMPLATE);
    let (settings, warning) = load_settings();
    let (policy, policy_problems) = read_policy();
    let (hotkeys, fallback) = tray_hotkeys(&settings);
    // Counters only, as a full account would slow start-up.
    let log_opened = log.is_some().then(cleanshot_win::log::memory::counted);
    let log = Rc::new(RefCell::new(log));
    cleanshot_editor::run(move |cx| {
        let gpui_started = since_start();
        let app = cx.to_async();
        let session = Rc::new_cyclic(|session: &Weak<Session>| {
            let (session, closing_log) = (session.clone(), Rc::clone(&log));
            let tray = Tray::create(&hotkeys, move |request| {
                let Some(session) = session.upgrade() else {
                    return log_dropped(&closing_log, request);
                };
                on_tray_request(&app, session, request);
            })
            .expect("the tray window and icon are created");
            Session::new(settings, policy, tray, log)
        });
        started(
            cx,
            session,
            policy_problems,
            [warning, log_warning, fallback],
            [("main_entered", entered), ("gpui_started", gpui_started)],
            log_opened,
        );
    })
    .expect("the Windows platform starts");
    drop(instance);
}

fn tell_user(text: &str) {
    if let Err(shown) = message_box("Cleanshot", text) {
        eprintln!("{text} ({shown})");
    }
}

fn on_tray_request(app: &AsyncApp, session: Rc<Session>, request: Request) {
    // The overlay time counts from here, inside the tray's handling of the hotkey message.
    let asked = Instant::now();
    let closer = app.clone();
    let defer = move |after: Duration, job: Box<dyn FnOnce()>| {
        let waited = closer.background_executor().timer(after);
        closer
            .spawn(async move |_| {
                waited.await;
                job();
            })
            .detach();
    };
    let opener = app.clone();
    let editing = Rc::clone(&session);
    let pass_on = move |image: RgbaImage, to: Handoff| {
        let session = Rc::clone(&editing);
        opener
            .spawn(async move |cx| match to {
                Handoff::Editor { from } => cx.update(|cx| open_editor(cx, image, from, &session)),
                Handoff::SaveAs => save_as(&session, image, cx).await,
            })
            .detach();
    };
    // The tray's window procedure can run while GPUI holds the app, so the request waits for
    // GPUI's executor instead of borrowing the app here.
    app.spawn(async move |cx| {
        cx.update(|cx| {
            let shot = match request {
                Request::Capture(action) => match action {
                    HotkeyAction::Region => Shot::Mode(CaptureMode::Region),
                    HotkeyAction::FullScreen => Shot::Mode(CaptureMode::FullScreen),
                    HotkeyAction::Window => Shot::Mode(CaptureMode::Window),
                    HotkeyAction::LastRegion => Shot::LastRegion,
                    HotkeyAction::CapturePanel => return panel::open(&session, defer, pass_on),
                },
                Request::EditLastCapture => return thumbnail::edit_last(&session, &pass_on),
                Request::SaveLastCapture => return thumbnail::save_last(&session),
                Request::OpenSettings => return open_settings(cx, &session),
                Request::Exit => return cx.quit(),
            };
            capture(&session, shot, asked, defer, pass_on);
        });
    })
    .detach();
}

/// What start-up does once the tray is up: logs the policy's problems, hides Settings under
/// `HideSettings`, tells the user what went wrong, starts the memory and CPU sample, and
/// hands the session to the app, which holds it until it quits and then removes the icon.
fn started(
    cx: &mut cleanshot_editor::App,
    session: Rc<Session>,
    policy_problems: Vec<PolicyProblem>,
    earlier: [Option<String>; 3],
    marks: [(&str, Result<Duration, LogError>); 2],
    log_opened: Option<Result<Counted, LogError>>,
) {
    remember_looks(cx, &session);
    let reader = Rc::clone(&session);
    cleanshot_editor::follow_window_surface(cx, move || window_surface(&reader));
    // Read once; unread, tooltips keep GPUI's delay.
    match cleanshot_win::hover_time() {
        Ok(hover) => cleanshot_editor::follow_hover_time(cx, hover),
        Err(error) => session.log_failure("editor", "hover_time not_read", &error),
    }
    for problem in policy_problems {
        session.log_line("policy", |_| format!("problem {problem}"));
    }
    if session.policy.settings_hidden() {
        session.tray.borrow().hide_settings_item();
    } else {
        let opener = Rc::clone(&session);
        cleanshot_editor::follow_settings_opener(cx, move |cx| open_settings(cx, &opener));
    }
    for (request, hotkey, registered) in session.tray.borrow().hotkeys() {
        session.log_line("hotkey", |_| match registered {
            Ok(()) => format!("{request:?} {hotkey} registered"),
            Err(error) => format!("{request:?} {hotkey} not_registered {error}"),
        });
    }
    let text = start_warnings(&session.tray.borrow(), earlier);
    if !text.is_empty() {
        session.warn(Warning::Said {
            kind: "start",
            text: &text,
        });
    }
    for (stage, at) in marks {
        match at {
            Ok(at) => session.perf(|_| format!("start {stage}_ms={}", at.as_millis())),
            Err(error) => session.log_failure("perf", "clock_unread", &error),
        }
    }
    session.perf(|since_start| format!("start tray_ready_ms={}", since_start.as_millis()));
    let count = |stage| {
        let logging = session.log.borrow().is_some();
        logging.then(|| (stage, cleanshot_win::log::memory::counted()))
    };
    let tray_ready = count("tray_ready");
    // What the process holds before any graphics, to set against the after-use account.
    session.log_memory("before_graphics");
    // Prewarm on the overlay thread once the tray is ready.
    let unwarmed = Overlay::prewarm().err();
    session.perf(|since_start| format!("start overlay_prewarmed_ms={}", since_start.as_millis()));
    let prewarmed = count("overlay_prewarmed");
    if let Some(error) = unwarmed {
        session.warn(Warning::Failed(
            "The capture overlay could not be prepared",
            &error,
        ));
    }
    // A failed prewarm only leaves the first capture to make its device.
    if let Err(error) = session.devices.borrow_mut().prewarm() {
        session.log_failure("capture", "devices_not_prewarmed", &error);
    }
    session.perf(|since_start| format!("start devices_prewarmed_ms={}", since_start.as_millis()));
    let devices_prewarmed = count("devices_prewarmed");
    let log_opened = log_opened.map(|counted| ("log_opened", counted));
    for (stage, counted) in [log_opened, tray_ready, prewarmed, devices_prewarmed]
        .into_iter()
        .flatten()
    {
        match counted {
            Ok(counted) => session.log_line("memory", |_| format!("at={stage} {counted}")),
            Err(error) => session.log_failure("memory", "counters_unread", &error),
        }
    }
    // The working set, and the CPU time spent with the tray idle, logged once.
    if let Some(start) = session.idle_start() {
        let idle = Rc::clone(&session);
        cx.spawn(async move |cx| {
            cx.background_executor().timer(IDLE_SAMPLE).await;
            idle.idle_use(start);
        })
        .detach();
    }
    if session.log.borrow().is_some() {
        let sampling = Rc::clone(&session);
        cleanshot_editor::on_graphics_released(cx, move |cx| {
            Rc::clone(&sampling).sample_graphics_released(cx);
        });
    }
    cx.on_app_quit(move |_| {
        match session.tray.borrow().remove_icon() {
            Ok(()) => session.log_line("tray", |_| "icon_removed".to_owned()),
            Err(error) => {
                session.log_line("tray", |_| format!("icon not removed at quit: {error}"));
            }
        }
        async {}
    })
    .detach();
}

/// Logs a request the tray passed on while the session was being dropped, as Cleanshot quits,
/// so it is never lost unseen. With no log open there is no one to tell.
fn log_dropped(log: &RefCell<Option<Log>>, request: Request) {
    let Some(log) = &*log.borrow() else { return };
    let written = log.write(&format!(
        "tray request {request:?} dropped: Cleanshot is quitting"
    ));
    if let Err(error) = written {
        eprintln!("tray request {request:?} dropped as Cleanshot quits, and not logged: {error}");
    }
}

/// The hotkey each capture request registers at start, and the warning when Snipping Tool
/// holds the region's.
fn tray_hotkeys(settings: &Settings) -> ([(Request, Hotkey); 5], Option<String>) {
    let (region, fallback) = region_hotkey(settings.hotkeys.region);
    let hotkeys = HotkeyAction::ALL.map(|action| {
        let hotkey = match action {
            HotkeyAction::Region => region,
            HotkeyAction::FullScreen
            | HotkeyAction::LastRegion
            | HotkeyAction::Window
            | HotkeyAction::CapturePanel => settings.hotkeys.get(action),
        };
        (Request::Capture(action), hotkey)
    });
    (hotkeys, fallback)
}

/// What start-up has to tell the user, in one line: `earlier`, then each hotkey another program
/// holds.
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
    /// The settings window changes them while Cleanshot runs.
    settings: RefCell<Settings>,
    /// Read once at start.
    policy: Policy,
    tray: RefCell<Tray>,
    overlay: RefCell<Option<Overlay>>,
    /// The capture panel, while it shows.
    panel: RefCell<Option<Panel>>,
    /// The wait before a timed capture, while it counts.
    countdown: RefCell<Option<Countdown>>,
    /// The panel's mode and timer, kept for the session and never written.
    capture_panel: Cell<CapturePanel>,
    /// The thumbnail of the last capture copied or saved, while it shows.
    thumbnail: RefCell<Option<thumbnail::Shown>>,
    /// How many thumbnails have shown, so an event from a replaced one does nothing.
    thumbnails_shown: Cell<u64>,
    /// The last selection copied or saved, kept in memory only.
    last_selection: Cell<Option<RECT>>,
    /// So an after-use memory sample knows whether a capture started after it was set.
    captures_started: Cell<u64>,
    /// The capture devices kept from one capture to the next.
    devices: RefCell<Devices>,
    /// Open only under `--log`. Shared with the tray's callback, which can outlive the
    /// session by a request as Cleanshot quits.
    log: Rc<RefCell<Option<Log>>>,
}

impl Session {
    fn new(settings: Settings, policy: Policy, tray: Tray, log: Rc<RefCell<Option<Log>>>) -> Self {
        Self {
            settings: RefCell::new(settings),
            policy,
            tray: RefCell::new(tray),
            overlay: RefCell::new(None),
            panel: RefCell::new(None),
            countdown: RefCell::new(None),
            capture_panel: Cell::new(CapturePanel::default()),
            thumbnail: RefCell::new(None),
            thumbnails_shown: Cell::new(0),
            last_selection: Cell::new(None),
            captures_started: Cell::new(0),
            devices: RefCell::new(Devices::default()),
            log,
        }
    }

    /// The settings a save uses: the user's, with the policy's folder and format where it sets
    /// them.
    fn saving_settings(&self) -> Settings {
        self.policy.applied_to(self.settings.borrow().clone())
    }

    /// Logs a `perf` line under `--log`, made from the time since the process started.
    fn perf(&self, line: impl FnOnce(Duration) -> String) {
        self.log_line("perf", line);
    }

    /// Under `--log`, how the desktop was copied, what DXGI said of each adapter and output, and
    /// how much of each monitor came out black.
    fn log_capture(
        &self,
        copied: &Copied,
        adapters: &[AdapterReport],
        outputs: &[OutputReport],
        monitors: &[MonitorImage],
    ) {
        if self.log.borrow().is_none() {
            return;
        }
        self.log_line("capture", |_| match copied {
            Copied::Duplication => String::from("copied=duplication"),
            Copied::Gdi { because } => format!("copied=gdi because={because:?}"),
        });
        for adapter in adapters {
            self.log_line("capture", |_| format!("adapter {adapter:?}"));
        }
        for output in outputs {
            self.log_line("capture", |_| format!("output {output:?}"));
        }
        // The outputs after the first should find their first frame already waiting.
        let acquires: Vec<_> = outputs
            .iter()
            .filter_map(|output| Some(output.frame.as_ref()?.acquire.as_micros()))
            .collect();
        if let [first, later @ ..] = acquires.as_slice()
            && let Some(later) = later.iter().max()
        {
            self.perf(|_| format!("overlay first_acquire_us={first} later_acquire_us={later}"));
        }
        for monitor in monitors {
            self.log_line("capture", |_| {
                format!(
                    "monitor bounds={:?} black_share={:.3}",
                    monitor.bounds,
                    black_share(&monitor.frame)
                )
            });
        }
    }

    /// Logs a `kind` line under `--log`, made from the time since the process started. A log
    /// that fails is closed, and the tray says so once.
    fn log_line(&self, kind: &str, line: impl FnOnce(Duration) -> String) {
        let written = match &*self.log.borrow() {
            Some(log) => {
                since_start().and_then(|since| log.write(&format!("{kind} {}", line(since))))
            }
            None => return,
        };
        if let Err(error) = written {
            drop(self.log.borrow_mut().take());
            self.warn(Warning::Failed("Logging stopped", &error));
        }
    }

    /// Logs a `kind` line under `--log` for `event`, a failure not returned to a caller, from
    /// its fixed words and `error`'s OS code only.
    fn log_failure(&self, kind: &str, event: &'static str, error: &(dyn Error + 'static)) {
        self.log_line(kind, |_| failure_line(event, error));
    }

    /// Shows `warning` from the tray icon and logs it under `--log`. The release exe has no
    /// console, so stderr is only the last resort, for when the tray cannot show it.
    fn warn(&self, warning: Warning) {
        let (text, logged) = match warning {
            Warning::Failed(what, error) => {
                (format!("{what}: {error}."), failure_line(what, error))
            }
            Warning::Refused(error) => (
                delivery_notice(&Err(error.clone())),
                format!("refused {}", refused_kind(error)),
            ),
            Warning::Said { kind, text } => (text.to_owned(), kind.to_owned()),
        };
        self.log_line("warn", |_| logged);
        if let Err(error) = self.tray.borrow().warn("Cleanshot", &text) {
            eprintln!("{text} ({error})");
        }
    }
}

/// What [`Session::warn`] shows from the tray. Its log line is made from fixed words, a
/// variant's name and OS codes, never from the text shown, which can hold a path and so the
/// user's name.
#[derive(Clone, Copy)]
enum Warning<'a> {
    /// `what` failed because of the error.
    Failed(&'static str, &'a (dyn Error + 'static)),
    /// A copy or save was refused; the tray shows [`delivery_notice`] for it.
    Refused(&'a DeliveryError),
    /// `text`, logged as `kind` alone.
    Said { kind: &'static str, text: &'a str },
}

fn failure_line(event: &str, error: &(dyn Error + 'static)) -> String {
    match os_code(error) {
        Some(code) => format!("{event} os {code:#x}"),
        None => event.to_owned(),
    }
}

fn window_surface(session: &Session) -> WindowSurface {
    cleanshot_win::high_contrast_colors()
        .and_then(|colors| match colors {
            Some(colors) => Ok(WindowSurface::HighContrast(colors)),
            None => cleanshot_win::glass_allowed().map(|glass| {
                if glass {
                    WindowSurface::Glass
                } else {
                    WindowSurface::Solid
                }
            }),
        })
        .unwrap_or_else(|error| {
            session.log_failure("editor", "surface not_read", &error);
            WindowSurface::Solid
        })
}

fn refused_kind(error: &DeliveryError) -> &'static str {
    match error {
        DeliveryError::TooLarge { .. } => "TooLarge",
        DeliveryError::ClipboardBusy => "ClipboardBusy",
        DeliveryError::NoSaveFolder => "NoSaveFolder",
        DeliveryError::SaveFolderNotFull { .. } => "SaveFolderNotFull",
        DeliveryError::NoFreeName { .. } => "NoFreeName",
        DeliveryError::NotWritten { .. } => "NotWritten",
        DeliveryError::TextNotDrawn => "TextNotDrawn",
        DeliveryError::ImageNotMade => "ImageNotMade",
        DeliveryError::NoSaveDialog => "NoSaveDialog",
        DeliveryError::SavingOff(SavingOff::ClipboardOnly) => "SavingOff ClipboardOnly",
        DeliveryError::SavingOff(SavingOff::FolderNotValid) => "SavingOff FolderNotValid",
        DeliveryError::OutsidePolicyFolder { .. } => "OutsidePolicyFolder",
    }
}

// Cleanshot never opens Settings for the user, so it says where the switch is.
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

/// `window`, which Cleanshot has just shown, comes to the front, as the user's hotkey or
/// click entitles it to; a refusal is logged under `kind`.
fn to_front(session: &Session, kind: &str, window: NonZeroIsize) {
    if let Err(error) = cleanshot_win::bring_to_front(window) {
        session.log_failure(kind, "foreground_refused", &error);
    }
}

/// Saves the overlay's `image` to the path chosen in Save As. A cancel and a save show
/// nothing, as the overlay's Save does; a failure is shown from the tray.
async fn save_as(session: &Rc<Session>, image: RgbaImage, cx: &mut AsyncApp) {
    let start = save_as_start(&session.saving_settings(), session.policy.saving());
    let (folder, name) = match start {
        Ok(start) => start,
        Err(error) => return session.warn(Warning::Refused(&error)),
    };
    let logging = Rc::clone(session);
    let log = move |event: &'static str| logging.log_line("capture", |_| event.to_owned());
    let chosen = cx.update(|cx| ask_save_path(cx, &folder, &name, log));
    save_chosen(session, image, chosen.await);
}

fn save_chosen(
    session: &Session,
    image: RgbaImage,
    chosen: Option<Result<PathBuf, DeliveryError>>,
) {
    let Some(chosen) = chosen else {
        return;
    };
    let document = Document::new(Arc::new(image));
    if let Err(error) = chosen.and_then(|path| {
        save_to(
            &path,
            &document,
            &session.saving_settings(),
            session.policy.saving(),
            &|event, error| session.log_failure("delivery", event, error),
        )
    }) {
        session.warn(Warning::Refused(&error));
    }
}

fn open_log() -> Result<Log, String> {
    let settings = settings_location().map_err(|error| error.to_string())?.path;
    Log::open(&settings).map_err(|error| error.to_string())
}

// Reading never stops Cleanshot; what is wrong becomes one warning, and defaults stand in.
fn load_settings() -> (Settings, Option<String>) {
    let location = match settings_location() {
        Ok(location) => location,
        Err(error) => {
            let warning =
                format!("The settings could not be read, so the defaults apply: {error}.");
            return (Settings::default(), Some(warning));
        }
    };
    let (settings, problem) = settings_at(&location.path);
    let ignored = location.ignored.as_ref().map(describe_ignored);
    let warning = match (ignored, problem) {
        (Some(ignored), Some(problem)) => Some(format!("{ignored} {problem}")),
        (ignored, problem) => ignored.or(problem),
    };
    (settings, warning)
}

// Says which file was passed over and what brings it back.
fn describe_ignored(ignored: &PortableIgnored) -> String {
    let unblock =
        "If you trust it, choose Unblock in the file's Properties and start Cleanshot again.";
    let reason = match ignored {
        PortableIgnored::FromInternet => "it came from the internet".to_owned(),
        PortableIgnored::MarkNotRead(error) => {
            format!("Cleanshot could not tell where it came from ({error})")
        }
        PortableIgnored::MarkNotUnderstood => {
            "Cleanshot could not tell where it came from".to_owned()
        }
    };
    format!("The cleanshot.toml beside Cleanshot was not used, because {reason}. {unblock}")
}

fn settings_at(path: &Path) -> (Settings, Option<String>) {
    let bytes = match read_settings(path, MAX_SETTINGS_BYTES) {
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

    #[test]
    fn start_registers_the_window_hotkey_alt_prtsc_by_default() {
        let (hotkeys, _) = tray_hotkeys(&Settings::default());
        let window = Request::Capture(HotkeyAction::Window);
        let registered = hotkeys.iter().find(|&&(request, _)| request == window);
        assert_eq!(
            registered.map(|&(_, hotkey)| hotkey.to_string()),
            Some("Alt+PrtSc".to_owned())
        );
    }

    #[test]
    fn start_registers_the_capture_panel_hotkey_ctrl_shift_5_by_default() {
        let (hotkeys, _) = tray_hotkeys(&Settings::default());
        let panel = Request::Capture(HotkeyAction::CapturePanel);
        let registered = hotkeys.iter().find(|&&(request, _)| request == panel);
        assert_eq!(
            registered.map(|&(_, hotkey)| hotkey.to_string()),
            Some("Ctrl+Shift+5".to_owned())
        );
    }

    #[test]
    fn a_warn_is_logged_with_its_os_code_and_no_part_of_a_folder_it_names() {
        let folder = std::env::temp_dir().join(format!("cleanshot-warn-{}", std::process::id()));
        let log = Log::open(&folder.join("settings.toml")).expect("the log opens");
        let tray = Tray::create(&[], |_| {}).expect("the tray");
        let session = Session::new(
            Settings::default(),
            Policy::default(),
            tray,
            Rc::new(RefCell::new(Some(log))),
        );
        let named = PathBuf::from(r"C:\Users\Someone Named\Pictures\Screenshots");
        session.warn(Warning::Refused(&DeliveryError::NotWritten {
            folder: named,
        }));
        let denied = std::io::Error::from_raw_os_error(5);
        session.warn(Warning::Failed(
            "The pointer could not be drawn into the capture",
            &denied,
        ));
        drop(session);
        let written = std::fs::read_to_string(folder.join("cleanshot.log")).expect("the log reads");
        std::fs::remove_dir_all(&folder).expect("the test folder is removed");
        let warns: Vec<_> = written
            .lines()
            .filter_map(|line| line.split_once(' ').map(|(_, event)| event))
            .collect();
        assert_eq!(
            warns,
            [
                "warn refused NotWritten",
                "warn The pointer could not be drawn into the capture os 0x5",
            ]
        );
    }
}
