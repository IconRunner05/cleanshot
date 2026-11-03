//! F-01's DPI matrix on the CI runner (D-71): set the display's mode and scale, show a known
//! pattern, and read back the screen Cleanshot copied.

use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, CDS_TYPE, ChangeDisplaySettingsW, DEVMODEW,
    DIB_RGB_COLORS, DISP_CHANGE_SUCCESSFUL, DM_PELSHEIGHT, DM_PELSWIDTH, EndPaint,
    MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint, PAINTSTRUCT, SetDIBitsToDevice, UpdateWindow,
};
use windows::Win32::System::Ole::CF_DIBV5;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, SPI_SETLOGICALDPIOVERRIDE, SPIF_SENDCHANGE,
    SPIF_UPDATEINIFILE, SW_SHOWNOACTIVATE, ShowWindow, SystemParametersInfoW, WM_PAINT,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::{FULL_SCREEN, press, pump_messages, within};
use crate::Error;
use crate::struct_size;

const PATTERN_CLASS: PCWSTR = w!("CleanshotFieldTestPattern");
/// Where the pattern sits on the primary monitor, in physical pixels.
const PATTERN_LEFT: i32 = 200;
const PATTERN_TOP: i32 = 200;
const PATTERN_SIDE: i32 = 256;
/// How long the display may take to reach a scale, and cleanshot.exe to copy the screen. P-02 is
/// judged from the logged copy time, never from this wait.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Why the screen copy at one scale did not hold the pattern pixel for pixel.
#[derive(Debug, thiserror::Error)]
pub enum DpiRunError {
    #[error("this thread could not be made per-monitor DPI aware")]
    Awareness,
    #[error("Windows refused the display mode (DISP_CHANGE {0})")]
    DisplayRefused(i32),
    #[error("Windows refused the scale step")]
    ScaleRefused(#[source] windows::core::Error),
    #[error("the primary monitor is at {found:?} dpi, not {expected}")]
    ScaleNotReached { expected: u32, found: Option<u32> },
    #[error("the pattern window could not be shown")]
    Pattern(#[source] Error),
    #[error("Ctrl+Shift+7 was refused by SendInput")]
    KeysRefused,
    #[error("cleanshot.exe logged no copy (P-02) within {0:?}")]
    NothingCopied(Duration),
    #[error("the clipboard could not be emptied or read")]
    ClipboardHeld(#[source] windows::core::Error),
    #[error("the clipboard's bitmap could not be read: {0}")]
    Unreadable(&'static str),
    #[error("the copy is {found_width}x{found_height}, the monitor {width}x{height}")]
    Size {
        width: u32,
        height: u32,
        found_width: u32,
        found_height: u32,
    },
    #[error("pattern pixel ({x}, {y}) is {found:?} in the copy, not {expected:?}")]
    Pixel {
        x: u32,
        y: u32,
        expected: [u8; 3],
        found: [u8; 3],
    },
}

/// Sets the primary display to `width` x `height` and `step` scale notches from the one
/// Windows recommends, and waits for the monitor to read `dpi`. At 1920x1080 on the runner,
/// steps 0, 1, 2 and 4 are 96, 120, 144 and 168 dpi (D-71).
///
/// # Errors
///
/// The first [`DpiRunError`] met.
pub fn set_scale(width: u32, height: u32, step: i32, dpi: u32) -> Result<(), DpiRunError> {
    physical_pixels()?;
    set_display(width, height)?;
    // SAFETY: the action takes its value in the second argument and reads no pointer.
    unsafe {
        SystemParametersInfoW(
            SPI_SETLOGICALDPIOVERRIDE,
            step.cast_unsigned(),
            None,
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )
    }
    .map_err(DpiRunError::ScaleRefused)?;
    let mut found = None;
    if !within(TIMEOUT, || {
        found = primary_dpi();
        found == Some(dpi)
    }) {
        return Err(DpiRunError::ScaleNotReached {
            expected: dpi,
            found,
        });
    }
    Ok(())
}

/// F-01 at the scale [`set_scale`] left: checks the monitor still reads `dpi`, shows a
/// pattern, and presses Ctrl+Shift+7 for the running cleanshot.exe to copy the full screen.
/// `copies` counts the copies cleanshot.exe has logged. The copy must be `width` x `height`
/// with the pattern in it pixel for pixel.
///
/// # Errors
///
/// The first [`DpiRunError`] met.
pub fn copy_at_scale(
    width: u32,
    height: u32,
    dpi: u32,
    copies: impl FnMut() -> u32,
) -> Result<(), DpiRunError> {
    physical_pixels()?;
    let found = primary_dpi();
    if found != Some(dpi) {
        return Err(DpiRunError::ScaleNotReached {
            expected: dpi,
            found,
        });
    }
    let _pattern = PatternWindow::show().map_err(DpiRunError::Pattern)?;
    check_copy(&copy_screen(copies)?, width, height)
}

/// Gives this thread the awareness cleanshot.exe's manifest gives it, so the pattern window
/// and the monitor's DPI are physical, whatever this exe's own manifest says.
fn physical_pixels() -> Result<(), DpiRunError> {
    // SAFETY: changes only this thread's DPI awareness, to a context constant.
    if unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
        .is_invalid()
    {
        return Err(DpiRunError::Awareness);
    }
    Ok(())
}

fn set_display(width: u32, height: u32) -> Result<(), DpiRunError> {
    let mode = DEVMODEW {
        dmSize: u16::try_from(size_of::<DEVMODEW>()).expect("fits in u16"),
        dmPelsWidth: width,
        dmPelsHeight: height,
        dmFields: DM_PELSWIDTH | DM_PELSHEIGHT,
        ..Default::default()
    };
    // SAFETY: `mode` is a complete DEVMODEW that outlives the call.
    let changed = unsafe { ChangeDisplaySettingsW(Some(&raw const mode), CDS_TYPE(0)) };
    if changed == DISP_CHANGE_SUCCESSFUL {
        Ok(())
    } else {
        Err(DpiRunError::DisplayRefused(changed.0))
    }
}

fn primary_dpi() -> Option<u32> {
    let origin = POINT { x: 0, y: 0 };
    // SAFETY: a lookup by point; the primary monitor holds the origin.
    let monitor = unsafe { MonitorFromPoint(origin, MONITOR_DEFAULTTOPRIMARY) };
    let (mut dpi_x, mut dpi_y) = (0, 0);
    // SAFETY: `monitor` came from MonitorFromPoint, and both outputs are writable.
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut dpi_x, &raw mut dpi_y) }
        .ok()
        .map(|()| dpi_x)
}

/// The pattern's colour at (`x`, `y`): a 2 x 2 tile of red, green, blue and white, so any
/// resampling of the copy mixes neighbours and shows.
fn pattern_rgb(x: u32, y: u32) -> [u8; 3] {
    match (x.is_multiple_of(2), y.is_multiple_of(2)) {
        (true, true) => [255, 0, 0],
        (false, true) => [0, 255, 0],
        (true, false) => [0, 0, 255],
        (false, false) => [255, 255, 255],
    }
}

/// A topmost window showing the pattern at [`PATTERN_LEFT`], [`PATTERN_TOP`], until dropped.
struct PatternWindow(HWND);

impl PatternWindow {
    /// Shows the pattern and gives the compositor time to put it on screen.
    fn show() -> Result<Self, Error> {
        crate::register_window_class(PATTERN_CLASS, paint_pattern)?;
        // SAFETY: the class is registered and every string is 'static.
        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                PATTERN_CLASS,
                w!("Cleanshot field test pattern"),
                WS_POPUP,
                PATTERN_LEFT,
                PATTERN_TOP,
                PATTERN_SIDE,
                PATTERN_SIDE,
                None,
                None,
                None,
                None,
            )
        }
        .map_err(Error::Window)?;
        // SAFETY: the window was created on this thread just above.
        unsafe {
            // ShowWindow returns whether the window was visible; a window that does not
            // paint fails the copy it is measured by.
            let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
            let _ = UpdateWindow(window);
        }
        pump_messages(Duration::from_millis(500));
        Ok(Self(window))
    }
}

