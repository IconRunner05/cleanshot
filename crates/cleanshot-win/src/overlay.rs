use std::cell::{Cell, RefCell};
use std::rc::Rc;

use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_BITMAP_PROPERTIES,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory,
    ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, MOD_NOREPEAT, RegisterHotKey, ReleaseCapture, SetCapture, UnregisterHotKey,
    VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW,
    RegisterClassExW, SW_SHOWNOACTIVATE, SetWindowLongPtrW, ShowWindow, WM_HOTKEY, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCDESTROY, WM_PAINT, WNDCLASSEXW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::Error;
use crate::capture::MonitorImage;

const WINDOW_CLASS: PCWSTR = w!("CleanshotOverlay");
const ESCAPE_HOTKEY_ID: i32 = 1;
const DIM_OPACITY: f32 = 0.4;
const DESKTOP_DPI: f32 = 96.0;
const BORDER_WIDTH: f32 = 1.5;
const ACCENT: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 0.039,
    g: 0.518,
    b: 1.0,
    a: 1.0,
};

/// What happened on the overlay, delivered on its thread's message loop.
#[derive(Debug)]
pub enum OverlayEvent {
    /// Esc was pressed (F-01: nothing is written).
    Cancelled,
    /// The user dragged out this rectangle, in physical virtual-desktop pixels, never empty.
    Selected(RECT),
    /// Direct2D could not draw a monitor's frozen image.
    DrawFailed(windows::core::Error),
}

type Listener = Rc<RefCell<Box<dyn FnMut(OverlayEvent)>>>;

struct Surface {
    target: ID2D1HwndRenderTarget,
    frozen: ID2D1Bitmap,
    dim: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    origin: POINT,
    drag: Cell<Option<(POINT, POINT)>>,
    listener: Listener,
}

impl Surface {
    fn send(&self, event: OverlayEvent) {
        if let Ok(mut listener) = self.listener.try_borrow_mut() {
            listener(event);
        }
    }

    fn draw(&self) -> windows::core::Result<()> {
        // SAFETY: the target, bitmap and brush come from one factory and live with `self`.
        unsafe {
            let size = self.target.GetSize();
            let whole = D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: size.width,
                bottom: size.height,
            };
            self.target.BeginDraw();
            self.target.DrawBitmap(
                &self.frozen,
                Some(&raw const whole),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                None,
            );
            let Some(selected) = self
                .drag
                .get()
                .and_then(|(anchor, current)| span(anchor, current))
            else {
                self.target.FillRectangle(&raw const whole, &self.dim);
                return self.target.EndDraw(None, None);
            };
            #[expect(clippy::cast_precision_loss, reason = "screen coordinates fit in f32")]
            let (left, top, right, bottom) = (
                selected.left as f32,
                selected.top as f32,
                selected.right as f32,
                selected.bottom as f32,
            );
            for band in [
                (0.0, 0.0, size.width, top),
                (0.0, bottom, size.width, size.height),
                (0.0, top, left, bottom),
                (right, top, size.width, bottom),
            ] {
                let band = D2D_RECT_F {
                    left: band.0,
                    top: band.1,
                    right: band.2,
                    bottom: band.3,
                };
                self.target.FillRectangle(&raw const band, &self.dim);
            }
            let border = D2D_RECT_F {
                left,
                top,
                right,
                bottom,
            };
            self.target
                .DrawRectangle(&raw const border, &self.accent, BORDER_WIDTH, None);
            self.target.EndDraw(None, None)
        }
    }
}

/// One topmost, never-activated window per monitor, each showing that monitor's frozen
/// capture dimmed by 40% (F-02). Esc is a hotkey only while the overlay is up, because a
/// window that never activates gets no key input (D-23, a prototype call).
///
/// Dropping it closes every window and gives Esc back.
pub struct Overlay {
    windows: Vec<HWND>,
}

