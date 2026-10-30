use std::ops::Range;

/// The text box being typed into (F-34): its text, caret and selection, and the IME's
/// composition, with every offset in UTF-16 code units as the platform's input methods count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    text: String,
    anchor: usize,
    head: usize,
    marked: Option<Range<usize>>,
}

/// Which way an arrow key moves the caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretMove {
    Left,
    Right,
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

    /// The IME's uncommitted composition, while there is one.
    #[must_use]
    pub fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
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
        let start = self.splice(range, new);
        self.anchor = start;
        self.head = start;
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

    /// An arrow key: moves the caret one character, or to the selection's edge when a selection
    /// is dropped; `extend` (Shift) keeps the selection's other end where it is.
    pub fn move_caret(&mut self, direction: CaretMove, extend: bool) {
        let selection = self.selection();
        self.head = match (direction, extend || selection.is_empty()) {
            (CaretMove::Left, false) => selection.start,
            (CaretMove::Right, false) => selection.end,
            (CaretMove::Left, true) => self.previous(self.head),
            (CaretMove::Right, true) => self.next(self.head),
        };
        if !extend {
            self.anchor = self.head;
        }
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
}
