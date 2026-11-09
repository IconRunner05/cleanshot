//! The editor and the capture toolbar as the Windows UI tests work them: through UI Automation
//! and real input. GPUI owns the main thread, so each test has no harness: its checks run on a
//! second thread that ends the process with the verdict.

use std::error::Error as _;
use std::path::PathBuf;
use std::process::exit;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use cleanshot_core::{DeliveryError, RgbaImage, SavingOff};
use cleanshot_editor::{App, EditorWindow, Outputs};
use cleanshot_win::accessibility::{
    ControlKind, Element, controls_in_own_windows, drag, point_in, wait_for_container,
};
use cleanshot_win::capture::{BgraFrame, MonitorImage, RECT};
use cleanshot_win::overlay::{Opening, Overlay, OverlayEvent};

/// How long a check waits for the editor: a debug build on a cold runner can take seconds to
/// lay out a frame or open a menu, and no budget is judged here.
pub const PATIENCE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);

/// The region [`check_toolbar`] opens the overlay with already selected.
pub const SELECTED: RECT = RECT {
    left: 100,
    top: 100,
    right: 300,
    bottom: 200,
};

/// Prints `test`'s verdict as a test harness would and ends the process with it.
pub fn finish(test: &str, verdict: Result<(), String>) -> ! {
    match verdict {
        Ok(()) => {
            println!("{test} ... ok");
            exit(0);
        }
        Err(reason) => {
            println!("{test} ... FAILED");
            println!("{reason}");
            exit(1);
        }
    }
}

/// A mid-grey image of the size asked for.
pub fn grey(width: u32, height: u32) -> RgbaImage {
    RgbaImage::new(width, height, vec![128; (width * height * 4) as usize])
        .expect("a grey image's bytes match its size")
}

/// Opens an editor on `capture` whose Copy and Save always fail.
pub fn open_editor(
    cx: &mut App,
    capture: RgbaImage,
    on_first_frame: impl FnOnce(Instant) + 'static,
) -> Result<EditorWindow, cleanshot_editor::Error> {
    cleanshot_editor::open(
        cx,
        capture,
        None,
        Outputs::new(
            |_, _| Err(DeliveryError::ClipboardBusy),
            || Ok((PathBuf::new(), String::new())),
            |_, _| Err(DeliveryError::ClipboardBusy),
        ),
        |_| None,
        on_first_frame,
        |_| {},
    )
}

/// Runs GPUI on this thread with an editor open on `capture`. Something else must end the
/// process: this one fails the run if the editor does not open or GPUI stops.
pub fn run_editor(capture: RgbaImage, on_first_frame: impl FnOnce(Instant) + 'static) -> ! {
    let ran = cleanshot_editor::run(move |cx| {
        if let Err(error) = open_editor(cx, capture, on_first_frame) {
            println!("the editor did not open: {error}");
            exit(1);
        }
    });
    println!("GPUI stopped before the checks finished: {ran:?}");
    exit(1);
}

/// Opens an editor on `capture` and, once it has drawn, ends the process with `check`'s verdict.
pub fn check_editor(
    test: &'static str,
    capture: RgbaImage,
    check: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> ! {
    let (first_frame, drawn) = mpsc::channel();
    thread::spawn(move || {
        let verdict = drawn
            .recv_timeout(PATIENCE)
            .map_err(|error| format!("the editor drew no first frame: {error}"))
            .and_then(|_: Instant| check());
        finish(test, verdict)
    });
    run_editor(capture, move |at| {
        first_frame
            .send(at)
            .expect("the check thread waits for the first frame");
    })
}

/// Opens the capture overlay on one 800x600 monitor with [`SELECTED`] selected. Once UI
/// Automation finds its toolbar, ends the process with the verdict of `check`, which gets the
/// toolbar's buttons and what `heard` makes of each overlay event.
pub fn check_toolbar<Heard: Send + 'static>(
    test: &'static str,
    saving_off: Option<SavingOff>,
    heard: impl Fn(OverlayEvent) -> Heard + 'static,
    check: impl FnOnce(Vec<Element>, Receiver<Heard>) -> Result<(), String> + Send + 'static,
) -> ! {
    let start = Instant::now();
    let (hear, events) = mpsc::channel();
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
        .and_then(|buttons| check(buttons, events));
        finish(test, verdict)
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
                saving_off,
                ..Opening::default()
            },
            Vec::new,
            move |event| {
                if hear.send(heard(event)).is_err() {
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
            Err(error) => finish(test, Err(format!("the overlay did not open: {error}"))),
        }
    });
    println!("GPUI stopped before the walk finished: {ran:?}");
    exit(1);
}

/// Reads the controls in this process's windows until `found` answers with a value, within
/// `PATIENCE`, or returns the last answer's reason. A read UI Automation refuses while a
/// window changes is tried again, as a not-yet answer is.
pub fn poll<T>(mut found: impl FnMut(&[Element]) -> Result<T, String>) -> Result<T, String> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let answer = match controls_in_own_windows() {
            Ok(controls) => found(&controls),
            Err(error) => Err(format!("{error}: {:?}", error.source())),
        };
        match answer {
            Ok(value) => return Ok(value),
            Err(reason) if Instant::now() >= deadline => return Err(reason),
            Err(_not_yet) => thread::sleep(POLL),
        }
    }
}

/// Picks the tool named `tool` as a person does: a click on its group's button opens the
/// group's menu, and a click on the menu's row of that name picks it.
pub fn pick_tool(tool: &str) -> Result<(), String> {
    let button = radio_buttons(tool, 1)?.remove(0);
    click(&button)?;
    let row = radio_buttons(tool, 2)?.remove(1);
    click(&row)
}

/// Clicks the middle of `control` with the left button.
pub fn click(control: &Element) -> Result<(), String> {
    let middle = point_in(control.bounds, 0.5, 0.5);
    drag(middle, middle).map_err(|error| format!("the click on {} failed: {error}", control.name))
}

/// The radio buttons named `name`, once there are `count` of them, the toolbar's first.
fn radio_buttons(name: &str, count: usize) -> Result<Vec<Element>, String> {
    poll(|controls| {
        let named: Vec<Element> = controls
            .iter()
            .filter(|control| control.kind == ControlKind::RadioButton && control.name == name)
            .cloned()
            .collect();
        if named.len() == count {
            Ok(named)
        } else {
            Err(format!(
                "{count} radio buttons named {name} wanted: {named:?}"
            ))
        }
    })
}
