use crate::{CaretMove, Modifiers, Output, Restack, Tool};

/// What a key press asks the editor to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Undo,
    Redo,
    RemoveSelected,
    /// Esc: the Select tool, with nothing selected (D-111).
    BackToSelect,
    /// Ctrl+] and Ctrl+[ move the selected shape one step, with Shift to the front or back
    /// (D-115).
    Restack(Restack),
    PickTool(Tool),
    /// Back to the view that fits the image in the window (F-39).
    FitView,
    /// Ctrl+1: one image pixel on each device pixel, about the canvas's centre (D-144).
    ActualSize,
    /// Ctrl+comma: the settings window (D-144).
    OpenSettings,
    /// Space went down: a drag pans the image until it comes up (F-39).
    HoldToPan,
    /// Tab and Shift+Tab: the next or previous toolbar button (F-58).
    FocusNext,
    FocusPrevious,
    /// Ctrl+C copies and Ctrl+S saves the edited screenshot (F-40, D-48).
    Deliver(Output),
    /// Ctrl+Shift+S: the user picks where to save the edited screenshot (F-22).
    SaveAs,
    /// A key for the text box being typed into (F-34).
    Text(TextKey),
    /// Ctrl+W: the window closes as its caption's close button closes it (D-136).
    CloseWindow,
}

/// What a key does to the text box being typed into. Characters do not come through here: the
/// toolkit hands them to the box's input handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKey {
    DeleteBackward,
    DeleteForward,
    /// Left, Right, Home or End; `extend` (Shift) keeps the selection's other end.
    MoveCaret {
        direction: CaretMove,
        extend: bool,
    },
    /// Up or Down: the editor's layout finds the offset on the next line at the caret's x.
    MoveLine {
        step: LineStep,
        extend: bool,
    },
    LineBreak,
    SelectAll,
    Copy,
    Cut,
    Paste,
    /// Esc: the text becomes a shape and the box closes.
    Commit,
}

/// Which neighbouring line Up or Down moves the caret to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineStep {
    Up,
    Down,
}

/// What the editor is in the middle of, which decides what a key means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Dragging,
    EditingText,
    /// An input method holds an uncommitted composition in the text box.
    Composing,
    /// The Save As dialog is open, and its answer is still to come (F-22).
    SavingAs,
}

/// The editor's key map. `key` is the key's name as the editor's toolkit gives it: one
/// character for a letter or digit, otherwise a word such as `delete` or `escape`.
#[must_use]
pub fn action_for_key(key: &str, modifiers: Modifiers, activity: Activity) -> Option<KeyAction> {
    // D-44: Alt and Win belong to the system.
    if modifiers.alt || modifiers.win {
        return None;
    }
    match activity {
        Activity::Idle | Activity::EditingText if closes_window(key, modifiers) => {
            Some(KeyAction::CloseWindow)
        }
        Activity::Idle => idle_action(key, modifiers),
        Activity::EditingText => text_key(key, modifiers).map(KeyAction::Text),
        // D-44: one drag stays one edit. F-34: every key during a composition is the IME's.
        // F-22: a second Save As would drop the first one's answer.
        Activity::Dragging | Activity::Composing | Activity::SavingAs => None,
    }
}

fn text_key(key: &str, modifiers: Modifiers) -> Option<TextKey> {
    if modifiers.ctrl {
        return match key {
            "a" => Some(TextKey::SelectAll),
            "c" => Some(TextKey::Copy),
            "x" => Some(TextKey::Cut),
            "v" => Some(TextKey::Paste),
            _ => None,
        };
    }
    let extend = modifiers.shift;
    let caret = |direction| Some(TextKey::MoveCaret { direction, extend });
    let line = |step| Some(TextKey::MoveLine { step, extend });
    match key {
        "backspace" => Some(TextKey::DeleteBackward),
        "delete" => Some(TextKey::DeleteForward),
        "left" => caret(CaretMove::Left),
        "right" => caret(CaretMove::Right),
        "home" => caret(CaretMove::LineStart),
        "end" => caret(CaretMove::LineEnd),
        "up" => line(LineStep::Up),
        "down" => line(LineStep::Down),
        "enter" => Some(TextKey::LineBreak),
        "escape" => Some(TextKey::Commit),
        _ => None,
    }
}

