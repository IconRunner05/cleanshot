use std::time::Duration;

use gpui::{Rgba, WindowAppearance, rgba};

/// The editor's colours for one system appearance (CHARTER 4, F-58, D-47), and how long the pointer
/// rests on a control before its tooltip shows (D-117).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Theme {
    /// Behind the image, where the canvas is not covered.
    pub backdrop: Rgba,
    /// Floating panels: the toolbar and the notice.
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

impl Theme {
    pub fn for_appearance(appearance: WindowAppearance, tooltip_delay: Option<Duration>) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self {
                backdrop: rgba(0x2c2c_2eff),
                material: rgba(0x1c1c_1eb8),
                hairline: rgba(0xffff_ff1f),
                text: rgba(0xf5f5_f7ff),
                hover: rgba(0xffff_ff1a),
                tooltip_delay,
            },
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self {
                backdrop: rgba(0xe5e5_eaff),
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
            let dark = Theme::for_appearance(appearance, None);
            assert_eq!(dark.material, rgba(0x1c1c_1eb8), "rgba(28,28,30,0.72)");
            assert_eq!(dark.hairline, rgba(0xffff_ff1f), "white 12%");
        }
        for appearance in [WindowAppearance::Light, WindowAppearance::VibrantLight] {
            let light = Theme::for_appearance(appearance, None);
            assert_eq!(light.material, rgba(0xf6f6_f8b8), "rgba(246,246,248,0.72)");
            assert_eq!(light.hairline, rgba(0x0000_001a), "black 10%");
        }
    }
}
