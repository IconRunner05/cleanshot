use std::rc::Rc;

use windows::Win32::Foundation::{HWND, LPARAM, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    GWLP_USERDATA, GetSystemMetrics, GetWindowLongPtrW, SM_CXDRAG, SM_CYDRAG, SetWindowLongPtrW,
    WM_NCDESTROY,
};

/// Stores `state` on `window`, holding one count of it until the window's `WM_NCDESTROY`.
///
/// # Safety
///
/// `window` exists, nothing is stored on it yet, and its window procedure passes every message
/// to [`for_message`] with this same `T`.
pub(crate) unsafe fn attach<T>(window: HWND, state: Rc<T>) {
    // SAFETY: the caller's window exists; the pointer is read back only as an Rc<T>.
    unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, Rc::into_raw(state) as isize) };
}

/// The state [`attach`] stored on `window`, with a count of its own for `message`, so a drop of
/// the window's owner while the message runs frees the state only once the message returns.
/// `None` before [`attach`], and for `WM_NCDESTROY`, which gives up the window's count.
///
/// # Safety
///
/// `T` is the type [`attach`] stored on `window`.
pub(crate) unsafe fn for_message<T>(window: HWND, message: u32) -> Option<Rc<T>> {
    // SAFETY: GWLP_USERDATA is 0 until attach stores the pointer, and WM_NCDESTROY clears it.
    let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const T;
    if state.is_null() {
        return None;
    }
    if message == WM_NCDESTROY {
        // SAFETY: this is the window's last message; the window's count is given up once and
        // the pointer cleared, so nothing reads it again. A message still running holds its
        // own count.
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            drop(Rc::from_raw(state));
        }
        return None;
    }
    // SAFETY: a non-null pointer carries the window's count until WM_NCDESTROY, handled above.
    unsafe {
        Rc::increment_strong_count(state);
        Some(Rc::from_raw(state))
    }
}

/// The state [`attach`] stored on `window`, borrowed with no count of its own.
///
/// # Safety
///
/// `T` is the type [`attach`] stored on `window`, and the window is not destroyed while the
/// borrow lives.
pub(crate) unsafe fn attached<'window, T>(window: HWND) -> &'window T {
    // SAFETY: the caller keeps the window, and so the window's count, alive for 'window.
    unsafe { &*(GetWindowLongPtrW(window, GWLP_USERDATA) as *const T) }
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
pub(crate) fn desktop_point(lparam: LPARAM, origin: POINT) -> POINT {
    let client = client_point(lparam);
    POINT {
        x: client.x + origin.x,
        y: client.y + origin.y,
    }
}

/// Whether the pointer at `now` has moved far enough from `start` for Windows to count a
/// press as the start of a drag.
pub(crate) fn left_the_press(start: POINT, now: POINT) -> bool {
    // SAFETY: GetSystemMetrics only reads a system setting.
    let (width, height) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
    (now.x - start.x).abs() > width / 2 || (now.y - start.y).abs() > height / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_on_a_monitor_left_of_and_above_the_primary_reads_in_desktop_pixels() {
        let origin = POINT { x: -1920, y: -1080 };
        let inside = LPARAM(20 * 0x1_0000 + 10);
        assert_eq!(desktop_point(inside, origin), POINT { x: -1910, y: -1060 });
        // A captured drag past the window's top-left: client x -5, y -3.
        let past = LPARAM(0xFFFD * 0x1_0000 + 0xFFFB);
        assert_eq!(desktop_point(past, origin), POINT { x: -1925, y: -1083 });
    }
}
