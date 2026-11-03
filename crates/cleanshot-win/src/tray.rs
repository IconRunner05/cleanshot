use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cleanshot_core::{Hotkey, HotkeyAction, Key, Modifiers};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey, VK_F1, VK_SNAPSHOT,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NIM_SETVERSION, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    FindWindowW, GetCursorPos, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_TOPMOST,
    MENU_ITEM_FLAGS, MF_GRAYED, MF_STRING, MessageBoxW, PostMessageW, RegisterWindowMessageW,
    SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP, WM_CLOSE,
    WM_CONTEXTMENU, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_HOTKEY, WM_LBUTTONDBLCLK, WM_NULL,
    WM_SETTINGCHANGE, WM_SYSCOLORCHANGE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::Error;
use crate::struct_size;
use crate::tray_icon::{TaskbarShade, TrayIcon};
use crate::window_state;

const WINDOW_CLASS: PCWSTR = w!("CleanshotTray");
// C-14: the only message a second launch sends. Its parameters are always zero and never read.
const CAPTURE_SIGNAL: PCWSTR = w!("Cleanshot.CaptureRegion");
const ICON_MESSAGE: u32 = WM_APP + 1;
const ICON_ID: u32 = 1;
const MENU_CAPTURE_REGION: usize = 1;
const MENU_CAPTURE_SCREEN: usize = 2;
const MENU_EXIT: usize = 3;
const MENU_SETTINGS: usize = 4;
const MENU_EDIT_LAST: usize = 5;
const MENU_SAVE_LAST: usize = 6;

/// What the user asked for through the tray icon, its menu or a global hotkey.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Capture(HotkeyAction),
    /// What a click on the post-capture thumbnail does, for keyboard users (D-96).
    EditLastCapture,
    /// What the thumbnail's Save does (D-96).
    SaveLastCapture,
    OpenSettings,
    Exit,
}

/// The tray menu's items for the capture the thumbnail shows, which it holds only while the
/// thumbnail shows (D-96).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LastCaptureItems {
    Hidden,
    /// Save last capture is greyed when the organisation's policy turns saving off (F-57).
    Shown {
        saving_off: bool,
    },
}

struct State {
    on_request: RefCell<Box<dyn FnMut(Request)>>,
    taskbar_created: u32,
    capture_signal: u32,
    icon_shown: Cell<bool>,
    /// F-57: off under the organisation's `HideSettings` policy.
    settings_item: Cell<bool>,
    last_capture: Cell<LastCaptureItems>,
    /// The icon the shell shows, kept until the shell has the next one (D-92).
    icon: RefCell<Option<TrayIcon>>,
    /// The request each registered hotkey sends; hotkey id `n` is entry `n - 1`.
    hotkey_requests: Vec<Request>,
}

impl State {
    fn send(&self, request: Request) {
        // A request that arrives while the callback runs a modal loop of its own is dropped:
        // the user is already in the middle of one.
        if let Ok(mut on_request) = self.on_request.try_borrow_mut() {
            on_request(request);
        }
    }
}

/// The hidden window that owns the tray icon and the global hotkeys (F-50, F-51).
///
/// It lives on the thread that creates it, and that thread's message loop delivers its
/// requests. Dropping it removes the icon and the hotkeys.
pub struct Tray {
    window: HWND,
    hotkeys: Vec<(Request, Hotkey, Result<(), Error>)>,
}

