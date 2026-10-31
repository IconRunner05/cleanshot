use cleanshot_core::Color;

#[expect(
    clippy::cast_possible_truncation,
    reason = "GPUI scales in f64 and rounds to f32 just so; the ratios must match its bits"
)]
const NORM13: f32 = ((0x10000 as f64) / (255.0 * 255.0) * 4.0) as f32;
#[expect(
    clippy::cast_possible_truncation,
    reason = "GPUI scales in f64 and rounds to f32 just so; the ratios must match its bits"
)]
const NORM24: f32 = ((0x100 as f64) / (255.0) * 4.0) as f32;

/// The alpha correction GPUI's `get_gamma_correction_ratios` gives for the export's fixed
/// DirectWrite gamma, 1.8 (D-51): its gamma 1.8 row, scaled as it scales them.
pub const TEXT_GAMMA_RATIOS: [f32; 4] = [
    0.1469 / 4.0 * NORM13,
    -0.8911 / 4.0 * NORM24,
    1.4644 / 4.0 * NORM13,
    -0.3234 / 4.0 * NORM24,
];

/// The export's fixed DirectWrite grayscale enhanced contrast (D-51).
const TEXT_ENHANCED_CONTRAST: f32 = 1.0;

/// For each glyph coverage byte, the coverage GPUI's shader would draw it with in `color`:
/// contrast enhanced for dark text, then gamma corrected (`alpha_correction.hlsl`).
pub(crate) fn coverage_table(color: Color) -> [u8; 256] {
    let [r, g, b] = [color.r, color.g, color.b].map(|channel| f32::from(channel) / 255.0);
    let brightness = r * 0.30 + g * 0.59 + b * 0.11;
    let contrast = TEXT_ENHANCED_CONTRAST * (4.0 * (0.75 - brightness)).clamp(0.0, 1.0);
    let [g0, g1, g2, g3] = TEXT_GAMMA_RATIOS;
    let mut table = [0; 256];
    for (coverage, corrected) in (0..=u8::MAX).zip(&mut table) {
        let sample = f32::from(coverage) / 255.0;
        let contrasted = sample * (contrast + 1.0) / (sample * contrast + 1.0);
        let correction = (g0 * brightness + g1) * contrasted + (g2 * brightness + g3);
        let alpha = contrasted + contrasted * (1.0 - contrasted) * correction;
        *corrected = byte(alpha);
    }
    table
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=255 and rounded first"
)]
fn byte(unit: f32) -> u8 {
    (unit.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn opaque(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    #[test]
    fn f34_no_and_full_coverage_stay_as_they_are_in_every_colour() {
        for color in [opaque(0, 0, 0), opaque(255, 255, 255), opaque(255, 59, 48)] {
            let table = coverage_table(color);
            assert_eq!((table[0], table[255]), (0, 255), "{color:?}");
        }
    }

    #[test]
    fn f34_partial_coverage_is_drawn_as_gpui_draws_it_for_dark_and_light_text() {
        let (black, white) = (
            coverage_table(opaque(0, 0, 0)),
            coverage_table(opaque(255, 255, 255)),
        );
        assert_eq!((black[128], black[192]), (118, 185));
        assert_eq!((white[64], white[192]), (110, 220));
    }
}
