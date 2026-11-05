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
    CaptureMode, CapturePanel, Delivered, DeliveryError, Document, Hotkey, HotkeyAction,
    MAX_SETTINGS_BYTES, Output, Policy, PolicyProblem, RgbaImage, SavingOff, Settings,
    SettingsProblem, delivery_notice,
};
use cleanshot_editor::{AsyncApp, ask_save_path};
use cleanshot_win::capture::{
    AdapterReport, Copied, Devices, MonitorImage, OutputReport, RECT, black_share, monitor_of,
};
use cleanshot_win::instance::Launch;
use cleanshot_win::log::memory::{self, Counted};
use cleanshot_win::log::{Log, LogError, Usage, os_code, since_start, usage};
use cleanshot_win::overlay::Overlay;
use cleanshot_win::panel::{Countdown, Panel};
use cleanshot_win::policy::read_policy;
use cleanshot_win::settings_file::{read_settings, settings_path};
use cleanshot_win::tray::{
    Request, Tray, ask_running_instance_to_capture, message_box, print_screen_fallback,
    snipping_tool_holds_print_screen,
};

mod capture;
mod delivery;
mod panel;
mod settings_window;
mod thumbnail;

use capture::{Handoff, Shot, capture};
use delivery::{deliver, save_as_start, save_to};
use settings_window::open_settings;

