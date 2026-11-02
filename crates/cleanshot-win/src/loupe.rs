use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, ID2D1Bitmap, ID2D1RenderTarget,
    ID2D1SolidColorBrush,
};

use crate::palette::Palette;
use crate::text::{Fonts, TextStyle};

/// Physical pixels each captured pixel becomes (F-04's 8x).
const ZOOM: i32 = 8;
/// Captured pixels across the loupe; odd, so one sits in the middle under the pointer.
const CELLS: i32 = 17;
const SIZE: i32 = ZOOM * CELLS;
/// DIPs between the pointer and the loupe, and the caption's text size and height.
const GAP: f32 = 24.0;
const CAPTION_TEXT: f32 = 12.0;
const CAPTION_HEIGHT: f32 = 22.0;

/// The captured pixels the loupe shows for a pointer `at`: `CELLS` square around it, moved
/// inside `monitor` at its edges.
pub(crate) fn source(at: POINT, monitor: RECT) -> RECT {
    let left = (at.x - CELLS / 2).clamp(monitor.left, (monitor.right - CELLS).max(monitor.left));
    let top = (at.y - CELLS / 2).clamp(monitor.top, (monitor.bottom - CELLS).max(monitor.top));
    RECT {
        left,
        top,
        right: left + CELLS,
        bottom: top + CELLS,
    }
}

