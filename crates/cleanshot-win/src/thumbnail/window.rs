use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use cleanshot_core::RgbaImage;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_ROUNDED_RECT,
    D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateRoundRectRgn, DeleteObject, EndPaint, InvalidateRect, PAINTSTRUCT,
    SetWindowRgn,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, KillTimer,
    MA_NOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetTimer, SetWindowLongPtrW,
    SetWindowPos, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NCDESTROY,
    WM_PAINT, WM_TIMER, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Interface, PCWSTR, w};

use super::drag::{self, Dragged, Ended, left_the_press};
use super::{
    Button, INSET, RADIUS, STAY, ThumbnailEvent, button_at, buttons, left_after, place, slid,
};
use crate::Error;
use crate::overlay::{bgra, client_point, target_and_bitmap};
use crate::palette::Palette;
use crate::text::{Fonts, TextStyle};

const WINDOW_CLASS: PCWSTR = w!("CleanshotThumbnail");
const SLIDE_TIMER: usize = 1;
const COUNTDOWN_TIMER: usize = 2;
const FRAME: u32 = 8;
const TICK: u32 = 100;
const TEXT_SIZE: f32 = 12.0;
const HAIRLINE: f32 = 1.0;

struct Surface {
    target: ID2D1HwndRenderTarget,
    preview: ID2D1Bitmap,
    material: ID2D1SolidColorBrush,
    hairline: ID2D1SolidColorBrush,
    label: ID2D1SolidColorBrush,
    text: TextStyle,
    buttons: [(Button, RECT); 2],
    rest: RECT,
    distance: i32,
    radius: f32,
    shown: Instant,
    ticked: Cell<Instant>,
    /// `None` once [`ThumbnailEvent::TimedOut`] has been sent.
    left: Cell<Option<Duration>>,
    hovered: Cell<bool>,
    /// A drag from the thumbnail is running; its modal loop still delivers the timers.
    dragging: Cell<bool>,
    /// Where the left button went down on the thumbnail, until it is released or a drag starts.
    pressed: Cell<Option<POINT>>,
    drag: Option<Box<dyn Fn() -> Option<Dragged>>>,
    listener: RefCell<Box<dyn FnMut(ThumbnailEvent)>>,
}

impl Surface {
    fn send(&self, event: ThumbnailEvent) {
        if let Ok(mut listener) = self.listener.try_borrow_mut() {
            listener(event);
        }
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "a thumbnail is a few hundred pixels"
    )]
    fn draw(&self) -> windows::core::Result<()> {
        let rect = |area: RECT| D2D_RECT_F {
            left: area.left as f32,
            top: area.top as f32,
            right: area.right as f32,
            bottom: area.bottom as f32,
        };
        // SAFETY: the target, bitmap and brushes come from one factory and live with `self`.
        unsafe {
            self.target.BeginDraw();
            let size = self.target.GetSize();
            let whole = D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: size.width,
                bottom: size.height,
            };
            self.target.DrawBitmap(
                &self.preview,
                Some(&raw const whole),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
            if self.hovered.get() {
                for (button, area) in self.buttons {
                    let pill = D2D1_ROUNDED_RECT {
                        rect: rect(area),
                        radiusX: (area.bottom - area.top) as f32 / 2.0,
                        radiusY: (area.bottom - area.top) as f32 / 2.0,
                    };
                    self.target
                        .FillRoundedRectangle(&raw const pill, &self.material);
                    self.target.DrawRoundedRectangle(
                        &raw const pill,
                        &self.hairline,
                        HAIRLINE,
                        None,
                    );
                    self.text.draw(
                        &self.target.cast()?,
                        button.label(),
                        &pill.rect,
                        &self.label,
                    );
                }
            }
            let edge = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: HAIRLINE / 2.0,
                    top: HAIRLINE / 2.0,
                    right: size.width - HAIRLINE / 2.0,
                    bottom: size.height - HAIRLINE / 2.0,
                },
                radiusX: self.radius,
                radiusY: self.radius,
            };
            self.target
                .DrawRoundedRectangle(&raw const edge, &self.hairline, HAIRLINE, None);
            self.target.EndDraw(None, None)
        }
    }

    /// Moves the window one frame further into its resting place, and stops once it is there.
    fn slide(&self, window: HWND) {
        let offset = slid(self.shown.elapsed(), self.distance);
        // SAFETY: `window` is this surface's own window; a failed move only skips a frame.
        unsafe {
            let _ = SetWindowPos(
                window,
                None,
                self.rest.left + offset.unwrap_or(0),
                self.rest.top,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
            );
            if offset.is_none() {
                let _ = KillTimer(Some(window), SLIDE_TIMER);
            }
        }
    }

    /// Starts the drag-out once a press has moved past Windows' drag distance (D-94).
    fn drag_from(&self, now: POINT) {
        let (Some(start), Some(drag)) = (self.pressed.get(), &self.drag) else {
            return;
        };
        if !left_the_press(start, now) {
            return;
        }
        self.pressed.set(None);
        self.dragging.set(true);
        let ended = drag().map(drag::drag);
        self.dragging.set(false);
        match ended {
            Some(Ok(Ended::Dropped)) => self.send(ThumbnailEvent::Dropped),
            Some(Ok(Ended::Cancelled)) | None => {}
            Some(Err(error)) => self.send(ThumbnailEvent::DragFailed(error)),
        }
    }

    fn count_down(&self, window: HWND) {
        let now = Instant::now();
        let elapsed = now - self.ticked.replace(now);
        let Some(left) = self.left.get() else { return };
        let held = self.hovered.get() || self.dragging.get();
        let left = left_after(left, elapsed, held);
        self.left.set(left);
        if left.is_none() {
            // SAFETY: `window` is this surface's own window.
            let _ = unsafe { KillTimer(Some(window), COUNTDOWN_TIMER) };
            self.send(ThumbnailEvent::TimedOut);
        }
    }
}

