//! PROBE (not for main): does Colour's Invoke fail right after the Shapes menu closes, and is
//! it the window walk (test helper) or the Invoke itself (product)?

mod on_screen;

use std::path::PathBuf;
use std::process::exit;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use cleanshot_core::{DeliveryError, RgbaImage};
use cleanshot_win::Error;
use cleanshot_win::accessibility::{ControlKind, invoke_in_own_windows};
use on_screen::{PATIENCE, pick_tool, poll};

const TEST: &str = "probe_colour_invoke_after_the_shapes_menu";

fn main() {
    let (first_frame, drawn) = mpsc::channel();
    thread::spawn(move || {
        let verdict = drawn
            .recv_timeout(PATIENCE)
            .map_err(|error| format!("the editor drew no first frame: {error}"))
            .and_then(|_: Instant| probe());
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
                    .expect("the probe thread waits for the first frame");
            },
            |_| {},
        );
        if let Err(error) = opened {
            println!("the editor did not open: {error}");
            exit(1);
        }
    });
    println!("GPUI stopped before the probe finished: {ran:?}");
    exit(1);
}

fn probe() -> Result<(), String> {
    for round in 1..=3 {
        pick_tool("Arrow")?;
        let start = Instant::now();
        let mut invoked = false;
        for attempt in 1..=60 {
            let answer = invoke_in_own_windows("Colour");
            let at = start.elapsed().as_millis();
            match &answer {
                Ok(found) => println!("round {round} attempt {attempt} at {at} ms: Ok({found})"),
                Err(Error::Accessibility(source)) => println!(
                    "round {round} attempt {attempt} at {at} ms: Accessibility {:?} {}",
                    source.code(),
                    source.message()
                ),
                Err(other) => println!("round {round} attempt {attempt} at {at} ms: {other}"),
            }
            if let Ok(true) = answer {
                invoked = true;
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        if !invoked {
            return Err(format!("round {round}: Colour was never invoked"));
        }
        let opened = poll(|controls| {
            controls
                .iter()
                .any(|control| {
                    control.kind == ControlKind::Button && control.name == "Custom colour"
                })
                .then_some(())
                .ok_or_else(|| "the Invoke on Colour opened no palette".to_owned())
        });
        println!(
            "round {round}: palette after Invoke at {} ms: {opened:?}",
            start.elapsed().as_millis()
        );
        opened?;
        let shut = invoke_in_own_windows("Colour");
        println!("round {round}: second Invoke on Colour (closes it): {shut:?}");
        poll(|controls| {
            (!controls.iter().any(|control| {
                control.kind == ControlKind::Button && control.name == "Custom colour"
            }))
            .then_some(())
            .ok_or_else(|| "the second Invoke left the palette open".to_owned())
        })?;
    }
    Ok(())
}
