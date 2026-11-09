use crate::screen::{Point, Rect, first_holding};
use crate::toolbar::TabDirection;

/// A window a window capture can pick: its frame, DWM's extended frame bounds without the
/// shadow, and its title for a screen reader to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pickable {
    pub frame: Rect,
    pub title: String,
}

/// What a window capture picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The window at this place in the list of windows that can be picked, topmost first.
    Window(usize),
    /// A monitor, picked where no window is.
    Monitor(Rect),
}

/// What a window capture picks at `point`: the topmost of `windows` holding it,
/// or else the monitor holding it.
#[must_use]
pub fn picked(windows: &[Pickable], monitors: &[Rect], point: Point) -> Option<Pick> {
    windows
        .iter()
        .position(|window| window.frame.holds(point))
        .map(Pick::Window)
        .or_else(|| first_holding(monitors.iter().copied(), point).map(Pick::Monitor))
}

/// The window Tab moves to from `hovered` in a list of `count`, wrapping at either end; from
/// a monitor or none, the first forwards and the last backwards. None when there is no window.
#[must_use]
pub fn next_window(count: usize, hovered: Option<Pick>, direction: TabDirection) -> Option<Pick> {
    let last = count.checked_sub(1)?;
    let next = match (hovered, direction) {
        (Some(Pick::Window(at)), TabDirection::Forward) => (at + 1) % count,
        (Some(Pick::Window(at)), TabDirection::Back) => (at + last) % count,
        (Some(Pick::Monitor(_)) | None, TabDirection::Forward) => 0,
        (Some(Pick::Monitor(_)) | None, TabDirection::Back) => last,
    };
    Some(Pick::Window(next))
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
    fn the_topmost_window_holding_the_pointer_is_picked_and_its_monitor_where_none_is() {
        let windows = [
            rect(10, 10, 50, 50),
            rect(0, 0, 100, 100),
            rect(200, 0, 300, 50),
        ]
        .map(|frame| Pickable {
            frame,
            title: String::new(),
        });
        let monitors = [rect(-400, 0, 0, 300), rect(0, 0, 400, 300)];
        let at = |x, y| picked(&windows, &monitors, Point { x, y });
        assert_eq!(at(20, 20), Some(Pick::Window(0)));
        assert_eq!(at(5, 5), Some(Pick::Window(1)));
        assert_eq!(at(50, 50), Some(Pick::Window(1)));
        assert_eq!(at(150, 20), Some(Pick::Monitor(monitors[1])));
        assert_eq!(at(-1, 20), Some(Pick::Monitor(monitors[0])));
        assert_eq!(at(0, 300), None);
    }

    #[test]
    fn tab_and_shift_tab_wrap_through_the_windows_and_enter_from_a_monitor_at_either_end() {
        use TabDirection::{Back, Forward};
        let monitor = Some(Pick::Monitor(rect(0, 0, 400, 300)));
        let window = |at| Some(Pick::Window(at));
        assert_eq!(next_window(3, window(0), Forward), window(1));
        assert_eq!(next_window(3, window(2), Forward), window(0));
        assert_eq!(next_window(3, window(0), Back), window(2));
        assert_eq!(next_window(3, window(2), Back), window(1));
        for hovered in [monitor, None] {
            assert_eq!(next_window(3, hovered, Forward), window(0));
            assert_eq!(next_window(3, hovered, Back), window(2));
        }
        assert_eq!(next_window(0, None, Forward), None);
    }
}