impl Overlay {
    /// Shows `captures`, one window per monitor at that monitor's physical bounds.
    ///
    /// # Errors
    ///
    /// [`Error::Window`] if a window or its Direct2D surface cannot be made;
    /// [`Error::HotkeyTaken`] if another program holds Esc as a hotkey.
    pub fn show(
        captures: &[MonitorImage],
        on_event: impl FnMut(OverlayEvent) + 'static,
    ) -> Result<Self, Error> {
        register_class()?;
        // SAFETY: D2D1CreateFactory has no preconditions.
        let factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
                .map_err(Error::Window)?;
        let listener: Listener = Rc::new(RefCell::new(Box::new(on_event)));
        let mut overlay = Self {
            windows: Vec::with_capacity(captures.len()),
        };
        for capture in captures {
            overlay
                .windows
                .push(open_window(&factory, capture, &listener)?);
        }
        let first = *overlay
            .windows
            .first()
            .ok_or(Error::Window(windows::core::Error::empty()))?;
        // SAFETY: `first` was created above on this thread.
        unsafe {
            RegisterHotKey(
                Some(first),
                ESCAPE_HOTKEY_ID,
                MOD_NOREPEAT,
                u32::from(VK_ESCAPE.0),
            )
        }
        .map_err(Error::HotkeyTaken)?;
        for &window in &overlay.windows {
            // SAFETY: `window` was created above; showing it without activation is F-02's rule.
            let _ = unsafe { ShowWindow(window, SW_SHOWNOACTIVATE) };
        }
        Ok(overlay)
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        if let Some(&first) = self.windows.first() {
            // SAFETY: unregistering an id this window may not hold only returns an error.
            let _ = unsafe { UnregisterHotKey(Some(first), ESCAPE_HOTKEY_ID) };
        }
        for &window in &self.windows {
            // SAFETY: each window belongs to this thread and is destroyed once, here.
            let destroyed = unsafe { DestroyWindow(window) };
            debug_assert!(destroyed.is_ok(), "an overlay window is destroyed once");
        }
    }
}

fn register_class() -> Result<(), Error> {
    // SAFETY: a null module name asks for this exe's own handle.
    let instance = unsafe { GetModuleHandleW(None) }.map_err(Error::Window)?;
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).expect("fits in u32"),
        lpfnWndProc: Some(window_procedure),
        hInstance: instance.into(),
        lpszClassName: WINDOW_CLASS,
        ..Default::default()
    };
    // SAFETY: `class` is fully initialised and its strings are 'static.
    if unsafe { RegisterClassExW(&raw const class) } != 0 {
        return Ok(());
    }
    let error = windows::core::Error::from_thread();
    if error.code() == ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
        return Ok(());
    }
    Err(Error::Window(error))
}

fn open_window(
    factory: &ID2D1Factory,
    capture: &MonitorImage,
    listener: &Listener,
) -> Result<HWND, Error> {
    let bounds = capture.bounds;
    let (width, height) = (capture.image.width(), capture.image.height());
    // SAFETY: the class is registered and every string is 'static. The process is
    // per-monitor DPI aware, so the bounds are physical pixels.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            WINDOW_CLASS,
            w!("Cleanshot capture"),
            WS_POPUP,
            bounds.left,
            bounds.top,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            None,
            None,
            None,
            None,
        )
    }
    .map_err(Error::Window)?;
    match surface(factory, window, capture, width, height, listener) {
        Ok(surface) => {
            // SAFETY: `window` exists, so WM_NCDESTROY will free this Box once.
            unsafe {
                SetWindowLongPtrW(
                    window,
                    GWLP_USERDATA,
                    Box::into_raw(Box::new(surface)) as isize,
                );
            }
            Ok(window)
        }
        Err(error) => {
            // SAFETY: the window was created above and holds no state yet.
            let destroyed = unsafe { DestroyWindow(window) };
            debug_assert!(destroyed.is_ok(), "a window made here is destroyed once");
            Err(Error::Window(error))
        }
    }
}

