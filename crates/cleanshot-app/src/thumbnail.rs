use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use cleanshot_core::{Document, Output, RgbaImage, Saving};
use cleanshot_win::capture::RECT;
use cleanshot_win::thumbnail::{self, Dragged, Thumbnail, ThumbnailEvent};
use cleanshot_win::tray::LastCaptureItems;

use crate::capture::{Defer, Handoff, PassOn};
use crate::delivery::{deliver, dragged};
use crate::{Session, Warning};

/// F-09: the thumbnail on screen and the capture it holds, dropped together when it closes
/// (D-94).
pub(super) struct Shown {
    thumbnail: Thumbnail,
    image: Arc<RgbaImage>,
    /// Which thumbnail this is, so an event from one already replaced does nothing.
    count: u64,
}

/// Shows the thumbnail of `image`, a capture just copied or saved from `selected`, when the
/// settings ask for it, replacing any thumbnail already shown (D-94, D-96).
pub(super) fn show(
    session: &Rc<Session>,
    image: Arc<RgbaImage>,
    selected: RECT,
    defer: &Rc<Defer>,
    pass_on: &Rc<PassOn>,
) {
    if !session.settings.borrow().show_thumbnail {
        return;
    }
    let (width, height) = match Thumbnail::largest_preview(selected) {
        Ok(size) => size,
        Err(error) => {
            return session.warn(Warning::Failed("The thumbnail could not be made", &error));
        }
    };
    let preview = match cleanshot_render::thumbnail(&image, width, height) {
        Ok(preview) => preview,
        Err(error) => {
            return session.warn(Warning::Failed("The thumbnail could not be made", &error));
        }
    };
    let saving_off = matches!(session.policy.saving(), Saving::Off(_));
    let saving = match session.policy.saving() {
        Saving::Off(off) => thumbnail::Saving::Off(*off),
        Saving::Allowed { .. } => thumbnail::Saving::On {
            drag: drag_out(Rc::downgrade(session), Arc::clone(&image)),
        },
    };
    let count = session.thumbnails_shown.get() + 1;
    session.thumbnails_shown.set(count);
    let (acting, defer, pass_on) = (Rc::downgrade(session), Rc::clone(defer), Rc::clone(pass_on));
    let shown = Thumbnail::show(&preview, selected, saving, move |event| {
        let (acting, pass_on) = (acting.clone(), Rc::clone(&pass_on));
        // The event arrives inside the thumbnail's window procedure, so it is acted on after.
        defer(
            Duration::ZERO,
            Box::new(move || {
                if let Some(session) = acting.upgrade() {
                    on_event(&session, count, event, &*pass_on);
                }
            }),
        );
    });
    match shown {
        Ok(thumbnail) => {
            *session.thumbnail.borrow_mut() = Some(Shown {
                thumbnail,
                image,
                count,
            });
            let items = LastCaptureItems::Shown { saving_off };
            session.tray.borrow().set_last_capture_items(items);
        }
        Err(error) => session.warn(Warning::Failed("The thumbnail could not open", &error)),
    }
}

/// Closes the thumbnail at once, before a new capture copies the screen.
pub(super) fn close_now(session: &Session) {
    drop(taken(session));
}

/// D-96: the tray's Edit last capture, which does what a click on the thumbnail does.
pub(super) fn edit_last(session: &Session, pass_on: &PassOn) {
    let Some(Shown { image, .. }) = taken(session) else {
        return;
    };
    pass_on(Arc::unwrap_or_clone(image), Handoff::Editor);
}

/// D-96: the tray's Save last capture, which does what the thumbnail's Save does.
pub(super) fn save_last(session: &Session) {
    let Some(Shown {
        thumbnail, image, ..
    }) = taken(session)
    else {
        return;
    };
    let saved = deliver(
        Output::Save,
        &Document::new(image),
        &session.saving_settings(),
        session.policy.saving(),
        &|event, error| session.log_failure("delivery", event, error),
    );
    fade(session, thumbnail);
    if let Err(error) = saved {
        session.warn(Warning::Refused(&error));
    }
}

fn on_event(session: &Session, count: u64, event: ThumbnailEvent, pass_on: &PassOn) {
    let current = session.thumbnail.borrow().as_ref().map(|shown| shown.count);
    if current != Some(count) {
        return;
    }
    match event {
        // F-13 (D-68): the editor takes over with no fade between them.
        ThumbnailEvent::Open => edit_last(session, pass_on),
        ThumbnailEvent::Save => save_last(session),
        ThumbnailEvent::Dismiss | ThumbnailEvent::TimedOut | ThumbnailEvent::Dropped => {
            if let Some(Shown { thumbnail, .. }) = taken(session) {
                fade(session, thumbnail);
            }
        }
        ThumbnailEvent::DragFailed(error) => {
            session.warn(Warning::Failed(
                "The capture could not be dragged out",
                &error,
            ));
        }
        ThumbnailEvent::DrawFailed(error) => {
            session.warn(Warning::Failed("The thumbnail could not draw", &error));
        }
    }
}

/// The shown thumbnail, taken from the session with the tray's items for it removed.
fn taken(session: &Session) -> Option<Shown> {
    let shown = session.thumbnail.borrow_mut().take()?;
    session
        .tray
        .borrow()
        .set_last_capture_items(LastCaptureItems::Hidden);
    Some(shown)
}

fn fade(session: &Session, thumbnail: Thumbnail) {
    if let Err(error) = thumbnail.close() {
        session.warn(Warning::Failed("The thumbnail could not fade out", &error));
    }
}

/// D-94: what a drag from the thumbnail offers, made as the drag starts; a failure is shown
/// and nothing drags.
fn drag_out(session: Weak<Session>, image: Arc<RgbaImage>) -> Box<dyn Fn() -> Option<Dragged>> {
    Box::new(move || {
        let session = session.upgrade()?;
        match dragged(&image, &|event, error| {
            session.log_failure("delivery", event, error);
        }) {
            Ok(dragged) => Some(dragged),
            Err(error) => {
                session.warn(Warning::Refused(&error));
                None
            }
        }
    })
}
