use std::borrow::Cow;
use std::rc::Rc;

use cleanshot_core::{
    Change, CommandError, History, KeyAction, Output, ShownTools, Tool, ToolGroup,
};
use gpui::prelude::FluentBuilder;
use gpui::{
    AccessibleAction, Anchor, App, AssetSource, BoxShadow, Div, ElementId, FocusHandle, Global,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Pixels, Rgba, Role, SharedString,
    Stateful, StatefulInteractiveElement, Styled, TextStyle, Toggled, Window, anchored, deferred,
    div, hsla, point, px, svg,
};

use crate::menu::{MenuFocus, MenuRow, RowState};
use crate::theme::{ACCENT, Fill, ON_ACCENT, Theme, filled};
use crate::tooltip::{tooltip_text, with_tooltip};

/// The Lucide icons the editor draws, embedded in the exe under their Lucide names.
pub(crate) struct Icons;

const ICONS: [(&str, &[u8]); 16] = [
    ("ellipsis", include_bytes!("../icons/ellipsis.svg")),
    ("chevron-down", include_bytes!("../icons/chevron-down.svg")),
    (
        "mouse-pointer-2",
        include_bytes!("../icons/mouse-pointer-2.svg"),
    ),
    (
        "move-up-right",
        include_bytes!("../icons/move-up-right.svg"),
    ),
    ("minus", include_bytes!("../icons/minus.svg")),
    ("square", include_bytes!("../icons/square.svg")),
    ("circle", include_bytes!("../icons/circle.svg")),
    ("pencil", include_bytes!("../icons/pencil.svg")),
    ("highlighter", include_bytes!("../icons/highlighter.svg")),
    ("type", include_bytes!("../icons/type.svg")),
    ("list-ordered", include_bytes!("../icons/list-ordered.svg")),
    (
        "message-square",
        include_bytes!("../icons/message-square.svg"),
    ),
    (
        "square-dashed",
        include_bytes!("../icons/square-dashed.svg"),
    ),
    ("focus", include_bytes!("../icons/focus.svg")),
    ("crop", include_bytes!("../icons/crop.svg")),
    ("copy", include_bytes!("../icons/copy.svg")),
];

impl AssetSource for Icons {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let name = path
            .strip_prefix("icons/")
            .and_then(|file| file.strip_suffix(".svg"));
        Ok(ICONS
            .iter()
            .find(|(icon, _)| Some(*icon) == name)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .map(|(icon, _)| SharedString::from(format!("icons/{icon}.svg")))
            .filter(|asset| asset.starts_with(path))
            .collect())
    }
}

/// Each tool's Lucide icon and its key.
fn icon_and_key(tool: Tool) -> (&'static str, &'static str) {
    match tool {
        Tool::Select => ("mouse-pointer-2", "V"),
        Tool::Arrow => ("move-up-right", "A"),
        Tool::Line => ("minus", "L"),
        Tool::Rectangle => ("square", "R"),
        Tool::Ellipse => ("circle", "O"),
        Tool::Pen => ("pencil", "P"),
        Tool::Highlighter => ("highlighter", "H"),
        Tool::Text => ("type", "T"),
        Tool::Step => ("list-ordered", "N"),
        Tool::Bubble => ("message-square", "B"),
        Tool::Redact => ("square-dashed", "X"),
        Tool::Spotlight => ("focus", "S"),
        Tool::Crop => ("crop", "C"),
    }
}

/// The group a group button stands for, as its help text says it (D-143).
fn group_name(group: ToolGroup) -> &'static str {
    match group {
        ToolGroup::Select => "Select",
        ToolGroup::Shapes => "Shapes",
        ToolGroup::Text => "Text",
        ToolGroup::Attention => "Attention",
        ToolGroup::Redact => "Redact",
        ToolGroup::Crop => "Crop",
    }
}

/// The tool each group's button shows, for as long as the app runs (D-143).
#[derive(Default)]
struct Shown(ShownTools);

impl Global for Shown {}

pub(crate) fn shown_tools(cx: &App) -> ShownTools {
    cx.try_global::<Shown>()
        .map_or_else(ShownTools::default, |Shown(shown)| *shown)
}

/// Shows `tool` on its group's button from now on.
pub(crate) fn show_tool(tool: Tool, cx: &mut App) {
    cx.default_global::<Shown>().0.pick(tool);
}

