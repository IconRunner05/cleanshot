use crate::{Modifiers, Tool};

/// What a key press asks the editor to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Undo,
    Redo,
    RemoveSelected,
    ClearSelection,
    PickTool(Tool),
    /// Back to the view that fits the image in the window (F-39).
    FitView,
    /// Space went down: a drag pans the image until it comes up (F-39).
    HoldToPan,
    /// Tab and Shift+Tab: the next or previous toolbar button (F-58).
    FocusNext,
    FocusPrevious,
}

/// The editor's key map. `key` is the key's name as the editor's toolkit gives it: one
/// character for a letter or digit, otherwise a word such as `delete` or `escape`.
#[must_use]
pub fn action_for_key(key: &str, modifiers: Modifiers, dragging: bool) -> Option<KeyAction> {
    // D-44: one drag stays one edit, and Alt and Win belong to the system.
    if modifiers.alt || modifiers.win || dragging {
        return None;
    }
    if modifiers.ctrl {
        // F-38: Ctrl+Z and Ctrl+Y.
        return match key {
            "z" => Some(KeyAction::Undo),
            "y" => Some(KeyAction::Redo),
            "0" => Some(KeyAction::FitView),
            _ => None,
        };
    }
    match key {
        "delete" => Some(KeyAction::RemoveSelected),
        "escape" => Some(KeyAction::ClearSelection),
        "space" => Some(KeyAction::HoldToPan),
        "tab" if modifiers.shift => Some(KeyAction::FocusPrevious),
        "tab" => Some(KeyAction::FocusNext),
        _ => single_char(key)
            .and_then(Tool::for_key)
            .map(KeyAction::PickTool),
    }
}

fn single_char(key: &str) -> Option<char> {
    let mut chars = key.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers {
        ctrl: false,
        alt: false,
        shift: false,
        win: false,
    };
    const CTRL: Modifiers = Modifiers { ctrl: true, ..NONE };

    #[test]
    fn f38_ctrl_z_undoes_and_ctrl_y_redoes() {
        assert_eq!(action_for_key("z", CTRL, false), Some(KeyAction::Undo));
        assert_eq!(action_for_key("y", CTRL, false), Some(KeyAction::Redo));
    }

    #[test]
    fn f38_other_ctrl_keys_do_nothing_and_never_pick_a_tool() {
        assert_eq!(action_for_key("r", CTRL, false), None);
        assert_eq!(action_for_key("delete", CTRL, false), None);
    }

    #[test]
    fn f30_delete_removes_the_selection_and_escape_clears_it() {
        assert_eq!(
            action_for_key("delete", NONE, false),
            Some(KeyAction::RemoveSelected)
        );
        assert_eq!(
            action_for_key("escape", NONE, false),
            Some(KeyAction::ClearSelection)
        );
    }

    #[test]
    fn f30_a_tool_letter_picks_its_tool_with_or_without_shift() {
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        assert_eq!(
            action_for_key("r", NONE, false),
            Some(KeyAction::PickTool(Tool::Rectangle))
        );
        assert_eq!(
            action_for_key("R", shift, false),
            Some(KeyAction::PickTool(Tool::Rectangle))
        );
        assert_eq!(action_for_key("q", NONE, false), None);
        assert_eq!(action_for_key("home", NONE, false), None);
    }

    #[test]
    fn f39_ctrl_0_fits_the_view_and_space_holds_the_pan() {
        assert_eq!(action_for_key("0", CTRL, false), Some(KeyAction::FitView));
        assert_eq!(action_for_key("0", NONE, false), None);
        assert_eq!(
            action_for_key("space", NONE, false),
            Some(KeyAction::HoldToPan)
        );
    }

    #[test]
    fn f58_tab_moves_focus_forward_and_shift_tab_back() {
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        assert_eq!(
            action_for_key("tab", NONE, false),
            Some(KeyAction::FocusNext)
        );
        assert_eq!(
            action_for_key("tab", shift, false),
            Some(KeyAction::FocusPrevious)
        );
    }

    #[test]
    fn d44_every_key_is_ignored_while_a_drag_is_in_progress() {
        for (key, modifiers) in [("z", CTRL), ("delete", NONE), ("escape", NONE), ("r", NONE)] {
            assert_eq!(action_for_key(key, modifiers, true), None, "{key}");
        }
    }

    #[test]
    fn d44_every_key_is_ignored_with_alt_or_win_held() {
        let alt = Modifiers { alt: true, ..CTRL };
        let win = Modifiers { win: true, ..NONE };
        assert_eq!(action_for_key("z", alt, false), None);
        assert_eq!(action_for_key("r", win, false), None);
        assert_eq!(action_for_key("delete", win, false), None);
    }
}
