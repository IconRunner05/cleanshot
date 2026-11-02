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
    Delivered, DeliveryError, Document, Hotkey, HotkeyAction, MAX_SETTINGS_BYTES, Output, Policy,
    RgbaImage, Settings, SettingsProblem, delivery_notice,
};
use cleanshot_editor::ask_save_path;
use cleanshot_win::capture::{Copied, MonitorImage, OutputReport, RECT, black_share};
use cleanshot_win::instance::Launch;
use cleanshot_win::log::{Log, Usage, since_start, usage};
use cleanshot_win::overlay::Overlay;
use cleanshot_win::policy::read_policy;
use cleanshot_win::settings_file::{read_settings, settings_path};
use cleanshot_win::tray::{
    Request, Tray, ask_running_instance_to_capture, message_box, print_screen_fallback,
    snipping_tool_holds_print_screen,
};

mod capture;
mod delivery;
mod settings_window;

use capture::{Handoff, capture};
use delivery::{deliver, save_as_start, save_to};
use settings_window::open_settings;

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
    let (policy, policy_problems) = read_policy();
    let (hotkeys, fallback) = tray_hotkeys(&settings);
    let tray = Rc::new(RefCell::new(None));
    let launched_tray = Rc::clone(&tray);
    let session = Rc::new(Session::new(settings, policy, Rc::clone(&tray), log));
    for problem in policy_problems {
        session.log_line("policy", |_| format!("problem {problem}"));
    }
    let launched = Rc::clone(&session);
    cleanshot_editor::run(move |cx| {
        // D-76: prewarm on the overlay thread before the tray is ready.
        let unwarmed = Overlay::prewarm()
            .err()
            .map(|error| format!("The capture overlay could not be prepared: {error}."));
        let app = cx.to_async();
        let created = Tray::create(&hotkeys, move |request| {
            // P-01 counts from here, inside the tray's handling of the hotkey message.
            let asked = Instant::now();
            let session = Rc::clone(&session);
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
                        Handoff::Editor => cx.update(|cx| open_editor(cx, image, &session)),
                        Handoff::SaveAs => {
                            let (folder, name) = save_as_start(&session.saving_settings());
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
                    let action = match request {
                        Request::Capture(action) => action,
                        Request::OpenSettings => return open_settings(cx, &session),
                        Request::Exit => return cx.quit(),
                    };
                    capture(&session, action, asked, defer, pass_on);
                });
            })
            .detach();
        })
        .expect("the tray window and icon are created");
        if launched.policy.settings_hidden() {
            created.hide_settings_item();
        }
        let text = start_warnings(&created, [warning, log_warning, fallback, unwarmed]);
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

/// The hotkey each capture request registers at start, and the warning when Snipping Tool
/// holds the region's (F-52).
fn tray_hotkeys(settings: &Settings) -> ([(Request, Hotkey); 3], Option<String>) {
    let (region, fallback) = region_hotkey(settings.hotkeys.region);
    let hotkeys = [
        (Request::Capture(HotkeyAction::Region), region),
        (
            Request::Capture(HotkeyAction::FullScreen),
            settings.hotkeys.full_screen,
        ),
        (
            Request::Capture(HotkeyAction::LastRegion),
            settings.hotkeys.last_region,
        ),
    ];
    (hotkeys, fallback)
}

/// What start-up has to tell the user, in one line: `earlier`, then each hotkey another program
/// holds (F-51: shown, not swallowed).
fn start_warnings(tray: &Tray, earlier: [Option<String>; 4]) -> String {
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
    /// F-55: the settings window changes them while Cleanshot runs.
    settings: RefCell<Settings>,
    /// F-57: read once at start.
    policy: Policy,
    tray: Rc<RefCell<Option<Tray>>>,
    overlay: RefCell<Option<Overlay>>,
    /// F-06: the last selection copied or saved, kept in memory only (C-20).
    last_selection: Cell<Option<RECT>>,
    /// So a P-04 after-use sample knows whether a capture started after it was set.
    captures_started: Cell<u64>,
    /// C-23: open only under `--log`.
    log: RefCell<Option<Log>>,
}