fn idle_action(key: &str, modifiers: Modifiers) -> Option<KeyAction> {
    if modifiers.ctrl {
        // F-38: Ctrl+Z and Ctrl+Y.
        return match key {
            "z" => Some(KeyAction::Undo),
            "y" => Some(KeyAction::Redo),
            "0" => Some(KeyAction::FitView),
            "1" => Some(KeyAction::ActualSize),
            "," => Some(KeyAction::OpenSettings),
            "]" if modifiers.shift => Some(KeyAction::Restack(Restack::Front)),
            "}" => Some(KeyAction::Restack(Restack::Front)),
            "]" => Some(KeyAction::Restack(Restack::Forward)),
            "[" if modifiers.shift => Some(KeyAction::Restack(Restack::Back)),
            "{" => Some(KeyAction::Restack(Restack::Back)),
            "[" => Some(KeyAction::Restack(Restack::Backward)),
            "c" => Some(KeyAction::Deliver(Output::Copy)),
            "s" if modifiers.shift => Some(KeyAction::SaveAs),
            "s" => Some(KeyAction::Deliver(Output::Save)),
            _ => None,
        };
    }
    match key {
        "delete" => Some(KeyAction::RemoveSelected),
        "escape" => Some(KeyAction::BackToSelect),
        "space" => Some(KeyAction::HoldToPan),
        "tab" if modifiers.shift => Some(KeyAction::FocusPrevious),
        "tab" => Some(KeyAction::FocusNext),
        _ => single_char(key)
            .and_then(Tool::for_key)
            .map(KeyAction::PickTool),
    }
}

