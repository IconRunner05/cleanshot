mod card;
mod input;
mod text_input;

use std::num::NonZeroIsize;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use cleanshot_core::{
    Color, ColorValue, Command, CommandError, Cropping, Delivered, DeliveryError, Document, Drag,
    History, KeyAction, Looks, Monitor, Output, PointF, Property, PropertyError, Restack,
    RgbaImage, ShapeId, TextBlockError, TextMasks, TextOrigin, Tool, delivery_notice,
    save_as_notice,
};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Axis, Bounds, Context, Entity, FocusHandle, Global,
    InteractiveElement, IntoElement, KeyUpEvent, ParentElement, Pixels, Point, Render, RenderImage,
    Role, SharedString, StatefulInteractiveElement, Styled, Subscription, Task, TitlebarOptions,
    WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, div, point, px, size,
};

use self::card::image_card;
use crate::canvas::{OnPointer, Overlay, Pointer, Previews, document_canvas, render_image};
use crate::caption::{CaptionRow, min_width};
use crate::placement::{MIN_HEIGHT, Placement, opening_size};
use crate::settings_window::settings_opener;
use crate::shape_menu::ShapeMenu;
use crate::strip::{StripGroup, crop_strip, property_strip};
use crate::text::{line_width, register_inter};
use crate::text_box::TextBox;
use crate::theme::{Theme, WindowSurface, window_surface};
use crate::toolbar::{
    Replay, ToolbarMenu, ToolbarMenus, actions, group_rows, more_rows, show_tool, shown_tools,
    toolbar,
};
use crate::tooltip::hover_time;
use crate::{Error, Log, MasksFor, open_window};
use text_input::paint_text_box;

/// The looks each new editor starts with, and who is told when an editor's strip changes them.
struct RememberedLooks {
    looks: Looks,
    on_changed: OnLooksChanged,
}

type OnLooksChanged = Rc<dyn Fn(&Looks, &mut App)>;

impl Global for RememberedLooks {}

/// Starts each editor opened from now on with `looks`, and calls `on_changed` with an editor's
/// looks each time its property strip changes them, which later editors then start with (F-59).
/// Replaces what an earlier call gave.
pub fn remember_looks(cx: &mut App, looks: Looks, on_changed: impl Fn(&Looks, &mut App) + 'static) {
    cx.set_global(RememberedLooks {
        looks,
        on_changed: Rc::new(on_changed),
    });
}

fn remembered_looks(cx: &App) -> Looks {
    cx.try_global::<RememberedLooks>()
        .map(|remembered| remembered.looks.clone())
        .unwrap_or_default()
}

fn looks_changed(looks: &Looks, cx: &mut App) {
    if !cx.has_global::<RememberedLooks>() {
        return;
    }
    let remembered = cx.global_mut::<RememberedLooks>();
    remembered.looks = looks.clone();
    let on_changed = Rc::clone(&remembered.on_changed);
    on_changed(looks, cx);
}

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

/// What a press held on the zoom controls moves (D-145, D-160).
#[derive(Clone, Copy, PartialEq)]
enum Held {
    Minimap,
    /// A scrollbar's thumb, pressed at `from` in the window with the image at `placement`.
    Bar {
        axis: Axis,
        from: Point<Pixels>,
        placement: Placement,
    },
}

/// A Space-drag in progress: where it started and where the image was then.
#[derive(Clone, Copy)]
struct Pan {
    from: Point<Pixels>,
    placement: Placement,
}

struct EditorView {
    history: History,
    /// What the window opened on; kept until it closes (D-142).
    surface: WindowSurface,
    /// The history's document as last drawn, with the Crop tool's draft crop; renewed after
    /// each edit.
    document: Rc<Document>,
    base: Arc<RenderImage>,
    tool: Tool,
    /// Set while the Crop tool is picked, and only then (D-159).
    cropping: Option<Cropping>,
    selected: Option<ShapeId>,
    drag: Option<Drag>,
    text_box: Option<TextBox>,
    /// The tool groups' and "..." buttons, and their open menu (D-143, D-144).
    toolbar_menus: ToolbarMenus,
    /// The right-click menu while it is open (D-115).
    menu: Option<ShapeMenu>,
    /// How each tool draws, as the property strip sets it; not part of the document.
    looks: Looks,
    previews: Rc<Previews>,
    log: Log,
    shift: bool,
    /// `None` while the image is fitted to the canvas, the default and Ctrl+0 (F-39).
    placement: Option<Placement>,
    /// The capture's average colour, which glows round the image card (D-146).
    glow: Color,
    /// While a press on the minimap or a scrollbar is held, its drags pan the view (D-145,
    /// D-160).
    held: Option<Held>,
    /// After the picker's "From image", the colour setting the next press on the screenshot
    /// sets (D-147).
    sampling: Option<ColorValue>,
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
        masks: MasksFor,
        log: Log,
        surface: WindowSurface,
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
            glow: document.base().average_color(),
            base: render_image(document.base()),
            document: Rc::new(document.clone()),
            history: History::new(document),
            tool: Tool::Select,
            cropping: None,
            selected: None,
            drag: None,
            text_box: None,
            menu: None,
            looks: remembered_looks(cx),
            toolbar_menus: ToolbarMenus::new(cx),
            surface,
            previews: Rc::new(Previews::new(masks, Rc::clone(&log))),
            log,
            shift: false,
            placement: None,
            held: None,
            sampling: None,
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

