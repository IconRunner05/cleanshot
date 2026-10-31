use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cleanshot_core::box_handles;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_BITMAP_PROPERTIES,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory,
    ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT, UpdateWindow,
};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, HOT_KEY_MODIFIERS, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey,
    ReleaseCapture, SetCapture, UnregisterHotKey, VIRTUAL_KEY, VK_DOWN, VK_E, VK_ESCAPE, VK_LEFT,
    VK_RETURN, VK_RIGHT, VK_S, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetCursorPos, GetWindowLongPtrW,
    SW_SHOWNOACTIVATE, SetWindowLongPtrW, ShowWindow, WM_DESTROY, WM_GETOBJECT, WM_HOTKEY,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCDESTROY, WM_PAINT, WNDCLASSEXW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Interface, PCWSTR, w};

use crate::Error;
use crate::capture::MonitorImage;
use crate::loupe;
use crate::palette::Palette;
use crate::selection::{Grip, as_box, dragged, grip_at, nudged};
use crate::size_pill;
use crate::toolbar::automation::{self, Mirror, Request};
use crate::toolbar::{self, Control, Layout, Paint};

const WINDOW_CLASS: PCWSTR = w!("CleanshotOverlay");
const DESKTOP_DPI: f32 = 96.0;
const BORDER_WIDTH: f32 = 1.5;
const HANDLE_SIZE: f32 = 8.0;
const SHIFT_NUDGE: i32 = 10;

/// What happened on the overlay, delivered on its thread's message loop.
#[derive(Debug)]
pub enum OverlayEvent {
    /// Esc was pressed (F-01: nothing is written).
    Cancelled,
    /// Copy was chosen for this selection, in physical virtual-desktop pixels (F-03, F-05).
    Copy(RECT),
    /// Save was chosen for this selection, in physical virtual-desktop pixels (F-05).
    Save(RECT),
    /// Edit was chosen for this selection, in physical virtual-desktop pixels (F-05).
    Edit(RECT),
    /// Direct2D could not draw a monitor's frozen image.
    DrawFailed(windows::core::Error),
}

/// A key the overlay holds as a hotkey while it is up (D-23).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    /// Chooses this toolbar control, wherever the focus is.
    Choose(Control),
    /// Chooses the focused toolbar control.
    Press,
    /// Moves the toolbar's focus this many controls along.
    Focus(i32),
    Nudge(i32, i32),
}

const KEYS: [(HOT_KEY_MODIFIERS, VIRTUAL_KEY, Key); 15] = [
    (MOD_NOREPEAT, VK_ESCAPE, Key::Choose(Control::Cancel)),
    (MOD_NOREPEAT, VK_RETURN, Key::Press),
    (MOD_NOREPEAT, VK_SPACE, Key::Press),
    (
        HOT_KEY_MODIFIERS(MOD_NOREPEAT.0 | MOD_CONTROL.0),
        VK_S,
        Key::Choose(Control::Save),
    ),
    (MOD_NOREPEAT, VK_E, Key::Choose(Control::Edit)),
    (HOT_KEY_MODIFIERS(0), VK_TAB, Key::Focus(1)),
    (MOD_SHIFT, VK_TAB, Key::Focus(-1)),
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
    pointer: POINT,
}

/// What every monitor's window shares: one selection on the virtual desktop.
struct Shared {
    listener: RefCell<Box<dyn FnMut(OverlayEvent)>>,
    selected: Cell<Option<RECT>>,
    gesture: Cell<Option<Gesture>>,
    windows: RefCell<Vec<HWND>>,
    /// The toolbar's monitor and its physical pixels per DIP (F-05).
    toolbar: Option<(RECT, f32)>,
    focus: Cell<Control>,
    mirror: Mirror,
    palette: Palette,
}

impl Shared {
    fn send(&self, event: OverlayEvent) {
        if let Ok(mut listener) = self.listener.try_borrow_mut() {
            listener(event);
        }
    }

    fn select(&self, selected: Option<RECT>) {
        self.selected.set(selected);
        self.changed();
    }

    /// Repaints every window and tells UI Automation what the toolbar now shows.
    fn changed(&self) {
        self.mirror
            .show(self.toolbar().map(|placed| (placed, self.focus.get())));
        for &window in self.windows.borrow().iter() {
            // SAFETY: invalidating this thread's own window; a failure only skips one repaint.
            let _ = unsafe { InvalidateRect(Some(window), None, false) };
        }
    }

    /// Where the toolbar sits for the current selection, if its monitor can hold it.
    fn toolbar(&self) -> Option<Layout> {
        let (monitor, scale) = self.toolbar?;
        toolbar::layout(monitor, self.selected.get(), scale)
    }

