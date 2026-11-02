//! F-55, F-56, F-57: the settings window's check boxes reach UI Automation, and pressed as
//! Narrator's activate command would press them, Show a thumbnail is written to the settings
//! file and Start with Windows sets and clears the HKCU Run value (D-30, D-93). Run as for
//! `toolbar_automation`: GPUI owns the window's thread and the walk ends the process.

use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use cleanshot_core::{
    Applied, MAX_SETTINGS_BYTES, Policy, PolicyValue, Settings, run_command, starts,
};
use cleanshot_win::accessibility::{ControlKind, toggle_in_own_windows, wait_for_container};
use cleanshot_win::settings_file::{read_settings, write_settings};
use cleanshot_win::startup::{remove_run_value, run_value, set_run_value};

const TEST: &str = "f55_f56_the_settings_check_boxes_write_the_file_and_the_run_value";
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
    remove_left_over(&file);
    let run_before = run_value().expect("the HKCU Run key reads");
    let start = Instant::now();
    let (opened, shown) = mpsc::channel();
    let walked_file = file.clone();
    let walked_exe = exe.clone();
    thread::spawn(move || {
        let verdict = walk(&walked_file, &walked_exe, start, &shown);
        remove_left_over(&walked_file);
        let restored = match &run_before {
            Some(command) => set_run_value(command),
            None => remove_run_value(),
        };
        match (verdict, restored) {
            (Ok(()), Ok(())) => {
                println!("{TEST} ... ok");
                exit(0);
            }
            (verdict, restored) => {
                println!("{TEST} ... FAILED: {verdict:?}, Run value restored: {restored:?}");
                exit(1);
            }
        }
    });
    let ran = cleanshot_editor::run(move |cx| {
        let (hidden, _) = Policy::from_values(
            PolicyValue::Missing,
            PolicyValue::Missing,
            PolicyValue::Missing,
            PolicyValue::Number(1),
        );
        // F-57: under HideSettings the window does not open; the tray's half is win's f57 test.
        let refused = cleanshot_editor::open_settings(
            cx,
            Settings::default(),
            hidden,
            |_| unreachable_apply(),
            false,
            |_| Ok(()),
        );
        if !matches!(refused, Err(cleanshot_editor::Error::SettingsHidden)) {
            println!("{TEST} ... FAILED: HideSettings opened the window: {refused:?}");
            exit(1);
        }
        let opened_now = cleanshot_editor::open_settings(
            cx,
            Settings::default(),
            Policy::default(),
            move |changed| Applied {
                saved: write_settings(&file, &changed),
                taken: Vec::new(),
            },
            false,
            move |on| {
                if on {
                    set_run_value(&run_command(&exe))
                } else {
                    remove_run_value()
                }
            },
        );
        match opened_now {
            Ok(()) => opened
                .send(Instant::now())
                .expect("the walk thread waits for the window"),
            Err(error) => {
                println!("{TEST} ... FAILED: the settings window did not open: {error}");
                exit(1);
            }
        }
    });
    println!("{TEST} ... FAILED: GPUI stopped before the walk finished: {ran:?}");
    exit(1);
}

// A file from an earlier run would pass the thumbnail check before any press.
fn remove_left_over(file: &Path) {
    match std::fs::remove_file(file) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            println!(
                "{TEST} ... FAILED: {} could not be removed: {error}",
                file.display()
            );
            exit(1);
        }
    }
}

fn unreachable_apply() -> Applied {
    println!("{TEST} ... FAILED: a hidden settings window applied a change");
    exit(1);
}

fn walk(
    file: &Path,
    exe: &str,
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

    press(THUMBNAIL)?;
    wait_until("the file to turn the thumbnail off", || {
        read_settings(file, MAX_SETTINGS_BYTES)
            .ok()
            .flatten()
            .is_some_and(|text| !Settings::from_toml(&text).0.show_thumbnail)
    })?;

    press(START_WITH_WINDOWS)?;
    wait_until("the Run value to start this exe", || {
        run_value().is_ok_and(|command| command.is_some_and(|command| starts(&command, exe)))
    })?;
    press(START_WITH_WINDOWS)?;
    wait_until("the Run value to go", || {
        run_value().is_ok_and(|command| command.is_none())
    })
}

fn press(name: &str) -> Result<(), String> {
    match toggle_in_own_windows(name) {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!("no check box named {name}")),
        Err(error) => Err(format!("{name} refused the toggle: {error}")),
    }
}

fn wait_until(what: &str, holds: impl Fn() -> bool) -> Result<(), String> {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if holds() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(format!("waited {PATIENCE:?} for {what}"))
}
