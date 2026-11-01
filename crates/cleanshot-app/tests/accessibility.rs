//! F-58: the editor's toolbar reaches UI Automation, as Narrator would read it. GPUI must own
//! the process's main thread, so this test has no harness: the walk runs on a second thread
//! and ends the process with the verdict.

use std::path::PathBuf;
use std::process::exit;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use cleanshot_core::{DeliveryError, RgbaImage};
use cleanshot_win::accessibility::{ControlKind, wait_for_toolbar};

const TOOLS: [&str; 11] = [
    "Select",
    "Arrow",
    "Line",
    "Rectangle",
    "Ellipse",
    "Pen",
    "Highlighter",
    "Text",
    "Redact",
    "Spotlight",
    "Crop",
];

const PATIENCE: Duration = Duration::from_secs(10);

fn main() {
    let start = Instant::now();
    let (first_frame, drawn) = mpsc::channel();
    thread::spawn(move || {
        match wait_for_toolbar("Tools", ControlKind::RadioButton, start, &drawn, PATIENCE) {
            Ok(buttons) if buttons == TOOLS => {
                println!("f58_the_editor_toolbar_names_every_tool_to_ui_automation ... ok");
                exit(0);
            }
            Ok(buttons) => {
                println!("f58_the_editor_toolbar_names_every_tool_to_ui_automation ... FAILED");
                println!("radio buttons {buttons:?}, expected {TOOLS:?}");
                exit(1);
            }
            Err(missing) => {
                println!("f58_the_editor_toolbar_names_every_tool_to_ui_automation ... FAILED");
                println!("{missing:?}");
                exit(1);
            }
        }
    });
    let capture = RgbaImage::new(320, 240, vec![128; 320 * 240 * 4]).expect("320x240 RGBA");
    let ran = cleanshot_editor::run(move |cx| {
        let opened = cleanshot_editor::open(
            cx,
            capture,
            |_, _| Err(DeliveryError::ClipboardBusy),
            || (PathBuf::new(), String::new()),
            |_, _| Err(DeliveryError::ClipboardBusy),
            move |at| {
                first_frame
                    .send(at)
                    .expect("the walk thread waits for the first frame");
            },
        );
        if let Err(error) = opened {
            println!("the editor did not open: {error}");
            exit(1);
        }
    });
    println!("GPUI stopped before the walk finished: {ran:?}");
    exit(1);
}
