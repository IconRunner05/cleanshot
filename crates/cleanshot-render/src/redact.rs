use std::ops::Range;

use cleanshot_core::{RectF, RedactMode, RedactStrength, RgbaImage};
use tiny_skia::Pixmap;

use crate::flatten::{Covered, covered_pixels};
use crate::{Error, Patch};

/// Replaces every pixel `rect` touches (C-22). Pixelate sets each cell, `strength`'s side, to its
/// average; blur pixelates first and then smooths the cells over the same radius, so neither
/// keeps more than the cell averages (D-123).
pub(crate) fn redact(pixmap: &mut Pixmap, rect: RectF, mode: RedactMode, strength: RedactStrength) {
    let cell = usize::from(strength.pixels());
    let Some((columns, rows)) = covered_pixels(rect, pixmap.width(), pixmap.height()) else {
        return;
    };
    let columns = columns.start as usize..columns.end as usize;
    let rows = rows.start as usize..rows.end as usize;
    let mut area = Area {
        stride: pixmap.width() as usize,
        data: pixmap.data_mut(),
    };
    for cell_rows in cells(rows.clone(), cell) {
        for cell_columns in cells(columns.clone(), cell) {
            let pixels: Vec<_> = cell_rows
                .clone()
                .flat_map(|y| cell_columns.clone().map(move |x| (x, y)))
                .collect();
            let average = average(pixels.iter().map(|&(x, y)| area.pixel(x, y)));
            for (x, y) in pixels {
                area.set_pixel(x, y, average);
            }
        }
    }
    match mode {
        RedactMode::Pixelate => {}
        RedactMode::Blur => {
            for y in rows.clone() {
                let line: Vec<_> = columns.clone().map(|x| area.pixel(x, y)).collect();
                for (x, pixel) in columns.clone().zip(box_blur(&line, cell)) {
                    area.set_pixel(x, y, pixel);
                }
            }
            for x in columns {
                let line: Vec<_> = rows.clone().map(|y| area.pixel(x, y)).collect();
                for (y, pixel) in rows.clone().zip(box_blur(&line, cell)) {
                    area.set_pixel(x, y, pixel);
                }
            }
        }
    }
}

/// The pixels a redaction of `rect` in `mode` at `strength` leaves of `image`, exactly as an
/// export draws them where no shape is under it (C-22, F-36); `None` if `rect` covers no pixel.
///
/// # Errors
///
/// [`Error::TooLarge`] if the covered area exceeds the renderer's size limit.
pub fn redacted(
    image: &RgbaImage,
    rect: RectF,
    mode: RedactMode,
    strength: RedactStrength,
) -> Result<Option<Patch>, Error> {
    let Some(mut covered) = Covered::of(image, rect)? else {
        return Ok(None);
    };
    let local = covered.local(rect);
    redact(&mut covered.pixmap, local, mode, strength);
    covered.into_patch().map(Some)
}

/// Premultiplied RGBA bytes, `stride` pixels to a row.
struct Area<'a> {
    stride: usize,
    data: &'a mut [u8],
}

impl Area<'_> {
    fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let start = (y * self.stride + x) * 4;
        [
            self.data[start],
            self.data[start + 1],
            self.data[start + 2],
            self.data[start + 3],
        ]
    }

    fn set_pixel(&mut self, x: usize, y: usize, pixel: [u8; 4]) {
        let start = (y * self.stride + x) * 4;
        self.data[start..start + 4].copy_from_slice(&pixel);
    }
}

/// `span` split into the fewest near-equal runs no longer than `cell`, so none is much shorter
/// than `cell` unless `span` itself is.
fn cells(span: Range<usize>, cell: usize) -> impl Iterator<Item = Range<usize>> {
    let count = span.len().div_ceil(cell);
    (0..count).map(move |index| {
        span.start + index * span.len() / count..span.start + (index + 1) * span.len() / count
    })
}

/// The rounded mean of each channel. The mean of premultiplied pixels is premultiplied.
fn average(pixels: impl Iterator<Item = [u8; 4]>) -> [u8; 4] {
    let mut sums = [0_usize; 4];
    let mut count = 0;
    for pixel in pixels {
        for (sum, channel) in sums.iter_mut().zip(pixel) {
            *sum += usize::from(channel);
        }
        count += 1;
    }
    sums.map(|sum| channel((sum + count / 2) / count.max(1)))
}

