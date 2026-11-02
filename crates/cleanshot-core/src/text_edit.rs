use std::ops::Range;

use crate::{
    Activity, Bubble, Color, Command, LineStep, PointF, Shape, ShapeId, Style, TextBlock,
    TextBlockError, Tool, text_on,
};

/// The text box being typed into (F-34): its text, caret and selection, and the IME's
/// composition, with every offset in UTF-16 code units as the platform's input methods count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    text: String,
    anchor: usize,
    head: usize,
    marked: Option<Range<usize>>,
    /// A press placed the caret and the button is still down.
    selecting: bool,
}

/// A place in the text as the editor's layout counts it: the line, from 0, and the byte within
/// that line's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinePosition {
    pub line: usize,
    pub byte: usize,
}

/// Where Up or Down sends the caret (F-34).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineMove {
    /// To this line, at the caret's x as the editor's layout finds it there.
    ToLine(usize),
    /// Past the first or last line: to this offset, the start or end of the text.
    ToOffset(usize),
}

/// Where an arrow, Home or End key moves the caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretMove {
    Left,
    Right,
    /// Home: the start of the caret's line.
    LineStart,
    /// End: the end of the caret's line, before its line break.
    LineEnd,
}

impl TextEdit {
    /// Editing `text` with the caret at its end.
    #[must_use]
    pub fn new(text: String) -> Self {
        let end = utf16_len(&text);
        Self {
            text,
            anchor: end,
            head: end,
            marked: None,
            selecting: false,
        }
    }

    /// The text as typed so far, composition included.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The selected range, empty at the caret.
    #[must_use]
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    /// Whether the caret is at the start of the selection.
    #[must_use]
    pub fn reversed(&self) -> bool {
        self.head < self.anchor
    }

    /// The end of the selection the caret is at.
    #[must_use]
    pub fn caret(&self) -> usize {
        self.head
    }

    /// The IME's uncommitted composition, while there is one.
    #[must_use]
    pub fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    /// What a press on the canvas does while this box is open, `inside` its layout box. During a
    /// composition every press belongs to the input method.
    #[must_use]
    pub fn press(&self, inside: bool) -> BoxPress {
        match (self.marked.is_some(), inside) {
            (true, _) => BoxPress::Ignore,
            (false, true) => BoxPress::PlaceCaret,
            (false, false) => BoxPress::Commit,
        }
    }

    /// What the box is in the middle of, which decides what a key means.
    #[must_use]
    pub fn activity(&self) -> Activity {
        match self.marked {
            Some(_) => Activity::Composing,
            None => Activity::EditingText,
        }
    }

    /// Ctrl+C: the selected text, or `None` when nothing is selected, which leaves the clipboard
    /// as it is.
    #[must_use]
    pub fn copied(&self) -> Option<String> {
        let selection = self.selection();
        (!selection.is_empty()).then(|| self.text_for_range(selection).0)
    }

    /// Ctrl+X: removes the selection and gives its text, as [`TextEdit::copied`] does.
    pub fn cut(&mut self) -> Option<String> {
        let text = self.copied()?;
        self.replace(Some(self.selection()), "");
        Some(text)
    }

    /// The text in `range`, and the range it actually covers once moved inside the text and
    /// onto whole characters.
    #[must_use]
    pub fn text_for_range(&self, range: Range<usize>) -> (String, Range<usize>) {
        let range = self.snapped(range);
        let bytes = self.byte_at(range.start)..self.byte_at(range.end);
        (self.text[bytes].to_owned(), range)
    }

    /// Types or pastes `new` over `range`, or over the composition or selection when `range` is
    /// `None`, ending any composition with the caret after it. Line breaks become `\n`.
    pub fn replace(&mut self, range: Option<Range<usize>>, new: &str) {
        let end = self.splice(range, new);
        self.anchor = end;
        self.head = end;
        self.marked = None;
    }

