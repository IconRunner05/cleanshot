use std::ops::Range;

use cleanshot_core::{RectF, RedactMode};
use tiny_skia::Pixmap;

use crate::flatten::covered_pixels;

/// The side of a pixelation cell in image pixels. Cells are spread evenly over the area, so
/// none is much smaller than this unless the area itself is.
const CELL: usize = 16;

/// Replaces every pixel `rect` touches (C-22). Pixelate sets each cell to its average; blur
/// pixelates first and then smooths the cells, so neither keeps more than the cell averages.
pub(crate) fn redact(pixmap: &mut Pixmap, rect: RectF, mode: RedactMode) {
    let Some((columns, rows)) = covered_pixels(rect, pixmap.width(), pixmap.height()) else {
        return;
    };
    let columns = columns.start as usize..columns.end as usize;
    let rows = rows.start as usize..rows.end as usize;
    let mut area = Area {
        stride: pixmap.width() as usize,
        data: pixmap.data_mut(),
    };
    for cell_rows in cells(rows.clone()) {
        for cell_columns in cells(columns.clone()) {
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
                for (x, pixel) in columns.clone().zip(box_blur(&line)) {
                    area.set_pixel(x, y, pixel);
                }
            }
            for x in columns {
                let line: Vec<_> = rows.clone().map(|y| area.pixel(x, y)).collect();
                for (y, pixel) in rows.clone().zip(box_blur(&line)) {
                    area.set_pixel(x, y, pixel);
                }
            }
        }
    }
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

/// `span` split into the fewest near-equal runs no longer than [`CELL`].
fn cells(span: Range<usize>) -> impl Iterator<Item = Range<usize>> {
    let count = span.len().div_ceil(CELL);
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

/// Each pixel becomes the mean of the [`CELL`] pixels either side of it, repeating the edge
/// pixel past either end.
fn box_blur(line: &[[u8; 4]]) -> Vec<[u8; 4]> {
    let last = line.len() - 1;
    (0..line.len())
        .map(|index| {
            average(
                (0..=2 * CELL).map(|offset| line[(index + offset).saturating_sub(CELL).min(last)]),
            )
        })
        .collect()
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

    use cleanshot_core::{Color, Command, Document, Fill, History, RgbaImage, Shape, Style};

    use super::*;
    use crate::flatten;

    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 48;
    const MAGENTA: [u8; 4] = [255, 0, 255, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];

    /// No average of two or more of these pixels equals either of them. `phase` 1 swaps the colours.
    fn checkerboard(phase: u32) -> RgbaImage {
        let mut pixels = Vec::new();
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                pixels.extend(if (x + y + phase).is_multiple_of(2) {
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
        Style {
            color: Color { r, g, b, a },
            stroke_width: 1.0,
        }
    }

    fn flatten_with(shapes: Vec<(Shape, Style)>) -> RgbaImage {
        let mut history = History::new(Document::new(Arc::new(checkerboard(0))));
        for (shape, style) in shapes {
            history.apply(Command::Add { shape, style }).unwrap();
        }
        flatten(history.document()).unwrap()
    }

    fn redaction(mode: RedactMode) -> (Shape, Style) {
        (Shape::Redact { rect: AREA, mode }, style(MAGENTA))
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
        let base = checkerboard(0);
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
    fn c22_blur_keeps_nothing_finer_than_the_cell_averages() {
        let whole_cells = RectF {
            x: 0.0,
            y: 0.0,
            width: 32.0,
            height: 16.0,
        };
        let blurred = |phase| {
            let mut history = History::new(Document::new(Arc::new(checkerboard(phase))));
            history
                .apply(Command::Add {
                    shape: Shape::Redact {
                        rect: whole_cells,
                        mode: RedactMode::Blur,
                    },
                    style: style(MAGENTA),
                })
                .unwrap();
            let flat = flatten(history.document()).unwrap();
            pixels(&flat)
                .filter(|&(x, y, _)| x < 32 && y < 16)
                .map(|(_, _, pixel)| pixel)
                .collect::<Vec<_>>()
        };
        assert_eq!(blurred(0), blurred(1));
    }
}
