use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cleanshot_core::{
    Color, Command, Document, Fill, History, PointF, RectF, RedactMode, RgbaImage, Shape, Style,
    TextBlock,
};
use gpui::{App, Pixels, Point, Window, point, px};

use crate::Error;
use crate::editor_window::{drag_redaction, open_with, show_at};
use crate::placement::Placement;

const WIDTH: u32 = 3840;
const HEIGHT: u32 = 2160;
const SHAPES: u32 = 200;
const COLUMNS: u32 = 20;
/// Where the bench's redaction drag is pressed, in image pixels.
const DRAG_FROM: PointF = PointF { x: 64.0, y: 64.0 };

/// Editor timings from one [`bench()`] run.
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
    /// F-36, D-58: median frame interval while a redaction is dragged from near the top-left
    /// corner to near the bottom-right one, its preview remade every step. Reported apart from
    /// `p50`, which it is not part of.
    pub redaction_drag_p50: Duration,
    /// F-36, D-58: 95th-percentile interval of the same drag.
    pub redaction_drag_p95: Duration,
}

type OnDone = Box<dyn FnOnce(&mut App, Result<BenchReport, Error>)>;

/// Opens the editor on a synthetic 4K image with 200 shapes, forces `redraws` redraws while
/// panning and zooming as F-39 does, then drags a redaction for a third as many frames, closes
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
    let kept = document.kept_area();
    let mut on_done: Option<OnDone> = Some(Box::new(on_done));
    let start = Instant::now();
    // The bench runs from cleanshot-fieldtest, a console tool whose stderr is read.
    let log = Rc::new(|event| eprintln!("{event}"));
    let opened = open_with(cx, document, None, None, log, |window, cx| {
        let Some(on_done) = on_done.take() else {
            return;
        };
        window.on_window_should_close(cx, |_, _| false);
        window.on_next_frame(move |window, cx| {
            let now = Instant::now();
            let canvas = window.viewport_size();
            Run {
                fitted: Placement::fitted(canvas, kept, window.scale_factor()),
                centre: point(canvas.width / 2.0, canvas.height / 2.0),
                first_frame: now - start,
                last_frame: now,
                intervals: Vec::with_capacity(redraws.get() as usize),
                drag_intervals: Vec::new(),
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
    drag_intervals: Vec<Duration>,
    redraws: NonZeroU32,
    on_done: OnDone,
}

impl Run {
    fn continue_after(self, window: &mut Window, cx: &mut App) {
        let drag_steps = self.redraws.get().div_ceil(3);
        if self.drag_intervals.len() == drag_steps as usize {
            window.remove_window();
            let report = self.report();
            (self.on_done)(cx, Ok(report));
            return;
        }
        if self.redrawing() {
            let frame = u32::try_from(self.intervals.len())
                .expect("the redraws stop after `redraws` frames, a u32");
            show_at(
                window,
                cx,
                placement(frame, self.redraws, self.fitted, self.centre),
            );
        } else {
            let step = u32::try_from(self.drag_intervals.len())
                .expect("the drag stops after a third of `redraws` steps, a u32");
            if step == 0 {
                show_at(window, cx, None);
            }
            let (from, to) = (DRAG_FROM, drag_corner(step + 1, drag_steps));
            drag_redaction(window, cx, from, to, self.fitted);
        }
        window.refresh();
        window.on_next_frame(move |window, cx| self.frame_drawn(window, cx));
    }

    fn frame_drawn(mut self, window: &mut Window, cx: &mut App) {
        let now = Instant::now();
        let interval = now - self.last_frame;
        if self.redrawing() {
            self.intervals.push(interval);
        } else {
            self.drag_intervals.push(interval);
        }
        self.last_frame = now;
        self.continue_after(window, cx);
    }

    fn redrawing(&self) -> bool {
        self.intervals.len() < self.redraws.get() as usize
    }

    fn report(&self) -> BenchReport {
        BenchReport {
            first_frame: self.first_frame,
            p50: nearest_rank(&self.intervals, 50),
            p95: nearest_rank(&self.intervals, 95),
            worst: nearest_rank(&self.intervals, 100),
            redaction_drag_p50: nearest_rank(&self.drag_intervals, 50),
            redaction_drag_p95: nearest_rank(&self.drag_intervals, 95),
        }
    }
}

/// The interval `percent` of `intervals` are no longer than (nearest rank); `intervals` is not
/// empty.
fn nearest_rank(intervals: &[Duration], percent: usize) -> Duration {
    let mut sorted = intervals.to_vec();
    sorted.sort_unstable();
    sorted[(sorted.len() * percent).div_ceil(100) - 1]
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

/// Where the dragged redaction's far corner is at drag step `step` of `steps`: on a line from
/// [`DRAG_FROM`] to as far from the bottom-right corner, reached at the last step.
#[expect(
    clippy::cast_precision_loss,
    reason = "step counts and image sides stay far below 2^24, where f32 stops being exact"
)]
fn drag_corner(step: u32, steps: u32) -> PointF {
    let progress = step as f32 / steps as f32;
    PointF {
        x: DRAG_FROM.x + (WIDTH as f32 - 2.0 * DRAG_FROM.x) * progress,
        y: DRAG_FROM.y + (HEIGHT as f32 - 2.0 * DRAG_FROM.y) * progress,
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
    let (shape, alpha) = match index % 9 {
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
        7 => (
            Shape::Text(
                TextBlock::new(from, format!("Shape {index}\nof {SHAPES}"), 24.0, 6.0)
                    .expect("24 px and 6 ems make a valid text block"),
            ),
            255,
        ),
        _ => (
            Shape::Redact {
                rect,
                mode: if index.is_multiple_of(2) {
                    RedactMode::Pixelate
                } else {
                    RedactMode::Blur
                },
            },
            255,
        ),
    };
    Command::Add {
        shape,
        style: Style::new(
            Color {
                r: 230,
                g: 60,
                b: 50,
                a: alpha,
            },
            4.0,
        ),
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

    #[test]
    fn d58_the_bench_drags_a_redaction_from_corner_to_corner_of_the_4k_image() {
        assert_eq!(drag_corner(0, 50), DRAG_FROM);
        assert_eq!(
            drag_corner(50, 50),
            PointF {
                x: 3776.0,
                y: 2096.0
            }
        );
        let halfway = drag_corner(25, 50);
        assert_eq!((halfway.x, halfway.y), (1920.0, 1080.0));
    }

    #[test]
    fn the_bench_reports_each_phase_by_nearest_rank() {
        let intervals: Vec<_> = (1..=20).map(Duration::from_millis).collect();
        assert_eq!(nearest_rank(&intervals, 50), Duration::from_millis(10));
        assert_eq!(nearest_rank(&intervals, 95), Duration::from_millis(19));
        assert_eq!(nearest_rank(&intervals, 100), Duration::from_millis(20));
    }
}
