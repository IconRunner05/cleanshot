use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::Graphics::Gdi::{RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, LWA_ALPHA, SPI_GETCLIENTAREAANIMATION,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetLayeredWindowAttributes, SetWindowLongPtrW,
    SystemParametersInfoW, WS_EX_LAYERED,
};
use windows::core::BOOL;

const FADE: Duration = Duration::from_millis(120);
const FRAME: Duration = Duration::from_millis(8);

/// Whether Windows' Animation effects setting is on.
///
/// # Errors
///
/// The error `SystemParametersInfoW` reports.
pub fn animations_on() -> windows::core::Result<bool> {
    let mut on = BOOL::default();
    // SAFETY: `on` is a writable BOOL, the size this action writes.
    unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut on).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }?;
    Ok(on.as_bool())
}

/// The opacity `elapsed` into a fade out, or `None` once it has finished.
fn opacity(elapsed: Duration) -> Option<u8> {
    let left = FADE.checked_sub(elapsed).filter(|left| !left.is_zero())?;
    Some(u8::try_from(left.as_micros() * 255 / FADE.as_micros()).expect("left <= FADE"))
}

/// Fades `windows` out together over 120 ms and returns once they are clear, still
/// open for the caller to destroy.
pub(crate) fn fade_out(windows: &[HWND]) -> windows::core::Result<()> {
    for &window in windows {
        // SAFETY: `window` belongs to this thread. A failed style change shows as the
        // SetLayeredWindowAttributes error below, which needs WS_EX_LAYERED.
        unsafe {
            let style = GetWindowLongPtrW(window, GWL_EXSTYLE);
            SetWindowLongPtrW(
                window,
                GWL_EXSTYLE,
                style | isize::try_from(WS_EX_LAYERED.0).expect("0x80000 fits"),
            );
            SetLayeredWindowAttributes(window, COLORREF(0), u8::MAX, LWA_ALPHA)?;
            RedrawWindow(Some(window), None, None, RDW_INVALIDATE | RDW_UPDATENOW).ok()?;
        }
    }
    let start = Instant::now();
    while let Some(alpha) = opacity(start.elapsed()) {
        for &window in windows {
            // SAFETY: `window` was made layered above.
            unsafe { SetLayeredWindowAttributes(window, COLORREF(0), alpha, LWA_ALPHA) }?;
        }
        std::thread::sleep(FRAME);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fade_out_runs_from_opaque_to_clear_in_120_ms() {
        let at = |millis| opacity(Duration::from_millis(millis));
        assert_eq!(at(0), Some(255));
        assert_eq!(at(60), Some(127));
        assert!(at(119).is_some_and(|alpha| alpha < 3), "{:?}", at(119));
        assert_eq!(at(120), None);
        assert_eq!(at(500), None);
    }
}
