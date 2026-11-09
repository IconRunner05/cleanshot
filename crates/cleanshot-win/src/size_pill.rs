use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
    D2D1_ROUNDED_RECT, ID2D1RenderTarget, ID2D1SolidColorBrush,
};

use crate::palette::Palette;
use crate::render_target::on_surface;
use crate::text::{Fonts, TextStyle};

// In device-independent pixels.
const TEXT_SIZE: f32 = 12.0;
const PADDING_X: f32 = 8.0;
const PADDING_Y: f32 = 4.0;
const GAP: f32 = 8.0;

/// The selection's size in physical pixels, as the pill shows it.
pub(crate) fn label(selected: RECT) -> String {
    format!(
        "{} \u{d7} {}",
        selected.right - selected.left,
        selected.bottom - selected.top
    )
}

/// Where a pill `width` x `height` goes for `selected` on `monitor`: centred under the
/// selection, above it when there is no room below, and inside its bottom edge when there is
/// room for neither. It never leaves the monitor.
pub(crate) fn place(selected: RECT, monitor: RECT, width: i32, height: i32, gap: i32) -> RECT {
    let left = (i32::midpoint(selected.left, selected.right) - width / 2)
        .clamp(monitor.left, (monitor.right - width).max(monitor.left));
    let below = selected.bottom + gap;
    let above = selected.top - gap - height;
    let top = if below + height <= monitor.bottom {
        below
    } else if above >= monitor.top {
        above
    } else {
        (selected.bottom - gap - height).max(monitor.top)
    };
    RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// Whether the pill for `selected` belongs to `monitor`: the one holding the middle of the
/// selection's bottom edge, so exactly one monitor draws it.
fn belongs_to(selected: RECT, monitor: RECT) -> bool {
    let x = i32::midpoint(selected.left, selected.right);
    let y = selected.bottom - 1;
    (monitor.left..monitor.right).contains(&x) && (monitor.top..monitor.bottom).contains(&y)
}

/// What one monitor's surface keeps to draw the pill.
pub(crate) struct Paint {
    text: TextStyle,
    material: ID2D1SolidColorBrush,
    label: ID2D1SolidColorBrush,
    monitor: RECT,
    scale: f32,
}

impl Paint {
    /// The pill's paint for a surface drawn by `target` over `monitor`, at `scale` physical
    /// pixels per DIP, in `palette`.
    pub(crate) fn new(
        target: &ID2D1RenderTarget,
        monitor: RECT,
        scale: f32,
        palette: &Palette,
    ) -> windows::core::Result<Self> {
        // SAFETY: each colour lives in `palette`, which outlives its call.
        let (material, label) = unsafe {
            (
                target.CreateSolidColorBrush(&raw const palette.material, None)?,
                target.CreateSolidColorBrush(&raw const palette.label, None)?,
            )
        };
        Ok(Self {
            text: Fonts::inter()?.text_style(TEXT_SIZE * scale)?,
            material,
            label,
            monitor,
            scale,
        })
    }

    /// Draws the pill for `selected` with `target`, shifted by `origin` into its pixels, if it
    /// belongs to this monitor.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "the pill is a few dozen pixels and screen coordinates fit in f32"
    )]
    pub(crate) fn draw(
        &self,
        target: &ID2D1RenderTarget,
        selected: RECT,
        origin: POINT,
    ) -> windows::core::Result<()> {
        if !belongs_to(selected, self.monitor) {
            return Ok(());
        }
        let text = label(selected);
        let (text_width, text_height) = self.text.measure(&text)?;
        let px = |dips: f32| (dips * self.scale).round() as i32;
        let width = text_width.ceil() as i32 + 2 * px(PADDING_X);
        let height = text_height.ceil() as i32 + 2 * px(PADDING_Y);
        let at = place(selected, self.monitor, width, height, px(GAP));
        let pill = D2D1_ROUNDED_RECT {
            rect: on_surface(at, origin),
            radiusX: height as f32 / 2.0,
            radiusY: height as f32 / 2.0,
        };
        let text_box = D2D_RECT_F {
            left: pill.rect.left + PADDING_X * self.scale,
            top: pill.rect.top + PADDING_Y * self.scale,
            right: pill.rect.right,
            bottom: pill.rect.bottom,
        };
        // SAFETY: the target and brush live for this call.
        unsafe { target.FillRoundedRectangle(&raw const pill, &self.material) };
        self.text.draw(target, &text, &text_box, &self.label);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: RECT = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn the_pill_reads_width_by_height_in_physical_pixels() {
        assert_eq!(label(rect(-1920, 10, -1000, 610)), "920 \u{d7} 600");
    }

    #[test]
    fn the_pill_sits_under_the_selection_then_above_then_inside() {
        let under = place(rect(100, 100, 300, 200), MONITOR, 80, 20, 8);
        assert_eq!(under, rect(160, 208, 240, 228));
        let above = place(rect(100, 1000, 300, 1070), MONITOR, 80, 20, 8);
        assert_eq!(above, rect(160, 972, 240, 992));
        let inside = place(MONITOR, MONITOR, 80, 20, 8);
        assert_eq!(inside, rect(920, 1052, 1000, 1072));
        let at_edge = place(rect(0, 100, 20, 200), MONITOR, 80, 20, 8);
        assert_eq!(at_edge.left, 0, "the pill stays on the monitor");
    }

    #[test]
    fn on_a_monitor_left_of_and_above_the_primary_the_pill_keeps_its_places() {
        let monitor = rect(-1920, -1080, 0, 0);
        let under = place(rect(-1820, -980, -1620, -880), monitor, 80, 20, 8);
        assert_eq!(under, rect(-1760, -872, -1680, -852));
        let above = place(rect(-1820, -80, -1620, -10), monitor, 80, 20, 8);
        assert_eq!(above, rect(-1760, -108, -1680, -88));
        let inside = place(monitor, monitor, 80, 20, 8);
        assert_eq!(inside, rect(-1000, -28, -920, -8));
        let at_edge = place(rect(-1920, -980, -1900, -880), monitor, 80, 20, 8);
        assert_eq!(at_edge.left, -1920, "the pill stays on the monitor");
    }

    #[test]
    fn of_a_monitor_and_the_one_above_it_one_draws_the_pill() {
        let above = rect(0, -1080, 1920, 0);
        let lower_half = rect(100, -50, 300, 50);
        assert!(belongs_to(lower_half, MONITOR) && !belongs_to(lower_half, above));
        let upper = rect(100, -60, 300, -1);
        assert!(belongs_to(upper, above) && !belongs_to(upper, MONITOR));
    }

    #[test]
    fn one_monitor_draws_the_pill() {
        let right = rect(1920, 0, 3840, 1080);
        let selected = rect(1800, 100, 2000, 300);
        assert!(belongs_to(selected, MONITOR) ^ belongs_to(selected, right));
        assert!(
            belongs_to(MONITOR, MONITOR),
            "a whole-monitor selection draws its own pill"
        );
    }
}