/// D-136: whether `key` with `modifiers` is Ctrl+W, which closes a Cleanshot window as its
/// caption's close button does.
#[must_use]
pub fn closes_window(key: &str, modifiers: Modifiers) -> bool {
    let ctrl_alone = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    key == "w" && modifiers == ctrl_alone
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
    fn d136_ctrl_w_closes_the_window_unless_a_drag_a_composition_or_save_as_holds_the_keys() {
        for activity in [Activity::Idle, Activity::EditingText] {
            assert_eq!(
                action_for_key("w", CTRL, activity),
                Some(KeyAction::CloseWindow),
                "{activity:?}"
            );
        }
        for activity in [Activity::Dragging, Activity::Composing, Activity::SavingAs] {
            assert_eq!(action_for_key("w", CTRL, activity), None, "{activity:?}");
        }
        let ctrl_shift = Modifiers {
            shift: true,
            ..CTRL
        };
        assert!(closes_window("w", CTRL));
        assert!(!closes_window("w", ctrl_shift));
        assert!(!closes_window("w", NONE));
        assert!(!closes_window("q", CTRL));
    }

    #[test]
    fn f38_ctrl_z_undoes_and_ctrl_y_redoes() {
        assert_eq!(
            action_for_key("z", CTRL, Activity::Idle),
            Some(KeyAction::Undo)
        );
        assert_eq!(
            action_for_key("y", CTRL, Activity::Idle),
            Some(KeyAction::Redo)
        );
    }

    #[test]
    fn f38_other_ctrl_keys_do_nothing_and_never_pick_a_tool() {
        assert_eq!(action_for_key("r", CTRL, Activity::Idle), None);
        assert_eq!(action_for_key("delete", CTRL, Activity::Idle), None);
    }

    #[test]
    fn f30_delete_removes_the_selection_and_escape_goes_back_to_select() {
        assert_eq!(
            action_for_key("delete", NONE, Activity::Idle),
            Some(KeyAction::RemoveSelected)
        );
        assert_eq!(
            action_for_key("escape", NONE, Activity::Idle),
            Some(KeyAction::BackToSelect)
        );
    }

    #[test]
    fn d115_ctrl_brackets_move_the_shape_a_step_and_with_shift_to_the_front_or_back() {
        let ctrl_shift = Modifiers {
            shift: true,
            ..CTRL
        };
        for (key, modifiers, to) in [
            ("]", CTRL, Restack::Forward),
            ("[", CTRL, Restack::Backward),
            ("]", ctrl_shift, Restack::Front),
            ("}", ctrl_shift, Restack::Front),
            ("[", ctrl_shift, Restack::Back),
            ("{", ctrl_shift, Restack::Back),
        ] {
            assert_eq!(
                action_for_key(key, modifiers, Activity::Idle),
                Some(KeyAction::Restack(to)),
                "{key} with {modifiers:?}"
            );
        }
        assert_eq!(action_for_key("]", NONE, Activity::Idle), None);
    }

    #[test]
    fn f30_a_tool_letter_picks_its_tool_with_or_without_shift() {
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        assert_eq!(
            action_for_key("r", NONE, Activity::Idle),
            Some(KeyAction::PickTool(Tool::Rectangle))
        );
        assert_eq!(
            action_for_key("R", shift, Activity::Idle),
            Some(KeyAction::PickTool(Tool::Rectangle))
        );
        assert_eq!(action_for_key("q", NONE, Activity::Idle), None);
        assert_eq!(action_for_key("home", NONE, Activity::Idle), None);
    }

    #[test]
    fn f39_ctrl_0_fits_the_view_and_space_holds_the_pan() {
        assert_eq!(
            action_for_key("0", CTRL, Activity::Idle),
            Some(KeyAction::FitView)
        );
        assert_eq!(action_for_key("0", NONE, Activity::Idle), None);
        assert_eq!(
            action_for_key("space", NONE, Activity::Idle),
            Some(KeyAction::HoldToPan)
        );
    }

    #[test]
    fn d144_ctrl_1_shows_actual_size_and_ctrl_comma_opens_settings() {
        assert_eq!(
            action_for_key("1", CTRL, Activity::Idle),
            Some(KeyAction::ActualSize)
        );
        assert_eq!(
            action_for_key(",", CTRL, Activity::Idle),
            Some(KeyAction::OpenSettings)
        );
        assert_eq!(action_for_key(",", NONE, Activity::Idle), None);
    }

    #[test]
    fn f58_tab_moves_focus_forward_and_shift_tab_back() {
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        assert_eq!(
            action_for_key("tab", NONE, Activity::Idle),
            Some(KeyAction::FocusNext)
        );
        assert_eq!(
            action_for_key("tab", shift, Activity::Idle),
            Some(KeyAction::FocusPrevious)
        );
    }

    #[test]
    fn f40_f43_ctrl_c_copies_and_ctrl_s_saves_while_c_and_s_alone_pick_crop_and_spotlight() {
        let copy = Some(KeyAction::Deliver(Output::Copy));
        assert_eq!(action_for_key("c", CTRL, Activity::Idle), copy);
        assert_eq!(
            action_for_key("s", CTRL, Activity::Idle),
            Some(KeyAction::Deliver(Output::Save))
        );
        assert_eq!(
            action_for_key("c", NONE, Activity::Idle),
            Some(KeyAction::PickTool(Tool::Crop))
        );
        assert_eq!(
            action_for_key("s", NONE, Activity::Idle),
            Some(KeyAction::PickTool(Tool::Spotlight))
        );
    }

    #[test]
    fn f22_ctrl_shift_s_saves_as_and_ctrl_s_saves() {
        let ctrl_shift = Modifiers {
            shift: true,
            ..CTRL
        };
        assert_eq!(
            action_for_key("s", ctrl_shift, Activity::Idle),
            Some(KeyAction::SaveAs)
        );
        assert_eq!(
            action_for_key("s", CTRL, Activity::Idle),
            Some(KeyAction::Deliver(Output::Save))
        );
    }

    #[test]
    fn d44_every_key_is_ignored_while_a_drag_is_in_progress() {
        for (key, modifiers) in [("z", CTRL), ("delete", NONE), ("escape", NONE), ("r", NONE)] {
            assert_eq!(
                action_for_key(key, modifiers, Activity::Dragging),
                None,
                "{key}"
            );
        }
    }

    #[test]
    fn f22_every_key_is_ignored_while_the_save_as_dialog_is_open() {
        let ctrl_shift = Modifiers {
            shift: true,
            ..CTRL
        };
        for (key, modifiers) in [("s", ctrl_shift), ("s", CTRL), ("z", CTRL), ("r", NONE)] {
            assert_eq!(
                action_for_key(key, modifiers, Activity::SavingAs),
                None,
                "{key}"
            );
        }
    }

    fn text(key: &str, modifiers: Modifiers) -> Option<KeyAction> {
        action_for_key(key, modifiers, Activity::EditingText)
    }

    #[test]
    fn f34_while_typing_tool_letters_space_and_tab_reach_the_text_box_not_the_editor() {
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        for (key, modifiers) in [
            ("t", NONE),
            ("v", NONE),
            ("R", shift),
            ("space", NONE),
            ("tab", NONE),
            ("z", CTRL),
            ("s", CTRL),
        ] {
            assert_eq!(text(key, modifiers), None, "{key}");
        }
    }

    #[test]
    fn f34_while_typing_home_end_up_and_down_move_the_caret_and_shift_extends() {
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        let caret =
            |direction, extend| Some(KeyAction::Text(TextKey::MoveCaret { direction, extend }));
        let line = |step, extend| Some(KeyAction::Text(TextKey::MoveLine { step, extend }));
        assert_eq!(text("home", NONE), caret(CaretMove::LineStart, false));
        assert_eq!(text("end", shift), caret(CaretMove::LineEnd, true));
        assert_eq!(text("up", NONE), line(LineStep::Up, false));
        assert_eq!(text("down", shift), line(LineStep::Down, true));
    }

    #[test]
    fn f34_while_typing_the_editing_keys_act_on_the_text() {
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        let key = |text_key| Some(KeyAction::Text(text_key));
        assert_eq!(text("backspace", NONE), key(TextKey::DeleteBackward));
        assert_eq!(text("delete", NONE), key(TextKey::DeleteForward));
        assert_eq!(text("enter", NONE), key(TextKey::LineBreak));
        assert_eq!(text("escape", NONE), key(TextKey::Commit));
        assert_eq!(
            text("left", NONE),
            key(TextKey::MoveCaret {
                direction: CaretMove::Left,
                extend: false
            })
        );
        assert_eq!(
            text("right", shift),
            key(TextKey::MoveCaret {
                direction: CaretMove::Right,
                extend: true
            })
        );
        assert_eq!(text("a", CTRL), key(TextKey::SelectAll));
        assert_eq!(text("c", CTRL), key(TextKey::Copy));
        assert_eq!(text("x", CTRL), key(TextKey::Cut));
        assert_eq!(text("v", CTRL), key(TextKey::Paste));
        assert_eq!(action_for_key("backspace", NONE, Activity::Idle), None);
    }

    #[test]
    fn f34_every_key_belongs_to_the_input_method_during_a_composition() {
        for (key, modifiers) in [
            ("escape", NONE),
            ("backspace", NONE),
            ("enter", NONE),
            ("a", CTRL),
        ] {
            assert_eq!(
                action_for_key(key, modifiers, Activity::Composing),
                None,
                "{key}"
            );
        }
    }

    #[test]
    fn d44_every_key_is_ignored_with_alt_or_win_held() {
        let alt_while_typing = Modifiers { alt: true, ..NONE };
        assert_eq!(text("backspace", alt_while_typing), None);
        let alt = Modifiers { alt: true, ..CTRL };
        let win = Modifiers { win: true, ..NONE };
        assert_eq!(action_for_key("z", alt, Activity::Idle), None);
        assert_eq!(action_for_key("r", win, Activity::Idle), None);
        assert_eq!(action_for_key("delete", win, Activity::Idle), None);
    }
}
