use std::time::Duration;

use cleanshot_core::RgbaImage;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, UpdateWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, SW_SHOWNOACTIVATE, ShowWindow};

use crate::Error;
use crate::fade;

mod drag;
mod window;

pub use drag::Dragged;

// D-96, in device-independent pixels.
const INSET: f32 = 16.0;
const RADIUS: f32 = 10.0;
const BUTTON_HEIGHT: f32 = 24.0;
const BUTTON_WIDTH: f32 = 64.0;
const BUTTON_GAP: f32 = 8.0;
const STAY: Duration = Duration::from_secs(5);
const SLIDE: Duration = Duration::from_millis(120);

/// What happened on the thumbnail, delivered inside its window's procedure. Dropping the
/// [`Thumbnail`] during the call is sound; closing it with a fade belongs after the call.
#[derive(Debug)]
pub enum ThumbnailEvent {
    /// The preview was clicked: open the capture in the editor.
    Open,
    /// Save was clicked.
    Save,
    /// Dismiss was clicked.
    Dismiss,
    /// Five seconds passed with the pointer off the thumbnail.
    TimedOut,
    /// The thumbnail was dragged out and dropped on another app (D-94).
    Dropped,
    /// Windows' drag loop failed.
    DragFailed(windows::core::Error),
    /// Direct2D could not draw the preview.
    DrawFailed(windows::core::Error),
}

/// A button the thumbnail shows while the pointer is on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Save,
    Dismiss,
}

impl Button {
    fn label(self) -> &'static str {
        match self {
            Self::Save => "Save",
            Self::Dismiss => "Dismiss",
        }
    }
}

/// F-09: the post-capture preview, a window in the bottom-right corner of a monitor's work
/// area that never activates and so never takes the focus (D-96).
///
/// Dropping it closes it at once.
pub struct Thumbnail {
    window: HWND,
}

impl Thumbnail {
    /// Shows `preview`, already scaled down, on the monitor holding `monitor`, sliding in over
    /// 120 ms or appearing at once when Windows' Animation effects is off (F-13). It sends
    /// [`ThumbnailEvent::TimedOut`] once it has shown for five seconds without the pointer on
    /// it. With `drag`, a press dragged past Windows' drag distance asks it for what to drag
    /// out (D-94); without it, the thumbnail cannot be dragged. The thread must have OLE
    /// started, as GPUI's does.
    ///
    /// # Errors
    ///
    /// [`Error::Window`] if the window or its Direct2D surface cannot be made, or the
    /// monitor's work area or DPI cannot be read.
    pub fn show(
        preview: &RgbaImage,
        monitor: RECT,
        drag: Option<Box<dyn Fn() -> Option<Dragged>>>,
        on_event: impl FnMut(ThumbnailEvent) + 'static,
    ) -> Result<Self, Error> {
        window::register_class()?;
        let work = work_area(monitor).map_err(Error::Window)?;
        let scale = crate::overlay::dpi_scale(monitor).map_err(Error::Window)?;
        let slide = fade::animations_on().map_err(Error::Window)?;
        let window = window::open_window(preview, work, scale, slide, drag, Box::new(on_event))?;
        // SAFETY: `window` was created above on this thread; D-96 shows it without activation.
        let _ = unsafe { ShowWindow(window, SW_SHOWNOACTIVATE) };
        let thumbnail = Self { window };
        // SAFETY: `window` was created above on this thread.
        unsafe { UpdateWindow(window) }
            .ok()
            .map_err(Error::Window)?;
        Ok(thumbnail)
    }

    /// Closes the thumbnail with F-13's 120 ms fade, or at once when Windows' Animation effects
    /// is off.
    ///
    /// # Errors
    ///
    /// The Windows error that stopped the fade; the thumbnail is closed either way.
    pub fn close(self) -> windows::core::Result<()> {
        if fade::animations_on()? {
            fade::fade_out(&[self.window])?;
        }
        Ok(())
    }
}

impl Drop for Thumbnail {
    fn drop(&mut self) {
        // SAFETY: the window belongs to this thread and is destroyed once, here.
        let destroyed = unsafe { DestroyWindow(self.window) };
        debug_assert!(destroyed.is_ok(), "a thumbnail window is destroyed once");
    }
}

