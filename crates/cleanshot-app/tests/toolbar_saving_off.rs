//! F-57: while the organisation's policy turns saving off, UI Automation reports the capture
//! toolbar's Save as not enabled, with the reason as its help text, and refuses to invoke it.
//! Run as for `toolbar_automation`: GPUI owns the overlay's thread and the walk ends the process.

use std::process::exit;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use cleanshot_core::{SavingOff, saving_off_notice};
use cleanshot_win::accessibility::{ControlKind, invoke_in_own_windows, wait_for_container};
use cleanshot_win::capture::{BgraFrame, MonitorImage, RECT};
use cleanshot_win::overlay::{Opening, Overlay};

const TEST: &str = "f57_ui_automation_reports_save_off_with_the_reason_and_refuses_to_press_it";
const SELECTED: RECT = RECT {
    left: 100,
    top: 100,
    right: 300,
    bottom: 200,
};
fn main() {
    let start = Instant::now();
    let reason = saving_off_notice(SavingOff::ClipboardOnly);
    let expected = reason.clone();
    let (chosen, choice) = mpsc::channel();
    let (painted, shown) = mpsc::channel();
    thread::spawn(move || {
        let verdict = wait_for_container(
            ControlKind::ToolBar,
            "Capture",
            ControlKind::Button,
            start,
            &shown,
        )
        .map_err(|missing| format!("{missing:?}"))
        .and_then(|buttons| {
            let off: Vec<_> = buttons
                .iter()
                .map(|button| {
                    (
                        button.name.as_str(),
                        button.enabled,
                        button.help_text.as_str(),
                    )
                })
                .collect();
            let wanted = [
                ("Options", true, ""),
                ("Copy", true, ""),
                ("Save", false, expected.as_str()),
                ("Edit", true, ""),
                ("Cancel", true, ""),
            ];
            if off != wanted {
                return Err(format!("buttons {off:?}, expected {wanted:?}"));
            }
            if invoke_in_own_windows("Save").is_ok() {
                return Err(String::from("UI Automation pressed the Save that is off"));
            }
            match choice.recv_timeout(Duration::from_secs(1)) {
                Err(mpsc::RecvTimeoutError::Timeout) => Ok(()),
                Ok(event) => Err(format!("the overlay sent {event} for a refused press")),
                Err(error) => Err(format!("the overlay's channel closed: {error}")),
            }
        });
        match verdict {
            Ok(()) => {
                println!("{TEST} ... ok");
                exit(0);
            }
            Err(failure) => {
                println!("{TEST} ... FAILED");
                println!("{failure}");
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
            frame: BgraFrame::new(800, 600, vec![128; 800 * 600 * 4]).expect("800x600 BGRA"),
        };
        let shown = Overlay::show(
            &[monitor],
            Opening {
                selected: Some(SELECTED),
                saving_off: Some(SavingOff::ClipboardOnly),
                ..Opening::default()
            },
            Vec::new,
            move |event| {
                if chosen.send(format!("{event:?}")).is_err() {
                    println!("the walk ended before the overlay answered");
                }
            },
        );
        match shown {
            // The overlay stays up until the walk ends the process.
            Ok(overlay) => {
                painted
                    .send(Instant::now())
                    .expect("the walk thread waits for the overlay");
                std::mem::forget(overlay);
            }
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
