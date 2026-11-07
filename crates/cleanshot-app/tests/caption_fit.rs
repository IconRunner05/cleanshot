//! D-161: at the editor's smallest width nothing in the caption row is clipped or overlaps,
//! with Copy right after the property strip (D-162), and the property strip's groups that do
//! not fit open from "More properties". Run as for `accessibility`: GPUI owns the window's
//! thread, and the checks run on a second thread that ends the process with the verdict.

use std::path::PathBuf;
use std::process::exit;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use cleanshot_core::{DeliveryError, RgbaImage};
use cleanshot_win::accessibility::{ControlKind, Element, controls_in_own_windows, drag, point_in};

const TEST: &str = "d161_nothing_in_the_caption_row_is_clipped_at_the_smallest_width";
const MORE_PROPERTIES: &str = "More properties";
const ARROW: &str = "Arrow";
// A debug build on a cold runner can take seconds to lay out a frame; no budget is judged here.
const PATIENCE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);

fn main() {
    let (first_frame, drawn) = mpsc::channel();
    thread::spawn(move || {
        let verdict = drawn
            .recv_timeout(PATIENCE)
            .map_err(|error| format!("the editor drew no first frame: {error}"))
            .and_then(|_: Instant| check_row());
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
    // A capture smaller than the toolbar opens the editor at its smallest width (D-110).
    let capture = RgbaImage::new(64, 64, vec![128; 64 * 64 * 4]).expect("64x64 RGBA");
    let ran = cleanshot_editor::run(move |cx| {
        let opened = cleanshot_editor::open(
            cx,
            capture,
            None,
            cleanshot_editor::Outputs::new(
                |_, _| Err(DeliveryError::ClipboardBusy),
                || Ok((PathBuf::new(), String::new())),
                |_, _| Err(DeliveryError::ClipboardBusy),
            ),
            |_| None,
            move |at| {
                first_frame
                    .send(at)
                    .expect("the check thread waits for the first frame");
            },
            |_| {},
        );
        if let Err(error) = opened {
            println!("the editor did not open: {error}");
            exit(1);
        }
    });
    println!("GPUI stopped before the checks finished: {ran:?}");
    exit(1);
}

fn check_row() -> Result<(), String> {
    // D-166: the select tool's strip is empty, so the arrow tool fills it.
    let tools = settled(|controls| {
        named(controls, ControlKind::RadioButton, ARROW)
            .is_none()
            .then(|| format!("no {ARROW} tool"))
    })?;
    let arrow = named(&tools, ControlKind::RadioButton, ARROW).expect("settled found the tool");
    let middle = point_in(arrow.bounds, 0.5, 0.5);
    drag(middle, middle).map_err(|error| format!("the click on {ARROW} failed: {error}"))?;
    let row = settled(|controls| {
        fits(controls).err().or_else(|| {
            named(controls, ControlKind::Button, MORE_PROPERTIES)
                .is_none()
                .then(|| "at the smallest width no group went behind More properties".to_owned())
        })
    })?;
    let more =
        named(&row, ControlKind::Button, MORE_PROPERTIES).expect("settled found the chevron");
    let middle = point_in(more.bounds, 0.5, 0.5);
    drag(middle, middle)
        .map_err(|error| format!("the click on {MORE_PROPERTIES} failed: {error}"))?;
    settled(|controls| {
        named(controls, ControlKind::Group, MORE_PROPERTIES)
            .is_none()
            .then(|| format!("{MORE_PROPERTIES} opened no panel"))
    })?;
    Ok(())
}

/// The caption row's controls, once `wrong` finds nothing wrong with every control seen, which
/// it must within `PATIENCE`: the strip is fitted on the frame after it is measured.
fn settled(wrong: impl Fn(&[Element]) -> Option<String>) -> Result<Vec<Element>, String> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let controls = controls_in_own_windows().map_err(|error| error.to_string())?;
        let Some(problem) = wrong(&controls) else {
            return Ok(controls);
        };
        if Instant::now() >= deadline {
            let every: Vec<String> = controls
                .iter()
                .map(|control| {
                    format!("{:?} {:?} {:?}", control.kind, control.name, control.bounds)
                })
                .collect();
            return Err(format!("{problem}; every control: {every:#?}"));
        }
        thread::sleep(POLL);
    }
}

/// Whether every button in the caption row lies inside the window, clear of every other, and
/// the property strip ends before Copy.
fn fits(controls: &[Element]) -> Result<(), String> {
    let close = named(controls, ControlKind::Button, "Close").ok_or("no Close button")?;
    let copy = named(controls, ControlKind::Button, "Copy").ok_or("no Copy button")?;
    let strip = named(controls, ControlKind::ToolBar, "Properties").ok_or("no Properties strip")?;
    if strip.bounds.right > copy.bounds.left {
        return Err(format!(
            "the strip {:?} runs under Copy {:?}",
            strip.bounds, copy.bounds
        ));
    }
    // D-162: Copy follows the strip; the drag space, at least 40 px, comes after it.
    if copy.bounds.left - strip.bounds.right >= 40 {
        return Err(format!(
            "Copy {:?} is not right after the strip {:?}",
            copy.bounds, strip.bounds
        ));
    }
    let in_row = |control: &Element| {
        control.bounds.top >= close.bounds.top && control.bounds.bottom <= close.bounds.bottom
    };
    let buttons: Vec<&Element> = controls
        .iter()
        .filter(|control| {
            matches!(
                control.kind,
                ControlKind::Button | ControlKind::RadioButton | ControlKind::CheckBox
            ) && in_row(control)
        })
        .collect();
    for (index, button) in buttons.iter().enumerate() {
        if button.bounds.right > close.bounds.right {
            return Err(format!(
                "{} {:?} runs past the window",
                button.name, button.bounds
            ));
        }
        let overlapped = buttons[index + 1..].iter().find(|other| {
            button.bounds.left < other.bounds.right && other.bounds.left < button.bounds.right
        });
        if let Some(other) = overlapped {
            return Err(format!(
                "{} {:?} overlaps {} {:?}",
                button.name, button.bounds, other.name, other.bounds
            ));
        }
    }
    Ok(())
}

fn named<'a>(controls: &'a [Element], kind: ControlKind, name: &str) -> Option<&'a Element> {
    controls
        .iter()
        .find(|control| control.kind == kind && control.name == name)
}
