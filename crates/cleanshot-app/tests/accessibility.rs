//! The editor's toolbars reach UI Automation, as Narrator would read them. GPUI must own
//! the process's main thread, so this test has no harness: the walks run on a second thread
//! and end the process with the verdict.

#[expect(dead_code, reason = "each test uses part of the harness")]
mod on_screen;

use std::process::exit;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use cleanshot_win::accessibility::{ControlKind, wait_for_container};
use on_screen::{grey, run_editor};

/// Each tool group's button, named for the tool it shows, the group's first until
/// another is picked.
const TOOLS: [&str; 6] = ["Select", "Arrow", "Text", "Highlighter", "Redact", "Crop"];

const WALKS: [(&str, &str, ControlKind, &[&str]); 2] = [
    (
        "the_editor_toolbar_names_each_tool_group_to_ui_automation",
        "Tools",
        ControlKind::RadioButton,
        &TOOLS,
    ),
    (
        "the_select_tools_property_strip_has_no_image_frame_check_boxes",
        "Properties",
        ControlKind::CheckBox,
        &[],
    ),
];

fn main() {
    let start = Instant::now();
    let (first_frame, drawn) = mpsc::channel();
    thread::spawn(move || {
        let mut failed = false;
        for (test, toolbar, kind, expected) in WALKS {
            let walked = wait_for_container(ControlKind::ToolBar, toolbar, kind, start, &drawn)
                .map(|controls| {
                    controls
                        .into_iter()
                        .map(|control| control.name)
                        .collect::<Vec<_>>()
                });
            match walked {
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
    run_editor(grey(320, 240), move |at| {
        // Each walk waits for the first frame on the same channel.
        for _ in WALKS {
            first_frame
                .send(at)
                .expect("the walk thread waits for the first frame");
        }
    });
}
