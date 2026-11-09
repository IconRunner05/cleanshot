//! Closing Cleanshot's GPUI windows. Ctrl+W closes the editor and the settings window as
//! their caption's close button does. An editor tells its opener exactly once that its
//! window is gone, whether the app closed it with `EditorWindow::close` or the window closed
//! itself, and at once when asked after the window is gone. GPUI holds no graphics device
//! before the first window opens or once the last one closes, and takes one again for the next,
//! and tells the app each time it gives it back. An editor closed by Windows, as its
//! close button does, gives its memory back. Runs real windows, so only on Windows; the
//! test ends the process.

#[expect(dead_code, reason = "each test uses part of the harness")]
mod on_screen;

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cleanshot_core::{Applied, Policy, RgbaImage, Settings};
use cleanshot_editor::{App, AsyncApp, EditorWindow, Keystroke};
use on_screen::{finish, grey};

const CTRL_W: &str = "ctrl_w_closes_the_editor_and_settings_as_their_close_button_does";
const ON_CLOSED: &str = "an_editor_tells_its_opener_once_when_its_window_is_gone";
const GRAPHICS: &str =
    "gpui_holds_a_graphics_device_only_while_a_window_is_open_and_a_reopened_editor_draws";
const RELEASED: &str = "the_app_is_told_each_time_gpui_gives_its_graphics_device_back";
const USER_CLOSE: &str = "an_editor_closed_as_a_user_does_gives_its_memory_back";
/// What a closed editor may leave held: well under the 19.8 MB its 3440x1440 image's texture
/// takes, which on the software renderer is the process's own memory.
const LEFT_HELD: usize = 8 * 1024 * 1024;
/// Long enough for the windows to open, draw and, once closed, be released.
const SETTLE: Duration = Duration::from_secs(2);
/// Long enough for GPUI to free a closed window's renderer, which took up to 5 s on
/// windows-latest.
const FREED: Duration = Duration::from_secs(10);

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
            close_as_a_user_does(cx, &closings).await;
            graphics_released(cx, &releases, 3, "a user-closed editor");
            println!("{RELEASED} ... ok");
            finish(USER_CLOSE, Ok(()))
        })
        .detach();
    });
    finish(
        ON_CLOSED,
        Err(format!("GPUI stopped before the test finished: {ran:?}")),
    );
}

/// One editor closed by the app, the other and the settings window by Ctrl+W.
async fn close_two_editors_and_settings(cx: &mut AsyncApp, closings: &Rc<Cell<u32>>) {
    let (by_the_app, by_ctrl_w) = cx.update(|cx| {
        let editors = (
            open_editor(cx, closings, grey(320, 240), |_| {}),
            open_editor(cx, closings, grey(320, 240), |_| {}),
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

/// With every window closed GPUI holds no device, and the app has been told of
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

/// An editor opened after GPUI gave its device back draws, and closes again.
async fn reopen_and_close(cx: &mut AsyncApp, closings: &Rc<Cell<u32>>) {
    let drawn = Rc::new(Cell::new(false));
    let first_frame = Rc::clone(&drawn);
    let reopened =
        cx.update(|cx| open_editor(cx, closings, grey(320, 240), move |_| first_frame.set(true)));
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

/// An editor on a monitor-sized image, closed with `WM_CLOSE` as its close button and
/// Alt+F4 do, leaves the process holding about what it held before the editor opened.
async fn close_as_a_user_does(cx: &mut AsyncApp, closings: &Rc<Cell<u32>>) {
    cx.background_executor().timer(FREED).await;
    let before = private_bytes();
    let editor = cx.update(|cx| open_editor(cx, closings, grey(3440, 1440), |_| {}));
    cx.background_executor().timer(SETTLE).await;
    if let Err(error) = cleanshot_win::accessibility::ask_to_close(editor.win32_handle()) {
        finish(USER_CLOSE, Err(format!("WM_CLOSE was not posted: {error}")));
    }
    cx.background_executor().timer(FREED).await;
    let left = cx.update(|cx| cx.windows().len());
    if left != 0 {
        finish(
            USER_CLOSE,
            Err(format!("{left} windows open after WM_CLOSE")),
        );
    }
    let after = private_bytes();
    println!(
        "{USER_CLOSE}: private working set {before} before the editor opened, {after} after it closed"
    );
    if after > before + LEFT_HELD {
        finish(
            USER_CLOSE,
            Err(format!(
                "the closed editor left {} bytes held",
                after - before
            )),
        );
    }
}

fn private_bytes() -> usize {
    cleanshot_win::log::usage()
        .expect("Windows gives this process its memory counters")
        .private_working_set_bytes
}

fn open_editor(
    cx: &mut App,
    closings: &Rc<Cell<u32>>,
    image: RgbaImage,
    on_first_frame: impl FnOnce(Instant) + 'static,
) -> EditorWindow {
    let closings = Rc::clone(closings);
    let editor = on_screen::open_editor(cx, image, on_first_frame).unwrap_or_else(|error| {
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