fn surface(
    factory: &ID2D1Factory,
    window: HWND,
    capture: &MonitorImage,
    width: u32,
    height: u32,
    listener: &Listener,
) -> windows::core::Result<Surface> {
    let format = D2D1_PIXEL_FORMAT {
        format: DXGI_FORMAT_B8G8R8A8_UNORM,
        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
    };
    let target_properties = D2D1_RENDER_TARGET_PROPERTIES {
        pixelFormat: format,
        dpiX: DESKTOP_DPI,
        dpiY: DESKTOP_DPI,
        ..Default::default()
    };
    let window_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
        hwnd: window,
        pixelSize: D2D_SIZE_U { width, height },
        ..Default::default()
    };
    let bitmap_properties = D2D1_BITMAP_PROPERTIES {
        pixelFormat: format,
        dpiX: DESKTOP_DPI,
        dpiY: DESKTOP_DPI,
    };
    // Captures are opaque, so straight and premultiplied alpha agree and only the channel
    // order changes.
    let mut bgra = capture.image.pixels().to_vec();
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    // SAFETY: every description is fully initialised; `bgra` holds width * height * 4 bytes
    // with a pitch of width * 4, and Direct2D copies it during CreateBitmap.
    unsafe {
        let target = factory
            .CreateHwndRenderTarget(&raw const target_properties, &raw const window_properties)?;
        let frozen = target.CreateBitmap(
            D2D_SIZE_U { width, height },
            Some(bgra.as_ptr().cast()),
            width * 4,
            &raw const bitmap_properties,
        )?;
        let black = D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: DIM_OPACITY,
        };
        let dim = target.CreateSolidColorBrush(&raw const black, None)?;
        let accent_colour = ACCENT;
        let accent = target.CreateSolidColorBrush(&raw const accent_colour, None)?;
        Ok(Surface {
            target,
            frozen,
            dim,
            accent,
            origin: POINT {
                x: capture.bounds.left,
                y: capture.bounds.top,
            },
            drag: Cell::new(None),
            listener: Rc::clone(listener),
        })
    }
}

/// The rectangle between two corners, or None when it has no area.
fn span(anchor: POINT, current: POINT) -> Option<RECT> {
    let selected = RECT {
        left: anchor.x.min(current.x),
        top: anchor.y.min(current.y),
        right: anchor.x.max(current.x),
        bottom: anchor.y.max(current.y),
    };
    (selected.right > selected.left && selected.bottom > selected.top).then_some(selected)
}

fn cursor(lparam: LPARAM) -> POINT {
    // Client coordinates are signed 16-bit halves of lparam; captured drags go negative.
    let bytes = lparam.0.to_le_bytes();
    let (x, y) = (
        i16::from_le_bytes([bytes[0], bytes[1]]),
        i16::from_le_bytes([bytes[2], bytes[3]]),
    );
    POINT {
        x: i32::from(x),
        y: i32::from(y),
    }
}

fn redraw(window: HWND) {
    // SAFETY: invalidating this thread's own window; a failure only skips one repaint.
    let _ = unsafe { InvalidateRect(Some(window), None, false) };
}

