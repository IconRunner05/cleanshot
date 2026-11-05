//! An editor tells its opener exactly once that its window is gone, whether the app closed it
//! with `EditorWindow::close` or the window closed itself, as Alt+F4 and the caption's close
//! button make it do, and at once when it asks after the window is gone. Runs real windows, so
//! only on Windows; the test ends the process.

use std::cell::Cell;
use std::path::PathBuf;
use std::process::exit;
use std::rc::Rc;
use std::time::Duration;

use cleanshot_core::{DeliveryError, RgbaImage};
use cleanshot_editor::{App, EditorWindow, Outputs};

const TEST: &str = "an_editor_tells_its_opener_once_when_its_window_is_gone";
/// Long enough for both windows to open, draw and, once closed, be released.
const SETTLE: Duration = Duration::from_secs(2);

fn main() {
    let ran = cleanshot_editor::run(|cx| {
        let closings = Rc::new(Cell::new(0_u32));
        let by_the_app = open(cx, &closings);
        let by_itself = open(cx, &closings);
        cx.spawn(async move |cx| {
            cx.background_executor().timer(SETTLE).await;
            if closings.get() != 0 {
                finish(Err(format!("{} closings before any close", closings.get())));
            }
            let closed = cx.update(|cx| {
                by_the_app.close(cx);
                cx.windows()
                    .into_iter()
                    .try_for_each(|window| window.update(cx, |_, window, _| window.remove_window()))
            });
            if let Err(error) = closed {
                finish(Err(format!("the second window did not close: {error}")));
            }
            cx.background_executor().timer(SETTLE).await;
            if closings.get() != 2 {
                finish(Err(format!(
                    "two windows closed, the opener was told {} times",
                    closings.get()
                )));
            }
            let late = Rc::clone(&closings);
            cx.update(|cx| by_itself.on_closed(cx, move |_| late.set(late.get() + 1)));
            match closings.get() {
                3 => finish(Ok(())),
                told => finish(Err(format!(
                    "asked after its window was gone, the opener was told {} times",
                    told - 2
                ))),
            }
        })
        .detach();
    });
    finish(Err(format!(
        "GPUI stopped before the test finished: {ran:?}"
    )));
}

fn open(cx: &mut App, closings: &Rc<Cell<u32>>) -> EditorWindow {
    let closings = Rc::clone(closings);
    let opened = cleanshot_editor::open(
        cx,
        RgbaImage::new(320, 240, vec![128; 320 * 240 * 4]).expect("320x240 RGBA"),
        None,
        Outputs::new(
            |_, _| Err(DeliveryError::ClipboardBusy),
            || Ok((PathBuf::new(), String::new())),
            |_, _| Err(DeliveryError::ClipboardBusy),
        ),
        |_| None,
        |_| {},
        |_| {},
    );
    let editor =
        opened.unwrap_or_else(|error| finish(Err(format!("the editor did not open: {error}"))));
    editor.on_closed(cx, move |_| closings.set(closings.get() + 1));
    editor
}

fn finish(verdict: Result<(), String>) -> ! {
    match verdict {
        Ok(()) => {
            println!("{TEST} ... ok");
            exit(0);
        }
        Err(failure) => {
            println!("{TEST} ... FAILED: {failure}");
            exit(1);
        }
    }
}
