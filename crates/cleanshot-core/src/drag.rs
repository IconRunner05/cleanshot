use crate::document::{Document, PointF, Shape, ShapeEntry, ShapeId};
use crate::drawing::{snapped_to_45_degrees, squared};
use crate::history::Command;
use crate::look::Look;
use crate::selection::{BoxHandle, Handle, handle_at, handles, moved, resized, shape_at};
use crate::text_edit::TextOrigin;
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
    /// The select tool moving a shape, which keeps `original` until release.
    Move {
        original: ShapeEntry,
        from: PointF,
        to: PointF,
    },
    /// The select tool dragging one of a shape's handles.
    Resize {
        original: ShapeEntry,
        handle: Handle,
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
    /// A press at `at` with `tool`, while `selected` is selected (F-30). With the select tool, a
    /// handle of the selected shape resizes it, otherwise the topmost shape under the pointer is
    /// selected and moved, and empty canvas clears the selection. A drawing tool clears the
    /// selection and starts drawing. The text tool clears the selection and opens a text box: on
    /// the topmost shape under the pointer if that is text, otherwise a new one at `at` (F-34).
    /// `look` is how `tool` draws, and `tolerance` is the reach of the pointer in image pixels.
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
        let start = match tool {
            // D-87: typing into a bubble waits for the B tool, until the export draws its text.
            Tool::Select | Tool::Bubble => None,
            Tool::Text => Some(Start::Type(text_origin(
                shape_at(document, at, tolerance).and_then(entry),
                at,
                look,
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
        if start.is_some() {
            return Press {
                selected: None,
                start,
            };
        }
        if let Some(original) = selected.and_then(entry)
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
        let Some(original) = shape_at(document, at, tolerance).and_then(entry) else {
            return Press {
                selected: None,
                start: None,
            };
        };
        Press {
            selected: Some(original.id),
            start: Some(Start::Drag(Self::Move {
                original: original.clone(),
                from: at,
                to: at,
            })),
        }
    }

    /// The pointer moved on to `at`.
    pub fn extend(&mut self, at: PointF) {
        match self {
            Self::Draw { path, .. } => path.push(at),
            Self::Move { to, .. } | Self::Resize { to, .. } => *to = at,
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
            Self::Move { original, from, to } => {
                let (mut x, mut y) = (to.x - from.x, to.y - from.y);
                if constrained {
                    if x.abs() < y.abs() {
                        x = 0.0;
                    } else {
                        y = 0.0;
                    }
                }
                replace(original, moved(&original.shape, PointF { x, y }))
            }
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
        }
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

/// The text box a text-tool press at `at` opens, with `hit` the topmost shape under it.
fn text_origin(hit: Option<&ShapeEntry>, at: PointF, look: Look) -> TextOrigin {
    match hit {
        Some(ShapeEntry {
            id,
            shape: Shape::Text(block),
            style,
        }) => TextOrigin::Existing {
            id: *id,
            block: block.clone(),
            style: *style,
        },
        Some(_) | None => TextOrigin::New {
            at,
            size: look.text_size(),
            style: look.style(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::document::{Fill, RectF, RgbaImage, Style};
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
        assert_eq!(on_text.selected, None);
        assert_eq!(
            on_text.start,
            Some(Start::Type(TextOrigin::Existing {
                id: text_id,
                block,
                style
            }))
        );
        let new_box = |at| {
            Some(Start::Type(TextOrigin::New {
                at,
                size: DEFAULT_TEXT_SIZE,
                style: Look::default_for(Tool::Text).style(),
            }))
        };
        assert_eq!(press(point(30.0, 20.0)).start, new_box(point(30.0, 20.0)));
        assert_eq!(press(point(190.0, 10.0)).start, new_box(point(190.0, 10.0)));
    }
}
