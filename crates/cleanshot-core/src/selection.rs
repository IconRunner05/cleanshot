use crate::document::{Document, Fill, PointF, RectF, Shape, ShapeId};

/// One of the eight points on a box's outline that resize it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

/// A point on a shape that a drag resizes it by: a box handle for shapes with an area or a
/// pen stroke's bounds, an end point for lines and arrows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Box(BoxHandle),
    Start,
    End,
}

/// The topmost shape within `tolerance` pixels of `point`, beyond half its stroke width (F-30).
#[must_use]
pub fn shape_at(document: &Document, point: PointF, tolerance: f32) -> Option<ShapeId> {
    document
        .shapes()
        .iter()
        .rev()
        .find(|entry| {
            let reach = tolerance + entry.style.stroke_width / 2.0;
            hits(&entry.shape, point, reach)
        })
        .map(|entry| entry.id)
}

/// The handles of `shape` and where each sits.
#[must_use]
pub fn handles(shape: &Shape) -> Vec<(Handle, PointF)> {
    match shape {
        Shape::Arrow { from, to } | Shape::Line { from, to } => {
            vec![(Handle::Start, *from), (Handle::End, *to)]
        }
        Shape::Rect { .. }
        | Shape::Ellipse { .. }
        | Shape::Pen(_)
        | Shape::Highlight { .. }
        | Shape::Redact { .. }
        | Shape::Spotlight(_) => bounds(shape).map_or_else(Vec::new, |rect| {
            box_handles(rect)
                .into_iter()
                .map(|(handle, at)| (Handle::Box(handle), at))
                .collect()
        }),
        // F-34, F-41: text and steps resize by their font size, on the property strip.
        Shape::Text(_) | Shape::Step(_) => Vec::new(),
        // D-119, D-128: the box's sides and corners set its width and height.
        Shape::Bubble(bubble) => box_handles(bubble.outline())
            .into_iter()
            .map(|(handle, at)| (Handle::Box(handle), at))
            .chain([(Handle::End, bubble.tail())])
            .collect(),
    }
}

/// The handle of `shape` nearest `point`, if one is within `tolerance` pixels.
#[must_use]
pub fn handle_at(shape: &Shape, point: PointF, tolerance: f32) -> Option<Handle> {
    nearest(handles(shape), point, tolerance)
}

/// The eight handles of `rect` and where each sits.
#[must_use]
pub fn box_handles(rect: RectF) -> [(BoxHandle, PointF); 8] {
    BOX_HANDLES.map(|handle| (handle, box_handle_point(rect, handle)))
}

/// The handle of `rect` nearest `point`, if one is within `tolerance` pixels.
#[must_use]
pub fn box_handle_at(rect: RectF, point: PointF, tolerance: f32) -> Option<BoxHandle> {
    nearest(box_handles(rect), point, tolerance)
}

/// `rect` with `handle` dragged to `to`. Dragged past its opposite side, the box flips rather than
/// getting a negative size.
#[must_use]
pub fn resized_box(rect: RectF, handle: BoxHandle, to: PointF) -> RectF {
    let (mut left, mut top) = (rect.x, rect.y);
    let (mut right, mut bottom) = (rect.x + rect.width, rect.y + rect.height);
    match handle {
        BoxHandle::TopLeft => (left, top) = (to.x, to.y),
        BoxHandle::Top => top = to.y,
        BoxHandle::TopRight => (right, top) = (to.x, to.y),
        BoxHandle::Right => right = to.x,
        BoxHandle::BottomRight => (right, bottom) = (to.x, to.y),
        BoxHandle::Bottom => bottom = to.y,
        BoxHandle::BottomLeft => (left, bottom) = (to.x, to.y),
        BoxHandle::Left => left = to.x,
    }
    box_between(
        PointF { x: left, y: top },
        PointF {
            x: right,
            y: bottom,
        },
    )
}

