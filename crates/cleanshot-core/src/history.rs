use std::mem;

use crate::document::{Document, Frame, RectF, Shape, ShapeEntry, ShapeId, SpotlightDim, Style};
use crate::tool::Tool;

/// One edit to a document (F-38: every edit is a command).
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Puts a new shape on top of the others.
    Add {
        shape: Shape,
        style: Style,
    },
    Remove(ShapeId),
    /// Changes a shape in place, keeping its id and stacking position.
    Replace {
        id: ShapeId,
        shape: Shape,
        style: Style,
    },
    /// Moves a shape up or down the stack (D-115). A restack that leaves the shape where it is
    /// is not an edit, and there is nothing to undo for it.
    Restack {
        id: ShapeId,
        to: Restack,
    },
    /// `None` exports the whole image (F-37).
    SetCrop(Option<RectF>),
    SetFrame(Frame),
    SetDim(SpotlightDim),
}

/// Where [`Command::Restack`] moves a shape: past the one above or below it, or to the top or
/// bottom of the stack (D-115).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restack {
    Forward,
    Backward,
    Front,
    Back,
}

impl Restack {
    /// Where in `document`'s stack this moves the shape `id`, bottom first, or `None` if it
    /// would stay where it is or the document has no such shape.
    #[must_use]
    pub fn index(self, document: &Document, id: ShapeId) -> Option<usize> {
        let from = position(document, id).ok()?;
        let top = document.shapes.len() - 1;
        let index = match self {
            Restack::Forward => (from + 1).min(top),
            Restack::Backward => from.saturating_sub(1),
            Restack::Front => top,
            Restack::Back => 0,
        };
        (index != from).then_some(index)
    }
}

/// Why [`History::apply`] refused a command.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum CommandError {
    #[error("no shape has id {0:?}")]
    UnknownShape(ShapeId),
    #[error("the crop {0:?} is not a non-empty area inside the image")]
    CropOutsideImage(RectF),
}

/// A document with every edit made to it, each one undoable, without limit (F-38).
#[derive(Debug)]
pub struct History {
    document: Document,
    next_id: u64,
    undo: Vec<(Change, Edit)>,
    redo: Vec<(Change, Edit)>,
}

/// What an edit changed, so the editor can name the step to undo or redo (D-116): a shape,
/// by the tool that draws it, the crop, the image frame or the spotlight dim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Shape(Tool),
    Crop,
    Frame,
    Dim,
}

/// A change that, applied, returns the change that reverts it.
#[derive(Debug)]
enum Edit {
    Insert { index: usize, entry: ShapeEntry },
    Remove(ShapeId),
    Replace(ShapeEntry),
    Restack { id: ShapeId, index: usize },
    SetCrop(Option<RectF>),
    SetFrame(Frame),
    SetDim(SpotlightDim),
}

