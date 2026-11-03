//! F-55, F-56, F-57: the settings window's check boxes reach UI Automation, and pressed as
//! Narrator's activate command would press them, Show a thumbnail is written to the settings
//! file and Start with Windows sets and clears the HKCU Run value (D-30, D-93). A format picked
//! from the Options menu (F-12) while the window is open is kept when the window next writes
//! the file. Run as for `toolbar_automation`: GPUI owns the window's thread and the walk ends
//! the process.

use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use cleanshot_core::{
    Applied, MAX_SETTINGS_BYTES, Policy, PolicyValue, SaveFormat, Settings, run_command, starts,
};
use cleanshot_win::accessibility::{ControlKind, toggle_in_own_windows, wait_for_container};
use cleanshot_win::settings_file::{read_settings, write_settings};
use cleanshot_win::startup::{remove_run_value, run_value, set_run_value};

const TEST: &str =
    "f12_f55_f56_the_settings_check_boxes_keep_a_menu_pick_and_write_the_file_and_the_run_value";
const THUMBNAIL: &str = "Show a thumbnail";
const START_WITH_WINDOWS: &str = "Start with Windows";
// A debug build on a cold runner can take seconds to apply a press; no budget is judged here.
const PATIENCE: Duration = Duration::from_secs(10);

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
    let walked = (
        file.clone(),
        exe.clone(),
        left_as_found.clone(),
        Arc::clone(&in_force),
    );
    thread::spawn(move || {
        let (file, exe, left_as_found, in_force) = walked;
        left_as_found.finish(walk(&file, &exe, &in_force, start, &shown));
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
        // F-57: under HideSettings the window does not open; the tray's half is win's f57 test.
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
        match opened_now {
            Ok(()) => opened
                .send(Instant::now())
                .expect("the walk thread waits for the window"),
            Err(error) => {
                left_as_found.finish(Err(format!("the settings window did not open: {error}")))
            }
        }
    });
    stopped.finish(Err(format!(
        "GPUI stopped before the walk finished: {ran:?}"
    )));
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
) -> Result<(), String> {
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

    // F-12: the Options menu picks JPG while the window is open, as its apply_settings would.
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
