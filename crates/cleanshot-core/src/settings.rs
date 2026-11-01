use toml::{Table, Value};

/// The largest settings file read, in bytes (C-13).
pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;

/// The longest `save_folder` read, in UTF-8 bytes: written back with every other setting at its
/// longest and every byte escaped (`\u001F`, 6 bytes), the file stays within
/// [`MAX_SETTINGS_BYTES`] (C-13).
const MAX_SAVE_FOLDER_BYTES: usize = (MAX_SETTINGS_BYTES - 1024) / 6;

// F-54: a folder within the limit is also within Windows' 32,767-unit path limit.
const _: () = assert!(MAX_SAVE_FOLDER_BYTES <= 32_767);

/// Everything `cleanshot.toml` holds (F-54). Each field has the default [`Settings::default`]
/// gives it.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is its own on/off setting in the file"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// `None` means the default folder, Pictures\Screenshots, which the caller resolves.
    pub save_folder: Option<String>,
    pub save_format: SaveFormat,
    pub after_capture: AfterCapture,
    pub show_thumbnail: bool,
    pub show_pointer: bool,
    pub remember_last_selection: bool,
    pub start_with_windows: bool,
    pub hotkeys: Hotkeys,
}

/// The image format Save writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveFormat {
    Png,
    Jpg,
    Bmp,
}

/// What happens once a region is selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterCapture {
    ShowToolbar,
    Copy,
}

/// The global hotkey for each capture action (F-51).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkeys {
    pub region: Hotkey,
    pub last_region: Hotkey,
    pub full_screen: Hotkey,
    pub window: Hotkey,
    pub capture_panel: Hotkey,
}

/// A capture action whose hotkey the tray registers and the settings window edits (F-51, F-55).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    Region,
    FullScreen,
    LastRegion,
}

impl HotkeyAction {
    /// Every action, in the order the settings window lists them.
    pub const ALL: [Self; 3] = [Self::Region, Self::FullScreen, Self::LastRegion];
}

/// A key with its modifiers, written as `Ctrl+Shift+4`. Built only by [`Hotkey::parse`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    modifiers: Modifiers,
    key: Key,
}

/// The modifier keys a hotkey needs held with its key.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each modifier key is held or not, independently"
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

/// The one non-modifier key of a hotkey: an ASCII capital letter or digit, F1 to F24, or
/// Print Screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Letter(char),
    Digit(char),
    Function(u8),
    PrintScreen,
}

/// A part of the settings file that was not used, so its default applies (F-54).
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
            show_pointer: false,
            remember_last_selection: false,
            start_with_windows: false,
            hotkeys: Hotkeys::default(),
        }
    }
}

impl Default for Hotkeys {
    fn default() -> Self {
        let modified = |modifiers, key| Hotkey { modifiers, key };
        let none = Modifiers::default();
        Self {
            region: modified(none, Key::PrintScreen),
            last_region: modified(
                Modifiers {
                    shift: true,
                    ..none
                },
                Key::PrintScreen,
            ),
            full_screen: modified(Modifiers { ctrl: true, ..none }, Key::PrintScreen),
            window: modified(Modifiers { alt: true, ..none }, Key::PrintScreen),
            capture_panel: modified(
                Modifiers {
                    ctrl: true,
                    shift: true,
                    ..none
                },
                Key::Digit('5'),
            ),
        }
    }
}

impl Settings {
    /// `self`, if a settings file holding it reads back with no problem; else the problems that
    /// file's reader would report, so nothing is kept that the next start would refuse (F-54,
    /// F-55).
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
    /// keys are ignored (F-54); input over [`MAX_SETTINGS_BYTES`] is not parsed (C-13).
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
            prefix: "",
            problems: Vec::new(),
        };
        let defaults = Self::default();
        let save_folder = reader.read("save_folder", |value| {
            value
                .as_str()
                .filter(|folder| !folder.is_empty() && folder.len() <= MAX_SAVE_FOLDER_BYTES)
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
            show_pointer: reader
                .read("show_pointer", Value::as_bool)
                .unwrap_or(defaults.show_pointer),
            remember_last_selection: reader
                .read("remember_last_selection", Value::as_bool)
                .unwrap_or(defaults.remember_last_selection),
            start_with_windows: reader
                .read("start_with_windows", Value::as_bool)
                .unwrap_or(defaults.start_with_windows),
            hotkeys: reader.hotkeys(defaults.hotkeys),
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
        table.insert("show_pointer".into(), self.show_pointer.into());
        table.insert(
            "remember_last_selection".into(),
            self.remember_last_selection.into(),
        );
        table.insert("start_with_windows".into(), self.start_with_windows.into());
        let mut hotkeys = Table::new();
        for (key, hotkey) in self.hotkeys.named() {
            hotkeys.insert(key.into(), hotkey.to_string().into());
        }
        table.insert("hotkeys".into(), hotkeys.into());
        table.to_string()
    }
}

