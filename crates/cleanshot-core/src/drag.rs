use crate::bubble::Bubble;
use crate::document::{Document, PointF, RectF, Shape, ShapeEntry, ShapeId};
use crate::drawing::{snapped_to_45_degrees, squared};
use crate::history::Command;
use crate::look::Look;
use crate::selection::{
    BoxHandle, Handle, box_handle_at, handle_at, handles, inside, moved, resized, resized_box,
    shape_at,
};
use crate::text::TextBlock;
use crate::text_edit::{TextFrame, TextOrigin};
use crate::tool::Tool;

/// A pointer drag on the canvas, from press to release. Its edit is one [`Command`], made on
/// release, so a drag is one undo step (F-38).
#[derive(Clone, Debug, PartialEq)]
pub enum Drag {
    /// A drawing tool's drag, through every point the pointer passed, drawing in the look its
    /// tool had at the press.
    Draw {
        tool: Tool,
        look: Look,
        path: Vec<PointF>,
    },
    /// A tool moving a shape it can edit, which keeps `original` until release.
    Move {
        original: ShapeEntry,
        from: PointF,
        to: PointF,
    },
    /// A tool dragging one of the handles of a shape it can edit.
    Resize {
        original: ShapeEntry,
        handle: Handle,
        to: PointF,
    },
    /// The crop tool moving the crop, pressed inside it, or dragging one of its handles
    /// (D-118). The crop stays inside `image`, the whole image.
    Recrop {
        original: RectF,
        handle: Option<BoxHandle>,
        image: RectF,
        from: PointF,
        to: PointF,
    },
}

/// What a press with the pointer does: the shape selected afterwards, and what it starts.
#[derive(Clone, Debug, PartialEq)]
pub struct Press {
    pub selected: Option<ShapeId>,
    pub start: Option<Start>,
}

/// What a press starts: a drag, or typing in a text box (F-34).
#[derive(Clone, Debug, PartialEq)]
pub enum Start {
    Drag(Drag),
    Type(TextOrigin),
}

impl Drag {
    /// A press at `at` with `tool`, while `selected` is selected (F-30). A shape `tool` can
    /// edit is any shape for the select tool, and for any other tool a shape that tool draws
    /// (D-118). A handle of the selected shape, if `tool` can edit it, resizes it; otherwise
    /// the topmost shape under the pointer, if `tool` can edit it, is selected and moved.
    /// Anywhere else the select tool clears the selection, a drawing tool clears it and starts
    /// drawing, and the text and bubble tools clear it and open a text box: on the topmost
    /// shape under the pointer if that is a text or bubble, otherwise a new one at `at` (F-34,
    /// F-42). `look` is how `tool` draws, and `tolerance` is the reach of the pointer in image
    /// pixels.
    #[must_use]
    pub fn press(
        document: &Document,
        tool: Tool,
        look: Look,
        selected: Option<ShapeId>,
        at: PointF,
        tolerance: f32,
    ) -> Press {
        let entry = |id| document.shapes().iter().find(|entry| entry.id == id);
        let editable =
            |entry: &&ShapeEntry| tool == Tool::Select || Tool::drawing(&entry.shape) == tool;
        if let Some(original) = selected.and_then(entry).filter(editable)
            && let Some(handle) = handle_at(&original.shape, at, tolerance)
        {
            return Press {
                selected,
                start: Some(Start::Drag(Self::Resize {
                    original: original.clone(),
                    handle,
                    to: at,
                })),
            };
        }
        let hit = shape_at(document, at, tolerance).and_then(entry);
        if let Some(original) = hit.filter(editable) {
            return Press {
                selected: Some(original.id),
                start: Some(Start::Drag(Self::Move {
                    original: original.clone(),
                    from: at,
                    to: at,
                })),
            };
        }
        if tool == Tool::Crop
            && let Some(crop) = document.crop()
        {
            let handle = box_handle_at(crop, at, tolerance);
            if handle.is_some() || inside(crop, at) {
                return Press {
                    selected: None,
                    start: Some(Start::Drag(Self::Recrop {
                        original: crop,
                        handle,
                        image: document.image_area(),
                        from: at,
                        to: at,
                    })),
                };
            }
        }
        let start = match tool {
            Tool::Select => None,
            Tool::Text => Some(Start::Type(text_origin(hit, at, look, TextFrame::Plain))),
            Tool::Bubble => Some(Start::Type(text_origin(
                hit,
                at,
                look,
                TextFrame::Bubble {
                    tail: Bubble::first_tail(at, look.text_size()),
                },
            ))),
            Tool::Arrow
            | Tool::Line
            | Tool::Rectangle
            | Tool::Ellipse
            | Tool::Pen
            | Tool::Highlighter
            | Tool::Redact
            | Tool::Crop
            | Tool::Spotlight
            | Tool::Step => Some(Start::Drag(Self::Draw {
                tool,
                look,
                path: vec![at],
            })),
        };
        Press {
            selected: None,
            start,
        }
    }

