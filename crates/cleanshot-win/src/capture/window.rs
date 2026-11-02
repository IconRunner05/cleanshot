use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DWMWINDOWATTRIBUTE, DwmGetWindowAttribute,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetShellWindow, GetWindowLongW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, WS_EX_LAYERED, WS_EX_TRANSPARENT,
};
use windows::core::BOOL;

use super::{MonitorImage, first_holding, holds};
use crate::Error;

/// A window a window capture can pick: its frame, DWM's extended frame bounds without the
/// shadow, and its title for a screen reader to say (F-08, F-58).
#[derive(Clone, Debug, PartialEq)]
pub struct Pickable {
    pub frame: RECT,
    pub title: String,
}

/// F-08 (D-100): the windows a window capture can pick, topmost first, as they are now:
/// visible, uncloaked, unminimised top-level windows of other processes, cut to the captured
/// monitors.
///
/// # Errors
///
/// [`Error::Capture`] if Windows cannot list its windows.
pub fn pickable_windows(captures: &[MonitorImage]) -> Result<Vec<Pickable>, Error> {
    let mut listed: Vec<HWND> = Vec::new();
    let target = LPARAM(std::ptr::from_mut(&mut listed) as isize);
    // SAFETY: the callback runs only during EnumWindows, while `listed` is borrowed here.
    unsafe { EnumWindows(Some(collect), target) }.map_err(Error::Capture)?;
    let desktop = desktop_bounds(captures);
    Ok(listed
        .into_iter()
        .filter(|&window| pickable(window))
        .filter_map(|window| {
            let frame = frame(window).and_then(|frame| cut_to(frame, desktop))?;
            Some(Pickable {
                frame,
                title: title(window),
            })
        })
        .collect())
}

/// What a window capture picks (F-08, D-100).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    /// The window at this place in the list [`pickable_windows`] gave.
    Window(usize),
    /// A monitor, picked where no window is.
    Monitor(RECT),
}

/// F-08 (D-100): what a window capture picks at `point`: the topmost of `windows`, from
/// [`pickable_windows`], holding it, or else the monitor holding it.
#[must_use]
pub fn picked(windows: &[Pickable], monitors: &[RECT], point: POINT) -> Option<Pick> {
    windows
        .iter()
        .position(|window| holds(window.frame, point))
        .map(Pick::Window)
        .or_else(|| first_holding(monitors.iter().copied(), point).map(Pick::Monitor))
}

extern "system" fn collect(window: HWND, listed: LPARAM) -> BOOL {
    // SAFETY: pickable_windows passes a pointer to its Vec, alive for the whole EnumWindows
    // call, and nothing else touches it meanwhile. EnumWindows calls this only during that
    // call, with that LPARAM; no window message carries it, so no other process can supply it.
    // ast-grep-ignore: pointer-from-handle-value
    unsafe { &mut *(listed.0 as *mut Vec<HWND>) }.push(window);
    true.into()
}

fn pickable(window: HWND) -> bool {
    let mut process = 0;
    // SAFETY: `window` comes from EnumWindows and `process` is writable.
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
    !own && visible
        && !minimised
        && !shell
        && extended & click_through != click_through
        && !cloaked(window)
        && !wallpaper(window)
}

fn cloaked(window: HWND) -> bool {
    let mut cloaked = 0_u32;
    attribute(window, DWMWA_CLOAKED, &mut cloaked).map_or(true, |()| cloaked != 0)
}

/// The window Windows draws the wallpaper in, behind the desktop's icons.
fn wallpaper(window: HWND) -> bool {
    let mut class = [0_u16; 16];
    // SAFETY: `class` is writable for its whole length.
    let length = unsafe { GetClassNameW(window, &mut class) };
    let wallpaper: Vec<u16> = "WorkerW".encode_utf16().collect();
    usize::try_from(length).is_ok_and(|length| class[..length] == *wallpaper)
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

fn desktop_bounds(captures: &[MonitorImage]) -> RECT {
    captures
        .iter()
        .map(|capture| capture.bounds)
        .reduce(|all, bounds| RECT {
            left: all.left.min(bounds.left),
            top: all.top.min(bounds.top),
            right: all.right.max(bounds.right),
            bottom: all.bottom.max(bounds.bottom),
        })
        .unwrap_or_default()
}

fn cut_to(frame: RECT, desktop: RECT) -> Option<RECT> {
    let cut = RECT {
        left: frame.left.max(desktop.left),
        top: frame.top.max(desktop.top),
        right: frame.right.min(desktop.right),
        bottom: frame.bottom.min(desktop.bottom),
    };
    (cut.left < cut.right && cut.top < cut.bottom).then_some(cut)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn f08_the_topmost_window_holding_the_pointer_is_picked_and_its_monitor_where_none_is() {
        let windows = [
            rect(10, 10, 50, 50),
            rect(0, 0, 100, 100),
            rect(200, 0, 300, 50),
        ]
        .map(|frame| Pickable {
            frame,
            title: String::new(),
        });
        let monitors = [rect(-400, 0, 0, 300), rect(0, 0, 400, 300)];
        let at = |x, y| picked(&windows, &monitors, POINT { x, y });
        assert_eq!(at(20, 20), Some(Pick::Window(0)));
        assert_eq!(at(5, 5), Some(Pick::Window(1)));
        assert_eq!(at(50, 50), Some(Pick::Window(1)));
        assert_eq!(at(150, 20), Some(Pick::Monitor(monitors[1])));
        assert_eq!(at(-1, 20), Some(Pick::Monitor(monitors[0])));
        assert_eq!(at(0, 300), None);
    }

    #[test]
    fn f08_a_frame_is_cut_to_the_desktop_and_a_frame_off_it_is_dropped() {
        let desktop = rect(-1920, 0, 1920, 1080);
        assert_eq!(
            cut_to(rect(1800, 900, 2100, 1200), desktop),
            Some(rect(1800, 900, 1920, 1080))
        );
        assert_eq!(
            cut_to(rect(-2000, -10, -1900, 10), desktop),
            Some(rect(-1920, 0, -1900, 10))
        );
        assert_eq!(cut_to(rect(1920, 0, 2000, 100), desktop), None);
    }
}
