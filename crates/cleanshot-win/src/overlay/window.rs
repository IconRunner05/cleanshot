use std::rc::Rc;

use cleanshot_core::{RgbaImage, box_handles};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_BITMAP_PROPERTIES,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_PROPERTIES, ID2D1Bitmap, ID2D1Factory,
    ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW,
    SetWindowLongPtrW, WM_DESTROY, WM_GETOBJECT, WM_HOTKEY, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_NCDESTROY, WM_PAINT, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Interface, PCWSTR, w};

use super::{DESKTOP_DPI, Gesture, KEYS, Mode, OverlayEvent, Shared, dpi_scale};
use crate::Error;
use crate::capture::MonitorImage;
use crate::loupe;
use crate::palette::Palette;
use crate::selection::{Grip, as_box, dragged, grip_at, nudged};
use crate::size_pill;
use crate::struct_size;
use crate::toolbar::automation::{self, Request};
use cleanshot_core::ToolbarControl;

use crate::toolbar::{OffTip, Paint};

pub(crate) const WINDOW_CLASS: PCWSTR = w!("CleanshotOverlay");
const BORDER_WIDTH: f32 = 1.5;
const HANDLE_SIZE: f32 = 8.0;

struct Surface {
    target: ID2D1HwndRenderTarget,
    frozen: ID2D1Bitmap,
    dim: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    handle: ID2D1SolidColorBrush,
    /// Only the toolbar's monitor paints it.
    toolbar: Option<Paint>,
    /// Why Save is off, on the toolbar's monitor while it is (F-57).
    off_tip: Option<OffTip>,
    pill: size_pill::Paint,
    loupe: loupe::Paint,
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
            // F-08: a window being picked is drawn as a selection with no handles or size.
            let (shown, editable) = match self.shared.mode.get() {
                Mode::Region => (self.shared.selected.get(), true),
                Mode::Window { hovered } => (hovered.map(|pick| self.shared.frame(pick)), false),
            };
            let Some(selected) = shown else {
                self.target.FillRectangle(&raw const whole, &self.dim);
                self.draw_loupe();
                self.draw_toolbar()?;
                return self.target.EndDraw(None, None);
            };
            let on_desktop = selected;
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
            if !editable {
                self.draw_toolbar()?;
                return self.target.EndDraw(None, None);
            }
            for (_, at) in box_handles(as_box(selected)) {
                let (x, y) = (at.x, at.y);
                let square = D2D_RECT_F {
                    left: x - HANDLE_SIZE / 2.0,
                    top: y - HANDLE_SIZE / 2.0,
                    right: x + HANDLE_SIZE / 2.0,
                    bottom: y + HANDLE_SIZE / 2.0,
                };
                self.target.FillRectangle(&raw const square, &self.handle);
                self.target
                    .DrawRectangle(&raw const square, &self.accent, 1.0, None);
            }
            self.pill.draw(&self.target, on_desktop, self.origin)?;
            self.draw_loupe();
            self.draw_toolbar()?;
            self.target.EndDraw(None, None)
        }
    }

    /// A press at `at` on `window`: a toolbar control, a window picked (F-08), or the start
    /// of a drag.
    fn pressed(&self, window: HWND, at: POINT) {
        if let Some(control) = self
            .shared
            .toolbar()
            .and_then(|placed| placed.control_at(at))
        {
            return self.shared.choose(control);
        }
        if let Mode::Window { .. } = self.shared.mode.get() {
            return self.shared.pick(self.shared.picked_at(at));
        }
        // SAFETY: the window belongs to this thread; capture keeps the drag going past its edge.
        unsafe { SetCapture(window) };
        let original = self.shared.selected.get();
        self.shared.gesture.set(Some(Gesture {
            grip: grip_at(original, at),
            original,
            start: at,
            pointer: at,
        }));
        self.shared.changed();
    }

    fn moved(&self, window: HWND, now: POINT) {
        if let Mode::Window { .. } = self.shared.mode.get() {
            return self.shared.hover(self.shared.picked_at(now));
        }
        // SAFETY: GetCapture only reads this thread's state.
        let dragging = unsafe { GetCapture() } == window;
        let (Some(gesture), true) = (self.shared.gesture.get(), dragging) else {
            return;
        };
        self.shared.gesture.set(Some(Gesture {
            pointer: now,
            ..gesture
        }));
        let selected = dragged(gesture.grip, gesture.original, gesture.start, now);
        // F-01: a draw with no area clears the selection; a resize keeps its last shape.
        match (selected, gesture.grip) {
            (Some(_), _) | (None, Grip::Draw) => self.shared.select(selected),
            // F-04: the loupe follows the pointer even when the selection cannot.
            (None, Grip::Move | Grip::Resize(_)) => self.shared.changed(),
        }
    }

    /// F-04: the loupe shows only while the pointer drags.
    fn draw_loupe(&self) {
        if let Some(gesture) = self.shared.gesture.get() {
            self.loupe.draw(&self.target, &self.frozen, gesture.pointer);
        }
    }

    fn draw_toolbar(&self) -> windows::core::Result<()> {
        match (&self.toolbar, self.shared.toolbar()) {
            (Some(paint), Some(placed)) => {
                if let Some(tip) = &self.off_tip {
                    let at = placed.area(ToolbarControl::Save);
                    tip.place(RECT {
                        left: at.left - self.origin.x,
                        top: at.top - self.origin.y,
                        right: at.right - self.origin.x,
                        bottom: at.bottom - self.origin.y,
                    });
                }
                paint.draw(&placed, self.origin, self.shared.focus.get(), |control| {
                    self.shared.mirror.off(control).is_some()
                })
            }
            (Some(_) | None, _) => Ok(()),
        }
    }
}

