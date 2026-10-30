use cleanshot_core::{BoxHandle, PointF, RectF, box_handle_at, box_handles, resized_box};
use windows::Win32::Foundation::{POINT, RECT};

const HANDLE_REACH: f32 = 6.0;

/// The selection as core's box, for the geometry editor and overlay share. Screen pixels fit
/// an f32 exactly.
#[expect(clippy::cast_precision_loss, reason = "screen coordinates fit in f32")]
pub(crate) fn as_box(selected: RECT) -> RectF {
    RectF {
        x: selected.left as f32,
        y: selected.top as f32,
        width: (selected.right - selected.left) as f32,
        height: (selected.bottom - selected.top) as f32,
    }
}

#[expect(clippy::cast_precision_loss, reason = "screen coordinates fit in f32")]
fn as_point(at: POINT) -> PointF {
    PointF {
        x: at.x as f32,
        y: at.y as f32,
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a box built from screen pixels and integer moves has whole, in-range sides"
)]
fn as_rect(resized: RectF) -> RECT {
    RECT {
        left: resized.x.round() as i32,
        top: resized.y.round() as i32,
        right: (resized.x + resized.width).round() as i32,
        bottom: (resized.y + resized.height).round() as i32,
    }
}

/// What a press starts, decided by where it lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Grip {
    Draw,
    Move,
    Resize(BoxHandle),
}

/// A handle within reach wins over the inside, so small selections stay resizable.
pub(crate) fn grip_at(selected: Option<RECT>, at: POINT) -> Grip {
    let Some(selected) = selected else {
        return Grip::Draw;
    };
    if let Some(handle) = box_handle_at(as_box(selected), as_point(at), HANDLE_REACH) {
        return Grip::Resize(handle);
    }
    let inside = (selected.left..selected.right).contains(&at.x)
        && (selected.top..selected.bottom).contains(&at.y);
    if inside { Grip::Move } else { Grip::Draw }
}

/// The selection after dragging `grip` from `start` to `now`, or None when the result has no
/// area. A resize dragged past the opposite edge flips instead of inverting.
pub(crate) fn dragged(
    grip: Grip,
    original: Option<RECT>,
    start: POINT,
    now: POINT,
) -> Option<RECT> {
    let (dx, dy) = (now.x - start.x, now.y - start.y);
    let Some(original) = original else {
        return span(start, now);
    };
    match grip {
        Grip::Draw => span(start, now),
        Grip::Move => Some(nudged(original, dx, dy)),
        Grip::Resize(handle) => {
            let rect = as_box(original);
            let (_, grabbed) = box_handles(rect)
                .into_iter()
                .find(|(each, _)| *each == handle)
                .expect("box_handles lists every handle");
            let moved = as_point(POINT { x: dx, y: dy });
            let to = PointF {
                x: grabbed.x + moved.x,
                y: grabbed.y + moved.y,
            };
            let resized = as_rect(resized_box(rect, handle, to));
            (resized.right > resized.left && resized.bottom > resized.top).then_some(resized)
        }
    }
}

pub(crate) fn nudged(selected: RECT, dx: i32, dy: i32) -> RECT {
    RECT {
        left: selected.left + dx,
        top: selected.top + dy,
        right: selected.right + dx,
        bottom: selected.bottom + dy,
    }
}

/// The rectangle between two corners, or None when it has no area.
pub(crate) fn span(anchor: POINT, current: POINT) -> Option<RECT> {
    let selected = RECT {
        left: anchor.x.min(current.x),
        top: anchor.y.min(current.y),
        right: anchor.x.max(current.x),
        bottom: anchor.y.max(current.y),
    };
    (selected.right > selected.left && selected.bottom > selected.top).then_some(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: i32, y: i32) -> POINT {
        POINT { x, y }
    }

    const SELECTED: RECT = RECT {
        left: 100,
        top: 100,
        right: 200,
        bottom: 160,
    };

    #[test]
    fn f01_a_drag_spans_its_corners_in_either_direction_and_a_click_spans_nothing() {
        let expected = RECT {
            left: 10,
            top: 20,
            right: 30,
            bottom: 50,
        };
        assert_eq!(span(point(10, 20), point(30, 50)), Some(expected));
        assert_eq!(span(point(30, 50), point(10, 20)), Some(expected));
        assert_eq!(span(point(10, 20), point(10, 50)), None);
        assert_eq!(span(point(10, 20), point(10, 20)), None);
    }

    #[test]
    fn f03_a_press_on_a_handle_resizes_inside_moves_and_outside_draws_anew() {
        let selected = Some(SELECTED);
        assert_eq!(
            grip_at(selected, point(103, 98)),
            Grip::Resize(BoxHandle::TopLeft)
        );
        assert_eq!(
            grip_at(selected, point(150, 160)),
            Grip::Resize(BoxHandle::Bottom)
        );
        assert_eq!(grip_at(selected, point(150, 130)), Grip::Move);
        assert_eq!(grip_at(selected, point(300, 130)), Grip::Draw);
        assert_eq!(grip_at(None, point(150, 130)), Grip::Draw);
    }

    #[test]
    fn f03_dragging_inside_moves_the_selection_without_resizing_it() {
        let moved = dragged(Grip::Move, Some(SELECTED), point(150, 130), point(140, 150));
        assert_eq!(moved, Some(nudged(SELECTED, -10, 20)));
    }

    #[test]
    fn f03_a_handle_moves_only_its_own_edges_and_flips_past_the_opposite_one() {
        let right = Grip::Resize(BoxHandle::Right);
        let wider = dragged(right, Some(SELECTED), point(200, 130), point(230, 130));
        assert_eq!(
            wider,
            Some(RECT {
                right: 230,
                ..SELECTED
            })
        );
        let flipped = dragged(right, Some(SELECTED), point(200, 130), point(60, 130));
        assert_eq!(
            flipped,
            Some(RECT {
                left: 60,
                right: 100,
                ..SELECTED
            })
        );
        let corner = Grip::Resize(BoxHandle::TopLeft);
        let shrunk = dragged(corner, Some(SELECTED), point(100, 100), point(110, 120));
        assert_eq!(
            shrunk,
            Some(RECT {
                left: 110,
                top: 120,
                ..SELECTED
            })
        );
    }
}
