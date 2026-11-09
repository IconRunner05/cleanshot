use toml::{Table, Value};

use crate::looks::Looks;
use crate::thumbnail_layout::ThumbnailStay;

mod hotkeys;
mod looks;

pub use hotkeys::{Hotkey, HotkeyAction, HotkeyRefusal, Hotkeys, Key, Modifiers};

/// The largest settings file read, in bytes.
pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;

/// The longest `save_folder` read, in UTF-8 bytes: written back with every other setting at its
/// longest and every byte escaped (`\u001F`, 6 bytes), the file stays within
/// [`MAX_SETTINGS_BYTES`].
const MAX_SAVE_FOLDER_BYTES: usize = (MAX_SETTINGS_BYTES - 2048) / 6;

// A folder within the limit is also within Windows' 32,767-unit path limit.
const _: () = assert!(MAX_SAVE_FOLDER_BYTES <= 32_767);

/// Everything `cleanshot.toml` holds. Each field has the default [`Settings::default`]
/// gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// `None` means the default folder, Pictures\Screenshots, which the caller resolves.
    pub save_folder: Option<String>,
    pub save_format: SaveFormat,
    pub after_capture: AfterCapture,
    pub show_thumbnail: bool,
    pub thumbnail_stay: ThumbnailStay,
    pub show_pointer: bool,
    pub remember_last_selection: bool,
    pub hotkeys: Hotkeys,
    /// How each tool draws, as the property strip last set it.
    pub looks: Looks,
}

/// The image format Save writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveFormat {
    Png,
    Jpg,
    Bmp,
}

impl SaveFormat {
    /// The format's name, as the Options menu and the settings window show it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpg => "JPG",
            Self::Bmp => "BMP",
        }
    }

    /// The format a file extension names, in any case: png, jpg or jpeg, bmp.
    #[must_use]
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpg),
            "bmp" => Some(Self::Bmp),
            _ => None,
        }
    }
}

/// What happens once a region is selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterCapture {
    ShowToolbar,
    Copy,
}

/// A part of the settings file that was not used, so its default applies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsProblem {
    /// Larger than [`MAX_SETTINGS_BYTES`]; nothing was read.
    TooLarge { bytes: usize },
    /// Not UTF-8 or not TOML; nothing was read.
    Unreadable,
    /// This key held a value of the wrong type or outside its choices.
    InvalidValue { key: String },
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            save_folder: None,
            save_format: SaveFormat::Png,
            after_capture: AfterCapture::ShowToolbar,
            show_thumbnail: true,
            thumbnail_stay: ThumbnailStay::default(),
            show_pointer: false,
            remember_last_selection: false,
            hotkeys: Hotkeys::default(),
            looks: Looks::default(),
        }
    }
}

impl Settings {
    /// `self`, if a settings file holding it reads back with no problem; else the problems that
    /// file's reader would report, so nothing is kept that the next start would refuse.
    ///
    /// # Errors
    ///
    /// Every [`SettingsProblem`] [`Settings::from_toml`] finds in [`Settings::to_toml`]'s text.
    pub fn checked(self) -> Result<Self, Vec<SettingsProblem>> {
        let (_, problems) = Self::from_toml(self.to_toml().as_bytes());
        if problems.is_empty() {
            Ok(self)
        } else {
            Err(problems)
        }
    }

