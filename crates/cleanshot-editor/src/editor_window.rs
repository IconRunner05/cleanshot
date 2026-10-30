use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{
    Command, CommandError, Document, Drag, History, KeyAction, Modifiers, Press, RgbaImage,
    ShapeId, Tool, action_for_key,
};
use gpui::{
    App, AppContext, Bounds, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    KeyUpEvent, ParentElement, Pixels, Point, Render, RenderImage, Role, SharedString,
    StatefulInteractiveElement, Styled, Subscription, TitlebarOptions, Window, WindowBounds,
    WindowHandle, WindowOptions, div, px, size,
};

use crate::Error;
use crate::canvas::{OnPointer, Overlay, Pointer, document_canvas, render_image};
use crate::placement::Placement;
use crate::theme::Theme;
use crate::toolbar::toolbar;

/// A Space-drag in progress: where it started and where the image was then.
#[derive(Clone, Copy)]
struct Pan {
    from: Point<Pixels>,
    placement: Placement,
}

struct EditorView {
    history: History,
    /// The history's document as last drawn; renewed after each edit.
    document: Rc<Document>,
    base: Arc<RenderImage>,
    tool: Tool,
    selected: Option<ShapeId>,
    drag: Option<Drag>,
    shift: bool,
    /// `None` while the image is fitted to the canvas, the default and Ctrl+0 (F-39).
    placement: Option<Placement>,
    space_held: bool,
    pan: Option<Pan>,
    /// Why the last edit was refused, until the next press.
    notice: Option<SharedString>,
    focus: FocusHandle,
    _appearance: Subscription,
}

impl EditorView {
    fn new(document: Document, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // F-58: redraw in the new theme when the system switches between light and dark.
        let appearance = cx.observe_window_appearance(window, |_, _, cx| cx.notify());
        Self {
            base: render_image(document.base()),
            document: Rc::new(document.clone()),
            history: History::new(document),
            tool: Tool::Select,
            selected: None,
            drag: None,
            shift: false,
            placement: None,
            space_held: false,
            pan: None,
            notice: None,
            focus,
            _appearance: appearance,
        }
    }

    fn pointer(&mut self, pointer: Pointer, cx: &mut Context<Self>) {
        match pointer {
            Pointer::Press {
                position,
                placement,
                ..
            } if self.space_held => {
                self.pan = Some(Pan {
                    from: position,
                    placement,
                });
            }
            Pointer::Press { at, reach, .. } => {
                let Press { selected, drag } =
                    Drag::press(&self.document, self.tool, self.selected, at, reach);
                (self.selected, self.drag, self.notice) = (selected, drag, None);
            }
            Pointer::Move { position, .. } if self.pan.is_some() => {
                self.placement = self
                    .pan
                    .map(|pan| pan.placement.panned(position - pan.from));
            }
            Pointer::Move { at, shift, .. } => {
                let Some(drag) = &mut self.drag else {
                    return;
                };
                drag.extend(at);
                self.shift = shift;
            }
            Pointer::Release { .. } if self.pan.is_some() => self.pan = None,
            Pointer::Release { shift } => {
                let Some(drag) = self.drag.take() else {
                    return;
                };
                if let Some(command) = drag.command(shift) {
                    self.apply(command);
                }
            }
            Pointer::Zoom {
                factor,
                around,
                placement,
            } => self.placement = Some(placement.zoomed(factor, around)),
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let gpui = event.keystroke.modifiers;
        let modifiers = Modifiers {
            ctrl: gpui.control,
            alt: gpui.alt,
            shift: gpui.shift,
            win: gpui.platform,
        };
        let dragging = self.drag.is_some();
        let Some(action) = action_for_key(&event.keystroke.key, modifiers, dragging) else {
            return;
        };
        match action {
            KeyAction::Undo => self.undo_or_redo(History::undo),
            KeyAction::Redo => self.undo_or_redo(History::redo),
            KeyAction::RemoveSelected => {
                if let Some(id) = self.selected.take() {
                    self.apply(Command::Remove(id));
                }
            }
            KeyAction::ClearSelection => self.selected = None,
            KeyAction::PickTool(tool) => self.tool = tool,
            KeyAction::FitView => self.placement = None,
            KeyAction::HoldToPan => self.space_held = true,
            KeyAction::FocusNext => window.focus_next(cx),
            KeyAction::FocusPrevious => window.focus_prev(cx),
        }
        cx.notify();
    }

    fn apply(&mut self, command: Command) {
        match self.history.apply(command) {
            Ok(()) => self.refresh(),
            Err(error) => self.notice = Some(refusal(&error).into()),
        }
    }

    fn undo_or_redo(&mut self, change: fn(&mut History) -> Result<bool, CommandError>) {
        match change(&mut self.history) {
            Ok(true) => self.refresh(),
            Ok(false) => {}
            Err(error) => self.notice = Some(refusal(&error).into()),
        }
    }

    fn refresh(&mut self) {
        self.document = Rc::new(self.history.document().clone());
        let document = &self.document;
        self.selected = self.selected.filter(|id| document.contains(*id));
    }
}

fn notice_panel(text: SharedString, theme: Theme) -> impl IntoElement {
    div()
        .id("notice")
        .role(Role::Status)
        .aria_label(text.clone())
        .absolute()
        .bottom(px(16.0))
        .left(px(16.0))
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(7.0))
        .bg(theme.material)
        .border_1()
        .border_color(theme.hairline)
        .text_color(theme.text)
        .text_size(px(13.0))
        .child(text)
}

fn refusal(error: &CommandError) -> &'static str {
    match error {
        CommandError::UnknownShape(_) => {
            "That shape is no longer in the image, so nothing changed."
        }
        CommandError::CropOutsideImage(_) => {
            "A crop needs an area inside the image, so the crop was not changed."
        }
    }
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_appearance(window.appearance());
        let view = cx.entity().downgrade();
        let picker = view.clone();
        let on_pick = move |tool, _: &mut Window, cx: &mut App| {
            // Err only means the window closed, and with it the tool there was to pick.
            let _ = picker.update(cx, |view, cx| {
                view.tool = tool;
                cx.notify();
            });
        };
        let on_pointer: OnPointer = Rc::new(move |pointer, _, cx| {
            // Err only means the window closed, and with it everything the pointer could change.
            let _ = view.update(cx, |view, cx| view.pointer(pointer, cx));
        });
        let overlay = Overlay {
            preview: self.drag.as_ref().and_then(|drag| drag.command(self.shift)),
            selected: self.selected,
        };
        let canvas = document_canvas(
            Rc::clone(&self.document),
            Arc::clone(&self.base),
            self.placement,
            overlay,
            on_pointer,
        );
        div()
            .id("editor")
            .role(Role::Application)
            .aria_label("Cleanshot editor")
            .size_full()
            .relative()
            .bg(theme.backdrop)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(|view, event: &KeyUpEvent, _, _| {
                if event.keystroke.key == "space" {
                    view.space_held = false;
                }
            }))
            .child(
                div()
                    .id("screenshot")
                    .role(Role::Image)
                    .aria_label("Screenshot")
                    .size_full()
                    .child(canvas.size_full()),
            )
            .child(toolbar(self.tool, theme, on_pick))
            .children(
                self.notice
                    .clone()
                    .map(|notice| notice_panel(notice, theme)),
            )
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
    let options = window_options(cx);
    cx.open_window(options, |window, cx| {
        on_open(window, cx);
        cx.new(|cx| EditorView::new(document, window, cx))
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
