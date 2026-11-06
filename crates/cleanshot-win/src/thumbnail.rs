use cleanshot_core::thumbnail_layout::{LARGEST_PREVIEW, ThumbnailStay};
use cleanshot_core::{RgbaImage, SavingOff};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, UpdateWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, SW_SHOWNOACTIVATE, ShowWindow};

use crate::Error;
use crate::fade;
use crate::struct_size;

mod drag;
mod window;

pub use drag::Dragged;

// D-96, in device-independent pixels; the other sizes are core's thumbnail_layout.
const RADIUS: f32 = 10.0;

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

/// Whether the thumbnail saves and drags out, which the organisation's policy can turn off
/// (F-57).
pub enum Export {
    /// Save works, and a drag offers what `drag` makes when it starts, or nothing for `None`.
    Allowed {
        drag: Box<dyn Fn() -> Option<Dragged>>,
    },
    /// Save shows greyed, with [`cleanshot_core::saving_off_notice`] for this reason as its
    /// tooltip, and sends nothing, as the toolbar's does (D-95); nothing drags out.
    Off(SavingOff),
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
    /// [`ThumbnailEvent::TimedOut`] once it has shown for `stay` without the pointer on it.
    /// With `drag`, a press dragged past Windows' drag distance asks it for what to drag out
    /// (D-94); without it, the thumbnail cannot be dragged. The thread must have OLE started,
    /// as GPUI's does.
    ///
    /// # Errors
    ///
    /// [`Error::Window`] if the window or its Direct2D surface cannot be made;
    /// [`Error::Monitor`] if the monitor's work area or DPI cannot be read;
    /// [`Error::PreviewTooLarge`] if `preview` is wider or taller than a window can be.
    pub fn show(
        preview: &RgbaImage,
        monitor: RECT,
        stay: ThumbnailStay,
        export: Export,
        on_event: impl FnMut(ThumbnailEvent) + 'static,
    ) -> Result<Self, Error> {
        window::register_class()?;
        let work = work_area(monitor).map_err(Error::Monitor)?;
        let scale = crate::capture::dpi_scale(monitor).map_err(Error::Monitor)?;
        let slide = fade::animations_on().map_err(Error::Window)?;
        let window = window::open_window(
            preview,
            work,
            scale,
            slide,
            stay.duration(),
            export,
            Box::new(on_event),
        )?;
        // SAFETY: `window` was created above on this thread; D-96 shows it without activation.
        let _ = unsafe { ShowWindow(window, SW_SHOWNOACTIVATE) };
        let thumbnail = Self { window };
        // SAFETY: `window` was created above on this thread.
        unsafe { UpdateWindow(window) }
            .ok()
            .map_err(Error::Window)?;
        Ok(thumbnail)
    }

    /// The largest preview, in physical pixels, that the thumbnail shows on the monitor
    /// holding `monitor`: scale the capture down to fit it, never up (D-94, D-96).
    ///
    /// # Errors
    ///
    /// [`Error::Monitor`] if the monitor's DPI cannot be read.
    pub fn largest_preview(monitor: RECT) -> Result<(u32, u32), Error> {
        let scale = crate::capture::dpi_scale(monitor).map_err(Error::Monitor)?;
        let px = |dips: f32| {
            // F-09: a few hundred pixels at any scale Windows offers.
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let pixels = (dips * scale).round() as u32;
            pixels
        };
        Ok((px(LARGEST_PREVIEW.0), px(LARGEST_PREVIEW.1)))
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
        cbSize: struct_size::<MONITORINFO>(),
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

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use cleanshot_core::thumbnail_layout::{buttons, window_size};

    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, IsWindow, MSG,
        PM_REMOVE, PeekMessageW, SendMessageW, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
        WS_EX_NOACTIVATE,
    };

    use super::*;

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

    fn no_drag() -> Export {
        Export::Allowed {
            drag: Box::new(|| None),
        }
    }

    fn shown(events: &Rc<RefCell<Vec<String>>>, export: Export) -> Thumbnail {
        staying(events, ThumbnailStay::default(), export)
    }

