use std::rc::Rc;
use std::time::{Duration, Instant};

use cleanshot_core::{PanelOutcome, RgbaImage, Timer};
use cleanshot_win::panel::{Countdown, CountdownEvent, Panel, PanelEvent};

use crate::Session;
use crate::capture::{Defer, Handoff, PassOn, Shot, capture};

/// F-11: shows the capture panel with the session's mode and timer (D-99), or brings it
/// forward when it already shows. A capture or countdown under way leaves it shut.
pub(super) fn open(
    session: &Rc<Session>,
    defer: impl Fn(Duration, Box<dyn FnOnce()>) + 'static,
    pass_on: impl Fn(RgbaImage, Handoff) + 'static,
) {
    if let Some(panel) = &*session.panel.borrow() {
        return panel.bring_forward();
    }
    if session.overlay.borrow().is_some() || session.countdown.borrow().is_some() {
        return;
    }
    let defer: Rc<Defer> = Rc::new(defer);
    let pass_on: Rc<PassOn> = Rc::new(pass_on);
    let closing = Rc::clone(session);
    let shown = Panel::show(session.capture_panel.get(), move |event| match event {
        PanelEvent::Closed { panel, outcome } => {
            closing.capture_panel.set(panel);
            // The event arrives inside the panel's window procedure, so the window goes, and
            // the countdown or capture starts, after it returns.
            let after = Rc::clone(&closing);
            let defer_again = Rc::clone(&defer);
            let pass_on = Rc::clone(&pass_on);
            defer(
                Duration::ZERO,
                Box::new(move || {
                    after.panel.borrow_mut().take();
                    match outcome {
                        PanelOutcome::Capture { mode, timer } => {
                            wait(&after, Shot::Mode(mode), timer, defer_again, pass_on);
                        }
                        PanelOutcome::Stay | PanelOutcome::Cancel => {}
                    }
                }),
            );
        }
        PanelEvent::DrawFailed(error) => {
            closing.warn(&format!("The capture panel could not be drawn: {error}."));
        }
    });
    match shown {
        Ok(shown) => *session.panel.borrow_mut() = Some(shown),
        Err(error) => session.warn(&format!("The capture panel could not open: {error}.")),
    }
}

/// F-10: captures `shot` once `timer` has counted down, or at once when it is Off.
fn wait(session: &Rc<Session>, shot: Shot, timer: Timer, defer: Rc<Defer>, pass_on: Rc<PassOn>) {
    if timer == Timer::Off {
        return start(session, shot, &defer, &pass_on);
    }
    let ending = Rc::clone(session);
    let counted = Countdown::start(timer, move |event| {
        if let CountdownEvent::DrawFailed(error) = &event {
            return ending.warn(&format!("The countdown could not be drawn: {error}."));
        }
        let ended = Rc::clone(&ending);
        let defer_again = Rc::clone(&defer);
        let pass_on = Rc::clone(&pass_on);
        // The event arrives inside the countdown's window procedure, so the window goes, and
        // the capture starts, after it returns.
        defer(
            Duration::ZERO,
            Box::new(move || {
                ended.countdown.borrow_mut().take();
                match event {
                    CountdownEvent::Elapsed => start(&ended, shot, &defer_again, &pass_on),
                    CountdownEvent::Cancelled | CountdownEvent::DrawFailed(_) => {}
                    CountdownEvent::TimerFailed(error) => ended.warn(&format!(
                        "The countdown stopped, so nothing was captured: {error}."
                    )),
                }
            }),
        );
    });
    match counted {
        Ok(counted) => *session.countdown.borrow_mut() = Some(counted),
        Err(error) => session.warn(&format!("The countdown could not start: {error}.")),
    }
}

fn start(session: &Rc<Session>, shot: Shot, defer: &Rc<Defer>, pass_on: &Rc<PassOn>) {
    let defer = Rc::clone(defer);
    let pass_on = Rc::clone(pass_on);
    capture(
        session,
        shot,
        Instant::now(),
        move |after, job| defer(after, job),
        move |image, to| pass_on(image, to),
    );
}
