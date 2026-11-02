use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cleanshot_core::{
    AfterCapture, Delivered, Document, HotkeyAction, Output, RgbaImage, delivery_notice,
};
use cleanshot_win::capture::{
    Desktop, MonitorImage, RECT, capture_desktop, monitor_under_cursor, region,
};
use cleanshot_win::overlay::{Overlay, OverlayEvent};

use crate::Session;
use crate::delivery::deliver;

/// Where the overlay hands a selection on to GPUI: the editor (F-05), or the Save As dialog
/// once the overlay has closed (F-22).
#[derive(Clone, Copy, Debug)]
pub(super) enum Handoff {
    Editor,
    SaveAs,
}

#[derive(Debug, PartialEq)]
enum Plan {
    Copy(RECT),
    Overlay(Option<RECT>),
}

/// What a capture of `action` does once the screen is copied, or None when the pointer's
/// monitor is unknown. F-07 follows `after_capture` (D-34); F-06 copies the last selection
/// with no overlay, or opens one when there is none yet (D-35).
fn plan(
    action: HotkeyAction,
    after_capture: AfterCapture,
    last_selection: Option<RECT>,
    pointer_monitor: impl FnOnce() -> Option<RECT>,
) -> Option<Plan> {
    Some(match (action, after_capture) {
        (HotkeyAction::Region, _) => Plan::Overlay(None),
        (HotkeyAction::FullScreen, AfterCapture::Copy) => Plan::Copy(pointer_monitor()?),
        (HotkeyAction::FullScreen, AfterCapture::ShowToolbar) => {
            Plan::Overlay(Some(pointer_monitor()?))
        }
        (HotkeyAction::LastRegion, _) => last_selection.map_or(Plan::Overlay(None), Plan::Copy),
    })
}

// F-01, F-02: the screen is copied before any window of ours appears, then frozen on every
// monitor until Esc. F-07: a screen capture selects the pointer's monitor, then runs the
// after-capture action. F-06: the last region is copied with no overlay; with none yet, the
// overlay opens as for a region.
pub(super) fn capture(
    session: &Rc<Session>,
    action: HotkeyAction,
    asked: Instant,
    defer: impl Fn(Duration, Box<dyn FnOnce()>) + 'static,
    pass_on: impl Fn(RgbaImage, Handoff) + 'static,
) {
    if session.overlay.borrow().is_some() {
        return;
    }
    session.captures_started.update(|started| started + 1);
    let Desktop {
        monitors: captures,
        copied,
        outputs,
    } = match capture_desktop() {
        Ok(desktop) => desktop,
        Err(error) => return session.warn(&format!("The screen could not be captured: {error}.")),
    };
    let under_pointer = || match monitor_under_cursor(&captures) {
        Ok(monitor) => Some(monitor),
        Err(error) => {
            session.warn(&format!(
                "Cleanshot could not tell which screen the pointer is on: {error}."
            ));
            None
        }
    };
    let last = session.last_selection.get();
    let selected = match plan(action, session.settings.after_capture, last, under_pointer) {
        None => {
            session.log_capture(&copied, &outputs, &captures);
            return session.sample_after_use(&defer);
        }
        Some(Plan::Overlay(selected)) => selected,
        Some(Plan::Copy(selected)) => {
            finish(session, &captures, selected, Output::Copy);
            session.log_capture(&copied, &outputs, &captures);
            return session.sample_after_use(&defer);
        }
    };
    let defer = Rc::new(defer);
    let captures = Rc::new(captures);
    let frozen = Rc::clone(&captures);
    let finishing = Rc::clone(session);
    let pass_on = Rc::new(pass_on);
    let shown = Overlay::show(&captures, selected, move |event| {
        // F-13 (D-68): the editor takes over from the overlay with no fade between them.
        let handoff = matches!(event, OverlayEvent::Edit(_));
        let saving_as = on_overlay_event(&finishing, &frozen, event, &*pass_on);
        // The event arrives inside an overlay window's procedure, so the windows close
        // after it returns, and the Save As dialog opens over the desktop, not the overlay.
        let closing = Rc::clone(&finishing);
        let pass_on = Rc::clone(&pass_on);
        let sampling = Rc::clone(&defer);
        defer(
            Duration::ZERO,
            Box::new(move || {
                let overlay = closing.overlay.borrow_mut().take();
                if let Some(overlay) = overlay {
                    if handoff {
                        drop(overlay);
                    } else if let Err(error) = overlay.close() {
                        closing.warn(&format!("The overlay could not fade out: {error}."));
                    }
                }
                if let Some(image) = saving_as {
                    pass_on(image, Handoff::SaveAs);
                }
                closing.sample_after_use(&*sampling);
            }),
        );
    });
    match shown {
        Ok(shown) => {
            *session.overlay.borrow_mut() = Some(shown);
            session.perf(|_| format!("P-01 overlay_visible_ms={}", asked.elapsed().as_millis()));
        }
        Err(error) => session.warn(&format!("The overlay could not open: {error}.")),
    }
    // P-01: the black-share pass runs after the overlay is visible.
    session.log_capture(&copied, &outputs, &captures);
}