impl Session {
    fn new(
        settings: Settings,
        policy: Policy,
        tray: Rc<RefCell<Option<Tray>>>,
        log: Option<Log>,
    ) -> Self {
        Self {
            settings: RefCell::new(settings),
            policy,
            tray,
            overlay: RefCell::new(None),
            last_selection: Cell::new(None),
            captures_started: Cell::new(0),
            log: RefCell::new(log),
        }
    }

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

    /// Under --log, logs the P-04 private working set 10 s from now if no capture starts
    /// before then; normal running starts no timer (P-05).
    fn sample_after_use(self: &Rc<Self>, defer: &dyn Fn(Duration, Box<dyn FnOnce()>)) {
        if self.log.borrow().is_none() {
            return;
        }
        let started = self.captures_started.get();
        let session = Rc::clone(self);
        defer(
            AFTER_USE_SAMPLE,
            Box::new(move || {
                if session.captures_started.get() != started {
                    return;
                }
                match usage() {
                    Ok(held) => session.perf(|_| {
                        format!(
                            "P-04 private_working_set_bytes={}",
                            held.private_working_set_bytes
                        )
                    }),
                    Err(error) => {
                        session.warn(&format!(
                            "Logging could not sample memory after use: {error}."
                        ));
                    }
                }
            }),
        );
    }

    /// The settings a save uses: the user's, with the policy's folder and format where it sets
    /// them (F-57).
    fn saving_settings(&self) -> Settings {
        self.policy.applied_to(self.settings.borrow().clone())
    }

    /// Logs a `perf` line under `--log`, made from the time since the process started.
    fn perf(&self, line: impl FnOnce(Duration) -> String) {
        self.log_line("perf", line);
    }

    /// Under `--log`, how the desktop was copied, what DXGI said of each output, and how much
    /// of each monitor came out black.
    fn log_capture(&self, copied: &Copied, outputs: &[OutputReport], monitors: &[MonitorImage]) {
        if self.log.borrow().is_none() {
            return;
        }
        self.log_line("capture", |_| match copied {
            Copied::Duplication => String::from("copied=duplication"),
            Copied::Gdi { because } => format!("copied=gdi because={because:?}"),
        });
        for output in outputs {
            self.log_line("capture", |_| format!("output {output:?}"));
        }
        for monitor in monitors {
            self.log_line("capture", |_| {
                format!(
                    "monitor bounds={:?} black_share={:.3}",
                    monitor.bounds,
                    black_share(&monitor.image)
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
    move |output, document| deliver(output, document, &session.saving_settings())
}

fn editor_save_as_start(session: &Rc<Session>) -> impl Fn() -> (PathBuf, String) + 'static {
    let session = Rc::clone(session);
    move || save_as_start(&session.saving_settings())
}

fn editor_save_to(
    session: &Rc<Session>,
) -> impl Fn(&Path, &Document) -> Result<Delivered, DeliveryError> + 'static {
    let session = Rc::clone(session);
    move |path, document| save_to(path, document, &session.saving_settings())
}

/// P-07: logs the time from now, as the editor opens, to the frame after its first draw.
fn editor_first_frame(session: &Rc<Session>) -> impl FnOnce(Instant) + 'static {
    let session = Rc::clone(session);
    let opened = Instant::now();
    move |shown| {
        session.perf(|_| format!("P-07 first_frame_ms={}", (shown - opened).as_millis()));
    }
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
    if let Err(error) =
        chosen.and_then(|path| save_to(&path, &document, &session.saving_settings()))
    {
        session.warn(&delivery_notice(&Err(error)));
    }
}

const IDLE_SAMPLE: Duration = Duration::from_secs(10);
const AFTER_USE_SAMPLE: Duration = Duration::from_secs(10);

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
