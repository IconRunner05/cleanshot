/// A point on the desktop, in physical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// A rectangle on the desktop, in physical pixels: `left` and `top` are inside it, `right`
/// and `bottom` just outside.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    /// Whether `point` is inside, its right and bottom edges excluded.
    #[must_use]
    pub fn holds(self, point: Point) -> bool {
        (self.left..self.right).contains(&point.x) && (self.top..self.bottom).contains(&point.y)
    }

    /// The part `other` shares with this one, or None when they share no pixel.
    #[must_use]
    pub fn overlap(self, other: Self) -> Option<Self> {
        let shared = Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        (shared.left < shared.right && shared.top < shared.bottom).then_some(shared)
    }

    /// The smallest rectangle holding every one of `rectangles`, such as the desktop around its
    /// monitors; None for none.
    #[must_use]
    pub fn bounding(rectangles: impl IntoIterator<Item = Self>) -> Option<Self> {
        rectangles.into_iter().reduce(|all, next| Self {
            left: all.left.min(next.left),
            top: all.top.min(next.top),
            right: all.right.max(next.right),
            bottom: all.bottom.max(next.bottom),
        })
    }
}

/// The first of `rectangles` holding `point`.
#[must_use]
pub fn first_holding(rectangles: impl IntoIterator<Item = Rect>, point: Point) -> Option<Rect> {
    rectangles.into_iter().find(|bounds| bounds.holds(point))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn f01_a_rectangle_holds_its_left_and_top_edges_but_not_its_right_and_bottom() {
        let bounds = rect(-1920, 0, 0, 1080);
        let at = |x, y| bounds.holds(Point { x, y });
        assert!(at(-1920, 0));
        assert!(at(-1, 1079));
        assert!(!at(0, 0));
        assert!(!at(-1, 1080));
        assert!(!at(-1921, 0));
    }

    #[test]
    fn f08_a_frame_is_cut_to_the_desktop_and_a_frame_off_it_is_dropped() {
        let desktop = rect(-1920, 0, 1920, 1080);
        assert_eq!(
            rect(1800, 900, 2100, 1200).overlap(desktop),
            Some(rect(1800, 900, 1920, 1080))
        );
        assert_eq!(
            rect(-2000, -10, -1900, 10).overlap(desktop),
            Some(rect(-1920, 0, -1900, 10))
        );
        assert_eq!(rect(1920, 0, 2000, 100).overlap(desktop), None);
    }

    #[test]
    fn f08_the_desktop_bounds_every_monitor_and_no_monitor_bounds_nothing() {
        assert_eq!(
            Rect::bounding([rect(0, 0, 1920, 1080), rect(-1280, -200, 0, 824)]),
            Some(rect(-1280, -200, 1920, 1080))
        );
        assert_eq!(Rect::bounding([]), None);
    }

    #[test]
    fn f07_the_first_rectangle_holding_the_pointer_is_found() {
        let rectangles = [
            rect(-400, 0, 0, 300),
            rect(0, 0, 400, 300),
            rect(0, 0, 800, 600),
        ];
        assert_eq!(
            first_holding(rectangles, Point { x: 0, y: 10 }),
            Some(rectangles[1])
        );
        assert_eq!(first_holding(rectangles, Point { x: 800, y: 10 }), None);
    }
}
