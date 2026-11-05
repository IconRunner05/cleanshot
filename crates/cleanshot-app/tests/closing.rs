//! Closing Cleanshot's GPUI windows. Ctrl+W closes the editor and the settings window as
//! their caption's close button does (D-136). An editor tells its opener exactly once that its
//! window is gone, whether the app closed it with `EditorWindow::close` or the window closed
//! itself, and at once when asked after the window is gone. Runs real windows, so only on
//! Windows; the test ends the process.

use std::cell::Cell;
use std::path::PathBuf;
use std::process::exit;
use std::rc::Rc;
use std::time::Duration;

use cleanshot_core::{Applied, DeliveryError, Policy, RgbaImage, Settings};
use cleanshot_editor::{App, EditorWindow, Keystroke, Outputs};

const CTRL_W: &str = "d136_ctrl_w_closes_the_editor_and_settings_as_their_close_button_does";
const ON_CLOSED: &str = "an_editor_tells_its_opener_once_when_its_window_is_gone";
/// Long enough for the windows to open, draw and, once closed, be released.
const SETTLE: Duration = Duration::from_secs(2);

fn main() {
    let ran = cleanshot_editor::run(|cx| {
        let closings = Rc::new(Cell::new(0_u32));
        let by_the_app = open_editor(cx, &closings);
        let by_ctrl_w = open_editor(cx, &closings);
        if let Err(error) = open_settings(cx) {
            finish(
                CTRL_W,
                Err(format!("the settings window did not open: {error}")),
            );
        }
        cx.spawn(async move |cx| {
            cx.background_executor().timer(SETTLE).await;
            if closings.get() != 0 {
                let told = closings.get();
                finish(
                    ON_CLOSED,
                    Err(format!("told {told} times before any close")),
                );
            }
            let pressed = cx.update(|cx| {
                by_the_app.close(cx);
                cx.windows().into_iter().try_for_each(|window| {
                    window.update(cx, |_, window, cx| {
                        let ctrl_w = Keystroke::parse("ctrl-w").expect("ctrl-w is a keystroke");
                        window.dispatch_keystroke(ctrl_w, cx);
                    })
                })
            });
            if let Err(error) = pressed {
                finish(CTRL_W, Err(format!("Ctrl+W could not be pressed: {error}")));
            }
            cx.background_executor().timer(SETTLE).await;
            let left = cx.update(|cx| cx.windows().len());
            if left != 0 {
                finish(CTRL_W, Err(format!("{left} windows open after Ctrl+W")));
            }
            println!("{CTRL_W} ... ok");
            if closings.get() != 2 {
                let told = closings.get();
                finish(
                    ON_CLOSED,
                    Err(format!("two editors closed, told {told} times")),
                );
            }
            let late = Rc::clone(&closings);
            cx.update(|cx| by_ctrl_w.on_closed(cx, move |_| late.set(late.get() + 1)));
            match closings.get() {
                3 => finish(ON_CLOSED, Ok(())),
                told => finish(
                    ON_CLOSED,
                    Err(format!(
                        "asked after its window was gone, told {} times",
                        told - 2
                    )),
                ),
            }
        })
        .detach();
    });
    finish(
        ON_CLOSED,
        Err(format!("GPUI stopped before the test finished: {ran:?}")),
    );
}

fn open_editor(cx: &mut App, closings: &Rc<Cell<u32>>) -> EditorWindow {
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
    let editor = opened.unwrap_or_else(|error| {
        finish(ON_CLOSED, Err(format!("the editor did not open: {error}")))
    });
    editor.on_closed(cx, move |_| closings.set(closings.get() + 1));
    editor
}

fn open_settings(cx: &mut App) -> Result<(), cleanshot_editor::Error> {
    cleanshot_editor::open_settings(
        cx,
        Settings::default,
        Policy::default(),
        |_| Applied {
            saved: Ok(()),
            taken: Vec::new(),
        },
        false,
        |_| Ok(()),
        |_| {},
    )
    .map(|_| ())
}

fn finish(test: &str, verdict: Result<(), String>) -> ! {
    match verdict {
        Ok(()) => {
            println!("{test} ... ok");
            exit(0);
        }
        Err(failure) => {
            println!("{test} ... FAILED: {failure}");
            exit(1);
        }
    }
}
