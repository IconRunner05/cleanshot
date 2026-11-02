use std::borrow::Cow;
use std::sync::Arc;

use cleanshot_core::{INTER, LINE_HEIGHT, Label, PointF, TextBlock, baseline};
use gpui::{
    App, Font, FontFeatures, FontWeight, Global, Pixels, Point, Rgba, ShapedLine, TextAlign,
    TextRun, Window, black, font, point, px,
};

use crate::Error;
use crate::placement::Fit;

/// The embedded Inter at the one weight F-34 draws text in, kerned (D-69).
fn inter() -> Font {
    Font {
        weight: FontWeight::SEMIBOLD,
        features: FontFeatures(Arc::new(vec![("kern".into(), 1)])),
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

/// One line of a text block shaped by GPUI, with the top-left corner and height of its line box.
pub(crate) struct LaidLine {
    pub shaped: ShapedLine,
    pub origin: Point<Pixels>,
    pub height: Pixels,
}

/// The size, in pixels per em, that [`em_width`] measures at; advances scale linearly with size
/// in the natural measuring mode, so any size gives the same ems, and a large one keeps them
/// precise.
const MEASURE_SIZE: f32 = 100.0;

/// `block`'s lines shaped in Inter and `color`, `scale` pixels to an image pixel, each line box
/// placed by `place` on core's baselines.
pub(crate) fn lay_out(
    block: &TextBlock,
    color: Rgba,
    scale: f32,
    place: impl Fn(PointF) -> Point<Pixels>,
    window: &Window,
) -> Vec<LaidLine> {
    let font_size = px(block.size() * scale);
    let height = font_size * LINE_HEIGHT;
    block
        .lines()
        .map(|(line, baseline)| {
            let shaped = shape(line, font_size, color, window);
            let origin = line_origin(place(baseline), height, shaped.ascent, shaped.descent);
            LaidLine {
                shaped,
                origin,
                height,
            }
        })
        .collect()
}

/// The widest line of `text` in ems, as [`TextBlock::new`] takes it (F-34).
pub(crate) fn em_width(text: &str, window: &Window) -> f32 {
    text.split('\n')
        .map(|line| f32::from(shape(line, px(MEASURE_SIZE), black().into(), window).width()))
        .fold(0.0, f32::max)
        / MEASURE_SIZE
}

/// Each of `texts` measured as a committed text block measures it, in ems, for F-34's drift
/// test against DirectWrite. The hidden window it measures in is closed before it returns.
///
/// # Errors
///
/// [`Error::FontLoad`] if Inter cannot be registered, [`Error::WindowOpen`] if the window does
/// not open.
#[cfg(feature = "drift-test")]
pub fn em_widths(texts: &[&str], cx: &mut App) -> Result<Vec<f32>, Error> {
    use gpui::{AppContext as _, EmptyView, WindowOptions};

    register_inter(cx)?;
    let options = WindowOptions {
        show: false,
        focus: false,
        ..WindowOptions::default()
    };
    let mut widths = Vec::new();
    let window = cx
        .open_window(options, |window, cx| {
            widths = texts.iter().map(|text| em_width(text, window)).collect();
            cx.new(|_| EmptyView)
        })
        .map_err(|error| Error::WindowOpen(error.into()))?;
    // Err only means the window is already gone, which is the state this leaves it in.
    let _ = window.update(cx, |_, window, _| window.remove_window());
    Ok(widths)
}

fn shape(line: &str, font_size: Pixels, color: Rgba, window: &Window) -> ShapedLine {
    let run = TextRun {
        len: line.len(),
        font: inter(),
        color: color.into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(line.to_owned().into(), font_size, &[run], None)
}

/// Paints `label` in Inter and `color`, laid out as the export lays it out (F-34, F-41).
pub(crate) fn paint_label(
    label: &Label,
    color: Rgba,
    fit: &Fit,
    window: &mut Window,
    cx: &mut App,
) {
    let lines = match label {
        Label::Block(block) => lay_out(block, color, fit.scale, |at| fit.point(at), window),
        Label::Centred {
            text,
            size,
            centre_x,
            top,
        } => {
            let font_size = px(size * fit.scale);
            let height = font_size * LINE_HEIGHT;
            let shaped = shape(text, font_size, color, window);
            let start = fit.point(PointF {
                x: *centre_x,
                y: baseline(*top, *size),
            });
            let left = point(start.x - shaped.width() / 2.0, start.y);
            let origin = line_origin(left, height, shaped.ascent, shaped.descent);
            vec![LaidLine {
                shaped,
                origin,
                height,
            }]
        }
    };
    for line in lines {
        // Fails only if a glyph cannot be rasterised; the document keeps the text.
        let _ = line
            .shaped
            .paint(line.origin, line.height, TextAlign::Left, None, window, cx);
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
