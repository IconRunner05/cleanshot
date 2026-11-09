use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AccessibleAction, App, Div, FocusHandle, InteractiveElement, KeyDownEvent, ParentElement, Rgba,
    Role, SharedString, Stateful, StatefulInteractiveElement, Styled, Toggled, Window, div, px,
};

use crate::icons::icon_of;
use crate::theme::{CORNER, ControlFill, Theme, filled, panel};

/// One row of a popup menu: its icon, name and shortcut.
pub(crate) struct MenuRow {
    pub icon: Option<&'static str>,
    pub name: SharedString,
    pub keys: &'static str,
    pub state: RowState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowState {
    /// Shown dimmed, and cannot be picked or reached with the keyboard.
    Dimmed,
    Enabled,
    /// One of the menu's choices, checked when it is the current one.
    Choice {
        checked: bool,
    },
}

impl RowState {
    fn enabled(self) -> bool {
        match self {
            Self::Dimmed => false,
            Self::Enabled | Self::Choice { .. } => true,
        }
    }

    fn fill(self) -> ControlFill {
        match self {
            Self::Choice { checked: true } => ControlFill::Accent,
            Self::Dimmed | Self::Enabled | Self::Choice { checked: false } => ControlFill::Neutral,
        }
    }

    /// Whether the row shows hover under the pointer: the checked choice keeps its accent, and a
    /// dimmed row cannot be picked.
    fn hovers(self) -> bool {
        match self {
            Self::Dimmed | Self::Choice { checked: true } => false,
            Self::Enabled | Self::Choice { checked: false } => true,
        }
    }

    /// The row's text colour in `theme`.
    pub(crate) fn ink(self, theme: Theme) -> Rgba {
        match self {
            Self::Dimmed => theme.dimmed,
            Self::Choice { checked: true } => theme.on_accent,
            Self::Enabled | Self::Choice { checked: false } => theme.text,
        }
    }
}

/// The keyboard's place in an open popup menu: one focus per row of the rows it opened with.
pub(crate) struct MenuFocus(Vec<FocusHandle>);

impl MenuFocus {
    /// A menu of `rows`, with the keyboard on the checked row, or else on the first that can
    /// be picked.
    pub(crate) fn open(rows: &[MenuRow], window: &mut Window, cx: &mut App) -> Self {
        let items: Vec<_> = rows.iter().map(|_| cx.focus_handle()).collect();
        let start = rows
            .iter()
            .position(|row| row.state == RowState::Choice { checked: true })
            .or_else(|| rows.iter().position(|row| row.state.enabled()));
        if let Some(start) = start {
            window.focus(&items[start], cx);
        }
        Self(items)
    }

    /// The menu's panel, named `label`, with the same `rows` it opened with. `on_pick` gets the
    /// index of the row picked by click, Enter or Space, or `None` when Esc or a press outside
    /// the menu closes it. Up and Down move between the rows that can be picked.
    pub(crate) fn element(
        &self,
        id: &'static str,
        label: &'static str,
        rows: &[MenuRow],
        theme: Theme,
        on_pick: impl Fn(Option<usize>, &mut Window, &mut App) + Clone + 'static,
    ) -> Stateful<Div> {
        let reachable: Vec<_> = rows
            .iter()
            .zip(&self.0)
            .filter(|(row, _)| row.state.enabled())
            .map(|(_, item)| item.clone())
            .collect();
        let on_close = on_pick.clone();
        let on_key = on_pick.clone();
        let items = rows
            .iter()
            .zip(&self.0)
            .enumerate()
            .map(|(index, (row, item))| {
                let on_pick = on_pick.clone();
                let ink = row.state.ink(theme);
                let fill = row.state.fill();
                div()
                    .id(row.name.clone())
                    .role(match row.state {
                        RowState::Dimmed | RowState::Enabled => Role::MenuItem,
                        RowState::Choice { .. } => Role::MenuItemRadio,
                    })
                    .aria_label(row.name.clone())
                    .aria_keyshortcuts(row.keys)
                    .when_some(
                        match row.state {
                            RowState::Dimmed | RowState::Enabled => None,
                            RowState::Choice { checked } => Some(checked),
                        },
                        |row, checked| {
                            row.aria_toggled(if checked {
                                Toggled::True
                            } else {
                                Toggled::False
                            })
                        },
                    )
                    .h(px(28.0))
                    .px(px(10.0))
                    .gap(px(24.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded(CORNER)
                    .text_size(px(13.0))
                    .text_color(ink)
                    .when(row.state.enabled(), |element| {
                        filled(element.track_focus(item), fill, theme)
                            .on_click(move |_, window, cx| on_pick(Some(index), window, cx))
                    })
                    .when(row.state.hovers(), |row| {
                        row.hover(move |row| row.bg(theme.hover))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .children(row.icon.map(|icon| icon_of(icon, ink)))
                            .child(row.name.clone()),
                    )
                    .child(row.keys)
            });
        panel(theme)
            .id(id)
            .role(Role::Menu)
            .aria_label(label)
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
                    "up" => reachable.len().saturating_sub(1),
                    _ => return,
                };
                let at = reachable.iter().position(|item| item.is_focused(window));
                if let Some(next) = at.map(|at| (at + step) % reachable.len()) {
                    window.focus(&reachable[next], cx);
                }
                cx.stop_propagation();
            })
            .children(items)
    }
}

/// `button`, which opens a menu or a popup, as a screen reader works it: UI
/// Automation reads `open` as its expanded state, and its Expand and Collapse call `set_open`
/// with true and false, as a click opens and closes it.
pub(crate) fn expandable<E: StatefulInteractiveElement>(
    button: E,
    open: bool,
    set_open: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> E {
    let expand = Rc::new(set_open);
    let collapse = Rc::clone(&expand);
    button
        .aria_expanded(open)
        .on_a11y_action(AccessibleAction::Expand, move |_, window, cx| {
            expand(true, window, cx);
        })
        .on_a11y_action(AccessibleAction::Collapse, move |_, window, cx| {
            collapse(false, window, cx);
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checked_choice_keeps_its_accent_under_the_pointer() {
        assert!(!RowState::Choice { checked: true }.hovers());
        assert!(RowState::Choice { checked: false }.hovers());
        assert!(RowState::Enabled.hovers());
        assert!(!RowState::Dimmed.hovers());
    }
}
