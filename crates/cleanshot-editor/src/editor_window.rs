mod text_input;

use std::num::NonZeroIsize;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use cleanshot_core::{
    Activity, BoxPress, Color, Command, CommandError, Delivered, DeliveryError, Document, Drag,
    History, KeyAction, Looks, Modifiers, Monitor, Output, PointF, Press, Property, PropertyError,
    Restack, RgbaImage, Setting, ShapeId, Start, TextBlockError, TextKey, TextMasks, TextOrigin,
    Tool, action_for_key, delivery_notice, save_as_notice, shape_at, text_reopened_at,
};
use gpui::{
    AnyElement, App, AppContext, Bounds, BoxShadow, Context, Entity, FocusHandle, Global,
    InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, ParentElement, Pixels, Point,
    Render, RenderImage, Rgba, Role, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Task, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowHandle,
    WindowOptions, div, hsla, point, px, size,
};

use crate::canvas::{OnPointer, Overlay, Pointer, Previews, document_canvas, render_image, rgba};
use crate::caption::caption_row;
use crate::placement::{
    CARD_INSET, MIN_WINDOW, Placement, canvas_size, fitted_placement, opening_size, shown_area,
};
use crate::settings_window::settings_opener;
use crate::shape_menu::ShapeMenu;
use crate::strip::property_strip;
use crate::text::{line_width, register_inter};
use crate::text_box::TextBox;
use crate::theme::{Theme, WindowSurface, window_surface};
use crate::toolbar::{
    Replay, ToolbarMenu, ToolbarMenus, actions, divider, group_rows, more_rows, show_tool,
    shown_tools, toolbar,
};
use crate::tooltip::hover_time;
use crate::zoom_panel::{OnPanelInput, PanelInput, Viewport, scrollbars, zoom_panel};
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
    /// The history's document as last drawn; renewed after each edit.
    document: Rc<Document>,
    base: Arc<RenderImage>,
    tool: Tool,
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
    /// While a press on the minimap is held, its drags pan the view (D-145).
    minimap_held: bool,
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
            minimap_held: false,
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

    fn pointer(&mut self, pointer: Pointer, window: &mut Window, cx: &mut Context<Self>) {
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
            Pointer::Press {
                at,
                reach,
                double_click,
                ..
            } => {
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
                if double_click && let Some(origin) = text_reopened_at(&self.document, at, reach) {
                    self.selected = None;
                    self.drag = None;
                    self.open_text_box(origin);
                    cx.notify();
                    return;
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
                if let Some(command) = drag.command(shift, |line| line_width(line, window)) {
                    self.apply(command);
                } else if let Some(origin) = drag.typed_on_release(self.tool) {
                    self.selected = None;
                    self.open_text_box(origin);
                }
            }
            Pointer::Zoom {
                factor,
                around,
                placement,
            } => self.placement = Some(placement.zoomed(factor, around)),
            Pointer::Menu {
                at,
                reach,
                position,
            } => {
                self.menu = None;
                let idle = self.text_box.is_none() && self.drag.is_none();
                if let Some(id) = shape_at(&self.document, at, reach).filter(|_| idle) {
                    self.selected = Some(id);
                    self.menu = Some(ShapeMenu::open(id, position, &self.document, window, cx));
                }
            }
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
        self.act(action, window, cx);
    }

    /// Does what a key asks, or a toolbar button that takes the same action (D-133).
    fn act(&mut self, action: KeyAction, window: &mut Window, cx: &mut Context<Self>) {
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
                self.pick_tool(Tool::Select, window, cx);
            }
            KeyAction::Restack(to) => {
                if let Some(id) = self.selected {
                    self.apply(Command::Restack { id, to });
                }
            }
            KeyAction::PickTool(tool) => self.pick_tool(tool, window, cx),
            KeyAction::FitView => self.placement = None,
            KeyAction::ActualSize => {
                let viewport = self.viewport(window);
                let centre = point(viewport.room.width / 2.0, viewport.room.height / 2.0);
                self.placement = Some(
                    viewport
                        .placement
                        .actual_size(centre, viewport.scale_factor),
                );
            }
            KeyAction::OpenSettings => {
                if let Some(open) = settings_opener(cx) {
                    cx.defer(move |cx| open(cx));
                }
            }
            KeyAction::HoldToPan => self.space_held = true,
            KeyAction::FocusNext => window.focus_next(cx),
            KeyAction::FocusPrevious => window.focus_prev(cx),
            KeyAction::Deliver(output) => self.deliver(output),
            KeyAction::SaveAs => self.save_as(window, cx),
            KeyAction::CloseWindow => window.remove_window(),
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
        // D-113: a new crop is shown fitted, whatever the zoom was.
        let refit = matches!(command, Command::SetCrop(_));
        match self.history.apply(command) {
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
        let document = self.history.document();
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
        self.tool = tool;
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
        self.document = Rc::new(self.history.document().clone());
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

    /// What the canvas shows now: the fitted placement until the image is zoomed or panned.
    fn viewport(&self, window: &Window) -> Viewport {
        let room = canvas_size(window.viewport_size());
        let scale_factor = window.scale_factor();
        let changing_crop = self.tool == Tool::Crop;
        Viewport {
            area: shown_area(&self.document, changing_crop),
            placement: self.placement.unwrap_or_else(|| {
                fitted_placement(&self.document, changing_crop, room, scale_factor)
            }),
            room,
            scale_factor,
        }
    }

    /// The zoom panel and scrollbars over the image card, none while the image is fitted
    /// (D-145).
    fn zoom_controls(&self, window: &Window, theme: Theme, cx: &Context<Self>) -> Vec<AnyElement> {
        if self.placement.is_none() {
            return Vec::new();
        }
        let viewport = self.viewport(window);
        let view = cx.entity().downgrade();
        let on_input: OnPanelInput = Rc::new(move |input, window, cx| {
            update_while_open(&view, cx, |view, cx| {
                view.zoom_input(input, window, cx);
            });
        });
        scrollbars(viewport)
            .into_iter()
            .map(IntoElement::into_any_element)
            .chain([zoom_panel(
                Arc::clone(&self.base),
                self.document.image_area(),
                viewport,
                theme,
                Rc::clone(&self.log),
                &on_input,
            )
            .into_any_element()])
            .collect()
    }

    fn zoom_input(&mut self, input: PanelInput, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = self.viewport(window);
        let centre = point(viewport.room.width / 2.0, viewport.room.height / 2.0);
        match input {
            PanelInput::ZoomBy(factor) => {
                self.placement = Some(viewport.placement.zoomed(factor, centre));
            }
            PanelInput::ActualSize => self.act(KeyAction::ActualSize, window, cx),
            PanelInput::Fit => self.act(KeyAction::FitView, window, cx),
            PanelInput::Grab(at) => {
                self.minimap_held = true;
                self.placement = Some(viewport.placement.centred_on(at, viewport.room));
            }
            PanelInput::Drag(at) => {
                if !self.minimap_held {
                    return;
                }
                self.placement = Some(viewport.placement.centred_on(at, viewport.room));
            }
            PanelInput::Release => {
                if !self.minimap_held {
                    return;
                }
                self.minimap_held = false;
            }
        }
        cx.notify();
    }

    /// The property strip for what the next strip change would restyle, the spotlight dim while
    /// a spotlight is picked, and the frame toggles.
    fn strip(&self, view: &WeakEntity<Self>, theme: Theme) -> impl IntoElement + use<> {
        let setter = view.clone();
        let on_set = move |property, window: &mut Window, cx: &mut App| {
            update_while_open(&setter, cx, |view, cx| {
                let before = view.looks.clone();
                view.set_property(property);
                if view.looks != before {
                    looks_changed(&view.looks, cx);
                }
                if view.text_box.is_some() {
                    window.focus(&view.focus, cx);
                }
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
        let settings = Setting::shown_for(strip_tool, &look);
        property_strip(
            settings,
            look,
            self.document.frame(),
            (strip_tool == Tool::Spotlight).then(|| self.document.dim()),
            theme,
            on_set,
            on_document,
        )
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
        let tools = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(toolbar)
            .child(divider(theme))
            .child(strip);
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
            .child(caption_row(tools, actions, window, theme))
            .child(image_card(canvas.size_full(), zoom, self.glow, theme))
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

/// D-146: how strongly the capture's colour glows round its card, as in the U2 and V2 mock-ups.
const GLOW_ALPHA: f32 = 0.28;

/// D-141: the image on a card with rounded corners and a soft shadow, the window showing
/// round it on the left, right and below, with `zoom` over it (D-145) and a soft `glow` of the
/// capture's colour round it (D-146).
fn image_card(
    canvas: impl IntoElement,
    zoom: Vec<AnyElement>,
    glow: Color,
    theme: Theme,
) -> impl IntoElement {
    div()
        .flex_1()
        .min_h_0()
        .px(CARD_INSET)
        .pb(CARD_INSET)
        .child(
            div()
                .id("screenshot")
                .role(Role::Image)
                .aria_label("Screenshot")
                .size_full()
                .relative()
                .rounded(px(10.0))
                .overflow_hidden()
                .bg(theme.card)
                .shadow(vec![
                    BoxShadow {
                        color: Rgba {
                            a: GLOW_ALPHA,
                            ..rgba(glow)
                        }
                        .into(),
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(90.0),
                        spread_radius: px(0.0),
                        inset: false,
                    },
                    BoxShadow {
                        color: hsla(0.0, 0.0, 0.0, 0.35),
                        offset: point(px(0.0), px(12.0)),
                        blur_radius: px(32.0),
                        spread_radius: px(0.0),
                        inset: false,
                    },
                ])
                .child(canvas)
                .children(zoom),
        )
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
    let window_size = opening_size(image, scale, work_area);
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
        window_min_size: Some(MIN_WINDOW),
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
