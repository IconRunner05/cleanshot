use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cleanshot_core::{
    Color, Command, Document, Fill, History, PointF, RectF, RgbaImage, Shape, Style, TextBlock,
};
use gpui::{App, Pixels, Point, Window, point, px};

use crate::Error;
use crate::editor_window::{open_with, show_at};
use crate::placement::Placement;

const WIDTH: u32 = 3840;
const HEIGHT: u32 = 2160;
const SHAPES: u32 = 200;
const COLUMNS: u32 = 20;

/// Editor timings from one [`bench`] run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BenchReport {
    /// P-07: from opening the window, image upload included, to the frame after its first draw,
    /// so up to one display refresh (about 16.7 ms at 60 Hz) over the draw itself.
    pub first_frame: Duration,
    /// P-08: median frame-to-frame interval over the forced redraws: a third held still, a third
    /// panning at 2x, a third zooming between fitted and 4x.
    pub p50: Duration,
    /// P-08: 95th-percentile interval (nearest rank).
    pub p95: Duration,
    /// P-08: longest interval.
    pub worst: Duration,
}

type OnDone = Box<dyn FnOnce(&mut App, Result<BenchReport, Error>)>;

/// Opens the editor on a synthetic 4K image with 200 shapes, forces `redraws` redraws while
/// panning and zooming as F-39 does, closes
/// the window and passes the timings to `on_done`, which is called exactly once.
///
/// The window ignores close requests until the run ends.
pub fn bench(
    cx: &mut App,
    redraws: NonZeroU32,
    on_done: impl FnOnce(&mut App, Result<BenchReport, Error>) + 'static,
) {
    let document = match synthetic_document() {
        Ok(document) => document,
        Err(error) => return on_done(cx, Err(error)),
    };
    let image = Arc::clone(document.base());
    let mut on_done: Option<OnDone> = Some(Box::new(on_done));
    let start = Instant::now();
    let opened = open_with(cx, document, None, |window, cx| {
        let Some(on_done) = on_done.take() else {
            return;
        };
        window.on_window_should_close(cx, |_, _| false);
        window.on_next_frame(move |window, cx| {
            let now = Instant::now();
            let canvas = window.viewport_size();
            Run {
                fitted: Placement::fitted(canvas, &image, window.scale_factor()),
                centre: point(canvas.width / 2.0, canvas.height / 2.0),
                first_frame: now - start,
                last_frame: now,
                intervals: Vec::with_capacity(redraws.get() as usize),
                redraws,
                on_done,
            }
            .continue_after(window, cx);
        });
    });
    if let Err(error) = opened
        && let Some(on_done) = on_done.take()
    {
        on_done(cx, Err(error));
    }
}

struct Run {
    fitted: Placement,
    centre: Point<Pixels>,
    first_frame: Duration,
    last_frame: Instant,
    intervals: Vec<Duration>,
    redraws: NonZeroU32,
    on_done: OnDone,
}

impl Run {
    fn continue_after(self, window: &mut Window, cx: &mut App) {
        if self.intervals.len() == self.redraws.get() as usize {
            window.remove_window();
            let report = self.report();
            (self.on_done)(cx, Ok(report));
            return;
        }
        let frame = u32::try_from(self.intervals.len())
            .expect("the run stops after `redraws` frames, a u32");
        show_at(
            window,
            cx,
            placement(frame, self.redraws, self.fitted, self.centre),
        );
        window.refresh();
        window.on_next_frame(move |window, cx| self.frame_drawn(window, cx));
    }

    fn frame_drawn(mut self, window: &mut Window, cx: &mut App) {
        let now = Instant::now();
        self.intervals.push(now - self.last_frame);
        self.last_frame = now;
        self.continue_after(window, cx);
    }

    fn report(&self) -> BenchReport {
        let mut sorted = self.intervals.clone();
        sorted.sort_unstable();
        let nearest_rank = |percent: usize| sorted[(sorted.len() * percent).div_ceil(100) - 1];
        BenchReport {
            first_frame: self.first_frame,
            p50: nearest_rank(50),
            p95: nearest_rank(95),
            worst: nearest_rank(100),
        }
    }
}

