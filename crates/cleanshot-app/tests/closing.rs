//! Closing Cleanshot's GPUI windows. Ctrl+W closes the editor and the settings window as
//! their caption's close button does (D-136). An editor tells its opener exactly once that its
//! window is gone, whether the app closed it with `EditorWindow::close` or the window closed
//! itself, and at once when asked after the window is gone. GPUI holds no graphics device
//! before the first window opens or once the last one closes, and takes one again for the next
//! (D-138), and tells the app each time it gives it back. Runs real windows, so only on
//! Windows; the test ends the process.

use std::cell::Cell;
use std::path::PathBuf;
use std::process::exit;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cleanshot_core::{Applied, DeliveryError, Policy, RgbaImage, Settings};
use cleanshot_editor::{App, AsyncApp, EditorWindow, Keystroke, Outputs};

const CTRL_W: &str = "d136_ctrl_w_closes_the_editor_and_settings_as_their_close_button_does";
const ON_CLOSED: &str = "an_editor_tells_its_opener_once_when_its_window_is_gone";
const GRAPHICS: &str =
    "d138_gpui_holds_a_graphics_device_only_while_a_window_is_open_and_a_reopened_editor_draws";
const RELEASED: &str = "d138_the_app_is_told_each_time_gpui_gives_its_graphics_device_back";
/// Long enough for the windows to open, draw and, once closed, be released.
const SETTLE: Duration = Duration::from_secs(2);

fn main() {
    let ran = cleanshot_editor::run(|cx| {
        if cx.graphical_environment().is_some() {
            finish(
                GRAPHICS,
                Err("a graphics device before any window opened".into()),
            );
        }
        let releases = Rc::new(Cell::new(0_u32));
        let counter = Rc::clone(&releases);
        cleanshot_editor::on_graphics_released(cx, move |_| counter.set(counter.get() + 1));
        cx.spawn(async move |cx| {
            let closings = Rc::new(Cell::new(0_u32));
            close_two_editors_and_settings(cx, &closings).await;
            graphics_released(cx, &releases, 1, "the last window");
            reopen_and_close(cx, &closings).await;
            graphics_released(cx, &releases, 2, "the reopened editor");
            println!("{GRAPHICS} ... ok");
            finish(RELEASED, Ok(()))
        })
        .detach();
    });
    finish(
        ON_CLOSED,
        Err(format!("GPUI stopped before the test finished: {ran:?}")),
    );
}

/// D-136 and task 18: one editor closed by the app, the other and the settings window by Ctrl+W.
async fn close_two_editors_and_settings(cx: &mut AsyncApp, closings: &Rc<Cell<u32>>) {
    let (by_the_app, by_ctrl_w) = cx.update(|cx| {
        let editors = (
            open_editor(cx, closings, |_| {}),
            open_editor(cx, closings, |_| {}),
        );
        if let Err(error) = open_settings(cx) {
            finish(
                CTRL_W,
                Err(format!("the settings window did not open: {error}")),
            );
        }
        editors
    });
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
    let late = Rc::clone(closings);
    cx.update(|cx| by_ctrl_w.on_closed(cx, move |_| late.set(late.get() + 1)));
    if closings.get() != 3 {
        let told = closings.get() - 2;
        finish(
            ON_CLOSED,
            Err(format!(
                "asked after its window was gone, told {told} times"
            )),
        );
    }
    println!("{ON_CLOSED} ... ok");
}

/// D-138: with every window closed GPUI holds no device, and the app has been told of
/// `expected` releases.
fn graphics_released(cx: &mut AsyncApp, releases: &Cell<u32>, expected: u32, closed: &str) {
    if cx.update(|cx| cx.graphical_environment().is_some()) {
        finish(
            GRAPHICS,
            Err(format!("the graphics device outlived {closed}")),
        );
    }
    let told = releases.get();
    if told != expected {
        finish(
            RELEASED,
            Err(format!("{expected} releases, the app told {told} times")),
        );
    }
}

/// D-138: an editor opened after GPUI gave its device back draws, and closes again.
async fn reopen_and_close(cx: &mut AsyncApp, closings: &Rc<Cell<u32>>) {
    let drawn = Rc::new(Cell::new(false));
    let first_frame = Rc::clone(&drawn);
    let reopened = cx.update(|cx| open_editor(cx, closings, move |_| first_frame.set(true)));
    cx.background_executor().timer(SETTLE).await;
    if !drawn.get() {
        finish(
            GRAPHICS,
            Err("the reopened editor drew no first frame".into()),
        );
    }
    cx.update(|cx| reopened.close(cx));
    cx.background_executor().timer(SETTLE).await;
}

fn open_editor(
    cx: &mut App,
    closings: &Rc<Cell<u32>>,
    on_first_frame: impl FnOnce(Instant) + 'static,
) -> EditorWindow {
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
        on_first_frame,
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