impl Tray {
    /// Adds the tray icon and registers each hotkey to send its request (F-51). A hotkey
    /// another program holds is reported by [`Tray::hotkeys`], not as an error here.
    ///
    /// # Errors
    ///
    /// [`Error::Window`] if the hidden window cannot be created. A shell that refuses the icon
    /// is not an error: the icon appears when the taskbar next starts.
    pub fn create(
        hotkeys: &[(Request, Hotkey)],
        on_request: impl FnMut(Request) + 'static,
    ) -> Result<Self, Error> {
        crate::register_window_class(WINDOW_CLASS, window_procedure)?;

        // F-50: not HWND_MESSAGE, which misses TaskbarCreated when Explorer restarts.
        // SAFETY: the class was registered above and every string is 'static.
        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                WINDOW_CLASS,
                w!("Cleanshot"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            )
        }
        .map_err(Error::Window)?;
        let state = Rc::new(State {
            on_request: RefCell::new(Box::new(on_request)),
            // SAFETY: the argument is a 'static, NUL-terminated string.
            taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
            capture_signal: capture_signal(),
            hotkey_requests: hotkeys.iter().map(|(request, _)| *request).collect(),
            icon_shown: Cell::new(false),
            settings_item: Cell::new(true),
            last_capture: Cell::new(LastCaptureItems::Hidden),
            icon: RefCell::new(None),
        });
        // SAFETY: `window` was created above with nothing stored on it, and window_procedure
        // takes every message's State through window_state.
        unsafe { window_state::attach(window, state) };

        let hotkeys = hotkeys
            .iter()
            .enumerate()
            .map(|(index, &(request, hotkey))| (request, hotkey, register(window, index, hotkey)))
            .collect();
        let tray = Self { window, hotkeys };
        tray.state().icon_shown.set(add_icon(window, tray.state()));
        Ok(tray)
    }

    /// Each hotkey, the request it sends, and whether Windows registered it.
    pub fn hotkeys(&self) -> impl Iterator<Item = (Request, Hotkey, Result<(), &Error>)> {
        self.hotkeys
            .iter()
            .map(|(request, hotkey, registered)| (*request, *hotkey, registered.as_ref().copied()))
    }

    /// Moves each request in `wanted` to its new hotkey (F-55). Every changed hotkey is
    /// released before any is registered, so two requests can swap keys. A request whose new
    /// hotkey another program holds goes back to its old one, and is returned.
    pub fn set_hotkeys(&mut self, wanted: &[(Request, Hotkey)]) -> Vec<Request> {
        let moves: Vec<(usize, Hotkey)> = self
            .hotkeys
            .iter()
            .enumerate()
            .filter_map(|(index, (request, old, _))| {
                let &(_, new) = wanted.iter().find(|(asked, _)| asked == request)?;
                (new != *old).then_some((index, new))
            })
            .collect();
        for &(index, _) in &moves {
            self.release(index);
        }
        let mut taken = Vec::new();
        for (index, new) in moves {
            let (request, old, _) = self.hotkeys[index];
            self.hotkeys[index] = if register(self.window, index, new).is_ok() {
                (request, new, Ok(()))
            } else {
                // Only a held key fails RegisterHotKey here, and `taken` reports it.
                taken.push(request);
                (request, old, register(self.window, index, old))
            };
        }
        taken
    }

    /// Leaves Settings out of the tray menu, for the organisation's `HideSettings` policy (F-57).
    pub fn hide_settings_item(&self) {
        self.state().settings_item.set(false);
    }

    /// Shows or hides the menu's items for the thumbnail's capture (D-96).
    pub fn set_last_capture_items(&self, items: LastCaptureItems) {
        self.state().last_capture.set(items);
    }

    /// Whether the shell took the icon at the last try: at creation or when the taskbar last
    /// started.
    #[must_use]
    pub fn icon_shown(&self) -> bool {
        self.state().icon_shown.get()
    }

    /// Shows `text` under `title` in a warning balloon from the tray icon, cut to the
    /// shell's 63 and 255 characters.
    ///
    /// # Errors
    ///
    /// [`Error::WarningNotShown`] when the shell has no icon to show it from.
    pub fn warn(&self, title: &str, text: &str) -> Result<(), Error> {
        let mut icon = icon_data(self.window);
        icon.uFlags = NIF_INFO;
        icon.dwInfoFlags = NIIF_WARNING;
        fill(&mut icon.szInfoTitle, title);
        fill(&mut icon.szInfo, text);
        // SAFETY: `icon` names this window's icon and outlives the call.
        let shown = unsafe { Shell_NotifyIconW(NIM_MODIFY, &raw const icon) };
        if shown.as_bool() {
            Ok(())
        } else {
            Err(Error::WarningNotShown)
        }
    }

    /// Removes the tray icon as Cleanshot quits (F-50). The window stays until the tray is
    /// dropped.
    ///
    /// # Errors
    ///
    /// [`Error::IconNotRemoved`] when the shell had no icon to remove.
    pub fn remove_icon(&self) -> Result<(), Error> {
        let icon = icon_data(self.window);
        // SAFETY: `icon` names this window's icon and outlives the call.
        if !unsafe { Shell_NotifyIconW(NIM_DELETE, &raw const icon) }.as_bool() {
            return Err(Error::IconNotRemoved);
        }
        self.state().icon_shown.set(false);
        Ok(())
    }

    /// Unregisters the hotkey at `index` if Windows registered it.
    fn release(&self, index: usize) {
        if self.hotkeys[index].2.is_ok() {
            // SAFETY: the hotkey was registered on this window, which is still alive.
            let unregistered = unsafe { UnregisterHotKey(Some(self.window), hotkey_id(index)) };
            debug_assert!(unregistered.is_ok(), "a registered hotkey unregisters");
        }
    }

    fn state(&self) -> &State {
        // SAFETY: Tray::create stored the State before returning, and WM_NCDESTROY gives up
        // the window's count only when Drop destroys the window, so it lives as long as `self`.
        unsafe { window_state::attached(self.window) }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        for index in 0..self.hotkeys.len() {
            self.release(index);
        }
        let icon = icon_data(self.window);
        // SAFETY: `icon` names this window's icon. Deleting an icon the shell lost is harmless,
        // so the result is dropped.
        let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &raw const icon) };
        // SAFETY: the window belongs to this thread and has not been destroyed.
        let destroyed = unsafe { DestroyWindow(self.window) };
        debug_assert!(destroyed.is_ok(), "the tray window is destroyed once");
    }
}