unsafe extern "system" fn window_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: GWLP_USERDATA is 0 until open_window stores the Surface pointer, and
    // WM_NCDESTROY clears it.
    let surface = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *mut Surface;
    if surface.is_null() {
        // SAFETY: forwarding an unhandled message to the default procedure.
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    match message {
        WM_NCDESTROY => {
            // SAFETY: this is the window's last message; the Box is freed once and the pointer
            // cleared.
            unsafe {
                SetWindowLongPtrW(window, GWLP_USERDATA, 0);
                drop(Box::from_raw(surface));
                DefWindowProcW(window, message, wparam, lparam)
            }
        }
        WM_PAINT => {
            // SAFETY: a non-null pointer is live until WM_NCDESTROY.
            let surface = unsafe { &*surface };
            let mut paint = PAINTSTRUCT::default();
            // SAFETY: BeginPaint and EndPaint bracket this window's paint, as WM_PAINT needs.
            unsafe { BeginPaint(window, &raw mut paint) };
            let drawn = surface.draw();
            // SAFETY: matches the BeginPaint above.
            let _ = unsafe { EndPaint(window, &raw const paint) };
            if let Err(error) = drawn {
                surface.send(OverlayEvent::DrawFailed(error));
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            // SAFETY: a non-null pointer is live until WM_NCDESTROY.
            let surface = unsafe { &*surface };
            // SAFETY: the window belongs to this thread; capture keeps the drag going past
            // its edge.
            unsafe { SetCapture(window) };
            let at = cursor(lparam);
            surface.drag.set(Some((at, at)));
            redraw(window);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            // SAFETY: a non-null pointer is live until WM_NCDESTROY.
            let surface = unsafe { &*surface };
            // SAFETY: GetCapture only reads this thread's state.
            let dragging = unsafe { GetCapture() } == window;
            if let (Some((anchor, _)), true) = (surface.drag.get(), dragging) {
                surface.drag.set(Some((anchor, cursor(lparam))));
                redraw(window);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            // SAFETY: a non-null pointer is live until WM_NCDESTROY.
            let surface = unsafe { &*surface };
            // SAFETY: releasing a capture this thread may not hold only returns an error.
            let _ = unsafe { ReleaseCapture() };
            let selected = surface
                .drag
                .take()
                .and_then(|(anchor, _)| span(anchor, cursor(lparam)));
            redraw(window);
            // F-01: a click with no drag selects nothing and leaves the overlay up.
            if let Some(selected) = selected {
                surface.send(OverlayEvent::Selected(RECT {
                    left: selected.left + surface.origin.x,
                    top: selected.top + surface.origin.y,
                    right: selected.right + surface.origin.x,
                    bottom: selected.bottom + surface.origin.y,
                }));
            }
            LRESULT(0)
        }
        WM_HOTKEY => {
            // SAFETY: a non-null pointer is live until WM_NCDESTROY.
            unsafe { &*surface }.send(OverlayEvent::Cancelled);
            LRESULT(0)
        }
        // SAFETY: forwarding an unhandled message to the default procedure.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use cleanshot_core::RgbaImage;
    use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW};

    use super::*;

    #[test]
    fn f01_a_drag_spans_its_corners_in_either_direction_and_a_click_spans_nothing() {
        let point = |x, y| POINT { x, y };
        let expected = RECT {
            left: 10,
            top: 20,
            right: 30,
            bottom: 50,
        };
        assert_eq!(span(point(10, 20), point(30, 50)), Some(expected));
        assert_eq!(span(point(30, 50), point(10, 20)), Some(expected));
        assert_eq!(span(point(10, 20), point(10, 50)), None);
        assert_eq!(span(point(10, 20), point(10, 20)), None);
    }

    /// Red rises down the rows and green across the columns, so a pixel drawn from the wrong
    /// row or column shows a different colour.
    fn gradient(x: u32, y: u32) -> [u8; 3] {
        let (x, y) = (
            u8::try_from(x).expect("x < 64"),
            u8::try_from(y).expect("y < 48"),
        );
        [y * 5, x * 4, 200]
    }

    #[test]
    fn f02_the_overlay_draws_a_frozen_capture() {
        let (width, height) = (64, 48);
        let pixels = (0..height)
            .flat_map(|y| (0..width).flat_map(move |x| gradient(x, y).into_iter().chain([255])))
            .collect();
        // The primary monitor's top-left corner is the virtual desktop's origin.
        let capture = MonitorImage {
            bounds: RECT {
                left: 0,
                top: 0,
                right: 64,
                bottom: 48,
            },
            image: RgbaImage::new(width, height, pixels).expect("64x48 RGBA"),
        };
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&events);
        let overlay = Overlay::show(&[capture], move |event| {
            sink.borrow_mut().push(format!("{event:?}"));
        })
        .expect("the overlay opens");
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            let mut message = MSG::default();
            // SAFETY: `message` is writable and the overlay windows belong to this thread.
            while unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                // SAFETY: `message` was just filled by PeekMessageW.
                unsafe { DispatchMessageW(&raw const message) };
            }
        }
        // SAFETY: the screen DC is read here on this thread and released once below.
        let shown: Vec<[u8; 3]> = unsafe {
            let screen = GetDC(None);
            let read = [(5, 7), (40, 30), (63, 47)]
                .map(|(x, y)| {
                    let colour = GetPixel(screen, x, y).0.to_le_bytes();
                    [colour[0], colour[1], colour[2]]
                })
                .into();
            ReleaseDC(None, screen);
            read
        };
        drop(overlay);
        let dimmed: Vec<[u8; 3]> = [(5, 7), (40, 30), (63, 47)]
            .map(|(x, y)| {
                gradient(x, y).map(|channel| {
                    u8::try_from((u16::from(channel) * 3 + 2) / 5)
                        .expect("60% of a byte fits a byte")
                })
            })
            .into();
        for (shown, dimmed) in shown.iter().zip(&dimmed) {
            assert!(
                shown.iter().zip(dimmed).all(|(s, d)| s.abs_diff(*d) <= 2),
                "the screen shows {shown:?} where the dimmed capture is {dimmed:?}"
            );
        }
        assert!(
            events.borrow().is_empty(),
            "overlay events: {:?}",
            events.borrow()
        );
    }
}
