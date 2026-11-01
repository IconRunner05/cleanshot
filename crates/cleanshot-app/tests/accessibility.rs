//! F-58: the editor's toolbar reaches UI Automation, as Narrator would read it. GPUI must own
//! the process's main thread, so this test has no harness: the walk runs on a second thread
//! and ends the process with the verdict.

use std::path::PathBuf;
use std::process::exit;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use cleanshot_core::{DeliveryError, RgbaImage};
use cleanshot_win::accessibility::{Control, ControlKind, controls_in_own_windows};

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
    thread::spawn(move || match toolbar_buttons(start, &drawn) {
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

/// The radio buttons UI Automation reports, in order, once a toolbar named "Tools" appears.
/// The first walk waits for the editor's first frame, as no user reaches the toolbar before it.
fn toolbar_buttons(start: Instant, first_frame: &Receiver<Instant>) -> Result<Vec<String>, String> {
    let drawn = first_frame
        .recv_timeout(PATIENCE)
        .map_err(|error| format!("the editor drew no first frame within {PATIENCE:?}: {error}"))?;
    let deadline = Instant::now() + PATIENCE;
    let mut walks = 0;
    let mut window_seen = None;
    let mut last = String::from("no window of this process was found");
    while Instant::now() < deadline {
        walks += 1;
        match controls_in_own_windows() {
            Ok(controls) if has_tools(&controls) => {
                return Ok(controls
                    .into_iter()
                    .filter(|control| control.kind == ControlKind::RadioButton)
                    .map(|control| control.name)
                    .collect());
            }
            Ok(controls) => {
                if !controls.is_empty() {
                    window_seen.get_or_insert(start.elapsed());
                }
                last = format!("no toolbar named Tools among {controls:?}");
            }
            Err(error) => last = format!("UI Automation failed: {error}"),
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(format!(
        "{walks} walks over {PATIENCE:?}; from the start, the first frame came at {:?} and a \
         window was first seen at {window_seen:?}; {last}",
        drawn - start
    ))
}

fn has_tools(controls: &[Control]) -> bool {
    controls
        .iter()
        .any(|control| control.kind == ControlKind::ToolBar && control.name == "Tools")
}