pub(super) fn register_class() -> Result<(), Error> {
    // SAFETY: a null module name asks for this exe's own handle.
    let instance = unsafe { GetModuleHandleW(None) }.map_err(Error::Window)?;
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).expect("fits in u32"),
        lpfnWndProc: Some(window_procedure),
        hInstance: instance.into(),
        lpszClassName: WINDOW_CLASS,
        ..Default::default()
    };
    crate::register_window_class(&class)
}

/// Creates the thumbnail's window, not yet shown, at the start of its slide when `slide` is
/// set and at its resting place otherwise.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the inset and radius are a few dozen pixels"
)]
pub(super) fn open_window(
    preview: &RgbaImage,
    work: RECT,
    scale: f32,
    slide: bool,
    drag: Option<Box<dyn Fn() -> Option<Dragged>>>,
    listener: Box<dyn FnMut(ThumbnailEvent)>,
) -> Result<HWND, Error> {
    let (width, height) = (preview.width(), preview.height());
    let (Ok(client_width), Ok(client_height)) = (i32::try_from(width), i32::try_from(height))
    else {
        return Err(Error::Window(windows::core::Error::empty()));
    };
    let px = |dips: f32| (dips * scale).round() as i32;
    let rest = place(work, client_width, client_height, px(INSET));
    let distance = if slide { px(INSET) } else { 0 };
    // SAFETY: the class is registered and every string is 'static. The process is
    // per-monitor DPI aware, so the rectangle is physical pixels.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            WINDOW_CLASS,
            w!("Screenshot preview"),
            WS_POPUP,
            rest.left + distance,
            rest.top,
            client_width,
            client_height,
            None,
            None,
            None,
            None,
        )
    }
    .map_err(Error::Window)?;
    let made =
        surface(window, preview, rest, distance, scale, drag, listener).and_then(|surface| {
            round_corners(window, client_width, client_height, px(RADIUS))?;
            // SAFETY: `window` exists, so WM_NCDESTROY will give up this count once. The timers
            // die with the window.
            unsafe {
                SetWindowLongPtrW(
                    window,
                    GWLP_USERDATA,
                    Rc::into_raw(Rc::new(surface)) as isize,
                );
                if distance > 0 {
                    SetTimer(Some(window), SLIDE_TIMER, FRAME, None);
                }
                SetTimer(Some(window), COUNTDOWN_TIMER, TICK, None);
            }
            Ok(())
        });
    if let Err(error) = made {
        // SAFETY: the window was created above and holds no state yet.
        let destroyed = unsafe { DestroyWindow(window) };
        debug_assert!(destroyed.is_ok(), "a window made here is destroyed once");
        return Err(Error::Window(error));
    }
    Ok(window)
}