/// The work area, in physical pixels, of the monitor holding `monitor`.
fn work_area(monitor: RECT) -> windows::core::Result<RECT> {
    let mut info = MONITORINFO {
        cbSize: u32::try_from(size_of::<MONITORINFO>()).expect("fits in u32"),
        ..Default::default()
    };
    // SAFETY: `info` is writable with its size set; MonitorFromRect always returns a monitor
    // here.
    unsafe {
        GetMonitorInfoW(
            MonitorFromRect(&raw const monitor, MONITOR_DEFAULTTONEAREST),
            &raw mut info,
        )
    }
    .ok()?;
    Ok(info.rcWork)
}

/// Where a thumbnail `width` x `height` rests: `inset` in from the bottom-right corner of
/// `work`.
fn place(work: RECT, width: i32, height: i32, inset: i32) -> RECT {
    let (right, bottom) = (work.right - inset, work.bottom - inset);
    RECT {
        left: right - width,
        top: bottom - height,
        right,
        bottom,
    }
}

/// How far right of its resting place the thumbnail is `elapsed` into a slide of `distance`
/// pixels, or `None` once it has arrived.
fn slid(elapsed: Duration, distance: i32) -> Option<i32> {
    let left = SLIDE.checked_sub(elapsed).filter(|left| !left.is_zero())?;
    let micros = i64::try_from(left.as_micros()).expect("left <= SLIDE");
    let slide = i64::try_from(SLIDE.as_micros()).expect("SLIDE fits");
    Some(i32::try_from(i64::from(distance) * micros / slide).expect("at most distance"))
}

/// The time the thumbnail has left to show after `elapsed` more, or `None` once it is up.
/// While it is `held`, by the pointer on it or a drag from it, the time stands still (D-96).
fn left_after(left: Duration, elapsed: Duration, held: bool) -> Option<Duration> {
    if held {
        return Some(left);
    }
    left.checked_sub(elapsed).filter(|left| !left.is_zero())
}

/// The buttons on a thumbnail `width` x `height` at `scale` physical pixels per DIP, in its
/// client pixels: Save and Dismiss side by side, centred along its bottom edge.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the buttons are a few dozen pixels"
)]
fn buttons(width: i32, height: i32, scale: f32) -> [(Button, RECT); 2] {
    let px = |dips: f32| (dips * scale).round() as i32;
    let (button, gap) = (px(BUTTON_WIDTH), px(BUTTON_GAP));
    let top = height - gap - px(BUTTON_HEIGHT);
    let left = (width - 2 * button - gap) / 2;
    let at = |left: i32| RECT {
        left,
        top,
        right: left + button,
        bottom: top + px(BUTTON_HEIGHT),
    };
    [
        (Button::Save, at(left)),
        (Button::Dismiss, at(left + button + gap)),
    ]
}

