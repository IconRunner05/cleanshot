use std::io::ErrorKind;
use std::path::PathBuf;

use crate::settings::{Hotkey, HotkeyAction, HotkeyRefusal, SettingsProblem};

/// What applying a change from the settings window did (F-55).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    pub saved: Result<(), SettingsSaveError>,
    /// The actions whose new hotkey another program holds; each keeps its old one.
    pub taken: Vec<HotkeyAction>,
}

/// Why changed settings were not written to the settings file. They still apply until
/// Cleanshot closes.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SettingsSaveError {
    #[error("Windows could not name the folder for the settings file")]
    NoSettingsPath,
    #[error("the settings file {path:?} could not be written: {kind}")]
    NotWritten { path: PathBuf, kind: ErrorKind },
}

/// Something the settings window tells the user about a change it made or refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsNotice {
    NotSaved(SettingsSaveError),
    /// Another program holds `wanted`, so `action` keeps `kept`.
    HotkeyTaken {
        action: HotkeyAction,
        wanted: Hotkey,
        kept: Hotkey,
    },
    /// `wanted` is already `other`'s hotkey, so `action` did not change.
    HotkeyShared {
        wanted: Hotkey,
        other: HotkeyAction,
    },
    HotkeyRefused(HotkeyRefusal),
    /// The settings file could not hold the change, so it was not made.
    Refused(Vec<SettingsProblem>),
    NoFolderDialog,
}

/// The sentence the settings window shows for `notice`: what happened and what to do.
#[must_use]
pub fn settings_notice(notice: &SettingsNotice) -> String {
    let lasts = "so this change lasts only until Cleanshot closes";
    match notice {
        SettingsNotice::NotSaved(SettingsSaveError::NoSettingsPath) => {
            format!("Windows could not find a folder for the settings file, {lasts}.")
        }
        SettingsNotice::NotSaved(SettingsSaveError::NotWritten { path, kind }) => format!(
            "The settings could not be saved to {} ({kind}), {lasts}.",
            path.display()
        ),
        SettingsNotice::HotkeyTaken {
            action,
            wanted,
            kept,
        } => format!(
            "Another program uses {wanted}, so {} keeps {kept}. Press another.",
            action.name()
        ),
        SettingsNotice::HotkeyShared { wanted, other } => format!(
            "{wanted} is already the hotkey for {}. Press another.",
            other.name()
        ),
        SettingsNotice::HotkeyRefused(HotkeyRefusal::UnknownKey) => {
            "That key cannot be a hotkey. Use a letter, a digit or F1 to F24.".to_owned()
        }
        SettingsNotice::HotkeyRefused(HotkeyRefusal::NoModifier) => {
            "A letter or digit needs Ctrl, Alt or Win held with it. Press another.".to_owned()
        }
        SettingsNotice::NoFolderDialog => {
            "The folder dialog did not open, so the save folder did not change.".to_owned()
        }
        SettingsNotice::Refused(problems) => {
            problems.iter().map(refusal).collect::<Vec<_>>().join(" ")
        }
    }
}

fn refusal(problem: &SettingsProblem) -> String {
    match problem {
        SettingsProblem::InvalidValue { key } if key == "save_folder" => {
            "The settings file cannot hold that folder's path, so the save folder did not change."
                .to_owned()
        }
        SettingsProblem::InvalidValue { key } => {
            format!("The settings file cannot hold that {key}, so it did not change.")
        }
        SettingsProblem::TooLarge { .. } | SettingsProblem::Unreadable => {
            "The settings file cannot hold that change, so nothing changed.".to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f55_each_notice_says_what_happened_and_what_to_do() {
        let hotkey = |text| Hotkey::parse(text).unwrap();
        let taken = SettingsNotice::HotkeyTaken {
            action: HotkeyAction::Region,
            wanted: hotkey("Ctrl+F9"),
            kept: hotkey("PrtSc"),
        };
        assert_eq!(
            settings_notice(&taken),
            "Another program uses Ctrl+F9, so Capture region keeps PrtSc. Press another."
        );
        let shared = SettingsNotice::HotkeyShared {
            wanted: hotkey("Ctrl+PrtSc"),
            other: HotkeyAction::FullScreen,
        };
        assert_eq!(
            settings_notice(&shared),
            "Ctrl+PrtSc is already the hotkey for Capture screen. Press another."
        );
        let unwritten = SettingsNotice::NotSaved(SettingsSaveError::NotWritten {
            path: PathBuf::from("cleanshot.toml"),
            kind: ErrorKind::PermissionDenied,
        });
        assert_eq!(
            settings_notice(&unwritten),
            "The settings could not be saved to cleanshot.toml (permission denied), so this \
             change lasts only until Cleanshot closes."
        );
        let folder = SettingsNotice::Refused(vec![SettingsProblem::InvalidValue {
            key: "save_folder".into(),
        }]);
        assert_eq!(
            settings_notice(&folder),
            "The settings file cannot hold that folder's path, so the save folder did not change."
        );
    }
}