fn surface(
    window: HWND,
    preview: &RgbaImage,
    rest: RECT,
    distance: i32,
    scale: f32,
    drag: Option<Box<dyn Fn() -> Option<Dragged>>>,
    listener: Box<dyn FnMut(ThumbnailEvent)>,
) -> windows::core::Result<Surface> {
    // SAFETY: D2D1CreateFactory has no preconditions.
    let factory: ID2D1Factory =
        unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }?;
    let (width, height) = (preview.width(), preview.height());
    let (target, preview) = target_and_bitmap(&factory, window, width, height, &bgra(preview))?;
    let palette = Palette::current()?;
    // SAFETY: each colour lives in `palette`, which outlives its call.
    let (material, hairline, label) = unsafe {
        (
            target.CreateSolidColorBrush(&raw const palette.material, None)?,
            target.CreateSolidColorBrush(&raw const palette.hairline, None)?,
            target.CreateSolidColorBrush(&raw const palette.label, None)?,
        )
    };
    let now = Instant::now();
    Ok(Surface {
        text: Fonts::inter()?.text_style(TEXT_SIZE * scale)?.centred()?,
        buttons: buttons(rest.right - rest.left, rest.bottom - rest.top, scale),
        target,
        preview,
        material,
        hairline,
        label,
        rest,
        distance,
        radius: RADIUS * scale,
        shown: now,
        ticked: Cell::new(now),
        left: Cell::new(Some(STAY)),
        hovered: Cell::new(false),
        dragging: Cell::new(false),
        pressed: Cell::new(None),
        drag,
        listener: RefCell::new(listener),
    })
}

/// Clips `window` to a rectangle `width` x `height` with corners of `radius` pixels.
fn round_corners(window: HWND, width: i32, height: i32, radius: i32) -> windows::core::Result<()> {
    // SAFETY: the region is new; on success the window owns it, so it is deleted only on
    // failure.
    unsafe {
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, 2 * radius, 2 * radius);
        if SetWindowRgn(window, Some(region), false) == 0 {
            let _ = DeleteObject(region.into());
            return Err(windows::core::Error::from_thread());
        }
    }
    Ok(())
}

unsafe extern "system" fn window_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: GWLP_USERDATA is 0 until open_window stores the Surface pointer, and
    // WM_NCDESTROY clears it.
    let surface = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const Surface;
    if surface.is_null() {
        // SAFETY: forwarding an unhandled message to the default procedure.
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    if message == WM_NCDESTROY {
        // SAFETY: this is the window's last message; the window's count is given up once and
        // the pointer cleared. A message still running below holds its own count.
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            drop(Rc::from_raw(surface));
            return DefWindowProcW(window, message, wparam, lparam);
        }
    }
    // SAFETY: a non-null pointer carries the window's count until WM_NCDESTROY. This message
    // takes a count of its own, so a Thumbnail dropped while it runs (from an event, or during
    // a drag's modal loop) frees the Surface only once the message returns.
    let surface = unsafe {
        Rc::increment_strong_count(surface);
        Rc::from_raw(surface)
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
                surface.send(ThumbnailEvent::DrawFailed(error));
            }
            LRESULT(0)
        }
        // D-96: a click never activates the thumbnail, so the focus stays where it was.
        WM_MOUSEACTIVATE => LRESULT(isize::try_from(MA_NOACTIVATE).expect("3 fits")),
        WM_MOUSEMOVE => {
            if !surface.hovered.replace(true) {
                let mut leave = TRACKMOUSEEVENT {
                    cbSize: u32::try_from(size_of::<TRACKMOUSEEVENT>()).expect("fits in u32"),
                    dwFlags: TME_LEAVE,
                    hwndTrack: window,
                    dwHoverTime: 0,
                };
                // SAFETY: `leave` is writable and names this thread's window. Without it the
                // buttons stay up until the next move, which is all a failure costs.
                unsafe {
                    let _ = TrackMouseEvent(&raw mut leave);
                    let _ = InvalidateRect(Some(window), None, false);
                }
            }
            surface.drag_from(client_point(lparam));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            surface.hovered.set(false);
            // SAFETY: invalidating this thread's own window; a failure only skips one repaint.
            let _ = unsafe { InvalidateRect(Some(window), None, false) };
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            surface.pressed.set(Some(client_point(lparam)));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if surface.pressed.take().is_none() {
                return LRESULT(0);
            }
            surface.send(match button_at(&surface.buttons, client_point(lparam)) {
                Some(Button::Save) => ThumbnailEvent::Save,
                Some(Button::Dismiss) => ThumbnailEvent::Dismiss,
                None => ThumbnailEvent::Open,
            });
            LRESULT(0)
        }
        WM_TIMER => {
            match wparam.0 {
                SLIDE_TIMER => surface.slide(window),
                COUNTDOWN_TIMER => surface.count_down(window),
                // SAFETY: forwarding a timer this window did not set.
                _ => return unsafe { DefWindowProcW(window, message, wparam, lparam) },
            }
            LRESULT(0)
        }
        // SAFETY: forwarding an unhandled message to the default procedure.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}
