use cleanshot_core::{Monitor, Point, Rect, RgbaImage, first_holding};
pub use windows::Win32::Foundation::RECT;
use windows::Win32::Foundation::{LPARAM, POINT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC, GetMonitorInfoW, HDC,
    HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, ReleaseDC, SRCCOPY,
    SelectObject,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::core::BOOL;

use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::Error;
use crate::struct_size;

mod duplication;
mod frame;
mod pointer;
mod window;

use duplication::duplicate_all;
pub use duplication::{
    AdapterReport, Copied, Desktop, Device, Devices, FrameReport, OutputReport, capture_desktop,
};
pub use frame::BgraFrame;
pub use pointer::draw_pointer;
pub use window::pickable_windows;

pub(crate) const DESKTOP_DPI: f32 = 96.0;

/// How the desktop is copied (CHARTER 5 runtime model: the M0 spike picks one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    GdiBitBlt,
    DesktopDuplication,
}

/// One monitor's pixels and where that monitor sits on the virtual desktop, in physical
/// pixels.
#[derive(Debug)]
pub struct MonitorImage {
    pub bounds: RECT,
    pub frame: BgraFrame,
}

/// Copies every monitor's pixels with `method`, before any window of ours is shown (F-02).
///
/// # Errors
///
/// [`Error::Capture`] if Windows refuses a step of the copy; [`Error::CaptureShape`] if a
/// monitor reports an empty size.
pub fn capture_all(method: Method) -> Result<Vec<MonitorImage>, Error> {
    match method {
        Method::GdiBitBlt => monitor_bounds()?.into_iter().map(bitblt).collect(),
        Method::DesktopDuplication => duplicate_all(&mut Devices::default())
            .results
            .into_iter()
            .collect(),
    }
}

/// F-07: the bounds of the monitor the pointer is on, in virtual-desktop pixels.
///
/// # Errors
///
/// [`Error::Capture`] if Windows cannot say where the pointer is; [`Error::NoMonitorAtPointer`]
/// if no captured monitor holds it.
pub fn monitor_under_cursor(captures: &[MonitorImage]) -> Result<RECT, Error> {
    let mut pointer = POINT::default();
    // SAFETY: `pointer` is writable.
    unsafe { GetCursorPos(&raw mut pointer) }.map_err(Error::Capture)?;
    monitor_containing(captures, pointer).ok_or(Error::NoMonitorAtPointer)
}

fn monitor_containing(captures: &[MonitorImage], point: POINT) -> Option<RECT> {
    first_holding(
        captures.iter().map(|capture| screen_rect(capture.bounds)),
        screen_point(point),
    )
    .map(win32_rect)
}

/// A Win32 rectangle as core's [`Rect`].
#[must_use]
pub fn screen_rect(rect: RECT) -> Rect {
    Rect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

pub(crate) fn win32_rect(rect: Rect) -> RECT {
    RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

pub(crate) fn screen_point(point: POINT) -> Point {
    Point {
        x: point.x,
        y: point.y,
    }
}

/// Physical pixels per DIP on the monitor holding `bounds`.
pub(crate) fn dpi_scale(bounds: RECT) -> windows::core::Result<f32> {
    monitor_scale(nearest_monitor(bounds))
}

/// D-110: the monitor holding most of `bounds`, a capture's rectangle on the virtual desktop,
/// as its work area and scale.
///
/// # Errors
///
/// [`Error::Monitor`] if Windows cannot describe that monitor.
pub fn monitor_of(bounds: RECT) -> Result<Monitor, Error> {
    let monitor = nearest_monitor(bounds);
    let mut info = MONITORINFO {
        cbSize: struct_size::<MONITORINFO>(),
        ..Default::default()
    };
    // SAFETY: `info` has its size set, as GetMonitorInfoW requires.
    if !unsafe { GetMonitorInfoW(monitor, &raw mut info) }.as_bool() {
        return Err(Error::Monitor(windows::core::Error::from_thread()));
    }
    Ok(Monitor {
        work_area: screen_rect(info.rcWork),
        scale: monitor_scale(monitor).map_err(Error::Monitor)?,
    })
}

fn nearest_monitor(bounds: RECT) -> HMONITOR {
    // SAFETY: a read of a local; MONITOR_DEFAULTTONEAREST always returns a monitor.
    unsafe { MonitorFromRect(&raw const bounds, MONITOR_DEFAULTTONEAREST) }
}

#[expect(clippy::cast_precision_loss, reason = "a DPI is a few hundred")]
fn monitor_scale(monitor: HMONITOR) -> windows::core::Result<f32> {
    let (mut x, mut y) = (0, 0);
    // SAFETY: both outputs are writable locals.
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut x, &raw mut y) }?;
    Ok(x as f32 / DESKTOP_DPI)
}

