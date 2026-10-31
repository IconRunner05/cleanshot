use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{
    Activity, BoxPress, Command, CommandError, Delivered, DeliveryError, Document, Drag, History,
    KeyAction, Modifiers, Output, Press, RgbaImage, ShapeId, Start, TextBlockError, TextEdit,
    TextKey, TextOrigin, Tool, action_for_key, delivery_notice,
};
use gpui::{
    App, AppContext, Bounds, Context, FocusHandle, InputHandler, InteractiveElement, IntoElement,
    KeyDownEvent, KeyUpEvent, ParentElement, Pixels, Point, Render, RenderImage, Role,
    SharedString, StatefulInteractiveElement, Styled, Subscription, TitlebarOptions,
    UTF16Selection, WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, div, px, size,
};

use crate::Error;
use crate::canvas::{OnPointer, Overlay, PaintOver, Pointer, document_canvas, render_image};
use crate::placement::{Fit, Placement};
use crate::text::register_inter;
use crate::text_box::TextBox;
use crate::theme::Theme;
use crate::toolbar::toolbar;

type Deliver = Rc<dyn Fn(Output, &Document) -> Result<Delivered, DeliveryError>>;

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
    text_box: Option<TextBox>,
    shift: bool,
    /// `None` while the image is fitted to the canvas, the default and Ctrl+0 (F-39).
    placement: Option<Placement>,
    space_held: bool,
    pan: Option<Pan>,
    /// Why the last edit was refused, until the next press.
    notice: Option<SharedString>,
    /// Copy and Save (F-40, D-48); absent in the bench window, which has nowhere to send them.
    deliver: Option<Deliver>,
    focus: FocusHandle,
    _appearance: Subscription,
    _activation: Subscription,
}

impl EditorView {
    fn new(
        document: Document,
        deliver: Option<Deliver>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // F-58: redraw in the new theme when the system switches between light and dark.
        let appearance = cx.observe_window_appearance(window, |_, _, cx| cx.notify());
        // D-46: Space released in another window never sends this one its key-up.
        let activation = cx.observe_window_activation(window, |view, window, cx| {
            if !window.is_window_active() {
                view.space_held = false;
                view.pan = None;
                cx.notify();
            }
        });
        Self {
            base: render_image(document.base()),
            document: Rc::new(document.clone()),
            history: History::new(document),
            tool: Tool::Select,
            selected: None,
            drag: None,
            text_box: None,
            shift: false,
            placement: None,
            space_held: false,
            pan: None,
            notice: None,
            deliver,
            focus,
            _appearance: appearance,
            _activation: activation,
        }
    }

    fn pointer(&mut self, pointer: Pointer, window: &Window, cx: &mut Context<Self>) {
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
                self.notice = None;
                if let Some(text_box) = &mut self.text_box {
                    match text_box.press(at, reach, window) {
                        BoxPress::PlaceCaret | BoxPress::Ignore => {
                            cx.notify();
                            return;
                        }
                        BoxPress::Commit => self.close_text_box(window),
                    }
                }
                let Press { selected, start } =
                    Drag::press(&self.document, self.tool, self.selected, at, reach);
                self.selected = selected;
                self.drag = None;
                match start {
                    Some(Start::Drag(drag)) => self.drag = Some(drag),
                    Some(Start::Type(origin)) => self.open_text_box(origin),
                    None => {}
                }
            }
            Pointer::Move { position, .. } if self.pan.is_some() => {
                self.placement = self
                    .pan
                    .map(|pan| pan.placement.panned(position - pan.from));
            }
            Pointer::Move { at, shift, .. } => {
                if let Some(text_box) = &mut self.text_box {
                    text_box.drag_to(at, window);
                    cx.notify();
                    return;
                }
                let Some(drag) = &mut self.drag else {
                    return;
                };
                drag.extend(at);
                self.shift = shift;
            }
            Pointer::Release { .. } if self.pan.is_some() => self.pan = None,
            Pointer::Release { shift } => {
                if let Some(text_box) = &mut self.text_box {
                    text_box.edit_mut().release();
                    return;
                }
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
        let activity = match (&self.text_box, &self.drag) {
            (Some(text_box), _) => text_box.edit().activity(),
            (None, Some(_)) => Activity::Dragging,
            (None, None) => Activity::Idle,
        };
        let Some(action) = action_for_key(&event.keystroke.key, modifiers, activity) else {
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
            KeyAction::Deliver(output) => self.deliver(output),
            KeyAction::Text(TextKey::Commit) => self.close_text_box(window),
            KeyAction::Text(key) => {
                if let Some(text_box) = &mut self.text_box {
                    text_box.key(key, window, cx);
                }
            }
        }
        cx.notify();
    }

