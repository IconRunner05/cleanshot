use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cleanshot_core::{
    CapturePanel, OptionsEntry, PanelOutcome, TabDirection, Toolbar, ToolbarControl,
};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_U;
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget,
};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_ESCAPE, VK_RETURN, VK_SHIFT, VK_SPACE, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetForegroundWindow, SW_HIDE, SW_SHOW,
    SetForegroundWindow, SetWindowDisplayAffinity, ShowWindow, WDA_EXCLUDEFROMCAPTURE, WM_DESTROY,
    WM_GETOBJECT, WM_KEYDOWN, WM_LBUTTONDOWN, WM_PAINT, WNDCLASSEXW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Interface, PCWSTR, w};

use super::{PanelEvent, timer_label};
use crate::Error;
use crate::capture::dpi_scale;
use crate::palette::Palette;
use crate::render_target::target;
use crate::struct_size;
use crate::toolbar::automation::{self, Change, Mirror, Request};
use crate::toolbar::{self, Layout, Look, Mark, Paint, options};
use crate::window_state::{self, desktop_point};

const WINDOW_CLASS: PCWSTR = w!("CleanshotCapturePanel");

type Listener = Box<dyn FnMut(PanelEvent)>;
/// The Options menu as it stands, read each time it opens (F-12).
pub(super) type Options = Box<dyn Fn() -> Vec<OptionsEntry>>;

struct Surface {
    target: ID2D1HwndRenderTarget,
    paint: Paint,
    layout: Layout,
    panel: Cell<CapturePanel>,
    focus: Cell<ToolbarControl>,
    mirror: Mirror,
    /// The window that had the focus before the panel took it, given it back on close.
    previous: HWND,
    /// Set by the press that ends the panel; every key, click and request after it is ignored.
    closed: Cell<bool>,
    listener: RefCell<Listener>,
    options: Options,
}

impl Surface {
    fn draw(&self) -> windows::core::Result<()> {
        let panel = self.panel.get();
        let origin = POINT {
            x: self.layout.pill.left,
            y: self.layout.pill.top,
        };
        // SAFETY: the target and paint come from one factory and live with `self`.
        unsafe { self.target.BeginDraw() };
        let drawn = self
            .paint
            .draw(&self.layout, origin, self.focus.get(), |control| {
                look(panel, control)
            });
        // SAFETY: matches the BeginDraw above.
        let ended = unsafe { self.target.EndDraw(None, None) };
        drawn.and(ended)
    }

    fn changed(&self, window: HWND) {
        self.mirror
            .show(Some((self.layout.clone(), self.focus.get())));
        self.mirror.show_choices(self.panel.get());
        // SAFETY: a repaint request for this thread's own window.
        let _ = unsafe { InvalidateRect(Some(window), None, false) };
    }

    fn press(&self, window: HWND, control: ToolbarControl) {
        let before = self.panel.get();
        let mut panel = before;
        let outcome = panel.press(control);
        self.panel.set(panel);
        match outcome {
            PanelOutcome::Stay => {
                self.changed(window);
                match control {
                    ToolbarControl::Mode(_) => self.raise(window, Change::Selected(control)),
                    ToolbarControl::Timer => self.raise(
                        window,
                        Change::Timer {
                            before: before.timer(),
                            after: panel.timer(),
                        },
                    ),
                    ToolbarControl::Options => self.options_menu(),
                    ToolbarControl::Capture
                    | ToolbarControl::Copy
                    | ToolbarControl::Save
                    | ToolbarControl::Edit
                    | ToolbarControl::Cancel => {}
                }
            }
            PanelOutcome::Capture { .. } | PanelOutcome::Cancel => self.close(window, outcome),
        }
    }

    /// F-12: the Options menu over its button.
    fn options_menu(&self) {
        let entries = (self.options)();
        match options::pick(self.layout.area(ToolbarControl::Options), &entries) {
            Ok(Some(item)) => self.send(PanelEvent::Options(item)),
            Ok(None) => {}
            Err(error) => self.send(PanelEvent::OptionsFailed(error)),
        }
    }

    fn focus(&self, window: HWND, control: ToolbarControl) {
        self.focus.set(control);
        self.changed(window);
        self.raise(window, Change::Focus(control));
    }

    fn raise(&self, window: HWND, change: Change) {
        // A change not raised is safe to drop: the tree already holds it for a screen reader
        // that looks (F-11).
        let _unraised = automation::raise(window, &self.mirror, change);
    }

    /// D-106: hides the panel before anything is captured, and gives the focus back.
    fn close(&self, window: HWND, outcome: PanelOutcome) {
        if self.closed.replace(true) {
            return;
        }
        // SAFETY: hiding and handing the focus on touch only these windows. ShowWindow returns
        // whether the window was visible, and a refused SetForegroundWindow leaves the focus
        // where Windows puts it, so neither result is an error.
        unsafe {
            let _ = ShowWindow(window, SW_HIDE);
            let _ = SetForegroundWindow(self.previous);
        }
        self.send(PanelEvent::Closed {
            panel: self.panel.get(),
            outcome,
        });
    }

    fn send(&self, event: PanelEvent) {
        (self.listener.borrow_mut())(event);
    }

    fn key(&self, window: HWND, key: u16) {
        if self.closed.get() {
            return;
        }
        match key {
            key if key == VK_TAB.0 => {
                // SAFETY: reads this thread's keyboard state.
                let shift = unsafe { GetKeyState(i32::from(VK_SHIFT.0)) } < 0;
                let direction = if shift {
                    TabDirection::Back
                } else {
                    TabDirection::Forward
                };
                let off = |control| self.mirror.off(control).is_some();
                self.focus(
                    window,
                    Toolbar::Panel.step(self.focus.get(), direction, off),
                );
            }
            key if key == VK_RETURN.0 || key == VK_SPACE.0 => self.press(window, self.focus.get()),
            key if key == VK_ESCAPE.0 => self.press(window, ToolbarControl::Cancel),
            _ => {}
        }
    }
}