/// The share of `frame`'s pixels whose red, green and blue are all 0.
#[expect(
    clippy::cast_precision_loss,
    reason = "a share for a log line, where a few lost bits do not matter"
)]
#[must_use]
pub fn black_share(frame: &BgraFrame) -> f64 {
    let pixels = frame.pixels().as_chunks::<4>().0;
    let black = pixels
        .iter()
        .filter(|pixel| pixel[..3] == [0, 0, 0])
        .count();
    black as f64 / pixels.len().max(1) as f64
}

/// The pixels of `selected`, a rectangle on the virtual desktop, cut from `captures` as
/// opaque RGBA. A part no monitor covers stays transparent.
///
/// # Errors
///
/// [`Error::CaptureShape`] if `selected` is empty.
pub fn region(captures: &[MonitorImage], selected: RECT) -> Result<RgbaImage, Error> {
    let (width, height) = size_of_rect(selected)?;
    let row_bytes = width as usize * 4;
    let mut pixels = vec![0; row_bytes * height as usize];
    for capture in captures {
        let bounds = capture.bounds;
        let Some(overlap) = screen_rect(selected).overlap(screen_rect(bounds)) else {
            continue;
        };
        let source_row_bytes = capture.frame.width() as usize * 4;
        let span = overlap.right.abs_diff(overlap.left) as usize * 4;
        for y in overlap.top..overlap.bottom {
            let source = offset(y - bounds.top, overlap.left - bounds.left, source_row_bytes);
            let target = offset(y - selected.top, overlap.left - selected.left, row_bytes);
            frame::to_rgba(
                &capture.frame.pixels()[source..source + span],
                &mut pixels[target..target + span],
            );
        }
    }
    Ok(RgbaImage::new(width, height, pixels)?)
}

fn offset(row: i32, column: i32, row_bytes: usize) -> usize {
    let row = usize::try_from(row).expect("the row lies inside the overlap");
    let column = usize::try_from(column).expect("the column lies inside the overlap");
    row * row_bytes + column * 4
}

