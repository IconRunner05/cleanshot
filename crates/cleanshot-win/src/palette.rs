use cleanshot_core::{Color, ContrastColors, chrome};
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Gdi::{
    COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT,
    GetSysColor,
};
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::w;

use crate::registry::read_number;
use crate::struct_size;

/// The colours the overlay, its toolbar and its size pill are drawn in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Palette {
    pub(crate) dim: D2D1_COLOR_F,
    pub(crate) accent: D2D1_COLOR_F,
    pub(crate) on_accent: D2D1_COLOR_F,
    pub(crate) handle: D2D1_COLOR_F,
    pub(crate) material: D2D1_COLOR_F,
    pub(crate) hairline: D2D1_COLOR_F,
    pub(crate) label: D2D1_COLOR_F,
    /// A control's label while the control is off.
    pub(crate) label_off: D2D1_COLOR_F,
}

/// The system colours a high-contrast theme sets that the overlay uses, as `GetSysColor`
/// gives them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SystemColours {
    pub(crate) window: COLORREF,
    pub(crate) window_text: COLORREF,
    pub(crate) highlight: COLORREF,
    pub(crate) highlight_text: COLORREF,
    pub(crate) gray_text: COLORREF,
}

const DIM: D2D1_COLOR_F = colour(0, 0, 0, 0.4);

impl Palette {
    /// The default colours, or with a high-contrast theme on, that theme's.
    pub(crate) fn current() -> windows::core::Result<Self> {
        Ok(Self::of(system_colours()?))
    }

    /// The default colours with `system` as `None`; otherwise the high-contrast theme's, every
    /// one opaque so no frozen pixel shows through the toolbar or the pill.
    pub(crate) fn of(system: Option<SystemColours>) -> Self {
        match system {
            None => Self {
                dim: DIM,
                accent: direct2d(chrome::ACCENT),
                on_accent: direct2d(chrome::ON_ACCENT),
                handle: colour(255, 255, 255, 1.0),
                material: direct2d(chrome::DARK.material),
                hairline: direct2d(chrome::DARK.hairline),
                label: direct2d(chrome::DARK.text),
                label_off: D2D1_COLOR_F {
                    a: 0.35,
                    ..direct2d(chrome::DARK.text)
                },
            },
            Some(system) => Self {
                dim: DIM,
                accent: opaque(system.highlight),
                on_accent: opaque(system.highlight_text),
                handle: opaque(system.window),
                material: opaque(system.window),
                hairline: opaque(system.window_text),
                label: opaque(system.window_text),
                label_off: opaque(system.gray_text),
            },
        }
    }
}

const fn colour(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}

const fn direct2d(color: Color) -> D2D1_COLOR_F {
    colour(color.r, color.g, color.b, color.a as f32 / 255.0)
}

const fn opaque(system: COLORREF) -> D2D1_COLOR_F {
    direct2d(core_color(system))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "each COLORREF channel is one byte"
)]
const fn core_color(system: COLORREF) -> Color {
    Color {
        r: system.0 as u8,
        g: (system.0 >> 8) as u8,
        b: (system.0 >> 16) as u8,
        a: 255,
    }
}

/// The colours of the high-contrast theme that is on, or `None` with none on.
///
/// # Errors
///
/// The Windows error if the setting cannot be read.
pub fn high_contrast_colors() -> windows::core::Result<Option<ContrastColors>> {
    Ok(system_colours()?.map(contrast_colors))
}

const fn contrast_colors(system: SystemColours) -> ContrastColors {
    ContrastColors {
        window: core_color(system.window),
        window_text: core_color(system.window_text),
        highlight: core_color(system.highlight),
        highlight_text: core_color(system.highlight_text),
        gray_text: core_color(system.gray_text),
    }
}

pub(crate) fn system_colours() -> windows::core::Result<Option<SystemColours>> {
    if !high_contrast_on()? {
        return Ok(None);
    }
    // SAFETY: reads of the current theme's colours.
    Ok(Some(unsafe {
        SystemColours {
            window: COLORREF(GetSysColor(COLOR_WINDOW)),
            window_text: COLORREF(GetSysColor(COLOR_WINDOWTEXT)),
            highlight: COLORREF(GetSysColor(COLOR_HIGHLIGHT)),
            highlight_text: COLORREF(GetSysColor(COLOR_HIGHLIGHTTEXT)),
            gray_text: COLORREF(GetSysColor(COLOR_GRAYTEXT)),
        }
    }))
}

