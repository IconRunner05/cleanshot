use cleanshot_core::{Pickable, Rect};
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DWMWINDOWATTRIBUTE, DwmGetWindowAttribute,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetShellWindow, GetWindowLongW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, WS_EX_LAYERED, WS_EX_TRANSPARENT,
};
use windows::core::BOOL;

use super::{MonitorImage, screen_rect};
use crate::Error;

/// The windows a window capture can pick, topmost first, as they are now:
/// visible, uncloaked, unminimised top-level windows of other processes or our editor and
/// settings windows, cut to the captured monitors.
///
/// # Errors
///
/// [`Error::Capture`] if Windows cannot list its windows.
pub fn pickable_windows(captures: &[MonitorImage]) -> Result<Vec<Pickable>, Error> {
    let mut listed: Vec<HWND> = Vec::new();
    let target = LPARAM(std::ptr::from_mut(&mut listed) as isize);
    // SAFETY: the callback runs only during EnumWindows, while `listed` is borrowed here.
    unsafe { EnumWindows(Some(collect), target) }.map_err(Error::Capture)?;
    let Some(desktop) = Rect::bounding(captures.iter().map(|capture| screen_rect(capture.bounds)))
    else {
        return Ok(Vec::new());
    };
    Ok(listed
        .into_iter()
        .filter(|&window| pickable(window))
        .filter_map(|window| {
            let frame = frame(window).and_then(|frame| screen_rect(frame).overlap(desktop))?;
            Some(Pickable {
                frame,
                title: title(window),
            })
        })
        .collect())
}

extern "system" fn collect(window: HWND, listed: LPARAM) -> BOOL {
    // SAFETY: pickable_windows passes a pointer to its Vec, alive for the whole EnumWindows
    // call, and nothing else touches it meanwhile. EnumWindows calls this only during that
    // call, with that LPARAM; no window message carries it, so no other process can supply it.
    // ast-grep-ignore: pointer-from-handle-value
    unsafe { &mut *(listed.0 as *mut Vec<HWND>) }.push(window);
    true.into()
}

pub(crate) fn pickable(window: HWND) -> bool {
    let mut process = 0;
    // SAFETY: `window` is a window handle, listed or a test's, and `process` is writable.
    unsafe { GetWindowThreadProcessId(window, Some(&raw mut process)) };
    // SAFETY: plain reads of a window's state; a window closed since EnumWindows reads as not
    // visible.
    let (own, visible, minimised, shell, extended) = unsafe {
        (
            process == GetCurrentProcessId(),
            IsWindowVisible(window).as_bool(),
            IsIconic(window).as_bool(),
            window == GetShellWindow(),
            GetWindowLongW(window, GWL_EXSTYLE).cast_unsigned(),
        )
    };
    // A layered window the pointer passes through is an overlay no click could reach.
    let click_through = WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0;
    // Of our own windows only the editor's and the settings window's are offered.
    (!own || class_is(window, GPUI_WINDOW_CLASS))
        && visible
        && !minimised
        && !shell
        && extended & click_through != click_through
        && !cloaked(window)
        && !class_is(window, WALLPAPER_CLASS)
}

fn cloaked(window: HWND) -> bool {
    let mut cloaked = 0_u32;
    attribute(window, DWMWA_CLOAKED, &mut cloaked).map_or(true, |()| cloaked != 0)
}

/// The class of the window Windows draws the wallpaper in, behind the desktop's icons.
const WALLPAPER_CLASS: &str = "WorkerW";
/// The class of every GPUI window, as the editor and the settings window are.
const GPUI_WINDOW_CLASS: &str = "Zed::Window";

fn class_is(window: HWND, name: &str) -> bool {
    let mut class = [0_u16; 32];
    // SAFETY: `class` is writable for its whole length.
    let length = unsafe { GetClassNameW(window, &mut class) };
    usize::try_from(length)
        .is_ok_and(|length| class[..length].iter().copied().eq(name.encode_utf16()))
}

fn title(window: HWND) -> String {
    let mut title = [0_u16; 256];
    // SAFETY: `title` is writable for its whole length; a window with no title reads as empty.
    let length = unsafe { GetWindowTextW(window, &mut title) };
    String::from_utf16_lossy(&title[..usize::try_from(length).unwrap_or(0)])
}

fn frame(window: HWND) -> Option<RECT> {
    let mut frame = RECT::default();
    attribute(window, DWMWA_EXTENDED_FRAME_BOUNDS, &mut frame).ok()?;
    Some(frame)
}

fn attribute<T>(
    window: HWND,
    attribute: DWMWINDOWATTRIBUTE,
    value: &mut T,
) -> windows::core::Result<()> {
    // SAFETY: `value` is writable for the size given, the size this attribute writes.
    unsafe {
        DwmGetWindowAttribute(
            window,
            attribute,
            std::ptr::from_mut(value).cast(),
            crate::struct_size::<T>(),
        )
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::{LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, SW_SHOWNOACTIVATE, ShowWindow,
        WS_EX_NOACTIVATE, WS_POPUP,
    };
    use windows::core::{PCWSTR, w};

    use super::*;
    use crate::overlay::tests::screen;

    extern "system" fn plain(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // SAFETY: the default procedure answers any message for any window.
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    /// A shown window of ours of `class`, destroyed when the test ends.
    struct Shown(HWND);

    impl Shown {
        fn new(class: PCWSTR) -> Self {
            crate::register_window_class(class, plain).expect("the class registers");
            // SAFETY: the class is registered and every string is 'static.
            let window = unsafe {
                CreateWindowExW(
                    WS_EX_NOACTIVATE,
                    class,
                    w!("Cleanshot pick test"),
                    WS_POPUP,
                    100,
                    100,
                    200,
                    200,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .expect("the window is made");
            // SAFETY: the window was made on this thread just above. ShowWindow returns
            // whether it was visible before, which it was not.
            let _ = unsafe { ShowWindow(window, SW_SHOWNOACTIVATE) };
            Self(window)
        }
    }

    impl Drop for Shown {
        fn drop(&mut self) {
            // SAFETY: the window was made on this thread and is destroyed once.
            let destroyed = unsafe { DestroyWindow(self.0) };
            debug_assert!(destroyed.is_ok(), "a test window is destroyed");
        }
    }

    #[test]
    fn a_window_of_ours_not_made_by_gpui_is_not_offered() {
        let _screen = screen();
        let ours = Shown::new(w!("CleanshotPickTest"));
        assert!(
            !pickable(ours.0),
            "a window of ours that is not GPUI's is not offered"
        );
    }
}