/// Where the image is for forced redraw `frame` of `frames`: fitted for the first third, then
/// panned 4 by 2 screen pixels a frame at 2x, then zoomed from fitted to 4x and back.
#[expect(
    clippy::cast_precision_loss,
    reason = "frame counts stay far below 2^24, where f32 stops being exact"
)]
fn placement(
    frame: u32,
    frames: NonZeroU32,
    fitted: Placement,
    centre: Point<Pixels>,
) -> Option<Placement> {
    let third = frames.get().div_ceil(3);
    match frame / third {
        0 => None,
        1 => {
            let step = (frame - third) as f32;
            Some(
                fitted
                    .zoomed(2.0, centre)
                    .panned(point(px(-4.0 * step), px(-2.0 * step))),
            )
        }
        _ => {
            let progress = (frame - 2 * third) as f32 / third as f32;
            let factor = 1.0 + 3.0 * (1.0 - (2.0 * progress - 1.0).abs());
            Some(fitted.zoomed(factor, centre))
        }
    }
}

fn synthetic_document() -> Result<Document, Error> {
    let mut pixels = Vec::with_capacity(WIDTH as usize * HEIGHT as usize * 4);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.extend([gradient(x, WIDTH), gradient(y, HEIGHT), 128, 255]);
        }
    }
    let base = RgbaImage::new(WIDTH, HEIGHT, pixels).map_err(Error::SyntheticImage)?;
    let mut history = History::new(Document::new(Arc::new(base)));
    for index in 0..SHAPES {
        history
            .apply(synthetic_shape(index))
            .map_err(Error::SyntheticShape)?;
    }
    Ok(history.document().clone())
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "position < extent keeps the value at most 255"
)]
fn gradient(position: u32, extent: u32) -> u8 {
    (position * 255 / (extent - 1)) as u8
}

#[expect(
    clippy::cast_precision_loss,
    reason = "every value is below 4000, where f32 is exact"
)]
fn synthetic_shape(index: u32) -> Command {
    let cell_width = (WIDTH / COLUMNS) as f32;
    let cell_height = (HEIGHT / (SHAPES / COLUMNS)) as f32;
    let left = (index % COLUMNS) as f32 * cell_width + 16.0;
    let top = (index / COLUMNS) as f32 * cell_height + 16.0;
    let (width, height) = (cell_width - 32.0, cell_height - 32.0);
    let rect = RectF {
        x: left,
        y: top,
        width,
        height,
    };
    let from = PointF { x: left, y: top };
    let to = PointF {
        x: left + width,
        y: top + height,
    };
    let (shape, alpha) = match index % 8 {
        0 => (Shape::Arrow { from, to }, 255),
        1 => (Shape::Line { from, to }, 255),
        2 => (
            Shape::Rect {
                rect,
                fill: Fill::Hollow,
            },
            255,
        ),
        3 => (
            Shape::Ellipse {
                rect,
                fill: Fill::Hollow,
            },
            255,
        ),
        4 => (
            Shape::Pen(
                (0..32u8)
                    .map(|step| PointF {
                        x: left + width * f32::from(step) / 31.0,
                        y: top + if step % 2 == 0 { 0.0 } else { height },
                    })
                    .collect(),
            ),
            255,
        ),
        5 => (Shape::Highlight(rect), 96),
        6 => (
            Shape::Ellipse {
                rect,
                fill: Fill::Solid,
            },
            255,
        ),
        _ => (
            Shape::Text(
                TextBlock::new(from, format!("Shape {index}\nof {SHAPES}"), 24.0, 6.0)
                    .expect("24 px and 6 ems make a valid text block"),
            ),
            255,
        ),
    };
    Command::Add {
        shape,
        style: Style {
            color: Color {
                r: 230,
                g: 60,
                b: 50,
                a: alpha,
            },
            stroke_width: 4.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p08_the_bench_holds_still_then_pans_at_2x_then_zooms_to_4x_and_back() {
        let fitted = Placement {
            corner: point(px(10.0), px(20.0)),
            scale: 0.5,
        };
        let centre = point(px(400.0), px(300.0));
        let frames = NonZeroU32::new(300).unwrap();
        let at = |frame| placement(frame, frames, fitted, centre);
        assert_eq!((at(0), at(99)), (None, None));
        let (first, next) = (at(100).unwrap(), at(101).unwrap());
        assert_eq!((first.scale, next.scale), (1.0, 1.0));
        assert_eq!(next.corner - first.corner, point(px(-4.0), px(-2.0)));
        assert_eq!(at(200).unwrap(), fitted);
        assert_eq!(at(250).unwrap().scale, 2.0);
        assert!(at(299).unwrap().scale < 0.6);
    }
}