pub(super) fn register_class() -> Result<(), Error> {
    // SAFETY: a null module name asks for this exe's own handle.
    let instance = unsafe { GetModuleHandleW(None) }.map_err(Error::Window)?;
    let class = WNDCLASSEXW {
        cbSize: struct_size::<WNDCLASSEXW>(),
        lpfnWndProc: Some(window_procedure),
        hInstance: instance.into(),
        lpszClassName: WINDOW_CLASS,
        ..Default::default()
    };
    crate::register_window_class(&class)
}

pub(super) fn open_window(
    factory: &ID2D1Factory,
    capture: &MonitorImage,
    shared: &Rc<Shared>,
    hosts_toolbar: bool,
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
    match surface(
        factory,
        window,
        capture,
        width,
        height,
        shared,
        hosts_toolbar,
    ) {
        Ok(surface) => {
            // SAFETY: `window` exists, so WM_NCDESTROY will give up this count once.
            unsafe {
                SetWindowLongPtrW(
                    window,
                    GWLP_USERDATA,
                    Rc::into_raw(Rc::new(surface)) as isize,
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

/// Pays Direct2D's and DirectWrite's one-time start-up on this thread, on a window that is
/// never shown, so the first overlay does not.
pub(super) fn prewarm(factory: &ID2D1Factory, palette: &Palette) -> windows::core::Result<()> {
    // SAFETY: the class is registered and every string is 'static. Without WS_VISIBLE the
    // window is never shown, and WS_EX_TOOLWINDOW keeps it off the taskbar and Alt+Tab.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            WINDOW_CLASS,
            w!(""),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            None,
            None,
        )
    }?;
    let warmed = target_and_bitmap(
        factory,
        window,
        D2D_SIZE_U {
            width: 1,
            height: 1,
        },
        (1, 1),
        &[0; 4],
    )
    .and_then(|(target, frozen)| Paint::new(&target.cast()?, &frozen, 1.0, palette).map(drop));
    // SAFETY: the window was created above and holds no state.
    let destroyed = unsafe { DestroyWindow(window) };
    debug_assert!(destroyed.is_ok(), "a window made here is destroyed once");
    warmed
}

/// A render target for `window`, `target` physical pixels, and a bitmap on it `width` x
/// `height` of `bgra`, rows of premultiplied BGRA `width` pixels long.
pub(crate) fn target_and_bitmap(
    factory: &ID2D1Factory,
    window: HWND,
    target: D2D_SIZE_U,
    (width, height): (u32, u32),
    bgra: &[u8],
) -> windows::core::Result<(ID2D1HwndRenderTarget, ID2D1Bitmap)> {
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
        pixelSize: target,
        ..Default::default()
    };
    let bitmap_properties = D2D1_BITMAP_PROPERTIES {
        pixelFormat: format,
        dpiX: DESKTOP_DPI,
        dpiY: DESKTOP_DPI,
    };
    assert_eq!(
        bgra.len(),
        width as usize * height as usize * 4,
        "bgra is width x height pixels"
    );
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
        Ok((target, frozen))
    }
}

fn surface(
    factory: &ID2D1Factory,
    window: HWND,
    capture: &MonitorImage,
    width: u32,
    height: u32,
    shared: &Rc<Shared>,
    hosts_toolbar: bool,
) -> windows::core::Result<Surface> {
    let bgra = bgra(&capture.image);
    let (target, frozen) = target_and_bitmap(
        factory,
        window,
        D2D_SIZE_U { width, height },
        (width, height),
        &bgra,
    )?;
    // SAFETY: every call takes owned or borrowed COM objects that outlive it.
    unsafe {
        let palette = shared.palette;
        let dim = target.CreateSolidColorBrush(&raw const palette.dim, None)?;
        let accent = target.CreateSolidColorBrush(&raw const palette.accent, None)?;
        let handle = target.CreateSolidColorBrush(&raw const palette.handle, None)?;
        let toolbar = match (hosts_toolbar, shared.toolbar) {
            (true, Some((_, scale))) => {
                Some(Paint::new(&target.cast()?, &frozen, scale, &palette)?)
            }
            (true | false, _) => None,
        };
        let off_tip = match (&toolbar, shared.mirror.off(ToolbarControl::Save)) {
            (Some(_), Some(reason)) => Some(OffTip::new(window, reason)?),
            (Some(_) | None, _) => None,
        };
        let scale = dpi_scale(capture.bounds)?;
        Ok(Surface {
            off_tip,
            pill: size_pill::Paint::new(&target, capture.bounds, scale, &palette)?,
            loupe: loupe::Paint::new(&target, bgra, capture.bounds, scale, &palette)?,
            toolbar,
            target,
            frozen,
            dim,
            accent,
            handle,
            origin: POINT {
                x: capture.bounds.left,
                y: capture.bounds.top,
            },
            shared: Rc::clone(shared),
        })
    }
}

/// The pixels of an opaque `image` as premultiplied BGRA. Captures are opaque, so straight and
/// premultiplied alpha agree and only the channel order changes.
pub(crate) fn bgra(image: &RgbaImage) -> Vec<u8> {
    let mut bgra = image.pixels().to_vec();
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    bgra
}

/// The client point of a mouse message's `lparam`.
pub(crate) fn client_point(lparam: LPARAM) -> POINT {
    // Client coordinates are signed 16-bit halves of lparam; captured drags go negative.
    let bytes = lparam.0.to_le_bytes();
    POINT {
        x: i32::from(i16::from_le_bytes([bytes[0], bytes[1]])),
        y: i32::from(i16::from_le_bytes([bytes[2], bytes[3]])),
    }
}

/// The desktop point of a mouse message's `lparam` on a window whose top-left is `origin`.
fn desktop_point(lparam: LPARAM, origin: POINT) -> POINT {
    let client = client_point(lparam);
    POINT {
        x: client.x + origin.x,
        y: client.y + origin.y,
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
    // takes a count of its own, so an Overlay dropped while it runs (from its listener, during
    // an event) frees the Surface, and the Shared it holds, only once the message returns.
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
                surface.shared.send(OverlayEvent::DrawFailed(error));
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            surface.pressed(window, desktop_point(lparam, surface.origin));
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            surface.moved(window, desktop_point(lparam, surface.origin));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            // SAFETY: releasing a capture this thread may not hold only returns an error.
            let _ = unsafe { ReleaseCapture() };
            surface.shared.gesture.set(None);
            surface.shared.changed();
            LRESULT(0)
        }
        WM_HOTKEY => {
            if let Some(&(_, _, key)) = KEYS.get(wparam.0) {
                surface.shared.press(key);
            }
            LRESULT(0)
        }
        WM_GETOBJECT => {
            let answered = surface
                .toolbar
                .as_ref()
                .and_then(|_| automation::answer(window, &surface.shared.mirror, wparam, lparam));
            // SAFETY: forwarding a request this window has no provider for.
            answered.unwrap_or_else(|| unsafe { DefWindowProcW(window, message, wparam, lparam) })
        }
        WM_DESTROY => {
            automation::forget(window);
            LRESULT(0)
        }
        automation::WAKE => {
            let shared = &surface.shared;
            for request in shared.mirror.requests() {
                match request {
                    Request::Choose(control) => shared.choose(control),
                    Request::Focus(control) => shared.focus(control),
                }
            }
            LRESULT(0)
        }
        // SAFETY: forwarding an unhandled message to the default procedure.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f03_a_press_on_a_monitor_left_of_and_above_the_primary_reads_in_desktop_pixels() {
        let origin = POINT { x: -1920, y: -1080 };
        let inside = LPARAM(20 * 0x1_0000 + 10);
        assert_eq!(desktop_point(inside, origin), POINT { x: -1910, y: -1060 });
        // A captured drag past the window's top-left: client x -5, y -3.
        let past = LPARAM(0xFFFD * 0x1_0000 + 0xFFFB);
        assert_eq!(desktop_point(past, origin), POINT { x: -1925, y: -1083 });
    }
}
