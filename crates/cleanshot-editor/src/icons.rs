use std::borrow::Cow;

use gpui::{AssetSource, IntoElement, Pixels, Rgba, SharedString, Styled, px, svg};

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

pub(crate) const ICON: Pixels = px(16.0);

pub(crate) fn icon_of(icon: &str, color: Rgba) -> impl IntoElement {
    svg()
        .path(format!("icons/{icon}.svg"))
        .size(ICON)
        .text_color(color)
}