    fn apply(&mut self, command: Command) {
        match self.history.apply(command) {
            Ok(()) => self.refresh(),
            Err(error) => self.notice = Some(refusal(&error).into()),
        }
    }

    fn open_text_box(&mut self, origin: TextOrigin) {
        match TextBox::open(origin) {
            Ok(text_box) => self.text_box = Some(text_box),
            Err(error) => self.notice = Some(text_refusal(&error).into()),
        }
    }

    /// F-34: the box's text becomes a shape in one undo step (F-38), and the box closes.
    fn close_text_box(&mut self, window: &Window) {
        let Some(text_box) = self.text_box.take() else {
            return;
        };
        match text_box.commit(window) {
            Ok(Some(command)) => self.apply(command),
            Ok(None) => {}
            Err(error) => self.notice = Some(text_refusal(&error).into()),
        }
    }

    fn deliver(&mut self, output: Output) {
        if let Some(deliver) = &self.deliver {
            self.notice = Some(delivery_notice(&deliver(output, self.history.document())).into());
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

fn text_refusal(error: &TextBlockError) -> &'static str {
    match error {
        TextBlockError::SizeNotPositive(_) => "That text has no size, so it cannot be edited.",
        TextBlockError::WidthNotMeasured(_) => {
            "The text could not be measured, so it was not changed."
        }
    }
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
        let on_pick = move |tool, window: &mut Window, cx: &mut App| {
            // Err only means the window closed, and with it the tool there was to pick.
            let _ = picker.update(cx, |view, cx| {
                view.close_text_box(window);
                view.tool = tool;
                cx.notify();
            });
        };
        let input = view.clone();
        let on_pointer: OnPointer = Rc::new(move |pointer, window, cx| {
            // Err only means the window closed, and with it everything the pointer could change.
            let _ = view.update(cx, |view, cx| view.pointer(pointer, window, cx));
        });
        let overlay = match &self.text_box {
            Some(text_box) => Overlay {
                preview: Some(text_box.preview(window)),
                selected: None,
                over: Some(paint_text_box(text_box.clone(), input, self.focus.clone())),
            },
            None => Overlay {
                preview: self.drag.as_ref().and_then(|drag| drag.command(self.shift)),
                selected: self.selected,
                over: None,
            },
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

/// F-34: draws the box's caret and selection over the canvas, and hands the window's text input
/// and input method to it while the editor has focus.
fn paint_text_box(
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

/// An open editor window. Dropping it leaves the window open; [`EditorWindow::close`] closes it.
pub struct EditorWindow(WindowHandle<EditorView>);

/// Opens an editor window on `capture`, with no shapes yet (F-23). Ctrl+C and Ctrl+S hand the
/// document to `deliver` and show what it reports (F-40, D-48).
///
/// # Errors
///
/// [`Error::WindowOpen`] if the platform cannot create the window.
pub fn open(
    cx: &mut App,
    capture: RgbaImage,
    deliver: impl Fn(Output, &Document) -> Result<Delivered, DeliveryError> + 'static,
) -> Result<EditorWindow, Error> {
    let document = Document::new(Arc::new(capture));
    open_with(cx, document, Some(Rc::new(deliver)), |_, _| {})
}

/// [`open`], running `on_open` on the new window before its first frame. `on_open` runs only
/// when the window opens, and then exactly once.
pub(crate) fn open_with(
    cx: &mut App,
    document: Document,
    deliver: Option<Deliver>,
    on_open: impl FnOnce(&mut Window, &mut App),
) -> Result<EditorWindow, Error> {
    register_inter(cx)?;
    let options = window_options(cx);
    cx.open_window(options, |window, cx| {
        on_open(window, cx);
        cx.new(|cx| EditorView::new(document, deliver, window, cx))
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