    /// The text box a release opens: a text or bubble the text or bubble tool pressed and let
    /// go of without moving it, so a click on one still types in it (F-34, F-42, D-118).
    #[must_use]
    pub fn typed_on_click(&self, tool: Tool) -> Option<TextOrigin> {
        let Self::Move { original, .. } = self else {
            return None;
        };
        match tool {
            Tool::Text | Tool::Bubble if self.command(false).is_none() => existing_text(original),
            Tool::Text
            | Tool::Bubble
            | Tool::Select
            | Tool::Arrow
            | Tool::Line
            | Tool::Rectangle
            | Tool::Ellipse
            | Tool::Pen
            | Tool::Highlighter
            | Tool::Redact
            | Tool::Crop
            | Tool::Spotlight
            | Tool::Step => None,
        }
    }

    /// The pointer moved on to `at`.
    pub fn extend(&mut self, at: PointF) {
        match self {
            Self::Draw { path, .. } => path.push(at),
            Self::Move { to, .. } | Self::Resize { to, .. } | Self::Recrop { to, .. } => *to = at,
        }
    }

    /// The edit this drag makes if released now, `constrained` while Shift is held. Shift keeps a
    /// move on one axis, snaps a line end to 45 degrees and squares a box resized by a corner.
    /// `None` if the drag changes nothing.
    #[must_use]
    pub fn command(&self, constrained: bool) -> Option<Command> {
        let replace = |original: &ShapeEntry, shape: Shape| {
            (shape != original.shape).then_some(Command::Replace {
                id: original.id,
                shape,
                style: original.style,
            })
        };
        match self {
            Self::Draw { tool, look, path } => tool.command(path, constrained, look),
            Self::Move { original, from, to } => replace(
                original,
                moved(&original.shape, offset(*from, *to, constrained)),
            ),
            Self::Resize {
                original,
                handle,
                to,
            } => {
                let to = if constrained {
                    constrained_handle(&original.shape, *handle, *to)
                } else {
                    *to
                };
                replace(original, resized(&original.shape, *handle, to))
            }
            Self::Recrop {
                original,
                handle,
                image,
                from,
                to,
            } => {
                let crop = match handle {
                    Some(handle) => overlap(resized_box(*original, *handle, *to), *image)?,
                    None => kept_inside(*original, offset(*from, *to, constrained), *image),
                };
                (crop != *original).then_some(Command::SetCrop(Some(crop)))
            }
        }
    }
}

/// How far a move from `from` to `to` goes; `constrained`, only along its longer axis.
fn offset(from: PointF, to: PointF, constrained: bool) -> PointF {
    let (x, y) = (to.x - from.x, to.y - from.y);
    match (constrained, x.abs() < y.abs()) {
        (false, _) => PointF { x, y },
        (true, true) => PointF { x: 0.0, y },
        (true, false) => PointF { x, y: 0.0 },
    }
}