/// Shows `text` under `title` in a warning box with an OK button, and returns when it is
/// closed. For a process with no tray icon to warn from.
///
/// # Errors
///
/// The Windows error if the box cannot be shown.
pub fn message_box(title: &str, text: &str) -> windows::core::Result<()> {
    // SAFETY: both strings live until MessageBoxW returns; the box has no owner window.
    let pressed = unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            &HSTRING::from(title),
            // With no owner the box would open behind whatever has focus.
            MB_OK | MB_ICONWARNING | MB_SETFOREGROUND | MB_TOPMOST,
        )
    };
    if pressed.0 == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(())
}

/// Asks the Cleanshot already running in this session to capture a region (C-14).
///
/// # Errors
///
/// [`Error::NoRunningTray`] if no tray window exists in this session, for example while the
/// other instance is still starting.
pub fn ask_running_instance_to_capture() -> Result<(), Error> {
    signal_capture(running_tray_window().map_err(Error::NoRunningTray)?)
}

/// The tray window of the Cleanshot running in this session.
pub(crate) fn running_tray_window() -> windows::core::Result<HWND> {
    // SAFETY: the class name is a 'static, NUL-terminated string.
    unsafe { FindWindowW(WINDOW_CLASS, PCWSTR::null()) }
}

fn signal_capture(window: HWND) -> Result<(), Error> {
    // SAFETY: posting copies no memory; the window may close first, which fails the post.
    unsafe { PostMessageW(Some(window), capture_signal(), WPARAM(0), LPARAM(0)) }
        .map_err(Error::NoRunningTray)
}

/// Registers `hotkey` on `window` as the hotkey at `index`, whose `WM_HOTKEY` id is one more.
fn register(window: HWND, index: usize, hotkey: Hotkey) -> Result<(), Error> {
    let (modifiers, key) = virtual_key(hotkey);
    // SAFETY: `window` is the tray's live window, on this thread.
    unsafe { RegisterHotKey(Some(window), hotkey_id(index), modifiers, key) }
        .map_err(Error::HotkeyTaken)
}

