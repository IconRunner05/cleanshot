use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cleanshot_core::{
    AfterCapture, CaptureMode, Delivered, DeliveryError, Document, Output, Pickable, Rect,
    RgbaImage, Saving,
};
use cleanshot_win::capture::{
    Desktop, MonitorImage, RECT, capture_desktop, draw_pointer, monitor_under_cursor,
    pickable_windows, region, screen_rect,
};
use cleanshot_win::overlay::{Opening, Overlay, OverlayEvent, Stages};

use crate::delivery::deliver;
use crate::{Session, Warning};
use crate::{settings_window, thumbnail};

/// Runs a job on GPUI's executor after a wait, outside any window procedure.
pub(super) type Defer = dyn Fn(Duration, Box<dyn FnOnce()>);
/// Hands an image on to the editor or the Save As dialog.
pub(super) type PassOn = dyn Fn(RgbaImage, Handoff);

/// Where the overlay hands a selection on to GPUI: the editor (F-05), or the Save As dialog
/// once the overlay has closed (F-22).
#[derive(Clone, Copy, Debug)]
pub(super) enum Handoff {
    Editor,
    SaveAs,
}

/// What follows an overlay event once the overlay has closed.
enum Next {
    Nothing,
    /// F-22: the Save As dialog for this image.
    SaveAs(RgbaImage),
    /// F-09: the thumbnail of a capture copied or saved from this selection.
    Thumbnail(Arc<RgbaImage>, RECT),
}

/// What a capture takes: a capture mode, from its hotkey or the capture panel (F-11), or the
/// last region (F-06).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shot {
    Mode(CaptureMode),
    LastRegion,
}

#[derive(Debug, PartialEq)]
enum Plan {
    Copy(RECT),
    Overlay(Option<RECT>),
    /// F-08: the overlay, picking a window.
    PickWindow,
}

/// What a capture of `shot` does once the screen is copied, or None when the pointer's
/// monitor is unknown. A region capture preselects `remembered` (F-12); F-07 follows
/// `after_capture` (D-34); F-06 copies the last selection with no overlay, or opens one when
/// there is none yet (D-35).
fn plan(
    shot: Shot,
    after_capture: AfterCapture,
    last_selection: Option<RECT>,
    remembered: Option<RECT>,
    pointer_monitor: impl FnOnce() -> Option<RECT>,
) -> Option<Plan> {
    Some(match (shot, after_capture) {
        (Shot::Mode(CaptureMode::Region), _) => Plan::Overlay(remembered),
        (Shot::Mode(CaptureMode::FullScreen), AfterCapture::Copy) => Plan::Copy(pointer_monitor()?),
        (Shot::Mode(CaptureMode::FullScreen), AfterCapture::ShowToolbar) => {
            Plan::Overlay(Some(pointer_monitor()?))
        }
        (Shot::LastRegion, _) => last_selection.map_or(Plan::Overlay(None), Plan::Copy),
        (Shot::Mode(CaptureMode::Window), _) => Plan::PickWindow,
    })
}

/// F-12 Remember last selection: the last selection, when the setting is on and every pixel
/// of it is still on one of `monitors`.
fn remembered(
    remember: bool,
    last_selection: Option<RECT>,
    monitors: impl IntoIterator<Item = RECT>,
) -> Option<RECT> {
    let area = |rect: Rect| i64::from(rect.right - rect.left) * i64::from(rect.bottom - rect.top);
    // Windows reports a mirrored display as one monitor, so no two overlap and the sum is
    // the area covered.
    let covered = |selection: Rect| {
        monitors
            .into_iter()
            .filter_map(|monitor| selection.overlap(screen_rect(monitor)))
            .map(area)
            .sum::<i64>()
    };
    last_selection.filter(|&selection| {
        remember && covered(screen_rect(selection)) == area(screen_rect(selection))
    })
}

