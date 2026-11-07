use cleanshot_core::{
    Activity, BoxPress, Command, Drag, History, KeyAction, Modifiers, PointF, Press, Start,
    TextKey, Tool, action_for_key, shape_at, text_reopened_at,
};
use gpui::{Context, KeyDownEvent, Window, point};

use super::{EditorView, Pan};
use crate::canvas::Pointer;
use crate::settings_window::settings_opener;
use crate::shape_menu::ShapeMenu;
use crate::text::line_width;

impl EditorView {
    pub(super) fn pointer(
        &mut self,
        pointer: Pointer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match pointer {
            Pointer::Press { at, .. } if self.sampling.is_some() => self.sample(at, window, cx),
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
            } => self.press(at, reach, double_click, window, cx),
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
            Pointer::Scroll { by, placement } => {
                let viewport = self.viewport(window);
                let scrolled = placement.scrolled(by, viewport.room, viewport.area);
                if scrolled != placement {
                    self.placement = Some(scrolled);
                }
            }
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

    /// A press on the screenshot, past the colour sampler and Space's pan.
    fn press(
        &mut self,
        at: PointF,
        reach: f32,
        double_click: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.notice = None;
        // D-159: Enter after a press on the canvas is the canvas's, not a toolbar button's.
        window.focus(&self.focus, cx);
        if double_click
            && self
                .cropping
                .is_some_and(|cropping| cropping.applied_by_double_click(&self.document, at))
        {
            self.drag = None;
            self.act(KeyAction::ApplyCrop, window, cx);
            return;
        }
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

    /// D-147: the press after the picker's "From image" sets the colour under it, and does
    /// nothing else.
    fn sample(&mut self, at: PointF, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(value) = self.sampling.take()
            && let Some(color) = self.document.base().color_at(at)
        {
            self.set_from_strip(value.property(color), window, cx);
        }
        cx.notify();
    }

    pub(super) fn key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            (None, None, None) if self.cropping.is_some() => Activity::Cropping,
            (None, None, None) => Activity::Idle,
        };
        let Some(action) = action_for_key(&event.keystroke.key, modifiers, activity) else {
            return;
        };
        // D-159: Enter on a focused button presses that button when the key comes up.
        if action == KeyAction::ApplyCrop && !self.focus.is_focused(window) {
            return;
        }
        self.act(action, window, cx);
    }

    /// Does what a key asks, or a toolbar button that takes the same action (D-133).
    pub(super) fn act(&mut self, action: KeyAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            KeyAction::Undo => self.undo_or_redo(History::undo),
            KeyAction::Redo => self.undo_or_redo(History::redo),
            KeyAction::RemoveSelected => {
                if let Some(id) = self.selected.take() {
                    self.apply(Command::Remove(id));
                }
            }
            KeyAction::BackToSelect if self.sampling.is_some() => self.sampling = None,
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
                // D-167: actual size that is also the fit is the fitted view, panel and all.
                self.placement = (!viewport.fit_is_actual_size()).then(|| {
                    viewport
                        .placement
                        .actual_size(centre, viewport.scale_factor)
                });
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
            KeyAction::ApplyCrop => {
                if let Some(cropping) = self.cropping {
                    self.pick_tool(cropping.back_to(), window, cx);
                }
            }
            KeyAction::CancelCrop => {
                if let Some(cropping) = self.cropping.take() {
                    self.pick_tool(cropping.back_to(), window, cx);
                }
            }
            KeyAction::Text(TextKey::Commit) => self.close_text_box(window),
            KeyAction::Text(key) => {
                if let Some(text_box) = &mut self.text_box {
                    text_box.key(key, window, cx);
                }
            }
        }
        cx.notify();
    }
}