/// A menu the caption row's buttons open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolbarMenu {
    /// A tool group's tools (D-143).
    Tools(ToolGroup),
    /// The "..." menu (D-144).
    More,
}

/// The menu buttons' keyboard places, and the menu open on one of them (D-143, D-144).
pub(crate) struct ToolbarMenus {
    groups: [FocusHandle; 6],
    more: FocusHandle,
    open: Option<(ToolbarMenu, MenuFocus)>,
}

impl ToolbarMenus {
    pub fn new(cx: &mut App) -> Self {
        Self {
            groups: ToolGroup::ALL.map(|_| cx.focus_handle()),
            more: cx.focus_handle(),
            open: None,
        }
    }

    /// Opens `menu` with `rows`, the keyboard on the checked row or else the first that can be
    /// picked.
    pub fn open(&mut self, menu: ToolbarMenu, rows: &[MenuRow], window: &mut Window, cx: &mut App) {
        self.open = Some((menu, MenuFocus::open(rows, window, cx)));
    }

    /// Closes the open menu, if there is one, the keyboard back on its button.
    pub fn close(&mut self, window: &mut Window, cx: &mut App) {
        if let Some((menu, _)) = self.open.take() {
            window.focus(self.button(menu), cx);
        }
    }

    fn button(&self, menu: ToolbarMenu) -> &FocusHandle {
        match menu {
            ToolbarMenu::Tools(group) => &self.groups[group as usize],
            ToolbarMenu::More => &self.more,
        }
    }

    fn open_on(&self, menu: ToolbarMenu) -> Option<&MenuFocus> {
        self.open
            .as_ref()
            .filter(|(open, _)| *open == menu)
            .map(|(_, focus)| focus)
    }
}

/// A group's menu: each of its tools with its icon, name and key, `current` checked.
pub(crate) fn group_rows(group: ToolGroup, current: Tool) -> Vec<MenuRow> {
    group
        .tools()
        .iter()
        .map(|&tool| {
            let (icon, keys) = icon_and_key(tool);
            MenuRow {
                icon: Some(icon),
                name: tool_name(tool).into(),
                keys,
                state: RowState::Choice {
                    checked: tool == current,
                },
            }
        })
        .collect()
}

/// The tool's accessible name, which its toolbar button and the undo step both use.
fn tool_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => "Select",
        Tool::Arrow => "Arrow",
        Tool::Line => "Line",
        Tool::Rectangle => "Rectangle",
        Tool::Ellipse => "Ellipse",
        Tool::Pen => "Pen",
        Tool::Highlighter => "Highlighter",
        Tool::Text => "Text",
        Tool::Step => "Numbered step",
        Tool::Bubble => "Speech bubble",
        Tool::Redact => "Redact",
        Tool::Spotlight => "Spotlight",
        Tool::Crop => "Crop",
    }
}

/// Undo or redo on `history`, as the menu row and the key do it.
pub(crate) type Replay = fn(&mut History) -> Result<bool, CommandError>;

/// When a "..." menu row can be picked.
#[derive(Clone, Copy)]
enum Availability {
    Always,
    /// While there is a step to make; the row names it (D-116).
    Step(fn(&History) -> Option<Change>),
    /// Once the app has said how to open the settings window.
    Settings,
    /// While fitting would zoom to other than actual size (D-167).
    Fit,
}

/// The "..." menu's rows (D-144): name, shortcut, the action the shortcut also takes, and
/// when the row can be picked.
const MORE: [(&str, &str, KeyAction, Availability); 7] = [
    (
        "Save",
        "Ctrl+S",
        KeyAction::Deliver(Output::Save),
        Availability::Always,
    ),
    (
        "Save As",
        "Ctrl+Shift+S",
        KeyAction::SaveAs,
        Availability::Always,
    ),
    (
        "Undo",
        "Ctrl+Z",
        KeyAction::Undo,
        Availability::Step(History::to_undo),
    ),
    (
        "Redo",
        "Ctrl+Y",
        KeyAction::Redo,
        Availability::Step(History::to_redo),
    ),
    (
        "Fit to window",
        "Ctrl+0",
        KeyAction::FitView,
        Availability::Fit,
    ),
    (
        "Actual size",
        "Ctrl+1",
        KeyAction::ActualSize,
        Availability::Always,
    ),
    (
        "Settings",
        "Ctrl+,",
        KeyAction::OpenSettings,
        Availability::Settings,
    ),
];

