use std::cell::{Cell, RefCell};

use cleanshot_core::{Hotkey, Key, Modifiers};
use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
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
    FindWindowW, GWLP_USERDATA, GetCursorPos, GetWindowLongPtrW, IDI_APPLICATION, LoadIconW,
    MB_ICONWARNING, MB_OK, MF_STRING, MessageBoxW, PostMessageW, RegisterWindowMessageW,
    SetForegroundWindow, SetWindowLongPtrW, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP,
    WM_CLOSE, WM_CONTEXTMENU, WM_HOTKEY, WM_LBUTTONDBLCLK, WM_NCDESTROY, WM_NULL, WNDCLASSEXW,
    WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::Error;

const WINDOW_CLASS: PCWSTR = w!("CleanshotTray");
// C-14: the only message a second launch sends. Its parameters are always zero and never read.
const CAPTURE_SIGNAL: PCWSTR = w!("Cleanshot.CaptureRegion");
const ICON_MESSAGE: u32 = WM_APP + 1;
const ICON_ID: u32 = 1;
const MENU_CAPTURE_REGION: usize = 1;
const MENU_CAPTURE_SCREEN: usize = 2;
const MENU_EXIT: usize = 3;

/// What the user asked for through the tray icon, its menu or a global hotkey.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    CaptureRegion,
    CaptureScreen,
    CaptureLastRegion,
    Exit,
}