/// `shape` moved by `offset`.
#[must_use]
pub fn moved(shape: &Shape, offset: PointF) -> Shape {
    let shift = |point: PointF| PointF {
        x: point.x + offset.x,
        y: point.y + offset.y,
    };
    let shift_rect = |rect: RectF| RectF {
        x: rect.x + offset.x,
        y: rect.y + offset.y,
        ..rect
    };
    match shape {
        Shape::Arrow { from, to } => Shape::Arrow {
            from: shift(*from),
            to: shift(*to),
        },
        Shape::Line { from, to } => Shape::Line {
            from: shift(*from),
            to: shift(*to),
        },
        Shape::Rect { rect, fill } => Shape::Rect {
            rect: shift_rect(*rect),
            fill: *fill,
        },
        Shape::Ellipse { rect, fill } => Shape::Ellipse {
            rect: shift_rect(*rect),
            fill: *fill,
        },
        Shape::Pen(points) => Shape::Pen(points.iter().copied().map(shift).collect()),
        Shape::Text(block) => Shape::Text(block.moved_to(shift(block.at()))),
        Shape::Step(step) => Shape::Step(step.moved_to(shift(step.centre()))),
        Shape::Bubble(bubble) => Shape::Bubble(bubble.moved_by(offset)),
        Shape::Highlight { rect, strength } => Shape::Highlight {
            rect: shift_rect(*rect),
            strength: *strength,
        },
        Shape::Spotlight(rect) => Shape::Spotlight(shift_rect(*rect)),
        Shape::Redact {
            rect,
            mode,
            strength,
        } => Shape::Redact {
            rect: shift_rect(*rect),
            mode: *mode,
            strength: *strength,
        },
    }
}

/// `shape` with `handle` dragged to `to`. A box dragged past its opposite side flips rather
/// than turning inside out. A handle the shape does not have leaves it unchanged. `measure`
/// gives a line's width in ems, for a bubble whose text wraps in its box (D-119).
#[must_use]
pub fn resized(shape: &Shape, handle: Handle, to: PointF, measure: impl Fn(&str) -> f32) -> Shape {
    let new_bounds = |handle| bounds(shape).map(|rect| resized_box(rect, handle, to));
    match (shape, handle) {
        (Shape::Arrow { to: end, .. }, Handle::Start) => Shape::Arrow { from: to, to: *end },
        (Shape::Arrow { from, .. }, Handle::End) => Shape::Arrow { from: *from, to },
        (Shape::Line { to: end, .. }, Handle::Start) => Shape::Line { from: to, to: *end },
        (Shape::Line { from, .. }, Handle::End) => Shape::Line { from: *from, to },
        (Shape::Bubble(bubble), Handle::End) => Shape::Bubble(bubble.with_tail(to)),
        (Shape::Bubble(bubble), Handle::Box(handle)) => {
            Shape::Bubble(bubble.resized(handle, to, measure))
        }
        (Shape::Rect { fill, .. }, Handle::Box(handle)) => match new_bounds(handle) {
            Some(rect) => Shape::Rect { rect, fill: *fill },
            None => shape.clone(),
        },
        (Shape::Ellipse { fill, .. }, Handle::Box(handle)) => match new_bounds(handle) {
            Some(rect) => Shape::Ellipse { rect, fill: *fill },
            None => shape.clone(),
        },
        (Shape::Highlight { strength, .. }, Handle::Box(handle)) => new_bounds(handle).map_or_else(
            || shape.clone(),
            |rect| Shape::Highlight {
                rect,
                strength: *strength,
            },
        ),
        (Shape::Spotlight(_), Handle::Box(handle)) => {
            new_bounds(handle).map_or_else(|| shape.clone(), Shape::Spotlight)
        }
        (Shape::Redact { mode, strength, .. }, Handle::Box(handle)) => match new_bounds(handle) {
            Some(rect) => Shape::Redact {
                rect,
                mode: *mode,
                strength: *strength,
            },
            None => shape.clone(),
        },
        (Shape::Pen(points), Handle::Box(handle)) => match (bounds(shape), new_bounds(handle)) {
            (Some(old), Some(new)) => Shape::Pen(
                points
                    .iter()
                    .map(|&point| rescale(point, old, new))
                    .collect(),
            ),
            _ => shape.clone(),
        },
        (Shape::Arrow { .. } | Shape::Line { .. }, Handle::Box(_))
        | (
            Shape::Rect { .. }
            | Shape::Ellipse { .. }
            | Shape::Pen(_)
            | Shape::Highlight { .. }
            | Shape::Redact { .. }
            | Shape::Spotlight(_),
            Handle::Start | Handle::End,
        )
        | (Shape::Text(_) | Shape::Step(_), Handle::Box(_) | Handle::Start | Handle::End)
        | (Shape::Bubble(_), Handle::Start) => shape.clone(),
    }
}

