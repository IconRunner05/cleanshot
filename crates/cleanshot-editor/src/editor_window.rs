use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{Document, RgbaImage};
use gpui::{
    App, AppContext, Bounds, Context, IntoElement, ParentElement, Render, RenderImage, Styled,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div, px, size,
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

/// Opens an editor window on `capture`, with no shapes yet (F-23).
///
/// # Errors
///
/// [`Error::WindowOpen`] if the platform cannot create the window.
pub fn open(cx: &mut App, capture: RgbaImage) -> Result<EditorWindow, Error> {
    open_with(cx, Document::new(Arc::new(capture)), |_, _| {})
}

/// [`open`], running `on_open` on the new window before its first frame. `on_open` runs only
/// when the window opens, and then exactly once.
pub(crate) fn open_with(
    cx: &mut App,
    document: Document,
    on_open: impl FnOnce(&mut Window, &mut App),
) -> Result<EditorWindow, Error> {
    let base = render_image(document.base());
    let document = Rc::new(document);
    let options = window_options(cx);
    cx.open_window(options, |window, cx| {
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

/// D-41: the editor opens centred, filling most of the primary display, under the native
/// title bar so the Windows caption buttons and Snap behave as everywhere else (CHARTER 4).
fn window_options(cx: &App) -> WindowOptions {
    let window_size = cx
        .primary_display()
        .map_or(size(px(1280.0), px(800.0)), |display| {
            display.visible_bounds().size.map(|side| side * 0.8)
        });
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            window_size,
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Cleanshot".into()),
            ..TitlebarOptions::default()
        }),
        window_min_size: Some(size(px(480.0), px(320.0))),
        ..WindowOptions::default()
    }
}
