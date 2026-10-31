//! F-05: the capture toolbar reaches UI Automation, and its Copy button works from there as
//! Narrator's activate command would press it. The overlay's windows need a message loop on
//! their thread, so GPUI runs it, the walk runs on a second thread, and that thread ends the
//! process with the verdict.

use std::process::exit;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use cleanshot_core::RgbaImage;
use cleanshot_win::accessibility::{
    Control, ControlKind, controls_in_own_windows, invoke_in_own_windows,
};
use cleanshot_win::capture::{MonitorImage, RECT};
use cleanshot_win::overlay::{Overlay, OverlayEvent};

const TEST: &str = "f05_ui_automation_names_the_toolbar_buttons_and_copy_presses_copy";
const BUTTONS: [&str; 4] = ["Copy", "Save", "Edit", "Cancel"];
const SELECTED: RECT = RECT {
    left: 100,
    top: 100,
    right: 300,
    bottom: 200,
};
const PATIENCE: Duration = Duration::from_secs(10);

fn main() {
    let (chosen, choice) = mpsc::channel();
    thread::spawn(move || {
        let verdict = toolbar_buttons().and_then(|buttons| {
            if buttons != BUTTONS {
                return Err(format!("buttons {buttons:?}, expected {BUTTONS:?}"));
            }
            match invoke_in_own_windows("Copy") {
                Ok(true) => {}
                Ok(false) => return Err(String::from("no button named Copy to invoke")),
                Err(error) => return Err(format!("Copy could not be invoked: {error}")),
            }
            match choice.recv_timeout(PATIENCE) {
                Ok(Some(selected)) if selected == SELECTED => Ok(()),
                Ok(other) => Err(format!("the overlay sent {other:?} for Copy")),
                Err(error) => Err(format!("the overlay sent nothing for Copy: {error}")),
            }
        });
        match verdict {
            Ok(()) => {
                println!("{TEST} ... ok");
                exit(0);
            }
            Err(reason) => {
                println!("{TEST} ... FAILED");
                println!("{reason}");
                exit(1);
            }
        }
    });
    let ran = cleanshot_editor::run(move |_| {
        let monitor = MonitorImage {
            bounds: RECT {
                left: 0,
                top: 0,
                right: 800,
                bottom: 600,
            },
            image: RgbaImage::new(800, 600, vec![128; 800 * 600 * 4]).expect("800x600 RGBA"),
        };
        let shown = Overlay::show(&[monitor], Some(SELECTED), move |event| {
            let copied = match event {
                OverlayEvent::Copy(selected) => Some(selected),
                OverlayEvent::Cancelled
                | OverlayEvent::Save(_)
                | OverlayEvent::SaveAs(_)
                | OverlayEvent::Edit(_)
                | OverlayEvent::DrawFailed(_) => None,
            };
            if chosen.send(copied).is_err() {
                println!("the walk ended before the overlay answered");
            }
        });
        match shown {
            // The overlay stays up until the walk ends the process.
            Ok(overlay) => std::mem::forget(overlay),
            Err(error) => {
                println!("{TEST} ... FAILED");
                println!("the overlay did not open: {error}");
                exit(1);
            }
        }
    });
    println!("GPUI stopped before the walk finished: {ran:?}");
    exit(1);
}

/// The buttons UI Automation reports, in order, once a toolbar named "Capture" appears.
fn toolbar_buttons() -> Result<Vec<String>, String> {
    let deadline = Instant::now() + PATIENCE;
    let mut last = String::from("no window of this process was found");
    while Instant::now() < deadline {
        match controls_in_own_windows() {
            Ok(controls) if has_toolbar(&controls) => {
                return Ok(controls
                    .into_iter()
                    .filter(|control| control.kind == ControlKind::Button)
                    .map(|control| control.name)
                    .collect());
            }
            Ok(controls) => last = format!("no toolbar named Capture among {controls:?}"),
            Err(error) => last = format!("UI Automation failed: {error}"),
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(format!("after {PATIENCE:?}, {last}"))
}

fn has_toolbar(controls: &[Control]) -> bool {
    controls
        .iter()
        .any(|control| control.kind == ControlKind::ToolBar && control.name == "Capture")
}
