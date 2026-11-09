//! The settings window's check boxes reach UI Automation, and pressed as
//! Narrator's activate command would press them, Show a thumbnail is written to the settings
//! file and Start with Windows sets and clears the HKCU Run value. A format picked
//! from the Options menu while the window is open is kept when the window next writes
//! the file. Closing the window, the last one open, leaves GPUI running for the tray.
//! A second open finds the window already open, and Windows brings its handle to the front.
//! The window has no title bar: the top of it is its own caption row, which Windows
//! treats as a title bar. A window capture offers the window to pick, as GPUI made it.
//! Run as for `toolbar_automation`: GPUI owns the window's thread and the walk ends the
//! process.

use std::num::NonZeroIsize;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use cleanshot_core::{
    Applied, MAX_SETTINGS_BYTES, Policy, PolicyValue, SaveFormat, Settings, run_command, starts,
};
use cleanshot_editor::{App, AsyncApp};
use cleanshot_win::accessibility::{
    ControlKind, HitArea, hit_test, invoke_in_own_windows, offered_to_window_pick,
    toggle_in_own_windows, wait_for_container,
};
use cleanshot_win::bring_to_front;
use cleanshot_win::settings_file::{read_settings, write_settings};
use cleanshot_win::startup::{remove_run_value, run_value, set_run_value};

const TEST: &str = "the_settings_check_boxes_keep_a_menu_pick_and_write_the_file_and_the_run_value";
const THUMBNAIL: &str = "Show a thumbnail";
const START_WITH_WINDOWS: &str = "Start with Windows";
// A debug build on a cold runner can take seconds to apply a press; no budget is judged here.
const PATIENCE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);
// GPUI quitting on a closed window returns from `run` well inside this.
const STILL_RUNNING: Duration = Duration::from_secs(1);
// 10 physical pixels down and 250 across is the caption row's empty space at any scale
// from 100% to 250%, right of the "Settings" title and left of the caption buttons.
const CAPTION_SPACE: (i32, i32) = (250, 10);

fn main() {
    let exe = std::env::current_exe()
        .expect("the test knows its own path")
        .into_os_string()
        .into_string()
        .expect("the test's path is Unicode");
    let file = PathBuf::from(&exe).with_file_name("settings_automation.toml");
    if let Err(problem) = remove_left_over(&file) {
        println!("{TEST} ... FAILED: {problem}");
        exit(1);
    }
    let left_as_found = LeftAsFound {
        file: file.clone(),
        run_value: run_value().expect("the HKCU Run key reads"),
    };
    let start = Instant::now();
    // The settings the app holds, as apply_settings keeps them and the Options menu changes them.
    let in_force = Arc::new(Mutex::new(Settings::default()));
    let (opened, shown) = mpsc::channel();
    let (handed, handle) = mpsc::channel();
    let walked = (file.clone(), exe.clone(), Arc::clone(&in_force));
    let (verdict, walk_verdict) = mpsc::channel();
    thread::spawn(move || {
        let (file, exe, in_force) = walked;
        verdict
            .send(walk(&file, &exe, &in_force, start, &shown, &handle))
            .expect("GPUI runs until the walk's verdict, or main finishes the run");
    });
    let stopped = left_as_found.clone();
    let ran = cleanshot_editor::run(move |cx| {
        let (hidden, _) = Policy::from_values(
            PolicyValue::Missing,
            PolicyValue::Missing,
            PolicyValue::Missing,
            PolicyValue::Number(1),
        );
        let applied_while_hidden = left_as_found.clone();
        // Under HideSettings the window does not open; the tray's half is tested in win's tray module.
        let refused = cleanshot_editor::open_settings(
            cx,
            Settings::default,
            hidden,
            move |_| -> Applied {
                applied_while_hidden.finish(Err("a hidden settings window applied a change".into()))
            },
            false,
            |_| Ok(()),
            |_| {},
        );
        if !matches!(refused, Err(cleanshot_editor::Error::SettingsHidden)) {
            left_as_found.finish(Err(format!("HideSettings opened the window: {refused:?}")));
        }
        let opened_now = cleanshot_editor::open_settings(
            cx,
            {
                let reading = Arc::clone(&in_force);
                move || {
                    reading
                        .lock()
                        .expect("no thread panics holding the settings")
                        .clone()
                }
            },
            Policy::default(),
            move |changed: Settings| {
                let saved = write_settings(&file, &changed, &|_, _| {});
                *in_force
                    .lock()
                    .expect("no thread panics holding the settings") = changed;
                Applied {
                    saved,
                    taken: Vec::new(),
                }
            },
            false,
            move |on| {
                if on {
                    set_run_value(&run_command(&exe))
                } else {
                    remove_run_value()
                }
            },
            |_| {},
        );
        let window = match opened_now {
            Ok(window) => window,
            Err(error) => {
                left_as_found.finish(Err(format!("the settings window did not open: {error}")))
            }
        };
        second_open_finds(cx, window, &left_as_found);
        handed
            .send(window)
            .expect("the walk thread waits for the window");
        opened
            .send(Instant::now())
            .expect("the walk thread waits for the window");
        cx.spawn(async move |cx| close_after_walk(cx, &walk_verdict, &left_as_found).await)
            .detach();
    });
    stopped.finish(Err(format!(
        "GPUI stopped before the walk finished: {ran:?}"
    )));
}

