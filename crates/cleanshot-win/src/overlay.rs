use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cleanshot_core::box_handles;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
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
    GetCapture, HOT_KEY_MODIFIERS, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey,
    ReleaseCapture, SetCapture, UnregisterHotKey, VIRTUAL_KEY, VK_DOWN, VK_ESCAPE, VK_LEFT,
    VK_RETURN, VK_RIGHT, VK_S, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW,
    SW_SHOWNOACTIVATE, SetWindowLongPtrW, ShowWindow, WM_HOTKEY, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_NCDESTROY, WM_PAINT, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::Error;
use crate::capture::MonitorImage;
use crate::selection::{Grip, as_box, dragged, grip_at, nudged};

const WINDOW_CLASS: PCWSTR = w!("CleanshotOverlay");
const DIM_OPACITY: f32 = 0.4;
const DESKTOP_DPI: f32 = 96.0;
const BORDER_WIDTH: f32 = 1.5;
const HANDLE_SIZE: f32 = 8.0;
const SHIFT_NUDGE: i32 = 10;
const WHITE: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};
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
    /// Enter was pressed on this selection, in physical virtual-desktop pixels (F-03, F-05).
    Copy(RECT),
    /// Ctrl+S was pressed on this selection, in physical virtual-desktop pixels (F-05).
    Save(RECT),
    /// Direct2D could not draw a monitor's frozen image.
    DrawFailed(windows::core::Error),
}

/// A key the overlay holds as a hotkey while it is up (D-23).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Cancel,
    Copy,
    Save,
    Nudge(i32, i32),
}

const KEYS: [(HOT_KEY_MODIFIERS, VIRTUAL_KEY, Key); 11] = [
    (MOD_NOREPEAT, VK_ESCAPE, Key::Cancel),
    (MOD_NOREPEAT, VK_RETURN, Key::Copy),
    (
        HOT_KEY_MODIFIERS(MOD_NOREPEAT.0 | MOD_CONTROL.0),
        VK_S,
        Key::Save,
    ),
    (HOT_KEY_MODIFIERS(0), VK_LEFT, Key::Nudge(-1, 0)),
    (HOT_KEY_MODIFIERS(0), VK_RIGHT, Key::Nudge(1, 0)),
    (HOT_KEY_MODIFIERS(0), VK_UP, Key::Nudge(0, -1)),
    (HOT_KEY_MODIFIERS(0), VK_DOWN, Key::Nudge(0, 1)),
    (MOD_SHIFT, VK_LEFT, Key::Nudge(-SHIFT_NUDGE, 0)),
    (MOD_SHIFT, VK_RIGHT, Key::Nudge(SHIFT_NUDGE, 0)),
    (MOD_SHIFT, VK_UP, Key::Nudge(0, -SHIFT_NUDGE)),
    (MOD_SHIFT, VK_DOWN, Key::Nudge(0, SHIFT_NUDGE)),
];

#[derive(Clone, Copy, Debug)]
struct Gesture {
    grip: Grip,
    original: Option<RECT>,
    start: POINT,
}

/// What every monitor's window shares: one selection on the virtual desktop.
struct Shared {
    listener: RefCell<Box<dyn FnMut(OverlayEvent)>>,
    selected: Cell<Option<RECT>>,
    gesture: Cell<Option<Gesture>>,
    windows: RefCell<Vec<HWND>>,
}

impl Shared {
    fn send(&self, event: OverlayEvent) {
        if let Ok(mut listener) = self.listener.try_borrow_mut() {
            listener(event);
        }
    }

    fn select(&self, selected: Option<RECT>) {
        self.selected.set(selected);
        for &window in self.windows.borrow().iter() {
            // SAFETY: invalidating this thread's own window; a failure only skips one repaint.
            let _ = unsafe { InvalidateRect(Some(window), None, false) };
        }
    }

    fn press(&self, key: Key) {
        let selected = self.selected.get();
        match (key, selected) {
            (Key::Cancel, _) => self.send(OverlayEvent::Cancelled),
            (Key::Copy, Some(selected)) => self.send(OverlayEvent::Copy(selected)),
            (Key::Save, Some(selected)) => self.send(OverlayEvent::Save(selected)),
            (Key::Nudge(dx, dy), Some(selected)) => self.select(Some(nudged(selected, dx, dy))),
            (Key::Copy | Key::Save | Key::Nudge(..), None) => {}
        }
    }
}

struct Surface {
    target: ID2D1HwndRenderTarget,
    frozen: ID2D1Bitmap,
    dim: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    white: ID2D1SolidColorBrush,
    origin: POINT,
    shared: Rc<Shared>,
}

impl Surface {
    fn draw(&self) -> windows::core::Result<()> {
        // SAFETY: the target, bitmap and brushes come from one factory and live with `self`.
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
            let Some(selected) = self.shared.selected.get() else {
                self.target.FillRectangle(&raw const whole, &self.dim);
                return self.target.EndDraw(None, None);
            };
            let selected = nudged(selected, -self.origin.x, -self.origin.y);
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
            for (_, at) in box_handles(as_box(selected)) {
                let (x, y) = (at.x, at.y);
                let square = D2D_RECT_F {
                    left: x - HANDLE_SIZE / 2.0,
                    top: y - HANDLE_SIZE / 2.0,
                    right: x + HANDLE_SIZE / 2.0,
                    bottom: y + HANDLE_SIZE / 2.0,
                };
                self.target.FillRectangle(&raw const square, &self.white);
                self.target
                    .DrawRectangle(&raw const square, &self.accent, 1.0, None);
            }
            self.target.EndDraw(None, None)
        }
    }
}

