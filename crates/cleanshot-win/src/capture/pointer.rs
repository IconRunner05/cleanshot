use std::ops::Range;

use cleanshot_core::RgbaImage;
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetObjectW, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetIconInfo, HCURSOR, HICON,
    ICONINFO,
};

use super::{MonitorImage, holds};
use crate::{Error, struct_size};

/// F-12 Show pointer: draws the pointer, in its shape at this moment, into the image of the
/// monitor it is on. Draws nothing while the pointer is hidden.
///
/// # Errors
///
/// [`Error::Capture`] if Windows cannot say where the pointer is or what it looks like.
pub fn draw_pointer(monitors: &mut [MonitorImage]) -> Result<(), Error> {
    let mut cursor = CURSORINFO {
        cbSize: struct_size::<CURSORINFO>(),
        ..Default::default()
    };
    // SAFETY: `cursor` is writable and its size field is set.
    unsafe { GetCursorInfo(&raw mut cursor) }.map_err(Error::Capture)?;
    if cursor.flags.0 & CURSOR_SHOWING.0 == 0 {
        return Ok(());
    }
    let Some(monitor) = monitors
        .iter_mut()
        .find(|monitor| holds(monitor.bounds, cursor.ptScreenPos))
    else {
        return Ok(());
    };
    draw_cursor(monitor, cursor.hCursor, cursor.ptScreenPos)
}

/// A cursor's size in pixels and its hotspot, the point that sits on the pointer's position.
struct Shape {
    width: i32,
    height: i32,
    hotspot: POINT,
}

impl Shape {
    fn of(cursor: HCURSOR) -> Result<Self, Error> {
        let mut icon = ICONINFO::default();
        // SAFETY: `icon` is writable; the bitmaps it receives are deleted below.
        unsafe { GetIconInfo(HICON(cursor.0), &raw mut icon) }.map_err(Error::Capture)?;
        let mut mask = BITMAP::default();
        // SAFETY: `mask` is a writable BITMAP of the size given, and the handle is the icon's.
        let filled = unsafe {
            GetObjectW(
                icon.hbmMask.into(),
                i32::try_from(size_of::<BITMAP>()).expect("a BITMAP is a few dozen bytes"),
                Some((&raw mut mask).cast()),
            )
        };
        let monochrome = icon.hbmColor.is_invalid();
        // SAFETY: GetIconInfo made these bitmaps for this call, and nothing else holds them.
        unsafe {
            let deleted = DeleteObject(icon.hbmMask.into());
            debug_assert!(deleted.as_bool(), "the icon's mask is deleted once");
            if !monochrome {
                let deleted = DeleteObject(icon.hbmColor.into());
                debug_assert!(
                    deleted.as_bool(),
                    "the icon's colour bitmap is deleted once"
                );
            }
        }
        if filled == 0 {
            return Err(Error::Capture(windows::core::Error::from_thread()));
        }
        let hotspot =
            |offset: u32| i32::try_from(offset).expect("a hotspot lies inside its cursor");
        Ok(Self {
            width: mask.bmWidth,
            // A monochrome cursor's mask holds the AND mask above the XOR mask.
            height: if monochrome {
                mask.bmHeight / 2
            } else {
                mask.bmHeight
            },
            hotspot: POINT {
                x: hotspot(icon.xHotspot),
                y: hotspot(icon.yHotspot),
            },
        })
    }
}

/// Draws `cursor` with its hotspot at `at`, a virtual-desktop point, over `monitor`'s image.
fn draw_cursor(monitor: &mut MonitorImage, cursor: HCURSOR, at: POINT) -> Result<(), Error> {
    let shape = Shape::of(cursor)?;
    let left = at.x - monitor.bounds.left - shape.hotspot.x;
    let top = at.y - monitor.bounds.top - shape.hotspot.y;
    let image_width = monitor.bounds.right - monitor.bounds.left;
    let image_height = monitor.bounds.bottom - monitor.bounds.top;
    let columns = left.max(0)..(left + shape.width).min(image_width);
    let rows = top.max(0)..(top + shape.height).min(image_height);
    if columns.is_empty() || rows.is_empty() {
        return Ok(());
    }
    let mut pixels = monitor.image.pixels().to_vec();
    let in_image = |x: i32, y: i32| index(y * image_width + x);
    let in_cursor = |x: i32, y: i32| index((y - top) * shape.width + (x - left));
    let mut patch = vec![0; index(shape.width * shape.height)];
    for (x, y) in each(&columns, &rows) {
        let (from, to) = (in_image(x, y), in_cursor(x, y));
        patch[to..to + 4].copy_from_slice(&swap_red_blue(&pixels[from..from + 4]));
    }
    paint(cursor, &shape, &mut patch)?;
    for (x, y) in each(&columns, &rows) {
        let (to, from) = (in_image(x, y), in_cursor(x, y));
        let [red, green, blue, _] = swap_red_blue(&patch[from..from + 4]);
        // A capture of the screen is opaque.
        pixels[to..to + 4].copy_from_slice(&[red, green, blue, u8::MAX]);
    }
    monitor.image = RgbaImage::new(monitor.image.width(), monitor.image.height(), pixels)?;
    Ok(())
}