/// A second Settings finds the open `window`, whose handle Windows brings to the front.
fn second_open_finds(cx: &mut App, window: NonZeroIsize, left_as_found: &LeftAsFound) {
    let stopped = left_as_found.clone();
    let second = cleanshot_editor::open_settings(
        cx,
        Settings::default,
        Policy::default(),
        move |_| -> Applied { stopped.finish(Err("the second open applied a change".into())) },
        false,
        |_| Ok(()),
        |_| {},
    );
    if second.as_ref().ok() != Some(&window) {
        left_as_found.finish(Err(format!(
            "a second open gave {second:?}, not the open window {window}"
        )));
    }
    if let Err(error) = bring_to_front(window) {
        left_as_found.finish(Err(format!("the settings window stayed behind: {error}")));
    }
}

/// The settings file and HKCU Run value as the run found them, put back on every exit.
#[derive(Clone)]
struct LeftAsFound {
    file: PathBuf,
    run_value: Option<String>,
}

impl LeftAsFound {
    fn finish(&self, verdict: Result<(), String>) -> ! {
        let removed = remove_left_over(&self.file);
        let restored = match &self.run_value {
            Some(command) => set_run_value(command),
            None => remove_run_value(),
        };
        match (verdict, removed, restored) {
            (Ok(()), Ok(()), Ok(())) => {
                println!("{TEST} ... ok");
                exit(0);
            }
            (verdict, removed, restored) => {
                println!(
                    "{TEST} ... FAILED: {verdict:?}, file removed: {removed:?}, \
                     Run value restored: {restored:?}"
                );
                exit(1);
            }
        }
    }
}

/// Once the walk passes, closes Settings, the last window open, and finishes only if
/// GPUI is still running for the tray a second later.
async fn close_after_walk(
    cx: &mut AsyncApp,
    walk_verdict: &mpsc::Receiver<Result<(), String>>,
    left_as_found: &LeftAsFound,
) -> ! {
    let walked = loop {
        match walk_verdict.try_recv() {
            Ok(walked) => break walked,
            Err(mpsc::TryRecvError::Empty) => cx.background_executor().timer(POLL).await,
            Err(mpsc::TryRecvError::Disconnected) => {
                break Err("the walk thread ended with no verdict".into());
            }
        }
    };
    if walked.is_err() {
        left_as_found.finish(walked);
    }
    let closed = cx.update(|cx| {
        cx.windows()
            .into_iter()
            .try_for_each(|window| window.update(cx, |_, window, _| window.remove_window()))
    });
    if let Err(error) = closed {
        left_as_found.finish(Err(format!("the settings window did not close: {error}")));
    }
    cx.background_executor().timer(STILL_RUNNING).await;
    left_as_found.finish(Ok(()))
}