    /// Puts the IME's composition `new` over `range` (as [`TextEdit::replace`] picks it),
    /// marked as uncommitted, with `selected` (relative to `new`) selected, or the caret after it.
    pub fn replace_and_mark(
        &mut self,
        range: Option<Range<usize>>,
        new: &str,
        selected: Option<Range<usize>>,
    ) {
        let end = self.splice(range, new);
        let start = end - utf16_len(&line_breaks(new));
        self.marked = (start < end).then_some(start..end);
        let selected = selected.unwrap_or(end - start..end - start);
        self.anchor = self.snap((start + selected.start).min(end));
        self.head = self.snap((start + selected.end).min(end));
    }

    /// Commits the composition as it stands.
    pub fn unmark(&mut self) {
        self.marked = None;
    }

    /// Ctrl+A: selects the whole text, with the caret at its end.
    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.head = utf16_len(&self.text);
    }

    /// Backspace: removes the selection, or the character before the caret.
    pub fn delete_backward(&mut self) {
        if self.anchor == self.head {
            self.anchor = self.previous(self.head);
        }
        self.replace(Some(self.selection()), "");
    }

    /// Delete: removes the selection, or the character after the caret.
    pub fn delete_forward(&mut self) {
        if self.anchor == self.head {
            self.anchor = self.next(self.head);
        }
        self.replace(Some(self.selection()), "");
    }

    /// An arrow, Home or End key: moves the caret one character, or to the selection's edge when
    /// an arrow drops a selection, or to the start or end of the caret's line; `extend` (Shift)
    /// keeps the selection's other end where it is.
    pub fn move_caret(&mut self, direction: CaretMove, extend: bool) {
        let selection = self.selection();
        let drop_selection = !extend && !selection.is_empty();
        let to = match direction {
            CaretMove::Left if drop_selection => selection.start,
            CaretMove::Right if drop_selection => selection.end,
            CaretMove::Left => self.previous(self.head),
            CaretMove::Right => self.next(self.head),
            CaretMove::LineStart => self.line_start(self.head),
            CaretMove::LineEnd => self.line_end(self.head),
        };
        self.move_to(to, extend);
    }

    /// Where Up or Down moves the caret from its line.
    #[must_use]
    pub fn line_move(&self, step: LineStep) -> LineMove {
        let line = self.position(self.head).line;
        let last = self.text.matches('\n').count();
        match step {
            LineStep::Up if line == 0 => LineMove::ToOffset(0),
            LineStep::Up => LineMove::ToLine(line - 1),
            LineStep::Down if line == last => LineMove::ToOffset(utf16_len(&self.text)),
            LineStep::Down => LineMove::ToLine(line + 1),
        }
    }

    /// Puts the caret at `offset`, moved onto a whole character inside the text, as Up and Down
    /// do once the editor's layout has found the offset below or above the caret; `extend`
    /// (Shift) keeps the selection's other end where it is.
    pub fn move_to(&mut self, offset: usize, extend: bool) {
        self.head = self.snap(offset);
        if !extend {
            self.anchor = self.head;
        }
    }

    /// A press inside the box: the caret goes to `offset`, and moves select from there until
    /// [`TextEdit::release`].
    pub fn place_caret(&mut self, offset: usize) {
        self.move_to(offset, false);
        self.selecting = true;
    }

    /// The pointer moved over `offset` with the button down: it selects from the placed caret,
    /// and does nothing unless a press placed it.
    pub fn drag_to(&mut self, offset: usize) {
        if self.selecting {
            self.move_to(offset, true);
        }
    }

    /// The button came up: moves no longer select.
    pub fn release(&mut self) {
        self.selecting = false;
    }

    /// Where `offset` falls, moved onto a whole character inside the text.
    #[must_use]
    pub fn position(&self, offset: usize) -> LinePosition {
        let byte = self.byte_at(offset);
        let before = &self.text[..byte];
        let line_start = before.rfind('\n').map_or(0, |line_break| line_break + 1);
        LinePosition {
            line: before.matches('\n').count(),
            byte: byte - line_start,
        }
    }

    /// The offset at `position`: past the last line is the end of the text, past a line's end is
    /// that end, and inside a character is that character's start.
    #[must_use]
    pub fn offset(&self, position: LinePosition) -> usize {
        let mut line_starts = std::iter::once(0).chain(
            self.text
                .match_indices('\n')
                .map(|(line_break, _)| line_break + 1),
        );
        let Some(line_start) = line_starts.nth(position.line) else {
            return utf16_len(&self.text);
        };
        let rest = &self.text[line_start..];
        let line = rest.split_once('\n').map_or(rest, |(line, _)| line);
        let byte = line.floor_char_boundary(position.byte);
        utf16_len(&self.text[..line_start + byte])
    }

    fn line_start(&self, offset: usize) -> usize {
        let byte = self.byte_at(offset);
        self.text[..byte]
            .rfind('\n')
            .map_or(0, |line_break| utf16_len(&self.text[..=line_break]))
    }

    fn line_end(&self, offset: usize) -> usize {
        let byte = self.byte_at(offset);
        self.text[byte..]
            .find('\n')
            .map_or(utf16_len(&self.text), |line_break| {
                utf16_len(&self.text[..byte + line_break])
            })
    }

    /// Replaces `range` (or the composition or selection) with `new`, returning where `new` ends.
    fn splice(&mut self, range: Option<Range<usize>>, new: &str) -> usize {
        let range = match range {
            Some(range) => self.snapped(range),
            None => self.marked.clone().unwrap_or_else(|| self.selection()),
        };
        let new = line_breaks(new);
        let bytes = self.byte_at(range.start)..self.byte_at(range.end);
        self.text.replace_range(bytes, &new);
        range.start + utf16_len(&new)
    }

    fn snapped(&self, range: Range<usize>) -> Range<usize> {
        let (start, end) = (self.snap(range.start), self.snap(range.end));
        start.min(end)..start.max(end)
    }

    /// `offset` moved inside the text and back onto the start of the character it falls in.
    fn snap(&self, offset: usize) -> usize {
        utf16_len(&self.text[..self.byte_at(offset)])
    }

    fn byte_at(&self, offset: usize) -> usize {
        let mut units = 0;
        for (byte, character) in self.text.char_indices() {
            units += character.len_utf16();
            if units > offset {
                return byte;
            }
        }
        self.text.len()
    }

    fn previous(&self, offset: usize) -> usize {
        let byte = self.byte_at(offset);
        self.text[..byte]
            .chars()
            .next_back()
            .map_or(0, |character| offset - character.len_utf16())
    }

    fn next(&self, offset: usize) -> usize {
        let byte = self.byte_at(offset);
        self.text[byte..]
            .chars()
            .next()
            .map_or(offset, |character| offset + character.len_utf16())
    }
}

fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// Windows' Enter and pasted text carry `\r\n` or `\r`; a text shape breaks lines at `\n` only.
fn line_breaks(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// What a press on the canvas does while a text box is open (F-34).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxPress {
    /// Inside the box: the caret goes where it was pressed.
    PlaceCaret,
    /// Outside the box: the text becomes a shape and the box closes, then the press does what it
    /// would with no box open.
    Commit,
    /// An input method is composing: the press is its.
    Ignore,
}

/// What a text box's text is shown in once committed: plain text (F-34) or a speech bubble
/// whose tail points at `tail` (F-42).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextFrame {
    Plain,
    Bubble { tail: PointF },
}

impl TextFrame {
    /// The tool that draws text in this frame.
    #[must_use]
    pub fn tool(self) -> Tool {
        match self {
            Self::Plain => Tool::Text,
            Self::Bubble { .. } => Tool::Bubble,
        }
    }

    /// The colour text in this frame is drawn in, for a shape in `color`: its own, or on a
    /// bubble filled with it, black or white as reads (D-87).
    #[must_use]
    pub fn text_color(self, color: Color) -> Color {
        match self {
            Self::Plain => color,
            Self::Bubble { .. } => text_on(color),
        }
    }

    /// `block` shown in this frame.
    #[must_use]
    pub fn around(self, block: TextBlock) -> Shape {
        match self {
            Self::Plain => Shape::Text(block),
            Self::Bubble { tail } => Shape::Bubble(Bubble::new(block, tail)),
        }
    }
}

