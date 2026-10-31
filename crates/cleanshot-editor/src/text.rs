use std::borrow::Cow;

use cleanshot_core::{INTER, LINE_HEIGHT, TextBlock};
use gpui::{
    App, Font, FontWeight, Global, Pixels, Point, Rgba, TextAlign, TextRun, Window, font, point, px,
};

use crate::Error;
use crate::placement::Fit;

/// The embedded Inter at the one weight F-34 draws text in.
fn inter() -> Font {
    Font {
        weight: FontWeight::SEMIBOLD,
        ..font("Inter")
    }
}

struct InterRegistered;

impl Global for InterRegistered {}

/// Hands core's Inter bytes to GPUI, once per app, so `inter()` finds them before any installed
/// font.
pub(crate) fn register_inter(cx: &mut App) -> Result<(), Error> {
    if cx.has_global::<InterRegistered>() {
        return Ok(());
    }
    cx.text_system()
        .add_fonts(vec![Cow::Borrowed(INTER)])
        .map_err(|error| Error::FontLoad(error.into()))?;
    cx.set_global(InterRegistered);
    Ok(())
}

pub(crate) fn paint_text(
    block: &TextBlock,
    color: Rgba,
    fit: &Fit,
    window: &mut Window,
    cx: &mut App,
) {
    let font_size = px(block.size() * fit.scale);
    let line_height = font_size * LINE_HEIGHT;
    for (line, baseline) in block.lines() {
        let run = TextRun {
            len: line.len(),
            font: inter(),
            color: color.into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped =
            window
                .text_system()
                .shape_line(line.to_owned().into(), font_size, &[run], None);
        let origin = line_origin(
            fit.point(baseline),
            line_height,
            shaped.ascent,
            shaped.descent,
        );
        // Fails only if a glyph cannot be rasterised; the document keeps the text.
        let _ = shaped.paint(origin, line_height, TextAlign::Left, None, window, cx);
    }
}

/// F-34: core places each baseline. GPUI paints a line's baseline
/// `(line_height - ascent - descent) / 2 + ascent` below the origin it is given, so this is the
/// origin that puts it on `baseline`.
fn line_origin(
    baseline: Point<Pixels>,
    line_height: Pixels,
    ascent: Pixels,
    descent: Pixels,
) -> Point<Pixels> {
    point(
        baseline.x,
        baseline.y - (line_height + ascent - descent) / 2.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f34_a_line_painted_from_its_origin_has_its_baseline_where_core_put_it() {
        let baseline = point(px(40.0), px(100.0));
        let (line_height, ascent, descent) = (px(30.0), px(19.0), px(6.0));
        let origin = line_origin(baseline, line_height, ascent, descent);
        let gpui_baseline = origin.y + (line_height - ascent - descent) / 2.0 + ascent;
        assert_eq!(origin.x, baseline.x);
        assert_eq!(gpui_baseline, baseline.y);
        assert_eq!(origin.y, px(78.5));
    }

    #[test]
    fn f34_the_exports_gamma_correction_is_gpuis_for_the_runners_gamma() {
        assert_eq!(
            cleanshot_render::TEXT_GAMMA_RATIOS,
            gpui::get_gamma_correction_ratios(1.8)
        );
    }
}
