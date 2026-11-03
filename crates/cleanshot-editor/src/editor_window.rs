mod text_input;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use cleanshot_core::{
    Activity, BoxPress, Command, CommandError, Delivered, DeliveryError, Document, Drag, History,
    KeyAction, Looks, Modifiers, Monitor, Output, PointF, Press, Property, PropertyError,
    RgbaImage, Setting, ShapeId, Start, TextBlockError, TextKey, TextOrigin, Tool, action_for_key,
    delivery_notice, save_as_notice,
};
use gpui::{
    App, AppContext, Bounds, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, KeyUpEvent, ParentElement, Pixels, Point, Render, RenderImage, Role,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Task, TitlebarOptions,
    WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, div, point, px, size,
};

use crate::canvas::{
    OnPointer, Overlay, Pointer, RedactionPreviews, document_canvas, render_image,
};
use crate::placement::{MIN_WINDOW, Placement, opening_size};
use crate::strip::property_strip;
use crate::text::register_inter;
use crate::text_box::TextBox;
use crate::theme::Theme;
use crate::toolbar::toolbar;
use crate::{Error, Log};
use text_input::paint_text_box;

type Deliver = Box<dyn Fn(Output, &Document) -> Result<Delivered, DeliveryError>>;
type SaveTo = Box<dyn Fn(&Path, &Document) -> Result<Delivered, DeliveryError>>;

/// Where Copy, Save and Save As send the edited screenshot (F-20 to F-22, F-40), from the app.
pub struct Outputs {
    deliver: Deliver,
    save_as_start: Box<dyn Fn() -> Result<(PathBuf, String), DeliveryError>>,
    save_to: SaveTo,
}

impl Outputs {
    /// Ctrl+C and Ctrl+S hand the document to `deliver` and show what it reports (F-40, D-48).
    /// Ctrl+Shift+S opens the save dialog on the folder and file name `save_as_start` gives at
    /// that moment, or shows why it cannot, and hands the document and the confirmed path to
    /// `save_to` (F-22).
    pub fn new(
        deliver: impl Fn(Output, &Document) -> Result<Delivered, DeliveryError> + 'static,
        save_as_start: impl Fn() -> Result<(PathBuf, String), DeliveryError> + 'static,
        save_to: impl Fn(&Path, &Document) -> Result<Delivered, DeliveryError> + 'static,
    ) -> Self {
        Self {
            deliver: Box::new(deliver),
            save_as_start: Box::new(save_as_start),
            save_to: Box::new(save_to),
        }
    }
}

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
    /// How each tool draws, as the property strip sets it; not part of the document.
    looks: Looks,
    redactions: Rc<RedactionPreviews>,
    log: Log,
    shift: bool,
    /// `None` while the image is fitted to the canvas, the default and Ctrl+0 (F-39).
    placement: Option<Placement>,
    space_held: bool,
    pan: Option<Pan>,
    /// Why the last edit was refused, until the next press.
    notice: Option<SharedString>,
    /// Absent in the bench window, which has nowhere to send the screenshot.
    outputs: Option<Outputs>,
    /// The Save As dialog while it is open (F-22).
    save_as: Option<Task<()>>,
    focus: FocusHandle,
    _appearance: Subscription,
    _activation: Subscription,
}

