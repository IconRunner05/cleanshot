use crate::document::{PointF, RectF};
use crate::selection::BoxHandle;
use crate::text::{LINE_HEIGHT, TextBlock};

/// The space between a bubble's text and its edge, and its corners' radius, as a multiple of
/// its text size (F-42, D-87).
pub const BUBBLE_PADDING: f32 = 0.5;

/// A speech bubble (F-42): text in a rounded box filled with the style colour, with a tail
/// from the box to `tail`. The box is not stored: it is the text's extent, padded.
#[derive(Clone, Debug, PartialEq)]
pub struct Bubble {
    block: TextBlock,
    tail: PointF,
}

impl Bubble {
    /// `block` in a bubble whose tail points at `tail`.
    #[must_use]
    pub fn new(block: TextBlock, tail: PointF) -> Self {
        Self { block, tail }
    }

    /// Where the text of a new bubble whose box is dragged out over `area` starts, and the width
    /// in ems it wraps in, for text of `size` pixels per em; `None` if `area` is narrower than one
    /// em of text, as for a resize (D-119, D-126).
    #[must_use]
    pub fn text_box_in(area: RectF, size: f32) -> Option<(PointF, f32)> {
        let padding = BUBBLE_PADDING * size;
        let width = (area.width - 2.0 * padding) / size;
        (width >= 1.0).then_some((
            PointF {
                x: area.x + padding,
                y: area.y + padding,
            },
            width,
        ))
    }

    /// Where a new bubble's tail points when its text starts at `at` in `size` pixels per em:
    /// left of the box, level with the middle of its first line, so the text grows away from it.
    #[must_use]
    pub fn first_tail(at: PointF, size: f32) -> PointF {
        PointF {
            x: at.x - 2.0 * size,
            y: at.y + LINE_HEIGHT * size / 2.0,
        }
    }

    #[must_use]
    pub fn block(&self) -> &TextBlock {
        &self.block
    }

    /// Where the tail points, in image pixels.
    #[must_use]
    pub fn tail(&self) -> PointF {
        self.tail
    }

    /// The rounded box, with corners of [`Bubble::corner_radius`].
    #[must_use]
    pub fn outline(&self) -> RectF {
        let padding = BUBBLE_PADDING * self.block.size();
        let text = self.block.extent();
        RectF {
            x: text.x - padding,
            y: text.y - padding,
            width: text.width + 2.0 * padding,
            height: text.height + 2.0 * padding,
        }
    }

    /// The radius of the box's corners, in image pixels: half the text size.
    #[must_use]
    pub fn corner_radius(&self) -> f32 {
        BUBBLE_PADDING * self.block.size()
    }

    /// The tail as a triangle: a base edge on the side of the box facing the tail, set a little
    /// inside the box so the two fill as one shape, then the tip. `None` while the tail is
    /// inside the box.
    #[must_use]
    pub fn tail_triangle(&self) -> Option<[PointF; 3]> {
        let outline = self.outline();
        let (left, top) = (outline.x, outline.y);
        let (right, bottom) = (left + outline.width, top + outline.height);
        let tip = self.tail;
        let beyond_x = tip.x - tip.x.clamp(left, right);
        let beyond_y = tip.y - tip.y.clamp(top, bottom);
        if beyond_x == 0.0 && beyond_y == 0.0 {
            return None;
        }
        let size = self.block.size();
        let inset = size / 4.0;
        let margin = self.corner_radius();
        let base_centre = |tip: f32, low: f32, high: f32, width: f32| {
            let (low, high) = (low + margin + width / 2.0, high - margin - width / 2.0);
            if low <= high {
                tip.clamp(low, high)
            } else {
                low.midpoint(high)
            }
        };
        let triangle = if beyond_y.abs() >= beyond_x.abs() {
            let width = size.min(outline.width - 2.0 * margin).max(0.0);
            let x = base_centre(tip.x, left, right, width);
            let y = if beyond_y > 0.0 {
                bottom - inset
            } else {
                top + inset
            };
            [
                PointF {
                    x: x - width / 2.0,
                    y,
                },
                tip,
                PointF {
                    x: x + width / 2.0,
                    y,
                },
            ]
        } else {
            let width = size.min(outline.height - 2.0 * margin).max(0.0);
            let y = base_centre(tip.y, top, bottom, width);
            let x = if beyond_x > 0.0 {
                right - inset
            } else {
                left + inset
            };
            [
                PointF {
                    x,
                    y: y - width / 2.0,
                },
                tip,
                PointF {
                    x,
                    y: y + width / 2.0,
                },
            ]
        };
        Some(triangle)
    }

