use windows::Win32::Foundation::{ERROR_SUCCESS, HWND};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, GetSysColor};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DestroyIcon, HICON, LR_DEFAULTCOLOR, SM_CXSMICON,
    SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::w;

const LIGHT_TASKBAR_ICON: &[u8] = include_bytes!("../../../assets/tray-light.ico");
const DARK_TASKBAR_ICON: &[u8] = include_bytes!("../../../assets/tray-dark.ico");
const ICON_FILE_HEADER_SIZE: usize = 6;
const ICON_FILE_ENTRY_SIZE: usize = 16;
const ICON_FORMAT_3: u32 = 0x0003_0000;

/// The colour behind the tray icons, which decides the glyph drawn on them (D-92).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskbarShade {
    /// A near-black glyph.
    Light,
    /// A white glyph.
    Dark,
}

impl TaskbarShade {
    /// D-92: in high contrast, the shade of the theme's window colour, which the taskbar is
    /// drawn in; otherwise Windows' `SystemUsesLightTheme`, where a value never set is Windows
    /// 11's dark taskbar.
    fn of(high_contrast_window: Option<u32>, system_uses_light_theme: Option<u32>) -> Self {
        let light = match high_contrast_window {
            Some(color) => {
                let [red, green, blue, _] = color.to_le_bytes().map(u32::from);
                red * 299 + green * 587 + blue * 114 >= 128 * 1000
            }
            None => system_uses_light_theme.is_some_and(|light| light != 0),
        };
        if light { Self::Light } else { Self::Dark }
    }

    /// The shade of the taskbar now.
    pub(crate) fn now() -> Self {
        Self::of(high_contrast_window(), system_uses_light_theme())
    }

    fn icon_file(self) -> &'static [u8] {
        match self {
            Self::Light => LIGHT_TASKBAR_ICON,
            Self::Dark => DARK_TASKBAR_ICON,
        }
    }
}

/// A tray icon this process made, destroyed when dropped.
pub(crate) struct TrayIcon(HICON);

impl TrayIcon {
    /// The glyph for `shade` at the small-icon size for `window`'s DPI, or the nearest larger
    /// one its .ico holds.
    pub(crate) fn new(window: HWND, shade: TaskbarShade) -> windows::core::Result<Self> {
        // SAFETY: both calls only read `window`'s DPI; a destroyed window gives the default.
        let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForWindow(window)) };
        Self::sized(shade, size)
    }

    fn sized(shade: TaskbarShade, size: i32) -> windows::core::Result<Self> {
        let image = nearest_image(shade.icon_file(), size);
        // SAFETY: `image` is one PNG image from an .ico built into the exe.
        let icon = unsafe {
            CreateIconFromResourceEx(image, true, ICON_FORMAT_3, size, size, LR_DEFAULTCOLOR)
        }?;
        Ok(Self(icon))
    }

    pub(crate) fn handle(&self) -> HICON {
        self.0
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        // SAFETY: the icon was made by CreateIconFromResourceEx and is destroyed once, here.
        let destroyed = unsafe { DestroyIcon(self.0) };
        debug_assert!(destroyed.is_ok(), "an icon this process made is destroyed");
    }
}

/// The image in the .ico `file` whose width is `size`, or else the smallest wider one, or
/// else the widest.
fn nearest_image(file: &'static [u8], size: i32) -> &'static [u8] {
    let count = usize::from(u16::from_le_bytes([file[4], file[5]]));
    let (entries, _) = file[ICON_FILE_HEADER_SIZE..].as_chunks::<ICON_FILE_ENTRY_SIZE>();
    let images = entries[..count].iter().map(|entry| {
        let width = if entry[0] == 0 {
            256
        } else {
            i32::from(entry[0])
        };
        let read = |at: usize| {
            let bytes = entry[at..at + 4]
                .try_into()
                .expect("an .ico entry is 16 bytes");
            usize::try_from(u32::from_le_bytes(bytes)).expect("an .ico fits in memory")
        };
        let (length, offset) = (read(8), read(12));
        (width, &file[offset..offset + length])
    });
    let wider = images
        .clone()
        .filter(|&(width, _)| width >= size)
        .min_by_key(|&(width, _)| width);
    wider
        .or_else(|| images.max_by_key(|&(width, _)| width))
        .expect("each tray .ico holds images")
        .1
}

/// The window colour of the high-contrast theme in force, as `0x00BBGGRR`; None outside high
/// contrast.
fn high_contrast_window() -> Option<u32> {
    let mut contrast = HIGHCONTRASTW {
        cbSize: crate::tray::struct_size::<HIGHCONTRASTW>(),
        ..Default::default()
    };
    // SAFETY: `contrast` is writable and its cbSize says how much.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            Some((&raw mut contrast).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    // A failed read shows the theme's glyph: only the high-contrast choice is lost.
    if read.is_err() || !contrast.dwFlags.contains(HCF_HIGHCONTRASTON) {
        return None;
    }
    // SAFETY: COLOR_WINDOW is a valid system colour index.
    Some(unsafe { GetSysColor(COLOR_WINDOW) })
}

/// Windows' `SystemUsesLightTheme` value, which colours the taskbar; None when it is not set.
fn system_uses_light_theme() -> Option<u32> {
    let mut light = 0u32;
    let mut bytes = crate::tray::struct_size::<u32>();
    // SAFETY: `light` is writable for `bytes` bytes and RRF_RT_REG_DWORD writes exactly 4.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut light).cast()),
            Some(&raw mut bytes),
        )
    };
    // A value missing or unreadable reads as Windows 11's default, the dark taskbar.
    (read == ERROR_SUCCESS).then_some(light)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: u32 = 0x0000_0000;
    const WHITE: u32 = 0x00FF_FFFF;

    #[test]
    fn d92_the_tray_glyph_follows_the_taskbar_theme_and_high_contrast_wins() {
        assert_eq!(TaskbarShade::of(None, Some(1)), TaskbarShade::Light);
        assert_eq!(TaskbarShade::of(None, Some(0)), TaskbarShade::Dark);
        assert_eq!(
            TaskbarShade::of(None, None),
            TaskbarShade::Dark,
            "a value never set is Windows 11's dark taskbar"
        );
        assert_eq!(TaskbarShade::of(Some(BLACK), Some(1)), TaskbarShade::Dark);
        assert_eq!(TaskbarShade::of(Some(WHITE), Some(0)), TaskbarShade::Light);
    }

    #[test]
    fn d92_each_tray_glyph_loads_at_every_scale_from_100_to_300_percent() {
        for shade in [TaskbarShade::Light, TaskbarShade::Dark] {
            for size in [16, 20, 24, 32, 40, 48] {
                if let Err(error) = TrayIcon::sized(shade, size) {
                    panic!("the {shade:?} glyph at {size} px did not load: {error}");
                }
            }
        }
    }
}
