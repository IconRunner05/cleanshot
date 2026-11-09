//! The thumbnail takes no keyboard focus, so a screen reader finds it by its
//! window name. Run as for `toolbar_automation`: GPUI owns the thumbnail's thread and the walk
//! ends the process.

#[expect(dead_code, reason = "each test uses part of the harness")]
mod on_screen;

use std::process::exit;
use std::thread;
use std::time::{Duration, Instant};

use cleanshot_core::thumbnail_layout::ThumbnailStay;
use cleanshot_win::accessibility::controls_in_own_windows;
use cleanshot_win::capture::RECT;
use cleanshot_win::thumbnail::{Export, Thumbnail};
use on_screen::{finish, grey};

const TEST: &str = "ui_automation_finds_the_thumbnail_by_its_window_name";
const NAME: &str = "Screenshot preview";
// A debug build on a cold runner can take over 10 s to start; no budget is judged here.
const PATIENCE: Duration = Duration::from_secs(60);

fn main() {
    thread::spawn(|| {
        let start = Instant::now();
        let mut last = None;
        while start.elapsed() < PATIENCE {
            match controls_in_own_windows() {
                Ok(controls) if controls.iter().any(|control| control.name == NAME) => {
                    finish(TEST, Ok(()));
                }
                walked => last = Some(walked),
            }
            thread::sleep(Duration::from_millis(250));
        }
        finish(
            TEST,
            Err(format!(
                "no window named {NAME:?} within {PATIENCE:?}; last walk {last:?}"
            )),
        );
    });
    let preview = grey(120, 80);
    let ran = cleanshot_editor::run(move |_| {
        let export = Export::Allowed {
            drag: Box::new(|| None),
        };
        match Thumbnail::show(
            &preview,
            RECT::default(),
            ThumbnailStay::default(),
            export,
            |_| {},
        ) {
            // The thumbnail stays up until the walk ends the process.
            Ok(thumbnail) => std::mem::forget(thumbnail),
            Err(error) => finish(TEST, Err(format!("the thumbnail did not open: {error}"))),
        }
    });
    println!("GPUI stopped before the walk finished: {ran:?}");
    exit(1);
}