/// The "..." menu for `history`: Undo and Redo name their step ("Undo arrow", D-116) and
/// are dimmed with no step to make; Settings is dimmed unless `can_open_settings`; Fit to
/// window is dimmed while `fit_is_actual_size` (D-167).
pub(crate) fn more_rows(
    history: &History,
    can_open_settings: bool,
    fit_is_actual_size: bool,
) -> Vec<MenuRow> {
    MORE.iter()
        .map(|&(name, keys, _, availability)| {
            let (name, can_pick) = match availability {
                Availability::Always => (name.into(), true),
                Availability::Step(next) => next(history).map_or((name.into(), false), |change| {
                    (step_name(name, change).into(), true)
                }),
                Availability::Settings => (name.into(), can_open_settings),
                Availability::Fit => (name.into(), !fit_is_actual_size),
            };
            MenuRow {
                icon: None,
                name,
                keys,
                state: if can_pick {
                    RowState::Enabled
                } else {
                    RowState::Dimmed
                },
            }
        })
        .collect()
}

/// The step's name: its verb and what it would change, as in "Undo arrow" (D-116).
fn step_name(verb: &str, change: Change) -> String {
    let changed = match change {
        Change::Shape(tool) => tool_name(tool).to_lowercase(),
        Change::Crop => "crop".to_owned(),
        Change::Dim => "spotlight dim".to_owned(),
    };
    format!("{verb} {changed}")
}

/// The tool groups (CHARTER 4: 28 px buttons with 7 px corners), each showing the tool
/// `shown` gives it, `menus`' menu open under its button. `on_pick` is called with the tool
/// clicked or picked from a menu, `on_menu` with the menu to open, or `None` to close it.
pub(crate) fn toolbar<Pick, Menu>(
    current: Tool,
    shown: ShownTools,
    menus: &ToolbarMenus,
    theme: Theme,
    on_pick: Pick,
    on_menu: Menu,
) -> impl IntoElement + use<Pick, Menu>
where
    Pick: Fn(Tool, &mut Window, &mut App) + Clone + 'static,
    Menu: Fn(Option<ToolbarMenu>, &mut Window, &mut App) + Clone + 'static,
{
    let buttons = ToolGroup::ALL.map(|group| {
        group_button(
            group,
            current,
            shown.shown(group),
            menus,
            theme,
            on_pick.clone(),
            on_menu.clone(),
        )
    });
    div()
        .flex()
        .items_center()
        .flex_none()
        .id("toolbar")
        .role(Role::Toolbar)
        .aria_label("Tools")
        .gap(TOOL_GAP)
        .children(buttons)
}

/// The caption row's sizes, which the window's smallest width adds up (D-161).
const BUTTON: Pixels = px(28.0);
const MENU_BUTTON_WIDTH: Pixels = px(38.0);
const TOOL_GAP: Pixels = px(4.0);
const ICON: Pixels = px(16.0);
const COPY: &str = "Copy";
const COPY_TEXT: Pixels = px(13.0);
const COPY_PADDING: Pixels = px(14.0);
const COPY_GAP: Pixels = px(6.0);
const ACTIONS_GAP: Pixels = px(8.0);
const ACTIONS_END: Pixels = px(8.0);

/// The toolbar's width: its buttons and the gaps between them (D-161).
pub(crate) fn toolbar_width() -> Pixels {
    ToolGroup::ALL
        .map(|group| {
            if group.tools().len() == 1 {
                BUTTON
            } else {
                MENU_BUTTON_WIDTH
            }
        })
        .into_iter()
        .fold(-TOOL_GAP, |width, button| width + TOOL_GAP + button)
}

/// The width of Copy and "...", Copy's word measured in the font it is drawn in (D-161).
pub(crate) fn actions_width(cx: &App) -> Pixels {
    let text = cx.text_system();
    let font = text.resolve_font(&TextStyle::default().font());
    let word = COPY
        .chars()
        .map(|letter| {
            text.advance(font, COPY_TEXT, letter)
                .expect("the system font has Copy's letters")
                .width
        })
        .fold(px(0.0), |word, letter| word + letter);
    COPY_PADDING * 2.0 + ICON + COPY_GAP + word + ACTIONS_GAP + BUTTON + ACTIONS_END
}