impl Hotkeys {
    /// The hotkey `action` is captured by.
    #[must_use]
    pub fn get(&self, action: HotkeyAction) -> Hotkey {
        match action {
            HotkeyAction::Region => self.region,
            HotkeyAction::FullScreen => self.full_screen,
            HotkeyAction::LastRegion => self.last_region,
        }
    }

    /// Makes `hotkey` capture `action`.
    pub fn set(&mut self, action: HotkeyAction, hotkey: Hotkey) {
        let slot = match action {
            HotkeyAction::Region => &mut self.region,
            HotkeyAction::FullScreen => &mut self.full_screen,
            HotkeyAction::LastRegion => &mut self.last_region,
        };
        *slot = hotkey;
    }

    /// The other action `hotkey` already captures, if any: no two actions share a hotkey.
    #[must_use]
    pub fn used_by_another(&self, action: HotkeyAction, hotkey: Hotkey) -> Option<HotkeyAction> {
        HotkeyAction::ALL
            .into_iter()
            .find(|&other| other != action && self.get(other) == hotkey)
    }

    fn named(&self) -> [(&'static str, Hotkey); 5] {
        [
            ("region", self.region),
            ("last_region", self.last_region),
            ("full_screen", self.full_screen),
            ("window", self.window),
            ("capture_panel", self.capture_panel),
        ]
    }
}

impl Hotkey {
    /// The hotkey `text` names: any of `Ctrl`, `Alt`, `Shift` and `Win` once each, then one key
    /// (`A`-`Z`, `0`-`9`, `F1`-`F24` or `PrtSc`), joined by `+`. Names ignore case; `None` for
    /// anything else.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let key = key(parts.pop()?)?;
        let mut modifiers = Modifiers::default();
        for part in parts {
            let flag = match part.to_ascii_lowercase().as_str() {
                "ctrl" => &mut modifiers.ctrl,
                "alt" => &mut modifiers.alt,
                "shift" => &mut modifiers.shift,
                "win" => &mut modifiers.win,
                _ => return None,
            };
            if *flag {
                return None;
            }
            *flag = true;
        }
        Some(Self { modifiers, key })
    }

    /// The modifier keys held with [`Hotkey::key`].
    #[must_use]
    pub fn modifiers(self) -> Modifiers {
        self.modifiers
    }

    /// The one non-modifier key.
    #[must_use]
    pub fn key(self) -> Key {
        self.key
    }
}

impl std::fmt::Display for Hotkey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Modifiers {
            ctrl,
            alt,
            shift,
            win,
        } = self.modifiers;
        for (held, name) in [(ctrl, "Ctrl"), (alt, "Alt"), (shift, "Shift"), (win, "Win")] {
            if held {
                write!(formatter, "{name}+")?;
            }
        }
        match self.key {
            Key::Letter(character) | Key::Digit(character) => write!(formatter, "{character}"),
            Key::Function(number) => write!(formatter, "F{number}"),
            Key::PrintScreen => write!(formatter, "PrtSc"),
        }
    }
}

fn key(name: &str) -> Option<Key> {
    if name.eq_ignore_ascii_case("prtsc") {
        return Some(Key::PrintScreen);
    }
    let mut characters = name.chars();
    let first = characters.next()?;
    let rest = characters.as_str();
    match (first.to_ascii_uppercase(), rest) {
        (letter @ 'A'..='Z', "") => Some(Key::Letter(letter)),
        (digit @ '0'..='9', "") => Some(Key::Digit(digit)),
        ('F', number) if !number.starts_with('0') => number
            .parse::<u8>()
            .ok()
            .filter(|number| (1..=24).contains(number))
            .map(Key::Function),
        _ => None,
    }
}

