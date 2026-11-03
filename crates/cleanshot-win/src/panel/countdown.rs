use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use cleanshot_core::{Timer, Toolbar};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D_SIZE_U};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, KillTimer, MA_NOACTIVATE, SW_HIDE,
    SW_SHOWNOACTIVATE, SetTimer, SetWindowDisplayAffinity, ShowWindow, WDA_EXCLUDEFROMCAPTURE,
    WM_HOTKEY, WM_LBUTTONDOWN, WM_MOUSEACTIVATE, WM_PAINT, WM_TIMER, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::monitor_under_pointer;
use crate::Error;
use crate::capture::dpi_scale;
use crate::palette::Palette;
use crate::render_target::target;
use crate::text::{Fonts, TextStyle};
use crate::toolbar;
use crate::window_state;

const WINDOW_CLASS: PCWSTR = w!("CleanshotCountdown");
const TICK: usize = 1;
const ESCAPE: i32 = 0;
const LABEL_SIZE: f32 = 13.0;

/// What happened to a countdown.
#[derive(Debug)]
pub enum CountdownEvent {
    /// The wait is over; the countdown has hidden, so the screen can be captured now.
    Elapsed,
    /// Esc or a click stopped the countdown before it was over.
    Cancelled,
    /// Direct2D could not draw the countdown.
    DrawFailed(windows::core::Error),
    /// Windows refused the timer that wakes the countdown each second, so it has ended.
    TimerFailed(windows::core::Error),
}

/// The wait before a timed capture (F-10): the seconds left, counting down in the panel's
/// place. It never takes the keyboard focus, holds Esc as a hotkey only while it counts, and
/// is left out of every capture (D-106, D-107).
pub struct Countdown {
    window: HWND,
}

impl Countdown {
    /// Counts `timer` down on the monitor under the pointer and tells `on_event` when it is
    /// over or cancelled. An Off timer is over at the first message the thread handles.
    ///
    /// # Errors
    ///
    /// [`Error::Window`] if the window or its Direct2D surface cannot be made;
    /// [`Error::Monitor`] if the pointer's monitor or its DPI cannot be read;
    /// [`Error::MonitorTooSmall`] if that monitor cannot hold it; [`Error::HotkeyTaken`] if
    /// another program holds Esc.
    pub fn start(
        timer: Timer,
        on_event: impl FnMut(CountdownEvent) + 'static,
    ) -> Result<Self, Error> {
        register_class()?;
        let monitor = monitor_under_pointer().map_err(Error::Monitor)?;
        let window = open_window(monitor, timer, Box::new(on_event))?;
        Ok(Self { window })
    }
}

impl Drop for Countdown {
    fn drop(&mut self) {
        // SAFETY: the window belongs to this thread and is destroyed once, here.
        let destroyed = unsafe {
            // An id the window no longer holds is the only way this fails.
            let _ = UnregisterHotKey(Some(self.window), ESCAPE);
            DestroyWindow(self.window)
        };
        debug_assert!(destroyed.is_ok(), "the countdown window is destroyed once");
    }
}

type Listener = Box<dyn FnMut(CountdownEvent)>;

struct Surface {
    target: ID2D1HwndRenderTarget,
    material: ID2D1SolidColorBrush,
    hairline: ID2D1SolidColorBrush,
    label: ID2D1SolidColorBrush,
    text: TextStyle,
    scale: f32,
    timer: Timer,
    started: Instant,
    left: Cell<Option<u32>>,
    /// Set when the countdown elapses, is cancelled or fails; nothing is sent after it.
    ended: Cell<bool>,
    listener: RefCell<Listener>,
}

impl Surface {
    fn draw(&self) -> windows::core::Result<()> {
        let Some(left) = self.left.get() else {
            return Ok(());
        };
        // SAFETY: the target and brushes come from one factory and live with `self`.
        unsafe {
            let size = self.target.GetSize();
            let half = 0.5 * self.scale;
            let pill = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: half,
                    top: half,
                    right: size.width - half,
                    bottom: size.height - half,
                },
                radiusX: size.height / 2.0,
                radiusY: size.height / 2.0,
            };
            self.target.BeginDraw();
            self.target.Clear(None);
            self.target
                .FillRoundedRectangle(&raw const pill, &self.material);
            self.target
                .DrawRoundedRectangle(&raw const pill, &self.hairline, self.scale, None);
            self.text
                .draw(&self.target, &spoken(left), &pill.rect, &self.label);
            self.target.EndDraw(None, None)
        }
    }

    /// Shows the seconds left now and wakes again when they change, or ends the countdown.
    fn tick(&self, window: HWND) {
        let elapsed = self.started.elapsed();
        let left = self.timer.remaining(elapsed);
        self.left.set(left);
        let Some(seconds) = left else {
            self.end(window, CountdownEvent::Elapsed);
            return;
        };
        let next = Duration::from_secs(u64::from(self.timer.seconds() - seconds + 1));
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a tick waits at most a second"
        )]
        let wait = next.saturating_sub(elapsed).as_millis() as u32;
        if let Err(error) = wake_in(window, wait) {
            self.end(window, CountdownEvent::TimerFailed(error));
            return;
        }
        // SAFETY: a repaint request for this thread's own window.
        let _ = unsafe { InvalidateRect(Some(window), None, false) };
    }

    /// D-106: hides the countdown before the listener can capture, and lets Esc go.
    fn end(&self, window: HWND, event: CountdownEvent) {
        if self.ended.replace(true) {
            return;
        }
        self.left.set(None);
        // SAFETY: stops this window's own timer and hotkey, and hides it.
        unsafe {
            // An id no longer held is the only way the first two fail, and ShowWindow returns
            // whether the window was visible, not an error.
            let _ = KillTimer(Some(window), TICK);
            let _ = UnregisterHotKey(Some(window), ESCAPE);
            let _ = ShowWindow(window, SW_HIDE);
        }
        self.send(event);
    }

    fn send(&self, event: CountdownEvent) {
        (self.listener.borrow_mut())(event);
    }
}

