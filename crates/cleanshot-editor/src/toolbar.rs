use std::borrow::Cow;

use cleanshot_core::{Change, CommandError, History, Tool};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyView, App, AppContext, AssetSource, BoxShadow, Context, Div, ElementId, InteractiveElement,
    IntoElement, ParentElement, Render, Rgba, Role, SharedString, StatefulInteractiveElement,
    Styled, Toggled, Window, div, hsla, point, px, svg,
};

use crate::theme::{ACCENT, ON_ACCENT, Theme};

/// The Lucide icons the editor draws, embedded in the exe under their Lucide names.
pub(crate) struct Icons;

const ICONS: [(&str, &[u8]); 15] = [
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
    ("undo-2", include_bytes!("../icons/undo-2.svg")),
    ("redo-2", include_bytes!("../icons/redo-2.svg")),
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

/// The tools in toolbar order, each with its Lucide icon and its key.
pub(crate) const TOOLS: [(Tool, &str, char); 13] = [
    (Tool::Select, "mouse-pointer-2", 'V'),
    (Tool::Arrow, "move-up-right", 'A'),
    (Tool::Line, "minus", 'L'),
    (Tool::Rectangle, "square", 'R'),
    (Tool::Ellipse, "circle", 'O'),
    (Tool::Pen, "pencil", 'P'),
    (Tool::Highlighter, "highlighter", 'H'),
    (Tool::Text, "type", 'T'),
    (Tool::Step, "list-ordered", 'N'),
    (Tool::Bubble, "message-square", 'B'),
    (Tool::Redact, "square-dashed", 'X'),
    (Tool::Spotlight, "focus", 'S'),
    (Tool::Crop, "crop", 'C'),
];

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

/// Undo or redo on `history`, as the toolbar button and the key do it.
pub(crate) type Replay = fn(&mut History) -> Result<bool, CommandError>;

/// What [`Replay`] would change next, or `None` when there is no step to make.
type NextChange = fn(&History) -> Option<Change>;

/// The undo and redo buttons (D-116): icon, verb, shortcut, what the step would change, and
/// the step itself.
const REPLAYS: [(&str, &str, &str, NextChange, Replay); 2] = [
    ("undo-2", "Undo", "Ctrl+Z", History::to_undo, History::undo),
    ("redo-2", "Redo", "Ctrl+Y", History::to_redo, History::redo),
];

/// The step's name: its verb and what it would change, as in "Undo arrow" (D-116).
fn step_name(verb: &str, change: Change) -> String {
    let changed = match change {
        Change::Shape(tool) => tool_name(tool).to_lowercase(),
        Change::Crop => "crop".to_owned(),
        Change::Frame => "image frame".to_owned(),
    };
    format!("{verb} {changed}")
}

/// The tool panel (CHARTER 4: 28 px buttons with 7 px corners), then Undo and Redo for
/// `history`. `on_pick` is called with the tool clicked, `on_replay` with the step clicked.
pub(crate) fn toolbar(
    current: Tool,
    history: &History,
    theme: Theme,
    on_pick: impl Fn(Tool, &mut gpui::Window, &mut gpui::App) + Clone + 'static,
    on_replay: impl Fn(Replay, &mut gpui::Window, &mut gpui::App) + Clone + 'static,
) -> impl IntoElement {
    let replays = REPLAYS.map(|(icon, verb, keys, next, replay)| {
        let step = next(history).map(|change| step_name(verb, change));
        let button = div()
            .id(verb)
            .role(Role::Button)
            .aria_label(step.clone().unwrap_or_else(|| verb.to_owned()))
            .aria_keyshortcuts(keys)
            .size(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.0));
        let Some(step) = step else {
            return button.child(icon_of(
                icon,
                Rgba {
                    a: 0.4,
                    ..theme.text
                },
            ));
        };
        let on_replay = on_replay.clone();
        let tip = SharedString::from(format!("{step} ({keys})"));
        button
            .focusable()
            .tab_stop(true)
            .focus_visible(|button| button.border_2().border_color(ACCENT))
            .hover(move |button| button.bg(theme.hover))
            .tooltip(move |_, cx| Tooltip::view(tip.clone(), theme, cx))
            .on_click(move |_, window, cx| on_replay(replay, window, cx))
            .child(icon_of(icon, theme.text))
    });
    let buttons = TOOLS.map(|(tool, icon, key)| {
        let name = tool_name(tool);
        let on_pick = on_pick.clone();
        let picked = tool == current;
        div()
            .id(ElementId::Name(name.into()))
            .focusable()
            .tab_stop(true)
            .role(Role::RadioButton)
            .aria_label(name)
            .aria_keyshortcuts(key.to_string())
            .aria_toggled(if picked {
                Toggled::True
            } else {
                Toggled::False
            })
            .focus_visible(|button| button.border_2().border_color(ACCENT))
            .size(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.0))
            .when(picked, |button| button.bg(ACCENT))
            .when(!picked, |button| {
                button.hover(move |button| button.bg(theme.hover))
            })
            .on_click(move |_, window, cx| on_pick(tool, window, cx))
            .child(icon_of(icon, if picked { ON_ACCENT } else { theme.text }))
    });
    panel(theme)
        .id("toolbar")
        .role(Role::Toolbar)
        .aria_label("Tools")
        .gap(px(4.0))
        .children(buttons)
        .child(div().w(px(1.0)).h(px(20.0)).mx(px(2.0)).bg(theme.hairline))
        .children(replays)
}

