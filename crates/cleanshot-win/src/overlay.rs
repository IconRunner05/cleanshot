use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cleanshot_core::{TabDirection, Toolbar, ToolbarControl};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1CreateFactory, ID2D1Factory,
};
use windows::Win32::Graphics::Gdi::{InvalidateRect, UpdateWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey,
    VIRTUAL_KEY, VK_DOWN, VK_E, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_S, VK_SPACE, VK_TAB,
    VK_UP, VK_W,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GetCursorPos, SW_SHOWNOACTIVATE, ShowWindow,
};

use crate::Error;
use crate::capture::{MonitorImage, Pickable, dpi_scale};
use crate::fade;
use crate::palette::Palette;
use crate::selection::{Grip, nudged};
use crate::toolbar::automation::Mirror;
use crate::toolbar::{self, Layout};

mod picking;
mod window;

use picking::Mode;
#[cfg(feature = "field-test")]
pub(crate) use window::WINDOW_CLASS;
pub(crate) use window::{bgra, target_and_bitmap};
use window::{open_window, register_class};

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
    /// Ctrl+Shift+S was pressed on this selection, in physical virtual-desktop pixels (F-22).
    SaveAs(RECT),
    /// Direct2D could not draw a monitor's frozen image.
    DrawFailed(windows::core::Error),
}

/// A key the overlay holds as a hotkey while it is up (D-23).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    /// Chooses this toolbar control, wherever the focus is.
    Choose(ToolbarControl),
    /// Chooses the focused toolbar control.
    Press,
    /// Moves the toolbar's focus one control along.
    Focus(TabDirection),
    Nudge(i32, i32),
    SaveAs,
    /// Switches between drawing a region and picking a window (F-08, D-100).
    SwitchMode,
}

