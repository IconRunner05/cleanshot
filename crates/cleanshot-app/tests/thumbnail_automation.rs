//! F-09, F-58 (D-96): the thumbnail takes no keyboard focus, so a screen reader finds it by its
//! window name. Run as for `toolbar_automation`: GPUI owns the thumbnail's thread and the walk
//! ends the process.

use std::process::exit;
use std::thread;
use std::time::{Duration, Instant};

use cleanshot_core::RgbaImage;
use cleanshot_win::accessibility::controls_in_own_windows;
use cleanshot_win::capture::RECT;
use cleanshot_win::thumbnail::{Saving, Thumbnail};

const TEST: &str = "f58_ui_automation_finds_the_thumbnail_by_its_window_name";
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
                    println!("{TEST} ... ok");
                    exit(0);
                }
                walked => last = Some(walked),
            }
            thread::sleep(Duration::from_millis(250));
        }
        println!("{TEST} ... FAILED");
        println!("no window named {NAME:?} within {PATIENCE:?}; last walk {last:?}");
        exit(1);
    });
    let preview = RgbaImage::new(120, 80, vec![128; 120 * 80 * 4]).expect("120x80 RGBA");
    let ran = cleanshot_editor::run(move |_| {
        let saving = Saving::On {
            drag: Box::new(|| None),
        };
        match Thumbnail::show(&preview, RECT::default(), saving, |_| {}) {
            // The thumbnail stays up until the walk ends the process.
            Ok(thumbnail) => std::mem::forget(thumbnail),
            Err(error) => {
                println!("{TEST} ... FAILED");
                println!("the thumbnail did not open: {error}");
                exit(1);
            }
        }
    });
    println!("GPUI stopped before the walk finished: {ran:?}");
    exit(1);
}
