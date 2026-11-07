//! D-154: the colour picker's hue bar and shade square follow a drag with the left button, as
//! a person makes it on screen, and the colour they set is what the colour button then names.
//! Run as for `accessibility`: GPUI owns the window's thread, and the drags run on a second
//! thread that ends the process with the verdict.

mod on_screen;

use std::path::PathBuf;
use std::process::exit;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use cleanshot_core::{DeliveryError, RgbaImage};
use cleanshot_win::accessibility::{ControlKind, Element, drag, point_in};
use on_screen::{PATIENCE, click, pick_tool, poll};

const TEST: &str = "d154_the_hue_bar_and_the_shade_square_follow_a_drag";
const COLOUR: &str = "Colour";
const HUE: &str = "Hue";
const SHADE: &str = "Saturation and brightness";

fn main() {
    let (first_frame, drawn) = mpsc::channel();
    thread::spawn(move || {
        let verdict = drawn
            .recv_timeout(PATIENCE)
            .map_err(|error| format!("the editor drew no first frame: {error}"))
            .and_then(|_: Instant| drag_both());
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
    let capture = RgbaImage::new(1000, 700, vec![128; 1000 * 700 * 4]).expect("1000x700 RGBA");
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
                    .expect("the drag thread waits for the first frame");
            },
            |_| {},
        );
        if let Err(error) = opened {
            println!("the editor did not open: {error}");
            exit(1);
        }
    });
    println!("GPUI stopped before the drags finished: {ran:?}");
    exit(1);
}

fn drag_both() -> Result<(), String> {
    pick_tool("Arrow")?;
    click(&shown(ControlKind::Button, COLOUR)?)?;
    click(&shown(ControlKind::Button, "Custom colour")?)?;
    let mut failed = Vec::new();
    for (slider, from, to) in [
        (SHADE, (0.2, 0.2), (0.9, 0.8)),
        (HUE, (0.5, 0.5), (0.5, 0.5)),
        (HUE, (0.1, 0.5), (0.7, 0.5)),
    ] {
        let before = shown(ControlKind::Slider, slider)?;
        let colour = shown(ControlKind::Button, COLOUR)?.value;
        let (start, end) = (
            point_in(before.bounds, from.0, from.1),
            point_in(before.bounds, to.0, to.1),
        );
        drag(start, end).map_err(|error| format!("the drag on {slider} failed: {error}"))?;
        let moved = changed(ControlKind::Slider, slider, before.value.as_ref()).and_then(|after| {
            Ok((
                after,
                changed(ControlKind::Button, COLOUR, colour.as_ref())?,
            ))
        });
        match moved {
            Ok((after, set)) => println!(
                "{slider} {start:?} to {end:?}: {:?} became {:?}; the colour {colour:?} became {:?}",
                before.value, after.value, set.value
            ),
            Err(reason) => {
                println!(
                    "{slider} {start:?} to {end:?} at {:?}: nothing changed",
                    before.bounds
                );
                failed.push(reason);
            }
        }
    }
    if let Some(first) = failed.first() {
        return Err(format!(
            "{} of 3 presses changed nothing; the first: {first}",
            failed.len()
        ));
    }
    Ok(())
}

fn shown(kind: ControlKind, name: &str) -> Result<Element, String> {
    first_within(kind, name, |_| true).map_err(|seen| format!("no {kind:?} named {name}: {seen}"))
}

/// The control once its value differs from `was`, which it must within `PATIENCE`.
fn changed(kind: ControlKind, name: &str, was: Option<&String>) -> Result<Element, String> {
    first_within(kind, name, |control| control.value.as_ref() != was)
        .map_err(|seen| format!("the {kind:?} {name} still names {was:?}: {seen}"))
}

fn first_within(
    kind: ControlKind,
    name: &str,
    holds: impl Fn(&Element) -> bool,
) -> Result<Element, String> {
    poll(|controls| {
        let found = controls
            .iter()
            .find(|control| control.kind == kind && control.name == name);
        if let Some(control) = found.filter(|control| holds(control)) {
            return Ok(control.clone());
        }
        let every: Vec<String> = controls
            .iter()
            .map(|control| {
                format!(
                    "{:?} {:?} {:?} {:?}",
                    control.kind, control.name, control.value, control.bounds
                )
            })
            .collect();
        Err(format!("last seen {found:?}; every control: {every:#?}"))
    })
}