/// The button under `point`, in client pixels, if any.
fn button_at(buttons: &[(Button, RECT); 2], point: POINT) -> Option<Button> {
    buttons.iter().find_map(|&(button, area)| {
        let inside = (area.left..area.right).contains(&point.x)
            && (area.top..area.bottom).contains(&point.y);
        inside.then_some(button)
    })
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::time::Instant;

    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, IsWindow, MSG,
        PM_REMOVE, PeekMessageW, SendMessageW, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
        WS_EX_NOACTIVATE,
    };

    use super::*;

    const WORK: RECT = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };

    #[test]
    fn f09_the_thumbnail_rests_its_inset_in_from_the_work_areas_bottom_right_corner() {
        assert_eq!(
            place(WORK, 300, 200, 16),
            RECT {
                left: 1604,
                top: 824,
                right: 1904,
                bottom: 1024,
            }
        );
    }

    #[test]
    fn f09_the_slide_starts_its_distance_out_and_arrives_in_120_ms() {
        let at = |millis| slid(Duration::from_millis(millis), 16);
        assert_eq!(at(0), Some(16));
        assert_eq!(at(60), Some(8));
        assert_eq!(at(119), Some(0));
        assert_eq!(at(120), None);
        assert_eq!(at(500), None);
    }

    #[test]
    fn f09_the_pointer_on_the_thumbnail_stops_its_five_seconds_running_out() {
        let second = Duration::from_secs(1);
        assert_eq!(left_after(STAY, second, true), Some(STAY));
        assert_eq!(
            left_after(STAY, second, false),
            Some(Duration::from_secs(4))
        );
        assert_eq!(left_after(second, second, false), None);
        assert_eq!(left_after(second, STAY, true), Some(second));
    }

    #[test]
    fn f09_save_and_dismiss_sit_side_by_side_along_the_bottom_and_nothing_else_is_a_button() {
        let placed = buttons(300, 200, 1.0);
        let [(Button::Save, save), (Button::Dismiss, dismiss)] = placed else {
            panic!("Save then Dismiss: {placed:?}");
        };
        assert_eq!((save.bottom, dismiss.bottom), (192, 192));
        assert_eq!(dismiss.left - save.right, 8);
        assert_eq!(save.left, 300 - dismiss.right);
        let point = |x, y| POINT { x, y };
        assert_eq!(
            button_at(&placed, point(save.left, save.top)),
            Some(Button::Save)
        );
        assert_eq!(
            button_at(&placed, point(dismiss.right - 1, dismiss.bottom - 1)),
            Some(Button::Dismiss)
        );
        assert_eq!(button_at(&placed, point(150, 20)), None);
        assert_eq!(button_at(&placed, point(save.right + 1, save.top)), None);
    }

    fn preview() -> RgbaImage {
        let (width, height) = (120, 80);
        let pixels = (0..width * height)
            .flat_map(|pixel| [u8::try_from(pixel % 256).expect("< 256"), 90, 200, 255])
            .collect();
        RgbaImage::new(width, height, pixels).expect("120x80 RGBA")
    }

    fn pump_until(deadline: Duration, done: impl Fn() -> bool) {
        let start = Instant::now();
        while start.elapsed() < deadline && !done() {
            let mut message = MSG::default();
            // SAFETY: `message` is writable and the thumbnail's window belongs to this thread.
            while unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                // SAFETY: `message` was just filled by PeekMessageW.
                unsafe { DispatchMessageW(&raw const message) };
            }
        }
    }

    /// Sends each `(message, lparam)` to `window` in turn, as the mouse would.
    fn click(window: HWND, messages: &[(u32, isize)]) {
        for &(message, lparam) in messages {
            // SAFETY: the window belongs to this thread, and a mouse message carries no pointer.
            unsafe { SendMessageW(window, message, Some(WPARAM(0)), Some(LPARAM(lparam))) };
        }
    }

    fn shown(
        events: &Rc<RefCell<Vec<String>>>,
        drag: Option<Box<dyn Fn() -> Option<Dragged>>>,
    ) -> Thumbnail {
        let sink = Rc::clone(events);
        Thumbnail::show(&preview(), RECT::default(), drag, move |event| {
            sink.borrow_mut().push(format!("{event:?}"));
        })
        .expect("the thumbnail opens")
    }

    #[test]
    fn f09_the_thumbnail_never_takes_the_focus_opens_on_a_click_and_leaves_no_window() {
        let events = Rc::new(RefCell::new(Vec::new()));
        // SAFETY: reads which window is in the foreground.
        let before = unsafe { GetForegroundWindow() };
        let thumbnail = shown(&events, None);
        let window = thumbnail.window;
        pump_until(Duration::from_millis(300), || false);
        // SAFETY: reads the window's own style and the foreground window.
        let (style, after) = unsafe {
            (
                GetWindowLongPtrW(window, GWL_EXSTYLE),
                GetForegroundWindow(),
            )
        };
        assert_eq!(after, before, "the foreground window stays where it was");
        assert_ne!(
            style & isize::try_from(WS_EX_NOACTIVATE.0).expect("fits"),
            0
        );
        // SAFETY: the window belongs to this thread; (10, 10) is on the preview, off the buttons.
        click(
            window,
            &[(WM_LBUTTONDOWN, 0x000A_000A), (WM_LBUTTONUP, 0x000A_000A)],
        );
        assert_eq!(*events.borrow(), ["Open"]);
        drop(thumbnail);
        // SAFETY: IsWindow only reads whether the handle still names a window.
        assert!(!unsafe { IsWindow(Some(window)) }.as_bool());
    }

    #[test]
    fn f09_a_press_dragged_past_the_drag_distance_asks_for_the_drag_and_is_not_a_click() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let asked = Rc::new(Cell::new(0));
        let asking = Rc::clone(&asked);
        let drag: Box<dyn Fn() -> Option<Dragged>> = Box::new(move || {
            asking.set(asking.get() + 1);
            None
        });
        let thumbnail = shown(&events, Some(drag));
        let (press, near, far) = (0x000A_000A, 0x000A_000B, 0x0028_0028);
        click(
            thumbnail.window,
            &[(WM_LBUTTONDOWN, press), (WM_MOUSEMOVE, near)],
        );
        assert_eq!(
            asked.get(),
            0,
            "a move inside the drag distance is still a press"
        );
        click(
            thumbnail.window,
            &[(WM_MOUSEMOVE, far), (WM_LBUTTONUP, far)],
        );
        assert_eq!(asked.get(), 1);
        assert!(events.borrow().is_empty(), "{:?}", events.borrow());
        drop(thumbnail);
    }

    const PRESS: isize = 0x000A_000A;
    const FAR: isize = 0x0028_0028;

    #[test]
    fn f09_the_countdown_stands_still_while_a_drag_runs() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let seen = Rc::clone(&events);
        let drag: Box<dyn Fn() -> Option<Dragged>> = Box::new(move || {
            // Stands in for DoDragDrop's modal loop, which dispatches this thread's timers.
            pump_until(STAY + Duration::from_secs(1), || !seen.borrow().is_empty());
            None
        });
        let thumbnail = shown(&events, Some(drag));
        click(
            thumbnail.window,
            &[(WM_LBUTTONDOWN, PRESS), (WM_MOUSEMOVE, FAR)],
        );
        assert!(
            events.borrow().is_empty(),
            "{:?} during the drag",
            events.borrow()
        );
        drop(thumbnail);
    }

    #[test]
    fn f09_a_thumbnail_dropped_during_its_drag_is_freed_only_once_the_drag_returns() {
        struct Freed(Rc<Cell<bool>>);
        impl Drop for Freed {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let freed = Rc::new(Cell::new(false));
        let guard = Freed(Rc::clone(&freed));
        let slot: Rc<RefCell<Option<Thumbnail>>> = Rc::default();
        let freed_during = Rc::new(Cell::new(None));
        let (closing, watching, report) = (
            Rc::clone(&slot),
            Rc::clone(&freed),
            Rc::clone(&freed_during),
        );
        let drag: Box<dyn Fn() -> Option<Dragged>> = Box::new(move || {
            drop(closing.borrow_mut().take());
            report.set(Some(watching.get()));
            None
        });
        let thumbnail = Thumbnail::show(&preview(), RECT::default(), Some(drag), move |_| {
            let Freed(_) = &guard;
        })
        .expect("the thumbnail opens");
        let window = thumbnail.window;
        *slot.borrow_mut() = Some(thumbnail);
        click(window, &[(WM_LBUTTONDOWN, PRESS), (WM_MOUSEMOVE, FAR)]);
        assert_eq!(
            freed_during.get(),
            Some(false),
            "freed under the running drag"
        );
        assert!(freed.get(), "freed once the drag returned");
        // SAFETY: IsWindow only reads whether the handle still names a window.
        assert!(!unsafe { IsWindow(Some(window)) }.as_bool());
    }

    #[test]
    fn f09_the_thumbnail_times_out_after_five_seconds_with_the_pointer_off_it() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let thumbnail = shown(&events, None);
        let start = Instant::now();
        pump_until(Duration::from_secs(4), || !events.borrow().is_empty());
        assert!(
            events.borrow().is_empty(),
            "{:?} before 4 s",
            events.borrow()
        );
        pump_until(Duration::from_secs(3), || !events.borrow().is_empty());
        assert_eq!(*events.borrow(), ["TimedOut"]);
        let took = start.elapsed();
        assert!(
            took >= STAY && took < STAY + Duration::from_millis(500),
            "{took:?}"
        );
        drop(thumbnail);
    }
}
