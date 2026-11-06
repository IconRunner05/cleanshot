use cleanshot_core::{Document, Restack, ShapeId};
use gpui::{App, IntoElement, ParentElement, Pixels, Point, Window, anchored, deferred};

use crate::menu::{MenuFocus, MenuRow, RowState};
use crate::theme::Theme;

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
    focus: MenuFocus,
}

/// The menu's rows for `id` in `document`: a move that would leave the shape where it is
/// shows dimmed.
fn rows(document: &Document, id: ShapeId) -> [MenuRow; 4] {
    MOVES.map(|(to, name, keys)| MenuRow {
        icon: None,
        name,
        keys,
        state: if to.index(document, id).is_some() {
            RowState::Enabled
        } else {
            RowState::Dimmed
        },
    })
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
        let focus = MenuFocus::open(&rows(document, id), window, cx);
        Self { id, at, focus }
    }

    /// The menu over everything else. `on_pick` gets the move picked by click, Enter or Space,
    /// or `None` when Esc or a press outside the menu closes it.
    pub fn element(
        &self,
        document: &Document,
        theme: Theme,
        on_pick: impl Fn(Option<Restack>, &mut Window, &mut App) + Clone + 'static,
    ) -> impl IntoElement {
        let menu = self.focus.element(
            "shape-menu",
            "Shape",
            &rows(document, self.id),
            theme,
            move |picked: Option<usize>, window: &mut Window, cx: &mut App| {
                on_pick(picked.map(|index| MOVES[index].0), window, cx);
            },
        );
        deferred(anchored().position(self.at).snap_to_window().child(menu))
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