// F-01, F-02: the screen is copied before any window of ours appears, then frozen on every
// monitor until Esc. F-07: a screen capture selects the pointer's monitor, then runs the
// after-capture action. F-06: the last region is copied with no overlay; with none yet, the
// overlay opens as for a region.
pub(super) fn capture(
    session: &Rc<Session>,
    shot: Shot,
    asked: Instant,
    defer: impl Fn(Duration, Box<dyn FnOnce()>) + 'static,
    pass_on: impl Fn(RgbaImage, Handoff) + 'static,
) {
    if session.overlay.borrow().is_some() {
        return;
    }
    // The thumbnail is topmost, so it would be in the copy of the screen. The capture panel
    // and countdown are left out of every copy (D-106), and close here too, so another
    // hotkey's capture never relies on that alone.
    thumbnail::close_now(session);
    session.panel.borrow_mut().take();
    session.countdown.borrow_mut().take();
    session.captures_started.update(|started| started + 1);
    let stage = |name: &str| {
        session.perf(|_| format!("P-01 {name}_ms={}", asked.elapsed().as_millis()));
    };
    stage("started");
    let defer: Rc<Defer> = Rc::new(defer);
    let pass_on: Rc<PassOn> = Rc::new(pass_on);
    let Some(Desktop {
        monitors: captures,
        copied,
        adapters,
        outputs,
    }) = copy_screen(session)
    else {
        return;
    };
    stage("screen_copied");
    let under_pointer = || match monitor_under_cursor(&captures) {
        Ok(monitor) => Some(monitor),
        Err(error) => {
            session.warn(Warning::Failed(
                "Cleanshot could not tell which screen the pointer is on",
                &error,
            ));
            None
        }
    };
    let last = session.last_selection.get();
    let remembered = remembered(
        session.settings.borrow().remember_last_selection,
        last,
        captures.iter().map(|capture| capture.bounds),
    );
    let (selected, picking) = match plan(
        shot,
        session.settings.borrow().after_capture,
        last,
        remembered,
        under_pointer,
    ) {
        None => {
            session.log_capture(&copied, &adapters, &outputs, &captures);
            return session.sample_after_use(&*defer);
        }
        Some(Plan::Overlay(selected)) => (selected, false),
        Some(Plan::PickWindow) => (None, true),
        Some(Plan::Copy(selected)) => {
            if let Some(image) = finish(session, &captures, selected, Output::Copy) {
                thumbnail::show(session, image, selected, &defer, &pass_on);
            }
            session.log_capture(&copied, &adapters, &outputs, &captures);
            return session.sample_after_use(&*defer);
        }
    };
    // F-08: listed before the overlay opens, so the list never holds it.
    let pickable = listed_windows(session, &captures);
    stage("windows_listed");
    let captures = Rc::new(captures);
    let frozen = Rc::clone(&captures);
    let saving_off = match session.policy.saving() {
        Saving::Off(off) => Some(*off),
        Saving::Allowed { .. } => None,
    };
    let options = settings_window::options_for(session);
    let shown = Overlay::show(
        &captures,
        Opening {
            selected,
            pickable,
            saving_off,
        },
        options,
        overlay_listener(session, frozen, defer, pass_on),
    );
    match shown {
        Ok(shown) => {
            if picking {
                shown.pick_window();
            }
            let opened = opening_line(shown.stages());
            *session.overlay.borrow_mut() = Some(shown);
            stage("overlay_visible");
            session.log_line("capture", |_| opened);
        }
        Err(error) => session.warn(Warning::Failed("The overlay could not open", &error)),
    }
    // P-01: the black-share pass runs after the overlay is visible.
    session.log_capture(&copied, &adapters, &outputs, &captures);
}

/// P-01: each step of opening the overlay and when it ended, in ms from the start of opening.
fn opening_line(stages: &Stages) -> String {
    let marks: Vec<String> = stages
        .marks()
        .iter()
        .map(|(stage, at)| format!("{stage}={:.1}", at.as_secs_f64() * 1000.0))
        .collect();
    format!("overlay opened_ms {}", marks.join(" "))
}

