#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::rc::Rc;

use cleanshot_win::tray::{Request, Tray};

fn main() {
    let tray = Rc::new(RefCell::new(None));
    let launched_tray = Rc::clone(&tray);
    cleanshot_editor::run(move |cx| {
        let app = cx.to_async();
        let created = Tray::create(move |request| {
            // The tray's window procedure can run while GPUI holds the app, so the request
            // waits for GPUI's executor instead of borrowing the app here.
            app.spawn(async move |cx| {
                cx.update(|cx| match request {
                    // F-01: region capture arrives with the capture and overlay commits.
                    Request::CaptureRegion => {}
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
}