    fn staying(
        events: &Rc<RefCell<Vec<String>>>,
        stay: ThumbnailStay,
        export: Export,
    ) -> Thumbnail {
        let sink = Rc::clone(events);
        Thumbnail::show(&preview(), RECT::default(), stay, export, move |event| {
            sink.borrow_mut().push(format!("{event:?}"));
        })
        .expect("the thumbnail opens")
    }

    #[test]
    fn f09_the_thumbnail_never_takes_the_focus_opens_on_a_click_and_leaves_no_window() {
        let _screen = crate::overlay::tests::screen();
        let events = Rc::new(RefCell::new(Vec::new()));
        // SAFETY: reads which window is in the foreground.
        let before = unsafe { GetForegroundWindow() };
        let thumbnail = shown(&events, no_drag());
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
        let thumbnail = shown(&events, Export::Allowed { drag });
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
            pump_until(
                ThumbnailStay::default().duration() + Duration::from_secs(1),
                || !seen.borrow().is_empty(),
            );
            None
        });
        let thumbnail = shown(&events, Export::Allowed { drag });
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
        let thumbnail = Thumbnail::show(
            &preview(),
            RECT::default(),
            ThumbnailStay::default(),
            Export::Allowed { drag },
            move |_| {
                let Freed(_) = &guard;
            },
        )
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
    fn f57_with_saving_off_a_click_on_save_sends_nothing_and_a_click_on_the_preview_still_opens() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let thumbnail = shown(&events, Export::Off(SavingOff::ClipboardOnly));
        let scale = crate::capture::dpi_scale(RECT::default()).expect("the primary monitor's dpi");
        let (width, height) = window_size(120, 80, scale);
        let [(_, save), _] = buttons(width, height, scale);
        let at = |x: i32, y: i32| isize::try_from((y << 16) | x).expect("a client point fits");
        let on_save = at(
            save.left.midpoint(save.right),
            save.top.midpoint(save.bottom),
        );
        click(
            thumbnail.window,
            &[
                (WM_MOUSEMOVE, on_save),
                (WM_LBUTTONDOWN, on_save),
                (WM_LBUTTONUP, on_save),
            ],
        );
        assert!(events.borrow().is_empty(), "{:?}", events.borrow());
        let on_preview = at(width / 2, 4);
        click(
            thumbnail.window,
            &[(WM_LBUTTONDOWN, on_preview), (WM_LBUTTONUP, on_preview)],
        );
        assert_eq!(*events.borrow(), ["Open"]);
        drop(thumbnail);
    }

    #[test]
    fn d117_the_thumbnail_tips_name_each_button_or_say_why_save_is_off_over_it() {
        let _screen = crate::overlay::tests::screen();
        let events = Rc::new(RefCell::new(Vec::new()));
        let thumbnail = shown(&events, Export::Off(SavingOff::ClipboardOnly));
        let (tools, _) =
            crate::toolbar::reading::tips_of(thumbnail.window).expect("the thumbnail has tips");
        drop(thumbnail);
        let scale = crate::capture::dpi_scale(RECT::default()).expect("the primary monitor's dpi");
        let (width, height) = window_size(120, 80, scale);
        let areas = buttons(width, height, scale).map(|(_, at)| crate::capture::win32_rect(at));
        assert_eq!(
            tools,
            [
                (
                    "Your organisation allows copying only. Use Copy.".to_owned(),
                    areas[0]
                ),
                ("Dismiss".to_owned(), areas[1]),
            ]
        );
    }

    #[test]
    fn d165_the_thumbnail_times_out_after_its_stay_with_the_pointer_off_it() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let stay = ThumbnailStay::from_ms(1500).expect("within the range");
        let start = Instant::now();
        let thumbnail = staying(&events, stay, no_drag());
        pump_until(Duration::from_secs(1), || !events.borrow().is_empty());
        assert!(
            events.borrow().is_empty(),
            "{:?} before 1 s",
            events.borrow()
        );
        pump_until(Duration::from_secs(3), || !events.borrow().is_empty());
        assert_eq!(*events.borrow(), ["TimedOut"]);
        let took = start.elapsed();
        let stay = stay.duration();
        assert!(
            took >= stay && took < stay + Duration::from_millis(500),
            "{took:?}"
        );
        drop(thumbnail);
    }
}