/// How the panel draws `control`: the chosen mode marked, the timer's wait as its label.
fn look(panel: CapturePanel, control: ToolbarControl) -> Look {
    let label = match control {
        ToolbarControl::Timer => timer_label(panel.timer()),
        ToolbarControl::Mode(_)
        | ToolbarControl::Options
        | ToolbarControl::Capture
        | ToolbarControl::Copy
        | ToolbarControl::Save
        | ToolbarControl::Edit
        | ToolbarControl::Cancel => toolbar::label(control).into(),
    };
    let mark = match control {
        ToolbarControl::Mode(mode) if mode == panel.mode() => Mark::Chosen,
        ToolbarControl::Mode(_)
        | ToolbarControl::Timer
        | ToolbarControl::Options
        | ToolbarControl::Capture
        | ToolbarControl::Copy
        | ToolbarControl::Save
        | ToolbarControl::Edit
        | ToolbarControl::Cancel => Mark::Plain,
    };
    Look { label, mark }
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

/// Opens the panel on `monitor`, shown, with the keyboard focus on Capture (D-106).
pub(super) fn open_window(
    monitor: RECT,
    panel: CapturePanel,
    options: Options,
    listener: Listener,
) -> Result<HWND, Error> {
    let scale = dpi_scale(monitor).map_err(Error::Monitor)?;
    let layout =
        toolbar::layout(Toolbar::Panel, monitor, None, scale).ok_or(Error::MonitorTooSmall)?;
    let pill = layout.pill;
    let (width, height) = (pill.right - pill.left, pill.bottom - pill.top);
    // SAFETY: reads which window has the focus.
    let previous = unsafe { GetForegroundWindow() };
    // SAFETY: the class is registered and every string is 'static. The process is
    // per-monitor DPI aware, so the rectangle is physical pixels.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            WINDOW_CLASS,
            w!("Capture"),
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
    let made =
        surface(window, layout, scale, panel, previous, options, listener).and_then(|surface| {
            crate::round_window(window, width, height, height / 2)?;
            // SAFETY: `window` was created above with nothing stored on it, and
            // window_procedure takes every message's Surface through window_state. D-106: the
            // panel is left out of every capture.
            unsafe {
                window_state::attach(window, Rc::new(surface));
                SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE)?;
            }
            Ok(())
        });
    if let Err(error) = made {
        // SAFETY: the window was created above; WM_NCDESTROY frees any state on it.
        let destroyed = unsafe { DestroyWindow(window) };
        debug_assert!(destroyed.is_ok(), "a window made here is destroyed once");
        return Err(Error::Window(error));
    }
    // SAFETY: the window was created above on this thread.
    let _ = unsafe { ShowWindow(window, SW_SHOW) };
    bring_forward(window);
    Ok(window)
}

/// Gives the panel the keyboard focus. Windows allows it after the hotkey that opened the
/// panel; otherwise the panel shows unfocused until it is clicked.
pub(super) fn bring_forward(window: HWND) {
    // SAFETY: a focus request for this thread's own window; a refusal leaves it shown.
    let _ = unsafe { SetForegroundWindow(window) };
}

fn surface(
    window: HWND,
    layout: Layout,
    scale: f32,
    panel: CapturePanel,
    previous: HWND,
    options: Options,
    listener: Listener,
) -> windows::core::Result<Surface> {
    // SAFETY: D2D1CreateFactory has no preconditions.
    let factory: ID2D1Factory =
        unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }?;
    let pill = layout.pill;
    let target = target(
        &factory,
        window,
        D2D_SIZE_U {
            width: (pill.right - pill.left).unsigned_abs(),
            height: (pill.bottom - pill.top).unsigned_abs(),
        },
    )?;
    let paint = Paint::new(&target.cast()?, None, scale, &Palette::current()?)?;
    let mirror = Mirror::new(Toolbar::Panel, None);
    let focus = ToolbarControl::Capture;
    mirror.show(Some((layout.clone(), focus)));
    mirror.show_choices(panel);
    Ok(Surface {
        target,
        paint,
        layout,
        panel: Cell::new(panel),
        focus: Cell::new(focus),
        mirror,
        previous,
        closed: Cell::new(false),
        listener: RefCell::new(listener),
        options,
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
                surface.send(PanelEvent::DrawFailed(error));
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            #[expect(clippy::cast_possible_truncation, reason = "a virtual key is 16 bits")]
            surface.key(window, wparam.0 as u16);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let origin = POINT {
                x: surface.layout.pill.left,
                y: surface.layout.pill.top,
            };
            if let Some(control) = surface.layout.control_at(desktop_point(lparam, origin))
                && !surface.closed.get()
            {
                surface.focus(window, control);
                surface.press(window, control);
            }
            LRESULT(0)
        }
        WM_GETOBJECT => automation::answer(window, &surface.mirror, wparam, lparam)
            // SAFETY: forwarding a request this window has no provider for.
            .unwrap_or_else(|| unsafe { DefWindowProcW(window, message, wparam, lparam) }),
        WM_DESTROY => {
            automation::forget(window);
            LRESULT(0)
        }
        automation::WAKE => {
            for request in surface.mirror.requests() {
                match request {
                    Request::Choose(control) => surface.press(window, control),
                    Request::Focus(control) => surface.focus(window, control),
                }
            }
            LRESULT(0)
        }
        // SAFETY: forwarding an unhandled message to the default procedure.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}
