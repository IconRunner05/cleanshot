use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cleanshot_core::HotkeyAction;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1CreateFactory, ID2D1Factory,
};
use windows::Win32::Graphics::Gdi::{InvalidateRect, UpdateWindow};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromRect};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey,
    VIRTUAL_KEY, VK_DOWN, VK_E, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_S, VK_SPACE, VK_TAB,
    VK_UP, VK_W,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GetCursorPos, SW_SHOWNOACTIVATE, ShowWindow,
};

use crate::Error;
use crate::capture::{MonitorImage, Pick, Pickable, picked};
use crate::fade;
use crate::palette::Palette;
use crate::selection::{Grip, nudged};
use crate::toolbar::automation::{self, Mirror};
use crate::toolbar::{self, Control, Layout};

mod window;

#[cfg(feature = "field-test")]
pub(crate) use window::WINDOW_CLASS;
pub(crate) use window::{bgra, client_point, target_and_bitmap};
use window::{open_window, register_class};

const DESKTOP_DPI: f32 = 96.0;
const WINDOW_TO_CAPTURE: &str = "Window to capture";
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
    Choose(Control),
    /// Chooses the focused toolbar control.
    Press,
    /// Moves the toolbar's focus this many controls along.
    Focus(i32),
    Nudge(i32, i32),
    SaveAs,
    /// Switches between drawing a region and picking a window (F-08, D-100).
    SwitchMode,
}