    fn apply(&mut self, command: Command) {
        // D-113: a new crop is shown fitted, whatever the zoom was.
        let refit = matches!(command, Command::SetCrop(_));
        let for_history = match &mut self.cropping {
            Some(cropping) => cropping.take(command, self.history.document()),
            None => Ok(Some(command)),
        };
        let applied = for_history.and_then(|command| match command {
            Some(command) => self.history.apply(command),
            None => Ok(()),
        });
        match applied {
            Ok(()) => {
                if refit {
                    self.placement = None;
                }
                self.refresh();
            }
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
            let delivered = (outputs.deliver)(output, &self.document);
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
        let closed = Rc::clone(&self.log);
        let dialog = ask_save_path(cx, &folder, &name, move |event| log(event));
        self.save_as = Some(cx.spawn_in(window, async move |view, cx| {
            let chosen = dialog.await;
            if view
                .update(cx, |view, cx| view.save_to(chosen, cx))
                .is_err()
            {
                closed("save_as window_closed");
            }
        }));
    }

    fn save_to(&mut self, chosen: Option<Result<PathBuf, DeliveryError>>, cx: &mut Context<Self>) {
        self.save_as = None;
        let Some(outputs) = &self.outputs else {
            return;
        };
        let document = &self.document;
        if let Some(notice) = save_as_notice(chosen, |path| (outputs.save_to)(path, document)) {
            self.notice = Some(notice.into());
            cx.notify();
        }
    }

    fn undo_or_redo(&mut self, change: Replay) {
        match change(&mut self.history) {
            Ok(true) => self.refresh(),
            Ok(false) => {}
            Err(error) => self.notice = Some(refusal(&error).into()),
        }
    }

    /// Makes `tool` the current tool, shown on its group's button, any open menu closed
    /// (D-143).
    fn pick_tool(&mut self, tool: Tool, window: &mut Window, cx: &mut App) {
        // D-159: leaving the Crop tool applies its crop.
        if tool != Tool::Crop
            && let Some(cropping) = self.cropping.take()
            && let Some(command) = cropping.applied(self.history.document())
        {
            self.apply(command);
        }
        if tool == Tool::Crop && self.cropping.is_none() {
            self.cropping = Some(Cropping::start(self.tool, self.history.document()));
        }
        self.tool = tool;
        self.refresh();
        show_tool(tool, cx);
        self.toolbar_menus.close(window, cx);
    }

    /// Closes the right-click menu, moving its shape by `picked` if a move was picked (D-115).
    fn close_menu(&mut self, picked: Option<Restack>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((menu, to)) = self.menu.take().zip(picked) {
            self.apply(Command::Restack { id: menu.id, to });
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn refresh(&mut self) {
        let history = self.history.document();
        self.document = Rc::new(match self.cropping {
            Some(cropping) => cropping.shown(history),
            None => history.clone(),
        });
        let document = &self.document;
        self.selected = self.selected.filter(|id| document.contains(*id));
        self.menu = self.menu.take().filter(|menu| document.contains(menu.id));
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
    /// The toolbar: the tools, Undo and Redo, and Copy, Save and Save As, each closing an open
    /// text box first.
    fn caption_controls(
        &self,
        view: &WeakEntity<Self>,
        theme: Theme,
        cx: &App,
    ) -> (impl IntoElement + use<>, impl IntoElement + use<>) {
        let picker = view.clone();
        let on_pick = move |tool, window: &mut Window, cx: &mut App| {
            update_while_open(&picker, cx, |view, cx| {
                view.close_text_box(window);
                view.pick_tool(tool, window, cx);
                cx.notify();
            });
        };
        let opener = view.clone();
        let on_menu = move |menu: Option<ToolbarMenu>, window: &mut Window, cx: &mut App| {
            update_while_open(&opener, cx, |view, cx| {
                let Some(menu) = menu else {
                    view.toolbar_menus.close(window, cx);
                    cx.notify();
                    return;
                };
                view.close_text_box(window);
                let rows = match menu {
                    ToolbarMenu::Tools(group) => group_rows(group, view.tool),
                    ToolbarMenu::More => more_rows(&view.history, settings_opener(cx).is_some()),
                };
                view.toolbar_menus.open(menu, &rows, window, cx);
                cx.notify();
            });
        };
        let actor = view.clone();
        let on_action = move |action: KeyAction, window: &mut Window, cx: &mut App| {
            update_while_open(&actor, cx, |view, cx| {
                view.toolbar_menus.close(window, cx);
                view.close_text_box(window);
                view.act(action, window, cx);
                cx.notify();
            });
        };
        let tools = toolbar(
            self.tool,
            shown_tools(cx),
            &self.toolbar_menus,
            theme,
            on_pick,
            on_menu.clone(),
        );
        let actions = actions(
            &self.toolbar_menus,
            &more_rows(&self.history, settings_opener(cx).is_some()),
            theme,
            on_action,
            on_menu,
        );
        (tools, actions)
    }

    /// The property strip for what the next strip change would restyle and the spotlight dim
    /// while a spotlight is picked; or the Crop tool's Apply and Cancel.
    fn strip(&self, view: &WeakEntity<Self>, theme: Theme) -> Vec<(StripGroup, AnyElement)> {
        if self.cropping.is_some() {
            let actor = view.clone();
            return crop_strip(theme, move |action, window: &mut Window, cx: &mut App| {
                update_while_open(&actor, cx, |view, cx| view.act(action, window, cx));
            });
        }
        let setter = view.clone();
        let on_set = move |property, window: &mut Window, cx: &mut App| {
            update_while_open(&setter, cx, |view, cx| {
                view.set_from_strip(property, window, cx);
            });
        };
        let sampler = view.clone();
        let on_sample = move |value, _: &mut Window, cx: &mut App| {
            update_while_open(&sampler, cx, |view, cx| {
                view.sampling = Some(value);
                cx.notify();
            });
        };
        let changer = view.clone();
        let on_document = move |command, _: &mut Window, cx: &mut App| {
            update_while_open(&changer, cx, |view, cx| {
                view.apply(command);
                cx.notify();
            });
        };
        let (strip_tool, look) = if let Some(text_box) = &self.text_box {
            (text_box.tool(), text_box.look(&self.looks))
        } else {
            let selected = self.selected.and_then(|id| self.document.shape(id));
            self.looks.shown(self.tool, selected)
        };
        property_strip(
            strip_tool,
            look,
            &self.document,
            theme,
            on_set,
            on_document,
            on_sample,
        )
    }

    /// Sets what the strip or the colour picker picked, and remembers a changed look.
    pub(super) fn set_from_strip(
        &mut self,
        property: Property,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.looks.clone();
        self.set_property(property);
        if self.looks != before {
            looks_changed(&self.looks, cx);
        }
        if self.text_box.is_some() {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }
}

/// Runs `change` on the editor behind `view` while its window is open.
fn update_while_open(
    view: &WeakEntity<EditorView>,
    cx: &mut App,
    change: impl FnOnce(&mut EditorView, &mut Context<EditorView>),
) {
    // Err only means a late event reached the view as its window closed: nothing is left to change.
    let _ = view.update(cx, change);
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_appearance(window.appearance(), self.surface, hover_time(cx));
        let view = cx.entity().downgrade();
        let restacker = view.clone();
        let (toolbar, actions) = self.caption_controls(&view, theme, cx);
        let strip = self.strip(&view, theme);
        let input = view.clone();
        let on_pointer: OnPointer = Rc::new(move |pointer, window, cx| {
            update_while_open(&view, cx, |view, cx| view.pointer(pointer, window, cx));
        });
        let overlay = match &self.text_box {
            Some(text_box) => Overlay {
                preview: Some(text_box.preview(window)),
                selected: None,
                crop_handles: false,
                over: Some(paint_text_box(text_box.clone(), input, self.focus.clone())),
            },
            None => Overlay {
                preview: self.drag.as_ref().and_then(|drag| {
                    drag.preview(self.shift, |line| line_width(line, window))
                        .expect("a look's text size is a text block's")
                }),
                selected: self.selected,
                crop_handles: self.tool == Tool::Crop,
                over: None,
            },
        };
        Rc::make_mut(&mut self.previews).update(&self.document);
        let canvas = document_canvas(
            Rc::clone(&self.document),
            Arc::clone(&self.base),
            self.placement,
            Rc::clone(&self.previews),
            overlay,
            on_pointer,
        );
        let zoom = self.zoom_controls(window, theme, cx);
        let backing = self.backing(window);
        div()
            .id("editor")
            .role(Role::Application)
            .aria_label("Cleanshot editor")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme.backdrop)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(|view, event: &KeyUpEvent, _, _| {
                if event.keystroke.key == "space" {
                    view.space_held = false;
                }
            }))
            .child(CaptionRow {
                toolbar: toolbar.into_any_element(),
                strip,
                actions: actions.into_any_element(),
                theme,
            })
            .child(image_card(
                div()
                    .size_full()
                    .when(self.sampling.is_some(), Styled::cursor_crosshair)
                    .child(canvas.size_full()),
                zoom,
                self.glow,
                backing,
                theme,
            ))
            .children(
                self.notice
                    .clone()
                    .map(|notice| notice_panel(notice, theme)),
            )
            .children(self.menu.as_ref().map(|menu| {
                menu.element(&self.document, theme, move |picked, window, cx| {
                    update_while_open(&restacker, cx, |view, cx| {
                        view.close_menu(picked, window, cx);
                    });
                })
            }))
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
pub struct EditorWindow {
    window: WindowHandle<EditorView>,
    win32: NonZeroIsize,
}

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
    masks: impl Fn(&Document) -> Option<TextMasks> + 'static,
    on_first_frame: impl FnOnce(Instant) + 'static,
    log: impl Fn(&'static str) + 'static,
) -> Result<EditorWindow, Error> {
    let document = Document::new(Arc::new(capture));
    open_with(
        cx,
        document,
        monitor,
        Some(outputs),
        Rc::new(masks),
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
    masks: MasksFor,
    log: Log,
    on_open: impl FnOnce(&mut Window, &mut App),
) -> Result<EditorWindow, Error> {
    register_inter(cx)?;
    let surface = window_surface(cx);
    let options = window_options(cx, document.base(), monitor, surface);
    let (window, win32) = open_window(cx, options, |window, cx| {
        on_open(window, cx);
        cx.new(|cx| EditorView::new(document, outputs, masks, log, surface, window, cx))
    })?;
    Ok(EditorWindow { window, win32 })
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
    window: &mut Window,
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
                double_click: false,
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
    /// The window's Win32 handle, for the app to bring it to the front once shown (D-124).
    #[must_use]
    pub fn win32_handle(&self) -> NonZeroIsize {
        self.win32
    }

    /// Runs `on_closed` once the window is gone, whether the user or [`EditorWindow::close`]
    /// closed it; at once if it is gone already. Called from inside this window's own update,
    /// the window is busy and `on_closed` runs at once, so callers ask from outside it.
    pub fn on_closed(&self, cx: &mut App, on_closed: impl FnOnce(&mut App) + 'static) {
        let mut waiting = Some(on_closed);
        let watched = self.window.update(cx, |_, _, cx| {
            if let Some(on_closed) = waiting.take() {
                cx.on_release(move |_, cx| on_closed(cx)).detach();
            }
        });
        if let (Err(_), Some(on_closed)) = (watched, waiting) {
            on_closed(cx);
        }
    }

    /// Closes the window. A window the user already closed is left as it is.
    pub fn close(self, cx: &mut App) {
        // Err only means the window is already gone, which is the state close asks for.
        let _ = self
            .window
            .update(cx, |_, window, _| window.remove_window());
    }
}

/// D-110: the editor opens sized round `image` and centred on `monitor`'s display, on
/// `surface` (D-142). With no monitor, or none GPUI knows, it opens on the primary display,
/// sized as at 100% scaling.
fn window_options(
    cx: &App,
    image: &RgbaImage,
    monitor: Option<Monitor>,
    surface: WindowSurface,
) -> WindowOptions {
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
    let smallest = size(min_width(cx), MIN_HEIGHT);
    let window_size = opening_size(image, scale, work_area, smallest);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            display_id,
            window_size,
            cx,
        ))),
        display_id,
        // D-141: the editor draws its own caption row; Windows keeps the title for the taskbar.
        titlebar: Some(TitlebarOptions {
            title: Some("Cleanshot".into()),
            appears_transparent: true,
            ..TitlebarOptions::default()
        }),
        window_min_size: Some(smallest),
        window_background: surface.background(),
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
