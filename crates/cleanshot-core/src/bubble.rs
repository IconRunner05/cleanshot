use crate::document::{PointF, RectF};
use crate::text::TextBlock;

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
    fn f42_the_tail_tip_is_dragged_alone_and_a_move_takes_box_and_tail_together() {
        use crate::document::Shape;
        use crate::selection::{Handle, handles, hits, moved, resized};
        let shape = Shape::Bubble(bubble(PointF { x: 120.0, y: 200.0 }));
        assert_eq!(
            handles(&shape),
            vec![(Handle::End, PointF { x: 120.0, y: 200.0 })]
        );
        assert!(hits(&shape, PointF { x: 120.0, y: 180.0 }, 1.0));
        assert!(hits(&shape, PointF { x: 150.0, y: 110.0 }, 1.0));
        assert!(!hits(&shape, PointF { x: 160.0, y: 180.0 }, 1.0));
        let Shape::Bubble(pointed) = resized(&shape, Handle::End, PointF { x: 0.0, y: 0.0 }) else {
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