impl EditorView {
    fn new(
        document: Document,
        outputs: Option<Outputs>,
        log: Log,
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
            looks: Looks::default(),
            redactions: Rc::new(RedactionPreviews::new(Rc::clone(&log))),
            log,
            shift: false,
            placement: None,
            space_held: false,
            pan: None,
            notice: None,
            outputs,
            save_as: None,
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
                let look = self.looks.look(self.tool);
                let Press { selected, start } =
                    Drag::press(&self.document, self.tool, look, self.selected, at, reach);
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
        let activity = match (&self.save_as, &self.text_box, &self.drag) {
            (Some(_), _, _) => Activity::SavingAs,
            (None, Some(text_box), _) => text_box.edit().activity(),
            (None, None, Some(_)) => Activity::Dragging,
            (None, None, None) => Activity::Idle,
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
            KeyAction::BackToSelect => {
                self.selected = None;
                self.tool = Tool::Select;
            }
            KeyAction::PickTool(tool) => self.tool = tool,
            KeyAction::FitView => self.placement = None,
            KeyAction::HoldToPan => self.space_held = true,
            KeyAction::FocusNext => window.focus_next(cx),
            KeyAction::FocusPrevious => window.focus_prev(cx),
            KeyAction::Deliver(output) => self.deliver(output),
            KeyAction::SaveAs => self.save_as(window, cx),
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

    /// A strip change (D-55): on the open text box, else on the selection, else on how the
    /// current tool draws next.
    fn set_property(&mut self, property: Property) {
        let restyled = if let Some(text_box) = &mut self.text_box {
            text_box.restyle(&mut self.looks, property)
        } else {
            let selected = self.selected.and_then(|id| self.document.shape(id));
            self.looks.set(property, self.tool, selected)
        };
        match restyled {
            Ok(Some(command)) => self.apply(command),
            Ok(None) => {}
            Err(PropertyError::NotPositive(_)) => {
                self.notice = Some("A size must be more than 0, so nothing changed.".into());
            }
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
        if let Some(outputs) = &self.outputs {
            let delivered = (outputs.deliver)(output, self.history.document());
            self.notice = Some(delivery_notice(&delivered).into());
        }
    }

    /// F-22: the dialog runs after this key's dispatch, as GPUI's own task, so it never
    /// re-enters GPUI from inside an event.
    fn save_as(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(outputs) = &self.outputs else {
            return;
        };
        let (folder, name) = match (outputs.save_as_start)() {
            Ok(start) => start,
            Err(error) => {
                self.notice = Some(delivery_notice(&Err(error)).into());
                return;
            }
        };
        let log = Rc::clone(&self.log);
        let dialog = ask_save_path(cx, &folder, &name, move |event| log(event));
        self.save_as = Some(cx.spawn_in(window, async move |view, cx| {
            let chosen = dialog.await;
            // Err only means the window closed while the dialog was open, with nothing to save.
            let _ = view.update(cx, |view, cx| view.save_to(chosen, cx));
        }));
    }

    fn save_to(&mut self, chosen: Option<Result<PathBuf, DeliveryError>>, cx: &mut Context<Self>) {
        self.save_as = None;
        let Some(outputs) = &self.outputs else {
            return;
        };
        let document = self.history.document();
        if let Some(notice) = save_as_notice(chosen, |path| (outputs.save_to)(path, document)) {
            self.notice = Some(notice.into());
            cx.notify();
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

impl EditorView {
    /// The property strip for what the next strip change would restyle, and the frame toggles.
    fn strip(&self, view: &WeakEntity<Self>, theme: Theme) -> impl IntoElement + use<> {
        let setter = view.clone();
        let on_set = move |property, window: &mut Window, cx: &mut App| {
            // Err only means the window closed, and with it what the strip could change.
            let _ = setter.update(cx, |view, cx| {
                view.set_property(property);
                if view.text_box.is_some() {
                    window.focus(&view.focus, cx);
                }
                cx.notify();
            });
        };
        let framer = view.clone();
        let on_frame = move |frame, _: &mut Window, cx: &mut App| {
            // Err only means the window closed, and with it the frame there was to change.
            let _ = framer.update(cx, |view, cx| {
                view.apply(Command::SetFrame(frame));
                cx.notify();
            });
        };
        let (strip_tool, look) = if let Some(text_box) = &self.text_box {
            (text_box.tool(), text_box.look(&self.looks))
        } else {
            let selected = self.selected.and_then(|id| self.document.shape(id));
            self.looks.shown(self.tool, selected)
        };
        let settings = Setting::for_tool(strip_tool);
        property_strip(
            settings,
            look,
            self.document.frame(),
            theme,
            on_set,
            on_frame,
        )
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
        let strip = self.strip(&view, theme);
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
        Rc::make_mut(&mut self.redactions).update(&self.document);
        let canvas = document_canvas(
            Rc::clone(&self.document),
            Arc::clone(&self.base),
            self.placement,
            Rc::clone(&self.redactions),
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
            .child(
                div()
                    .absolute()
                    .top(px(16.0))
                    .left_0()
                    .right_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .child(toolbar(self.tool, theme, on_pick))
                    .child(strip),
            )
            .children(
                self.notice
                    .clone()
                    .map(|notice| notice_panel(notice, theme)),
            )
    }
}

/// F-22: opens the Save As dialog on `folder`, offering `name`. The answer is the path the
/// user confirmed, [`DeliveryError::NoSaveDialog`] if the dialog failed, or `None` for a cancel.
/// Why it failed goes to `log`.
pub fn ask_save_path<L: Fn(&'static str) + 'static>(
    cx: &App,
    folder: &Path,
    name: &str,
    log: L,
) -> impl Future<Output = Option<Result<PathBuf, DeliveryError>>> + use<L> {
    let dialog = cx.prompt_for_new_path(folder, Some(name));
    async move {
        match dialog.await {
            Ok(Ok(chosen)) => chosen.map(Ok),
            Ok(Err(_)) => {
                log("save_dialog not_opened");
                Some(Err(DeliveryError::NoSaveDialog))
            }
            Err(_) => {
                log("save_dialog no_answer");
                Some(Err(DeliveryError::NoSaveDialog))
            }
        }
    }
}

/// An open editor window. Dropping it leaves the window open; [`EditorWindow::close`] closes it.
pub struct EditorWindow(WindowHandle<EditorView>);

/// Opens an editor window on `capture`, with no shapes yet (F-23), sized round it and centred
/// on `monitor`, the one it came from, or on the primary display for `None` (D-110). Copy, Save
/// and Save As go to `outputs`. `on_first_frame` gets the moment the frame after the window's
/// first draw begins (P-07), as the bench times it.
/// What fails out of sight, such as a redaction preview, goes to `log`.
///
/// # Errors
///
/// [`Error::WindowOpen`] if the platform cannot create the window.
pub fn open(
    cx: &mut App,
    capture: RgbaImage,
    monitor: Option<Monitor>,
    outputs: Outputs,
    on_first_frame: impl FnOnce(Instant) + 'static,
    log: impl Fn(&'static str) + 'static,
) -> Result<EditorWindow, Error> {
    let document = Document::new(Arc::new(capture));
    open_with(
        cx,
        document,
        monitor,
        Some(outputs),
        Rc::new(log),
        move |window, _| {
            window.on_next_frame(move |_, _| on_first_frame(Instant::now()));
        },
    )
}

/// [`open`], running `on_open` on the new window before its first frame. `on_open` runs only
/// when the window opens, and then exactly once.
pub(crate) fn open_with(
    cx: &mut App,
    document: Document,
    monitor: Option<Monitor>,
    outputs: Option<Outputs>,
    log: Log,
    on_open: impl FnOnce(&mut Window, &mut App),
) -> Result<EditorWindow, Error> {
    register_inter(cx)?;
    let options = window_options(cx, document.base(), monitor);
    cx.open_window(options, |window, cx| {
        on_open(window, cx);
        cx.new(|cx| EditorView::new(document, outputs, log, window, cx))
    })
    .map(EditorWindow)
    .map_err(|error| Error::WindowOpen(error.into()))
}

/// P-08's bench: draws the editor in `window` at `placement`, where a pan or zoom (F-39) would
/// leave it, or fitted for `None`.
pub(crate) fn show_at(window: &Window, cx: &mut App, placement: Option<Placement>) {
    bench_view(window).update(cx, |view, cx| {
        view.placement = placement;
        cx.notify();
    });
}

/// P-08's bench: the redaction tool dragged in `window` as the pointer drives it, pressed at
/// `from` the first time and moved to `to`, in image pixels, with the image at `placement`.
pub(crate) fn drag_redaction(
    window: &Window,
    cx: &mut App,
    from: PointF,
    to: PointF,
    placement: Placement,
) {
    let position = point(px(0.0), px(0.0));
    bench_view(window).update(cx, |view, cx| {
        if view.drag.is_none() {
            view.tool = Tool::Redact;
            let press = Pointer::Press {
                at: from,
                reach: 0.0,
                position,
                placement,
            };
            view.pointer(press, window, cx);
        }
        let shift = false;
        view.pointer(
            Pointer::Move {
                at: to,
                shift,
                position,
            },
            window,
            cx,
        );
    });
}

fn bench_view(window: &Window) -> Entity<EditorView> {
    window
        .root::<EditorView>()
        .flatten()
        .expect("the bench draws only in a window open_with opened on an editor")
}

impl EditorWindow {
    /// Closes the window. A window the user already closed is left as it is.
    pub fn close(self, cx: &mut App) {
        // Err only means the window is already gone, which is the state close asks for.
        let _ = self.0.update(cx, |_, window, _| window.remove_window());
    }
}

/// D-110: the editor opens sized round `image` and centred on `monitor`'s display, under the
/// native title bar so the Windows caption buttons and Snap behave as everywhere else
/// (CHARTER 4). With no monitor, or none GPUI knows, it opens on the primary display, sized
/// as at 100% scaling.
fn window_options(cx: &App, image: &RgbaImage, monitor: Option<Monitor>) -> WindowOptions {
    let display = monitor
        .and_then(|monitor| {
            cx.displays()
                .into_iter()
                .find(|display| shows(display.visible_bounds().origin, monitor))
        })
        .or_else(|| cx.primary_display());
    let display_id = display.as_ref().map(|display| display.id());
    let work_area = display.map_or(size(px(1280.0), px(800.0)), |display| {
        display.visible_bounds().size
    });
    let scale = monitor.map_or(1.0, |monitor| monitor.scale);
    let window_size = opening_size(image, scale, work_area);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            display_id,
            window_size,
            cx,
        ))),
        display_id,
        titlebar: Some(TitlebarOptions {
            title: Some("Cleanshot".into()),
            ..TitlebarOptions::default()
        }),
        window_min_size: Some(MIN_WINDOW),
        ..WindowOptions::default()
    }
}

/// Whether a display whose work area starts at `origin`, in DIPs, is `monitor`: GPUI places a
/// display's work area at its physical corner divided by its scale.
fn shows(origin: Point<Pixels>, monitor: Monitor) -> bool {
    #[expect(
        clippy::cast_precision_loss,
        reason = "desktop coordinates are far below 2^24, where f32 stops being exact"
    )]
    let near =
        |at: Pixels, physical: i32| (f32::from(at) - physical as f32 / monitor.scale).abs() < 0.5;
    near(origin.x, monitor.work_area.left) && near(origin.y, monitor.work_area.top)
}
