use std::ops::Range;

use cleanshot_core::{
    BoxPress, Color, Command, LineMove, LinePosition, LineStep, Look, Looks, PointF, Property,
    PropertyError, Style, TextBlock, TextBlockError, TextEdit, TextKey, TextOrigin, Tool,
    commit_text, hits,
};
use gpui::{
    App, BorderStyle, Bounds, ClipboardItem, Pixels, Point, UTF16Selection, Window, fill, point,
    px, quad, size, transparent_black,
};

use crate::canvas::rgba;
use crate::placement::Fit;
use crate::text::{LaidLine, em_width, lay_out};
use crate::theme::ACCENT;

const CARET_WIDTH: f32 = 1.5;
/// Screen pixels between the text and the dashed outline around it.
const OUTLINE_MARGIN: f32 = 4.0;

/// The text box being typed into (F-34): the shape it will become, and its text, caret and
/// composition.
#[derive(Clone)]
pub(crate) struct TextBox {
    origin: TextOrigin,
    edit: TextEdit,
}

impl TextBox {
    /// A box on `origin` holding the text already there, with the caret at its end.
    ///
    /// # Errors
    ///
    /// [`TextBlockError::SizeNotPositive`] if `origin`'s size cannot be a text block's.
    pub fn open(origin: TextOrigin) -> Result<Self, TextBlockError> {
        let (at, size, text) = match &origin {
            TextOrigin::New { at, size, .. } => (*at, *size, String::new()),
            TextOrigin::Existing { block, .. } => {
                (block.at(), block.size(), block.text().to_owned())
            }
        };
        TextBlock::new(at, String::new(), size, 0.0)?;
        Ok(Self {
            origin,
            edit: TextEdit::new(text),
        })
    }

    pub fn edit(&self) -> &TextEdit {
        &self.edit
    }

    pub fn edit_mut(&mut self) -> &mut TextEdit {
        &mut self.edit
    }

    /// The tool whose settings the property strip shows for the box: text or bubble.
    pub fn tool(&self) -> Tool {
        self.origin.frame().tool()
    }

    /// How the box draws, as the property strip shows it.
    pub fn look(&self, looks: &Looks) -> Look {
        looks.text_look(&self.origin)
    }

    /// Sets `property` on the box, and on the text it reopened by the returned command.
    ///
    /// # Errors
    ///
    /// [`PropertyError`] if `property`'s value is refused; then nothing changes.
    pub fn restyle(
        &mut self,
        looks: &mut Looks,
        property: Property,
    ) -> Result<Option<Command>, PropertyError> {
        looks.restyle_text(property, &mut self.origin)
    }

    /// The command that makes the box's text a shape, or `None` if that changes nothing.
    ///
    /// # Errors
    ///
    /// [`TextBlockError`] only if GPUI measured a width that is not a number of ems.
    pub fn commit(&self, window: &Window) -> Result<Option<Command>, TextBlockError> {
        commit_text(&self.origin, &self.edit, em_width(self.edit.text(), window))
    }

    /// What the canvas draws while the box is open: its text as the shape it would become, in
    /// place of the shape it reopened.
    pub fn preview(&self, window: &Window) -> Command {
        let shape = self.origin.frame().around(self.block(window));
        match &self.origin {
            TextOrigin::New { style, .. } => Command::Add {
                shape,
                style: *style,
            },
            TextOrigin::Existing { id, style, .. } => Command::Replace {
                id: *id,
                shape,
                style: *style,
            },
        }
    }

    /// A press at `at` in image pixels, gripping within `reach` as a press on a shape does.
    /// Inside the box it moves the caret to the character nearest `at`, and a drag from there
    /// selects up to the pointer until the button comes up.
    pub fn press(&mut self, at: PointF, reach: f32, window: &Window) -> BoxPress {
        let inside = hits(&self.origin.frame().around(self.block(window)), at, reach);
        let press = self.edit.press(inside);
        if press == BoxPress::PlaceCaret {
            let offset = self.offset_at(at, window);
            self.edit.place_caret(offset);
        }
        press
    }

    /// The pointer moved to `at` in image pixels with the button down.
    pub fn drag_to(&mut self, at: PointF, window: &Window) {
        let offset = self.offset_at(at, window);
        self.edit.drag_to(offset);
    }