/// Draws `cursor` at the top left of `patch`, a top-down BGRA image of the cursor's size,
/// over what `patch` already holds.
fn paint(cursor: HCURSOR, shape: &Shape, patch: &mut [u8]) -> Result<(), Error> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: struct_size::<BITMAPINFOHEADER>(),
            biWidth: shape.width,
            biHeight: -shape.height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: every GDI object is created, used and released here on one thread. The DIB is
    // top-down 32-bit of the cursor's size, so `bits` points at patch.len() bytes while the
    // bitmap lives, and is copied in before the draw and out after it.
    let painted = unsafe {
        let memory = CreateCompatibleDC(None);
        let mut bits = std::ptr::null_mut();
        let painted = CreateDIBSection(
            Some(memory),
            &raw const info,
            DIB_RGB_COLORS,
            &raw mut bits,
            None,
            0,
        )
        .and_then(|bitmap| {
            let previous = SelectObject(memory, bitmap.into());
            let canvas = std::slice::from_raw_parts_mut(bits.cast::<u8>(), patch.len());
            canvas.copy_from_slice(patch);
            let drawn = DrawIconEx(
                memory,
                0,
                0,
                HICON(cursor.0),
                shape.width,
                shape.height,
                0,
                None,
                DI_NORMAL,
            )
            .map(|()| patch.copy_from_slice(canvas));
            SelectObject(memory, previous);
            let deleted = DeleteObject(bitmap.into());
            debug_assert!(deleted.as_bool(), "a bitmap made here is deleted once");
            drawn
        });
        let deleted = DeleteDC(memory);
        debug_assert!(deleted.as_bool(), "a memory DC made here is deleted once");
        painted
    };
    painted.map_err(Error::Capture)
}

fn each(columns: &Range<i32>, rows: &Range<i32>) -> impl Iterator<Item = (i32, i32)> {
    rows.clone()
        .flat_map(move |y| columns.clone().map(move |x| (x, y)))
}

fn swap_red_blue(pixel: &[u8]) -> [u8; 4] {
    [pixel[2], pixel[1], pixel[0], pixel[3]]
}

#[expect(
    clippy::cast_sign_loss,
    reason = "a pixel's offset, clipped to its image, is never negative"
)]
fn index(pixel: i32) -> usize {
    pixel as usize * 4
}
#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{IDC_ARROW, LoadCursorW};

    use super::*;

    const GREY: [u8; 4] = [128, 128, 128, 255];

    fn grey_monitor() -> MonitorImage {
        let (width, height) = (200, 100);
        MonitorImage {
            bounds: RECT {
                left: 1_000,
                top: 500,
                right: 1_000 + width,
                bottom: 500 + height,
            },
            image: RgbaImage::new(200, 100, GREY.repeat(200 * 100)).expect("200x100 RGBA"),
        }
    }

    fn pixel(monitor: &MonitorImage, x: i32, y: i32) -> [u8; 4] {
        let at = index(y * 200 + x);
        monitor.image.pixels()[at..at + 4]
            .try_into()
            .expect("four bytes a pixel")
    }

    #[test]
    fn f12_the_pointer_is_drawn_with_its_hotspot_on_the_point_it_was_at() {
        // SAFETY: loads a shared system cursor, which is never destroyed.
        let arrow = unsafe { LoadCursorW(None, IDC_ARROW) }.expect("the arrow cursor loads");
        let mut monitor = grey_monitor();
        // Monitor pixel (50, 40), in virtual-desktop pixels; the arrow's hotspot is its tip.
        draw_cursor(&mut monitor, arrow, POINT { x: 1_050, y: 540 }).expect("the arrow draws");
        let changed = |columns: std::ops::Range<i32>, rows: std::ops::Range<i32>| {
            rows.flat_map(|y| columns.clone().map(move |x| (x, y)))
                .filter(|&(x, y)| pixel(&monitor, x, y) != GREY)
                .count()
        };
        assert!(
            changed(50..60, 40..56) > 20,
            "the arrow is drawn below and right of its tip"
        );
        assert_eq!(
            changed(0..50, 0..100) + changed(50..200, 0..40),
            0,
            "nothing above or left of the tip"
        );
        assert!(
            monitor
                .image
                .pixels()
                .chunks(4)
                .all(|each| each[3] == u8::MAX),
            "every pixel stays opaque"
        );
    }

    #[test]
    fn f12_a_pointer_at_the_monitor_edge_is_cut_off_there() {
        // SAFETY: loads a shared system cursor, which is never destroyed.
        let arrow = unsafe { LoadCursorW(None, IDC_ARROW) }.expect("the arrow cursor loads");
        let mut monitor = grey_monitor();
        draw_cursor(&mut monitor, arrow, POINT { x: 1_198, y: 598 }).expect("the arrow draws");
        assert_ne!(
            pixel(&monitor, 198, 98),
            GREY,
            "the tip is drawn at the corner"
        );
    }
}