/// Wakes `window` with a tick after `milliseconds`, replacing any tick not yet due.
fn wake_in(window: HWND, milliseconds: u32) -> windows::core::Result<()> {
    // SAFETY: a timer on this thread's own window; SetTimer returns 0 when it fails.
    match unsafe { SetTimer(Some(window), TICK, milliseconds.max(1), None) } {
        0 => Err(windows::core::Error::from_thread()),
        _ => Ok(()),
    }
}

/// The countdown's text for `seconds` left.
fn spoken(seconds: u32) -> String {
    format!("Capturing in {seconds}")
}

fn register_class() -> Result<(), Error> {
    crate::register_window_class(WINDOW_CLASS, window_procedure)
}

fn open_window(monitor: RECT, timer: Timer, listener: Listener) -> Result<HWND, Error> {
    let scale = dpi_scale(monitor).map_err(Error::Monitor)?;
    let pill = toolbar::layout(Toolbar::Panel, monitor, None, scale)
        .ok_or(Error::MonitorTooSmall)?
        .pill;
    let (width, height) = (pill.right - pill.left, pill.bottom - pill.top);
    // SAFETY: the class is registered and every string is 'static. The process is
    // per-monitor DPI aware, so the rectangle is physical pixels.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            WINDOW_CLASS,
            w!("Countdown"),
            WS_POPUP,
            pill.left,
            pill.top,
            width,
            height,
            None,
            None,
            None,
            None,
        )
    }
    .map_err(Error::Window)?;
    let made = surface(window, (width, height), scale, timer, listener)
        .map_err(Error::Window)
        .and_then(|surface| {
            crate::round_window(window, width, height, height / 2).map_err(Error::Window)?;
            // SAFETY: `window` was created above with nothing stored on it, and
            // window_procedure takes every message's Surface through window_state. D-106: the
            // countdown is left out of every capture, and holds Esc only while it counts.
            unsafe {
                window_state::attach(window, Rc::new(surface));
                SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE).map_err(Error::Window)?;
                RegisterHotKey(Some(window), ESCAPE, MOD_NOREPEAT, u32::from(VK_ESCAPE.0))
                    .map_err(Error::HotkeyTaken)
            }
        });
    if let Err(error) = made {
        // SAFETY: the window was created above; WM_NCDESTROY frees any state on it.
        let destroyed = unsafe { DestroyWindow(window) };
        debug_assert!(destroyed.is_ok(), "a window made here is destroyed once");
        return Err(error);
    }
    if let Err(error) = wake_in(window, 0) {
        // SAFETY: the window was created above; WM_NCDESTROY frees its state.
        let destroyed = unsafe { DestroyWindow(window) };
        debug_assert!(destroyed.is_ok(), "a window made here is destroyed once");
        return Err(Error::Window(error));
    }
    // SAFETY: the window was created above on this thread; it never takes the focus.
    let _ = unsafe { ShowWindow(window, SW_SHOWNOACTIVATE) };
    Ok(window)
}

fn surface(
    window: HWND,
    (width, height): (i32, i32),
    scale: f32,
    timer: Timer,
    listener: Listener,
) -> windows::core::Result<Surface> {
    let factory = crate::render_target::factory()?;
    let target = target(
        &factory,
        window,
        D2D_SIZE_U {
            width: width.unsigned_abs(),
            height: height.unsigned_abs(),
        },
    )?;
    let palette = Palette::current()?;
    // SAFETY: each brush is made on the target it is drawn with.
    let (material, hairline, label) = unsafe {
        (
            target.CreateSolidColorBrush(&raw const palette.material, None)?,
            target.CreateSolidColorBrush(&raw const palette.hairline, None)?,
            target.CreateSolidColorBrush(&raw const palette.label, None)?,
        )
    };
    Ok(Surface {
        target,
        material,
        hairline,
        label,
        text: Fonts::inter()?.text_style(LABEL_SIZE * scale)?.centred()?,
        scale,
        timer,
        started: Instant::now(),
        left: Cell::new(timer.remaining(Duration::ZERO)),
        ended: Cell::new(false),
        listener: RefCell::new(listener),
    })
}