/// Copy, the one primary button, as a pill, then "..." with `more` under it while `menus`
/// has it open (D-144). `on_action` is called with Copy's action or the row picked from the
/// menu, `on_menu` with the menu to open, or `None` to close it.
pub(crate) fn actions<Act, Menu>(
    menus: &ToolbarMenus,
    more: &[MenuRow],
    theme: Theme,
    on_action: Act,
    on_menu: Menu,
) -> impl IntoElement + use<Act, Menu>
where
    Act: Fn(KeyAction, &mut Window, &mut App) + Clone + 'static,
    Menu: Fn(Option<ToolbarMenu>, &mut Window, &mut App) + Clone + 'static,
{
    let on_copy = on_action.clone();
    let copy = div()
        .id("Copy")
        .role(Role::Button)
        .aria_label("Copy")
        .aria_keyshortcuts("Ctrl+C")
        .focusable()
        .tab_stop(true)
        .map(|element| with_tooltip(element, tooltip_text("Copy", Some("Ctrl+C")), theme))
        .h(BUTTON)
        .px(COPY_PADDING)
        .gap(COPY_GAP)
        .flex()
        .items_center()
        .rounded_full()
        .text_size(COPY_TEXT)
        .map(|button| filled(button, Fill::Accent))
        .hover(|button| button.opacity(0.9))
        .on_click(move |_, window, cx| on_copy(KeyAction::Deliver(Output::Copy), window, cx))
        .child(icon_of("copy", ON_ACCENT))
        .child(COPY);
    let focus = menus.open_on(ToolbarMenu::More);
    let open = focus.is_some();
    let on_click = on_menu.clone();
    let on_key = on_menu.clone();
    let on_expand = on_menu.clone();
    let button = button("More")
        .track_focus(&menus.more)
        .tab_stop(true)
        .role(Role::Button)
        .aria_label("More")
        .map(|button| {
            expandable(button, open, move |value, window, cx| {
                on_expand(value.then_some(ToolbarMenu::More), window, cx);
            })
        })
        .map(|element| with_tooltip(element, tooltip_text("More", None), theme))
        .focus_visible(|button| button.border_2().border_color(ACCENT))
        .hover(move |button| button.bg(theme.hover))
        .on_click(move |_, window, cx| on_click((!open).then_some(ToolbarMenu::More), window, cx))
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            if event.keystroke.key == "down" && event.keystroke.modifiers.alt {
                on_key(Some(ToolbarMenu::More), window, cx);
                cx.stop_propagation();
            }
        })
        .child(icon_of("ellipsis", theme.text));
    let menu = focus.map(|focus| {
        let menu = focus.element(
            "more-menu",
            "More",
            more,
            theme,
            move |picked: Option<usize>, window: &mut Window, cx: &mut App| match picked {
                Some(index) => on_action(MORE[index].2, window, cx),
                None => on_menu(None, window, cx),
            },
        );
        div().absolute().top(px(32.0)).right_0().child(deferred(
            anchored()
                .anchor(Anchor::TopRight)
                .snap_to_window()
                .child(menu),
        ))
    });
    div()
        .flex()
        .items_center()
        .flex_none()
        .gap(ACTIONS_GAP)
        .pr(ACTIONS_END)
        .child(copy)
        .child(div().relative().child(button).children(menu))
}