    /// This bubble with the box's `handle` dragged to `to`: the box takes the new width, at
    /// least one em of text, and the text wraps inside it; the text size and the tail stay, and
    /// the box's height follows the wrapped text (D-119). `measure` gives a line's width in ems.
    pub(crate) fn resized(
        &self,
        handle: BoxHandle,
        to: PointF,
        measure: impl Fn(&str) -> f32,
    ) -> Self {
        let size = self.block.size();
        let padding = BUBBLE_PADDING * size;
        let outline = self.outline();
        let (left, right) = (outline.x, outline.x + outline.width);
        let narrowest = size + 2.0 * padding;
        let (left, right) = match handle {
            BoxHandle::TopLeft | BoxHandle::BottomLeft | BoxHandle::Left => {
                (to.x.min(right - narrowest), right)
            }
            BoxHandle::TopRight | BoxHandle::BottomRight | BoxHandle::Right => {
                (left, to.x.max(left + narrowest))
            }
            BoxHandle::Top | BoxHandle::Bottom => (left, right),
        };
        let width = (right - left - 2.0 * padding) / size;
        self.with_block(self.block.in_box(left + padding, width, measure))
    }

    pub(crate) fn with_block(&self, block: TextBlock) -> Self {
        Self {
            block,
            tail: self.tail,
        }
    }

    pub(crate) fn with_tail(&self, tail: PointF) -> Self {
        Self {
            block: self.block.clone(),
            tail,
        }
    }