fn hotkey_id(index: usize) -> i32 {
    i32::try_from(index + 1).expect("a tray has a handful of hotkeys")
}

/// The `RegisterHotKey` modifiers and virtual key for `hotkey`. Auto-repeat never fires it
/// twice.
fn virtual_key(hotkey: Hotkey) -> (HOT_KEY_MODIFIERS, u32) {
    let held = hotkey.modifiers();
    let mut modifiers = MOD_NOREPEAT;
    for (down, flag) in [
        (held.ctrl, MOD_CONTROL),
        (held.alt, MOD_ALT),
        (held.shift, MOD_SHIFT),
        (held.win, MOD_WIN),
    ] {
        if down {
            modifiers |= flag;
        }
    }
    // Letters and digits are their own upper-case ASCII codes as virtual keys.
    let key = match hotkey.key() {
        Key::Letter(letter) | Key::Digit(letter) => u32::from(letter),
        Key::Function(number) => u32::from(VK_F1.0) + u32::from(number) - 1,
        Key::PrintScreen => u32::from(VK_SNAPSHOT.0),
    };
    (modifiers, key)
}

/// Whether Windows' "Use the Print screen key to open screen capture" switch is on, so a
/// bare `PrtSc` opens Snipping Tool and never reaches a registered hotkey (F-52). A switch
/// never set reads as on, Windows 11's default (D-33).
///
/// # Errors
///
/// [`Error::PrintScreenSetting`] if the value exists but cannot be read.
pub fn snipping_tool_holds_print_screen() -> Result<bool, Error> {
    let read = crate::registry::read_number(
        HKEY_CURRENT_USER,
        w!("Control Panel\\Keyboard"),
        w!("PrintScreenKeyForSnippingEnabled"),
    );
    match read {
        Ok(enabled) => Ok(enabled.is_none_or(|enabled| enabled != 0)),
        Err(refused) => Err(Error::PrintScreenSetting(refused.to_hresult().into())),
    }
}

/// F-52: `Ctrl+Shift+4` in place of a bare `PrtSc` that Snipping Tool holds; None when
/// `configured` can fire as it is.
///
/// # Panics
///
/// Never: the fallback's text is fixed and valid.
#[must_use]
pub fn print_screen_fallback(
    configured: Hotkey,
    snipping_tool_holds_print_screen: bool,
) -> Option<Hotkey> {
    let bare_print_screen =
        configured.key() == Key::PrintScreen && configured.modifiers() == Modifiers::default();
    (snipping_tool_holds_print_screen && bare_print_screen)
        .then(|| Hotkey::parse("Ctrl+Shift+4").expect("Ctrl+Shift+4 is a valid hotkey"))
}

fn capture_signal() -> u32 {
    // SAFETY: the argument is a 'static, NUL-terminated string.
    unsafe { RegisterWindowMessageW(CAPTURE_SIGNAL) }
}

fn icon_data(window: HWND) -> NOTIFYICONDATAW {
    let mut icon = NOTIFYICONDATAW {
        cbSize: struct_size::<NOTIFYICONDATAW>(),
        hWnd: window,
        uID: ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: ICON_MESSAGE,
        ..Default::default()
    };
    icon.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    fill(&mut icon.szTip, "Cleanshot");
    icon
}

/// Copies as much of `text` as fits, leaving the last slot as the terminating NUL.
fn fill(slots: &mut [u16], text: &str) {
    let room = slots.len().saturating_sub(1);
    for (slot, unit) in slots[..room].iter_mut().zip(text.encode_utf16()) {
        *slot = unit;
    }
}

