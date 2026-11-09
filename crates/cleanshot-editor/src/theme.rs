use std::rc::Rc;
use std::time::Duration;

use cleanshot_core::{ContrastColors, chrome};
use gpui::{
    App, BoxShadow, Div, Global, InteractiveElement, Pixels, Rgba, Styled, WindowAppearance,
    WindowBackgroundAppearance, div, hsla, point, px, rgba,
};

use crate::canvas::rgba as core_color;

/// The editor's colours for one system appearance, and how long the pointer
/// rests on a control before its tooltip shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Theme {
    /// The window round the image's card: a tint over the blurred desktop on glass.
    pub backdrop: Rgba,
    /// The image's card, where the image does not cover it.
    pub card: Rgba,
    /// Floating panels: menus and the notice.
    pub material: Rgba,
    pub hairline: Rgba,
    pub text: Rgba,
    /// Text and icons of a control that cannot be used.
    pub dimmed: Rgba,
    /// Under a toolbar button the pointer is over.
    pub hover: Rgba,
    /// The current tool, selection handles, the crop.
    pub accent: Rgba,
    /// What is drawn on the accent.
    pub on_accent: Rgba,
    /// Inside a selection handle.
    pub handle: Rgba,
    /// Under the Close caption button while the pointer is over it.
    pub close_hover: Rgba,
    /// The zoomed view's scroll bars.
    pub scroll_thumb: Rgba,
    /// Round the minimap's view and the colour picker's marker.
    pub frame: Rgba,
    /// Whether to leave off shadows and fading hovers, whose blends are no system colour.
    flat: bool,
    /// `None` leaves the delay to GPUI.
    pub tooltip_delay: Option<Duration>,
}

/// The corners of buttons, inputs and chips.
pub(crate) const CORNER: Pixels = px(chrome::CONTROL_CORNER);

/// Windows 11's close-button red, under the pointer.
const CLOSE_HOVER: Rgba = Rgba {
    r: 0.769,
    g: 0.169,
    b: 0.11,
    a: 1.0,
};

const HANDLE: Rgba = Rgba {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};

const SCROLL_THUMB: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.3,
};

/// What a control is filled with, which decides the colour of its keyboard focus ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ControlFill {
    /// The accent: the current tool, the checked menu row, Copy.
    Accent,
    /// The panel, or the hover grey, under the control.
    Neutral,
}

impl ControlFill {
    /// The keyboard focus ring that shows on this fill.
    fn ring(self, theme: Theme) -> Rgba {
        match self {
            Self::Accent => theme.on_accent,
            Self::Neutral => theme.accent,
        }
    }
}

/// `element` with `fill`, its text on it when that is the accent, and the keyboard focus
/// ring that shows on it.
pub(crate) fn filled<E: Styled + InteractiveElement>(
    element: E,
    fill: ControlFill,
    theme: Theme,
) -> E {
    let ring = fill.ring(theme);
    let element = element.focus_visible(move |control| control.border_2().border_color(ring));
    match fill {
        ControlFill::Accent => element.bg(theme.accent).text_color(theme.on_accent),
        ControlFill::Neutral => element,
    }
}

/// What a window shows behind its controls and the image's card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowSurface {
    /// The desktop behind the window, blurred and tinted.
    Glass,
    /// One opaque colour, for when Windows' transparency effects are off.
    Solid,
    /// The high-contrast theme's own colours, opaque.
    HighContrast(ContrastColors),
}

impl WindowSurface {
    pub(crate) fn background(self) -> WindowBackgroundAppearance {
        match self {
            Self::Glass => WindowBackgroundAppearance::Blurred,
            Self::Solid | Self::HighContrast(_) => WindowBackgroundAppearance::Opaque,
        }
    }
}

struct SurfaceReader(Rc<dyn Fn() -> WindowSurface>);

impl Global for SurfaceReader {}

/// Makes each window ask `read` which surface to open with. Until this is called,
/// windows open solid.
pub fn follow_window_surface(cx: &mut App, read: impl Fn() -> WindowSurface + 'static) {
    cx.set_global(SurfaceReader(Rc::new(read)));
}

/// The surface a window opening now gets.
pub(crate) fn window_surface(cx: &App) -> WindowSurface {
    cx.try_global::<SurfaceReader>()
        .map_or(WindowSurface::Solid, |SurfaceReader(read)| read())
}