    fn press(&self, key: Key) {
        match (key, self.selected.get()) {
            (Key::Choose(control), _) => self.choose(control),
            (Key::Press, _) => self.choose(self.focus.get()),
            (Key::Focus(step), _) => self.focus(self.focus.get().step(step)),
            (Key::Nudge(dx, dy), Some(selected)) => self.select(Some(nudged(selected, dx, dy))),
            (Key::Nudge(..), None) => {}
        }
    }

    fn focus(&self, control: Control) {
        self.focus.set(control);
        self.changed();
    }

    /// F-05: Cancel always closes; the others need a selection to act on.
    fn choose(&self, control: Control) {
        match (control, self.selected.get()) {
            (Control::Cancel, _) => self.send(OverlayEvent::Cancelled),
            (Control::Copy, Some(selected)) => self.send(OverlayEvent::Copy(selected)),
            (Control::Save, Some(selected)) => self.send(OverlayEvent::Save(selected)),
            (Control::Edit, Some(selected)) => self.send(OverlayEvent::Edit(selected)),
            (Control::Copy | Control::Save | Control::Edit, None) => {}
        }
    }
}

struct Surface {
    target: ID2D1HwndRenderTarget,
    frozen: ID2D1Bitmap,
    dim: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    handle: ID2D1SolidColorBrush,
    /// Only the toolbar's monitor paints it.
    toolbar: Option<Paint>,
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
            let Some(selected) = self.shared.selected.get() else {
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

    /// F-04: the loupe shows only while the pointer drags.
    fn draw_loupe(&self) {
        if let Some(gesture) = self.shared.gesture.get() {
            self.loupe.draw(&self.target, &self.frozen, gesture.pointer);
        }
    }

    fn draw_toolbar(&self) -> windows::core::Result<()> {
        match (&self.toolbar, self.shared.toolbar()) {
            (Some(paint), Some(placed)) => {
                paint.draw(&placed, self.origin, self.shared.focus.get())
            }
            (Some(_) | None, _) => Ok(()),
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
    /// [`Error::Window`] if a window or its Direct2D surface cannot be made, or the pointer's
    /// position or its monitor's DPI cannot be read;
    /// [`Error::HotkeyTaken`] if another program holds one of its keys as a hotkey.
    pub fn show(
        captures: &[MonitorImage],
        selected: Option<RECT>,
        on_event: impl FnMut(OverlayEvent) + 'static,
    ) -> Result<Self, Error> {
        register_class()?;
        // SAFETY: D2D1CreateFactory has no preconditions.
        let factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
                .map_err(Error::Window)?;
        let host = toolbar_monitor(captures).map_err(Error::Window)?;
        let toolbar = match host {
            Some(capture) => Some((
                capture.bounds,
                dpi_scale(capture.bounds).map_err(Error::Window)?,
            )),
            None => None,
        };
        let shared = Rc::new(Shared {
            listener: RefCell::new(Box::new(on_event)),
            selected: Cell::new(selected),
            gesture: Cell::new(None),
            windows: RefCell::new(Vec::with_capacity(captures.len())),
            toolbar,
            focus: Cell::new(Control::Copy),
            mirror: Mirror::default(),
            palette: Palette::current().map_err(Error::Window)?,
        });
        shared.changed();
        let mut overlay = Self {
            windows: Vec::with_capacity(captures.len()),
        };
        for capture in captures {
            let hosts_toolbar = host.is_some_and(|host| std::ptr::eq(host, capture));
            let window = open_window(&factory, capture, &shared, hosts_toolbar)?;
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
            // P-01: the first frame is painted before show returns, so a caller timing show
            // times the overlay becoming visible.
            // SAFETY: `window` was created above on this thread.
            unsafe { UpdateWindow(window) }
                .ok()
                .map_err(Error::Window)?;
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

/// F-05: the capture under the pointer, or the first when the pointer is on none.
fn toolbar_monitor(captures: &[MonitorImage]) -> windows::core::Result<Option<&MonitorImage>> {
    let mut pointer = POINT::default();
    // SAFETY: `pointer` is writable.
    unsafe { GetCursorPos(&raw mut pointer) }?;
    let found = captures.iter().find(|capture| {
        let bounds = capture.bounds;
        (bounds.left..bounds.right).contains(&pointer.x)
            && (bounds.top..bounds.bottom).contains(&pointer.y)
    });
    Ok(found.or(captures.first()))
}

/// Physical pixels per DIP on the monitor holding `bounds`.
#[expect(clippy::cast_precision_loss, reason = "a DPI is a few hundred")]
fn dpi_scale(bounds: RECT) -> windows::core::Result<f32> {
    let (mut x, mut y) = (0, 0);
    // SAFETY: both outputs are writable locals; MonitorFromRect always returns a monitor here.
    unsafe {
        GetDpiForMonitor(
            MonitorFromRect(&raw const bounds, MONITOR_DEFAULTTONEAREST),
            MDT_EFFECTIVE_DPI,
            &raw mut x,
            &raw mut y,
        )
    }?;
    Ok(x as f32 / DESKTOP_DPI)
}

fn open_window(
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
    hosts_toolbar: bool,
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
        let scale = dpi_scale(capture.bounds)?;
        Ok(Surface {
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
            let at = surface.virtual_point(lparam);
            if let Some(control) = surface
                .shared
                .toolbar()
                .and_then(|placed| placed.control_at(at))
            {
                surface.shared.choose(control);
                return LRESULT(0);
            }
            // SAFETY: the window belongs to this thread; capture keeps the drag going past
            // its edge.
            unsafe { SetCapture(window) };
            let original = surface.shared.selected.get();
            surface.shared.gesture.set(Some(Gesture {
                grip: grip_at(original, at),
                original,
                start: at,
                pointer: at,
            }));
            surface.shared.changed();
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            // SAFETY: a non-null pointer is live until WM_NCDESTROY.
            let surface = unsafe { &*surface };
            // SAFETY: GetCapture only reads this thread's state.
            let dragging = unsafe { GetCapture() } == window;
            if let (Some(gesture), true) = (surface.shared.gesture.get(), dragging) {
                let now = surface.virtual_point(lparam);
                surface.shared.gesture.set(Some(Gesture {
                    pointer: now,
                    ..gesture
                }));
                let selected = dragged(gesture.grip, gesture.original, gesture.start, now);
                // F-01: a draw with no area clears the selection; a resize keeps its last shape.
                match (selected, gesture.grip) {
                    (Some(_), _) | (None, Grip::Draw) => surface.shared.select(selected),
                    // F-04: the loupe follows the pointer even when the selection cannot.
                    (None, Grip::Move | Grip::Resize(_)) => surface.shared.changed(),
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
            surface.shared.changed();
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
        WM_GETOBJECT => {
            // SAFETY: a non-null pointer is live until WM_NCDESTROY.
            let surface = unsafe { &*surface };
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
            // SAFETY: a non-null pointer is live until WM_NCDESTROY.
            let shared = &unsafe { &*surface }.shared;
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
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use cleanshot_core::RgbaImage;
    use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, GetUpdateRect, ReleaseDC};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, PostMessageW,
    };

    use super::*;

    /// An overlay registers its keys as system-wide hotkeys and covers the same corner of the
    /// screen, so one test's overlay is up at a time.
    static SCREEN: Mutex<()> = Mutex::new(());

    /// Red rises down the rows and green across the columns, so a pixel drawn from the wrong
    /// row or column shows a different colour.
    fn gradient(x: u32, y: u32) -> [u8; 3] {
        let (x, y) = (
            u8::try_from(x).expect("x < 64"),
            u8::try_from(y).expect("y < 48"),
        );
        [y * 5, x * 4, 200]
    }

    /// A 64x48 gradient capture at the primary monitor's top-left corner, the virtual
    /// desktop's origin.
    fn monitor() -> MonitorImage {
        let (width, height) = (64, 48);
        let pixels = (0..height)
            .flat_map(|y| (0..width).flat_map(move |x| gradient(x, y).into_iter().chain([255])))
            .collect();
        MonitorImage {
            bounds: RECT {
                left: 0,
                top: 0,
                right: 64,
                bottom: 48,
            },
            image: RgbaImage::new(width, height, pixels).expect("64x48 RGBA"),
        }
    }

    fn pump(duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            let mut message = MSG::default();
            // SAFETY: `message` is writable and the overlay windows belong to this thread.
            while unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                // SAFETY: `message` was just filled by PeekMessageW.
                unsafe { DispatchMessageW(&raw const message) };
            }
        }
    }

    #[test]
    fn f02_the_overlay_draws_a_frozen_capture() {
        let _screen = SCREEN
            .lock()
            .expect("no overlay test panicked holding the screen");
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&events);
        let overlay = Overlay::show(&[monitor()], None, move |event| {
            sink.borrow_mut().push(format!("{event:?}"));
        })
        .expect("the overlay opens");
        pump(Duration::from_millis(500));
        let points = [(5, 7), (40, 30), (63, 47)];
        // SAFETY: the screen DC is read here on this thread and released once below.
        let shown = unsafe {
            let screen = GetDC(None);
            let read = points.map(|(x, y)| {
                let colour = GetPixel(screen, x, y).0.to_le_bytes();
                [colour[0], colour[1], colour[2]]
            });
            ReleaseDC(None, screen);
            read
        };
        drop(overlay);
        let dimmed = points.map(|(x, y)| {
            gradient(x.unsigned_abs(), y.unsigned_abs()).map(|channel| {
                u8::try_from((u16::from(channel) * 3 + 2) / 5).expect("60% of a byte fits a byte")
            })
        });
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

    #[test]
    fn p01_show_returns_with_every_window_painted() {
        let _screen = SCREEN
            .lock()
            .expect("no overlay test panicked holding the screen");
        let overlay = Overlay::show(&[monitor()], None, |_| {}).expect("the overlay opens");
        let pending: Vec<bool> = overlay
            .windows
            .iter()
            // SAFETY: a read of this thread's own window's update region.
            .map(|&window| unsafe { GetUpdateRect(window, None, false) }.as_bool())
            .collect();
        drop(overlay);
        assert!(
            pending.iter().all(|&waiting| !waiting),
            "windows still waiting for their first paint: {pending:?}"
        );
    }

    #[test]
    fn f07_an_overlay_opened_with_a_selection_copies_it_on_enter() {
        let _screen = SCREEN
            .lock()
            .expect("no overlay test panicked holding the screen");
        let screen = monitor().bounds;
        let copied = Rc::new(Cell::new(None));
        let sink = Rc::clone(&copied);
        let overlay = Overlay::show(&[monitor()], Some(screen), move |event| {
            if let OverlayEvent::Copy(selected) = event {
                sink.set(Some(selected));
            }
        })
        .expect("the overlay opens");
        let enter = KEYS
            .iter()
            .position(|&(_, _, key)| matches!(key, Key::Press))
            .expect("Enter is one of the overlay's keys");
        // SAFETY: posting copies no memory, and the window belongs to this thread.
        unsafe {
            PostMessageW(
                Some(overlay.windows[0]),
                WM_HOTKEY,
                WPARAM(enter),
                LPARAM(0),
            )
        }
        .expect("the overlay window takes the message");
        pump(Duration::from_millis(200));
        drop(overlay);
        assert_eq!(copied.get(), Some(screen));
    }

    /// Whether another registration holds `key`: a thread hotkey for it is refused.
    fn held(modifiers: HOT_KEY_MODIFIERS, key: VIRTUAL_KEY) -> bool {
        const PROBE: i32 = 0xBFFF;
        // SAFETY: a thread hotkey needs no window; a granted one is released at once.
        unsafe {
            if RegisterHotKey(None, PROBE, modifiers, u32::from(key.0)).is_err() {
                return true;
            }
            UnregisterHotKey(None, PROBE).expect("the probe hotkey was just registered");
        }
        false
    }

    #[test]
    fn f05_the_toolbar_keys_are_free_again_after_esc() {
        let _screen = SCREEN
            .lock()
            .expect("no overlay test panicked holding the screen");
        let cancelled = Rc::new(Cell::new(false));
        let sink = Rc::clone(&cancelled);
        let overlay = Overlay::show(&[monitor()], None, move |event| {
            sink.set(matches!(event, OverlayEvent::Cancelled));
        })
        .expect("the overlay opens");
        assert!(
            held(HOT_KEY_MODIFIERS(0), VK_TAB),
            "Tab is the overlay's while it is up"
        );
        let escape = KEYS
            .iter()
            .position(|&(_, _, key)| matches!(key, Key::Choose(Control::Cancel)))
            .expect("Esc is one of the overlay's keys");
        // SAFETY: posting copies no memory, and the window belongs to this thread.
        unsafe {
            PostMessageW(
                Some(overlay.windows[0]),
                WM_HOTKEY,
                WPARAM(escape),
                LPARAM(0),
            )
        }
        .expect("the overlay window takes the message");
        pump(Duration::from_millis(200));
        assert!(cancelled.get(), "Esc cancels");
        drop(overlay);
        for (modifiers, key, _) in KEYS {
            assert!(!held(modifiers, key), "{key:?} is still held after Esc");
        }
    }

    #[test]
    fn f05_an_overlay_that_cannot_take_its_keys_frees_the_ones_it_took() {
        const TAKEN: i32 = 0xBFFE;
        let _screen = SCREEN
            .lock()
            .expect("no overlay test panicked holding the screen");
        let (modifiers, last, _) = KEYS[KEYS.len() - 1];
        // SAFETY: a thread hotkey needs no window; it is released below.
        unsafe { RegisterHotKey(None, TAKEN, modifiers, u32::from(last.0)) }
            .expect("no other program holds the overlay's last key");
        let shown = Overlay::show(&[monitor()], None, |_| {});
        // SAFETY: TAKEN was registered above on this thread.
        unsafe { UnregisterHotKey(None, TAKEN) }.expect("the taken key was registered");
        assert!(
            matches!(shown, Err(Error::HotkeyTaken(_))),
            "the overlay reports the taken key"
        );
        for (modifiers, key, _) in KEYS {
            assert!(
                !held(modifiers, key),
                "{key:?} is still held after the failed open"
            );
        }
    }
}
