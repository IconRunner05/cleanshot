#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::rc::Rc;

use cleanshot_win::capture::{Method, capture_all};
use cleanshot_win::instance::Launch;
use cleanshot_win::overlay::{Overlay, OverlayEvent};
use cleanshot_win::tray::{Request, Tray, ask_running_instance_to_capture};

fn main() {
    cleanshot_win::load_dlls_from_system32_only().expect("the DLL search path is restricted");
    let instance = match Launch::claim().expect("the single-instance mutex opens") {
        Launch::First(instance) => instance,
        Launch::AlreadyRunning => {
            // C-14: a second launch asks the running one to capture, then exits. If that
            // one is still starting it has no tray window yet, and this launch is lost.
            if let Err(error) = ask_running_instance_to_capture() {
                eprintln!("Cleanshot is already running and could not be reached: {error}");
            }
            return;
        }
    };
    let tray = Rc::new(RefCell::new(None));
    let launched_tray = Rc::clone(&tray);
    let overlay: Rc<RefCell<Option<Overlay>>> = Rc::new(RefCell::new(None));
    cleanshot_editor::run(move |cx| {
        let app = cx.to_async();
        let overlay = Rc::clone(&overlay);
        let created = Tray::create(move |request| {
            let overlay = Rc::clone(&overlay);
            let closer = app.clone();
            // The tray's window procedure can run while GPUI holds the app, so the request
            // waits for GPUI's executor instead of borrowing the app here.
            app.spawn(async move |cx| {
                cx.update(|cx| match request {
                    Request::CaptureRegion => open_overlay(&overlay, move |job| {
                        closer.spawn(async move |_| job()).detach();
                    }),
                    Request::Exit => cx.quit(),
                });
            })
            .detach();
        })
        .expect("the tray window and icon are created");
        launched_tray.replace(Some(created));
    })
    .expect("the Windows platform starts");
    drop(tray);
    drop(instance);
}

// F-01, F-02: the screen is copied before any window of ours appears, then frozen on every
// monitor until Esc.
fn open_overlay(
    overlay: &Rc<RefCell<Option<Overlay>>>,
    defer: impl Fn(Box<dyn FnOnce()>) + 'static,
) {
    if overlay.borrow().is_some() {
        return;
    }
    let captures = match capture_all(Method::GdiBitBlt) {
        Ok(captures) => captures,
        Err(error) => return eprintln!("the screen could not be captured: {error}"),
    };
    let closing = Rc::clone(overlay);
    let shown = Overlay::show(&captures, move |event| {
        if let OverlayEvent::DrawFailed(error) = &event {
            eprintln!("the overlay could not draw: {error}");
        }
        // The event arrives inside an overlay window's procedure, so the windows close
        // after it returns.
        let closing = Rc::clone(&closing);
        defer(Box::new(move || drop(closing.borrow_mut().take())));
    });
    match shown {
        Ok(shown) => *overlay.borrow_mut() = Some(shown),
        Err(error) => eprintln!("the overlay could not open: {error}"),
    }
}