// F-50: refused while the taskbar is down or the glyph fails to load; TaskbarCreated retries.
fn add_icon(window: HWND, state: &State) -> bool {
    let mut icon = icon_data(window);
    let Ok(glyph) = TrayIcon::new(window, TaskbarShade::now()) else {
        return false;
    };
    icon.hIcon = glyph.handle();
    state.icon.replace(Some(glyph));
    // SAFETY: `icon` is fully initialised and outlives both calls.
    unsafe {
        let added = Shell_NotifyIconW(NIM_ADD, &raw const icon).as_bool();
        if added {
            let versioned = Shell_NotifyIconW(NIM_SETVERSION, &raw const icon);
            debug_assert!(
                versioned.as_bool(),
                "the shell takes version 4 for an icon it added"
            );
        }
        added
    }
}

/// Gives the shell the glyph for the taskbar as it is now. An icon the shell does not have
/// is left to `TaskbarCreated`.
fn redraw_icon(window: HWND, state: &State) {
    if !state.icon_shown.get() {
        return;
    }
    let Ok(glyph) = TrayIcon::new(window, TaskbarShade::now()) else {
        return;
    };
    let mut icon = icon_data(window);
    icon.uFlags = NIF_ICON;
    icon.hIcon = glyph.handle();
    // SAFETY: `icon` names this window's icon and outlives the call.
    if unsafe { Shell_NotifyIconW(NIM_MODIFY, &raw const icon) }.as_bool() {
        state.icon.replace(Some(glyph));
    }
}

/// The tray menu's items in order, each with its command id and flags.
fn menu_items(
    settings_item: bool,
    last_capture: LastCaptureItems,
) -> Vec<(usize, HSTRING, MENU_ITEM_FLAGS)> {
    let mut items = Vec::new();
    if let LastCaptureItems::Shown { saving_off } = last_capture {
        let save = if saving_off { MF_GRAYED } else { MF_STRING };
        items.push((
            MENU_EDIT_LAST,
            HSTRING::from("Edit last capture"),
            MF_STRING,
        ));
        items.push((MENU_SAVE_LAST, HSTRING::from("Save last capture"), save));
    }
    items.push((
        MENU_CAPTURE_REGION,
        HSTRING::from(HotkeyAction::Region.name()),
        MF_STRING,
    ));
    items.push((
        MENU_CAPTURE_SCREEN,
        HSTRING::from(HotkeyAction::FullScreen.name()),
        MF_STRING,
    ));
    if settings_item {
        items.push((MENU_SETTINGS, HSTRING::from("Settings"), MF_STRING));
    }
    items.push((MENU_EXIT, HSTRING::from("Exit"), MF_STRING));
    items
}

fn show_menu(window: HWND, state: &State) -> Option<Request> {
    // SAFETY: the menu is created, shown and destroyed here, all on this window's thread.
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let built = menu_items(state.settings_item.get(), state.last_capture.get())
            .iter()
            .try_for_each(|(id, text, flags)| AppendMenuW(menu, *flags, *id, text));
        let mut cursor = POINT::default();
        let chosen = if built.is_ok() && GetCursorPos(&raw mut cursor).is_ok() {
            // The menu closes on an outside click only if its window is in the foreground; when
            // Windows refuses, the menu still works and stays open until Esc.
            let _ = SetForegroundWindow(window);
            let chosen = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                cursor.x,
                cursor.y,
                None,
                window,
                None,
            );
            // Wakes the window so the next click outside opens no stale menu; a lost wake-up
            // only costs that.
            let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
            usize::try_from(chosen.0).ok()
        } else {
            None
        };
        let destroyed = DestroyMenu(menu);
        debug_assert!(destroyed.is_ok(), "a menu created here is destroyed once");
        match chosen? {
            MENU_CAPTURE_REGION => Some(Request::Capture(HotkeyAction::Region)),
            MENU_CAPTURE_SCREEN => Some(Request::Capture(HotkeyAction::FullScreen)),
            MENU_EDIT_LAST => Some(Request::EditLastCapture),
            MENU_SAVE_LAST => Some(Request::SaveLastCapture),
            MENU_SETTINGS => Some(Request::OpenSettings),
            MENU_EXIT => Some(Request::Exit),
            _ => None,
        }
    }
}

