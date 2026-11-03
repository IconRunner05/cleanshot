use cleanshot_core::{Document, Restack, ShapeId};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Pixels, Point,
    Rgba, Role, StatefulInteractiveElement, Styled, Window, anchored, deferred, div, px,
};

use crate::theme::{ACCENT, ON_ACCENT, Theme};
use crate::toolbar::panel;

/// The menu's items: each move, its name and its shortcut (D-115).
const MOVES: [(Restack, &str, &str); 4] = [
    (Restack::Forward, "Bring forward", "Ctrl+]"),
    (Restack::Front, "Bring to front", "Ctrl+Shift+]"),
    (Restack::Backward, "Send backward", "Ctrl+["),
    (Restack::Back, "Send to back", "Ctrl+Shift+["),
];

/// The right-click menu on the shape `id`, open at `at` in the window (D-115).
pub(crate) struct ShapeMenu {
    pub id: ShapeId,
    at: Point<Pixels>,
    items: [FocusHandle; 4],
}

impl ShapeMenu {
    /// The menu on `id` at `at`, with the keyboard on its first move that would change
    /// `document`.
    pub fn open(
        id: ShapeId,
        at: Point<Pixels>,
        document: &Document,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let items = [(); 4].map(|()| cx.focus_handle());
        let first = MOVES
            .iter()
            .zip(&items)
            .find(|((to, ..), _)| to.index(document, id).is_some());
        if let Some((_, item)) = first {
            window.focus(item, cx);
        }
        Self { id, at, items }
    }

    /// The menu over everything else. `on_pick` gets the move picked by click, Enter or Space,
    /// or `None` when Esc or a press outside the menu closes it. A move that would leave the
    /// shape where it is shows dimmed and cannot be picked or reached with the keyboard.
    pub fn element(
        &self,
        document: &Document,
        theme: Theme,
        on_pick: impl Fn(Option<Restack>, &mut Window, &mut App) + Clone + 'static,
    ) -> impl IntoElement {
        let movable: Vec<_> = MOVES
            .iter()
            .zip(&self.items)
            .filter(|((to, ..), _)| to.index(document, self.id).is_some())
            .map(|(_, item)| item.clone())
            .collect();
        let on_close = on_pick.clone();
        let on_key = on_pick.clone();
        let items = MOVES
            .iter()
            .zip(&self.items)
            .map(|(&(to, name, keys), item)| {
                let can_move = to.index(document, self.id).is_some();
                let on_pick = on_pick.clone();
                div()
                    .id(name)
                    .role(Role::MenuItem)
                    .aria_label(name)
                    .aria_keyshortcuts(keys)
                    .h(px(28.0))
                    .px(px(10.0))
                    .gap(px(24.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded(px(7.0))
                    .text_size(px(13.0))
                    .when(can_move, |row| {
                        row.track_focus(item)
                            .text_color(theme.text)
                            .hover(move |row| row.bg(theme.hover))
                            .focus(|row| row.bg(ACCENT).text_color(ON_ACCENT))
                            .on_click(move |_, window, cx| on_pick(Some(to), window, cx))
                    })
                    .when(!can_move, |row| {
                        row.text_color(Rgba {
                            a: 0.4,
                            ..theme.text
                        })
                    })
                    .child(name)
                    .child(keys)
            });
        deferred(
            anchored().position(self.at).snap_to_window().child(
                panel(theme)
                    .id("shape-menu")
                    .role(Role::Menu)
                    .aria_label("Shape")
                    .flex_col()
                    .items_stretch()
                    .on_mouse_down_out(move |_, window, cx| on_close(None, window, cx))
                    .on_key_down(move |event: &KeyDownEvent, window, cx| {
                        let step = match event.keystroke.key.as_str() {
                            "escape" => {
                                on_key(None, window, cx);
                                cx.stop_propagation();
                                return;
                            }
                            "down" => 1,
                            "up" => movable.len().saturating_sub(1),
                            _ => return,
                        };
                        let at = movable.iter().position(|item| item.is_focused(window));
                        if let Some(next) = at.map(|at| (at + step) % movable.len()) {
                            window.focus(&movable[next], cx);
                        }
                        cx.stop_propagation();
                    })
                    .children(items),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use cleanshot_core::{Activity, KeyAction, Modifiers, action_for_key};

    use super::*;

    #[test]
    fn d115_each_menu_item_names_the_shortcut_that_makes_the_same_move() {
        for (to, name, keys) in MOVES {
            let shift = keys.contains("Shift+");
            let key = keys.rsplit('+').next().expect("a shortcut ends in its key");
            let modifiers = Modifiers {
                ctrl: keys.starts_with("Ctrl+"),
                alt: false,
                shift,
                win: false,
            };
            assert_eq!(
                action_for_key(key, modifiers, Activity::Idle),
                Some(KeyAction::Restack(to)),
                "{name}: {keys}"
            );
        }
    }
}