const BOX_HANDLES: [BoxHandle; 8] = [
    BoxHandle::TopLeft,
    BoxHandle::Top,
    BoxHandle::TopRight,
    BoxHandle::Right,
    BoxHandle::BottomRight,
    BoxHandle::Bottom,
    BoxHandle::BottomLeft,
    BoxHandle::Left,
];

/// Whether a press at `point` lands on `shape`, within `reach` image pixels of its outline or inside
/// it, as [`shape_at`] tests every shape.
#[must_use]
pub fn hits(shape: &Shape, point: PointF, reach: f32) -> bool {
    match shape {
        Shape::Arrow { from, to } | Shape::Line { from, to } => {
            segment_distance(*from, *to, point) <= reach
        }
        Shape::Pen(points) => match points.as_slice() {
            [only] => distance(*only, point) <= reach,
            points => points
                .windows(2)
                .any(|pair| segment_distance(pair[0], pair[1], point) <= reach),
        },
        Shape::Rect {
            rect,
            fill: Fill::Hollow,
        }
        | Shape::Spotlight(rect) => {
            inside(grown(*rect, reach), point) && !inside(grown(*rect, -reach), point)
        }
        Shape::Rect {
            rect,
            fill: Fill::Solid,
        }
        | Shape::Highlight { rect, .. }
        | Shape::Redact { rect, .. } => inside(grown(*rect, reach), point),
        Shape::Ellipse {
            rect,
            fill: Fill::Hollow,
        } => {
            inside_ellipse(grown(*rect, reach), point)
                && !inside_ellipse(grown(*rect, -reach), point)
        }
        Shape::Ellipse {
            rect,
            fill: Fill::Solid,
        } => inside_ellipse(grown(*rect, reach), point),
        Shape::Text(block) => inside(grown(block.extent(), reach), point),
        Shape::Step(step) => inside_ellipse(grown(step.circle(), reach), point),
        Shape::Bubble(bubble) => {
            inside(grown(bubble.outline(), reach), point)
                || bubble.tail_triangle().is_some_and(|[left, tip, right]| {
                    segment_distance(left, tip, point) <= reach
                        || segment_distance(tip, right, point) <= reach
                        || inside_triangle([left, tip, right], point)
                })
        }
    }
}

/// The smallest box around a shape's area or points; `None` for shapes resized by end points.
fn bounds(shape: &Shape) -> Option<RectF> {
    match shape {
        Shape::Rect { rect, .. }
        | Shape::Ellipse { rect, .. }
        | Shape::Highlight { rect, .. }
        | Shape::Redact { rect, .. }
        | Shape::Spotlight(rect) => Some(*rect),
        Shape::Pen(points) => {
            let (first, rest) = points.split_first()?;
            let (mut low, mut high) = (*first, *first);
            for point in rest {
                low = PointF {
                    x: low.x.min(point.x),
                    y: low.y.min(point.y),
                };
                high = PointF {
                    x: high.x.max(point.x),
                    y: high.y.max(point.y),
                };
            }
            Some(box_between(low, high))
        }
        Shape::Text(block) => Some(block.extent()),
        Shape::Step(step) => Some(step.circle()),
        Shape::Bubble(bubble) => Some(bubble.outline()),
        Shape::Arrow { .. } | Shape::Line { .. } => None,
    }
}