fn main() {
    let entered = since_start();
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
    cleanshot_win::crash::install(log.clone());
    std::sync::LazyLock::force(&delivery::FILE_NAME_TEMPLATE);
    let (settings, warning) = load_settings();
    let (policy, policy_problems) = read_policy();
    let (hotkeys, fallback) = tray_hotkeys(&settings);
    // P-04: counters only, as a full account would slow P-03.
    let log_opened = log.is_some().then(memory::counted);
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

fn on_tray_request(app: &AsyncApp, session: Rc<Session>, request: Request) {
    // P-01 counts from here, inside the tray's handling of the hotkey message.
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
/// `HideSettings` (F-57), tells the user what went wrong, starts the P-04 and P-05 sample, and
/// hands the session to the app, which holds it until it quits and then removes the icon
/// (F-50).
fn started(
    cx: &mut cleanshot_editor::App,
    session: Rc<Session>,
    policy_problems: Vec<PolicyProblem>,
    earlier: [Option<String>; 3],
    marks: [(&str, Result<Duration, LogError>); 2],
    log_opened: Option<Result<Counted, LogError>>,
) {
    // D-117: read once; unread, tooltips keep GPUI's delay.
    match cleanshot_win::hover_time() {
        Ok(hover) => cleanshot_editor::follow_hover_time(cx, hover),
        Err(error) => session.log_failure("editor", "hover_time not_read", &error),
    }
    for problem in policy_problems {
        session.log_line("policy", |_| format!("problem {problem}"));
    }
    if session.policy.settings_hidden() {
        session.tray.borrow().hide_settings_item();
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
            Ok(at) => session.perf(|_| format!("P-03 {stage}_ms={}", at.as_millis())),
            Err(error) => session.log_failure("perf", "clock_unread", &error),
        }
    }
    session.perf(|since_start| format!("P-03 tray_ready_ms={}", since_start.as_millis()));
    let count = |stage| {
        let logging = session.log.borrow().is_some();
        logging.then(|| (stage, memory::counted()))
    };
    let tray_ready = count("tray_ready");
    // P-04: what the process holds before any graphics, to set against the after-use account.
    session.log_memory("before_graphics");
    // D-137: prewarm on the overlay thread once the tray is ready (D-76).
    let unwarmed = Overlay::prewarm().err();
    session.perf(|since_start| format!("P-03 overlay_prewarmed_ms={}", since_start.as_millis()));
    let prewarmed = count("overlay_prewarmed");
    if let Some(error) = unwarmed {
        session.warn(Warning::Failed(
            "The capture overlay could not be prepared",
            &error,
        ));
    }
    let log_opened = log_opened.map(|counted| ("log_opened", counted));
    for (stage, counted) in [log_opened, tray_ready, prewarmed].into_iter().flatten() {
        match counted {
            Ok(counted) => session.log_line("memory", |_| format!("P-04 at={stage} {counted}")),
            Err(error) => session.log_failure("memory", "counters_unread", &error),
        }
    }
    // P-04, P-05: the working set, and the CPU time spent with the tray idle, logged once.
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
/// holds the region's (F-52).
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
    /// F-55: the settings window changes them while Cleanshot runs.
    settings: RefCell<Settings>,
    /// F-57: read once at start.
    policy: Policy,
    tray: RefCell<Tray>,
    overlay: RefCell<Option<Overlay>>,
    /// F-11: the capture panel, while it shows.
    panel: RefCell<Option<Panel>>,
    /// F-10: the wait before a timed capture, while it counts.
    countdown: RefCell<Option<Countdown>>,
    /// D-99: the panel's mode and timer, kept for the session and never written.
    capture_panel: Cell<CapturePanel>,
    /// F-09: the thumbnail of the last capture copied or saved, while it shows.
    thumbnail: RefCell<Option<thumbnail::Shown>>,
    /// How many thumbnails have shown, so an event from a replaced one does nothing.
    thumbnails_shown: Cell<u64>,
    /// F-06: the last selection copied or saved, kept in memory only (C-20).
    last_selection: Cell<Option<RECT>>,
    /// So a P-04 after-use sample knows whether a capture started after it was set.
    captures_started: Cell<u64>,
    /// P-01: the capture devices kept from one capture to the next.
    devices: RefCell<Devices>,
    /// C-23: open only under `--log`. Shared with the tray's callback, which can outlive the
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

    /// Under --log, the usage to measure P-05 from; None without it, so normal running starts
    /// no timer (P-05).
    fn idle_start(&self) -> Option<Usage> {
        self.log.borrow().as_ref()?;
        usage()
            .inspect_err(|error| {
                self.warn(Warning::Failed("Logging could not sample idle use", error));
            })
            .ok()
    }

    /// Logs the P-04 working set now and the P-05 CPU time spent since `start`.
    fn idle_use(&self, start: Usage) {
        let end = match usage() {
            Ok(end) => end,
            Err(error) => {
                return self.warn(Warning::Failed("Logging could not sample idle use", &error));
            }
        };
        // Logged before the sample's lines, which the field test waits for.
        self.log_memory("idle");
        self.perf(|_| {
            format!(
                "P-04 working_set_bytes={} idle_private_bytes={}",
                end.working_set_bytes, end.private_working_set_bytes
            )
        });
        self.perf(|_| {
            format!(
                "P-05 cpu_ms={} over_ms={}",
                end.cpu.saturating_sub(start.cpu).as_millis(),
                IDLE_SAMPLE.as_millis()
            )
        });
    }

    /// After a capture finishes: if no capture starts in the next [`GRAPHICS_IDLE`], lets go
    /// of the Direct2D factory the capture windows shared, which frees its device once the
    /// thumbnail closes, and of the kept capture devices (P-04); under --log samples memory
    /// after use.
    fn after_use(self: &Rc<Self>, defer: &dyn Fn(Duration, Box<dyn FnOnce()>)) {
        let started = self.captures_started.get();
        let session = Rc::clone(self);
        defer(
            GRAPHICS_IDLE,
            Box::new(move || {
                if session.captures_started.get() != started {
                    return;
                }
                let before = session.private_bytes();
                cleanshot_win::release_factory();
                let factory_released = session.private_bytes();
                session.devices.borrow_mut().release();
                if let (Some(before), Some(factory_released), Some(after)) =
                    (before, factory_released, session.private_bytes())
                {
                    session.perf(|_| {
                        format!(
                            "P-04 graphics_released private_before={before} \
                             private_factory_released={factory_released} private_after={after}"
                        )
                    });
                }
            }),
        );
        self.sample_after_use(defer);
    }

    /// Under --log, the private working set now; None without it, or if it cannot be read.
    fn private_bytes(&self) -> Option<usize> {
        self.log.borrow().as_ref()?;
        usage()
            .inspect_err(|error| {
                self.warn(Warning::Failed(
                    "Logging could not sample memory after use",
                    error,
                ));
            })
            .ok()
            .map(|held| held.private_working_set_bytes)
    }

    /// Under --log, logs what holds the working set now, by kind and size, as of `stage`
    /// (P-04).
    fn log_memory(&self, stage: &str) {
        if self.log.borrow().is_none() {
            return;
        }
        match memory::held() {
            Ok(held) => self.log_line("memory", |_| format!("P-04 at={stage} {held}")),
            Err(error) => self.warn(Warning::Failed(
                "Logging could not account for memory",
                &error,
            )),
        }
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
                let held = session.private_bytes();
                // Logged before the sample's line, which the field test waits for.
                session.log_memory("after_use");
                if let Some(held) = held {
                    session.perf(|_| format!("P-04 private_working_set_bytes={held}"));
                }
            }),
        );
    }

    /// D-140: accounts for memory as GPUI gives its graphics device back, and again
    /// [`RENDERER_FREED`] later, once the closed window's renderer has been freed.
    fn sample_graphics_released(self: Rc<Self>, cx: &mut cleanshot_editor::App) {
        self.log_memory("graphics_released");
        cx.spawn(async move |cx| {
            cx.background_executor().timer(RENDERER_FREED).await;
            self.log_memory("graphics_released_6s");
        })
        .detach();
    }

    /// Under --log, logs the private working set past [`GRAPHICS_IDLE`] and
    /// [`AFTER_USE_SAMPLE`] after an editor closed, if no capture starts before then; reported,
    /// not judged (P-04).
    fn sample_after_editor(self: Rc<Self>, cx: &mut cleanshot_editor::App) {
        let started = self.captures_started.get();
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(GRAPHICS_IDLE + AFTER_USE_SAMPLE)
                .await;
            if self.captures_started.get() != started {
                return;
            }
            let held = self.private_bytes();
            self.log_memory("after_editor");
            if let Some(held) = held {
                self.perf(|_| format!("P-04 after_editor_private_bytes={held}"));
            }
        })
        .detach();
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
        // D-139: the outputs after the first should find their first frame already waiting.
        let acquires: Vec<_> = outputs
            .iter()
            .filter_map(|output| Some(output.frame.as_ref()?.acquire.as_micros()))
            .collect();
        if let [first, later @ ..] = acquires.as_slice()
            && let Some(later) = later.iter().max()
        {
            self.perf(|_| format!("P-01 first_acquire_us={first} later_acquire_us={later}"));
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
    /// its fixed words and `error`'s OS code only (C-23).
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
/// user's name (C-23).
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

/// F-05: opens `image`, captured from `from`, in the editor on that monitor (D-110), or says in
/// the tray why it could not. A monitor Windows cannot describe leaves the primary display.
fn open_editor(
    cx: &mut cleanshot_editor::App,
    image: RgbaImage,
    from: RECT,
    session: &Rc<Session>,
) {
    let monitor = monitor_of(from)
        .inspect_err(|error| session.log_failure("editor", "monitor_unread", error))
        .ok();
    let opened = cleanshot_editor::open(
        cx,
        image,
        monitor,
        cleanshot_editor::Outputs::new(
            editor_deliver(session),
            editor_save_as_start(session),
            editor_save_to(session),
        ),
        {
            let logging = Rc::clone(session);
            move |document: &Document| {
                cleanshot_win::text::masks(document)
                    .inspect_err(|error| logging.log_failure("editor", "text_masks_failed", error))
                    .ok()
            }
        },
        editor_first_frame(session),
        {
            let logging = Rc::clone(session);
            move |event| logging.log_line("editor", |_| event.to_owned())
        },
    );
    match opened {
        Ok(editor) => {
            if session.log.borrow().is_some() {
                let sampling = Rc::clone(session);
                editor.on_closed(cx, move |cx| sampling.sample_after_editor(cx));
            }
            to_front(session, "editor", editor.win32_handle());
        }
        Err(error) => session.warn(Warning::Failed("The editor could not open", &error)),
    }
}

/// D-124: `window`, which Cleanshot has just shown, comes to the front, as the user's hotkey or
/// click entitles it to; a refusal is logged under `kind`.
fn to_front(session: &Session, kind: &str, window: NonZeroIsize) {
    if let Err(error) = cleanshot_win::bring_to_front(window) {
        session.log_failure(kind, "foreground_refused", &error);
    }
}

/// F-05: the editor's Copy and Save deliver as the overlay does (D-48).
fn editor_deliver(
    session: &Rc<Session>,
) -> impl Fn(Output, &Document) -> Result<Delivered, DeliveryError> + 'static {
    let session = Rc::clone(session);
    move |output, document| {
        deliver(
            output,
            document,
            &session.saving_settings(),
            session.policy.saving(),
            &|event, error| session.log_failure("delivery", event, error),
        )
    }
}