unsafe extern "system" fn window_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Tray::create stores a State on the one window of this class. The count this
    // message holds keeps a Tray dropped while it runs (from its on_request, on Exit) from
    // freeing the State until the message returns.
    let Some(held) = (unsafe { window_state::for_message::<State>(window, message) }) else {
        // SAFETY: forwarding an unhandled message to the default procedure.
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    };
    let state: &State = &held;

    let request = match message {
        WM_HOTKEY => wparam
            .0
            .checked_sub(1)
            .and_then(|index| state.hotkey_requests.get(index))
            .copied(),
        WM_CLOSE => Some(Request::Exit),
        ICON_MESSAGE => match u32::try_from(lparam.0 & 0xFFFF) {
            Ok(WM_LBUTTONDBLCLK) => Some(Request::Capture(HotkeyAction::Region)),
            Ok(WM_CONTEXTMENU) => show_menu(window, state),
            _ => None,
        },
        message if message == state.capture_signal => Some(Request::Capture(HotkeyAction::Region)),
        message if message == state.taskbar_created => {
            state.icon_shown.set(add_icon(window, state));
            None
        }
        // D-92: the taskbar's theme, high contrast, its colours or the DPI may have changed.
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_DPICHANGED | WM_DISPLAYCHANGE => {
            redraw_icon(window, state);
            // SAFETY: forwarding the notification to the default procedure as well.
            return unsafe { DefWindowProcW(window, message, wparam, lparam) };
        }
        _ => {
            // SAFETY: forwarding an unhandled message to the default procedure.
            return unsafe { DefWindowProcW(window, message, wparam, lparam) };
        }
    };
    if let Some(request) = request {
        state.send(request);
    }
    LRESULT(0)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use windows::Win32::UI::Input::KeyboardAndMouse::VK_F24;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, IsWindow, MSG, PM_REMOVE, PeekMessageW, SendMessageW,
    };

    use super::*;

    fn hotkey(text: &str) -> Hotkey {
        Hotkey::parse(text).expect("a valid hotkey")
    }

    #[test]
    fn f51_a_hotkey_maps_to_its_modifiers_and_virtual_key() {
        assert_eq!(
            virtual_key(hotkey("Ctrl+Shift+4")),
            (MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, u32::from(b'4'))
        );
        assert_eq!(
            virtual_key(hotkey("Win+Alt+Q")),
            (MOD_WIN | MOD_ALT | MOD_NOREPEAT, u32::from(b'Q'))
        );
        assert_eq!(
            virtual_key(hotkey("F24")),
            (MOD_NOREPEAT, u32::from(VK_F24.0))
        );
        assert_eq!(
            virtual_key(hotkey("PrtSc")),
            (MOD_NOREPEAT, u32::from(VK_SNAPSHOT.0))
        );
    }

    #[test]
    fn f52_only_a_bare_print_screen_that_snipping_tool_holds_falls_back() {
        let fallback = Some(hotkey("Ctrl+Shift+4"));
        assert_eq!(print_screen_fallback(hotkey("PrtSc"), true), fallback);
        assert_eq!(print_screen_fallback(hotkey("PrtSc"), false), None);
        assert_eq!(print_screen_fallback(hotkey("Shift+PrtSc"), true), None);
        assert_eq!(print_screen_fallback(hotkey("Ctrl+Alt+Q"), true), None);
    }

    #[test]
    fn f07_each_hotkey_sends_its_own_request() {
        let received = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&received);
        let tray = Tray::create(
            &[
                (
                    Request::Capture(HotkeyAction::Region),
                    hotkey("Ctrl+Alt+F17"),
                ),
                (
                    Request::Capture(HotkeyAction::FullScreen),
                    hotkey("Ctrl+Alt+F18"),
                ),
            ],
            move |request| sink.borrow_mut().push(request),
        )
        .expect("the tray window is created");
        for id in [2, 1] {
            // SAFETY: posting copies no memory, and the window belongs to this thread.
            unsafe { PostMessageW(Some(tray.window), WM_HOTKEY, WPARAM(id), LPARAM(0)) }
                .expect("the tray window takes the message");
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while received.borrow().len() < 2 && Instant::now() < deadline {
            let mut message = MSG::default();
            // SAFETY: `message` is writable and the tray window belongs to this thread.
            while unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                // SAFETY: `message` was just filled by PeekMessageW.
                unsafe { DispatchMessageW(&raw const message) };
            }
        }
        drop(tray);
        assert_eq!(
            *received.borrow(),
            [
                Request::Capture(HotkeyAction::FullScreen),
                Request::Capture(HotkeyAction::Region)
            ]
        );
    }

    #[test]
    fn f07_a_tray_dropped_by_its_exit_request_is_freed_only_once_the_message_returns() {
        struct Freed(Rc<Cell<bool>>);
        impl Drop for Freed {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let freed = Rc::new(Cell::new(false));
        let guard = Freed(Rc::clone(&freed));
        let slot: Rc<RefCell<Option<Tray>>> = Rc::default();
        let freed_during = Rc::new(Cell::new(None));
        let (closing, watching, report) = (
            Rc::clone(&slot),
            Rc::clone(&freed),
            Rc::clone(&freed_during),
        );
        let tray = Tray::create(&[], move |request| {
            let Freed(_) = &guard;
            if request == Request::Exit {
                drop(closing.borrow_mut().take());
                report.set(Some(watching.get()));
            }
        })
        .expect("the tray window is created");
        let window = tray.window;
        *slot.borrow_mut() = Some(tray);
        // SAFETY: the window belongs to this thread, so this runs its procedure right here.
        unsafe { SendMessageW(window, WM_CLOSE, None, None) };
        assert_eq!(
            freed_during.get(),
            Some(false),
            "freed under the running request"
        );
        assert!(freed.get(), "freed once the message returned");
        // SAFETY: IsWindow only reads whether the handle still names a window.
        assert!(!unsafe { IsWindow(Some(window)) }.as_bool());
    }

    #[test]
    fn f51_a_hotkey_already_held_is_reported_not_fatal() {
        let taken = hotkey("Ctrl+Alt+F15");
        let free = hotkey("Ctrl+Alt+F16");
        let first = Tray::create(&[(Request::Capture(HotkeyAction::Region), taken)], |_| {})
            .expect("the first tray");
        let second = Tray::create(
            &[
                (Request::Capture(HotkeyAction::Region), taken),
                (Request::Capture(HotkeyAction::FullScreen), free),
            ],
            |_| {},
        )
        .expect("the second tray");
        assert!(first.hotkeys().all(|(_, _, registered)| registered.is_ok()));
        let outcomes: Vec<_> = second.hotkeys().collect();
        assert!(matches!(
            outcomes[0],
            (
                Request::Capture(HotkeyAction::Region),
                _,
                Err(Error::HotkeyTaken(_))
            )
        ));
        assert!(matches!(
            outcomes[1],
            (Request::Capture(HotkeyAction::FullScreen), _, Ok(()))
        ));
    }

    #[test]
    fn f55_changed_hotkeys_can_swap_and_one_another_program_holds_keeps_its_old_key() {
        let (first, second, held) = (
            hotkey("Ctrl+Alt+F19"),
            hotkey("Ctrl+Alt+F20"),
            hotkey("Ctrl+Alt+F21"),
        );
        let (region, screen) = (
            Request::Capture(HotkeyAction::Region),
            Request::Capture(HotkeyAction::FullScreen),
        );
        let in_use = |key| {
            let probe = Tray::create(&[(region, key)], |_| {}).expect("the probe tray");
            probe
                .hotkeys()
                .all(|(_, _, registered)| registered.is_err())
        };
        let mut tray =
            Tray::create(&[(region, first), (screen, second)], |_| {}).expect("the tray");
        let swapped = tray.set_hotkeys(&[(region, second), (screen, first)]);
        assert_eq!(swapped, []);
        let now: Vec<_> = tray.hotkeys().collect();
        assert!(matches!(now[0], (_, key, Ok(())) if key == second));
        assert!(matches!(now[1], (_, key, Ok(())) if key == first));

        let other = Tray::create(&[(region, held)], |_| {}).expect("the other program");
        assert_eq!(tray.set_hotkeys(&[(region, held)]), [region]);
        assert!(matches!(tray.hotkeys().next(), Some((_, key, Ok(()))) if key == second));
        assert!(in_use(second), "the region keeps its old key registered");
        drop(other);
    }

    #[test]
    fn c14_a_second_launch_signal_asks_the_tray_to_capture() {
        let received = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&received);
        let tray = Tray::create(
            &[(
                Request::Capture(HotkeyAction::Region),
                hotkey("Ctrl+Alt+F13"),
            )],
            move |request| {
                sink.borrow_mut().push(request);
            },
        )
        .expect("the tray window is created");
        // Other tests' trays share the window class, so the signal goes to this test's own.
        signal_capture(tray.window).expect("the tray window takes the signal");
        let deadline = Instant::now() + Duration::from_secs(5);
        while received.borrow().is_empty() && Instant::now() < deadline {
            let mut message = MSG::default();
            // SAFETY: `message` is writable and the tray window belongs to this thread.
            while unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                // SAFETY: `message` was just filled by PeekMessageW.
                unsafe { DispatchMessageW(&raw const message) };
            }
        }
        drop(tray);
        assert_eq!(*received.borrow(), [Request::Capture(HotkeyAction::Region)]);
    }

    #[test]
    fn f54_a_settings_warning_shows_from_the_tray_icon() {
        let tray = Tray::create(
            &[(
                Request::Capture(HotkeyAction::Region),
                hotkey("Ctrl+Alt+F14"),
            )],
            |_| {},
        )
        .expect("the tray window is created");
        assert!(tray.icon_shown(), "the runner's shell takes the icon");
        let long = "x".repeat(400);
        tray.warn("Cleanshot settings", &long)
            .expect("the balloon is shown from the icon");
    }

    #[test]
    fn f57_the_tray_menu_leaves_out_settings_only_under_hide_settings() {
        let ids = |shown| {
            menu_items(shown, LastCaptureItems::Hidden)
                .into_iter()
                .map(|(id, _, _)| id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(true),
            [
                MENU_CAPTURE_REGION,
                MENU_CAPTURE_SCREEN,
                MENU_SETTINGS,
                MENU_EXIT
            ]
        );
        assert_eq!(
            ids(false),
            [MENU_CAPTURE_REGION, MENU_CAPTURE_SCREEN, MENU_EXIT]
        );
    }

    #[test]
    fn f09_while_the_thumbnail_shows_the_menu_leads_with_its_items_and_greys_save_under_saving_off()
    {
        let leading = |items| {
            menu_items(true, items)
                .into_iter()
                .take(2)
                .map(|(id, _, flags)| (id, flags))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            leading(LastCaptureItems::Shown { saving_off: false }),
            [(MENU_EDIT_LAST, MF_STRING), (MENU_SAVE_LAST, MF_STRING)]
        );
        assert_eq!(
            leading(LastCaptureItems::Shown { saving_off: true }),
            [(MENU_EDIT_LAST, MF_STRING), (MENU_SAVE_LAST, MF_GRAYED)]
        );
        assert_eq!(
            leading(LastCaptureItems::Hidden),
            [
                (MENU_CAPTURE_REGION, MF_STRING),
                (MENU_CAPTURE_SCREEN, MF_STRING)
            ]
        );
    }
}