fn icon_of(icon: &str, color: Rgba) -> impl IntoElement {
    svg()
        .path(format!("icons/{icon}.svg"))
        .size(px(16.0))
        .text_color(color)
}

/// The text shown while the pointer rests on a toolbar button.
struct Tooltip {
    text: SharedString,
    theme: Theme,
}

impl Tooltip {
    fn view(text: SharedString, theme: Theme, cx: &mut App) -> AnyView {
        cx.new(|_| Self { text, theme }).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(7.0))
            .bg(self.theme.material)
            .border_1()
            .border_color(self.theme.hairline)
            .text_color(self.theme.text)
            .text_size(px(12.0))
            .child(self.text.clone())
    }
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

    #[test]
    fn f30_every_toolbar_button_has_an_embedded_icon_and_names_its_key() {
        for (tool, icon, key) in TOOLS {
            let svg = Icons.load(&format!("icons/{icon}.svg")).unwrap().unwrap();
            assert!(
                svg.starts_with(b"<!-- @license lucide-static v1.65.0"),
                "{icon}"
            );
            assert_eq!(Tool::for_key(key), Some(tool), "{}", tool_name(tool));
        }
        assert_eq!(Icons.load("icons/not-an-icon.svg").unwrap(), None);
    }

    #[test]
    fn d116_undo_and_redo_name_the_step_and_their_shortcuts_make_it() {
        use cleanshot_core::{Activity, KeyAction, Modifiers, action_for_key};

        assert_eq!(step_name("Undo", Change::Shape(Tool::Arrow)), "Undo arrow");
        assert_eq!(
            step_name("Redo", Change::Shape(Tool::Bubble)),
            "Redo speech bubble"
        );
        assert_eq!(step_name("Undo", Change::Crop), "Undo crop");
        assert_eq!(step_name("Redo", Change::Frame), "Redo image frame");
        let ctrl = Modifiers {
            ctrl: true,
            alt: false,
            shift: false,
            win: false,
        };
        let actions = [KeyAction::Undo, KeyAction::Redo];
        for ((icon, verb, keys, ..), action) in REPLAYS.into_iter().zip(actions) {
            assert!(Icons.load(&format!("icons/{icon}.svg")).unwrap().is_some());
            let key = keys.strip_prefix("Ctrl+").unwrap().to_lowercase();
            assert_eq!(
                action_for_key(&key, ctrl, Activity::Idle),
                Some(action),
                "{verb}"
            );
        }
    }

    #[test]
    fn f58_every_toolbar_button_has_its_own_accessible_name_and_key() {
        let names: std::collections::HashSet<_> =
            TOOLS.iter().map(|tool| tool_name(tool.0)).collect();
        let keys: std::collections::HashSet<_> = TOOLS.iter().map(|tool| tool.2).collect();
        assert_eq!(names.len(), TOOLS.len());
        assert_eq!(keys.len(), TOOLS.len());
    }
}
