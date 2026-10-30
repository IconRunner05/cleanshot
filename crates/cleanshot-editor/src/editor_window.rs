use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::Document;
use gpui::{
    App, AppContext, Context, IntoElement, ParentElement, Render, RenderImage, Styled, Window,
    WindowHandle, WindowOptions, div,
};

use crate::Error;
use crate::canvas::{document_canvas, render_image};

struct EditorView {
    document: Rc<Document>,
    base: Arc<RenderImage>,
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(document_canvas(Rc::clone(&self.document), Arc::clone(&self.base)).size_full())
    }
}

/// An open editor window. Dropping it leaves the window open; [`EditorWindow::close`] closes it.
pub struct EditorWindow(WindowHandle<EditorView>);

/// Opens an editor window showing `document`.
///
/// # Errors
///
/// [`Error::WindowOpen`] if the platform cannot create the window.
pub fn open(cx: &mut App, document: Document) -> Result<EditorWindow, Error> {
    open_with(cx, document, |_, _| {})
}

/// [`open`], running `on_open` on the new window before its first frame. `on_open` runs only
/// when the window opens, and then exactly once.
pub(crate) fn open_with(
    cx: &mut App,
    document: Document,
    on_open: impl FnOnce(&mut Window, &mut App),
) -> Result<EditorWindow, Error> {
    let base = render_image(&document.base);
    let document = Rc::new(document);
    cx.open_window(WindowOptions::default(), |window, cx| {
        on_open(window, cx);
        cx.new(|_| EditorView { document, base })
    })
    .map(EditorWindow)
    .map_err(|error| Error::WindowOpen(error.into()))
}

impl EditorWindow {
    /// Closes the window. A window the user already closed is left as it is.
    pub fn close(self, cx: &mut App) {
        // Err only means the window is already gone, which is the state close asks for.
        let _ = self.0.update(cx, |_, window, _| window.remove_window());
    }
}
