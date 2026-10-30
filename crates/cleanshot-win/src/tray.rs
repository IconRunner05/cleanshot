use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey,
    VK_SNAPSHOT,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    GWLP_USERDATA, GetCursorPos, GetWindowLongPtrW, IDI_APPLICATION, LoadIconW, MF_STRING,
    PostMessageW, RegisterClassExW, RegisterWindowMessageW, SetForegroundWindow, SetWindowLongPtrW,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_HOTKEY,
    WM_LBUTTONDBLCLK, WM_NCDESTROY, WM_NULL, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::Error;

const WINDOW_CLASS: PCWSTR = w!("CleanshotTray");
const ICON_MESSAGE: u32 = WM_APP + 1;
const ICON_ID: u32 = 1;
const MENU_CAPTURE_REGION: usize = 1;
const MENU_EXIT: usize = 2;

/// What the user asked for through the tray icon, its menu or a global hotkey.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    CaptureRegion,
    Exit,
}

/// A global hotkey that captures a region (F-51).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hotkey {
    PrintScreen,
    CtrlShift4,
}

impl Hotkey {
    const ALL: [Self; 2] = [Self::PrintScreen, Self::CtrlShift4];

    fn id(self) -> i32 {
        match self {
            Self::PrintScreen => 1,
            Self::CtrlShift4 => 2,
        }
    }

    fn keys(self) -> (HOT_KEY_MODIFIERS, u32) {
        match self {
            Self::PrintScreen => (MOD_NOREPEAT, u32::from(VK_SNAPSHOT.0)),
            Self::CtrlShift4 => (MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, u32::from(b'4')),
        }
    }
}

struct State {
    on_request: RefCell<Box<dyn FnMut(Request)>>,
    taskbar_created: u32,
    icon_shown: Cell<bool>,
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
    hotkeys: Vec<(Hotkey, Result<(), Error>)>,
}

impl Tray {
    /// Adds the tray icon and registers every [`Hotkey`]. A hotkey another program holds is
    /// reported by [`Tray::hotkeys`], not as an error here.
    ///
    /// # Errors
    ///
    /// [`Error::Window`] if the hidden window cannot be created. A shell that refuses the icon
    /// is not an error: the icon appears when the taskbar next starts.
    pub fn create(on_request: impl FnMut(Request) + 'static) -> Result<Self, Error> {
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
        // SAFETY: `class` is fully initialised and its strings are 'static.
        if unsafe { RegisterClassExW(&raw const class) } == 0 {
            return Err(Error::Window(windows::core::Error::from_thread()));
        }

        // F-50: a top-level window, not HWND_MESSAGE, because message-only windows miss the
        // TaskbarCreated broadcast that re-adds the icon after Explorer restarts.
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
            icon_shown: Cell::new(false),
        });
        // SAFETY: `window` exists, so it will get WM_NCDESTROY, where the window procedure
        // frees this Box once.
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, Box::into_raw(state) as isize) };

        let hotkeys = Hotkey::ALL
            .into_iter()
            .map(|hotkey| {
                let (modifiers, key) = hotkey.keys();
                // SAFETY: `window` was created above on this thread.
                let result = unsafe { RegisterHotKey(Some(window), hotkey.id(), modifiers, key) };
                (hotkey, result.map_err(Error::HotkeyTaken))
            })
            .collect();
        let tray = Self { window, hotkeys };
        tray.state().icon_shown.set(add_icon(window));
        Ok(tray)
    }

    /// The outcome of registering each [`Hotkey`].
    #[must_use]
    pub fn hotkeys(&self) -> &[(Hotkey, Result<(), Error>)] {
        &self.hotkeys
    }

    /// Whether the shell took the icon at the last try: at creation or when the taskbar last
    /// started.
    #[must_use]
    pub fn icon_shown(&self) -> bool {
        self.state().icon_shown.get()
    }

    fn state(&self) -> &State {
        // SAFETY: Tray::create stored the State before returning, and WM_NCDESTROY frees it
        // only when Drop destroys the window, so it lives as long as `self`.
        unsafe { &*(GetWindowLongPtrW(self.window, GWLP_USERDATA) as *const State) }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        for (hotkey, result) in &self.hotkeys {
            if result.is_ok() {
                // SAFETY: the hotkey was registered on this window, which is still alive.
                let unregistered = unsafe { UnregisterHotKey(Some(self.window), hotkey.id()) };
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
    for (slot, unit) in icon.szTip.iter_mut().zip("Cleanshot".encode_utf16()) {
        *slot = unit;
    }
    icon
}

// F-50: the shell refuses the icon while the taskbar is not up, and then sends TaskbarCreated,
// which calls this again. A system icon that fails to load counts as a refusal.
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
        WM_HOTKEY => Some(Request::CaptureRegion),
        WM_CLOSE => Some(Request::Exit),
        ICON_MESSAGE => match u32::try_from(lparam.0 & 0xFFFF) {
            Ok(WM_LBUTTONDBLCLK) => Some(Request::CaptureRegion),
            Ok(WM_CONTEXTMENU) => show_menu(window),
            _ => None,
        },
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