/// Each pixel becomes the mean of the `radius` pixels either side of it, repeating the edge
/// pixel past either end.
fn box_blur(line: &[[u8; 4]], radius: usize) -> Vec<[u8; 4]> {
    let last = line.len() - 1;
    let padded = |index: usize| line[index.saturating_sub(radius).min(last)];
    let window = 2 * radius + 1;
    let mut sums = [0_usize; 4];
    for index in 0..window {
        for (sum, channel) in sums.iter_mut().zip(padded(index)) {
            *sum += usize::from(channel);
        }
    }
    let mut blurred = Vec::with_capacity(line.len());
    for index in 0..line.len() {
        blurred.push(sums.map(|sum| channel((sum + window / 2) / window)));
        let (leaving, entering) = (padded(index), padded(index + window));
        for ((sum, out), into) in sums.iter_mut().zip(leaving).zip(entering) {
            *sum = *sum + usize::from(into) - usize::from(out);
        }
    }
    blurred
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a mean of u8 channels fits in u8"
)]
fn channel(mean: usize) -> u8 {
    mean as u8
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cleanshot_core::{
        Color, Command, Document, Fill, History, RgbaImage, Shape, Style, TextMasks,
    };

    use super::*;
    use crate::flatten;

    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 48;
    const MAGENTA: [u8; 4] = [255, 0, 255, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];

    /// No average of two or more of these pixels equals either of them. `phase` 1 swaps the colours.
    fn checkerboard(phase: u32, square: u32) -> RgbaImage {
        let mut pixels = Vec::new();
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                pixels.extend(if (x / square + y / square + phase).is_multiple_of(2) {
                    MAGENTA
                } else {
                    GREEN
                });
            }
        }
        RgbaImage::new(WIDTH, HEIGHT, pixels).unwrap()
    }

    /// 33 by 17 pixels: one past two cells each way, so uneven cells would leave a 1x1 corner.
    const AREA: RectF = RectF {
        x: 3.0,
        y: 2.0,
        width: 33.0,
        height: 17.0,
    };

    fn in_area(x: u32, y: u32) -> bool {
        (3..36).contains(&x) && (2..19).contains(&y)
    }

    fn style(rgba: [u8; 4]) -> Style {
        let [r, g, b, a] = rgba;
        Style::new(Color { r, g, b, a }, 1.0)
    }

    fn flatten_with(shapes: Vec<(Shape, Style)>) -> RgbaImage {
        let mut history = History::new(Document::new(Arc::new(checkerboard(0, 1))));
        for (shape, style) in shapes {
            history.apply(Command::Add { shape, style }).unwrap();
        }
        flatten(history.document(), &TextMasks::default())
            .unwrap()
            .image()
            .clone()
    }

    fn redaction(mode: RedactMode) -> (Shape, Style) {
        (
            Shape::Redact {
                rect: AREA,
                mode,
                strength: RedactStrength::WEAKEST,
            },
            style(MAGENTA),
        )
    }

    fn pixels(image: &RgbaImage) -> impl Iterator<Item = (u32, u32, [u8; 4])> + '_ {
        image
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .zip(0..)
            .map(|(&pixel, index)| (index % WIDTH, index / WIDTH, pixel))
    }

    fn assert_no_original_pixel_survives(mode: RedactMode) {
        let flat = flatten_with(vec![redaction(mode)]);
        let base = checkerboard(0, 1);
        for ((x, y, pixel), (_, _, original)) in pixels(&flat).zip(pixels(&base)) {
            if in_area(x, y) {
                assert!(
                    pixel != MAGENTA && pixel != GREEN,
                    "({x}, {y}) kept {pixel:?}"
                );
            } else {
                assert_eq!(pixel, original, "({x}, {y}) outside the area changed");
            }
        }
    }

    #[test]
    fn c22_pixelate_leaves_no_original_pixel_in_the_area() {
        assert_no_original_pixel_survives(RedactMode::Pixelate);
    }

    #[test]
    fn c22_blur_leaves_no_original_pixel_in_the_area() {
        assert_no_original_pixel_survives(RedactMode::Blur);
    }

    #[test]
    fn c22_a_shape_under_a_redaction_is_redacted_and_one_over_it_is_kept() {
        let under = [255, 0, 0, 255];
        let over = [0, 0, 255, 255];
        let band = |y: f32| RectF {
            x: 0.0,
            y,
            width: 64.0,
            height: 2.0,
        };
        let solid = |rect| Shape::Rect {
            rect,
            fill: Fill::Solid,
        };
        let flat = flatten_with(vec![
            (solid(band(4.0)), style(under)),
            redaction(RedactMode::Pixelate),
            (solid(band(12.0)), style(over)),
        ]);

        for (x, y, pixel) in pixels(&flat) {
            if in_area(x, y) {
                assert_ne!(
                    pixel, under,
                    "({x}, {y}) kept the shape under the redaction"
                );
            }
            if (12..14).contains(&y) {
                assert_eq!(pixel, over, "({x}, {y}) lost the shape over the redaction");
            }
        }
    }

    #[test]
    fn c22_a_redaction_in_a_dimmed_area_keeps_no_original_pixel_dimmed_or_not() {
        let spotlight = (
            Shape::Spotlight(RectF {
                x: 40.0,
                y: 30.0,
                width: 20.0,
                height: 10.0,
            }),
            style(MAGENTA),
        );
        let dimmed = flatten_with(vec![spotlight.clone()]);
        for mode in [RedactMode::Pixelate, RedactMode::Blur] {
            let flat = flatten_with(vec![spotlight.clone(), redaction(mode)]);
            for ((x, y, pixel), (_, _, dimmed_original)) in pixels(&flat).zip(pixels(&dimmed)) {
                if in_area(x, y) {
                    assert!(
                        ![MAGENTA, GREEN, dimmed_original].contains(&pixel),
                        "{mode:?} kept ({x}, {y}) as {pixel:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn c22_at_16_px_neither_mode_keeps_anything_finer_than_the_cell_averages() {
        let whole_cells = RectF {
            x: 0.0,
            y: 0.0,
            width: 32.0,
            height: 16.0,
        };
        let redacted = |phase, mode| {
            let mut history = History::new(Document::new(Arc::new(checkerboard(phase, 8))));
            history
                .apply(Command::Add {
                    shape: Shape::Redact {
                        rect: whole_cells,
                        mode,
                        strength: RedactStrength::FINE,
                    },
                    style: style(MAGENTA),
                })
                .unwrap();
            let flat = flatten(history.document(), &TextMasks::default())
                .unwrap()
                .image()
                .clone();
            pixels(&flat)
                .filter(|&(x, y, _)| x < 32 && y < 16)
                .map(|(_, _, pixel)| pixel)
                .collect::<Vec<_>>()
        };
        for mode in [RedactMode::Pixelate, RedactMode::Blur] {
            assert_eq!(redacted(0, mode), redacted(1, mode), "{mode:?}");
        }
    }

    #[test]
    fn d123_each_strength_pixelates_in_cells_of_its_own_side() {
        let (width, height) = (96, 8);
        let ramp = (0..height)
            .flat_map(|_| (0..width).flat_map(|x| [u8::try_from(x * 2).unwrap(), 0, 0, 255]))
            .collect();
        let image = RgbaImage::new(width, height, ramp).unwrap();
        let whole = RectF {
            x: 0.0,
            y: 0.0,
            width: 96.0,
            height: 8.0,
        };
        for (strength, cells) in [
            (RedactStrength::FINE, 6),
            (RedactStrength::MEDIUM, 4),
            (RedactStrength::COARSE, 3),
            (RedactStrength::COARSEST, 2),
        ] {
            let patch = redacted(&image, whole, RedactMode::Pixelate, strength)
                .unwrap()
                .unwrap();
            let row = &patch.pixels.pixels()[..96 * 4];
            let changes = row
                .as_chunks::<4>()
                .0
                .windows(2)
                .filter(|pair| pair[0] != pair[1])
                .count();
            assert_eq!(changes + 1, cells, "{strength:?}");
        }
    }

    /// No two neighbouring pixels alike, and every other column half transparent.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "each channel is reduced modulo 256 first"
    )]
    fn varied() -> RgbaImage {
        let mut pixels = Vec::new();
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let alpha = if x % 2 == 0 { 255 } else { 128 };
                pixels.extend([
                    ((x * 37 + y * 11) % 256) as u8,
                    ((x * 5 + y * 53) % 256) as u8,
                    ((x * y * 7) % 256) as u8,
                    alpha,
                ]);
            }
        }
        RgbaImage::new(WIDTH, HEIGHT, pixels).unwrap()
    }

    #[test]
    fn f36_the_editor_preview_of_a_redaction_is_the_exported_pixels() {
        let rect = RectF {
            x: 3.4,
            y: 2.6,
            width: 32.5,
            height: 16.2,
        };
        for mode in [RedactMode::Pixelate, RedactMode::Blur] {
            let mut history = History::new(Document::new(Arc::new(varied())));
            let command = Command::Add {
                shape: Shape::Redact {
                    rect,
                    mode,
                    strength: RedactStrength::FINE,
                },
                style: style(MAGENTA),
            };
            history.apply(command).unwrap();
            let export = flatten(history.document(), &TextMasks::default()).unwrap();
            let preview = redacted(&varied(), rect, mode, RedactStrength::FINE)
                .unwrap()
                .unwrap();
            let size = (preview.pixels.width(), preview.pixels.height());
            assert_eq!(
                (preview.left, preview.top, size),
                (3, 2, (33, 17)),
                "{mode:?}"
            );
            for (x, y, pixel) in pixels(export.image()) {
                if (3..36).contains(&x) && (2..19).contains(&y) {
                    let at = ((y - 2) * 33 + (x - 3)) as usize * 4;
                    assert_eq!(
                        &preview.pixels.pixels()[at..at + 4],
                        pixel,
                        "{mode:?} {x},{y}"
                    );
                }
            }
        }
    }

    #[test]
    fn d130_a_blurred_pixel_is_the_rounded_mean_of_the_radius_either_side_at_any_radius() {
        let line: Vec<[u8; 4]> = (0..=255_u8)
            .cycle()
            .take(300)
            .map(|x| [x.wrapping_mul(37), x.wrapping_mul(101), x % 7, 255])
            .collect();
        for length in [1, 2, 5, 300] {
            let line = &line[..length];
            for radius in [1, 16, 128] {
                let last = length - 1;
                let expected: Vec<_> =
                    (0..length)
                        .map(|index| {
                            average((0..=2 * radius).map(|offset| {
                                line[(index + offset).saturating_sub(radius).min(last)]
                            }))
                        })
                        .collect();
                assert_eq!(
                    box_blur(line, radius),
                    expected,
                    "{length} pixels, radius {radius}"
                );
            }
        }
    }
}