struct State {
    on_request: RefCell<Box<dyn FnMut(Request)>>,
    taskbar_created: u32,
    capture_signal: u32,
    icon_shown: Cell<bool>,
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
        // SAFETY: a null module name asks for this exe's own handle, which lives as long as
        // the process.
        let instance = unsafe { GetModuleHandleW(None) }.map_err(Error::Window)?;
        let class = WNDCLASSEXW {
            cbSize: struct_size::<WNDCLASSEXW>(),
            lpfnWndProc: Some(window_procedure),
            hInstance: instance.into(),
            lpszClassName: WINDOW_CLASS,
            ..Default::default()
        };
        crate::register_window_class(&class)?;

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
                Some(instance.into()),
                None,
            )
        }
        .map_err(Error::Window)?;
        let state = Box::new(State {
            on_request: RefCell::new(Box::new(on_request)),
            // SAFETY: the argument is a 'static, NUL-terminated string.
            taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
            capture_signal: capture_signal(),
            hotkey_requests: hotkeys.iter().map(|(request, _)| *request).collect(),
            icon_shown: Cell::new(false),
        });
        // SAFETY: `window` exists, so it will get WM_NCDESTROY, where the window procedure
        // frees this Box once.
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, Box::into_raw(state) as isize) };

        let hotkeys = hotkeys
            .iter()
            .zip(1..)
            .map(|(&(request, hotkey), id)| {
                let (modifiers, key) = virtual_key(hotkey);
                // SAFETY: `window` was created above on this thread.
                let registered = unsafe { RegisterHotKey(Some(window), id, modifiers, key) };
                (request, hotkey, registered.map_err(Error::HotkeyTaken))
            })
            .collect();
        let tray = Self { window, hotkeys };
        tray.state().icon_shown.set(add_icon(window));
        Ok(tray)
    }

    /// Each hotkey, the request it sends, and whether Windows registered it.
    pub fn hotkeys(&self) -> impl Iterator<Item = (Request, Hotkey, Result<(), &Error>)> {
        self.hotkeys
            .iter()
            .map(|(request, hotkey, registered)| (*request, *hotkey, registered.as_ref().copied()))
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

    fn state(&self) -> &State {
        // SAFETY: Tray::create stored the State before returning, and WM_NCDESTROY frees it
        // only when Drop destroys the window, so it lives as long as `self`.
        unsafe { &*(GetWindowLongPtrW(self.window, GWLP_USERDATA) as *const State) }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        for ((_, _, registered), id) in self.hotkeys.iter().zip(1..) {
            if registered.is_ok() {
                // SAFETY: the hotkey was registered on this window, which is still alive.
                let unregistered = unsafe { UnregisterHotKey(Some(self.window), id) };
                debug_assert!(unregistered.is_ok(), "a registered hotkey unregisters");
            }
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
            MB_OK | MB_ICONWARNING,
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
    // SAFETY: the class name is a 'static, NUL-terminated string.
    let window =
        unsafe { FindWindowW(WINDOW_CLASS, PCWSTR::null()) }.map_err(Error::NoRunningTray)?;
    signal_capture(window)
}

fn signal_capture(window: HWND) -> Result<(), Error> {
    // SAFETY: posting copies no memory; the window may close first, which fails the post.
    unsafe { PostMessageW(Some(window), capture_signal(), WPARAM(0), LPARAM(0)) }
        .map_err(Error::NoRunningTray)
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
    let mut enabled = 0u32;
    let mut bytes = struct_size::<u32>();
    // SAFETY: `enabled` is writable for `bytes` bytes and RRF_RT_REG_DWORD writes exactly 4.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Control Panel\\Keyboard"),
            w!("PrintScreenKeyForSnippingEnabled"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut enabled).cast()),
            Some(&raw mut bytes),
        )
    };
    match read {
        ERROR_SUCCESS => Ok(enabled != 0),
        ERROR_FILE_NOT_FOUND => Ok(true),
        refused => Err(Error::PrintScreenSetting(refused.to_hresult().into())),
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

fn struct_size<T>() -> u32 {
    u32::try_from(size_of::<T>()).expect("a Win32 struct is smaller than 4 GiB")
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

// F-50: refused while the taskbar is down or the system icon fails to load; TaskbarCreated retries.
fn add_icon(window: HWND) -> bool {
    let mut icon = icon_data(window);
    // SAFETY: IDI_APPLICATION is a system icon, so no module handle is needed.
    let Ok(system_icon) = (unsafe { LoadIconW(None, IDI_APPLICATION) }) else {
        return false;
    };
    icon.hIcon = system_icon;
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

fn show_menu(window: HWND) -> Option<Request> {
    // SAFETY: the menu is created, shown and destroyed here, all on this window's thread.
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let built = AppendMenuW(menu, MF_STRING, MENU_CAPTURE_REGION, w!("Capture region"))
            .and_then(|()| AppendMenuW(menu, MF_STRING, MENU_CAPTURE_SCREEN, w!("Capture screen")))
            .and_then(|()| AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("Exit")));
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
            MENU_CAPTURE_REGION => Some(Request::CaptureRegion),
            MENU_CAPTURE_SCREEN => Some(Request::CaptureScreen),
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
    // SAFETY: GWLP_USERDATA is 0 until Tray::create stores the State pointer, and WM_NCDESTROY
    // clears it.
    let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const State;
    if state.is_null() {
        // SAFETY: forwarding an unhandled message to the default procedure.
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    if message == WM_NCDESTROY {
        // SAFETY: this is the window's last message; the Box is freed once and the pointer
        // cleared, so nothing reads it again.
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            drop(Box::from_raw(state.cast_mut()));
        }
        // SAFETY: forwarding to the default procedure.
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    // SAFETY: a non-null pointer is live until WM_NCDESTROY, handled above.
    let state = unsafe { &*state };

    let request = match message {
        WM_HOTKEY => wparam
            .0
            .checked_sub(1)
            .and_then(|index| state.hotkey_requests.get(index))
            .copied(),
        WM_CLOSE => Some(Request::Exit),
        ICON_MESSAGE => match u32::try_from(lparam.0 & 0xFFFF) {
            Ok(WM_LBUTTONDBLCLK) => Some(Request::CaptureRegion),
            Ok(WM_CONTEXTMENU) => show_menu(window),
            _ => None,
        },
        message if message == state.capture_signal => Some(Request::CaptureRegion),
        message if message == state.taskbar_created => {
            state.icon_shown.set(add_icon(window));
            None
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
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW};

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
                (Request::CaptureRegion, hotkey("Ctrl+Alt+F17")),
                (Request::CaptureScreen, hotkey("Ctrl+Alt+F18")),
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
            [Request::CaptureScreen, Request::CaptureRegion]
        );
    }

    #[test]
    fn f51_a_hotkey_already_held_is_reported_not_fatal() {
        let taken = hotkey("Ctrl+Alt+F15");
        let free = hotkey("Ctrl+Alt+F16");
        let first =
            Tray::create(&[(Request::CaptureRegion, taken)], |_| {}).expect("the first tray");
        let second = Tray::create(
            &[
                (Request::CaptureRegion, taken),
                (Request::CaptureScreen, free),
            ],
            |_| {},
        )
        .expect("the second tray");
        assert!(first.hotkeys().all(|(_, _, registered)| registered.is_ok()));
        let outcomes: Vec<_> = second.hotkeys().collect();
        assert!(matches!(
            outcomes[0],
            (Request::CaptureRegion, _, Err(Error::HotkeyTaken(_)))
        ));
        assert!(matches!(outcomes[1], (Request::CaptureScreen, _, Ok(()))));
    }

    #[test]
    fn c14_a_second_launch_signal_asks_the_tray_to_capture() {
        let received = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&received);
        let tray = Tray::create(
            &[(Request::CaptureRegion, hotkey("Ctrl+Alt+F13"))],
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
        assert_eq!(*received.borrow(), [Request::CaptureRegion]);
    }

    #[test]
    fn f54_a_settings_warning_shows_from_the_tray_icon() {
        let tray = Tray::create(&[(Request::CaptureRegion, hotkey("Ctrl+Alt+F14"))], |_| {})
            .expect("the tray window is created");
        assert!(tray.icon_shown(), "the runner's shell takes the icon");
        let long = "x".repeat(400);
        tray.warn("Cleanshot settings", &long)
            .expect("the balloon is shown from the icon");
    }
}