impl Theme {
    pub(crate) fn for_appearance(
        appearance: WindowAppearance,
        surface: WindowSurface,
        tooltip_delay: Option<Duration>,
    ) -> Self {
        if let WindowSurface::HighContrast(colors) = surface {
            return Self::high_contrast(colors, tooltip_delay);
        }
        let glass = surface == WindowSurface::Glass;
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self {
                backdrop: if glass {
                    rgba(0x1e20_286b)
                } else {
                    rgba(0x2c2c_2eff)
                },
                card: rgba(0x1c1c_1eff),
                material: core_color(chrome::DARK.material),
                hairline: core_color(chrome::DARK.hairline),
                text: core_color(chrome::DARK.text),
                dimmed: rgba(0xf5f5_f766),
                hover: rgba(0xffff_ff1a),
                accent: core_color(chrome::ACCENT),
                on_accent: core_color(chrome::ON_ACCENT),
                handle: HANDLE,
                close_hover: CLOSE_HOVER,
                scroll_thumb: SCROLL_THUMB,
                frame: HANDLE,
                flat: false,
                tooltip_delay,
            },
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self {
                backdrop: if glass {
                    rgba(0xfafb_fd8c)
                } else {
                    rgba(0xe5e5_eaff)
                },
                card: rgba(0xf6f6_f8ff),
                material: core_color(chrome::LIGHT.material),
                hairline: core_color(chrome::LIGHT.hairline),
                text: core_color(chrome::LIGHT.text),
                dimmed: rgba(0x1d1d_1f66),
                hover: rgba(0x0000_0014),
                accent: core_color(chrome::ACCENT),
                on_accent: core_color(chrome::ON_ACCENT),
                handle: HANDLE,
                close_hover: CLOSE_HOVER,
                scroll_thumb: SCROLL_THUMB,
                frame: HANDLE,
                flat: false,
                tooltip_delay,
            },
        }
    }

    /// As high-contrast-parameter on Microsoft Learn asks: the text pair for everything, and
    /// Highlight for what must stand out. A hover changes nothing, since any tint
    /// under Window Text would lower its contrast.
    fn high_contrast(colors: ContrastColors, tooltip_delay: Option<Duration>) -> Self {
        let window = core_color(colors.window);
        let window_text = core_color(colors.window_text);
        let highlight = core_color(colors.highlight);
        Self {
            backdrop: window,
            card: window,
            material: window,
            hairline: window_text,
            text: window_text,
            dimmed: core_color(colors.gray_text),
            hover: window,
            accent: highlight,
            on_accent: core_color(colors.highlight_text),
            handle: window,
            close_hover: highlight,
            scroll_thumb: window_text,
            frame: highlight,
            flat: true,
            tooltip_delay,
        }
    }

    /// `shadows`, or none in a high-contrast theme.
    pub(crate) fn shadows(self, shadows: Vec<BoxShadow>) -> Vec<BoxShadow> {
        if self.flat { Vec::new() } else { shadows }
    }

    /// The opacity for a hover to fade a filled button to: `opacity`, or none of a fade in a
    /// high-contrast theme.
    pub(crate) fn hover_opacity(self, opacity: f32) -> f32 {
        if self.flat { 1.0 } else { opacity }
    }
}

/// A floating panel: dark material, hairline border, soft shadow, 12 px corners,
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
        .shadow(theme.shadows(vec![BoxShadow {
            color: hsla(0.0, 0.0, 0.0, 0.25),
            offset: point(px(0.0), px(8.0)),
            blur_radius: px(24.0),
            spread_radius: px(0.0),
            inset: false,
        }]))
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

#[cfg(test)]
mod tests {
    use cleanshot_core::Color;

    use super::*;

    #[test]
    fn the_focus_ring_shows_on_the_fill_it_rings() {
        for appearance in [WindowAppearance::Dark, WindowAppearance::Light] {
            let theme = Theme::for_appearance(appearance, WindowSurface::Solid, None);
            assert_ne!(ControlFill::Accent.ring(theme), theme.accent);
            for under in [theme.material, theme.hover, theme.backdrop] {
                assert_ne!(ControlFill::Neutral.ring(theme), under);
            }
        }
    }