    pub(crate) fn moved_by(&self, offset: PointF) -> Self {
        let at = self.block.at();
        Self {
            block: self.block.moved_to(PointF {
                x: at.x + offset.x,
                y: at.y + offset.y,
            }),
            tail: PointF {
                x: self.tail.x + offset.x,
                y: self.tail.y + offset.y,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bubble(tail: PointF) -> Bubble {
        // Text at (100, 100), 20 px, 4 ems: one line 80 x 25, so a 100 x 45 box at (90, 90).
        let block = TextBlock::new(PointF { x: 100.0, y: 100.0 }, "Hi".into(), 20.0, 4.0).unwrap();
        Bubble::new(block, tail)
    }

    #[test]
    fn f42_a_new_bubbles_tail_points_left_of_its_box_from_its_first_line() {
        let at = PointF { x: 100.0, y: 100.0 };
        let new = Bubble::new(
            TextBlock::new(at, "Hi\nthere".into(), 20.0, 4.0).unwrap(),
            Bubble::first_tail(at, 20.0),
        );
        let outline = new.outline();
        let [_, tip, _] = new.tail_triangle().expect("the tail is drawn");
        assert!(tip.x < outline.x, "the tip {tip:?} is left of the box");
        assert!(
            (outline.y..outline.y + LINE_HEIGHT * 20.0 + BUBBLE_PADDING * 20.0).contains(&tip.y),
            "the tip {tip:?} is level with the first line"
        );
    }

    #[test]
    fn f42_the_tail_tip_is_dragged_alone_and_a_move_takes_box_and_tail_together() {
        use crate::document::Shape;
        use crate::selection::{Handle, handles, hits, moved, resized};
        let shape = Shape::Bubble(bubble(PointF { x: 120.0, y: 200.0 }));
        assert_eq!(
            handles(&shape).last(),
            Some(&(Handle::End, PointF { x: 120.0, y: 200.0 }))
        );
        assert!(hits(&shape, PointF { x: 120.0, y: 180.0 }, 1.0));
        assert!(hits(&shape, PointF { x: 150.0, y: 110.0 }, 1.0));
        assert!(!hits(&shape, PointF { x: 160.0, y: 180.0 }, 1.0));
        let Shape::Bubble(pointed) =
            resized(&shape, Handle::End, PointF { x: 0.0, y: 0.0 }, |_| 0.0)
        else {
            panic!("a bubble stays a bubble");
        };
        assert_eq!(pointed.tail(), PointF { x: 0.0, y: 0.0 });
        assert_eq!(pointed.outline().x, 90.0);
        let Shape::Bubble(shifted) = moved(&shape, PointF { x: 5.0, y: -5.0 }) else {
            panic!("a bubble stays a bubble");
        };
        assert_eq!(shifted.tail(), PointF { x: 125.0, y: 195.0 });
        assert_eq!((shifted.outline().x, shifted.outline().y), (95.0, 85.0));
    }

    #[test]
    fn d119_a_bubbles_sides_and_corners_set_its_width_and_the_text_wraps_at_the_same_size() {
        use crate::document::Shape;
        use crate::selection::{BoxHandle, Handle, handles, resized};
        let half_an_em_a_letter = |line: &str| line.chars().fold(0.0, |width, _| width + 0.5);
        let tail = PointF { x: 120.0, y: 200.0 };
        let block = TextBlock::new(PointF { x: 100.0, y: 100.0 }, "aa bb cc".into(), 20.0, 4.0);
        let shape = Shape::Bubble(Bubble::new(block.unwrap(), tail));
        let sides: Vec<_> = handles(&shape)
            .into_iter()
            .filter_map(|(handle, _)| match handle {
                Handle::Box(side) => Some(side),
                Handle::Start | Handle::End => None,
            })
            .collect();
        assert_eq!(
            sides,
            [
                BoxHandle::TopLeft,
                BoxHandle::TopRight,
                BoxHandle::Right,
                BoxHandle::BottomRight,
                BoxHandle::BottomLeft,
                BoxHandle::Left,
            ]
        );
        let drag = |handle, x| match resized(
            &shape,
            Handle::Box(handle),
            PointF { x, y: 0.0 },
            half_an_em_a_letter,
        ) {
            Shape::Bubble(bubble) => bubble,
            other => panic!("a bubble stays a bubble, not {other:?}"),
        };
        let narrowed = drag(BoxHandle::Right, 150.0);
        let lines: Vec<_> = narrowed.block().lines().map(|(line, _)| line).collect();
        assert_eq!(lines, ["aa ", "bb ", "cc"]);
        assert_eq!(
            narrowed.outline(),
            RectF {
                x: 90.0,
                y: 90.0,
                width: 60.0,
                height: 95.0,
            }
        );
        assert_eq!((narrowed.block().size(), narrowed.tail()), (20.0, tail));
        let narrowest = drag(BoxHandle::BottomLeft, 160.0);
        assert_eq!(
            (narrowest.outline().x, narrowest.outline().width),
            (150.0, 40.0)
        );
        let retyped = narrowed
            .block()
            .with_text("aa bb cc dd".into(), half_an_em_a_letter)
            .unwrap();
        assert_eq!(retyped.wrap_width(), Some(2.0));
        assert_eq!(retyped.lines().count(), 4);
    }

    #[test]
    fn f42_a_bubbles_box_is_its_text_padded_by_half_its_size() {
        assert_eq!(
            bubble(PointF { x: 0.0, y: 0.0 }).outline(),
            RectF {
                x: 90.0,
                y: 90.0,
                width: 100.0,
                height: 45.0
            }
        );
    }

    #[test]
    fn f42_the_tail_leaves_the_side_facing_its_tip_and_vanishes_inside_the_box() {
        let below = bubble(PointF { x: 120.0, y: 200.0 })
            .tail_triangle()
            .unwrap();
        assert_eq!(below[1], PointF { x: 120.0, y: 200.0 });
        assert_eq!((below[0].y, below[2].y), (130.0, 130.0));
        assert_eq!((below[0].x, below[2].x), (110.0, 130.0));
        let left = bubble(PointF { x: 10.0, y: 112.0 })
            .tail_triangle()
            .unwrap();
        assert_eq!((left[0].x, left[2].x), (95.0, 95.0));
        assert_eq!((left[0].y, left[2].y), (102.0, 122.0));
        let corner = bubble(PointF { x: 0.0, y: 400.0 }).tail_triangle().unwrap();
        assert_eq!(corner[0], PointF { x: 100.0, y: 130.0 });
        assert_eq!(bubble(PointF { x: 140.0, y: 110.0 }).tail_triangle(), None);
    }
}