fn editor_save_as_start(
    session: &Rc<Session>,
) -> impl Fn() -> Result<(PathBuf, String), DeliveryError> + 'static {
    let session = Rc::clone(session);
    move || save_as_start(&session.saving_settings(), session.policy.saving())
}

fn editor_save_to(
    session: &Rc<Session>,
) -> impl Fn(&Path, &Document) -> Result<Delivered, DeliveryError> + 'static {
    let session = Rc::clone(session);
    move |path, document| {
        save_to(
            path,
            document,
            &session.saving_settings(),
            session.policy.saving(),
            &|event, error| session.log_failure("delivery", event, error),
        )
    }
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

const IDLE_SAMPLE: Duration = Duration::from_secs(10);
const AFTER_USE_SAMPLE: Duration = Duration::from_secs(10);
/// P-01 against P-04: captures this close together share one Direct2D device; after a
/// longer pause it is let go, before [`AFTER_USE_SAMPLE`] measures what stays.
const GRAPHICS_IDLE: Duration = Duration::from_secs(5);
/// How long after GPUI gives its device back the closed window's renderer is gone too (D-140).
const RENDERER_FREED: Duration = Duration::from_secs(6);

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
    use super::*;

    #[test]
    fn f08_start_registers_the_window_hotkey_alt_prtsc_by_default() {
        let (hotkeys, _) = tray_hotkeys(&Settings::default());
        let window = Request::Capture(HotkeyAction::Window);
        let registered = hotkeys.iter().find(|&&(request, _)| request == window);
        assert_eq!(
            registered.map(|&(_, hotkey)| hotkey.to_string()),
            Some("Alt+PrtSc".to_owned())
        );
    }

    #[test]
    fn f11_start_registers_the_capture_panel_hotkey_ctrl_shift_5_by_default() {
        let (hotkeys, _) = tray_hotkeys(&Settings::default());
        let panel = Request::Capture(HotkeyAction::CapturePanel);
        let registered = hotkeys.iter().find(|&&(request, _)| request == panel);
        assert_eq!(
            registered.map(|&(_, hotkey)| hotkey.to_string()),
            Some("Ctrl+Shift+5".to_owned())
        );
    }

    #[test]
    fn c23_a_warn_is_logged_with_its_os_code_and_no_part_of_a_folder_it_names() {
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
