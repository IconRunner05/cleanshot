use crate::document::{Color, PointF, RectF};
use crate::text::{LINE_HEIGHT, Label};

/// A step's circle across, as a multiple of its number's font size (F-41, D-85).
pub const STEP_DIAMETER: f32 = 1.6;

/// A numbered step (F-41): a circle filled with the style colour, its number centred in it.
/// The number is not stored: it is the step's place among the document's steps, counted in
/// drawing order, so removing one renumbers the steps after it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    centre: PointF,
    size: f32,
}

impl Step {
    /// A step at `centre` whose number is `size` pixels per em; the caller has checked that
    /// `size` is positive.
    pub(crate) fn new(centre: PointF, size: f32) -> Self {
        Self { centre, size }
    }

    /// The circle's centre, in image pixels.
    #[must_use]
    pub fn centre(&self) -> PointF {
        self.centre
    }

    /// The number's font size in image pixels, always positive.
    #[must_use]
    pub fn size(&self) -> f32 {
        self.size
    }

    /// The box the circle fills.
    #[must_use]
    pub fn circle(&self) -> RectF {
        let diameter = STEP_DIAMETER * self.size;
        RectF {
            x: self.centre.x - diameter / 2.0,
            y: self.centre.y - diameter / 2.0,
            width: diameter,
            height: diameter,
        }
    }

    /// `number` on one line, centred on the circle: its line box's middle on the centre.
    #[must_use]
    pub fn label(&self, number: usize) -> Label<'static> {
        Label::Centred {
            text: number.to_string(),
            size: self.size,
            centre_x: self.centre.x,
            top: self.centre.y - LINE_HEIGHT * self.size / 2.0,
        }
    }

    pub(crate) fn with_size(self, size: f32) -> Self {
        Self { size, ..self }
    }

    pub(crate) fn moved_to(self, centre: PointF) -> Self {
        Self { centre, ..self }
    }
}

/// The colour a step's number is drawn in on a circle of `fill`: black on a light colour,
/// white on a dark one, by the colour's luma (BT.601).
#[must_use]
pub fn number_color(fill: Color) -> Color {
    let luma = 299 * u32::from(fill.r) + 587 * u32::from(fill.g) + 114 * u32::from(fill.b);
    let level = if luma > 150_000 { 0 } else { 255 };
    Color {
        r: level,
        g: level,
        b: level,
        a: 255,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    #[test]
    fn f41_a_step_number_is_white_on_dark_colours_and_black_on_light_ones() {
        let white = rgb(255, 255, 255);
        let black = rgb(0, 0, 0);
        for dark in [rgb(255, 59, 48), rgb(0, 122, 255), rgb(175, 82, 222), black] {
            assert_eq!(number_color(dark), white, "{dark:?}");
        }
        for light in [rgb(255, 230, 0), rgb(255, 149, 0), white] {
            assert_eq!(number_color(light), black, "{light:?}");
        }
    }

    #[test]
    fn f41_a_step_number_sits_centred_on_its_circle() {
        let step = Step::new(PointF { x: 50.0, y: 40.0 }, 20.0);
        assert_eq!(
            step.circle(),
            RectF {
                x: 34.0,
                y: 24.0,
                width: 32.0,
                height: 32.0
            }
        );
        assert_eq!(
            step.label(12),
            Label::Centred {
                text: "12".to_owned(),
                size: 20.0,
                centre_x: 50.0,
                top: 27.5
            }
        );
    }
}