fn high_contrast_on() -> windows::core::Result<bool> {
    let mut contrast = HIGHCONTRASTW {
        cbSize: struct_size::<HIGHCONTRASTW>(),
        ..HIGHCONTRASTW::default()
    };
    // SAFETY: `contrast` is writable and its size is given in cbSize.
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            Some((&raw mut contrast).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }?;
    Ok(contrast.dwFlags.contains(HCF_HIGHCONTRASTON))
}

/// Whether windows may look translucent, as Windows' transparency effects setting and
/// high contrast say.
///
/// # Errors
///
/// The Windows error if either setting cannot be read.
pub fn glass_allowed() -> windows::core::Result<bool> {
    let transparency = read_number(
        HKEY_CURRENT_USER,
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
        w!("EnableTransparency"),
    )
    .map_err(windows::core::Error::from)?;
    Ok(glass(transparency, high_contrast_on()?))
}

/// Glass only with transparency effects on, which a missing value means, and high contrast off.
fn glass(transparency: Option<u32>, high_contrast: bool) -> bool {
    transparency != Some(0) && !high_contrast
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_are_glass_only_with_transparency_on_and_high_contrast_off() {
        assert!(
            glass(None, false),
            "a missing value is Windows' default, on"
        );
        assert!(glass(Some(1), false));
        assert!(!glass(Some(0), false));
        assert!(!glass(None, true));
        assert!(!glass(Some(1), true));
    }

    #[test]
    fn the_editor_gets_each_theme_colour_opaque_from_its_own_system_colour() {
        let color = |r, g, b| Color { r, g, b, a: 255 };
        let colors = contrast_colors(SystemColours {
            window: COLORREF(0x0003_0201),
            window_text: COLORREF(0x0013_1211),
            highlight: COLORREF(0x0023_2221),
            highlight_text: COLORREF(0x0033_3231),
            gray_text: COLORREF(0x0043_4241),
        });
        assert_eq!(colors.window, color(0x01, 0x02, 0x03));
        assert_eq!(colors.window_text, color(0x11, 0x12, 0x13));
        assert_eq!(colors.highlight, color(0x21, 0x22, 0x23));
        assert_eq!(colors.highlight_text, color(0x31, 0x32, 0x33));
        assert_eq!(colors.gray_text, color(0x41, 0x42, 0x43));
    }

    #[test]
    fn the_overlay_draws_the_accent_and_panel_colours_core_holds() {
        let palette = Palette::of(None);
        assert_eq!(palette.accent, direct2d(chrome::ACCENT));
        assert_eq!(palette.on_accent, direct2d(chrome::ON_ACCENT));
        assert_eq!(palette.material, direct2d(chrome::DARK.material));
        assert_eq!(palette.hairline, direct2d(chrome::DARK.hairline));
        assert_eq!(palette.label, direct2d(chrome::DARK.text));
    }

    #[test]
    fn high_contrast_draws_the_overlay_in_opaque_system_colours() {
        let palette = Palette::of(Some(SystemColours {
            window: COLORREF(0x0000_0000),
            window_text: COLORREF(0x00FF_FFFF),
            highlight: COLORREF(0x0000_FFFF),
            highlight_text: COLORREF(0x0000_0000),
            gray_text: COLORREF(0x0000_FF00),
        }));
        let yellow = colour(255, 255, 0, 1.0);
        let black = colour(0, 0, 0, 1.0);
        let white = colour(255, 255, 255, 1.0);
        assert_eq!(palette.material, black);
        assert_eq!(palette.label, white);
        assert_eq!(palette.hairline, white);
        assert_eq!(palette.accent, yellow);
        assert_eq!(palette.on_accent, black);
        assert_eq!(palette.handle, black);
        assert_eq!(palette.label_off, colour(0, 255, 0, 1.0));
    }
}