/// `group`'s button, showing `tool`, with its menu under it while `menus` has it open
/// (D-143). A group of one tool picks it; a larger group opens its menu on a click, Enter,
/// Space or Alt+Down.
fn group_button(
    group: ToolGroup,
    current: Tool,
    tool: Tool,
    menus: &ToolbarMenus,
    theme: Theme,
    on_pick: impl Fn(Tool, &mut Window, &mut App) + Clone + 'static,
    on_menu: impl Fn(Option<ToolbarMenu>, &mut Window, &mut App) + Clone + 'static,
) -> Div {
    let (icon, key) = icon_and_key(tool);
    let name = tool_name(tool);
    let picked = ToolGroup::of(current) == group;
    let focus = menus.open_on(ToolbarMenu::Tools(group));
    let color = if picked { ON_ACCENT } else { theme.text };
    let button = button(ElementId::Name(group_name(group).into()))
        .track_focus(&menus.groups[group as usize])
        .tab_stop(true)
        .role(Role::RadioButton)
        .aria_label(name)
        .aria_keyshortcuts(key)
        .map(|element| with_tooltip(element, tooltip_text(name, Some(key)), theme))
        .aria_toggled(if picked {
            Toggled::True
        } else {
            Toggled::False
        })
        .map(|button| filled(button, if picked { Fill::Accent } else { Fill::Neutral }))
        .when(!picked, |button| {
            button.hover(move |button| button.bg(theme.hover))
        })
        .child(icon_of(icon, color));
    if group.tools().len() == 1 {
        return div().child(button.on_click(move |_, window, cx| on_pick(tool, window, cx)));
    }
    let open = focus.is_some();
    let on_key = on_menu.clone();
    let on_click = on_menu.clone();
    let on_expand = on_menu.clone();
    let button = button
        .w(MENU_BUTTON_WIDTH)
        .gap(px(1.0))
        .map(|button| {
            expandable(button, open, move |value, window, cx| {
                on_expand(value.then_some(ToolbarMenu::Tools(group)), window, cx);
            })
        })
        .aria_description(format!(
            "{}. Alt+Down opens the other tools",
            group_name(group)
        ))
        .on_click(move |_, window, cx| {
            on_click((!open).then_some(ToolbarMenu::Tools(group)), window, cx);
        })
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            if event.keystroke.key == "down" && event.keystroke.modifiers.alt {
                on_key(Some(ToolbarMenu::Tools(group)), window, cx);
                cx.stop_propagation();
            }
        })
        .child(
            svg()
                .path("icons/chevron-down.svg")
                .size(px(8.0))
                .text_color(color),
        );
    div().relative().child(button).children(focus.map(|focus| {
        let tools = group.tools();
        let menu = focus.element(
            "tool-menu",
            group_name(group),
            &group_rows(group, current),
            theme,
            move |picked: Option<usize>, window: &mut Window, cx: &mut App| match picked {
                Some(index) => on_pick(tools[index], window, cx),
                None => on_menu(None, window, cx),
            },
        );
        div()
            .absolute()
            .top(px(32.0))
            .left_0()
            .child(deferred(anchored().snap_to_window().child(menu)))
    }))
}

/// CHARTER 4: a 28 px toolbar button with 7 px corners, its icon centred.
fn button(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .size(BUTTON)
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.0))
}

pub(crate) fn divider(theme: Theme) -> Div {
    div()
        .w(px(1.0))
        .h(px(20.0))
        .mx(DIVIDER_MARGIN)
        .bg(theme.hairline)
}

const DIVIDER_MARGIN: Pixels = px(2.0);
/// The divider's line and the margin each side of it.
pub(crate) const DIVIDER_WIDTH: Pixels = px(1.0 + 2.0 * 2.0);

pub(crate) fn icon_of(icon: &str, color: Rgba) -> impl IntoElement {
    svg()
        .path(format!("icons/{icon}.svg"))
        .size(ICON)
        .text_color(color)
}

/// `button`, which opens a menu or a popup, as a screen reader works it (D-168): UI
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