/// What the overlay's `event` does as it arrives: copy, save or edit the selection from
/// `frozen`. For Save As, the image to save once the overlay has closed.
fn on_overlay_event(
    session: &Session,
    frozen: &[MonitorImage],
    event: OverlayEvent,
    pass_on: &dyn Fn(RgbaImage, Handoff),
) -> Option<RgbaImage> {
    match event {
        OverlayEvent::Cancelled => {}
        OverlayEvent::Copy(selected) => {
            session.last_selection.set(Some(selected));
            finish(session, frozen, selected, Output::Copy);
        }
        OverlayEvent::Save(selected) => {
            session.last_selection.set(Some(selected));
            finish(session, frozen, selected, Output::Save);
        }
        OverlayEvent::SaveAs(selected) => {
            session.last_selection.set(Some(selected));
            match region(frozen, selected) {
                Ok(image) => return Some(image),
                Err(error) => session.warn(&format!("The capture did not finish: {error}.")),
            }
        }
        OverlayEvent::Edit(selected) => match region(frozen, selected) {
            Ok(image) => pass_on(image, Handoff::Editor),
            Err(error) => session.warn(&format!("The capture could not be edited: {error}.")),
        },
        OverlayEvent::DrawFailed(error) => {
            session.warn(&format!("The overlay could not draw: {error}."));
        }
    }
    None
}

// The selection becomes a document with nothing drawn on it, so the overlay delivers through
// the same path as the editor.
fn finish(session: &Session, captures: &[MonitorImage], selected: RECT, output: Output) {
    let started = Instant::now();
    let image = match region(captures, selected) {
        Ok(image) => image,
        Err(error) => return session.warn(&format!("The capture did not finish: {error}.")),
    };
    match deliver(output, &Document::new(Arc::new(image)), &session.settings) {
        Ok(Delivered::Copied) => {
            session.perf(|_| format!("P-02 copied_ms={}", started.elapsed().as_millis()));
        }
        Ok(Delivered::Saved(_)) => {}
        Err(error) => session.warn(&delivery_notice(&Err(error))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: RECT = RECT {
        left: -1920,
        top: 0,
        right: 0,
        bottom: 1080,
    };
    const LAST: RECT = RECT {
        left: 10,
        top: 20,
        right: 110,
        bottom: 220,
    };

    #[test]
    fn f07_full_screen_copies_or_preselects_the_pointer_monitor_by_after_capture() {
        let monitor = || Some(MONITOR);
        assert_eq!(
            plan(HotkeyAction::FullScreen, AfterCapture::Copy, None, monitor),
            Some(Plan::Copy(MONITOR))
        );
        assert_eq!(
            plan(
                HotkeyAction::FullScreen,
                AfterCapture::ShowToolbar,
                None,
                monitor
            ),
            Some(Plan::Overlay(Some(MONITOR)))
        );
        assert_eq!(
            plan(HotkeyAction::FullScreen, AfterCapture::Copy, None, || None),
            None
        );
    }

    #[test]
    fn f06_last_region_copies_the_last_selection_or_opens_the_overlay_without_one() {
        let monitor = || Some(MONITOR);
        for after in [AfterCapture::Copy, AfterCapture::ShowToolbar] {
            assert_eq!(
                plan(HotkeyAction::LastRegion, after, Some(LAST), monitor),
                Some(Plan::Copy(LAST))
            );
            assert_eq!(
                plan(HotkeyAction::LastRegion, after, None, monitor),
                Some(Plan::Overlay(None))
            );
        }
        assert_eq!(
            plan(
                HotkeyAction::Region,
                AfterCapture::Copy,
                Some(LAST),
                monitor
            ),
            Some(Plan::Overlay(None))
        );
    }
}