    /// The settings in `bytes`, with every unusable part at its default and reported. Unknown
    /// keys are ignored; input over [`MAX_SETTINGS_BYTES`] is not parsed.
    #[must_use]
    pub fn from_toml(bytes: &[u8]) -> (Self, Vec<SettingsProblem>) {
        if bytes.len() > MAX_SETTINGS_BYTES {
            let problem = SettingsProblem::TooLarge { bytes: bytes.len() };
            return (Self::default(), vec![problem]);
        }
        let Some(table) = std::str::from_utf8(bytes)
            .ok()
            .and_then(|text| text.parse::<Table>().ok())
        else {
            return (Self::default(), vec![SettingsProblem::Unreadable]);
        };
        let mut reader = Reader {
            table: &table,
            prefix: String::new(),
            problems: Vec::new(),
        };
        let defaults = Self::default();
        let save_folder = reader.read("save_folder", |value| {
            value
                .as_str()
                .filter(|folder| {
                    !folder.is_empty()
                        && folder.len() <= MAX_SAVE_FOLDER_BYTES
                        // No folder has a C0 control character in its name.
                        && !folder.bytes().any(|byte| byte < b' ')
                        // Saving to a share or a device would reach the network or a driver.
                        && matches!(folder_kind(folder), FolderKind::Drive | FolderKind::Other)
                })
                .map(|folder| Some(folder.to_owned()))
        });
        let settings = Self {
            save_folder: save_folder.unwrap_or(defaults.save_folder),
            save_format: reader
                .read("save_format", |value| match value.as_str()? {
                    "png" => Some(SaveFormat::Png),
                    "jpg" => Some(SaveFormat::Jpg),
                    "bmp" => Some(SaveFormat::Bmp),
                    _ => None,
                })
                .unwrap_or(defaults.save_format),
            after_capture: reader
                .read("after_capture", |value| match value.as_str()? {
                    "toolbar" => Some(AfterCapture::ShowToolbar),
                    "copy" => Some(AfterCapture::Copy),
                    _ => None,
                })
                .unwrap_or(defaults.after_capture),
            show_thumbnail: reader
                .read("show_thumbnail", Value::as_bool)
                .unwrap_or(defaults.show_thumbnail),
            thumbnail_stay: reader
                .read("thumbnail_ms", |value| {
                    ThumbnailStay::from_ms(u32::try_from(value.as_integer()?).ok()?)
                })
                .unwrap_or(defaults.thumbnail_stay),
            show_pointer: reader
                .read("show_pointer", Value::as_bool)
                .unwrap_or(defaults.show_pointer),
            remember_last_selection: reader
                .read("remember_last_selection", Value::as_bool)
                .unwrap_or(defaults.remember_last_selection),
            hotkeys: reader.hotkeys(defaults.hotkeys),
            looks: reader.looks(),
        };
        (settings, reader.problems)
    }

    /// These settings as `cleanshot.toml` text, which [`Settings::from_toml`] reads back
    /// unchanged.
    #[must_use]
    pub fn to_toml(&self) -> String {
        let mut table = Table::new();
        if let Some(folder) = &self.save_folder {
            table.insert("save_folder".into(), folder.clone().into());
        }
        let save_format = match self.save_format {
            SaveFormat::Png => "png",
            SaveFormat::Jpg => "jpg",
            SaveFormat::Bmp => "bmp",
        };
        let after_capture = match self.after_capture {
            AfterCapture::ShowToolbar => "toolbar",
            AfterCapture::Copy => "copy",
        };
        table.insert("save_format".into(), save_format.into());
        table.insert("after_capture".into(), after_capture.into());
        table.insert("show_thumbnail".into(), self.show_thumbnail.into());
        table.insert(
            "thumbnail_ms".into(),
            i64::from(self.thumbnail_stay.ms()).into(),
        );
        table.insert("show_pointer".into(), self.show_pointer.into());
        table.insert(
            "remember_last_selection".into(),
            self.remember_last_selection.into(),
        );
        table.insert("hotkeys".into(), hotkeys::table(&self.hotkeys).into());
        let looks = looks::table(&self.looks);
        if !looks.is_empty() {
            table.insert("looks".into(), looks.into());
        }
        table.to_string()
    }
}

/// What a folder path names, read as Windows reads it, either slash being a separator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FolderKind {
    /// `C:\...`
    Drive,
    /// `\\server\share...`
    Share,
    /// Any other path with two leading separators, such as `\\server` with no share name.
    Server,
    /// `\\?\...`, `\\.\...` or `\??\...`
    Device,
    /// A relative path, or one from the current drive's root (`\Shots`, `C:Shots`).
    Other,
}

pub(crate) fn folder_kind(folder: &str) -> FolderKind {
    let separator = |byte: &u8| *byte == b'\\' || *byte == b'/';
    match folder.as_bytes() {
        [drive, b':', slash, ..] if drive.is_ascii_alphabetic() && separator(slash) => {
            FolderKind::Drive
        }
        [b'\\', b'?', b'?', b'\\', ..] => FolderKind::Device,
        [first, second, rest @ ..] if separator(first) && separator(second) => match rest {
            [b'?' | b'.', ..] => FolderKind::Device,
            _ => match rest.iter().position(separator) {
                Some(end) if end > 0 && rest.get(end + 1).is_some_and(|b| !separator(b)) => {
                    FolderKind::Share
                }
                Some(_) | None => FolderKind::Server,
            },
        },
        _ => FolderKind::Other,
    }
}