fn box_handle_point(rect: RectF, handle: BoxHandle) -> PointF {
    let (left, top) = (rect.x, rect.y);
    let (right, bottom) = (rect.x + rect.width, rect.y + rect.height);
    let (middle_x, middle_y) = (left + rect.width / 2.0, top + rect.height / 2.0);
    let (x, y) = match handle {
        BoxHandle::TopLeft => (left, top),
        BoxHandle::Top => (middle_x, top),
        BoxHandle::TopRight => (right, top),
        BoxHandle::Right => (right, middle_y),
        BoxHandle::BottomRight => (right, bottom),
        BoxHandle::Bottom => (middle_x, bottom),
        BoxHandle::BottomLeft => (left, bottom),
        BoxHandle::Left => (left, middle_y),
    };
    PointF { x, y }
}

fn nearest<H>(
    handles: impl IntoIterator<Item = (H, PointF)>,
    point: PointF,
    tolerance: f32,
) -> Option<H> {
    handles
        .into_iter()
        .map(|(handle, at)| (handle, distance(at, point)))
        .filter(|&(_, gap)| gap <= tolerance)
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(handle, _)| handle)
}

pub(crate) fn box_between(a: PointF, b: PointF) -> RectF {
    RectF {
        x: a.x.min(b.x),
        y: a.y.min(b.y),
        width: (a.x - b.x).abs(),
        height: (a.y - b.y).abs(),
    }
}

/// `point` moved from its place in `old` to the same place in `new`. A side of zero length
/// keeps its coordinate and moves with the box.
fn rescale(point: PointF, old: RectF, new: RectF) -> PointF {
    let axis = |value: f32, old_start: f32, old_length: f32, new_start: f32, new_length: f32| {
        if old_length == 0.0 {
            new_start
        } else {
            new_start + (value - old_start) / old_length * new_length
        }
    };
    PointF {
        x: axis(point.x, old.x, old.width, new.x, new.width),
        y: axis(point.y, old.y, old.height, new.y, new.height),
    }
}

fn grown(rect: RectF, by: f32) -> RectF {
    RectF {
        x: rect.x - by,
        y: rect.y - by,
        width: rect.width + 2.0 * by,
        height: rect.height + 2.0 * by,
    }
}

pub(crate) fn inside(rect: RectF, point: PointF) -> bool {
    rect.width >= 0.0
        && rect.height >= 0.0
        && (rect.x..=rect.x + rect.width).contains(&point.x)
        && (rect.y..=rect.y + rect.height).contains(&point.y)
}

fn inside_ellipse(rect: RectF, point: PointF) -> bool {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return false;
    }
    let (radius_x, radius_y) = (rect.width / 2.0, rect.height / 2.0);
    let dx = (point.x - rect.x - radius_x) / radius_x;
    let dy = (point.y - rect.y - radius_y) / radius_y;
    dx * dx + dy * dy <= 1.0
}

fn inside_triangle([a, b, c]: [PointF; 3], point: PointF) -> bool {
    let side = |from: PointF, to: PointF| {
        (to.x - from.x) * (point.y - from.y) - (to.y - from.y) * (point.x - from.x)
    };
    let sides = [side(a, b), side(b, c), side(c, a)];
    sides.iter().all(|&side| side >= 0.0) || sides.iter().all(|&side| side <= 0.0)
}

fn distance(a: PointF, b: PointF) -> f32 {
    (a.x - b.x).hypot(a.y - b.y)
}