/// One topmost, never-activated window per monitor, each showing that monitor's frozen
/// capture dimmed by 40% (F-02), with one selection drawn, moved and resized across them
/// (F-03). Its keys are hotkeys only while it is up, because a window that never activates
/// gets no key input (D-23).
///
/// Dropping it closes every window and gives its keys back.
pub struct Overlay {
    windows: Vec<HWND>,
}

impl Overlay {
    /// Shows `captures`, one window per monitor at that monitor's physical bounds.
    ///
    /// # Errors
    ///
    /// [`Error::Window`] if a window or its Direct2D surface cannot be made;
    /// [`Error::HotkeyTaken`] if another program holds one of its keys as a hotkey.
    pub fn show(
        captures: &[MonitorImage],
        on_event: impl FnMut(OverlayEvent) + 'static,
    ) -> Result<Self, Error> {
        register_class()?;
        // SAFETY: D2D1CreateFactory has no preconditions.
        let factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
                .map_err(Error::Window)?;
        let shared = Rc::new(Shared {
            listener: RefCell::new(Box::new(on_event)),
            selected: Cell::new(None),
            gesture: Cell::new(None),
            windows: RefCell::new(Vec::with_capacity(captures.len())),
        });
        let mut overlay = Self {
            windows: Vec::with_capacity(captures.len()),
        };
        for capture in captures {
            let window = open_window(&factory, capture, &shared)?;
            overlay.windows.push(window);
            shared.windows.borrow_mut().push(window);
        }
        let first = *overlay
            .windows
            .first()
            .ok_or(Error::Window(windows::core::Error::empty()))?;
        for (id, (modifiers, key, _)) in (0..).zip(KEYS) {
            // SAFETY: `first` was created above on this thread.
            unsafe { RegisterHotKey(Some(first), id, modifiers, u32::from(key.0)) }
                .map_err(Error::HotkeyTaken)?;
        }
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
            for (id, _) in (0..).zip(KEYS) {
                // SAFETY: unregistering an id this window may not hold only returns an error.
                let _ = unsafe { UnregisterHotKey(Some(first), id) };
            }
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
    crate::register_window_class(&class)
}

fn open_window(
    factory: &ID2D1Factory,
    capture: &MonitorImage,
    shared: &Rc<Shared>,
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
    match surface(factory, window, capture, width, height, shared) {
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
    shared: &Rc<Shared>,
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
        let white_colour = WHITE;
        let white = target.CreateSolidColorBrush(&raw const white_colour, None)?;
        Ok(Surface {
            target,
            frozen,
            dim,
            accent,
            white,
            origin: POINT {
                x: capture.bounds.left,
                y: capture.bounds.top,
            },
            shared: Rc::clone(shared),
        })
    }
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

impl Surface {
    fn virtual_point(&self, lparam: LPARAM) -> POINT {
        let local = cursor(lparam);
        POINT {
            x: local.x + self.origin.x,
            y: local.y + self.origin.y,
        }
    }
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
                surface.shared.send(OverlayEvent::DrawFailed(error));
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            // SAFETY: a non-null pointer is live until WM_NCDESTROY.
            let surface = unsafe { &*surface };
            // SAFETY: the window belongs to this thread; capture keeps the drag going past
            // its edge.
            unsafe { SetCapture(window) };
            let at = surface.virtual_point(lparam);
            let original = surface.shared.selected.get();
            surface.shared.gesture.set(Some(Gesture {
                grip: grip_at(original, at),
                original,
                start: at,
            }));
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            // SAFETY: a non-null pointer is live until WM_NCDESTROY.
            let surface = unsafe { &*surface };
            // SAFETY: GetCapture only reads this thread's state.
            let dragging = unsafe { GetCapture() } == window;
            if let (Some(gesture), true) = (surface.shared.gesture.get(), dragging) {
                let now = surface.virtual_point(lparam);
                let selected = dragged(gesture.grip, gesture.original, gesture.start, now);
                // F-01: a draw with no area clears the selection; a resize keeps its last shape.
                match (selected, gesture.grip) {
                    (Some(_), _) | (None, Grip::Draw) => surface.shared.select(selected),
                    (None, Grip::Move | Grip::Resize(_)) => {}
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            // SAFETY: a non-null pointer is live until WM_NCDESTROY.
            let surface = unsafe { &*surface };
            // SAFETY: releasing a capture this thread may not hold only returns an error.
            let _ = unsafe { ReleaseCapture() };
            surface.shared.gesture.set(None);
            LRESULT(0)
        }
        WM_HOTKEY => {
            // SAFETY: a non-null pointer is live until WM_NCDESTROY.
            let surface = unsafe { &*surface };
            if let Some(&(_, _, key)) = KEYS.get(wparam.0) {
                surface.shared.press(key);
            }
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