/// Reads typed fields from one table, noting each key present with an unusable value.
struct Reader<'a> {
    table: &'a Table,
    prefix: &'static str,
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

    fn hotkeys(&mut self, defaults: Hotkeys) -> Hotkeys {
        let Some(table) = self.read("hotkeys", |value| value.as_table().cloned()) else {
            return defaults;
        };
        let mut reader = Reader {
            table: &table,
            prefix: "hotkeys",
            problems: Vec::new(),
        };
        let mut read = |key, default| {
            reader
                .read(key, |value| value.as_str().and_then(Hotkey::parse))
                .unwrap_or(default)
        };
        let hotkeys = Hotkeys {
            region: read("region", defaults.region),
            last_region: read("last_region", defaults.last_region),
            full_screen: read("full_screen", defaults.full_screen),
            window: read("window", defaults.window),
            capture_panel: read("capture_panel", defaults.capture_panel),
        };
        self.problems.append(&mut reader.problems);
        hotkeys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hotkey(text: &str) -> Hotkey {
        Hotkey::parse(text).unwrap()
    }

    #[test]
    fn f54_an_empty_file_gives_every_default() {
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
    fn f54_unknown_keys_are_ignored_without_a_problem() {
        let text = "colour = \"blue\"\nshow_pointer = true\n[hotkeys]\nsparkle = \"F9\"\n";
        let (settings, problems) = Settings::from_toml(text.as_bytes());
        assert_eq!(problems, []);
        assert!(settings.show_pointer);
    }

    #[test]
    fn f54_an_invalid_value_falls_back_to_its_default_and_is_reported() {
        let text = "save_format = \"gif\"\nshow_pointer = \"yes\"\nstart_with_windows = true\n\
                    [hotkeys]\nregion = \"Ctrl+Ctrl+4\"\nwindow = \"Win+W\"\n";
        let (settings, problems) = Settings::from_toml(text.as_bytes());

        assert_eq!(settings.save_format, SaveFormat::Png);
        assert!(!settings.show_pointer);
        assert_eq!(settings.hotkeys.region, hotkey("PrtSc"));
        assert!(settings.start_with_windows);
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
    fn f54_a_hotkeys_entry_that_is_not_a_table_is_reported_and_defaulted() {
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
    fn c13_a_file_over_64_kb_is_not_read() {
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
    fn f54_text_that_is_not_toml_gives_every_default() {
        for bytes in [&b"show_pointer = = true"[..], &[0xFF, 0xFE, 0x00]] {
            let (settings, problems) = Settings::from_toml(bytes);
            assert_eq!(settings, Settings::default());
            assert_eq!(problems, [SettingsProblem::Unreadable]);
        }
    }

    #[test]
    fn f54_settings_written_out_read_back_unchanged() {
        let settings = Settings {
            save_folder: Some(r"D:\Shots".into()),
            save_format: SaveFormat::Bmp,
            after_capture: AfterCapture::Copy,
            show_thumbnail: false,
            show_pointer: true,
            remember_last_selection: true,
            start_with_windows: true,
            hotkeys: Hotkeys {
                region: hotkey("Ctrl+Shift+4"),
                ..Hotkeys::default()
            },
        };
        let (read_back, problems) = Settings::from_toml(settings.to_toml().as_bytes());
        assert_eq!(problems, []);
        assert_eq!(read_back, settings);
    }

    #[test]
    fn f55_a_setting_the_next_start_would_refuse_is_refused_now_with_its_problem() {
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
    fn f55_no_two_actions_share_a_hotkey() {
        let mut hotkeys = Hotkeys::default();
        let screen = hotkeys.get(HotkeyAction::FullScreen);
        assert_eq!(
            hotkeys.used_by_another(HotkeyAction::Region, screen),
            Some(HotkeyAction::FullScreen)
        );
        assert_eq!(
            hotkeys.used_by_another(HotkeyAction::FullScreen, screen),
            None
        );
        hotkeys.set(HotkeyAction::FullScreen, hotkey("Ctrl+F9"));
        assert_eq!(hotkeys.full_screen, hotkey("Ctrl+F9"));
        assert_eq!(hotkeys.used_by_another(HotkeyAction::Region, screen), None);
    }

    #[test]
    fn c13_the_largest_settings_read_write_back_within_the_size_limit() {
        let longest = hotkey("Ctrl+Alt+Shift+Win+F24");
        let largest = Settings {
            save_folder: Some("\u{1F}".repeat(MAX_SAVE_FOLDER_BYTES)),
            hotkeys: Hotkeys {
                region: longest,
                last_region: longest,
                full_screen: longest,
                window: longest,
                capture_panel: longest,
            },
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

    #[test]
    fn f51_hotkey_names_ignore_case_and_print_in_one_spelling() {
        assert_eq!(hotkey(" shift + ctrl + f12 ").to_string(), "Ctrl+Shift+F12");
        assert_eq!(hotkey("win+alt+a").to_string(), "Alt+Win+A");
        assert_eq!(hotkey("prtsc").key(), Key::PrintScreen);
        assert_eq!(hotkey("F24").key(), Key::Function(24));
    }

    #[test]
    fn f51_a_hotkey_needs_exactly_one_known_key_after_distinct_modifiers() {
        for text in [
            "",
            "Ctrl+",
            "Ctrl+Ctrl+A",
            "Ctrl+A+B",
            "Hyper+A",
            "F0",
            "F25",
            "F01",
            "AB",
            "Ctrl+Ä",
        ] {
            assert_eq!(Hotkey::parse(text), None, "{text:?}");
        }
    }

    const EVERY_SETTING: &str = "save_folder = 'D:\\Shots\\Bildschirmfotos \u{1F4F7}'\n\
        save_format = \"jpg\"\nafter_capture = \"copy\"\nshow_thumbnail = false\n\
        show_pointer = true\nremember_last_selection = true\nstart_with_windows = true\n\
        [hotkeys]\nregion = \"Ctrl+Shift+4\"\nlast_region = \"Shift+PrtSc\"\n\
        full_screen = \"Ctrl+PrtSc\"\nwindow = \"Alt+F24\"\ncapture_panel = \"Win+5\"\n";

    const KEYS: [&str; 13] = [
        "save_folder",
        "save_format",
        "after_capture",
        "show_thumbnail",
        "show_pointer",
        "remember_last_selection",
        "start_with_windows",
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

    /// C-13, F-54: whatever the bytes, `from_toml` returns, and what it returns reads back the
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
                    assert!(KEYS.contains(&key.as_str()), "{key} in {context}");
                    assert!(!seen.contains(key), "{key} twice in {context}");
                    seen.push(key.clone());
                }
            }
        }
        let read_back = Settings::from_toml(settings.to_toml().as_bytes());
        assert_eq!(read_back, (settings, Vec::new()), "{context}");
    }

    #[test]
    fn c13_the_corruption_fixture_sets_every_setting_without_a_problem() {
        let (settings, problems) = Settings::from_toml(EVERY_SETTING.as_bytes());
        assert_eq!(problems, []);
        assert_eq!(settings.save_format, SaveFormat::Jpg);
        assert_eq!(settings.hotkeys.capture_panel, hotkey("Win+5"));
    }

    #[test]
    fn c13_random_bytes_are_read_safely() {
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
    fn c13_every_truncation_of_a_settings_file_is_read_safely() {
        let bytes = EVERY_SETTING.as_bytes();
        for end in 0..=bytes.len() {
            assert_read_safely(&bytes[..end]);
            assert_read_safely(&bytes[end..]);
        }
    }

    #[test]
    fn c13_a_settings_file_with_corrupted_bytes_is_read_safely() {
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
    fn c13_invalid_utf8_anywhere_makes_the_file_unreadable() {
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
    fn c13_huge_values_are_read_safely() {
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
            "F-1",
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
    fn c13_nesting_past_the_toml_depth_limit_is_unreadable_on_a_main_thread_stack() {
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