/// What the overlay's events do: an Options pick applies and leaves it open (F-12); every
/// other event closes it once its window procedure has returned.
fn overlay_listener(
    session: &Rc<Session>,
    frozen: Rc<Vec<MonitorImage>>,
    defer: Rc<Defer>,
    pass_on: Rc<PassOn>,
) -> impl FnMut(OverlayEvent) + 'static {
    let finishing = Rc::clone(session);
    move |event| {
        // F-12: a pick on the Options menu leaves the overlay open.
        if let OverlayEvent::Options(item) = event {
            let overlay = finishing.overlay.borrow();
            let folder = || overlay.as_ref().map_or(Ok(None), Overlay::choose_folder);
            return settings_window::apply_option(&finishing, item, folder);
        }
        // F-13 (D-68): the editor takes over from the overlay with no fade between them.
        let handoff = matches!(event, OverlayEvent::Edit(_));
        let next = on_overlay_event(&finishing, &frozen, event, &*pass_on);
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
                        closing.warn(Warning::Failed("The overlay could not fade out", &error));
                    }
                }
                match next {
                    Next::Nothing => {}
                    Next::SaveAs(image) => pass_on(image, Handoff::SaveAs),
                    Next::Thumbnail(image, selected) => {
                        thumbnail::show(&closing, image, selected, &sampling, &pass_on);
                    }
                }
                closing.sample_after_use(&*sampling);
            }),
        );
    }
}

/// Copies every monitor, with the pointer drawn in when Show pointer is on (F-12), so the
/// overlay shows what is copied or saved. Warns and gives nothing if the copy fails.
fn copy_screen(session: &Session) -> Option<Desktop> {
    let mut desktop = match capture_desktop() {
        Ok(desktop) => desktop,
        Err(error) => {
            session.warn(Warning::Failed("The screen could not be captured", &error));
            return None;
        }
    };
    if session.settings.borrow().show_pointer
        && let Err(error) = draw_pointer(&mut desktop.monitors)
    {
        session.warn(Warning::Failed(
            "The pointer could not be drawn into the capture",
            &error,
        ));
    }
    Some(desktop)
}

/// The windows a window capture can pick from `captures`, with how many and how long the
/// listing took logged; a failed listing leaves only the monitors to pick (D-100).
fn listed_windows(session: &Session, captures: &[MonitorImage]) -> Vec<Pickable> {
    let listing = Instant::now();
    let pickable = match pickable_windows(captures) {
        Ok(pickable) => pickable,
        Err(error) => {
            session.log_line("capture", |_| format!("windows not_listed {error}"));
            Vec::new()
        }
    };
    session.log_line("capture", |_| {
        format!(
            "windows listed={} ms={}",
            pickable.len(),
            listing.elapsed().as_millis()
        )
    });
    pickable
}

/// What the overlay's `event` does as it arrives: copy, save or edit the selection from
/// `frozen`, and what follows once the overlay has closed.
fn on_overlay_event(
    session: &Session,
    frozen: &[MonitorImage],
    event: OverlayEvent,
    pass_on: &PassOn,
) -> Next {
    match event {
        OverlayEvent::Cancelled => {}
        OverlayEvent::Copy(selected) => {
            session.last_selection.set(Some(selected));
            if let Some(image) = finish(session, frozen, selected, Output::Copy) {
                return Next::Thumbnail(image, selected);
            }
        }
        OverlayEvent::Save(selected) => {
            session.last_selection.set(Some(selected));
            if let Some(image) = finish(session, frozen, selected, Output::Save) {
                return Next::Thumbnail(image, selected);
            }
        }
        OverlayEvent::SaveAs(selected) => {
            session.last_selection.set(Some(selected));
            // F-57: no Save As dialog opens when the policy turns saving off.
            if let Saving::Off(off) = session.policy.saving() {
                session.warn(Warning::Refused(&DeliveryError::SavingOff(*off)));
                return Next::Nothing;
            }
            match region(frozen, selected) {
                Ok(image) => return Next::SaveAs(image),
                Err(error) => session.warn(Warning::Failed("The capture did not finish", &error)),
            }
        }
        OverlayEvent::Edit(selected) => match region(frozen, selected) {
            Ok(image) => pass_on(image, Handoff::Editor),
            Err(error) => session.warn(Warning::Failed("The capture could not be edited", &error)),
        },
        OverlayEvent::DrawFailed(error) => {
            session.warn(Warning::Failed("The overlay could not draw", &error));
        }
        OverlayEvent::Options(_) => unreachable!("the overlay's listener applies Options picks"),
        OverlayEvent::OptionsFailed(error) => {
            session.warn(Warning::Failed("The Options menu could not open", &error));
        }
    }
    Next::Nothing
}