const KEYS: [(HOT_KEY_MODIFIERS, VIRTUAL_KEY, Key); 17] = [
    (MOD_NOREPEAT, VK_ESCAPE, Key::Choose(ToolbarControl::Cancel)),
    (MOD_NOREPEAT, VK_RETURN, Key::Press),
    (MOD_NOREPEAT, VK_SPACE, Key::Press),
    (
        HOT_KEY_MODIFIERS(MOD_NOREPEAT.0 | MOD_CONTROL.0),
        VK_S,
        Key::Choose(ToolbarControl::Save),
    ),
    (
        HOT_KEY_MODIFIERS(MOD_NOREPEAT.0 | MOD_CONTROL.0 | MOD_SHIFT.0),
        VK_S,
        Key::SaveAs,
    ),
    (MOD_NOREPEAT, VK_E, Key::Choose(ToolbarControl::Edit)),
    (MOD_NOREPEAT, VK_W, Key::SwitchMode),
    (
        HOT_KEY_MODIFIERS(0),
        VK_TAB,
        Key::Focus(TabDirection::Forward),
    ),
    (MOD_SHIFT, VK_TAB, Key::Focus(TabDirection::Back)),
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
    mode: Cell<Mode>,
    /// The windows a window capture can pick, topmost first (D-100).
    pickable: Vec<Pickable>,
    monitors: Vec<RECT>,
    windows: RefCell<Vec<HWND>>,
    /// The window that shows the toolbar and answers UI Automation for it.
    toolbar_window: Cell<Option<HWND>>,
    /// The toolbar's monitor and its physical pixels per DIP (F-05).
    toolbar: Option<(RECT, f32)>,
    focus: Cell<ToolbarControl>,
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
        self.mirror.show_pick(match self.mode.get() {
            Mode::Window {
                hovered: Some(pick),
            } => Some((self.spoken(pick), self.frame(pick))),
            Mode::Window { hovered: None } | Mode::Region => None,
        });
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
        if let Mode::Window { hovered } = self.mode.get() {
            return self.press_picking(key, hovered);
        }
        match (key, self.selected.get()) {
            (Key::SwitchMode, _) => {
                self.mode.set(Mode::Window { hovered: None });
                self.select(None);
            }
            (Key::Choose(control), _) => self.choose(control),
            (Key::Press, _) => self.choose(self.focus.get()),
            (Key::Focus(direction), _) => {
                let off = |control| self.mirror.off(control).is_some();
                self.focus(Toolbar::Overlay.step(self.focus.get(), direction, off));
            }
            (Key::Nudge(dx, dy), Some(selected)) => self.select(Some(nudged(selected, dx, dy))),
            (Key::Nudge(..) | Key::SaveAs, None) => {}
            (Key::SaveAs, Some(selected)) => self.send(OverlayEvent::SaveAs(selected)),
        }
    }

    fn focus(&self, control: ToolbarControl) {
        self.focus.set(control);
        self.changed();
    }

    /// F-05: Cancel always closes; the others need a selection to act on.
    fn choose(&self, control: ToolbarControl) {
        match (control, self.selected.get()) {
            (ToolbarControl::Cancel, _) => self.send(OverlayEvent::Cancelled),
            (ToolbarControl::Copy, Some(selected)) => self.send(OverlayEvent::Copy(selected)),
            (ToolbarControl::Save, Some(selected)) => self.send(OverlayEvent::Save(selected)),
            (ToolbarControl::Edit, Some(selected)) => self.send(OverlayEvent::Edit(selected)),
            (ToolbarControl::Copy | ToolbarControl::Save | ToolbarControl::Edit, None) => {}
            (ToolbarControl::Mode(_) | ToolbarControl::Timer | ToolbarControl::Capture, _) => {
                unreachable!("the overlay toolbar shows no capture panel control")
            }
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
    shared: Rc<Shared>,
}

impl Overlay {
    /// Makes and drops what the first overlay on this thread would otherwise pay to start
    /// Direct2D and DirectWrite, with no window ever shown (P-01).
    ///
    /// # Errors
    ///
    /// [`Error::Window`] if Direct2D, DirectWrite or the hidden window cannot be made; the
    /// overlay then most likely cannot open either.
    pub fn prewarm() -> Result<(), Error> {
        register_class()?;
        // SAFETY: D2D1CreateFactory has no preconditions.
        let factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
                .map_err(Error::Window)?;
        let palette = Palette::current().map_err(Error::Window)?;
        window::prewarm(&factory, &palette).map_err(Error::Window)
    }

    /// Shows `captures`, one window per monitor at that monitor's physical bounds, with W
    /// switching to picking one of `pickable`, the frames [`crate::capture::pickable_windows`]
    /// listed at the freeze (F-08). With `saving_off`, the toolbar shows Save greyed, out of Tab
    /// order, with that sentence as its accessible help text (F-57); choosing it still sends
    /// [`OverlayEvent::Save`].
    ///
    /// # Errors
    ///
    /// [`Error::Window`] if a window or its Direct2D surface cannot be made;
    /// [`Error::Monitor`] if the pointer's position or its monitor's DPI cannot be read;
    /// [`Error::NoCaptures`] if `captures` is empty;
    /// [`Error::HotkeyTaken`] if another program holds one of its keys as a hotkey.
    pub fn show(
        captures: &[MonitorImage],
        selected: Option<RECT>,
        pickable: Vec<Pickable>,
        saving_off: Option<String>,
        on_event: impl FnMut(OverlayEvent) + 'static,
    ) -> Result<Self, Error> {
        register_class()?;
        // SAFETY: D2D1CreateFactory has no preconditions.
        let factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
                .map_err(Error::Window)?;
        let host = toolbar_monitor(captures).map_err(Error::Monitor)?;
        let toolbar = match host {
            Some(capture) => Some((
                capture.bounds,
                dpi_scale(capture.bounds).map_err(Error::Monitor)?,
            )),
            None => None,
        };
        let shared = Rc::new(Shared {
            listener: RefCell::new(Box::new(on_event)),
            selected: Cell::new(selected),
            gesture: Cell::new(None),
            mode: Cell::new(Mode::Region),
            pickable,
            monitors: captures.iter().map(|capture| capture.bounds).collect(),
            windows: RefCell::new(Vec::with_capacity(captures.len())),
            toolbar_window: Cell::new(None),
            toolbar,
            focus: Cell::new(ToolbarControl::Copy),
            mirror: Mirror::new(saving_off),
            palette: Palette::current().map_err(Error::Window)?,
        });
        shared.changed();
        let mut overlay = Self {
            windows: Vec::with_capacity(captures.len()),
            shared: Rc::clone(&shared),
        };
        for capture in captures {
            let hosts_toolbar = host.is_some_and(|host| std::ptr::eq(host, capture));
            let window = open_window(&factory, capture, &shared, hosts_toolbar)?;
            if hosts_toolbar {
                shared.toolbar_window.set(Some(window));
            }
            overlay.windows.push(window);
            shared.windows.borrow_mut().push(window);
        }
        let first = *overlay.windows.first().ok_or(Error::NoCaptures)?;
        for (id, (modifiers, key, _)) in (0..).zip(KEYS) {
            // SAFETY: `first` was created above on this thread.
            unsafe { RegisterHotKey(Some(first), id, modifiers, u32::from(key.0)) }
                .map_err(Error::HotkeyTaken)?;
        }
        for &window in &overlay.windows {
            // SAFETY: `window` was created above; showing it without activation is F-02's rule.
            let _ = unsafe { ShowWindow(window, SW_SHOWNOACTIVATE) };
            // P-01: show paints the first frame before it returns, so timing show times visibility.
            // SAFETY: `window` was created above on this thread.
            unsafe { UpdateWindow(window) }
                .ok()
                .map_err(Error::Window)?;
        }
        Ok(overlay)
    }

    /// Starts picking a window, as W does (F-08): the window capture hotkey opens the overlay
    /// this way.
    pub fn pick_window(&self) {
        self.shared.press(Key::SwitchMode);
    }

    /// Closes the overlay with F-13's 120 ms fade, or at once when Windows' Animation effects
    /// is off. Dropping it instead closes it at once, as a handoff to the editor should.
    ///
    /// # Errors
    ///
    /// The Windows error that stopped the fade; the overlay is closed either way.
    pub fn close(self) -> windows::core::Result<()> {
        self.free_hotkeys();
        if fade::animations_on()? {
            fade::fade_out(&self.windows)?;
        }
        Ok(())
    }

    fn free_hotkeys(&self) {
        if let Some(&first) = self.windows.first() {
            for (id, _) in (0..).zip(KEYS) {
                // SAFETY: unregistering an id this window may not hold only returns an error.
                let _ = unsafe { UnregisterHotKey(Some(first), id) };
            }
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.free_hotkeys();
        for &window in &self.windows {
            // SAFETY: each window belongs to this thread and is destroyed once, here.
            let destroyed = unsafe { DestroyWindow(window) };
            debug_assert!(destroyed.is_ok(), "an overlay window is destroyed once");
        }
    }
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

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard, PoisonError};
    use std::time::{Duration, Instant};

    use cleanshot_core::RgbaImage;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetPixel,
        GetUpdateRect, ReleaseDC, SelectObject,
    };
    use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, EnumThreadWindows, IsWindow, MSG, PM_REMOVE, PeekMessageW, PostMessageW,
        SendMessageW, WM_HOTKEY, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    };
    use windows::core::BOOL;

    use super::*;

    /// An overlay registers its keys as system-wide hotkeys and covers the same corner of the
    /// screen, so one test's overlay is up at a time.
    static SCREEN: Mutex<()> = Mutex::new(());

    /// The screen, once no other overlay test holds it. A test that failed holding it leaves
    /// nothing behind once its overlay is dropped, so the next test takes it as it is.
    pub(super) fn screen() -> MutexGuard<'static, ()> {
        SCREEN.lock().unwrap_or_else(PoisonError::into_inner)
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

    /// A 64x48 gradient capture at the primary monitor's top-left corner, the virtual
    /// desktop's origin.
    pub(super) fn monitor() -> MonitorImage {
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

    pub(super) fn pump(duration: Duration) {
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
        let _screen = screen();
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&events);
        let overlay = Overlay::show(&[monitor()], None, Vec::new(), None, move |event| {
            sink.borrow_mut().push(format!("{event:?}"));
        })
        .expect("the overlay opens");
        pump(Duration::from_millis(500));
        let points = [(5, 7), (40, 30), (63, 47)];
        // Another test process's window may cover the desktop's corner, so this reads the
        // overlay window's own pixels rather than the screen's.
        // SAFETY: every DC and the bitmap are made, used and freed here on this thread, and
        // the window belongs to this thread.
        let shown = unsafe {
            let screen = GetDC(None);
            let copy = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, 64, 48);
            let previous = SelectObject(copy, bitmap.into());
            let printed = PrintWindow(overlay.windows[0], copy, PRINT_WINDOW_FLAGS(0)).as_bool();
            let read = points.map(|(x, y)| {
                let colour = GetPixel(copy, x, y).0.to_le_bytes();
                [colour[0], colour[1], colour[2]]
            });
            SelectObject(copy, previous);
            let freed = DeleteObject(bitmap.into()).as_bool() && DeleteDC(copy).as_bool();
            ReleaseDC(None, screen);
            assert!(printed && freed, "the overlay window prints into a bitmap");
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
    fn f13_closing_fades_the_overlay_only_with_animation_effects_on() {
        let _screen = screen();
        let animations = fade::animations_on().expect("the setting is readable");
        let overlay =
            Overlay::show(&[monitor()], None, Vec::new(), None, |_| {}).expect("the overlay opens");
        let closing = Instant::now();
        overlay.close().expect("the overlay fades");
        let closed = closing.elapsed();
        assert_eq!(
            closed >= Duration::from_millis(120),
            animations,
            "closing took {closed:?} with Animation effects {}",
            if animations { "on" } else { "off" }
        );
    }

    #[test]
    fn p01_show_returns_with_every_window_painted() {
        let _screen = screen();
        let overlay =
            Overlay::show(&[monitor()], None, Vec::new(), None, |_| {}).expect("the overlay opens");
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
        let _screen = screen();
        let screen = monitor().bounds;
        let copied = Rc::new(Cell::new(None));
        let sink = Rc::clone(&copied);
        let overlay = Overlay::show(&[monitor()], Some(screen), Vec::new(), None, move |event| {
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

    /// How many windows this thread owns now.
    fn thread_windows() -> usize {
        unsafe extern "system" fn count(_: HWND, counted: LPARAM) -> BOOL {
            // SAFETY: `counted` is the address of the usize below, live for the enumeration.
            // EnumThreadWindows calls this only during our call, with our own LPARAM; no window
            // message carries it, so no other process can supply it.
            // ast-grep-ignore: pointer-from-handle-value
            unsafe { *(counted.0 as *mut usize) += 1 };
            BOOL(1)
        }
        let mut counted = 0_usize;
        // SAFETY: the callback writes only through the pointer to `counted`, which outlives it.
        let _ = unsafe {
            EnumThreadWindows(
                GetCurrentThreadId(),
                Some(count),
                LPARAM(&raw mut counted as isize),
            )
        };
        counted
    }

    #[test]
    fn p01_the_prewarm_leaves_no_window_behind() {
        let _screen = screen();
        let before = thread_windows();
        Overlay::prewarm().expect("Direct2D and DirectWrite start");
        assert_eq!(thread_windows(), before);
    }

    #[test]
    fn f03_a_drag_on_a_monitor_left_of_and_above_the_primary_selects_in_desktop_pixels() {
        let _screen = screen();
        let above_left = MonitorImage {
            bounds: RECT {
                left: -64,
                top: -48,
                right: 0,
                bottom: 0,
            },
            image: monitor().image,
        };
        let copied = Rc::new(Cell::new(None));
        let sink = Rc::clone(&copied);
        let overlay = Overlay::show(
            &[monitor(), above_left],
            None,
            Vec::new(),
            None,
            move |event| {
                if let OverlayEvent::Copy(selected) = event {
                    sink.set(Some(selected));
                }
            },
        )
        .expect("the overlay opens");
        let window = overlay.windows[1];
        let client = |x: isize, y: isize| Some(LPARAM(y * 0x1_0000 + x));
        // SAFETY: each message is handled on this thread by a window created above, and none
        // carries a pointer.
        unsafe {
            SendMessageW(window, WM_LBUTTONDOWN, None, client(10, 8));
            SendMessageW(window, WM_MOUSEMOVE, None, client(30, 20));
            SendMessageW(window, WM_LBUTTONUP, None, client(30, 20));
        }
        let enter = KEYS
            .iter()
            .position(|&(_, _, key)| matches!(key, Key::Press))
            .expect("Enter is one of the overlay's keys");
        // SAFETY: posting copies no memory, and the window belongs to this thread.
        unsafe { PostMessageW(Some(window), WM_HOTKEY, WPARAM(enter), LPARAM(0)) }
            .expect("the overlay window takes the message");
        pump(Duration::from_millis(200));
        drop(overlay);
        let expected = RECT {
            left: -54,
            top: -40,
            right: -34,
            bottom: -28,
        };
        assert_eq!(copied.get(), Some(expected));
    }

    #[test]
    fn f22_ctrl_shift_s_asks_to_save_the_selection_as() {
        let _screen = screen();
        let screen = monitor().bounds;
        let asked = Rc::new(Cell::new(None));
        let sink = Rc::clone(&asked);
        let overlay = Overlay::show(&[monitor()], Some(screen), Vec::new(), None, move |event| {
            if let OverlayEvent::SaveAs(selected) = event {
                sink.set(Some(selected));
            }
        })
        .expect("the overlay opens");
        let save_as = KEYS
            .iter()
            .position(|&(_, _, key)| matches!(key, Key::SaveAs))
            .expect("Ctrl+Shift+S is one of the overlay's keys");
        // SAFETY: posting copies no memory, and the window belongs to this thread.
        unsafe {
            PostMessageW(
                Some(overlay.windows[0]),
                WM_HOTKEY,
                WPARAM(save_as),
                LPARAM(0),
            )
        }
        .expect("the overlay window takes the message");
        pump(Duration::from_millis(200));
        drop(overlay);
        assert_eq!(asked.get(), Some(screen));
    }

    /// Posts the hotkey message for `key`, as Windows would on its press.
    pub(super) fn press(window: HWND, key: Key) {
        let index = KEYS
            .iter()
            .position(|&(_, _, held)| held == key)
            .expect("the key is one of the overlay's");
        // SAFETY: posting copies no memory, and the window belongs to this thread.
        unsafe { PostMessageW(Some(window), WM_HOTKEY, WPARAM(index), LPARAM(0)) }
            .expect("the overlay window takes the message");
        pump(Duration::from_millis(50));
    }

    #[test]
    fn f57_tab_and_shift_tab_move_the_toolbar_focus_past_save_while_saving_is_off() {
        let _screen = screen();
        let chosen = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&chosen);
        let selected = monitor().bounds;
        let off = Some("Saving is off".to_owned());
        let overlay = Overlay::show(
            &[monitor()],
            Some(selected),
            Vec::new(),
            off,
            move |event| {
                sink.borrow_mut().push(match event {
                    OverlayEvent::Copy(_) => "Copy",
                    OverlayEvent::Save(_) => "Save",
                    OverlayEvent::Edit(_) => "Edit",
                    OverlayEvent::Cancelled => "Cancel",
                    OverlayEvent::SaveAs(_) | OverlayEvent::DrawFailed(_) => "other",
                });
            },
        )
        .expect("the overlay opens");
        let window = overlay.windows[0];
        let (tab, back) = (
            Key::Focus(TabDirection::Forward),
            Key::Focus(TabDirection::Back),
        );
        for key in [tab, Key::Press, back, Key::Press, back, Key::Press] {
            press(window, key);
        }
        drop(overlay);
        assert_eq!(*chosen.borrow(), ["Edit", "Copy", "Cancel"]);
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
    fn f01_an_overlay_dropped_by_its_listener_is_freed_only_once_the_message_returns() {
        struct Freed(Rc<Cell<bool>>);
        impl Drop for Freed {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let _screen = screen();
        let freed = Rc::new(Cell::new(false));
        let guard = Freed(Rc::clone(&freed));
        let slot: Rc<RefCell<Option<Overlay>>> = Rc::default();
        let freed_during = Rc::new(Cell::new(None));
        let (closing, watching, report) = (
            Rc::clone(&slot),
            Rc::clone(&freed),
            Rc::clone(&freed_during),
        );
        let overlay = Overlay::show(&[monitor()], None, Vec::new(), None, move |_| {
            let Freed(_) = &guard;
            drop(closing.borrow_mut().take());
            report.set(Some(watching.get()));
        })
        .expect("the overlay opens");
        let window = overlay.windows[0];
        *slot.borrow_mut() = Some(overlay);
        press(window, Key::Choose(ToolbarControl::Cancel));
        assert_eq!(
            freed_during.get(),
            Some(false),
            "freed under the running listener"
        );
        assert!(freed.get(), "freed once the message returned");
        // SAFETY: IsWindow only reads whether the handle still names a window.
        assert!(!unsafe { IsWindow(Some(window)) }.as_bool());
    }

    #[test]
    fn f01_an_overlay_given_no_monitor_says_so_and_opens_nothing() {
        let _screen = screen();
        let shown = Overlay::show(&[], None, Vec::new(), None, |_| {});
        assert!(matches!(shown, Err(Error::NoCaptures)), "{:?}", shown.err());
    }

    #[test]
    fn f05_the_toolbar_keys_are_free_again_after_esc() {
        let _screen = screen();
        let cancelled = Rc::new(Cell::new(false));
        let sink = Rc::clone(&cancelled);
        let overlay = Overlay::show(&[monitor()], None, Vec::new(), None, move |event| {
            sink.set(matches!(event, OverlayEvent::Cancelled));
        })
        .expect("the overlay opens");
        assert!(
            held(HOT_KEY_MODIFIERS(0), VK_TAB),
            "Tab is the overlay's while it is up"
        );
        let escape = KEYS
            .iter()
            .position(|&(_, _, key)| matches!(key, Key::Choose(ToolbarControl::Cancel)))
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
        let _screen = screen();
        let (modifiers, last, _) = KEYS[KEYS.len() - 1];
        // SAFETY: a thread hotkey needs no window; it is released below.
        unsafe { RegisterHotKey(None, TAKEN, modifiers, u32::from(last.0)) }
            .expect("no other program holds the overlay's last key");
        let shown = Overlay::show(&[monitor()], None, Vec::new(), None, |_| {});
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

    /// The dpi job (D-71) sets the runner's 1920x1080 display to each scale and names its dpi
    /// in `CLEANSHOT_DPI`; outside that job there is no known scale to check.
    #[test]
    #[ignore = "needs the dpi job's display scale (D-71)"]
    fn d71_the_toolbar_takes_the_scale_windows_gives_the_monitor() {
        let dpi: u32 = std::env::var("CLEANSHOT_DPI")
            .expect("the dpi job sets CLEANSHOT_DPI")
            .parse()
            .expect("CLEANSHOT_DPI is a number");
        // SAFETY: sets this test thread's awareness to cleanshot.exe's, PerMonitorV2.
        let earlier =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        assert!(!earlier.is_invalid(), "the thread became PerMonitorV2");
        let primary = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };

        let scale = dpi_scale(primary).expect("the primary monitor has a DPI");
        let pill = toolbar::layout(primary, None, scale)
            .expect("a 1080p monitor holds the toolbar at 175%")
            .pill;

        let expected = i32::try_from((44 * dpi + 48) / 96).expect("a pill is a few pixels tall");
        assert_eq!(pill.bottom - pill.top, expected, "at {dpi} dpi");
    }
}