pub(crate) fn monitor_handles() -> Result<Vec<HMONITOR>, Error> {
    unsafe extern "system" fn add(monitor: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> BOOL {
        // SAFETY: `list` is the &mut Vec that monitor_handles passed, alive for the whole call.
        // EnumDisplayMonitors calls this only during our call, with our own LPARAM; no window
        // message carries it, so no other process can supply it.
        // ast-grep-ignore: pointer-from-handle-value
        let list = unsafe { &mut *(list.0 as *mut Vec<HMONITOR>) };
        list.push(monitor);
        true.into()
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: `add` only pushes onto `handles`, which outlives the call.
    let listed =
        unsafe { EnumDisplayMonitors(None, None, Some(add), LPARAM(&raw mut handles as isize)) };
    if !listed.as_bool() {
        return Err(Error::Capture(windows::core::Error::from_thread()));
    }
    Ok(handles)
}

fn monitor_bounds() -> Result<Vec<RECT>, Error> {
    monitor_handles()?
        .into_iter()
        .map(|handle| {
            let mut info = MONITORINFO {
                cbSize: struct_size::<MONITORINFO>(),
                ..Default::default()
            };
            // SAFETY: `info` has its size set, as GetMonitorInfoW requires.
            if !unsafe { GetMonitorInfoW(handle, &raw mut info) }.as_bool() {
                return Err(Error::Capture(windows::core::Error::from_thread()));
            }
            Ok(info.rcMonitor)
        })
        .collect()
}

fn size_of_rect(bounds: RECT) -> Result<(u32, u32), Error> {
    let width = u32::try_from(bounds.right - bounds.left).unwrap_or(0);
    let height = u32::try_from(bounds.bottom - bounds.top).unwrap_or(0);
    if width == 0 || height == 0 {
        return Err(Error::CaptureShape(cleanshot_core::ImageError::ZeroSize {
            width,
            height,
        }));
    }
    Ok((width, height))
}

fn bitblt(bounds: RECT) -> Result<MonitorImage, Error> {
    let (width, height) = size_of_rect(bounds)?;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: struct_size::<BITMAPINFOHEADER>(),
            biWidth: bounds.right - bounds.left,
            biHeight: -(bounds.bottom - bounds.top),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let length = width as usize * height as usize * 4;
    // SAFETY: every GDI object is created, used and released here on one thread. The DIB is
    // top-down 32-bit, so `bits` points at width * height * 4 bytes while the bitmap lives,
    // and is copied out before it is deleted.
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let mut bits = std::ptr::null_mut();
        let copied = CreateDIBSection(
            Some(memory),
            &raw const info,
            DIB_RGB_COLORS,
            &raw mut bits,
            None,
            0,
        )
        .and_then(|bitmap| {
            let previous = SelectObject(memory, bitmap.into());
            let blitted = BitBlt(
                memory,
                0,
                0,
                info.bmiHeader.biWidth,
                -info.bmiHeader.biHeight,
                Some(screen),
                bounds.left,
                bounds.top,
                SRCCOPY | CAPTUREBLT,
            )
            .map(|()| std::slice::from_raw_parts(bits.cast::<u8>(), length).to_vec());
            SelectObject(memory, previous);
            let deleted = DeleteObject(bitmap.into());
            debug_assert!(deleted.as_bool(), "a bitmap made here is deleted once");
            blitted
        });
        let deleted = DeleteDC(memory);
        debug_assert!(deleted.as_bool(), "a memory DC made here is deleted once");
        ReleaseDC(None, screen);
        let pixels = copied.map_err(Error::Capture)?;
        Ok(MonitorImage {
            bounds,
            frame: BgraFrame::new(width, height, pixels)
                .expect("the DIB holds width x height pixels"),
        })
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    pub(super) fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    fn grey(value: u8) -> [u8; 4] {
        [value, value, value, u8::MAX]
    }

    /// A monitor whose pixel n, counted row by row, is the grey `first + n`, so a pixel taken
    /// from the wrong row or column has the wrong value.
    fn numbered(bounds: RECT, first: u8) -> MonitorImage {
        let (width, height) = size_of_rect(bounds).expect("a test monitor has an area");
        let pixels = (first..)
            .take(width as usize * height as usize)
            .flat_map(grey)
            .collect();
        MonitorImage {
            bounds,
            frame: BgraFrame::new(width, height, pixels).expect("sized to match"),
        }
    }

    pub(super) fn filled(bounds: RECT, value: u8) -> MonitorImage {
        let (width, height) = size_of_rect(bounds).expect("a test monitor has an area");
        let pixels = grey(value).repeat(width as usize * height as usize);
        MonitorImage {
            bounds,
            frame: BgraFrame::new(width, height, pixels).expect("sized to match"),
        }
    }

    #[test]
    fn f01_a_region_across_a_negative_origin_monitor_takes_each_side_from_its_own_monitor() {
        let captures = [
            numbered(rect(-4, 0, 0, 2), 10),
            numbered(rect(0, 0, 4, 2), 30),
        ];
        let image = region(&captures, rect(-2, 1, 2, 2)).expect("the region has an area");
        assert_eq!((image.width(), image.height()), (4, 1));
        assert_eq!(
            image.pixels(),
            [grey(16), grey(17), grey(34), grey(35)].concat()
        );
    }

    /// A 3840x2160 monitor at 200% left of a 1920x1080 one at 100%, as a per-monitor-aware
    /// process sees them: both in physical pixels, so neither side is scaled to the other.
    #[test]
    fn f01_a_region_across_a_monitor_above_the_primary_takes_each_side_from_its_own_monitor() {
        let captures = [
            numbered(rect(0, -2, 4, 0), 10),
            numbered(rect(0, 0, 4, 2), 30),
        ];
        let image = region(&captures, rect(1, -1, 3, 1)).expect("the region has an area");
        assert_eq!((image.width(), image.height()), (2, 2));
        assert_eq!(
            image.pixels(),
            [grey(15), grey(16), grey(31), grey(32)].concat()
        );
        assert_eq!(
            monitor_containing(&captures, POINT { x: 0, y: -1 }),
            Some(rect(0, -2, 4, 0))
        );
    }

    #[test]
    fn d71_a_region_across_a_200_percent_monitor_and_a_100_percent_one_keeps_physical_pixels() {
        let shade =
            |x: i32, y: i32| grey(u8::try_from((x * 3 + y * 7).rem_euclid(251)).expect("< 251"));
        let painted = |bounds: RECT| {
            let pixels = (bounds.top..bounds.bottom)
                .flat_map(|y| (bounds.left..bounds.right).flat_map(move |x| shade(x, y)))
                .collect();
            let (width, height) = size_of_rect(bounds).expect("a test monitor has an area");
            MonitorImage {
                bounds,
                frame: BgraFrame::new(width, height, pixels).expect("sized to match"),
            }
        };
        let captures = [
            painted(rect(-3840, 0, 0, 2160)),
            painted(rect(0, 0, 1920, 1080)),
        ];
        let selected = rect(-2, 1078, 2, 1082);

        let image = region(&captures, selected).expect("the region has an area");

        let expected: Vec<u8> = (selected.top..selected.bottom)
            .flat_map(|y| {
                (selected.left..selected.right).flat_map(move |x| {
                    if x >= 0 && y >= 1080 {
                        [0; 4]
                    } else {
                        shade(x, y)
                    }
                })
            })
            .collect();
        assert_eq!((image.width(), image.height()), (4, 4));
        assert_eq!(image.pixels(), expected);
    }

    #[test]
    fn p01_a_region_is_cut_from_the_frozen_bgra_frames_as_opaque_rgba() {
        let blue_then_red = MonitorImage {
            bounds: rect(0, 0, 2, 1),
            frame: BgraFrame::new(2, 1, vec![255, 0, 0, 0, 0, 0, 255, 7]).expect("2x1 BGRA"),
        };
        let image = region(&[blue_then_red], rect(0, 0, 2, 1)).expect("the region has an area");
        assert_eq!(image.pixels(), [0, 0, 255, 255, 255, 0, 0, 255]);
    }

    #[test]
    fn f01_a_region_no_monitor_covers_stays_transparent() {
        let captures = [filled(rect(0, 0, 2, 2), 30)];
        let image = region(&captures, rect(1, 1, 3, 2)).expect("the region has an area");
        assert_eq!(image.pixels(), [grey(30), [0; 4]].concat());
    }

    #[test]
    fn f02_black_share_counts_pixels_with_no_red_green_or_blue() {
        let mut pixels = [[0, 0, 0, 255], [0, 0, 1, 255], [7, 0, 0, 255], [0, 0, 0, 0]].concat();
        let frame = BgraFrame::new(2, 2, pixels.clone()).expect("2x2 BGRA");
        assert!((black_share(&frame) - 0.5).abs() < f64::EPSILON);
        pixels.fill(9);
        let frame = BgraFrame::new(2, 2, pixels).expect("2x2 BGRA");
        assert!(black_share(&frame).abs() < f64::EPSILON);
    }

    #[test]
    fn d110_a_capture_names_the_work_area_of_the_monitor_holding_it() {
        for bounds in monitor_bounds().expect("the monitors are listed") {
            let inside = rect(
                bounds.left + 1,
                bounds.top + 1,
                bounds.left + 2,
                bounds.top + 2,
            );
            let monitor = monitor_of(inside).expect("the monitor is described");
            let work = monitor.work_area;
            assert!(
                work.right > work.left && work.bottom > work.top,
                "{monitor:?}"
            );
            assert_eq!(
                screen_rect(bounds).overlap(work),
                Some(work),
                "the work area lies on {bounds:?}"
            );
            assert!(monitor.scale > 0.0, "{monitor:?}");
        }
    }

    #[test]
    fn f07_the_monitor_holding_the_pointer_is_chosen_across_a_negative_origin() {
        let captures = [filled(rect(-4, 0, 0, 2), 1), filled(rect(0, 0, 4, 2), 2)];
        assert_eq!(
            monitor_containing(&captures, POINT { x: -1, y: 1 }),
            Some(rect(-4, 0, 0, 2))
        );
        assert_eq!(
            monitor_containing(&captures, POINT { x: 0, y: 0 }),
            Some(rect(0, 0, 4, 2))
        );
        assert_eq!(monitor_containing(&captures, POINT { x: 4, y: 0 }), None);
    }
}
