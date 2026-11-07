use gpui::prelude::FluentBuilder;
use gpui::{
    App, Div, FocusHandle, InteractiveElement, KeyDownEvent, ParentElement, Rgba, Role,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Toggled, Window, div, px,
};

use crate::theme::{Fill, ON_ACCENT, Theme, filled};
use crate::toolbar::{icon_of, panel};

/// One row of a popup menu: its icon, name and shortcut (D-115, D-143).
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

    fn fill(self) -> Fill {
        match self {
            Self::Choice { checked: true } => Fill::Accent,
            Self::Dimmed | Self::Enabled | Self::Choice { checked: false } => Fill::Neutral,
        }
    }

    /// Whether the row shows hover under the pointer: the checked choice keeps its accent
    /// (D-155), and a dimmed row cannot be picked.
    fn hovers(self) -> bool {
        match self {
            Self::Dimmed | Self::Choice { checked: true } => false,
            Self::Enabled | Self::Choice { checked: false } => true,
        }
    }

    /// The row's text colour in `theme`.
    pub(crate) fn ink(self, theme: Theme) -> Rgba {
        match self {
            Self::Dimmed => Rgba {
                a: 0.4,
                ..theme.text
            },
            Self::Choice { checked: true } => ON_ACCENT,
            Self::Enabled | Self::Choice { checked: false } => theme.text,
        }
    }
}

/// The keyboard's place in an open popup menu: one focus per row of the rows it opened with.
pub(crate) struct MenuFocus(Vec<FocusHandle>);

impl MenuFocus {
    /// A menu of `rows`, with the keyboard on the checked row, or else on the first that can
    /// be picked.
    pub fn open(rows: &[MenuRow], window: &mut Window, cx: &mut App) -> Self {
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
    pub fn element(
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
                    .rounded(px(7.0))
                    .text_size(px(13.0))
                    .text_color(ink)
                    .when(row.state.enabled(), |element| {
                        filled(element.track_focus(item), fill)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d155_the_checked_choice_keeps_its_accent_under_the_pointer() {
        assert!(!RowState::Choice { checked: true }.hovers());
        assert!(RowState::Choice { checked: false }.hovers());
        assert!(RowState::Enabled.hovers());
        assert!(!RowState::Dimmed.hovers());
    }
}
