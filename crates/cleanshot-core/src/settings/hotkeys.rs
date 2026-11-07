use toml::Table;

use super::Reader;

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
    Window,
    CapturePanel,
}

impl HotkeyAction {
    /// Every action, in the order the settings window lists them.
    pub const ALL: [Self; 5] = [
        Self::Region,
        Self::FullScreen,
        Self::LastRegion,
        Self::Window,
        Self::CapturePanel,
    ];

    /// The action's name, as the settings window shows it and the tray menu, for the actions
    /// it lists (F-50, F-55).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Region => "Capture region",
            Self::FullScreen => "Capture screen",
            Self::LastRegion => "Capture last region",
            Self::Window => "Capture window",
            Self::CapturePanel => "Capture panel",
        }
    }
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

/// Why a key pressed in the settings window cannot be a hotkey.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyRefusal {
    UnknownKey,
    NoModifier,
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

impl Hotkeys {
    /// The hotkey `action` is captured by.
    #[must_use]
    pub fn get(&self, action: HotkeyAction) -> Hotkey {
        match action {
            HotkeyAction::Region => self.region,
            HotkeyAction::FullScreen => self.full_screen,
            HotkeyAction::LastRegion => self.last_region,
            HotkeyAction::Window => self.window,
            HotkeyAction::CapturePanel => self.capture_panel,
        }
    }

    /// Makes `hotkey` capture `action`.
    pub fn set(&mut self, action: HotkeyAction, hotkey: Hotkey) {
        let slot = match action {
            HotkeyAction::Region => &mut self.region,
            HotkeyAction::FullScreen => &mut self.full_screen,
            HotkeyAction::LastRegion => &mut self.last_region,
            HotkeyAction::Window => &mut self.window,
            HotkeyAction::CapturePanel => &mut self.capture_panel,
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

    /// The hotkey for `key`, as the settings window's keyboard reports it, held with `modifiers`
    /// (F-55). A letter or digit needs Ctrl, Alt or Win, or every program would lose that key.
    ///
    /// # Errors
    ///
    /// [`HotkeyRefusal::UnknownKey`] for a key [`Hotkey::parse`] does not name, and
    /// [`HotkeyRefusal::NoModifier`] for a letter or digit alone or with Shift only.
    pub fn recorded(key: &str, modifiers: Modifiers) -> Result<Self, HotkeyRefusal> {
        let held = Self {
            modifiers,
            key: self::key(key).ok_or(HotkeyRefusal::UnknownKey)?,
        };
        let typed = matches!(held.key, Key::Letter(_) | Key::Digit(_));
        if typed && !(modifiers.ctrl || modifiers.alt || modifiers.win) {
            return Err(HotkeyRefusal::NoModifier);
        }
        Ok(held)
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

/// `hotkeys` as the settings file's `[hotkeys]` table, every action named (F-51).
pub(super) fn table(hotkeys: &Hotkeys) -> Table {
    let mut table = Table::new();
    for (key, hotkey) in hotkeys.named() {
        table.insert(key.into(), hotkey.to_string().into());
    }
    table
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

impl Reader<'_> {
    pub(super) fn hotkeys(&mut self, defaults: Hotkeys) -> Hotkeys {
        let Some(table) = self.read("hotkeys", |value| value.as_table().cloned()) else {
            return defaults;
        };
        let mut reader = Reader {
            table: &table,
            prefix: "hotkeys".into(),
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
pub(super) mod tests {
    use super::*;
    use crate::settings::Settings;

    pub(in crate::settings) fn hotkey(text: &str) -> Hotkey {
        Hotkey::parse(text).unwrap()
    }

    #[test]
    fn f08_the_window_action_is_the_window_hotkey_alt_prtsc_by_default_and_no_other_may_take_it() {
        let mut hotkeys = Settings::default().hotkeys;
        assert_eq!(hotkeys.get(HotkeyAction::Window), hotkey("Alt+PrtSc"));
        hotkeys.set(HotkeyAction::Window, hotkey("Win+W"));
        assert_eq!(hotkeys.window, hotkey("Win+W"));
        assert_eq!(
            hotkeys.used_by_another(HotkeyAction::Region, hotkey("Win+W")),
            Some(HotkeyAction::Window)
        );
    }

    #[test]
    fn f55_a_recorded_letter_or_digit_needs_ctrl_alt_or_win_and_a_function_key_does_not() {
        let held = |ctrl, shift| Modifiers {
            ctrl,
            shift,
            ..Modifiers::default()
        };
        assert_eq!(
            Hotkey::recorded("4", held(true, true)),
            Ok(hotkey("Ctrl+Shift+4"))
        );
        assert_eq!(Hotkey::recorded("f9", held(false, false)), Ok(hotkey("F9")));
        assert_eq!(
            Hotkey::recorded("a", held(false, true)),
            Err(HotkeyRefusal::NoModifier)
        );
        assert_eq!(
            Hotkey::recorded("escape", held(true, false)),
            Err(HotkeyRefusal::UnknownKey)
        );
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
}
