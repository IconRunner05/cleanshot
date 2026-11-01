//! F-58: the editor's toolbars reach UI Automation, as Narrator would read them. GPUI must own
//! the process's main thread, so this test has no harness: the walks run on a second thread
//! and end the process with the verdict.

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

const FRAME_TOGGLES: [&str; 2] = ["Border", "Shadow"];

const WALKS: [(&str, &str, ControlKind, &[&str]); 2] = [
    (
        "f58_the_editor_toolbar_names_every_tool_to_ui_automation",
        "Tools",
        ControlKind::RadioButton,
        &TOOLS,
    ),
    (
        "f58_the_property_strip_names_its_frame_toggles_as_check_boxes",
        "Properties",
        ControlKind::CheckBox,
        &FRAME_TOGGLES,
    ),
];

const PATIENCE: Duration = Duration::from_secs(10);

fn main() {
    let start = Instant::now();
    let (first_frame, drawn) = mpsc::channel();
    thread::spawn(move || {
        let mut failed = false;
        for (test, toolbar, kind, expected) in WALKS {
            match wait_for_toolbar(toolbar, kind, start, &drawn, PATIENCE) {
                Ok(names) if names == expected => println!("{test} ... ok"),
                Ok(names) => {
                    println!("{test} ... FAILED");
                    println!("{kind:?} controls {names:?}, expected {expected:?}");
                    failed = true;
                }
                Err(missing) => {
                    println!("{test} ... FAILED");
                    println!("{missing:?}");
                    failed = true;
                }
            }
        }
        exit(i32::from(failed));
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
                // Each walk waits for the first frame on the same channel.
                for _ in WALKS {
                    first_frame
                        .send(at)
                        .expect("the walk thread waits for the first frame");
                }
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
