use std::rc::Rc;
use std::time::Duration;

use gpui::{App, Global, Rgba, WindowAppearance, WindowBackgroundAppearance, rgba};

/// The editor's colours for one system appearance (CHARTER 4, F-58, D-47), and how long the pointer
/// rests on a control before its tooltip shows (D-117).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Theme {
    /// The window round the image's card: a tint over the blurred desktop on glass.
    pub backdrop: Rgba,
    /// The image's card, where the image does not cover it (D-141).
    pub card: Rgba,
    /// Floating panels: menus and the notice.
    pub material: Rgba,
    pub hairline: Rgba,
    pub text: Rgba,
    /// Under a toolbar button the pointer is over.
    pub hover: Rgba,
    /// `None` leaves the delay to GPUI.
    pub tooltip_delay: Option<Duration>,
}

/// The one accent, the same in both appearances: the current tool, selection handles, the crop.
pub(crate) const ACCENT: Rgba = Rgba {
    r: 0.0,
    g: 0.478,
    b: 1.0,
    a: 1.0,
};

/// Text and icons on the accent.
pub(crate) const ON_ACCENT: Rgba = Rgba {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};

/// What a window shows behind its controls and the image's card (D-142).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowSurface {
    /// The desktop behind the window, blurred and tinted.
    Glass,
    /// One opaque colour, for when Windows' transparency effects are off or high contrast is on.
    Solid,
}

impl WindowSurface {
    pub(crate) fn background(self) -> WindowBackgroundAppearance {
        match self {
            Self::Glass => WindowBackgroundAppearance::Blurred,
            Self::Solid => WindowBackgroundAppearance::Opaque,
        }
    }
}

struct SurfaceReader(Rc<dyn Fn() -> WindowSurface>);

impl Global for SurfaceReader {}

/// Makes each window ask `read` which surface to open with (D-142). Until this is called,
/// windows open solid.
pub fn follow_window_surface(cx: &mut App, read: impl Fn() -> WindowSurface + 'static) {
    cx.set_global(SurfaceReader(Rc::new(read)));
}

/// The surface a window opening now gets (D-142).
pub(crate) fn window_surface(cx: &App) -> WindowSurface {
    cx.try_global::<SurfaceReader>()
        .map_or(WindowSurface::Solid, |SurfaceReader(read)| read())
}

impl Theme {
    pub fn for_appearance(
        appearance: WindowAppearance,
        surface: WindowSurface,
        tooltip_delay: Option<Duration>,
    ) -> Self {
        let glass = surface == WindowSurface::Glass;
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self {
                backdrop: if glass {
                    rgba(0x1e20_286b)
                } else {
                    rgba(0x2c2c_2eb8)
                },
                card: rgba(0x1c1c_1eff),
                material: rgba(0x1c1c_1eb8),
                hairline: rgba(0xffff_ff1f),
                text: rgba(0xf5f5_f7ff),
                hover: rgba(0xffff_ff1a),
                tooltip_delay,
            },
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self {
                backdrop: if glass {
                    rgba(0xfafb_fd8c)
                } else {
                    rgba(0xe5e5_eaff)
                },
                card: rgba(0xf6f6_f8ff),
                material: rgba(0xf6f6_f8b8),
                hairline: rgba(0x0000_001a),
                text: rgba(0x1d1d_1fff),
                hover: rgba(0x0000_0014),
                tooltip_delay,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f58_each_system_appearance_gets_its_charter_material_and_hairline() {
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
    fn d142_glass_tints_the_blurred_desktop_and_the_solid_fallback_hides_it() {
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