/// Where a text box came from: opened on empty canvas with the current tool's style, or on a
/// text or bubble already there, which keeps its own style.
#[derive(Clone, Debug, PartialEq)]
pub enum TextOrigin {
    New {
        at: PointF,
        size: f32,
        style: Style,
        frame: TextFrame,
    },
    Existing {
        id: ShapeId,
        block: TextBlock,
        style: Style,
        frame: TextFrame,
    },
}

impl TextOrigin {
    /// What the box's text is shown in once committed.
    #[must_use]
    pub fn frame(&self) -> TextFrame {
        match self {
            Self::New { frame, .. } | Self::Existing { frame, .. } => *frame,
        }
    }
}

/// The one command that commits `edit` (F-34, F-38: one undo step per text edit), with
/// `em_width` measured for its text, or `None` when committing changes nothing: an empty new
/// box, or an existing shape whose text is unchanged. Emptying an existing shape removes it. A
/// composition still in progress is committed as it stands, as Windows completes one when its
/// field loses focus (D-52).
///
/// # Errors
///
/// [`TextBlockError`] when the origin's size or `em_width` is not a valid [`TextBlock`]'s.
pub fn commit_text(
    origin: &TextOrigin,
    edit: &TextEdit,
    em_width: f32,
) -> Result<Option<Command>, TextBlockError> {
    let text = edit.text();
    Ok(match origin {
        TextOrigin::New { .. } if text.is_empty() => None,
        TextOrigin::New {
            at,
            size,
            style,
            frame,
        } => Some(Command::Add {
            shape: frame.around(TextBlock::new(*at, text.to_owned(), *size, em_width)?),
            style: *style,
        }),
        TextOrigin::Existing { block, .. } if block.text() == text => None,
        TextOrigin::Existing { id, .. } if text.is_empty() => Some(Command::Remove(*id)),
        TextOrigin::Existing {
            id,
            block,
            style,
            frame,
        } => Some(Command::Replace {
            id: *id,
            shape: frame.around(TextBlock::new(
                block.at(),
                text.to_owned(),
                block.size(),
                em_width,
            )?),
            style: *style,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f34_typing_inserts_at_the_caret_and_replaces_a_selection() {
        let mut edit = TextEdit::new("ab".into());
        edit.replace(None, "c");
        assert_eq!((edit.text(), edit.selection()), ("abc", 3..3));
        edit.move_caret(CaretMove::Left, true);
        edit.move_caret(CaretMove::Left, true);
        edit.replace(None, "XY");
        assert_eq!((edit.text(), edit.selection()), ("aXY", 3..3));
        edit.replace(Some(0..1), "\r\n");
        assert_eq!((edit.text(), edit.selection()), ("\nXY", 1..1));
    }

    #[test]
    fn f34_ctrl_a_then_backspace_empties_the_box() {
        let mut edit = TextEdit::new("two\nlines".into());
        edit.select_all();
        assert_eq!(edit.selection(), 0..9);
        edit.delete_backward();
        assert_eq!((edit.text(), edit.selection()), ("", 0..0));
        edit.delete_backward();
        assert_eq!(edit.text(), "");
    }

    #[test]
    fn f34_backspace_and_delete_remove_one_whole_character_even_outside_the_basic_plane() {
        let mut edit = TextEdit::new("a\u{1F600}b".into());
        edit.move_caret(CaretMove::Left, false);
        edit.delete_backward();
        assert_eq!((edit.text(), edit.selection()), ("ab", 1..1));
        edit.replace(None, "\u{1F600}");
        edit.move_caret(CaretMove::Left, false);
        edit.delete_forward();
        assert_eq!((edit.text(), edit.selection()), ("ab", 1..1));
        edit.move_caret(CaretMove::Right, false);
        edit.delete_forward();
        assert_eq!((edit.text(), edit.selection()), ("ab", 2..2));
    }

    #[test]
    fn f34_an_ime_composition_stays_marked_until_it_is_committed() {
        let mut edit = TextEdit::new("x".into());
        edit.replace_and_mark(None, "ni", None);
        assert_eq!(
            (edit.text(), edit.marked(), edit.selection()),
            ("xni", Some(1..3), 3..3)
        );
        edit.replace_and_mark(None, "\u{4F60}", Some(0..1));
        assert_eq!(
            (edit.text(), edit.marked(), edit.selection()),
            ("x\u{4F60}", Some(1..2), 1..2)
        );
        edit.replace(None, "\u{4F60}\u{597D}");
        assert_eq!(
            (edit.text(), edit.marked(), edit.selection()),
            ("x\u{4F60}\u{597D}", None, 3..3)
        );
        edit.replace_and_mark(None, "a", None);
        edit.replace_and_mark(None, "", None);
        assert_eq!((edit.text(), edit.marked()), ("x\u{4F60}\u{597D}", None));
        edit.replace_and_mark(None, "b", None);
        edit.unmark();
        assert_eq!((edit.text(), edit.marked()), ("x\u{4F60}\u{597D}b", None));
    }

    #[test]
    fn f34_home_and_end_go_to_the_carets_own_line_and_shift_extends() {
        let mut edit = TextEdit::new("one\ntwo\nthree".into());
        edit.move_to(5, false);
        edit.move_caret(CaretMove::LineStart, false);
        assert_eq!(edit.selection(), 4..4);
        edit.move_caret(CaretMove::LineEnd, true);
        assert_eq!((edit.selection(), edit.reversed()), (4..7, false));
        edit.move_caret(CaretMove::LineEnd, false);
        assert_eq!(edit.selection(), 7..7);
        edit.move_to(2, true);
        edit.move_caret(CaretMove::LineStart, true);
        assert_eq!((edit.selection(), edit.reversed()), (0..7, true));
        edit.move_to(13, false);
        edit.move_caret(CaretMove::LineStart, false);
        assert_eq!(edit.selection(), 8..8);
        edit.move_caret(CaretMove::LineEnd, false);
        assert_eq!(edit.selection(), 13..13);
    }

    #[test]
    fn f34_moving_the_caret_to_an_offset_lands_on_a_whole_character_inside_the_text() {
        let mut edit = TextEdit::new("a\u{1F600}b".into());
        edit.move_to(2, false);
        assert_eq!(edit.selection(), 1..1);
        edit.move_to(99, true);
        assert_eq!((edit.selection(), edit.reversed()), (1..4, false));
    }

    #[test]
    fn f34_an_arrow_drops_a_selection_at_its_edge_and_shift_extends_it_either_way() {
        let mut edit = TextEdit::new("abcd".into());
        edit.move_caret(CaretMove::Left, true);
        edit.move_caret(CaretMove::Left, true);
        assert_eq!((edit.selection(), edit.reversed()), (2..4, true));
        edit.move_caret(CaretMove::Right, false);
        assert_eq!(edit.selection(), 4..4);
        edit.select_all();
        edit.move_caret(CaretMove::Left, false);
        assert_eq!(edit.selection(), 0..0);
        edit.move_caret(CaretMove::Left, false);
        assert_eq!(edit.selection(), 0..0);
        edit.move_caret(CaretMove::Right, true);
        assert_eq!((edit.selection(), edit.reversed()), (0..1, false));
    }

    #[test]
    fn f34_ranges_from_the_input_method_are_kept_inside_the_text_and_on_whole_characters() {
        let edit = TextEdit::new("a\u{1F600}b".into());
        assert_eq!(edit.text_for_range(1..3), ("\u{1F600}".into(), 1..3));
        assert_eq!(edit.text_for_range(2..9), ("\u{1F600}b".into(), 1..4));
        assert_eq!(
            edit.text_for_range(Range { start: 9, end: 0 }),
            ("a\u{1F600}b".into(), 0..4)
        );
        let mut edit = edit;
        edit.replace(Some(2..2), "!");
        assert_eq!(edit.text(), "a!\u{1F600}b");
        edit.replace_and_mark(Some(7..8), "c", Some(0..5));
        assert_eq!(
            (edit.text(), edit.marked(), edit.selection()),
            ("a!\u{1F600}bc", Some(5..6), 5..6)
        );
        edit.replace_and_mark(None, "d", Some(3..4));
        assert_eq!((edit.text(), edit.selection()), ("a!\u{1F600}bd", 6..6));
    }

    const STYLE: Style = Style {
        color: crate::Color {
            r: 0,
            g: 0,
            b: 255,
            a: 255,
        },
        stroke_width: 2.0,
    };
    const OWN_STYLE: Style = Style {
        color: crate::Color {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        },
        stroke_width: 4.0,
    };
    const AT: PointF = PointF { x: 4.0, y: 8.0 };

    fn existing(text: &str) -> TextOrigin {
        TextOrigin::Existing {
            id: ShapeId(7),
            block: TextBlock::new(AT, text.into(), 30.0, 1.0).unwrap(),
            style: OWN_STYLE,
            frame: TextFrame::Plain,
        }
    }

    fn commit(origin: &TextOrigin, typed: &str) -> Option<Command> {
        commit_text(origin, &TextEdit::new(typed.into()), 2.5).unwrap()
    }

    #[test]
    fn f34_a_new_box_commits_one_add_with_its_measured_width_or_nothing_when_empty() {
        let new = TextOrigin::New {
            at: AT,
            size: 24.0,
            style: STYLE,
            frame: TextFrame::Plain,
        };
        assert_eq!(
            commit(&new, "hi"),
            Some(Command::Add {
                shape: Shape::Text(TextBlock::new(AT, "hi".into(), 24.0, 2.5).unwrap()),
                style: STYLE,
            })
        );
        assert_eq!(commit(&new, ""), None);
    }

    #[test]
    fn f34_reopened_text_commits_one_replace_in_its_own_style_nothing_when_unchanged_and_a_remove_when_emptied()
     {
        assert_eq!(
            commit(&existing("old"), "new"),
            Some(Command::Replace {
                id: ShapeId(7),
                shape: Shape::Text(TextBlock::new(AT, "new".into(), 30.0, 2.5).unwrap()),
                style: OWN_STYLE,
            })
        );
        assert_eq!(commit(&existing("same"), "same"), None);
        assert_eq!(
            commit(&existing("old"), ""),
            Some(Command::Remove(ShapeId(7)))
        );
    }

    #[test]
    fn f42_a_bubbles_text_commits_as_a_bubble_keeping_its_tail() {
        let tail = PointF { x: 0.0, y: 50.0 };
        let frame = TextFrame::Bubble { tail };
        let new = TextOrigin::New {
            at: AT,
            size: 24.0,
            style: STYLE,
            frame,
        };
        let bubble = |text: &str, size| {
            Shape::Bubble(Bubble::new(
                TextBlock::new(AT, text.into(), size, 2.5).unwrap(),
                tail,
            ))
        };
        assert_eq!(
            commit(&new, "hi"),
            Some(Command::Add {
                shape: bubble("hi", 24.0),
                style: STYLE,
            })
        );
        let TextOrigin::Existing {
            id, block, style, ..
        } = existing("old")
        else {
            panic!("an existing text");
        };
        let reopened = TextOrigin::Existing {
            id,
            block,
            style,
            frame,
        };
        assert_eq!(
            commit(&reopened, "new"),
            Some(Command::Replace {
                id: ShapeId(7),
                shape: bubble("new", 30.0),
                style: OWN_STYLE,
            })
        );
    }

    #[test]
    fn f34_a_commit_with_an_unmeasured_width_is_refused() {
        let new = TextOrigin::New {
            at: AT,
            size: 24.0,
            style: STYLE,
            frame: TextFrame::Plain,
        };
        let edit = TextEdit::new("hi".into());
        assert!(matches!(
            commit_text(&new, &edit, f32::NAN),
            Err(TextBlockError::WidthNotMeasured(width)) if width.is_nan()
        ));
    }

    #[test]
    fn f34_an_offset_maps_to_its_line_and_byte_and_back_on_whole_characters() {
        let edit = TextEdit::new("ab\n\u{1F600}c\n".into());
        let at = |line, byte| LinePosition { line, byte };
        assert_eq!(edit.position(0), at(0, 0));
        assert_eq!(edit.position(2), at(0, 2));
        assert_eq!(edit.position(3), at(1, 0));
        assert_eq!(edit.position(4), at(1, 0));
        assert_eq!(edit.position(5), at(1, 4));
        assert_eq!(edit.position(6), at(1, 5));
        assert_eq!(edit.position(99), at(2, 0));
        assert_eq!(edit.offset(at(1, 4)), 5);
        assert_eq!(edit.offset(at(1, 5)), 6);
        assert_eq!(edit.offset(at(1, 2)), 3);
        assert_eq!(edit.offset(at(0, 9)), 2);
        assert_eq!(edit.offset(at(2, 3)), 7);
        assert_eq!(edit.offset(at(7, 0)), 7);
    }

    #[test]
    fn f34_a_press_places_the_caret_inside_the_box_commits_it_outside_and_belongs_to_a_composition()
    {
        let mut edit = TextEdit::new("ab".into());
        assert_eq!(edit.press(true), BoxPress::PlaceCaret);
        assert_eq!(edit.press(false), BoxPress::Commit);
        edit.replace_and_mark(None, "k", None);
        assert_eq!(edit.press(true), BoxPress::Ignore);
        assert_eq!(edit.press(false), BoxPress::Ignore);
        edit.unmark();
        assert_eq!(edit.press(false), BoxPress::Commit);
    }

    #[test]
    fn f34_up_and_down_go_to_the_next_line_or_past_the_first_and_last_to_the_ends() {
        let mut edit = TextEdit::new("ab\ncd\nef".into());
        assert_eq!(edit.line_move(LineStep::Down), LineMove::ToOffset(8));
        assert_eq!(edit.line_move(LineStep::Up), LineMove::ToLine(1));
        edit.move_to(4, false);
        edit.move_to(1, true);
        assert_eq!(edit.caret(), 1);
        assert_eq!(edit.line_move(LineStep::Up), LineMove::ToOffset(0));
        assert_eq!(edit.line_move(LineStep::Down), LineMove::ToLine(1));
    }

    #[test]
    fn f34_copy_and_cut_take_the_selection_and_leave_the_clipboard_alone_without_one() {
        let mut edit = TextEdit::new("abc".into());
        assert_eq!((edit.copied(), edit.cut()), (None, None));
        assert_eq!(edit.text(), "abc");
        edit.move_caret(CaretMove::Left, true);
        assert_eq!(edit.copied().as_deref(), Some("c"));
        assert_eq!(edit.cut().as_deref(), Some("c"));
        assert_eq!((edit.text(), edit.selection()), ("ab", 2..2));
    }

    #[test]
    fn f34_a_composition_makes_the_box_composing_until_it_is_committed() {
        let mut edit = TextEdit::new(String::new());
        assert_eq!(edit.activity(), Activity::EditingText);
        edit.replace_and_mark(None, "ni", None);
        assert_eq!(edit.activity(), Activity::Composing);
        edit.unmark();
        assert_eq!(edit.activity(), Activity::EditingText);
    }

    #[test]
    fn f34_a_composition_in_progress_is_committed_as_it_stands() {
        let new = TextOrigin::New {
            at: AT,
            size: 24.0,
            style: STYLE,
            frame: TextFrame::Plain,
        };
        let mut edit = TextEdit::new("a".into());
        edit.replace_and_mark(None, "ni", None);
        let committed = commit_text(&new, &edit, 2.5).unwrap();
        assert_eq!(
            committed,
            Some(Command::Add {
                shape: Shape::Text(TextBlock::new(AT, "ani".into(), 24.0, 2.5).unwrap()),
                style: STYLE,
            })
        );
    }

    #[test]
    fn f34_a_drag_from_a_placed_caret_selects_until_the_button_comes_up() {
        let mut edit = TextEdit::new("hello".into());
        edit.drag_to(1);
        assert_eq!(edit.selection(), 5..5);
        edit.place_caret(1);
        edit.drag_to(4);
        assert_eq!((edit.selection(), edit.caret()), (1..4, 4));
        edit.release();
        edit.drag_to(0);
        assert_eq!(edit.selection(), 1..4);
    }
}