const KEYS: [(HOT_KEY_MODIFIERS, VIRTUAL_KEY, Key); 17] = [
    (MOD_NOREPEAT, VK_ESCAPE, Key::Choose(Control::Cancel)),
    (MOD_NOREPEAT, VK_RETURN, Key::Press),
    (MOD_NOREPEAT, VK_SPACE, Key::Press),
    (
        HOT_KEY_MODIFIERS(MOD_NOREPEAT.0 | MOD_CONTROL.0),
        VK_S,
        Key::Choose(Control::Save),
    ),
    (
        HOT_KEY_MODIFIERS(MOD_NOREPEAT.0 | MOD_CONTROL.0 | MOD_SHIFT.0),
        VK_S,
        Key::SaveAs,
    ),
    (MOD_NOREPEAT, VK_E, Key::Choose(Control::Edit)),
    (MOD_NOREPEAT, VK_W, Key::SwitchMode),
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

/// Whether the pointer draws a region or picks a window (F-08).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Region,
    /// The frame the pointer or Tab is on, highlighted until a press picks it.
    Window {
        hovered: Option<Pick>,
    },
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
            (Key::Focus(step), _) => {
                let off = |control| self.mirror.off(control).is_some();
                self.focus(self.focus.get().step(step, off));
            }
            (Key::Nudge(dx, dy), Some(selected)) => self.select(Some(nudged(selected, dx, dy))),
            (Key::Nudge(..) | Key::SaveAs, None) => {}
            (Key::SaveAs, Some(selected)) => self.send(OverlayEvent::SaveAs(selected)),
        }
    }

    /// F-08: in window mode Tab moves the highlight through the windows and Enter picks it.
    fn press_picking(&self, key: Key, hovered: Option<Pick>) {
        match key {
            Key::SwitchMode => {
                self.mode.set(Mode::Region);
                self.changed();
            }
            Key::Focus(step) => self.hover(next_window(self.pickable.len(), hovered, step)),
            Key::Press => self.pick(hovered),
            Key::Choose(Control::Cancel) => self.send(OverlayEvent::Cancelled),
            Key::Choose(Control::Copy | Control::Save | Control::Edit)
            | Key::Nudge(..)
            | Key::SaveAs => {}
        }
    }

    /// What a press at `at` picks in window mode: its window, or else its monitor (D-100).
    fn picked_at(&self, at: POINT) -> Option<Pick> {
        picked(&self.pickable, &self.monitors, at)
    }

    /// F-58: a move of the highlight is said aloud, since the overlay never takes the focus.
    fn hover(&self, hovered: Option<Pick>) {
        let now = Mode::Window { hovered };
        if self.mode.replace(now) == now {
            return;
        }
        self.changed();
        if let (Some(pick), Some(window)) = (hovered, self.toolbar_window.get()) {
            // A failed announcement is safe to drop: the highlight has moved, and the toolbar's
            // Text element names the pick for a screen reader that looks.
            let _unannounced = automation::announce(window, &self.mirror, &self.spoken(pick));
        }
    }

    /// What a screen reader says for `pick`: the window's title, or the tray's name for a
    /// capture of the whole screen.
    fn spoken(&self, pick: Pick) -> String {
        match pick {
            Pick::Window(at) => match self.window(at).title.as_str() {
                "" => WINDOW_TO_CAPTURE.to_owned(),
                title => format!("{WINDOW_TO_CAPTURE}: {title}"),
            },
            Pick::Monitor(_) => HotkeyAction::FullScreen.name().to_owned(),
        }
    }

    fn window(&self, at: usize) -> &Pickable {
        self.pickable
            .get(at)
            .expect("a pick names a window in the overlay's list")
    }

    /// Selects what `pick` names as a region, which the toolbar then acts on as any selection.
    fn pick(&self, pick: Option<Pick>) {
        if let Some(pick) = pick {
            self.mode.set(Mode::Region);
            self.select(Some(self.frame(pick)));
        }
    }

    fn frame(&self, pick: Pick) -> RECT {
        match pick {
            Pick::Window(at) => self.window(at).frame,
            Pick::Monitor(bounds) => bounds,
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

/// The window `step` places along from `hovered` in a list of `count`, wrapping at either
/// end; from a monitor or none, the first forwards and the last backwards.
fn next_window(count: usize, hovered: Option<Pick>, step: i32) -> Option<Pick> {
    let next = match hovered {
        Some(Pick::Window(at)) => {
            (at + count).checked_add_signed(isize::try_from(step).ok()?)? % count.max(1)
        }
        Some(Pick::Monitor(_)) | None if step < 0 => count.checked_sub(1)?,
        Some(Pick::Monitor(_)) | None => 0,
    };
    (next < count).then_some(Pick::Window(next))
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
    /// [`Error::Window`] if a window or its Direct2D surface cannot be made, or the pointer's
    /// position or its monitor's DPI cannot be read;
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
            mode: Cell::new(Mode::Region),
            pickable,
            monitors: captures.iter().map(|capture| capture.bounds).collect(),
            windows: RefCell::new(Vec::with_capacity(captures.len())),
            toolbar_window: Cell::new(None),
            toolbar,
            focus: Cell::new(Control::Copy),
            mirror: Mirror::new(saving_off),
            palette: Palette::current().map_err(Error::Window)?,
        });
        shared.changed();
        let mut overlay = Self {
            windows: Vec::with_capacity(captures.len()),
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
            // P-01: show paints the first frame before it returns, so timing show times visibility.
            // SAFETY: `window` was created above on this thread.
            unsafe { UpdateWindow(window) }
                .ok()
                .map_err(Error::Window)?;
        }
        Ok(overlay)
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

/// Physical pixels per DIP on the monitor holding `bounds`.
#[expect(clippy::cast_precision_loss, reason = "a DPI is a few hundred")]
pub(crate) fn dpi_scale(bounds: RECT) -> windows::core::Result<f32> {
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
        DispatchMessageW, EnumThreadWindows, MSG, PM_REMOVE, PeekMessageW, PostMessageW,
        SendMessageW, WM_HOTKEY, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    };
    use windows::core::BOOL;

    use super::*;

    /// An overlay registers its keys as system-wide hotkeys and covers the same corner of the
    /// screen, so one test's overlay is up at a time.
    static SCREEN: Mutex<()> = Mutex::new(());

    /// The screen, once no other overlay test holds it. A test that failed holding it leaves
    /// nothing behind once its overlay is dropped, so the next test takes it as it is.
    fn screen() -> MutexGuard<'static, ()> {
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
    fn press(window: HWND, key: Key) {
        let index = KEYS
            .iter()
            .position(|&(_, _, held)| held == key)
            .expect("the key is one of the overlay's");
        // SAFETY: posting copies no memory, and the window belongs to this thread.
        unsafe { PostMessageW(Some(window), WM_HOTKEY, WPARAM(index), LPARAM(0)) }
            .expect("the overlay window takes the message");
        pump(Duration::from_millis(50));
    }

    /// An overlay on `monitor()` picking from `pickable`, and the selections it copies.
    fn picking(frames: Vec<RECT>) -> (Overlay, Rc<RefCell<Vec<RECT>>>) {
        let pickable = frames
            .into_iter()
            .map(|frame| Pickable {
                frame,
                title: String::new(),
            })
            .collect();
        let copied = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&copied);
        let overlay = Overlay::show(&[monitor()], None, pickable, None, move |event| {
            if let OverlayEvent::Copy(selected) = event {
                sink.borrow_mut().push(selected);
            }
        })
        .expect("the overlay opens");
        (overlay, copied)
    }

    #[test]
    fn f08_after_w_a_click_picks_the_window_under_the_pointer_or_else_its_monitor() {
        let _screen = screen();
        let frame = RECT {
            left: 10,
            top: 8,
            right: 40,
            bottom: 30,
        };
        let (overlay, copied) = picking(vec![frame]);
        let window = overlay.windows[0];
        for (x, y) in [(20_isize, 20_isize), (50, 40)] {
            press(window, Key::SwitchMode);
            let at = Some(LPARAM(y * 0x1_0000 + x));
            // SAFETY: each message is handled on this thread by a window created above, and
            // none carries a pointer.
            unsafe {
                SendMessageW(window, WM_MOUSEMOVE, None, at);
                SendMessageW(window, WM_LBUTTONDOWN, None, at);
                SendMessageW(window, WM_LBUTTONUP, None, at);
            }
            press(window, Key::Press);
        }
        drop(overlay);
        assert_eq!(*copied.borrow(), [frame, monitor().bounds]);
    }

    #[test]
    fn f08_after_w_tab_and_shift_tab_move_through_every_window_even_two_with_one_frame() {
        let _screen = screen();
        let frame = |left| RECT {
            left,
            top: 0,
            right: left + 10,
            bottom: 10,
        };
        // Two maximised windows share one frame, and Tab still moves past the first.
        let (shared, other) = (frame(0), frame(20));
        let (overlay, copied) = picking(vec![shared, shared, other]);
        let window = overlay.windows[0];
        for keys in [
            &[Key::Focus(1), Key::Focus(1), Key::Focus(1)][..],
            &[Key::Focus(-1), Key::Focus(-1)],
        ] {
            press(window, Key::SwitchMode);
            for &key in keys {
                press(window, key);
            }
            press(window, Key::Press);
            press(window, Key::Press);
        }
        drop(overlay);
        assert_eq!(*copied.borrow(), [other, shared]);
    }

    #[test]
    #[cfg(feature = "ui-automation-test")]
    fn f58_ui_automation_names_the_window_tab_puts_the_highlight_on() {
        let _screen = screen();
        let (width, height) = (800, 600);
        let desktop = MonitorImage {
            bounds: RECT {
                left: 0,
                top: 0,
                right: 800,
                bottom: 600,
            },
            image: RgbaImage::new(width, height, vec![128; 800 * 600 * 4]).expect("800x600 RGBA"),
        };
        // One window over the whole desktop, so the pointer and Tab can only highlight it.
        let pickable = vec![Pickable {
            frame: desktop.bounds,
            title: "Notes".to_owned(),
        }];
        let overlay =
            Overlay::show(&[desktop], None, pickable, None, |_| {}).expect("the overlay opens");
        let window = overlay.windows[0];
        for key in [Key::SwitchMode, Key::Focus(1)] {
            press(window, key);
        }
        let walk = std::thread::spawn(crate::accessibility::controls_in_own_windows);
        while !walk.is_finished() {
            pump(Duration::from_millis(20));
        }
        let names: Vec<String> = walk
            .join()
            .expect("the walk thread ends")
            .expect("UI Automation walks the overlay")
            .into_iter()
            .map(|control| control.name)
            .collect();
        drop(overlay);
        assert!(
            names.iter().any(|name| name == "Window to capture: Notes"),
            "{names:?}"
        );
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
