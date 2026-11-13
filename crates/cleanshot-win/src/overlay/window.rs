use std::cell::RefCell;
use std::rc::Rc;

use cleanshot_core::box_handles;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, ID2D1Bitmap, ID2D1Factory,
    ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, WM_DESTROY, WM_GETOBJECT, WM_HOTKEY,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Interface, PCWSTR, w};

use super::{Gesture, KEYS, Mode, OverlayEvent, Shared, Stages};
use crate::Error;
use crate::capture::{MonitorImage, dpi_scale};
use crate::loupe;
use crate::palette::Palette;
use crate::render_target::target_and_bitmap;
use crate::selection::{Grip, as_box, dragged, grip_at, nudged};
use crate::size_pill;
use crate::toolbar::automation::{self, Request};
use cleanshot_core::Toolbar;

use crate::toolbar::tips::{Tips, toolbar_tips};
use crate::toolbar::{self, ControlFace, Mark, Paint};
use crate::window_state::{self, desktop_point, left_the_press};

pub(crate) const WINDOW_CLASS: PCWSTR = w!("CleanshotOverlay");
const BORDER_WIDTH: f32 = 1.5;
const HANDLE_SIZE: f32 = 8.0;

struct Surface {
    target: ID2D1HwndRenderTarget,
    frozen: ID2D1Bitmap,
    dim: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    handle: ID2D1SolidColorBrush,
    /// Made the first time the toolbar comes to this monitor.
    toolbar: RefCell<Option<Paint>>,
    /// The toolbar's tooltips, made on the first move over its monitor, so they cost
    /// the first frame nothing, and dropped when the toolbar leaves it.
    tips: RefCell<Option<Tips>>,
    window: HWND,
    monitor: RECT,
    scale: f32,
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
            // A window being picked is drawn as a selection with no handles or size.
            let (shown, editable) = match (self.shared.mode.get(), self.shared.selected.get()) {
                (Mode::Region { .. }, Some(selected)) => (Some(selected), true),
                (Mode::Region { hovered } | Mode::Window { hovered }, None)
                | (Mode::Window { hovered }, Some(_)) => {
                    (hovered.map(|pick| self.shared.frame(pick)), false)
                }
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

    /// A press at `at` on `window`: a toolbar control, a window picked, or the start
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
            return self.pick_with_pointer(at);
        }
        // SAFETY: the window belongs to this thread; capture keeps the drag going past its edge.
        unsafe { SetCapture(window) };
        let original = self.shared.selected.get();
        let grip = grip_at(original, at);
        self.shared.mode.set(Mode::Region { hovered: None });
        self.shared.gesture.set(Some(Gesture {
            grip,
            original,
            start: at,
            pointer: at,
            clicking: match grip {
                Grip::Draw => true,
                Grip::Move | Grip::Resize(_) => false,
            },
        }));
        self.shared.changed();
    }

    fn moved(&self, window: HWND, now: POINT) {
        if let Err(error) = self.take_toolbar(window) {
            return self.shared.send(OverlayEvent::DrawFailed(error));
        }
        if self.tips.borrow().is_none() {
            self.make_tips(window);
        }
        if let Mode::Window { .. } = self.shared.mode.get() {
            return self.shared.hover(self.shared.picked_at(now));
        }
        // SAFETY: GetCapture only reads this thread's state.
        let dragging = unsafe { GetCapture() } == window;
        let (Some(gesture), true) = (self.shared.gesture.get(), dragging) else {
            return self.shared.hover_region(now);
        };
        let clicking = gesture.clicking && !left_the_press(gesture.start, now);
        self.shared.gesture.set(Some(Gesture {
            pointer: now,
            clicking,
            ..gesture
        }));
        if clicking {
            return self.shared.changed();
        }
        let selected = dragged(gesture.grip, gesture.original, gesture.start, now);
        // A draw with no area clears the selection; a resize keeps its last shape.
        match (selected, gesture.grip) {
            (Some(_), _) | (None, Grip::Draw) => self.shared.select(selected),
            // The loupe follows the pointer even when the selection cannot.
            (None, Grip::Move | Grip::Resize(_)) => self.shared.changed(),
        }
    }

    /// The end of a press at `at`: a press that never left Windows' drag distance picks the
    /// window under it.
    fn released(&self, at: POINT) {
        let Some(gesture) = self.shared.gesture.take() else {
            return self.shared.changed();
        };
        if gesture.clicking {
            return self.pick_with_pointer(at);
        }
        self.shared.changed();
        if gesture.grip == Grip::Draw && self.shared.selected.get().is_some() {
            self.shared.selected_with_pointer();
        }
    }

    /// Brings the toolbar to this window, making its paint the first time, outside any draw.
    fn take_toolbar(&self, window: HWND) -> windows::core::Result<()> {
        if self.toolbar.borrow().is_none() {
            let paint = Paint::new(
                &self.target.cast()?,
                Some(&self.frozen),
                self.scale,
                &self.shared.palette,
            )?;
            *self.toolbar.borrow_mut() = Some(paint);
        }
        self.shared.host_toolbar(window, self.monitor, self.scale);
        Ok(())
    }

    fn pick_with_pointer(&self, at: POINT) {
        let picked = self.shared.picked_at(at);
        self.shared.pick(picked);
        if picked.is_some() {
            self.shared.selected_with_pointer();
        }
    }

    /// The loupe shows only while the pointer drags.
    fn draw_loupe(&self) {
        if let Some(gesture) = self.shared.gesture.get() {
            self.loupe.draw(&self.target, &self.frozen, gesture.pointer);
        }
    }

    fn draw_toolbar(&self) -> windows::core::Result<()> {
        if self.shared.toolbar_window.get() != Some(self.window) {
            self.tips.borrow_mut().take();
            return Ok(());
        }
        let placed = self.shared.toolbar();
        if let Some(tips) = &*self.tips.borrow() {
            tips.place(
                placed
                    .iter()
                    .flat_map(|placed| placed.client_areas(self.origin)),
            );
        }
        let toolbar = self.toolbar.borrow();
        let (Some(paint), Some(placed)) = (&*toolbar, placed) else {
            return Ok(());
        };
        paint.draw(&placed, self.origin, self.shared.focus.get(), |control| {
            ControlFace {
                label: toolbar::label(control).into(),
                mark: match self.shared.mirror.off(control) {
                    Some(_) => Mark::Off,
                    None => Mark::Plain,
                },
            }
        })
    }

    fn make_tips(&self, window: HWND) {
        let mirror = &self.shared.mirror;
        let texts = toolbar_tips(Toolbar::Overlay, |control| {
            mirror.off(control).map(str::to_owned)
        });
        match Tips::new(window, &texts) {
            Ok(tips) => {
                if let Some(placed) = self.shared.toolbar() {
                    tips.place(placed.client_areas(self.origin));
                }
                *self.tips.borrow_mut() = Some(tips);
            }
            Err(error) => self.shared.send(OverlayEvent::DrawFailed(error)),
        }
    }
}