/// A floating panel (CHARTER 4): dark material, hairline border, soft shadow, 12 px corners,
/// laying its children out in a row.
pub(crate) fn panel(theme: Theme) -> Div {
    div()
        .occlude()
        .flex()
        .items_center()
        .p(px(6.0))
        .rounded(px(12.0))
        .bg(theme.material)
        .border_1()
        .border_color(theme.hairline)
        .shadow(vec![BoxShadow {
            color: hsla(0.0, 0.0, 0.0, 0.25),
            offset: point(px(0.0), px(8.0)),
            blur_radius: px(24.0),
            spread_radius: px(0.0),
            inset: false,
        }])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_tool() -> impl Iterator<Item = Tool> {
        ToolGroup::ALL
            .into_iter()
            .flat_map(|group| group.tools().iter().copied())
    }

    #[test]
    fn f30_every_tool_has_an_embedded_icon_and_names_its_key() {
        for tool in every_tool() {
            let (icon, key) = icon_and_key(tool);
            let svg = Icons.load(&format!("icons/{icon}.svg")).unwrap().unwrap();
            assert!(
                svg.starts_with(b"<!-- @license lucide-static v1.65.0"),
                "{icon}"
            );
            let key = key.chars().next().unwrap();
            assert_eq!(Tool::for_key(key), Some(tool), "{}", tool_name(tool));
        }
        for icon in ["chevron-down", "ellipsis", "copy"] {
            assert!(
                Icons.load(&format!("icons/{icon}.svg")).unwrap().is_some(),
                "{icon}"
            );
        }
        assert_eq!(Icons.load("icons/not-an-icon.svg").unwrap(), None);
    }

    #[test]
    fn d116_an_undo_or_redo_is_named_for_what_it_changes() {
        assert_eq!(step_name("Undo", Change::Shape(Tool::Arrow)), "Undo arrow");
        assert_eq!(
            step_name("Redo", Change::Shape(Tool::Bubble)),
            "Redo speech bubble"
        );
        assert_eq!(step_name("Undo", Change::Crop), "Undo crop");
        assert_eq!(step_name("Undo", Change::Dim), "Undo spotlight dim");
    }

    #[test]
    fn d144_each_more_menu_row_does_what_the_shortcut_it_shows_does() {
        use cleanshot_core::{Activity, Modifiers, action_for_key};

        for (name, keys, action, _) in MORE {
            let key = keys.rsplit('+').next().unwrap().to_lowercase();
            let modifiers = Modifiers {
                ctrl: keys.starts_with("Ctrl+"),
                alt: false,
                shift: keys.contains("Shift+"),
                win: false,
            };
            assert_eq!(
                action_for_key(&key, modifiers, Activity::Idle),
                Some(action),
                "{name}: {keys}"
            );
        }
    }

    #[test]
    fn d144_undo_redo_and_settings_rows_are_dimmed_until_they_can_act() {
        use std::sync::Arc;

        use cleanshot_core::{Command, Document, RectF, RgbaImage};

        let image = RgbaImage::new(8, 8, vec![0; 8 * 8 * 4]).unwrap();
        let mut history = History::new(Document::new(Arc::new(image)));
        let states = |rows: Vec<MenuRow>| -> Vec<(String, RowState)> {
            rows.into_iter()
                .map(|row| (row.name.to_string(), row.state))
                .collect()
        };
        let fresh = states(more_rows(&history, false, false));
        assert_eq!(fresh[2], ("Undo".to_owned(), RowState::Dimmed));
        assert_eq!(fresh[3], ("Redo".to_owned(), RowState::Dimmed));
        assert_eq!(fresh[6], ("Settings".to_owned(), RowState::Dimmed));
        assert!(
            [0, 1, 4, 5]
                .iter()
                .all(|&row| fresh[row].1 == RowState::Enabled)
        );
        let crop = RectF {
            x: 1.0,
            y: 1.0,
            width: 4.0,
            height: 4.0,
        };
        history.apply(Command::SetCrop(Some(crop))).unwrap();
        let edited = states(more_rows(&history, true, false));
        assert_eq!(edited[2], ("Undo crop".to_owned(), RowState::Enabled));
        assert_eq!(edited[3], ("Redo".to_owned(), RowState::Dimmed));
        assert_eq!(edited[6], ("Settings".to_owned(), RowState::Enabled));
    }

    #[test]
    fn d167_fit_to_window_is_dimmed_while_it_would_show_actual_size() {
        use std::sync::Arc;

        use cleanshot_core::{Document, RgbaImage};

        let image = RgbaImage::new(8, 8, vec![0; 8 * 8 * 4]).unwrap();
        let history = History::new(Document::new(Arc::new(image)));
        let fit = |fit_is_actual_size| {
            let rows = more_rows(&history, true, fit_is_actual_size);
            (rows[4].name.to_string(), rows[4].state)
        };
        assert_eq!(fit(true), ("Fit to window".to_owned(), RowState::Dimmed));
        assert_eq!(fit(false), ("Fit to window".to_owned(), RowState::Enabled));
    }

    #[test]
    fn f58_every_tool_has_its_own_accessible_name_and_key() {
        let names: std::collections::HashSet<_> = every_tool().map(tool_name).collect();
        let keys: std::collections::HashSet<_> =
            every_tool().map(|tool| icon_and_key(tool).1).collect();
        assert_eq!(names.len(), 13);
        assert_eq!(keys.len(), 13);
    }
}
