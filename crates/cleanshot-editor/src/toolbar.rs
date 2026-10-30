use std::borrow::Cow;

use cleanshot_core::Tool;
use gpui::prelude::FluentBuilder;
use gpui::{
    AssetSource, BoxShadow, ElementId, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, hsla, point, px, svg,
};

use crate::theme::{ACCENT, ON_ACCENT, Theme};

/// The Lucide icons the editor draws, embedded in the exe under their Lucide names.
pub(crate) struct Icons;

const ICONS: [(&str, &[u8]); 9] = [
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
    (
        "square-dashed",
        include_bytes!("../icons/square-dashed.svg"),
    ),
    ("crop", include_bytes!("../icons/crop.svg")),
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

/// The tools in toolbar order, each with its Lucide icon and its name as the user reads it.
pub(crate) const TOOLS: [(Tool, &str, &str); 9] = [
    (Tool::Select, "mouse-pointer-2", "Select (V)"),
    (Tool::Arrow, "move-up-right", "Arrow (A)"),
    (Tool::Line, "minus", "Line (L)"),
    (Tool::Rectangle, "square", "Rectangle (R)"),
    (Tool::Ellipse, "circle", "Ellipse (O)"),
    (Tool::Pen, "pencil", "Pen (P)"),
    (Tool::Highlighter, "highlighter", "Highlighter (H)"),
    (Tool::Redact, "square-dashed", "Redact (X)"),
    (Tool::Crop, "crop", "Crop (C)"),
];

/// The floating tool panel (CHARTER 4: dark material, hairline border, soft shadow, 12 px
/// corners, 28 px buttons with 7 px corners). `on_pick` is called with the tool clicked.
pub(crate) fn toolbar(
    current: Tool,
    theme: Theme,
    on_pick: impl Fn(Tool, &mut gpui::Window, &mut gpui::App) + Clone + 'static,
) -> impl IntoElement {
    let buttons = TOOLS.map(|(tool, icon, name)| {
        let on_pick = on_pick.clone();
        let picked = tool == current;
        div()
            .id(ElementId::Name(name.into()))
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
            .child(
                svg()
                    .path(format!("icons/{icon}.svg"))
                    .size(px(16.0))
                    .text_color(if picked { ON_ACCENT } else { theme.text }),
            )
    });
    div()
        .absolute()
        .top(px(16.0))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(
            div()
                .id("toolbar")
                .occlude()
                .flex()
                .gap(px(4.0))
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
                .children(buttons),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f30_every_toolbar_button_has_an_embedded_icon_and_names_its_key() {
        for (tool, icon, name) in TOOLS {
            let svg = Icons.load(&format!("icons/{icon}.svg")).unwrap().unwrap();
            assert!(
                svg.starts_with(b"<!-- @license lucide-static v1.65.0"),
                "{icon}"
            );
            let key = name.chars().rev().nth(1).unwrap();
            assert_eq!(Tool::for_key(key), Some(tool), "{name}");
        }
        assert_eq!(Icons.load("icons/not-an-icon.svg").unwrap(), None);
    }
}