    /// Acts on `key`. [`TextKey::Commit`] closes the box, which only the editor can do, so it is
    /// the editor's to handle and does nothing here.
    pub fn key(&mut self, key: TextKey, window: &Window, cx: &mut App) {
        match key {
            TextKey::DeleteBackward => self.edit.delete_backward(),
            TextKey::DeleteForward => self.edit.delete_forward(),
            TextKey::MoveCaret { direction, extend } => self.edit.move_caret(direction, extend),
            TextKey::MoveLine { step, extend } => self.move_line(step, extend, window),
            TextKey::LineBreak => self.edit.replace(None, "\n"),
            TextKey::SelectAll => self.edit.select_all(),
            TextKey::Copy => write_to_clipboard(self.edit.copied(), cx),
            TextKey::Cut => write_to_clipboard(self.edit.cut(), cx),
            TextKey::Paste => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.edit.replace(None, &text);
                }
            }
            TextKey::Commit => {}
        }
    }

    /// The selection as the input method reads it.
    pub fn selected(&self) -> UTF16Selection {
        UTF16Selection {
            range: self.edit.selection(),
            reversed: self.edit.reversed(),
        }
    }

    /// Where `range` starts on screen, a caret wide, for the input method's candidate window.
    pub fn bounds_for(&self, range: &Range<usize>, fit: &Fit, window: &Window) -> Bounds<Pixels> {
        let lines = self.screen_lines(fit, window);
        caret(&lines, self.edit.position(range.start))
    }

    /// The offset of the character boundary nearest `position` on screen.
    pub fn offset_at_screen(&self, position: Point<Pixels>, fit: &Fit, window: &Window) -> usize {
        self.offset_at(fit.image_point(position), window)
    }

    /// Draws the dashed outline, the selection, the composition's underline and the caret over
    /// the box's text, which the canvas draws as [`TextBox::preview`].
    pub fn paint(&self, fit: &Fit, window: &mut Window) {
        let outline = fit
            .bounds(self.block(window).extent())
            .dilate(px(OUTLINE_MARGIN));
        window.paint_quad(quad(
            outline,
            px(2.0),
            transparent_black(),
            px(1.0),
            ACCENT,
            BorderStyle::Dashed,
        ));
        let lines = self.screen_lines(fit, window);
        let selection = self.edit.selection();
        let selected = gpui::Rgba { a: 0.3, ..ACCENT };
        for bounds in self.spans(&lines, &selection) {
            window.paint_quad(fill(bounds, selected));
        }
        if let Some(marked) = self.edit.marked() {
            for bounds in self.spans(&lines, &marked) {
                let underline = Bounds::new(
                    point(bounds.origin.x, bounds.bottom() - px(2.0)),
                    size(bounds.size.width, px(1.0)),
                );
                window.paint_quad(fill(underline, ACCENT));
            }
        }
        let head = self.edit.position(self.edit.caret());
        window.paint_quad(fill(caret(&lines, head), rgba(self.text_color())));
    }

    fn text_color(&self) -> Color {
        self.origin.frame().text_color(self.style().color)
    }

    fn style(&self) -> Style {
        match &self.origin {
            TextOrigin::New { style, .. } | TextOrigin::Existing { style, .. } => *style,
        }
    }

    /// The box's text as the text block it would become, measured now.
    fn block(&self, window: &Window) -> TextBlock {
        let (at, size) = match &self.origin {
            TextOrigin::New { at, size, .. } => (*at, *size),
            TextOrigin::Existing { block, .. } => (block.at(), block.size()),
        };
        let text = self.edit.text();
        TextBlock::new(at, text.to_owned(), size, em_width(text, window))
            .expect("open checked the size, and GPUI measures a width of 0 or more ems")
    }

    fn screen_lines(&self, fit: &Fit, window: &Window) -> Vec<LaidLine> {
        let color = rgba(self.text_color());
        lay_out(
            &self.block(window),
            color,
            fit.scale,
            |at| fit.point(at),
            window,
        )
    }

    /// Laid out at one pixel per image pixel, where Up, Down and a press measure.
    fn image_lines(&self, window: &Window) -> Vec<LaidLine> {
        let color = rgba(self.text_color());
        lay_out(
            &self.block(window),
            color,
            1.0,
            |at| point(px(at.x), px(at.y)),
            window,
        )
    }

    /// The offset nearest `at` in image pixels: on the line core places at its height, at the
    /// character boundary nearest its x.
    fn offset_at(&self, at: PointF, window: &Window) -> usize {
        let line = self.block(window).line_at(at.y);
        let lines = self.image_lines(window);
        let byte = lines[line]
            .shaped
            .closest_index_for_x(px(at.x) - lines[line].origin.x);
        self.edit.offset(LinePosition { line, byte })
    }

    /// Up or Down, where core sends it: to the character nearest the caret's x on that line.
    fn move_line(&mut self, step: LineStep, extend: bool, window: &Window) {
        let offset = match self.edit.line_move(step) {
            LineMove::ToOffset(offset) => offset,
            LineMove::ToLine(line) => {
                let lines = self.image_lines(window);
                let from = self.edit.position(self.edit.caret());
                let x = lines[from.line].shaped.x_for_index(from.byte);
                let byte = lines[line].shaped.closest_index_for_x(x);
                self.edit.offset(LinePosition { line, byte })
            }
        };
        self.edit.move_to(offset, extend);
    }

    /// One rectangle per line that `range` covers, from its start to its end on that line.
    fn spans(&self, lines: &[LaidLine], range: &Range<usize>) -> Vec<Bounds<Pixels>> {
        if range.is_empty() {
            return Vec::new();
        }
        let (start, end) = (
            self.edit.position(range.start),
            self.edit.position(range.end),
        );
        lines[start.line..=end.line]
            .iter()
            .zip(start.line..)
            .map(|(line, index)| {
                let from = if index == start.line { start.byte } else { 0 };
                let to = if index == end.line {
                    end.byte
                } else {
                    line.shaped.len()
                };
                let left = line.origin.x + line.shaped.x_for_index(from);
                let right = line.origin.x + line.shaped.x_for_index(to);
                Bounds::new(point(left, line.origin.y), size(right - left, line.height))
            })
            .collect()
    }
}

fn write_to_clipboard(text: Option<String>, cx: &mut App) {
    if let Some(text) = text {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}

fn caret(lines: &[LaidLine], at: LinePosition) -> Bounds<Pixels> {
    let line = &lines[at.line];
    let x = line.origin.x + line.shaped.x_for_index(at.byte);
    Bounds::new(point(x, line.origin.y), size(px(CARET_WIDTH), line.height))
}
