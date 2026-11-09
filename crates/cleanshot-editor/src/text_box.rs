use std::ops::Range;

use cleanshot_core::{
    BoxPress, Color, Command, LineMove, LinePosition, LineStep, Look, Looks, PointF, Property,
    PropertyError, Style, TextBlock, TextBlockError, TextEdit, TextEditError, TextKey, TextOrigin,
    Tool, commit_text, hits,
};
use gpui::{
    App, BorderStyle, Bounds, ClipboardItem, Pixels, Point, Rgba, UTF16Selection, Window, fill,
    point, px, quad, size, transparent_black,
};

use crate::canvas::rgba;
use crate::placement::Fit;
use crate::text::{LaidLine, lay_out, line_width};

const CARET_WIDTH: f32 = 1.5;
/// Screen pixels between the text and the dashed outline around it.
const OUTLINE_MARGIN: f32 = 4.0;

/// The text box being typed into: the shape it will become, and its text, caret and
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
    pub(crate) fn open(origin: TextOrigin) -> Result<Self, TextBlockError> {
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

    pub(crate) fn edit(&self) -> &TextEdit {
        &self.edit
    }

    pub(crate) fn edit_mut(&mut self) -> &mut TextEdit {
        &mut self.edit
    }

    /// Applies `change`, keeping the result only if GPUI can measure it.
    ///
    /// # Errors
    ///
    /// [`TextEditError`] if `change` or the measuring refused it, the box left as it was.
    pub(crate) fn edit_measured(
        &mut self,
        window: &Window,
        change: impl FnOnce(&mut TextEdit) -> Result<(), TextEditError>,
    ) -> Result<(), TextEditError> {
        self.edit
            .measured(&self.origin, |line| line_width(line, window), change)
    }

    /// The tool whose settings the property strip shows for the box: text or bubble.
    pub(crate) fn tool(&self) -> Tool {
        self.origin.frame().tool()
    }

    /// How the box draws, as the property strip shows it.
    pub(crate) fn look(&self, looks: &Looks) -> Look {
        looks.text_look(&self.origin)
    }

    /// Sets `property` on the box, and on the text it reopened by the returned command.
    ///
    /// # Errors
    ///
    /// [`PropertyError`] if `property`'s value is refused; then nothing changes.
    pub(crate) fn restyle(
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
    pub(crate) fn commit(&self, window: &Window) -> Result<Option<Command>, TextBlockError> {
        commit_text(&self.origin, &self.edit, |line| line_width(line, window))
    }

    /// What the canvas draws while the box is open: its text as the shape it would become, in
    /// place of the shape it reopened.
    pub(crate) fn preview(&self, window: &Window) -> Command {
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
    pub(crate) fn press(&mut self, at: PointF, reach: f32, window: &Window) -> BoxPress {
        let inside = hits(&self.origin.frame().around(self.block(window)), at, reach);
        let press = self.edit.press(inside);
        if press == BoxPress::PlaceCaret {
            let offset = self.offset_at(at, window);
            self.edit.place_caret(offset);
        }
        press
    }

    /// The pointer moved to `at` in image pixels with the button down.
    pub(crate) fn drag_to(&mut self, at: PointF, window: &Window) {
        let offset = self.offset_at(at, window);
        self.edit.drag_to(offset);
    }

    /// Acts on `key`. [`TextKey::Commit`] closes the box, which only the editor can do, so it is
    /// the editor's to handle and does nothing here.
    ///
    /// # Errors
    ///
    /// [`TextEditError`] if a line break or paste was refused, the box left as it was.
    pub(crate) fn key(
        &mut self,
        key: TextKey,
        window: &Window,
        cx: &mut App,
    ) -> Result<(), TextEditError> {
        match key {
            TextKey::DeleteBackward => self.edit.delete_backward(),
            TextKey::DeleteForward => self.edit.delete_forward(),
            TextKey::MoveCaret { direction, extend } => self.edit.move_caret(direction, extend),
            TextKey::MoveLine { step, extend } => self.move_line(step, extend, window),
            TextKey::LineBreak => {
                self.edit_measured(window, |edit| edit.replace(None, "\n"))?;
            }
            TextKey::SelectAll => self.edit.select_all(),
            TextKey::Copy => write_to_clipboard(self.edit.copied(), cx),
            TextKey::Cut => write_to_clipboard(self.edit.cut(), cx),
            TextKey::Paste => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.edit_measured(window, |edit| edit.replace(None, &text))?;
                }
            }
            TextKey::Commit => {}
        }
        Ok(())
    }

    /// The selection as the input method reads it.
    pub(crate) fn selected(&self) -> UTF16Selection {
        UTF16Selection {
            range: self.edit.selection(),
            reversed: self.edit.reversed(),
        }
    }

    /// Where `range` starts on screen, a caret wide, for the input method's candidate window.
    pub(crate) fn bounds_for(
        &self,
        range: &Range<usize>,
        fit: &Fit,
        window: &Window,
    ) -> Bounds<Pixels> {
        let lines = self.screen_lines(fit, window);
        caret(
            &lines,
            self.edit.position(range.start, &self.line_ranges(window)),
        )
    }

    /// The offset of the character boundary nearest `position` on screen.
    pub(crate) fn offset_at_screen(
        &self,
        position: Point<Pixels>,
        fit: &Fit,
        window: &Window,
    ) -> usize {
        self.offset_at(fit.image_point(position), window)
    }

    /// Draws the dashed outline, the selection, the composition's underline and the caret over
    /// the box's text, which the canvas draws as [`TextBox::preview`].
    pub(crate) fn paint(&self, fit: &Fit, accent: Rgba, window: &mut Window) {
        let outline = fit
            .bounds(self.block(window).extent())
            .dilate(px(OUTLINE_MARGIN));
        window.paint_quad(quad(
            outline,
            px(2.0),
            transparent_black(),
            px(1.0),
            accent,
            BorderStyle::Dashed,
        ));
        let lines = self.screen_lines(fit, window);
        let ranges = self.line_ranges(window);
        let selection = self.edit.selection();
        let selected = Rgba { a: 0.3, ..accent };
        for bounds in self.spans(&lines, &ranges, &selection) {
            window.paint_quad(fill(bounds, selected));
        }
        if let Some(marked) = self.edit.marked() {
            for bounds in self.spans(&lines, &ranges, &marked) {
                let underline = Bounds::new(
                    point(bounds.origin.x, bounds.bottom() - px(2.0)),
                    size(bounds.size.width, px(1.0)),
                );
                window.paint_quad(fill(underline, accent));
            }
        }
        let head = self.edit.position(self.edit.caret(), &ranges);
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

    /// The box's text as the text block it would become, measured and wrapped now.
    fn block(&self, window: &Window) -> TextBlock {
        self.origin
            .block(self.edit.text().to_owned(), |line| line_width(line, window))
            .expect("open checked the size, and the box keeps only text that measured")
    }

    /// The byte range of each line as the box lays its text out, wrapped lines included.
    fn line_ranges(&self, window: &Window) -> Vec<Range<usize>> {
        self.block(window).line_ranges().collect()
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
        self.edit
            .offset(LinePosition { line, byte }, &self.line_ranges(window))
    }

    /// Up or Down, where core sends it: to the character nearest the caret's x on that line.
    fn move_line(&mut self, step: LineStep, extend: bool, window: &Window) {
        let ranges = self.line_ranges(window);
        let offset = match self.edit.line_move(step, &ranges) {
            LineMove::ToOffset(offset) => offset,
            LineMove::ToLine(line) => {
                let lines = self.image_lines(window);
                let from = self.edit.position(self.edit.caret(), &ranges);
                let x = lines[from.line].shaped.x_for_index(from.byte);
                let byte = lines[line].shaped.closest_index_for_x(x);
                self.edit
                    .offset(LinePosition { line, byte }, &self.line_ranges(window))
            }
        };
        self.edit.move_to(offset, extend);
    }

    /// One rectangle per line that `range` covers, from its start to its end on that line.
    fn spans(
        &self,
        lines: &[LaidLine],
        ranges: &[Range<usize>],
        range: &Range<usize>,
    ) -> Vec<Bounds<Pixels>> {
        if range.is_empty() {
            return Vec::new();
        }
        let (start, end) = (
            self.edit.position(range.start, ranges),
            self.edit.position(range.end, ranges),
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