// A file from an earlier run would pass the thumbnail check before any press.
fn remove_left_over(file: &Path) -> Result<(), String> {
    match std::fs::remove_file(file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{} could not be removed: {error}", file.display())),
    }
}

fn walk(
    file: &Path,
    exe: &str,
    in_force: &Mutex<Settings>,
    start: Instant,
    shown: &mpsc::Receiver<Instant>,
    handle: &mpsc::Receiver<NonZeroIsize>,
) -> Result<(), String> {
    let window = handle
        .recv_timeout(PATIENCE)
        .map_err(|error| format!("no window handle came: {error}"))?;
    let boxes = wait_for_container(
        ControlKind::Group,
        "Capture",
        ControlKind::CheckBox,
        start,
        shown,
    )
    .map_err(|missing| format!("{missing:?}"))?;
    let names: Vec<_> = boxes.iter().map(|control| control.name.as_str()).collect();
    if names != [THUMBNAIL, START_WITH_WINDOWS] {
        return Err(format!("the Capture check boxes are {names:?}"));
    }
    let caption = hit_test(window, CAPTION_SPACE).map_err(|error| error.to_string())?;
    if caption != HitArea::Caption {
        return Err(format!(
            "the top of the window is {caption:?}, not its own caption row"
        ));
    }
    if !offered_to_window_pick(window) {
        return Err("a window capture does not offer the settings window".to_owned());
    }

    // The Options menu picks JPG while the window is open, as its apply_settings would.
    in_force
        .lock()
        .map_err(|_| "a thread panicked holding the settings".to_owned())?
        .save_format = SaveFormat::Jpg;
    press(THUMBNAIL)?;
    wait_until(
        "the file to turn the thumbnail off and keep the menu's JPG",
        || {
            read_settings(file, MAX_SETTINGS_BYTES).map(|text| {
                text.is_some_and(|text| {
                    let (written, _) = Settings::from_toml(&text);
                    !written.show_thumbnail && written.save_format == SaveFormat::Jpg
                })
            })
        },
    )?;

    // + makes the thumbnail stay a second longer than the default 5.
    match invoke_in_own_windows("Longer thumbnail stay") {
        Ok(true) => {}
        Ok(false) => return Err("no button named Longer thumbnail stay".to_owned()),
        Err(error) => return Err(format!("Longer thumbnail stay refused: {error}")),
    }
    wait_until("the file to keep the thumbnail 6 seconds", || {
        read_settings(file, MAX_SETTINGS_BYTES).map(|text| {
            text.is_some_and(|text| Settings::from_toml(&text).0.thumbnail_stay.ms() == 6000)
        })
    })?;

    press(START_WITH_WINDOWS)?;
    wait_until("the Run value to start this exe", || {
        run_value().map(|command| command.is_some_and(|command| starts(&command, exe)))
    })?;
    press(START_WITH_WINDOWS)?;
    wait_until("the Run value to go", || {
        run_value().map(|command| command.is_none())
    })
}

fn press(name: &str) -> Result<(), String> {
    match toggle_in_own_windows(name) {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!("no check box named {name}")),
        Err(error) => Err(format!("{name} refused the toggle: {error}")),
    }
}

fn wait_until<E: std::fmt::Debug>(
    what: &str,
    holds: impl Fn() -> Result<bool, E>,
) -> Result<(), String> {
    let deadline = Instant::now() + PATIENCE;
    let mut last_error = None;
    while Instant::now() < deadline {
        match holds() {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => last_error = Some(error),
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(format!(
        "waited {PATIENCE:?} for {what}; last read error: {last_error:?}"
    ))
}
