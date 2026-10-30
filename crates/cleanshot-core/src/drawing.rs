use crate::document::PointF;

/// `to` moved onto the nearest of the eight directions from `from` that are a multiple of 45
/// degrees, as Shift does for lines and arrows (F-31). A diagonal keeps the mean of the drag's
/// horizontal and vertical extents.
#[must_use]
pub fn snapped_to_45_degrees(from: PointF, to: PointF) -> PointF {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let tan_22_5_degrees = std::f32::consts::SQRT_2 - 1.0;
    if dy.abs() <= dx.abs() * tan_22_5_degrees {
        return PointF { x: to.x, y: from.y };
    }
    if dx.abs() <= dy.abs() * tan_22_5_degrees {
        return PointF { x: from.x, y: to.y };
    }
    let side = f32::midpoint(dx.abs(), dy.abs());
    PointF {
        x: from.x + side.copysign(dx),
        y: from.y + side.copysign(dy),
    }
}

/// `corner` moved so the box from `anchor` to it is square, its side the longer of the drag's
/// two extents, as Shift does for rectangles and ellipses (F-32).
#[must_use]
pub fn squared(anchor: PointF, corner: PointF) -> PointF {
    let (dx, dy) = (corner.x - anchor.x, corner.y - anchor.y);
    let side = dx.abs().max(dy.abs());
    PointF {
        x: anchor.x + side.copysign(dx),
        y: anchor.y + side.copysign(dy),
    }
}

/// The path a pen stroke stores, from the points the pointer passed through (F-33, D-40).
/// Points within a pixel of the last kept one are dropped, then each corner is cut twice; the
/// first and last points stay where they were.
#[must_use]
pub fn smoothed_pen(raw: &[PointF]) -> Vec<PointF> {
    let mut kept: Vec<PointF> = Vec::with_capacity(raw.len());
    for &point in raw {
        let near_last = kept
            .last()
            .is_some_and(|last| (point.x - last.x).hypot(point.y - last.y) < 1.0);
        if !near_last {
            kept.push(point);
        }
    }
    if let (Some(&last_raw), Some(last_kept)) = (raw.last(), kept.last_mut()) {
        *last_kept = last_raw;
    }
    cut_corners(&cut_corners(&kept))
}

/// One round of Chaikin's corner cutting: each segment is replaced by the points a quarter and
/// three quarters along it, keeping the two ends.
fn cut_corners(points: &[PointF]) -> Vec<PointF> {
    let (Some(&first), Some(&last)) = (points.first(), points.last()) else {
        return Vec::new();
    };
    if points.len() < 3 {
        return points.to_vec();
    }
    let along = |a: PointF, b: PointF, share: f32| PointF {
        x: a.x + (b.x - a.x) * share,
        y: a.y + (b.y - a.y) * share,
    };
    let mut cut = vec![first];
    for pair in points.windows(2) {
        cut.push(along(pair[0], pair[1], 0.25));
        cut.push(along(pair[0], pair[1], 0.75));
    }
    cut.push(last);
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f32, y: f32) -> PointF {
        PointF { x, y }
    }

    #[test]
    fn f31_shift_snaps_a_drag_to_the_nearest_multiple_of_45_degrees() {
        let origin = point(10.0, 10.0);
        let snap = |x, y| snapped_to_45_degrees(origin, point(x, y));
        assert_eq!(snap(110.0, 30.0), point(110.0, 10.0));
        assert_eq!(snap(-10.0, -90.0), point(10.0, -90.0));
        assert_eq!(snap(60.0, 70.0), point(65.0, 65.0));
        assert_eq!(snap(-30.0, 60.0), point(-35.0, 55.0));
    }

    #[test]
    fn f31_the_switch_from_straight_to_diagonal_is_at_22_5_degrees() {
        let origin = point(0.0, 0.0);
        let (below, above) = (22.4_f32.to_radians(), 22.6_f32.to_radians());
        let at = |angle: f32| point(100.0 * angle.cos(), 100.0 * angle.sin());
        assert_eq!(snapped_to_45_degrees(origin, at(below)).y, 0.0);
        let diagonal = snapped_to_45_degrees(origin, at(above));
        assert_eq!(diagonal.x, diagonal.y);
    }

    #[test]
    fn f32_shift_makes_a_box_square_on_its_longer_side_toward_the_drag() {
        let anchor = point(50.0, 50.0);
        assert_eq!(squared(anchor, point(80.0, 60.0)), point(80.0, 80.0));
        assert_eq!(squared(anchor, point(40.0, 0.0)), point(0.0, 0.0));
        assert_eq!(squared(anchor, point(70.0, 25.0)), point(75.0, 25.0));
    }

    #[test]
    fn f33_smoothing_keeps_the_stroke_end_points() {
        let raw = [
            point(0.0, 0.0),
            point(10.0, 0.0),
            point(10.0, 10.0),
            point(20.0, 10.0),
        ];
        let smooth = smoothed_pen(&raw);
        assert_eq!(smooth.first(), Some(&raw[0]));
        assert_eq!(smooth.last(), Some(&raw[3]));
    }

    #[test]
    fn f33_smoothing_cuts_every_corner_twice() {
        let smooth = smoothed_pen(&[point(0.0, 0.0), point(16.0, 0.0), point(16.0, 16.0)]);
        assert!(!smooth.contains(&point(16.0, 0.0)));
        assert_eq!(smooth.len(), 12);
        assert!(smooth.contains(&point(13.0, 1.0)) && smooth.contains(&point(15.0, 3.0)));
    }

    #[test]
    fn f33_points_within_a_pixel_of_the_last_kept_one_are_dropped() {
        let raw = [
            point(0.0, 0.0),
            point(0.5, 0.0),
            point(0.9, 0.3),
            point(10.0, 0.0),
            point(10.4, 0.0),
        ];
        assert_eq!(smoothed_pen(&raw), [point(0.0, 0.0), point(10.4, 0.0)]);
    }

    #[test]
    fn f33_a_stroke_of_one_point_stays_one_point() {
        assert_eq!(smoothed_pen(&[point(3.0, 4.0)]), [point(3.0, 4.0)]);
        assert_eq!(smoothed_pen(&[]), []);
    }
}