impl History {
    /// Starts the history of `document`, with nothing to undo.
    #[must_use]
    pub fn new(document: Document) -> Self {
        let next_id = document
            .shapes
            .iter()
            .map(|entry| entry.id.0 + 1)
            .max()
            .unwrap_or(0);
        Self {
            document,
            next_id,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// The document with every applied edit, less those undone.
    #[must_use]
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// Applies `command` and makes it the next edit to undo. Clears the edits to redo.
    ///
    /// # Errors
    ///
    /// [`CommandError::UnknownShape`] if the command names a shape the document lacks;
    /// [`CommandError::CropOutsideImage`] for a crop that is empty or reaches outside the image.
    /// The document is then unchanged.
    pub fn apply(&mut self, command: Command) -> Result<(), CommandError> {
        let shape_of = |id| {
            let index = position(&self.document, id)?;
            Ok(Change::Shape(Tool::drawing(
                &self.document.shapes[index].shape,
            )))
        };
        let (change, edit) = match command {
            Command::Add { shape, style } => {
                let id = ShapeId(self.next_id);
                self.next_id += 1;
                let change = Change::Shape(Tool::drawing(&shape));
                let index = self.document.shapes.len();
                let entry = ShapeEntry { id, shape, style };
                (change, Edit::Insert { index, entry })
            }
            Command::Remove(id) => (shape_of(id)?, Edit::Remove(id)),
            Command::Replace { id, shape, style } => (
                Change::Shape(Tool::drawing(&shape)),
                Edit::Replace(ShapeEntry { id, shape, style }),
            ),
            Command::Restack { id, to } => {
                let change = shape_of(id)?;
                let Some(index) = to.index(&self.document, id) else {
                    return Ok(());
                };
                (change, Edit::Restack { id, index })
            }
            Command::SetCrop(crop) => (Change::Crop, Edit::SetCrop(crop)),
            Command::SetFrame(frame) => (Change::Frame, Edit::SetFrame(frame)),
            Command::SetDim(dim) => (Change::Dim, Edit::SetDim(dim)),
        };
        let inverse = edit.apply(&mut self.document)?;
        self.undo.push((change, inverse));
        self.redo.clear();
        Ok(())
    }

    /// Reverts the latest edit not yet undone. `Ok(false)` if there is none.
    ///
    /// # Errors
    ///
    /// A [`CommandError`] only if the history no longer matches its document, which no sequence
    /// of [`History`] calls produces.
    pub fn undo(&mut self) -> Result<bool, CommandError> {
        replay(&mut self.undo, &mut self.redo, &mut self.document)
    }

    /// Makes the latest undone edit again. `Ok(false)` if there is none.
    ///
    /// # Errors
    ///
    /// As for [`History::undo`].
    pub fn redo(&mut self) -> Result<bool, CommandError> {
        replay(&mut self.redo, &mut self.undo, &mut self.document)
    }

    /// What [`History::undo`] would change next, or `None` when there is nothing to undo.
    #[must_use]
    pub fn to_undo(&self) -> Option<Change> {
        self.undo.last().map(|(change, _)| *change)
    }

    /// What [`History::redo`] would change next, or `None` when there is nothing to redo.
    #[must_use]
    pub fn to_redo(&self) -> Option<Change> {
        self.redo.last().map(|(change, _)| *change)
    }
}

fn replay(
    from: &mut Vec<(Change, Edit)>,
    to: &mut Vec<(Change, Edit)>,
    document: &mut Document,
) -> Result<bool, CommandError> {
    let Some((change, edit)) = from.pop() else {
        return Ok(false);
    };
    to.push((change, edit.apply(document)?));
    Ok(true)
}

impl Edit {
    fn apply(self, document: &mut Document) -> Result<Edit, CommandError> {
        match self {
            Edit::Insert { index, entry } => {
                let id = entry.id;
                document.shapes.insert(index, entry);
                Ok(Edit::Remove(id))
            }
            Edit::Remove(id) => {
                let index = position(document, id)?;
                let entry = document.shapes.remove(index);
                Ok(Edit::Insert { index, entry })
            }
            Edit::Replace(entry) => {
                let index = position(document, entry.id)?;
                Ok(Edit::Replace(mem::replace(
                    &mut document.shapes[index],
                    entry,
                )))
            }
            Edit::Restack { id, index } => {
                let from = position(document, id)?;
                let entry = document.shapes.remove(from);
                document.shapes.insert(index, entry);
                Ok(Edit::Restack { id, index: from })
            }
            Edit::SetCrop(crop) => {
                if let Some(rect) = crop
                    && !inside_image(rect, document)
                {
                    return Err(CommandError::CropOutsideImage(rect));
                }
                Ok(Edit::SetCrop(mem::replace(&mut document.crop, crop)))
            }
            Edit::SetFrame(frame) => Ok(Edit::SetFrame(mem::replace(&mut document.frame, frame))),
            Edit::SetDim(dim) => Ok(Edit::SetDim(mem::replace(&mut document.dim, dim))),
        }
    }
}

fn position(document: &Document, id: ShapeId) -> Result<usize, CommandError> {
    document
        .shapes
        .iter()
        .position(|entry| entry.id == id)
        .ok_or(CommandError::UnknownShape(id))
}

#[expect(
    clippy::cast_precision_loss,
    reason = "image sides are far below 2^24, where f32 stops being exact"
)]
fn inside_image(rect: RectF, document: &Document) -> bool {
    rect.width > 0.0
        && rect.height > 0.0
        && rect.x >= 0.0
        && rect.y >= 0.0
        && rect.x + rect.width <= document.base.width() as f32
        && rect.y + rect.height <= document.base.height() as f32
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::document::{Color, Fill, RgbaImage};

    fn history() -> History {
        let base = RgbaImage::new(100, 50, vec![0; 100 * 50 * 4]).unwrap();
        History::new(Document::new(Arc::new(base)))
    }

    fn rect(x: f32) -> Shape {
        Shape::Rect {
            rect: RectF {
                x,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
            fill: Fill::Hollow,
        }
    }

    fn style() -> Style {
        Style::new(
            Color {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            },
            3.0,
        )
    }

    fn add(history: &mut History, shape: Shape) -> ShapeId {
        history
            .apply(Command::Add {
                shape,
                style: style(),
            })
            .unwrap();
        history.document().shapes().last().unwrap().id
    }

    fn xs(history: &History) -> Vec<f32> {
        history
            .document()
            .shapes()
            .iter()
            .map(|entry| match &entry.shape {
                Shape::Rect { rect, .. } => rect.x,
                other => panic!("tests add only rectangles, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn f38_undo_then_redo_returns_to_each_state_in_turn() {
        let mut history = history();
        let empty = history.document().clone();
        add(&mut history, rect(1.0));
        let added = history.document().clone();

        assert!(history.undo().unwrap());
        assert_eq!(history.document(), &empty);
        assert!(history.redo().unwrap());
        assert_eq!(history.document(), &added);
    }

    #[test]
    fn d115_a_restack_moves_a_shape_a_step_or_to_an_end_and_undoes_to_where_it_was() {
        let mut history = history();
        let ids: Vec<_> = [0.0, 1.0, 2.0, 3.0]
            .into_iter()
            .map(|x| add(&mut history, rect(x)))
            .collect();
        let mut restack = |index: usize, to| {
            history
                .apply(Command::Restack { id: ids[index], to })
                .unwrap();
            xs(&history)
        };
        assert_eq!(restack(1, Restack::Forward), [0.0, 2.0, 1.0, 3.0]);
        assert_eq!(restack(1, Restack::Front), [0.0, 2.0, 3.0, 1.0]);
        assert_eq!(restack(2, Restack::Backward), [2.0, 0.0, 3.0, 1.0]);
        assert_eq!(restack(1, Restack::Back), [1.0, 2.0, 0.0, 3.0]);
        assert!(history.undo().unwrap());
        assert_eq!(xs(&history), [2.0, 0.0, 3.0, 1.0]);
        assert!(history.undo().unwrap());
        assert!(history.undo().unwrap());
        assert!(history.undo().unwrap());
        assert_eq!(xs(&history), [0.0, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn d115_a_restack_that_moves_nothing_leaves_nothing_to_undo() {
        let mut history = history();
        let bottom = add(&mut history, rect(0.0));
        let top = add(&mut history, rect(1.0));
        add(&mut history, rect(2.0));
        assert!(history.undo().unwrap());
        for (id, to) in [
            (top, Restack::Forward),
            (top, Restack::Front),
            (bottom, Restack::Backward),
            (bottom, Restack::Back),
        ] {
            history.apply(Command::Restack { id, to }).unwrap();
        }
        assert_eq!(xs(&history), [0.0, 1.0]);
        assert!(
            history.redo().unwrap(),
            "the undone add can still be redone"
        );
        assert_eq!(xs(&history), [0.0, 1.0, 2.0]);
        assert!(history.undo().unwrap());
        assert!(history.undo().unwrap());
        assert_eq!(xs(&history), [0.0], "no restack was left to undo");
    }

    #[test]
    fn d115_a_move_is_offered_only_where_it_would_move_the_shape() {
        let mut history = history();
        let bottom = add(&mut history, rect(0.0));
        let middle = add(&mut history, rect(1.0));
        let top = add(&mut history, rect(2.0));
        let document = history.document();
        let offered = |id| {
            [
                Restack::Forward,
                Restack::Front,
                Restack::Backward,
                Restack::Back,
            ]
            .map(|to| to.index(document, id))
        };
        assert_eq!(offered(bottom), [Some(1), Some(2), None, None]);
        assert_eq!(offered(middle), [Some(2), Some(2), Some(0), Some(0)]);
        assert_eq!(offered(top), [None, None, Some(1), Some(0)]);
    }

    #[test]
    fn d116_the_next_step_to_undo_or_redo_names_what_it_changed() {
        let mut history = history();
        assert_eq!((history.to_undo(), history.to_redo()), (None, None));
        let id = add(&mut history, rect(0.0));
        let rectangle = Some(Change::Shape(Tool::Rectangle));
        assert_eq!((history.to_undo(), history.to_redo()), (rectangle, None));
        let crop = RectF {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        };
        history.apply(Command::SetCrop(Some(crop))).unwrap();
        assert_eq!(history.to_undo(), Some(Change::Crop));
        assert!(history.undo().unwrap());
        assert_eq!(
            (history.to_undo(), history.to_redo()),
            (rectangle, Some(Change::Crop))
        );
        history
            .apply(Command::SetFrame(Frame {
                border: true,
                shadow: false,
            }))
            .unwrap();
        assert_eq!(
            (history.to_undo(), history.to_redo()),
            (Some(Change::Frame), None)
        );
        history.apply(Command::Remove(id)).unwrap();
        assert!(history.undo().unwrap());
        assert_eq!(
            history.to_redo(),
            rectangle,
            "a removal names the shape it took away"
        );
    }

    #[test]
    fn f38_undo_is_unlimited() {
        let mut history = history();
        for step in 0..1000_u16 {
            add(&mut history, rect(f32::from(step)));
        }
        for _ in 0..1000 {
            assert!(history.undo().unwrap());
        }
        assert_eq!(history.document().shapes(), []);
        assert!(!history.undo().unwrap());
    }

    #[test]
    fn f38_undoing_a_removal_puts_the_shape_back_at_its_stacking_position() {
        let mut history = history();
        add(&mut history, rect(1.0));
        let middle = add(&mut history, rect(2.0));
        add(&mut history, rect(3.0));

        history.apply(Command::Remove(middle)).unwrap();
        assert_eq!(xs(&history), [1.0, 3.0]);
        history.undo().unwrap();
        assert_eq!(xs(&history), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn f38_undoing_a_replace_restores_the_previous_shape_in_place() {
        let mut history = history();
        let first = add(&mut history, rect(1.0));
        add(&mut history, rect(2.0));

        history
            .apply(Command::Replace {
                id: first,
                shape: rect(9.0),
                style: style(),
            })
            .unwrap();
        assert_eq!(xs(&history), [9.0, 2.0]);
        history.undo().unwrap();
        assert_eq!(xs(&history), [1.0, 2.0]);
    }

    #[test]
    fn f38_undoing_a_crop_restores_the_previous_crop() {
        let mut history = history();
        let crop = RectF {
            x: 10.0,
            y: 10.0,
            width: 20.0,
            height: 20.0,
        };
        history.apply(Command::SetCrop(Some(crop))).unwrap();
        assert_eq!(history.document().crop(), Some(crop));
        history.undo().unwrap();
        assert_eq!(history.document().crop(), None);
    }

    #[test]
    fn f38_a_new_edit_discards_the_edits_to_redo() {
        let mut history = history();
        add(&mut history, rect(1.0));
        history.undo().unwrap();
        add(&mut history, rect(2.0));

        assert!(!history.redo().unwrap());
        assert_eq!(xs(&history), [2.0]);
    }

    #[test]
    fn f38_an_added_shape_never_reuses_an_undone_shapes_id() {
        let mut history = history();
        let undone = add(&mut history, rect(1.0));
        history.undo().unwrap();
        let added = add(&mut history, rect(2.0));

        assert_ne!(undone, added);
    }

    #[test]
    fn f38_a_command_on_an_unknown_shape_changes_nothing() {
        let mut history = history();
        let id = add(&mut history, rect(1.0));
        history.undo().unwrap();
        let before = history.document().clone();

        assert_eq!(
            history.apply(Command::Remove(id)),
            Err(CommandError::UnknownShape(id))
        );
        assert_eq!(history.document(), &before);
        assert!(
            history.redo().unwrap(),
            "a failed command keeps the edits to redo"
        );
    }

    #[test]
    fn f37_a_crop_must_be_a_non_empty_area_inside_the_image() {
        let mut history = history();
        let outside = [
            (-1.0, 0.0, 10.0, 10.0),
            (0.0, 0.0, 101.0, 10.0),
            (0.0, 45.0, 10.0, 10.0),
            (5.0, 5.0, 0.0, 10.0),
            (5.0, 5.0, 10.0, -1.0),
            (f32::NAN, 0.0, 10.0, 10.0),
        ];
        for (x, y, width, height) in outside {
            let crop = RectF {
                x,
                y,
                width,
                height,
            };
            assert!(
                matches!(
                    history.apply(Command::SetCrop(Some(crop))),
                    Err(CommandError::CropOutsideImage(_))
                ),
                "{crop:?} was accepted"
            );
        }
        assert_eq!(history.document().crop(), None);
    }

    #[test]
    fn f38_history_of_a_document_with_shapes_issues_fresh_ids() {
        let mut first = history();
        let existing = add(&mut first, rect(1.0));
        let mut second = History::new(first.document().clone());

        assert_ne!(add(&mut second, rect(2.0)), existing);
    }

    #[test]
    fn f38_an_undone_add_leaves_no_shape_with_its_id_and_redo_brings_it_back() {
        let mut history = history();
        let id = add(&mut history, rect(1.0));
        assert!(history.document().contains(id));
        assert_eq!(history.undo(), Ok(true));
        assert!(!history.document().contains(id));
        assert_eq!(history.redo(), Ok(true));
        assert!(history.document().contains(id));
    }

    #[test]
    fn f44_a_new_document_has_no_frame_and_setting_one_is_an_edit_to_undo() {
        let mut history = history();
        assert_eq!(history.document().frame(), Frame::default());
        let framed = Frame {
            border: true,
            shadow: true,
        };
        history.apply(Command::SetFrame(framed)).unwrap();
        assert_eq!(history.document().frame(), framed);
        assert_eq!(history.undo(), Ok(true));
        assert_eq!(history.document().frame(), Frame::default());
        assert_eq!(history.redo(), Ok(true));
        assert_eq!(history.document().frame(), framed);
    }

    #[test]
    fn d123_the_spotlight_dim_starts_at_60_percent_and_setting_it_is_an_edit_to_undo() {
        let mut history = history();
        assert_eq!(history.document().dim(), SpotlightDim::Medium);
        assert_eq!(
            SpotlightDim::Medium.color().a,
            153,
            "the dim F-43 shipped with"
        );
        history.apply(Command::SetDim(SpotlightDim::Dark)).unwrap();
        assert_eq!(history.document().dim(), SpotlightDim::Dark);
        assert_eq!(history.to_undo(), Some(Change::Dim));
        assert_eq!(history.undo(), Ok(true));
        assert_eq!(history.document().dim(), SpotlightDim::Medium);
    }
}