/// The part `a` shares with `b`, if it has an area.
fn overlap(a: RectF, b: RectF) -> Option<RectF> {
    let (left, top) = (a.x.max(b.x), a.y.max(b.y));
    let (right, bottom) = (
        (a.x + a.width).min(b.x + b.width),
        (a.y + a.height).min(b.y + b.height),
    );
    (left < right && top < bottom).then_some(RectF {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

/// `rect` moved by `offset`, but no further than keeps it inside `within`.
fn kept_inside(rect: RectF, offset: PointF, within: RectF) -> RectF {
    let along = |at: f32, by: f32, size: f32, start: f32, length: f32| {
        (at + by).clamp(start, (start + length - size).max(start))
    };
    RectF {
        x: along(rect.x, offset.x, rect.width, within.x, within.width),
        y: along(rect.y, offset.y, rect.height, within.y, within.height),
        ..rect
    }
}

/// Where Shift puts a dragged handle: on 45 degrees from a line's other end, or square with a
/// box's opposite corner. Edge handles, which change one side only, are left where they are.
fn constrained_handle(shape: &Shape, handle: Handle, to: PointF) -> PointF {
    let ends = match shape {
        Shape::Arrow { from, to } | Shape::Line { from, to } => Some((*from, *to)),
        Shape::Rect { .. }
        | Shape::Ellipse { .. }
        | Shape::Pen(_)
        | Shape::Text(_)
        | Shape::Highlight(_)
        | Shape::Redact { .. }
        | Shape::Spotlight(_)
        | Shape::Step(_)
        | Shape::Bubble(_) => None,
    };
    match (handle, ends) {
        (Handle::Start, Some((_, end))) => snapped_to_45_degrees(end, to),
        (Handle::End, Some((start, _))) => snapped_to_45_degrees(start, to),
        (Handle::Box(corner), _) => {
            opposite_corner(shape, corner).map_or(to, |anchor| squared(anchor, to))
        }
        (Handle::Start | Handle::End, None) => to,
    }
}

fn opposite_corner(shape: &Shape, corner: BoxHandle) -> Option<PointF> {
    let opposite = match corner {
        BoxHandle::TopLeft => BoxHandle::BottomRight,
        BoxHandle::TopRight => BoxHandle::BottomLeft,
        BoxHandle::BottomRight => BoxHandle::TopLeft,
        BoxHandle::BottomLeft => BoxHandle::TopRight,
        BoxHandle::Top | BoxHandle::Right | BoxHandle::Bottom | BoxHandle::Left => return None,
    };
    handles(shape)
        .into_iter()
        .find(|(handle, _)| *handle == Handle::Box(opposite))
        .map(|(_, at)| at)
}

/// The text box a text or bubble press at `at` opens, with `hit` the topmost shape under it: a
/// text or bubble there reopens, whichever tool pressed it, and anywhere else a new box opens
/// in `new`.
fn text_origin(hit: Option<&ShapeEntry>, at: PointF, look: Look, new: TextFrame) -> TextOrigin {
    hit.and_then(existing_text).unwrap_or(TextOrigin::New {
        at,
        size: look.text_size(),
        style: look.style(),
        frame: new,
    })
}

/// The text box that reopens `entry`, if it is a text or bubble.
fn existing_text(entry: &ShapeEntry) -> Option<TextOrigin> {
    let existing = |block: &TextBlock, frame| TextOrigin::Existing {
        id: entry.id,
        block: block.clone(),
        style: entry.style,
        frame,
    };
    match &entry.shape {
        Shape::Text(block) => Some(existing(block, TextFrame::Plain)),
        Shape::Bubble(bubble) => Some(existing(
            bubble.block(),
            TextFrame::Bubble {
                tail: bubble.tail(),
            },
        )),
        Shape::Arrow { .. }
        | Shape::Line { .. }
        | Shape::Rect { .. }
        | Shape::Ellipse { .. }
        | Shape::Pen(_)
        | Shape::Highlight(_)
        | Shape::Redact { .. }
        | Shape::Spotlight(_)
        | Shape::Step(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::document::{Fill, RgbaImage, Style};
    use crate::history::History;
    use crate::text::{DEFAULT_TEXT_SIZE, TextBlock};

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

    fn box_shape() -> Shape {
        Shape::Rect {
            rect: rect(10.0, 10.0, 40.0, 20.0),
            fill: Fill::Solid,
        }
    }

    fn line_shape() -> Shape {
        Shape::Line {
            from: point(100.0, 100.0),
            to: point(150.0, 100.0),
        }
    }

    fn document() -> (Document, ShapeId, ShapeId) {
        let image = RgbaImage::new(200, 200, vec![0; 200 * 200 * 4]).unwrap();
        let mut history = History::new(Document::new(Arc::new(image)));
        let style = Look::default_for(Tool::Rectangle).style();
        for shape in [box_shape(), line_shape()] {
            history.apply(Command::Add { shape, style }).unwrap();
        }
        let ids: Vec<_> = history
            .document()
            .shapes()
            .iter()
            .map(|entry| entry.id)
            .collect();
        (history.document().clone(), ids[0], ids[1])
    }

    fn drag(press: Press) -> Option<Drag> {
        match press.start? {
            Start::Drag(drag) => Some(drag),
            Start::Type(origin) => panic!("a drag, not a text box at {origin:?}"),
        }
    }

    fn dragged(press: Press, to: PointF, constrained: bool) -> Option<Command> {
        let mut drag = drag(press)?;
        drag.extend(to);
        drag.command(constrained)
    }

    #[test]
    fn f30_a_press_on_a_shape_selects_it_and_a_press_on_nothing_clears_the_selection() {
        let (document, box_id, line_id) = document();
        let on_box = Drag::press(
            &document,
            Tool::Select,
            Look::default_for(Tool::Select),
            Some(line_id),
            point(30.0, 20.0),
            4.0,
        );
        assert_eq!(on_box.selected, Some(box_id));
        assert!(matches!(drag(on_box), Some(Drag::Move { .. })));

        let on_nothing = Drag::press(
            &document,
            Tool::Select,
            Look::default_for(Tool::Select),
            Some(box_id),
            point(190.0, 190.0),
            4.0,
        );
        assert_eq!(
            on_nothing,
            Press {
                selected: None,
                start: None
            }
        );
    }

    #[test]
    fn f30_a_press_on_a_handle_of_the_selected_shape_resizes_it() {
        let (document, box_id, _) = document();
        let press = Drag::press(
            &document,
            Tool::Select,
            Look::default_for(Tool::Select),
            Some(box_id),
            point(51.0, 31.0),
            4.0,
        );
        assert_eq!(
            dragged(press, point(70.0, 40.0), false),
            Some(Command::Replace {
                id: box_id,
                shape: Shape::Rect {
                    rect: rect(10.0, 10.0, 60.0, 30.0),
                    fill: Fill::Solid
                },
                style: Look::default_for(Tool::Rectangle).style(),
            })
        );
        let unselected = Drag::press(
            &document,
            Tool::Select,
            Look::default_for(Tool::Select),
            None,
            point(51.0, 31.0),
            4.0,
        );
        assert!(matches!(drag(unselected), Some(Drag::Move { .. })));
    }

    #[test]
    fn f30_a_drawing_tool_clears_the_selection_and_draws_through_the_whole_path() {
        let (document, box_id, _) = document();
        let press = Drag::press(
            &document,
            Tool::Pen,
            Look::default_for(Tool::Pen),
            Some(box_id),
            point(0.0, 0.0),
            4.0,
        );
        assert_eq!(press.selected, None);
        let mut drag = drag(press).unwrap();
        let rest = [point(16.0, 0.0), point(16.0, 16.0)];
        for at in rest {
            drag.extend(at);
        }
        assert_eq!(
            drag.command(false),
            Tool::Pen.command(
                &[point(0.0, 0.0), rest[0], rest[1]],
                false,
                &Look::default_for(Tool::Pen)
            )
        );
    }

    #[test]
    fn f38_a_move_is_one_replace_and_a_drag_back_to_the_start_changes_nothing() {
        let (document, box_id, _) = document();
        let press = || {
            Drag::press(
                &document,
                Tool::Select,
                Look::default_for(Tool::Select),
                None,
                point(30.0, 20.0),
                4.0,
            )
        };
        let Some(Command::Replace { id, shape, .. }) = dragged(press(), point(35.0, 22.0), false)
        else {
            panic!("no replace");
        };
        assert_eq!((id, shape), (box_id, moved(&box_shape(), point(5.0, 2.0))));
        assert_eq!(dragged(press(), point(30.0, 20.0), false), None);
    }

    #[test]
    fn f30_shift_keeps_a_move_on_its_longer_axis() {
        let (document, _, _) = document();
        let press = Drag::press(
            &document,
            Tool::Select,
            Look::default_for(Tool::Select),
            None,
            point(30.0, 20.0),
            4.0,
        );
        let Some(Command::Replace { shape, .. }) = dragged(press, point(33.0, 29.0), true) else {
            panic!("no replace");
        };
        assert_eq!(shape, moved(&box_shape(), point(0.0, 9.0)));
    }

    #[test]
    fn f30_shift_squares_a_corner_resize_and_snaps_a_line_end_to_45_degrees() {
        let (document, box_id, line_id) = document();
        let corner = Drag::press(
            &document,
            Tool::Select,
            Look::default_for(Tool::Select),
            Some(box_id),
            point(50.0, 30.0),
            4.0,
        );
        let Some(Command::Replace { shape, .. }) = dragged(corner, point(80.0, 40.0), true) else {
            panic!("no replace");
        };
        assert_eq!(
            shape,
            Shape::Rect {
                rect: rect(10.0, 10.0, 70.0, 70.0),
                fill: Fill::Solid
            }
        );

        let end = Drag::press(
            &document,
            Tool::Select,
            Look::default_for(Tool::Select),
            Some(line_id),
            point(150.0, 100.0),
            4.0,
        );
        let Some(Command::Replace { shape, .. }) = dragged(end, point(190.0, 130.0), true) else {
            panic!("no replace");
        };
        assert_eq!(
            shape,
            Shape::Line {
                from: point(100.0, 100.0),
                to: point(190.0, 100.0)
            }
        );
    }

    #[test]
    fn f34_the_text_tool_reopens_the_text_under_the_pointer_and_opens_a_new_box_anywhere_else() {
        let (document, box_id, _) = document();
        let mut history = History::new(document);
        let block = TextBlock::new(point(100.0, 150.0), "hi".into(), 20.0, 2.0).unwrap();
        let style = Style {
            color: crate::Color {
                r: 0,
                g: 0,
                b: 255,
                a: 255,
            },
            stroke_width: 1.0,
        };
        let shape = Shape::Text(block.clone());
        history.apply(Command::Add { shape, style }).unwrap();
        let document = history.document();
        let text_id = document.shapes().last().unwrap().id;
        let press = |at| {
            Drag::press(
                document,
                Tool::Text,
                Look::default_for(Tool::Text),
                Some(box_id),
                at,
                4.0,
            )
        };

        let on_text = press(point(130.0, 160.0));
        assert_eq!(on_text.selected, Some(text_id));
        let Some(Start::Drag(clicked)) = on_text.start else {
            panic!("a press on a text with the text tool picks it up");
        };
        assert_eq!(
            clicked.typed_on_click(Tool::Text),
            Some(TextOrigin::Existing {
                id: text_id,
                block,
                style,
                frame: TextFrame::Plain,
            })
        );
        let new_box = |at| {
            Some(Start::Type(TextOrigin::New {
                at,
                size: DEFAULT_TEXT_SIZE,
                style: Look::default_for(Tool::Text).style(),
                frame: TextFrame::Plain,
            }))
        };
        assert_eq!(press(point(30.0, 20.0)).start, new_box(point(30.0, 20.0)));
        assert_eq!(press(point(190.0, 10.0)).start, new_box(point(190.0, 10.0)));
    }

    #[test]
    fn f42_the_bubble_tool_opens_a_bubble_anywhere_and_either_text_tool_reopens_a_bubble() {
        let (document, _, _) = document();
        let mut history = History::new(document);
        let block = TextBlock::new(point(300.0, 300.0), "hi".into(), 20.0, 2.0).unwrap();
        let tail = point(250.0, 400.0);
        let style = Look::default_for(Tool::Bubble).style();
        let shape = Shape::Bubble(Bubble::new(block.clone(), tail));
        history.apply(Command::Add { shape, style }).unwrap();
        let document = history.document();
        let bubble_id = document.shapes().last().unwrap().id;
        let press =
            |tool, at| Drag::press(document, tool, Look::default_for(tool), None, at, 4.0).start;

        let reopened = TextOrigin::Existing {
            id: bubble_id,
            block,
            style,
            frame: TextFrame::Bubble { tail },
        };
        let Some(Start::Drag(clicked)) = press(Tool::Bubble, point(310.0, 305.0)) else {
            panic!("a press on a bubble with the bubble tool picks it up");
        };
        assert_eq!(clicked.typed_on_click(Tool::Bubble), Some(reopened.clone()));
        assert_eq!(
            press(Tool::Text, point(310.0, 305.0)),
            Some(Start::Type(reopened))
        );
        let at = point(30.0, 20.0);
        assert_eq!(
            press(Tool::Bubble, at),
            Some(Start::Type(TextOrigin::New {
                at,
                size: DEFAULT_TEXT_SIZE,
                style,
                frame: TextFrame::Bubble {
                    tail: Bubble::first_tail(at, DEFAULT_TEXT_SIZE),
                },
            }))
        );
    }

    #[test]
    fn d118_a_tool_moves_and_resizes_its_own_shapes_and_draws_anywhere_else() {
        let (document, box_id, line_id) = document();
        let press = |tool, selected, at| {
            Drag::press(&document, tool, Look::default_for(tool), selected, at, 4.0)
        };
        let on_line = press(Tool::Line, None, point(125.0, 100.0));
        assert_eq!(on_line.selected, Some(line_id));
        assert!(matches!(
            on_line.start,
            Some(Start::Drag(Drag::Move { .. }))
        ));
        let line_end = press(Tool::Line, Some(line_id), point(150.0, 100.0));
        assert_eq!(line_end.selected, Some(line_id));
        assert!(matches!(
            line_end.start,
            Some(Start::Drag(Drag::Resize {
                handle: Handle::End,
                ..
            }))
        ));
        for (tool, selected, at) in [
            (Tool::Line, Some(line_id), point(20.0, 20.0)),
            (Tool::Rectangle, Some(line_id), point(150.0, 100.0)),
            (Tool::Rectangle, Some(box_id), point(180.0, 180.0)),
        ] {
            let drawn = press(tool, selected, at);
            assert_eq!(drawn.selected, None, "{tool:?} at {at:?}");
            assert!(
                matches!(drawn.start, Some(Start::Drag(Drag::Draw { .. }))),
                "{tool:?} at {at:?}"
            );
        }
    }

    #[test]
    fn d118_the_bubble_tool_drags_a_bubbles_tail_and_a_moved_bubble_opens_no_text_box() {
        let (document, _, _) = document();
        let mut history = History::new(document);
        let block = TextBlock::new(point(100.0, 150.0), "hi".into(), 20.0, 2.0).unwrap();
        let tail = point(60.0, 190.0);
        let style = Look::default_for(Tool::Bubble).style();
        let shape = Shape::Bubble(Bubble::new(block.clone(), tail));
        history.apply(Command::Add { shape, style }).unwrap();
        let document = history.document();
        let bubble_id = document.shapes().last().unwrap().id;
        let look = Look::default_for(Tool::Bubble);
        let Some(Start::Drag(mut tip)) =
            Drag::press(document, Tool::Bubble, look, Some(bubble_id), tail, 4.0).start
        else {
            panic!("the bubble tool picks up the tail's tip");
        };
        tip.extend(point(20.0, 195.0));
        assert_eq!(
            tip.command(false),
            Some(Command::Replace {
                id: bubble_id,
                shape: Shape::Bubble(Bubble::new(block, point(20.0, 195.0))),
                style,
            })
        );
        let Some(Start::Drag(mut held)) =
            Drag::press(document, Tool::Bubble, look, None, point(110.0, 155.0), 4.0).start
        else {
            panic!("the bubble tool picks up the bubble");
        };
        held.extend(point(130.0, 160.0));
        assert!(held.command(false).is_some());
        assert_eq!(held.typed_on_click(Tool::Bubble), None);
    }

    #[test]
    fn d118_the_crop_tool_moves_and_resizes_the_crop_inside_the_image_and_draws_anywhere_else() {
        let (document, _, _) = document();
        let mut history = History::new(document);
        let crop = rect(50.0, 50.0, 100.0, 80.0);
        history.apply(Command::SetCrop(Some(crop))).unwrap();
        let document = history.document();
        let dragged = |tool, from, to| {
            let mut drag =
                match Drag::press(document, tool, Look::default_for(tool), None, from, 4.0).start {
                    Some(Start::Drag(drag)) => drag,
                    other => panic!("{tool:?} at {from:?} starts {other:?}"),
                };
            drag.extend(to);
            drag
        };
        let recrop = |from, to| match dragged(Tool::Crop, from, to) {
            drag @ Drag::Recrop { .. } => drag.command(false),
            other => panic!("the crop tool at {from:?} starts {other:?}"),
        };
        assert_eq!(
            recrop(point(100.0, 90.0), point(120.0, 80.0)),
            Some(Command::SetCrop(Some(rect(70.0, 40.0, 100.0, 80.0))))
        );
        assert_eq!(
            recrop(point(100.0, 90.0), point(300.0, 90.0)),
            Some(Command::SetCrop(Some(rect(100.0, 50.0, 100.0, 80.0))))
        );
        assert_eq!(
            recrop(point(150.0, 130.0), point(180.0, 170.0)),
            Some(Command::SetCrop(Some(rect(50.0, 50.0, 130.0, 120.0))))
        );
        assert_eq!(
            recrop(point(150.0, 130.0), point(250.0, 260.0)),
            Some(Command::SetCrop(Some(rect(50.0, 50.0, 150.0, 150.0))))
        );
        assert_eq!(recrop(point(100.0, 90.0), point(100.0, 90.0)), None);
        for (tool, at) in [
            (Tool::Crop, point(10.0, 180.0)),
            (Tool::Rectangle, point(100.0, 90.0)),
        ] {
            assert!(
                matches!(dragged(tool, at, at), Drag::Draw { .. }),
                "{tool:?} at {at:?}"
            );
        }
    }
}