/// Reads typed fields from one table, noting each key present with an unusable value.
struct Reader<'a> {
    table: &'a Table,
    prefix: String,
    problems: Vec<SettingsProblem>,
}

impl Reader<'_> {
    /// The decoded value of `key`; `None` if it is absent or could not be decoded, the
    /// latter noted as a problem.
    fn read<T>(&mut self, key: &'static str, decode: impl Fn(&Value) -> Option<T>) -> Option<T> {
        let value = self.table.get(key)?;
        let decoded = decode(value);
        if decoded.is_none() {
            self.problems.push(SettingsProblem::InvalidValue {
                key: if self.prefix.is_empty() {
                    key.to_owned()
                } else {
                    format!("{}.{key}", self.prefix)
                },
            });
        }
        decoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;
    use hotkeys::tests::hotkey;

    #[test]
    fn a_save_folder_holding_a_control_character_falls_back_to_the_default() {
        for folder in ["C:\\\\a\\u0000b", "C:\\\\a\\u001Fb"] {
            let toml = format!("save_folder = \"{folder}\"\n");
            let (settings, problems) = Settings::from_toml(toml.as_bytes());
            assert_eq!(settings.save_folder, None, "{folder}");
            assert_eq!(
                problems,
                [SettingsProblem::InvalidValue {
                    key: "save_folder".into()
                }],
                "{folder}"
            );
        }
    }

    #[test]
    fn a_typed_extension_names_its_format_in_any_case() {
        for (extension, format) in [
            ("png", Some(SaveFormat::Png)),
            ("PNG", Some(SaveFormat::Png)),
            ("jpg", Some(SaveFormat::Jpg)),
            ("JPEG", Some(SaveFormat::Jpg)),
            ("Bmp", Some(SaveFormat::Bmp)),
            ("gif", None),
            ("", None),
        ] {
            assert_eq!(SaveFormat::from_extension(extension), format, "{extension}");
        }
    }

    #[test]
    fn the_capture_panel_action_is_the_capture_panel_hotkey_ctrl_shift_5_by_default_and_no_other_may_take_it()
     {
        let mut hotkeys = Settings::default().hotkeys;
        assert_eq!(
            hotkeys.get(HotkeyAction::CapturePanel),
            hotkey("Ctrl+Shift+5")
        );
        hotkeys.set(HotkeyAction::CapturePanel, hotkey("Win+5"));
        assert_eq!(hotkeys.capture_panel, hotkey("Win+5"));
        assert_eq!(
            hotkeys.used_by_another(HotkeyAction::Region, hotkey("Win+5")),
            Some(HotkeyAction::CapturePanel)
        );
    }

    #[test]
    fn an_empty_file_gives_every_default() {
        let (settings, problems) = Settings::from_toml(b"");
        assert_eq!(problems, []);
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.save_format, SaveFormat::Png);
        assert_eq!(settings.after_capture, AfterCapture::ShowToolbar);
        assert!(settings.show_thumbnail);
        assert_eq!(settings.hotkeys.region, hotkey("PrtSc"));
        assert_eq!(settings.hotkeys.last_region, hotkey("Shift+PrtSc"));
        assert_eq!(settings.hotkeys.full_screen, hotkey("Ctrl+PrtSc"));
        assert_eq!(settings.hotkeys.window, hotkey("Alt+PrtSc"));
        assert_eq!(settings.hotkeys.capture_panel, hotkey("Ctrl+Shift+5"));
    }

    #[test]
    fn unknown_keys_are_ignored_without_a_problem() {
        let text = "colour = \"blue\"\nshow_pointer = true\n[hotkeys]\nsparkle = \"F9\"\n";
        let (settings, problems) = Settings::from_toml(text.as_bytes());
        assert_eq!(problems, []);
        assert!(settings.show_pointer);
    }

    #[test]
    fn a_file_with_start_with_windows_reads_without_a_problem_and_is_written_back_without_it() {
        let (settings, problems) =
            Settings::from_toml(b"start_with_windows = true\nshow_pointer = true\n");
        assert_eq!(problems, []);
        assert!(settings.show_pointer);
        assert!(!settings.to_toml().contains("start_with_windows"));
    }

    #[test]
    fn an_invalid_value_falls_back_to_its_default_and_is_reported() {
        let text = "save_format = \"gif\"\nshow_pointer = \"yes\"\nstart_with_windows = true\n\
                    [hotkeys]\nregion = \"Ctrl+Ctrl+4\"\nwindow = \"Win+W\"\n";
        let (settings, problems) = Settings::from_toml(text.as_bytes());

        assert_eq!(settings.save_format, SaveFormat::Png);
        assert!(!settings.show_pointer);
        assert_eq!(settings.hotkeys.region, hotkey("PrtSc"));
        assert_eq!(settings.hotkeys.window, hotkey("Win+W"));
        let keys: Vec<_> = problems
            .iter()
            .map(|problem| match problem {
                SettingsProblem::InvalidValue { key } => key.as_str(),
                SettingsProblem::TooLarge { .. } | SettingsProblem::Unreadable => "",
            })
            .collect();
        assert_eq!(keys, ["save_format", "show_pointer", "hotkeys.region"]);
    }

    #[test]
    fn a_hotkeys_entry_that_is_not_a_table_is_reported_and_defaulted() {
        let (settings, problems) = Settings::from_toml(b"hotkeys = \"PrtSc\"\n");
        assert_eq!(settings.hotkeys, Hotkeys::default());
        assert_eq!(
            problems,
            [SettingsProblem::InvalidValue {
                key: "hotkeys".into()
            }]
        );
    }

    #[test]
    fn a_file_over_64_kb_is_not_read() {
        let mut text = String::from("show_pointer = true\n");
        let at_limit = format!("{text}{}", "#".repeat(MAX_SETTINGS_BYTES - text.len()));
        assert!(Settings::from_toml(at_limit.as_bytes()).0.show_pointer);

        text.push_str(&"#".repeat(MAX_SETTINGS_BYTES));
        let (settings, problems) = Settings::from_toml(text.as_bytes());
        assert_eq!(settings, Settings::default());
        assert_eq!(
            problems,
            [SettingsProblem::TooLarge {
                bytes: MAX_SETTINGS_BYTES + 20
            }]
        );
    }

    #[test]
    fn text_that_is_not_toml_gives_every_default() {
        for bytes in [&b"show_pointer = = true"[..], &[0xFF, 0xFE, 0x00]] {
            let (settings, problems) = Settings::from_toml(bytes);
            assert_eq!(settings, Settings::default());
            assert_eq!(problems, [SettingsProblem::Unreadable]);
        }
    }

    #[test]
    fn thumbnail_ms_from_1000_to_60000_is_kept_and_any_other_falls_back_to_5000() {
        let stay = |text: &str| {
            let (settings, problems) = Settings::from_toml(text.as_bytes());
            (settings.thumbnail_stay.ms(), problems.len())
        };
        assert_eq!(stay(""), (5000, 0));
        assert_eq!(stay("thumbnail_ms = 1000"), (1000, 0));
        assert_eq!(stay("thumbnail_ms = 60000"), (60_000, 0));
        for invalid in [
            "thumbnail_ms = 999",
            "thumbnail_ms = 60001",
            "thumbnail_ms = -1",
            "thumbnail_ms = 5000.0",
            "thumbnail_ms = \"5000\"",
        ] {
            assert_eq!(stay(invalid), (5000, 1), "{invalid}");
        }
    }

    #[test]
    fn settings_written_out_read_back_unchanged() {
        let settings = Settings {
            save_folder: Some(r"D:\Shots".into()),
            save_format: SaveFormat::Bmp,
            after_capture: AfterCapture::Copy,
            show_thumbnail: false,
            thumbnail_stay: ThumbnailStay::from_ms(12_345).expect("within the range"),
            show_pointer: true,
            remember_last_selection: true,
            hotkeys: Hotkeys {
                region: hotkey("Ctrl+Shift+4"),
                ..Hotkeys::default()
            },
            looks: Looks::default(),
        };
        let (read_back, problems) = Settings::from_toml(settings.to_toml().as_bytes());
        assert_eq!(problems, []);
        assert_eq!(read_back, settings);
    }

    #[test]
    fn a_setting_the_next_start_would_refuse_is_refused_now_with_its_problem() {
        let folder = |folder: String| Settings {
            save_folder: Some(folder),
            ..Settings::default()
        };
        let longest = folder("x".repeat(MAX_SAVE_FOLDER_BYTES));
        assert_eq!(longest.clone().checked(), Ok(longest));
        let invalid = vec![SettingsProblem::InvalidValue {
            key: "save_folder".into(),
        }];
        assert_eq!(
            folder("x".repeat(MAX_SAVE_FOLDER_BYTES + 1)).checked(),
            Err(invalid.clone())
        );
        assert_eq!(folder(String::new()).checked(), Err(invalid));
    }

    #[test]
    fn a_save_folder_on_a_share_or_device_is_refused_and_a_drive_folder_is_kept() {
        let read = |folder: &str| {
            let text = format!("save_folder = '{folder}'");
            Settings::from_toml(text.as_bytes())
        };
        let invalid = vec![SettingsProblem::InvalidValue {
            key: "save_folder".into(),
        }];
        for folder in [
            "\\\\server\\share",
            "\\\\?\\UNC\\server\\share",
            "\\\\.\\pipe\\shots",
            "\\\\?\\C:\\Shots",
            "//server/share",
            "\\/server/share",
            "/\\server\\share",
            "\\??\\UNC\\server\\share",
            "\\\\fileserver",
            "//fileserver/",
        ] {
            assert_eq!(
                read(folder),
                (Settings::default(), invalid.clone()),
                "{folder}"
            );
        }
        for folder in ["C:\\Users\\me\\Shots", "z:/shots"] {
            let (settings, problems) = read(folder);
            assert_eq!(
                (settings.save_folder.as_deref(), problems),
                (Some(folder), vec![])
            );
        }
        let chosen_in_the_window = Settings {
            save_folder: Some("\\\\server\\share".into()),
            ..Settings::default()
        };
        assert_eq!(chosen_in_the_window.checked(), Err(invalid));
    }

    #[test]
    fn the_largest_settings_read_write_back_within_the_size_limit() {
        let longest = hotkey("Ctrl+Alt+Shift+Win+F24");
        let largest = Settings {
            save_folder: Some("\u{7F}".repeat(MAX_SAVE_FOLDER_BYTES)),
            hotkeys: Hotkeys {
                region: longest,
                last_region: longest,
                full_screen: longest,
                window: longest,
                capture_panel: longest,
            },
            looks: looks::tests::largest(),
            ..Settings::default()
        };
        let written = largest.to_toml();
        assert!(
            written.len() <= MAX_SETTINGS_BYTES,
            "{} bytes",
            written.len()
        );
        assert_eq!(
            Settings::from_toml(written.as_bytes()),
            (largest, Vec::new())
        );

        let too_long = format!(
            "save_folder = '{}'\n",
            "x".repeat(MAX_SAVE_FOLDER_BYTES + 1)
        );
        let (settings, problems) = Settings::from_toml(too_long.as_bytes());
        assert_eq!(settings.save_folder, None);
        let invalid = SettingsProblem::InvalidValue {
            key: "save_folder".into(),
        };
        assert_eq!(problems, [invalid]);
    }

    const EVERY_SETTING: &str = "save_folder = 'D:\\Shots\\Bildschirmfotos \u{1F4F7}'\n\
        save_format = \"jpg\"\nafter_capture = \"copy\"\nshow_thumbnail = false\n\
        thumbnail_ms = 8000\nshow_pointer = true\nremember_last_selection = true\n\
        [hotkeys]\nregion = \"Ctrl+Shift+4\"\nlast_region = \"Shift+PrtSc\"\n\
        full_screen = \"Ctrl+PrtSc\"\nwindow = \"Alt+F24\"\ncapture_panel = \"Win+5\"\n\
        [looks]\ncolor = \"#007AFF\"\n[looks.rectangle]\nstroke_width = 8\nfill = \"filled\"\n\
        border = true\nborder_width = 6\n[looks.text]\ntext_size = 36.5\nalignment = \"center\"\n\
        [looks.bubble]\nalignment = \"bottom-right\"\n[looks.redact]\nstrength = 64\n";

    const KEYS: [&str; 13] = [
        "save_folder",
        "save_format",
        "after_capture",
        "show_thumbnail",
        "thumbnail_ms",
        "show_pointer",
        "remember_last_selection",
        "hotkeys",
        "hotkeys.region",
        "hotkeys.last_region",
        "hotkeys.full_screen",
        "hotkeys.window",
        "hotkeys.capture_panel",
    ];

    /// A fixed-seed xorshift, so a failing input is the same on every run.
    struct Dice(u64);

    impl Dice {
        fn below(&mut self, bound: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            usize::try_from(self.0 % bound as u64).expect("the remainder is below a usize")
        }

        fn byte(&mut self) -> u8 {
            const TOML_BYTES: &[u8] = b"[]{}=\"'.,#+-_ \n\t\\0123456789aefilnrstuxyzCFPS";
            if self.below(2) == 0 {
                TOML_BYTES[self.below(TOML_BYTES.len())]
            } else {
                u8::try_from(self.below(256)).expect("below 256 fits a byte")
            }
        }
    }

    /// Whatever the bytes, `from_toml` returns, and what it returns reads back the
    /// same: oversize and unreadable input give the defaults and that one problem, any other
    /// problem names a key Cleanshot reads, once.
    fn assert_read_safely(bytes: &[u8]) {
        let shown = String::from_utf8_lossy(&bytes[..bytes.len().min(120)]).into_owned();
        let Ok((settings, problems)) = std::panic::catch_unwind(|| Settings::from_toml(bytes))
        else {
            panic!(
                "from_toml panicked on {} bytes starting {shown:?}",
                bytes.len()
            );
        };
        let context = format!("{} bytes starting {shown:?}: {problems:?}", bytes.len());
        if bytes.len() > MAX_SETTINGS_BYTES || problems.contains(&SettingsProblem::Unreadable) {
            assert_eq!(settings, Settings::default(), "{context}");
            assert_eq!(problems.len(), 1, "{context}");
        }
        if bytes.len() > MAX_SETTINGS_BYTES {
            let too_large = SettingsProblem::TooLarge { bytes: bytes.len() };
            assert_eq!(problems, [too_large], "{context}");
        }
        let mut seen = Vec::new();
        for problem in &problems {
            match problem {
                SettingsProblem::TooLarge { .. } | SettingsProblem::Unreadable => {}
                SettingsProblem::InvalidValue { key } => {
                    assert!(
                        KEYS.contains(&key.as_str()) || looks::tests::keys().contains(key),
                        "{key} in {context}"
                    );
                    assert!(!seen.contains(key), "{key} twice in {context}");
                    seen.push(key.clone());
                }
            }
        }
        let read_back = Settings::from_toml(settings.to_toml().as_bytes());
        assert_eq!(read_back, (settings, Vec::new()), "{context}");
    }

    #[test]
    fn the_corruption_fixture_sets_every_setting_without_a_problem() {
        let (settings, problems) = Settings::from_toml(EVERY_SETTING.as_bytes());
        assert_eq!(problems, []);
        assert_eq!(settings.save_format, SaveFormat::Jpg);
        assert_eq!(settings.hotkeys.capture_panel, hotkey("Win+5"));
        assert_eq!(
            settings.looks.look(Tool::Rectangle).style().stroke_width,
            8.0
        );
    }

    #[test]
    fn random_bytes_are_read_safely() {
        let mut dice = Dice(0x5EED_C0DE);
        for _ in 0..20_000 {
            let length = dice.below(200);
            let bytes: Vec<u8> = (0..length).map(|_| dice.byte()).collect();
            assert_read_safely(&bytes);
        }
        let oversize: Vec<u8> = (0..=MAX_SETTINGS_BYTES).map(|_| dice.byte()).collect();
        assert_read_safely(&oversize);
    }

    #[test]
    fn every_truncation_of_a_settings_file_is_read_safely() {
        let bytes = EVERY_SETTING.as_bytes();
        for end in 0..=bytes.len() {
            assert_read_safely(&bytes[..end]);
            assert_read_safely(&bytes[end..]);
        }
    }

    #[test]
    fn a_settings_file_with_corrupted_bytes_is_read_safely() {
        let mut dice = Dice(0xBAD_F11E);
        for _ in 0..20_000 {
            let mut bytes = EVERY_SETTING.as_bytes().to_vec();
            for _ in 0..=dice.below(4) {
                let at = dice.below(bytes.len());
                match dice.below(3) {
                    0 => bytes[at] = dice.byte(),
                    1 => bytes.insert(at, dice.byte()),
                    _ => drop(bytes.remove(at)),
                }
            }
            assert_read_safely(&bytes);
        }
    }

    #[test]
    fn invalid_utf8_anywhere_makes_the_file_unreadable() {
        let bytes = EVERY_SETTING.as_bytes();
        let invalid: [&[u8]; 5] = [
            b"\xFF",
            b"\xC0\xAF",
            b"\xED\xA0\x80",
            b"\xF0\x9F\x93",
            b"\x80",
        ];
        for sequence in invalid {
            for at in [0, 15, bytes.len() / 2, bytes.len()] {
                let corrupted = [&bytes[..at], sequence, &bytes[at..]].concat();
                assert_read_safely(&corrupted);
                assert_eq!(
                    Settings::from_toml(&corrupted).1,
                    [SettingsProblem::Unreadable],
                    "{sequence:?} at {at}"
                );
            }
        }
    }

    #[test]
    fn huge_values_are_read_safely() {
        let long = "x".repeat(MAX_SETTINGS_BYTES - 32);
        let mut texts = vec![
            "show_pointer = 99999999999999999999999999\n".to_owned(),
            "show_pointer = -9223372036854775808\n".to_owned(),
            "save_format = 1e999\nafter_capture = nan\nshow_thumbnail = -inf\n".to_owned(),
            "save_format = 1979-05-27T07:32:00Z\n".to_owned(),
            format!("save_folder = \"{long}\"\n"),
            format!("{long} = 1\n"),
            format!(
                "hotkeys = [{}1]\n",
                "1,".repeat(MAX_SETTINGS_BYTES / 2 - 16)
            ),
            (0..6000)
                .map(|n| format!("k{n} = 1\n"))
                .collect::<Vec<_>>()
                .concat(),
            (0..4000)
                .map(|n| format!("[t{n}]\n"))
                .collect::<Vec<_>>()
                .concat(),
            format!("[hotkeys]\nregion = \"{}A\"\n", "Ctrl+".repeat(10_000)),
        ];
        for key in [
            "F255",
            "F256",
            "F99999999999999999999",
            "f-1",
            "F\u{FF11}",
            "+",
            "\u{0}",
        ] {
            texts.push(format!(
                "[hotkeys]\nregion = \"{}\"\n",
                key.escape_default()
            ));
        }
        for text in texts {
            assert_read_safely(text.as_bytes());
        }
    }

    /// The app reads its settings on the main thread, whose stack on Windows is 1 MiB.
    #[test]
    fn nesting_past_the_toml_depth_limit_is_unreadable_on_a_main_thread_stack() {
        let read_on_main_thread_stack = |text: String| {
            std::thread::Builder::new()
                .stack_size(1024 * 1024)
                .spawn(move || {
                    assert_read_safely(text.as_bytes());
                    Settings::from_toml(text.as_bytes()).1
                })
                .expect("the test thread starts")
                .join()
                .expect("reading did not overflow the stack")
        };
        let shapes = [
            ("a = ", "[", "", "]", ""),
            ("a = ", "{a = ", "1", "}", ""),
            ("[", "a.", "a", "", "]"),
            ("", "a.", "a = 1", "", ""),
        ];
        for (before, opener, middle, closer, after) in shapes {
            let deepest = (MAX_SETTINGS_BYTES - 16) / (opener.len() + closer.len());
            for depth in [1, 79, 80, 81, 1_000, deepest] {
                let (opened, closed) = (opener.repeat(depth), closer.repeat(depth));
                let text = format!("{before}{opened}{middle}{closed}{after}");
                let problems = read_on_main_thread_stack(text);
                if depth > 100 {
                    assert_eq!(
                        problems,
                        [SettingsProblem::Unreadable],
                        "{opener} x {depth}"
                    );
                }
            }
        }
    }
}