unsafe extern "system" fn window_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: open_window stores a Surface on every window of this class.
    let Some(surface) = (unsafe { window_state::for_message::<Surface>(window, message) }) else {
        // SAFETY: forwarding a message that comes before the Surface or after it.
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    };
    match message {
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            // SAFETY: BeginPaint and EndPaint bracket this window's paint, as WM_PAINT needs.
            unsafe { BeginPaint(window, &raw mut paint) };
            let drawn = surface.draw();
            // SAFETY: matches the BeginPaint above.
            let _ = unsafe { EndPaint(window, &raw const paint) };
            if let Err(error) = drawn {
                surface.send(CountdownEvent::DrawFailed(error));
            }
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TICK => {
            surface.tick(window);
            LRESULT(0)
        }
        WM_HOTKEY | WM_LBUTTONDOWN => {
            surface.end(window, CountdownEvent::Cancelled);
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(isize::try_from(MA_NOACTIVATE).expect("3 fits")),
        // SAFETY: forwarding an unhandled message to the default procedure.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetWindowRect, IsWindowVisible, MSG, PM_REMOVE, PeekMessageW,
        PostMessageW,
    };

    use super::*;
    use crate::capture::{Method, capture_all, region};
    use crate::overlay::tests::screen;

    type Heard = Rc<RefCell<Vec<&'static str>>>;

    fn started(timer: Timer) -> (Countdown, Heard) {
        let heard: Heard = Rc::default();
        let sink = Rc::clone(&heard);
        let countdown = Countdown::start(timer, move |event| {
            sink.borrow_mut().push(match event {
                CountdownEvent::Elapsed => "elapsed",
                CountdownEvent::Cancelled => "cancelled",
                CountdownEvent::DrawFailed(error) => panic!("the countdown draws: {error}"),
                CountdownEvent::TimerFailed(error) => panic!("the countdown ticks: {error}"),
            });
        })
        .expect("the countdown starts");
        (countdown, heard)
    }

    fn pump(duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            let mut message = MSG::default();
            // SAFETY: `message` is writable and the countdown window belongs to this thread.
            while unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                // SAFETY: `message` was just filled by PeekMessageW.
                unsafe { DispatchMessageW(&raw const message) };
            }
        }
    }

    fn visible(countdown: &Countdown) -> bool {
        // SAFETY: IsWindowVisible only reads the window's style.
        unsafe { IsWindowVisible(countdown.window) }.as_bool()
    }

    #[test]
    fn f10_a_three_second_countdown_elapses_once_after_three_seconds_and_hides_first() {
        let _screen = screen();
        let (countdown, heard) = started(Timer::Three);
        pump(Duration::from_millis(2_700));
        assert!(heard.borrow().is_empty(), "elapsed early");
        assert!(visible(&countdown));
        pump(Duration::from_millis(1_000));
        assert_eq!(*heard.borrow(), ["elapsed"]);
        assert!(!visible(&countdown));
    }

    #[test]
    fn f10_esc_cancels_the_countdown_and_nothing_elapses_after() {
        let _screen = screen();
        let (countdown, heard) = started(Timer::Three);
        pump(Duration::from_millis(200));
        // SAFETY: posting copies no memory; the window belongs to this thread.
        unsafe { PostMessageW(Some(countdown.window), WM_HOTKEY, WPARAM(0), LPARAM(0)) }
            .expect("the post is queued");
        pump(Duration::from_millis(3_500));
        assert_eq!(*heard.borrow(), ["cancelled"]);
        assert!(!visible(&countdown));
    }

    #[test]
    fn f10_an_off_timer_elapses_at_once() {
        let _screen = screen();
        let (_countdown, heard) = started(Timer::Off);
        assert!(
            heard.borrow().is_empty(),
            "nothing is sent from inside start"
        );
        pump(Duration::from_millis(100));
        assert_eq!(*heard.borrow(), ["elapsed"]);
    }

    // Both copies are taken with the countdown up and nothing else changing, so the screen
    // under it is the same; only its own number differs between them.
    #[test]
    fn f11_the_countdown_is_left_out_of_a_capture() {
        let _screen = screen();
        let (countdown, _heard) = started(Timer::Ten);
        pump(Duration::from_millis(100));
        let mut at = RECT::default();
        // SAFETY: `at` is writable and the window belongs to this thread.
        unsafe { GetWindowRect(countdown.window, &raw mut at) }.expect("it has a frame");
        let at_ten = capture_all(Method::GdiBitBlt).expect("the screen copies");
        pump(Duration::from_millis(1_000));
        let at_nine = capture_all(Method::GdiBitBlt).expect("the screen copies");
        assert_eq!(
            region(&at_ten, at).expect("its place is on a monitor"),
            region(&at_nine, at).expect("its place is on a monitor"),
            "the countdown shows in the capture: its number changed in it"
        );
    }
}
