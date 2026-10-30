//! F-58: the editor's toolbar reaches UI Automation, as Narrator would read it. GPUI must own
//! the process's main thread, so this test has no harness: the walk runs on a second thread
//! and ends the process with the verdict.

use std::path::PathBuf;
use std::process::exit;
use std::thread;
use std::time::{Duration, Instant};

use cleanshot_core::{DeliveryError, RgbaImage};
use cleanshot_win::accessibility::{Control, ControlKind, controls_in_own_windows};

const TOOLS: [&str; 10] = [
    "Select",
    "Arrow",
    "Line",
    "Rectangle",
    "Ellipse",
    "Pen",
    "Highlighter",
    "Text",
    "Redact",
    "Crop",
];

const PATIENCE: Duration = Duration::from_secs(10);

fn main() {
    thread::spawn(|| match toolbar_buttons() {
        Ok(buttons) if buttons == TOOLS => {
            println!("f58_the_editor_toolbar_names_every_tool_to_ui_automation ... ok");
            exit(0);
        }
        Ok(buttons) => {
            println!("f58_the_editor_toolbar_names_every_tool_to_ui_automation ... FAILED");
            println!("radio buttons {buttons:?}, expected {TOOLS:?}");
            exit(1);
        }
        Err(reason) => {
            println!("f58_the_editor_toolbar_names_every_tool_to_ui_automation ... FAILED");
            println!("{reason}");
            exit(1);
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
        );
        if let Err(error) = opened {
            println!("the editor did not open: {error}");
            exit(1);
        }
    });
    println!("GPUI stopped before the walk finished: {ran:?}");
    exit(1);
}

/// The radio buttons UI Automation reports, in order, once a toolbar named "Tools" appears.
fn toolbar_buttons() -> Result<Vec<String>, String> {
    let deadline = Instant::now() + PATIENCE;
    let mut last = String::from("no window of this process was found");
    while Instant::now() < deadline {
        match controls_in_own_windows() {
            Ok(controls) if has_tools(&controls) => {
                return Ok(controls
                    .into_iter()
                    .filter(|control| control.kind == ControlKind::RadioButton)
                    .map(|control| control.name)
                    .collect());
            }
            Ok(controls) => last = format!("no toolbar named Tools among {controls:?}"),
            Err(error) => last = format!("UI Automation failed: {error}"),
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(format!("after {PATIENCE:?}, {last}"))
}

fn has_tools(controls: &[Control]) -> bool {
    controls
        .iter()
        .any(|control| control.kind == ControlKind::ToolBar && control.name == "Tools")
}