impl Drop for PatternWindow {
    fn drop(&mut self) {
        // SAFETY: the window belongs to this thread and is destroyed once, here.
        let destroyed = unsafe { DestroyWindow(self.0) };
        debug_assert!(destroyed.is_ok(), "the pattern window is destroyed once");
    }
}

extern "system" fn paint_pattern(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message != WM_PAINT {
        // SAFETY: every other message gets the default handling.
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    let side = PATTERN_SIDE.cast_unsigned();
    let bgra: Vec<u8> = (0..side)
        .flat_map(|y| (0..side).map(move |x| pattern_rgb(x, y)))
        .flat_map(|[red, green, blue]| [blue, green, red, 0])
        .collect();
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: struct_size::<BITMAPINFOHEADER>(),
            biWidth: PATTERN_SIDE,
            biHeight: -PATTERN_SIDE,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: painting this thread's own window between BeginPaint and EndPaint; `bgra` holds
    // PATTERN_SIDE top-down rows of 32-bit pixels, as `info` says.
    unsafe {
        let device = BeginPaint(window, &raw mut paint);
        SetDIBitsToDevice(
            device,
            0,
            0,
            side,
            side,
            0,
            0,
            0,
            side,
            bgra.as_ptr().cast(),
            &raw const info,
            DIB_RGB_COLORS,
        );
        // EndPaint always returns nonzero.
        let _ = EndPaint(window, &raw const paint);
    }
    LRESULT(0)
}

/// A bitmap read from the clipboard, as RGB rows top-down.
struct CopiedImage {
    width: u32,
    height: u32,
    rgb: Vec<[u8; 3]>,
}

impl CopiedImage {
    fn at(&self, x: u32, y: u32) -> [u8; 3] {
        self.rgb[(y * self.width + x) as usize]
    }
}

/// Empties the clipboard, so no earlier copy can pass, presses Ctrl+Shift+7, the full-screen
/// hotkey the dpi job gives cleanshot.exe with `after_capture = "copy"` (F-07), and reads the
/// bitmap it puts on the clipboard.
fn copy_screen(mut copies: impl FnMut() -> u32) -> Result<CopiedImage, DpiRunError> {
    crate::clipboard::replace_with(&[]).map_err(DpiRunError::ClipboardHeld)?;
    let before = copies();
    if press(&FULL_SCREEN) != 6 {
        return Err(DpiRunError::KeysRefused);
    }
    // The clipboard is read only once cleanshot.exe has logged the copy, so this process never
    // holds it open while cleanshot.exe writes it.
    if !within(TIMEOUT, || copies() > before) {
        return Err(DpiRunError::NothingCopied(TIMEOUT));
    }
    parse_dib(&crate::clipboard::read(u32::from(CF_DIBV5.0)).map_err(DpiRunError::ClipboardHeld)?)
}

fn parse_dib(bytes: &[u8]) -> Result<CopiedImage, DpiRunError> {
    let field = |offset: usize| -> Result<[u8; 4], DpiRunError> {
        bytes
            .get(offset..offset + 4)
            .and_then(|field| field.try_into().ok())
            .ok_or(DpiRunError::Unreadable("the header is cut short"))
    };
    let header = u32::from_le_bytes(field(0)?) as usize;
    let width = i32::from_le_bytes(field(4)?);
    let height = i32::from_le_bytes(field(8)?);
    let [_, _, bits, _] = field(12)?;
    if bits != 32 {
        return Err(DpiRunError::Unreadable("not 32 bits per pixel"));
    }
    let Ok(width) = u32::try_from(width) else {
        return Err(DpiRunError::Unreadable("a negative width"));
    };
    let rows = height.unsigned_abs();
    let pixels = bytes
        .get(header..header + width as usize * rows as usize * 4)
        .ok_or(DpiRunError::Unreadable("fewer pixels than the header says"))?;
    let row = |y: u32| if height > 0 { rows - 1 - y } else { y };
    let rgb = (0..rows)
        .flat_map(|y| (0..width).map(move |x| (row(y) * width + x) as usize * 4))
        .map(|at| [pixels[at + 2], pixels[at + 1], pixels[at]])
        .collect();
    Ok(CopiedImage {
        width,
        height: rows,
        rgb,
    })
}

fn check_copy(copy: &CopiedImage, width: u32, height: u32) -> Result<(), DpiRunError> {
    if (copy.width, copy.height) != (width, height) {
        return Err(DpiRunError::Size {
            width,
            height,
            found_width: copy.width,
            found_height: copy.height,
        });
    }
    let (left, top, side) = (
        PATTERN_LEFT.cast_unsigned(),
        PATTERN_TOP.cast_unsigned(),
        PATTERN_SIDE.cast_unsigned(),
    );
    for y in 0..side {
        for x in 0..side {
            let (expected, found) = (pattern_rgb(x, y), copy.at(left + x, top + y));
            if found != expected {
                return Err(DpiRunError::Pixel {
                    x,
                    y,
                    expected,
                    found,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f01_a_bottom_up_clipboard_bitmap_reads_top_row_first() {
        let mut bytes = vec![0; 124];
        bytes[0..4].copy_from_slice(&124_u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&2_i32.to_le_bytes());
        bytes[8..12].copy_from_slice(&2_i32.to_le_bytes());
        bytes[14] = 32;
        let bottom = [[0, 0, 255, 255], [255, 255, 255, 255]];
        let top = [[0, 0, 255, 255], [0, 255, 0, 255]];
        bytes.extend(bottom.into_iter().chain(top).flatten());

        let copy = parse_dib(&bytes).unwrap();

        assert_eq!((copy.width, copy.height), (2, 2));
        assert_eq!(copy.at(1, 0), [0, 255, 0]);
        assert_eq!(copy.at(1, 1), [255, 255, 255]);
    }
}