/// Where a box `width` x `height` goes for a pointer `at`: `gap` below and right of it,
/// flipped left or up where `monitor` has no room.
pub(crate) fn place(at: POINT, monitor: RECT, width: i32, height: i32, gap: i32) -> RECT {
    let left = if at.x + gap + width <= monitor.right {
        at.x + gap
    } else {
        at.x - gap - width
    };
    let top = if at.y + gap + height <= monitor.bottom {
        at.y + gap
    } else {
        at.y - gap - height
    };
    RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// F-04: the pointer's desktop coordinates and the colour under it, from `bgra` rows `width`
/// pixels wide whose first pixel is at `origin`.
pub(crate) fn caption(bgra: &[u8], width: i32, origin: POINT, at: POINT) -> Option<String> {
    let column = usize::try_from(at.x - origin.x).ok()?;
    let row = usize::try_from(at.y - origin.y).ok()?;
    let width = usize::try_from(width).ok()?;
    if column >= width {
        return None;
    }
    let pixel = bgra.get((row * width + column) * 4..)?.get(..3)?;
    Some(format!(
        "{}, {}  #{:02X}{:02X}{:02X}",
        at.x, at.y, pixel[2], pixel[1], pixel[0]
    ))
}

/// What one monitor's surface keeps to draw the loupe: its frozen pixels, for the colour.
pub(crate) struct Paint {
    bgra: Vec<u8>,
    monitor: RECT,
    scale: f32,
    text: TextStyle,
    material: ID2D1SolidColorBrush,
    grid: ID2D1SolidColorBrush,
    label: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
}

impl Paint {
    /// The loupe's paint for a surface drawn by `target` over `monitor`, whose frozen pixels
    /// are `bgra`, at `scale` physical pixels per DIP, in `palette`.
    pub(crate) fn new(
        target: &ID2D1RenderTarget,
        bgra: Vec<u8>,
        monitor: RECT,
        scale: f32,
        palette: &Palette,
    ) -> windows::core::Result<Self> {
        // SAFETY: each colour lives in `palette`, which outlives its call.
        let brush = |colour| unsafe { target.CreateSolidColorBrush(colour, None) };
        Ok(Self {
            bgra,
            monitor,
            scale,
            text: Fonts::inter()?.text_style(CAPTION_TEXT * scale)?,
            material: brush(&raw const palette.material)?,
            grid: brush(&raw const palette.hairline)?,
            label: brush(&raw const palette.label)?,
            accent: brush(&raw const palette.accent)?,
        })
    }

    /// Draws the loupe for a pointer `at` from `frozen` with `target`, shifted by `origin`
    /// into its pixels, if the pointer is on this monitor.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "the loupe is a few hundred pixels and screen coordinates fit in f32"
    )]
    pub(crate) fn draw(&self, target: &ID2D1RenderTarget, frozen: &ID2D1Bitmap, at: POINT) {
        let origin = POINT {
            x: self.monitor.left,
            y: self.monitor.top,
        };
        let Some(caption) = caption(
            &self.bgra,
            self.monitor.right - self.monitor.left,
            origin,
            at,
        ) else {
            return;
        };
        let caption_height = (CAPTION_HEIGHT * self.scale).round() as i32;
        let gap = (GAP * self.scale).round() as i32;
        let whole = place(at, self.monitor, SIZE, SIZE + caption_height, gap);
        let local = |x: i32, y: i32| ((x - origin.x) as f32, (y - origin.y) as f32);
        let (left, top) = local(whole.left, whole.top);
        let zoomed = D2D_RECT_F {
            left,
            top,
            right: left + SIZE as f32,
            bottom: top + SIZE as f32,
        };
        let shown = source(at, self.monitor);
        let (source_left, source_top) = local(shown.left, shown.top);
        let from = D2D_RECT_F {
            left: source_left,
            top: source_top,
            right: source_left + CELLS as f32,
            bottom: source_top + CELLS as f32,
        };
        let under = D2D_RECT_F {
            left: zoomed.left + ((at.x - shown.left) * ZOOM) as f32,
            top: zoomed.top + ((at.y - shown.top) * ZOOM) as f32,
            right: zoomed.left + ((at.x - shown.left + 1) * ZOOM) as f32,
            bottom: zoomed.top + ((at.y - shown.top + 1) * ZOOM) as f32,
        };
        let band = D2D_RECT_F {
            left: zoomed.left,
            top: zoomed.bottom,
            right: zoomed.right,
            bottom: zoomed.bottom + caption_height as f32,
        };
        // SAFETY: the target, bitmap and brushes live for this call.
        unsafe {
            target.DrawBitmap(
                frozen,
                Some(&raw const zoomed),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                Some(&raw const from),
            );
            for line in 1..CELLS {
                let offset = (line * ZOOM) as f32;
                let column = D2D_RECT_F {
                    left: zoomed.left + offset,
                    right: zoomed.left + offset + 1.0,
                    ..zoomed
                };
                let row = D2D_RECT_F {
                    top: zoomed.top + offset,
                    bottom: zoomed.top + offset + 1.0,
                    ..zoomed
                };
                target.FillRectangle(&raw const column, &self.grid);
                target.FillRectangle(&raw const row, &self.grid);
            }
            target.DrawRectangle(&raw const under, &self.accent, 2.0, None);
            target.DrawRectangle(&raw const zoomed, &self.grid, 1.0, None);
            target.FillRectangle(&raw const band, &self.material);
        }
        self.text.draw(target, &caption, &band, &self.label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: RECT = RECT {
        left: -1920,
        top: 0,
        right: 0,
        bottom: 1080,
    };

    #[test]
    fn f04_the_loupe_shows_the_pixels_round_the_pointer_and_stays_on_its_monitor() {
        let middle = source(POINT { x: -1000, y: 500 }, MONITOR);
        assert_eq!((middle.left, middle.top), (-1008, 492));
        let corner = source(POINT { x: -1, y: 1079 }, MONITOR);
        assert_eq!((corner.right, corner.bottom), (0, 1080));
        let placed = place(POINT { x: -10, y: 1070 }, MONITOR, 136, 158, 24);
        assert!(placed.right <= 0 && placed.bottom <= 1080, "{placed:?}");
        assert_eq!(placed.left, -10 - 24 - 136);
    }

    #[test]
    fn f04_on_a_monitor_above_the_primary_the_loupe_stays_on_it() {
        let above = RECT {
            left: 0,
            top: -1080,
            right: 1920,
            bottom: 0,
        };
        let corner = source(POINT { x: 0, y: -1080 }, above);
        assert_eq!((corner.left, corner.top), (0, -1080));
        let placed = place(POINT { x: 10, y: -5 }, above, 136, 158, 24);
        assert_eq!(placed.top, -5 - 24 - 158);
        assert!(placed.bottom <= 0, "{placed:?}");
    }

    #[test]
    fn f04_the_caption_names_the_pointer_and_the_colour_under_it() {
        let mut bgra = vec![0; 3 * 2 * 4];
        bgra[(3 + 2) * 4..(3 + 2) * 4 + 4].copy_from_slice(&[0x56, 0x34, 0x12, 0xFF]);
        let origin = POINT { x: -1920, y: 0 };
        let at = POINT { x: -1918, y: 1 };
        assert_eq!(
            caption(&bgra, 3, origin, at).as_deref(),
            Some("-1918, 1  #123456")
        );
        assert_eq!(caption(&bgra, 3, origin, POINT { x: -1917, y: 1 }), None);
    }
}
