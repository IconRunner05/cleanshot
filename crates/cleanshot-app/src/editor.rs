use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use cleanshot_core::{Delivered, DeliveryError, Document, Output, RgbaImage};
use cleanshot_win::capture::{RECT, monitor_of};

use crate::delivery::{deliver, save_as_start, save_to};
use crate::{Session, Warning, to_front};

/// Opens `image`, captured from `from`, in the editor on that monitor, or says in
/// the tray why it could not. A monitor Windows cannot describe leaves the primary display.
pub(super) fn open_editor(
    cx: &mut cleanshot_editor::App,
    image: RgbaImage,
    from: RECT,
    session: &Rc<Session>,
) {
    let monitor = monitor_of(from)
        .inspect_err(|error| session.log_failure("editor", "monitor_unread", error))
        .ok();
    let opened = cleanshot_editor::open(
        cx,
        image,
        monitor,
        cleanshot_editor::Outputs::new(
            editor_deliver(session),
            editor_save_as_start(session),
            editor_save_to(session),
        ),
        {
            let logging = Rc::clone(session);
            move |document: &Document| {
                cleanshot_win::text::masks(document)
                    .inspect_err(|error| logging.log_failure("editor", "text_masks_failed", error))
                    .ok()
            }
        },
        editor_first_frame(session),
        {
            let logging = Rc::clone(session);
            move |event| logging.log_line("editor", |_| event.to_owned())
        },
    );
    match opened {
        Ok(editor) => {
            if session.log.borrow().is_some() {
                let sampling = Rc::clone(session);
                editor.on_closed(cx, move |cx| sampling.sample_after_editor(cx));
            }
            to_front(session, "editor", editor.win32_handle());
        }
        Err(error) => session.warn(Warning::Failed("The editor could not open", &error)),
    }
}

/// The editor's Copy and Save deliver as the overlay does.
fn editor_deliver(
    session: &Rc<Session>,
) -> impl Fn(Output, &Document) -> Result<Delivered, DeliveryError> + 'static {
    let session = Rc::clone(session);
    move |output, document| {
        deliver(
            output,
            document,
            &session.saving_settings(),
            session.policy.saving(),
            &|event, error| session.log_failure("delivery", event, error),
        )
    }
}

fn editor_save_as_start(
    session: &Rc<Session>,
) -> impl Fn() -> Result<(PathBuf, String), DeliveryError> + 'static {
    let session = Rc::clone(session);
    move || save_as_start(&session.saving_settings(), session.policy.saving())
}

fn editor_save_to(
    session: &Rc<Session>,
) -> impl Fn(&Path, &Document) -> Result<Delivered, DeliveryError> + 'static {
    let session = Rc::clone(session);
    move |path, document| {
        save_to(
            path,
            document,
            &session.saving_settings(),
            session.policy.saving(),
            &|event, error| session.log_failure("delivery", event, error),
        )
    }
}

/// Logs the time from now, as the editor opens, to the frame after its first draw.
fn editor_first_frame(session: &Rc<Session>) -> impl FnOnce(Instant) + 'static {
    let session = Rc::clone(session);
    let opened = Instant::now();
    move |shown| {
        session.perf(|_| {
            format!(
                "editor_open first_frame_ms={}",
                (shown - opened).as_millis()
            )
        });
    }
}