// The selection becomes a document with nothing drawn on it, so the overlay delivers through
// the same path as the editor. The image delivered, for the thumbnail (F-09).
fn finish(
    session: &Session,
    captures: &[MonitorImage],
    selected: RECT,
    output: Output,
) -> Option<Arc<RgbaImage>> {
    let started = Instant::now();
    let image = match region(captures, selected) {
        Ok(image) => Arc::new(image),
        Err(error) => {
            session.warn(Warning::Failed("The capture did not finish", &error));
            return None;
        }
    };
    match deliver(
        output,
        &Document::new(Arc::clone(&image)),
        &session.saving_settings(),
        session.policy.saving(),
        &|event, error| session.log_failure("delivery", event, error),
    ) {
        Ok(Delivered::Copied) => {
            session.perf(|_| format!("P-02 copied_ms={}", started.elapsed().as_millis()));
        }
        Ok(Delivered::Saved(_)) => {}
        Err(error) => {
            session.warn(Warning::Refused(&error));
            return None;
        }
    }
    Some(image)
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
            plan(
                Shot::Mode(CaptureMode::FullScreen),
                AfterCapture::Copy,
                None,
                None,
                monitor
            ),
            Some(Plan::Copy(MONITOR))
        );
        assert_eq!(
            plan(
                Shot::Mode(CaptureMode::FullScreen),
                AfterCapture::ShowToolbar,
                None,
                None,
                monitor
            ),
            Some(Plan::Overlay(Some(MONITOR)))
        );
        assert_eq!(
            plan(
                Shot::Mode(CaptureMode::FullScreen),
                AfterCapture::Copy,
                None,
                None,
                || None
            ),
            None
        );
    }

    #[test]
    fn f08_the_window_hotkey_opens_the_overlay_picking_a_window_whatever_was_last() {
        for after in [AfterCapture::Copy, AfterCapture::ShowToolbar] {
            assert_eq!(
                plan(
                    Shot::Mode(CaptureMode::Window),
                    after,
                    Some(LAST),
                    Some(LAST),
                    || Some(MONITOR)
                ),
                Some(Plan::PickWindow)
            );
        }
    }

    #[test]
    fn f06_last_region_copies_the_last_selection_or_opens_the_overlay_without_one() {
        let monitor = || Some(MONITOR);
        for after in [AfterCapture::Copy, AfterCapture::ShowToolbar] {
            assert_eq!(
                plan(Shot::LastRegion, after, Some(LAST), None, monitor),
                Some(Plan::Copy(LAST))
            );
            assert_eq!(
                plan(Shot::LastRegion, after, None, None, monitor),
                Some(Plan::Overlay(None))
            );
        }
        assert_eq!(
            plan(
                Shot::Mode(CaptureMode::Region),
                AfterCapture::Copy,
                Some(LAST),
                None,
                monitor
            ),
            Some(Plan::Overlay(None))
        );
    }

    #[test]
    fn f12_a_region_capture_opens_with_the_remembered_selection_preselected() {
        for after in [AfterCapture::Copy, AfterCapture::ShowToolbar] {
            assert_eq!(
                plan(
                    Shot::Mode(CaptureMode::Region),
                    after,
                    None,
                    Some(LAST),
                    || None
                ),
                Some(Plan::Overlay(Some(LAST)))
            );
        }
    }

    #[test]
    fn f12_the_last_selection_is_remembered_only_when_on_and_still_on_the_monitors() {
        let right = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let across = RECT {
            left: -100,
            top: 100,
            right: 100,
            bottom: 200,
        };
        assert_eq!(
            remembered(true, Some(across), [MONITOR, right]),
            Some(across)
        );
        assert_eq!(remembered(false, Some(across), [MONITOR, right]), None);
        assert_eq!(remembered(true, None, [MONITOR, right]), None);
        assert_eq!(
            remembered(true, Some(across), [right]),
            None,
            "half of it was on a monitor no longer there"
        );
        let below = RECT {
            top: 1000,
            bottom: 1200,
            ..across
        };
        assert_eq!(remembered(true, Some(below), [MONITOR, right]), None);
    }
}