    #[test]
    fn a_high_contrast_theme_paints_only_its_system_colours() {
        let [window, window_text, highlight, highlight_text, gray_text] =
            [0x10, 0x20, 0x30, 0x40, 0x50].map(|level| Color {
                r: level,
                g: level,
                b: level,
                a: 255,
            });
        let colors = ContrastColors {
            window,
            window_text,
            highlight,
            highlight_text,
            gray_text,
        };
        let shadow = BoxShadow {
            color: hsla(0.0, 0.0, 0.0, 1.0),
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(1.0),
            spread_radius: px(0.0),
            inset: false,
        };
        let plain = Theme::for_appearance(WindowAppearance::Dark, WindowSurface::Solid, None);
        assert_eq!(plain.shadows(vec![shadow.clone()]).len(), 1);
        assert!((plain.hover_opacity(0.5) - 0.5).abs() < f32::EPSILON);
        let surface = WindowSurface::HighContrast(colors);
        assert_eq!(surface.background(), WindowBackgroundAppearance::Opaque);
        for appearance in [WindowAppearance::Dark, WindowAppearance::Light] {
            let theme = Theme::for_appearance(appearance, surface, None);
            for chrome in [theme.backdrop, theme.card, theme.material, theme.hover] {
                assert_eq!(chrome, core_color(window), "{appearance:?}");
            }
            assert_eq!(theme.handle, core_color(window), "{appearance:?}");
            for ink in [theme.text, theme.hairline, theme.scroll_thumb] {
                assert_eq!(ink, core_color(window_text), "{appearance:?}");
            }
            assert_eq!(theme.dimmed, core_color(gray_text), "{appearance:?}");
            assert_eq!(theme.accent, core_color(highlight), "{appearance:?}");
            assert_eq!(theme.close_hover, core_color(highlight), "{appearance:?}");
            assert_eq!(
                theme.on_accent,
                core_color(highlight_text),
                "{appearance:?}"
            );
            assert_eq!(theme.frame, core_color(highlight), "{appearance:?}");
            assert!(
                theme.shadows(vec![shadow.clone()]).is_empty(),
                "{appearance:?}"
            );
            assert!(
                (theme.hover_opacity(0.5) - 1.0).abs() < f32::EPSILON,
                "{appearance:?}"
            );
        }
    }

    #[test]
    fn each_system_appearance_gets_its_material_and_hairline() {
        for appearance in [WindowAppearance::Dark, WindowAppearance::VibrantDark] {
            let dark = Theme::for_appearance(appearance, WindowSurface::Solid, None);
            assert_eq!(dark.material, rgba(0x1c1c_1eb8), "rgba(28,28,30,0.72)");
            assert_eq!(dark.hairline, rgba(0xffff_ff1f), "white 12%");
        }
        for appearance in [WindowAppearance::Light, WindowAppearance::VibrantLight] {
            let light = Theme::for_appearance(appearance, WindowSurface::Solid, None);
            assert_eq!(light.material, rgba(0xf6f6_f8b8), "rgba(246,246,248,0.72)");
            assert_eq!(light.hairline, rgba(0x0000_001a), "black 10%");
        }
    }

    #[test]
    fn the_editor_draws_one_accent_with_white_on_it_in_both_appearances() {
        for appearance in [WindowAppearance::Dark, WindowAppearance::Light] {
            let theme = Theme::for_appearance(appearance, WindowSurface::Solid, None);
            assert_eq!(theme.accent, rgba(0x0a84_ffff), "{appearance:?}");
            assert_eq!(theme.on_accent, rgba(0xffff_ffff), "{appearance:?}");
        }
    }

    #[test]
    fn glass_tints_the_blurred_desktop_and_the_solid_fallback_hides_it() {
        assert_eq!(
            WindowSurface::Glass.background(),
            WindowBackgroundAppearance::Blurred
        );
        assert_eq!(
            WindowSurface::Solid.background(),
            WindowBackgroundAppearance::Opaque
        );
        for appearance in [WindowAppearance::Dark, WindowAppearance::Light] {
            let glass = Theme::for_appearance(appearance, WindowSurface::Glass, None);
            let solid = Theme::for_appearance(appearance, WindowSurface::Solid, None);
            assert!(
                glass.backdrop.a < 0.6,
                "{appearance:?}: the desktop shows through"
            );
            assert_eq!(
                solid.backdrop.a, 1.0,
                "{appearance:?}: nothing shows through"
            );
            assert_eq!(glass.card.a, 1.0, "{appearance:?}: the card stays opaque");
        }
    }
}
