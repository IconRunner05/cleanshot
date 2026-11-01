use std::ops::Range;

use cleanshot_core::TextEdit;
use gpui::{
    App, Bounds, FocusHandle, InputHandler, Pixels, Point, UTF16Selection, WeakEntity, Window,
};

use super::EditorView;
use crate::canvas::PaintOver;
use crate::placement::Fit;
use crate::text_box::TextBox;

/// F-34: draws the box's caret and selection over the canvas, and hands the window's text input
/// and input method to it while the editor has focus.
pub(super) fn paint_text_box(
    text_box: TextBox,
    view: WeakEntity<EditorView>,
    focus: FocusHandle,
) -> PaintOver {
    Box::new(move |fit, window, cx| {
        text_box.paint(fit, window);
        window.handle_input(&focus, BoxInput { view, fit: *fit }, cx);
    })
}

/// The open text box as GPUI's text input and input method see it, in UTF-16 offsets.
struct BoxInput {
    view: WeakEntity<EditorView>,
    fit: Fit,
}

impl BoxInput {
    fn read<R>(&self, cx: &App, read: impl FnOnce(&TextBox) -> R) -> Option<R> {
        self.view.upgrade()?.read(cx).text_box.as_ref().map(read)
    }

    fn change(&self, cx: &mut App, change: impl FnOnce(&mut TextEdit)) {
        // Err only means the window closed, and with it the box being typed into.
        let _ = self.view.update(cx, |view, cx| {
            if let Some(text_box) = &mut view.text_box {
                change(text_box.edit_mut());
                cx.notify();
            }
        });
    }
}

impl InputHandler for BoxInput {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        self.read(cx, TextBox::selected)
    }

    fn marked_text_range(&mut self, _: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.read(cx, |text_box| text_box.edit().marked()).flatten()
    }

    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        let (text, range) = self.read(cx, |text_box| text_box.edit().text_for_range(range))?;
        *adjusted = Some(range);
        Some(text)
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut App,
    ) {
        self.change(cx, |edit| edit.replace(range, text));
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut App,
    ) {
        self.change(cx, |edit| edit.replace_and_mark(range, text, selected));
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut App) {
        self.change(cx, TextEdit::unmark);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.read(cx, |text_box| {
            text_box.bounds_for(&range, &self.fit, window)
        })
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        self.read(cx, |text_box| {
            text_box.offset_at_screen(point, &self.fit, window)
        })
    }
}
