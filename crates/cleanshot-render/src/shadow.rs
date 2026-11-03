use cleanshot_core::{Color, Style};
use tiny_skia::Pixmap;

use crate::flatten::source_over;

const BLACK: Color = Color {
    r: 0,
    g: 0,
    b: 0,
    a: 255,
};

/// The weights of a Gaussian whose blur radius is `blur` pixels, twice its standard deviation,
/// for offsets `-reach..=reach`, with `reach`. The weights are not normalised.
pub(crate) fn gaussian(blur: u32) -> (u32, Vec<f64>) {
    let sigma = f64::from(blur) / 2.0;
    let reach = blur * 3 / 2;
    let weights = (-i64::from(reach)..=i64::from(reach))
        .map(|offset| {
            #[expect(clippy::cast_precision_loss, reason = "offsets are under 2^52")]
            let offset = offset as f64;
            (-(offset * offset) / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    (reach, weights)
}

/// Paints onto `pixmap` the shadow of what `layer` covers (D-114): its coverage blurred by
/// [`Style::SHADOW_BLUR`], moved down by [`Style::SHADOW_OFFSET`], in black at
/// [`Style::SHADOW_ALPHA`]. `layer` is the size of `pixmap`.
pub(crate) fn cast_shadow(pixmap: &mut Pixmap, layer: &Pixmap) {
    let width = layer.width() as usize;
    let height = layer.height() as usize;
    let coverage = |x: usize, y: usize| f64::from(layer.pixels()[y * width + x].alpha()) / 255.0;
    let mut bounds: Option<(usize, usize, usize, usize)> = None;
    for y in 0..height {
        for x in (0..width).filter(|&x| coverage(x, y) > 0.0) {
            let (left, right, top, _) = bounds.unwrap_or((x, x, y, y));
            bounds = Some((left.min(x), right.max(x), top, y));
        }
    }
    let Some((left, right, top, bottom)) = bounds else {
        return;
    };
    let (reach, weights) = gaussian(Style::SHADOW_BLUR);
    let total: f64 = weights.iter().sum();
    let spread = reach as usize;
    let columns = left.saturating_sub(spread)..(right + spread + 1).min(width);
    let rows = top.saturating_sub(spread)..(bottom + spread + 1).min(height);
    let reach = i64::from(reach);
    let blur = |at: usize, sample: &dyn Fn(usize) -> f64, within: (usize, usize)| {
        (-reach..=reach)
            .zip(&weights)
            .filter_map(|(offset, weight)| {
                let index = usize::try_from(i64::try_from(at).ok()? + offset).ok()?;
                (within.0..=within.1)
                    .contains(&index)
                    .then(|| weight * sample(index))
            })
            .sum::<f64>()
            / total
    };
    let across: Vec<Vec<f64>> = (top..=bottom)
        .map(|y| {
            columns
                .clone()
                .map(|x| blur(x, &|column| coverage(column, y), (left, right)))
                .collect()
        })
        .collect();
    let offset = Style::SHADOW_OFFSET as usize;
    let pixels = pixmap.pixels_mut();
    for y in rows {
        let below = y + offset;
        if below >= height {
            continue;
        }
        for (column, x) in columns.clone().enumerate() {
            let shade = blur(y, &|row| across[row - top][column], (top, bottom));
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "shade is in 0..=1, so the product is in 0..=255"
            )]
            let alpha = (f64::from(Style::SHADOW_ALPHA) * shade * 255.0).round() as u8;
            let pixel = &mut pixels[below * width + x];
            *pixel = source_over(*pixel, BLACK, alpha);
        }
    }
}

#[cfg(test)]
mod tests {
    use tiny_skia::{Color as SkiaColor, Paint, Rect, Transform};

    use super::*;

    fn red(pixmap: &Pixmap, x: u32, y: u32) -> u8 {
        pixmap.pixel(x, y).unwrap().red()
    }

    #[test]
    fn d114_a_shadow_falls_below_the_shape_fades_out_and_leaves_the_rest_alone() {
        let mut layer = Pixmap::new(80, 60).unwrap();
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 0, 0, 255);
        layer.fill_rect(
            Rect::from_xywh(20.0, 20.0, 20.0, 10.0).unwrap(),
            &paint,
            Transform::identity(),
            None,
        );
        let mut pixmap = Pixmap::new(80, 60).unwrap();
        pixmap.fill(SkiaColor::WHITE);
        cast_shadow(&mut pixmap, &layer);
        let (below, above) = (red(&pixmap, 30, 31), red(&pixmap, 30, 18));
        assert!(below < above, "below {below}, above {above}");
        let darkest = (0..60).map(|y| red(&pixmap, 30, y)).min().unwrap();
        assert!(
            f32::from(255 - darkest) <= Style::SHADOW_ALPHA * 255.0 + 1.0,
            "{darkest}"
        );
        assert_eq!(red(&pixmap, 2, 2), 255, "far from the shape");
        assert_eq!(red(&pixmap, 30, 50), 255, "past the blur");
        assert_eq!(
            red(&pixmap, 16, 25),
            red(&pixmap, 43, 25),
            "left and right alike"
        );
    }
}