fn segment_distance(from: PointF, to: PointF, point: PointF) -> f32 {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length_squared = dx * dx + dy * dy;
    if length_squared == 0.0 {
        return distance(from, point);
    }
    let along =
        (((point.x - from.x) * dx + (point.y - from.y) * dy) / length_squared).clamp(0.0, 1.0);
    distance(
        PointF {
            x: from.x + along * dx,
            y: from.y + along * dy,
        },
        point,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::document::{Color, HighlightStrength, RgbaImage, Style};
    use crate::history::{Command, History};

    fn point(x: f32, y: f32) -> PointF {
        PointF { x, y }
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> RectF {
        RectF {
            x,
            y,
            width,
            height,
        }
    }

    fn document(shapes: Vec<Shape>) -> (History, Vec<ShapeId>) {
        let base = RgbaImage::new(1, 1, vec![0; 4]).unwrap();
        let mut history = History::new(Document::new(Arc::new(base)));
        for shape in shapes {
            history
                .apply(Command::Add {
                    shape,
                    style: Style::new(
                        Color {
                            r: 255,
                            g: 0,
                            b: 0,
                            a: 255,
                        },
                        2.0,
                    ),
                })
                .unwrap();
        }
        let ids = history
            .document()
            .shapes()
            .iter()
            .map(|entry| entry.id)
            .collect();
        (history, ids)
    }

    #[test]
    fn f30_a_click_selects_the_topmost_shape_under_the_pointer() {
        let solid = |rect| Shape::Rect {
            rect,
            fill: Fill::Solid,
        };
        let (history, ids) = document(vec![
            solid(rect(0.0, 0.0, 50.0, 50.0)),
            solid(rect(25.0, 25.0, 50.0, 50.0)),
        ]);
        let document = history.document();

        assert_eq!(shape_at(document, point(30.0, 30.0), 0.0), Some(ids[1]));
        assert_eq!(shape_at(document, point(10.0, 10.0), 0.0), Some(ids[0]));
        assert_eq!(shape_at(document, point(90.0, 90.0), 0.0), None);
    }

    #[test]
    fn f30_a_line_is_hit_within_the_tolerance_beyond_half_its_stroke() {
        let (history, ids) = document(vec![Shape::Line {
            from: point(0.0, 0.0),
            to: point(100.0, 0.0),
        }]);
        let document = history.document();

        assert_eq!(shape_at(document, point(50.0, 4.0), 3.0), Some(ids[0]));
        assert_eq!(shape_at(document, point(50.0, 4.5), 3.0), None);
        assert_eq!(shape_at(document, point(104.5, 0.0), 3.0), None);
    }

    #[test]
    fn f30_a_hollow_shape_is_hit_on_its_outline_and_not_inside() {
        for shape in [
            Shape::Rect {
                rect: rect(0.0, 0.0, 100.0, 60.0),
                fill: Fill::Hollow,
            },
            Shape::Ellipse {
                rect: rect(0.0, 0.0, 100.0, 60.0),
                fill: Fill::Hollow,
            },
        ] {
            let (history, ids) = document(vec![shape]);
            let document = history.document();
            assert_eq!(shape_at(document, point(100.0, 30.0), 2.0), Some(ids[0]));
            assert_eq!(shape_at(document, point(50.0, 30.0), 2.0), None);
        }
    }

    #[test]
    fn f30_dragging_a_corner_past_the_opposite_one_flips_the_box() {
        let shape = Shape::Rect {
            rect: rect(10.0, 10.0, 20.0, 20.0),
            fill: Fill::Hollow,
        };
        let resized = resized(
            &shape,
            Handle::Box(BoxHandle::TopLeft),
            point(40.0, 50.0),
            |_| 0.0,
        );
        assert_eq!(
            resized,
            Shape::Rect {
                rect: rect(30.0, 30.0, 10.0, 20.0),
                fill: Fill::Hollow,
            }
        );
    }

    #[test]
    fn f30_an_edge_handle_moves_only_its_side() {
        let shape = Shape::Highlight {
            rect: rect(10.0, 10.0, 20.0, 20.0),
            strength: HighlightStrength::Strong,
        };
        let resized = resized(
            &shape,
            Handle::Box(BoxHandle::Right),
            point(50.0, 99.0),
            |_| 0.0,
        );
        assert_eq!(
            resized,
            Shape::Highlight {
                rect: rect(10.0, 10.0, 40.0, 20.0),
                strength: HighlightStrength::Strong
            }
        );
    }

    #[test]
    fn f30_a_pen_stroke_resizes_by_scaling_its_points_into_the_new_box() {
        let shape = Shape::Pen(vec![point(0.0, 0.0), point(10.0, 5.0), point(20.0, 10.0)]);
        let resized = resized(
            &shape,
            Handle::Box(BoxHandle::BottomRight),
            point(40.0, 30.0),
            |_| 0.0,
        );
        assert_eq!(
            resized,
            Shape::Pen(vec![point(0.0, 0.0), point(20.0, 15.0), point(40.0, 30.0)])
        );
    }

    #[test]
    fn f30_a_line_resizes_by_its_end_points() {
        let shape = Shape::Arrow {
            from: point(0.0, 0.0),
            to: point(10.0, 0.0),
        };
        assert_eq!(handle_at(&shape, point(11.0, 1.0), 2.0), Some(Handle::End));
        assert_eq!(handle_at(&shape, point(5.0, 3.0), 2.0), None);
        assert_eq!(
            resized(&shape, Handle::End, point(5.0, 20.0), |_| 0.0),
            Shape::Arrow {
                from: point(0.0, 0.0),
                to: point(5.0, 20.0),
            }
        );
    }

    #[test]
    fn f30_a_move_shifts_every_point_by_the_offset() {
        let shape = Shape::Pen(vec![point(0.0, 0.0), point(10.0, 5.0)]);
        assert_eq!(
            moved(&shape, point(3.0, -2.0)),
            Shape::Pen(vec![point(3.0, -2.0), point(13.0, 3.0)])
        );
    }

    fn text(at: PointF) -> Shape {
        Shape::Text(crate::TextBlock::new(at, "two\nlines".into(), 20.0, 3.0).unwrap())
    }

    #[test]
    fn f34_a_text_shape_is_hit_anywhere_in_its_measured_lines() {
        let (history, ids) = document(vec![text(point(10.0, 10.0))]);
        let document = history.document();

        assert_eq!(shape_at(document, point(69.0, 59.0), 0.0), Some(ids[0]));
        assert_eq!(shape_at(document, point(72.0, 30.0), 0.0), None);
        assert_eq!(shape_at(document, point(30.0, 62.0), 0.0), None);
        assert_eq!(shape_at(document, point(73.0, 30.0), 2.0), Some(ids[0]));
    }

    #[test]
    fn f34_a_move_shifts_a_text_shape_and_keeps_its_text_and_size() {
        assert_eq!(
            moved(&text(point(10.0, 10.0)), point(3.0, -2.0)),
            text(point(13.0, 8.0))
        );
    }

    #[test]
    fn f30_box_handles_sit_on_the_corners_and_the_middle_of_each_side() {
        let handles = box_handles(rect(10.0, 20.0, 100.0, 50.0));
        assert_eq!(
            handles.map(|(_, at)| (at.x, at.y)),
            [
                (10.0, 20.0),
                (60.0, 20.0),
                (110.0, 20.0),
                (110.0, 45.0),
                (110.0, 70.0),
                (60.0, 70.0),
                (10.0, 70.0),
                (10.0, 45.0),
            ]
        );
    }

    #[test]
    fn f30_the_nearest_box_handle_within_the_tolerance_is_gripped() {
        let small = rect(0.0, 0.0, 4.0, 4.0);
        assert_eq!(
            box_handle_at(small, point(3.5, -1.0), 5.0),
            Some(BoxHandle::TopRight)
        );
        assert_eq!(
            box_handle_at(small, point(2.2, -1.0), 5.0),
            Some(BoxHandle::Top)
        );
        assert_eq!(box_handle_at(small, point(4.0, -6.0), 5.0), None);
    }

    #[test]
    fn f30_a_box_dragged_past_its_opposite_corner_flips() {
        let dragged = resized_box(
            rect(0.0, 0.0, 100.0, 50.0),
            BoxHandle::BottomRight,
            point(-20.0, -10.0),
        );
        assert_eq!(dragged, rect(-20.0, -10.0, 20.0, 10.0));
    }

    #[test]
    fn f43_a_spotlight_is_gripped_by_its_edge_and_not_inside() {
        let spotlight = Shape::Spotlight(rect(0.0, 0.0, 100.0, 50.0));
        assert!(hits(&spotlight, point(100.0, 25.0), 3.0));
        assert!(!hits(&spotlight, point(50.0, 25.0), 3.0));
    }
}