pub(super) fn register_class() -> Result<(), Error> {
    crate::register_window_class(WINDOW_CLASS, window_procedure)
}

pub(super) fn open_window(
    factory: &ID2D1Factory,
    capture: &MonitorImage,
    shared: &Rc<Shared>,
    hosts_toolbar: bool,
    stages: &mut Stages,
) -> Result<HWND, Error> {
    let bounds = capture.bounds;
    let (width, height) = (capture.frame.width(), capture.frame.height());
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
    stages.mark("window_created");
    match surface(
        factory,
        window,
        capture,
        (width, height),
        shared,
        hosts_toolbar,
        stages,
    ) {
        Ok(surface) => {
            // SAFETY: `window` was created above with nothing stored on it, and
            // window_procedure takes every message's Surface through window_state.
            unsafe { window_state::attach(window, Rc::new(surface)) };
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
pub(super) fn prewarm(
    factory: &ID2D1Factory,
    palette: &Palette,
    mark: &mut impl FnMut(&'static str),
) -> windows::core::Result<()> {
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
        D2D1_ALPHA_MODE_IGNORE,
    )
    .and_then(|(target, frozen)| {
        mark("target");
        Paint::new(&target.cast()?, Some(&frozen), 1.0, palette)?;
        mark("paint");
        // Direct2D makes a target's device at its first draw, not when the target is made.
        // SAFETY: BeginDraw and EndDraw bracket one draw on a target this thread made.
        unsafe {
            target.BeginDraw();
            target.Clear(None);
            target.EndDraw(None, None)
        }?;
        mark("first_draw");
        Ok(())
    });
    // SAFETY: the window was created above and holds no state.
    let destroyed = unsafe { DestroyWindow(window) };
    debug_assert!(destroyed.is_ok(), "a window made here is destroyed once");
    warmed
}

fn surface(
    factory: &ID2D1Factory,
    window: HWND,
    capture: &MonitorImage,
    (width, height): (u32, u32),
    shared: &Rc<Shared>,
    hosts_toolbar: bool,
    stages: &mut Stages,
) -> windows::core::Result<Surface> {
    // The frozen frame is drawn in the order Windows copied it, with no pass over it.
    let (target, frozen) = target_and_bitmap(
        factory,
        window,
        D2D_SIZE_U { width, height },
        (width, height),
        capture.frame.pixels(),
        D2D1_ALPHA_MODE_IGNORE,
    )?;
    stages.mark("target_and_bitmap");
    // SAFETY: every call takes owned or borrowed COM objects that outlive it.
    unsafe {
        let palette = shared.palette;
        let dim = target.CreateSolidColorBrush(&raw const palette.dim, None)?;
        let accent = target.CreateSolidColorBrush(&raw const palette.accent, None)?;
        let handle = target.CreateSolidColorBrush(&raw const palette.handle, None)?;
        let scale = dpi_scale(capture.bounds)?;
        let toolbar = if hosts_toolbar {
            Some(Paint::new(&target.cast()?, Some(&frozen), scale, &palette)?)
        } else {
            None
        };
        let pill = size_pill::Paint::new(&target, capture.bounds, scale, &palette)?;
        let loupe = loupe::Paint::new(
            &target,
            capture.frame.pixels().to_vec(),
            capture.bounds,
            scale,
            &palette,
        )?;
        stages.mark("paints");
        Ok(Surface {
            tips: RefCell::new(None),
            pill,
            loupe,
            toolbar: RefCell::new(toolbar),
            window,
            monitor: capture.bounds,
            scale,
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

unsafe extern "system" fn window_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: open_window stores a Surface on every window of this class. The count this
    // message holds keeps an Overlay dropped while it runs (from its listener, during an
    // event) from freeing the Surface, and the Shared it holds, until the message returns.
    let Some(surface) = (unsafe { window_state::for_message::<Surface>(window, message) }) else {
        // SAFETY: forwarding an unhandled message to the default procedure.
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
            surface.released(desktop_point(lparam, surface.origin));
            LRESULT(0)
        }
        WM_HOTKEY => {
            if let Some(&(_, _, key)) = KEYS.get(wparam.0) {
                surface.shared.press(key);
            }
            LRESULT(0)
        }
        WM_GETOBJECT => {
            let answered = (surface.shared.toolbar_window.get() == Some(window))
                .then(|| automation::answer(window, &surface.shared.mirror, wparam, lparam))
                .flatten();
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
