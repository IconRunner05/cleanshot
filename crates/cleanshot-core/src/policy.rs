use crate::settings::{FolderKind, SaveFormat, Settings, folder_kind};

/// One policy value as Windows read it, before it is checked (F-57).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyValue {
    Missing,
    /// A `REG_DWORD`.
    Number(u32),
    /// A `REG_SZ`, or a `REG_EXPAND_SZ` with its variables expanded.
    Text(String),
    /// Present, but of another type or not readable.
    Unreadable,
}

/// The registry key under `HKEY_LOCAL_MACHINE` that holds the policy values (D-86).
pub const POLICY_KEY: &str = r"SOFTWARE\Policies\Cleanshot";

/// A policy an organisation can set (F-57).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicySetting {
    SaveFolder,
    ClipboardOnly,
    ForcePng,
    HideSettings,
}

impl PolicySetting {
    /// Every policy, in the order the Group Policy template lists them.
    pub const ALL: [Self; 4] = [
        Self::SaveFolder,
        Self::ClipboardOnly,
        Self::ForcePng,
        Self::HideSettings,
    ];

    /// The name of its value under [`POLICY_KEY`], as the Group Policy template writes it.
    #[must_use]
    pub fn value_name(self) -> &'static str {
        match self {
            Self::SaveFolder => "SaveFolder",
            Self::ClipboardOnly => "ClipboardOnly",
            Self::ForcePng => "ForcePng",
            Self::HideSettings => "HideSettings",
        }
    }
}

/// A policy value that was present but not valid, so its strictest setting applies (D-86).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the {0:?} policy is not valid, so its strictest setting applies")]
pub struct PolicyProblem(pub PolicySetting);

/// Whether, and how, a capture may be saved to a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Saving {
    /// Saved as the settings say, except into `folder` when the policy names one, and only as
    /// PNG when `png_only`.
    Allowed {
        folder: Option<String>,
        png_only: bool,
    },
    /// Never saved; a capture can only be copied.
    Off(SavingOff),
}

impl Saving {
    /// Ok when a capture may be saved; otherwise why the policy turned saving off (F-57).
    ///
    /// # Errors
    ///
    /// The [`SavingOff`] reason when saving is off.
    pub fn allowed(&self) -> Result<(), SavingOff> {
        match self {
            Self::Allowed { .. } => Ok(()),
            Self::Off(off) => Err(*off),
        }
    }

    /// The format a save uses: PNG when the policy forces it (F-57), else the format `typed`
    /// in the file name (F-22), else `chosen` in the settings.
    #[must_use]
    pub fn format(&self, typed: Option<SaveFormat>, chosen: SaveFormat) -> SaveFormat {
        match self {
            Self::Allowed { png_only: true, .. } => SaveFormat::Png,
            Self::Allowed { .. } | Self::Off(_) => typed.unwrap_or(chosen),
        }
    }
}

/// Why a policy turned saving off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavingOff {
    ClipboardOnly,
    /// The policy's save folder is not a full path on a drive or a share.
    FolderNotValid,
}

/// What an organisation's policy decides, read once at start (F-57, D-86). The default is no
/// policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    saving: Saving,
    settings_hidden: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            saving: Saving::Allowed {
                folder: None,
                png_only: false,
            },
            settings_hidden: false,
        }
    }
}

impl Policy {
    /// The policy the four values set. A value that is present but not valid takes its
    /// strictest setting and is reported: a switch is on, and a save folder turns saving off.
    #[must_use]
    pub fn from_values(
        save_folder: PolicyValue,
        clipboard_only: PolicyValue,
        force_png: PolicyValue,
        hide_settings: PolicyValue,
    ) -> (Self, Vec<PolicyProblem>) {
        let mut problems = Vec::new();
        let mut switch = |setting, value| match value {
            PolicyValue::Missing | PolicyValue::Number(0) => false,
            PolicyValue::Number(_) => true,
            PolicyValue::Text(_) | PolicyValue::Unreadable => {
                problems.push(PolicyProblem(setting));
                true
            }
        };
        let clipboard_only = switch(PolicySetting::ClipboardOnly, clipboard_only);
        let png_only = switch(PolicySetting::ForcePng, force_png);
        let settings_hidden = switch(PolicySetting::HideSettings, hide_settings);
        let folder = match save_folder {
            PolicyValue::Missing => Ok(None),
            PolicyValue::Text(folder)
                if matches!(folder_kind(&folder), FolderKind::Drive | FolderKind::Share) =>
            {
                Ok(Some(folder))
            }
            PolicyValue::Text(_) | PolicyValue::Number(_) | PolicyValue::Unreadable => {
                problems.push(PolicyProblem(PolicySetting::SaveFolder));
                Err(SavingOff::FolderNotValid)
            }
        };
        let saving = match (clipboard_only, folder) {
            (true, _) => Saving::Off(SavingOff::ClipboardOnly),
            (false, Err(off)) => Saving::Off(off),
            (false, Ok(folder)) => Saving::Allowed { folder, png_only },
        };
        let policy = Self {
            saving,
            settings_hidden,
        };
        (policy, problems)
    }

    #[must_use]
    pub fn saving(&self) -> &Saving {
        &self.saving
    }

    /// Whether the settings window is never opened. The settings file is still read.
    #[must_use]
    pub fn settings_hidden(&self) -> bool {
        self.settings_hidden
    }

    /// The settings a save uses: `settings` with the policy's folder and format in place of the
    /// file's, where the policy sets them.
    #[must_use]
    pub fn applied_to(&self, settings: Settings) -> Settings {
        match &self.saving {
            Saving::Allowed { folder, png_only } => Settings {
                save_folder: folder.clone().or(settings.save_folder),
                save_format: if *png_only {
                    SaveFormat::Png
                } else {
                    settings.save_format
                },
                ..settings
            },
            Saving::Off(_) => settings,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use PolicyValue::{Missing, Number, Text, Unreadable};

    fn text(value: &str) -> PolicyValue {
        Text(value.to_owned())
    }

    #[test]
    fn f22_f57_a_save_uses_png_when_forced_else_the_typed_format_else_the_setting() {
        let (allowed, _) = Policy::from_values(Missing, Missing, Missing, Missing);
        let (png_only, _) = Policy::from_values(Missing, Missing, Number(1), Missing);
        let typed = Some(SaveFormat::Bmp);
        assert_eq!(
            allowed.saving().format(typed, SaveFormat::Jpg),
            SaveFormat::Bmp
        );
        assert_eq!(
            allowed.saving().format(None, SaveFormat::Jpg),
            SaveFormat::Jpg
        );
        assert_eq!(
            png_only.saving().format(typed, SaveFormat::Jpg),
            SaveFormat::Png
        );
        assert_eq!(
            png_only.saving().format(None, SaveFormat::Jpg),
            SaveFormat::Png
        );
    }

    #[test]
    fn f57_no_policy_values_change_nothing() {
        let (policy, problems) = Policy::from_values(Missing, Missing, Missing, Missing);
        assert_eq!(policy, Policy::default());
        assert_eq!(problems, Vec::new());
        let settings = Settings::default();
        assert_eq!(policy.applied_to(settings.clone()), settings);
    }

    #[test]
    fn f57_a_switch_is_on_for_any_number_but_0_and_on_for_a_value_of_the_wrong_type() {
        for (value, on, problem) in [
            (Number(0), false, false),
            (Number(1), true, false),
            (Number(7), true, false),
            (text("0"), true, true),
            (Unreadable, true, true),
        ] {
            let (policy, problems) = Policy::from_values(Missing, Missing, Missing, value.clone());
            assert_eq!(policy.settings_hidden(), on, "{value:?}");
            assert_eq!(
                problems,
                if problem {
                    vec![PolicyProblem(PolicySetting::HideSettings)]
                } else {
                    Vec::new()
                },
                "{value:?}"
            );
        }
    }

    #[test]
    fn f57_a_policy_folder_and_png_replace_the_files_and_a_share_is_allowed() {
        let settings = Settings {
            save_folder: Some(r"D:\Mine".to_owned()),
            save_format: SaveFormat::Jpg,
            ..Settings::default()
        };
        for folder in [r"C:\Shots", "c:/shots", r"\\fileserver\screenshots"] {
            let (policy, problems) = Policy::from_values(text(folder), Missing, Number(1), Missing);
            assert_eq!(problems, Vec::new(), "{folder}");
            let applied = policy.applied_to(settings.clone());
            assert_eq!(applied.save_folder.as_deref(), Some(folder));
            assert_eq!(applied.save_format, SaveFormat::Png);
        }
    }

    #[test]
    fn f57_a_policy_folder_that_is_not_valid_turns_saving_off_rather_than_falling_back() {
        for value in [
            text(""),
            text(r"Shots"),
            text(r"\Shots"),
            text(r"\\?\C:\Shots"),
            text(r"\\.\pipe\shots"),
            text(r"\\fileserver"),
            text(r"\\fileserver\"),
            Number(1),
            Unreadable,
        ] {
            let (policy, problems) = Policy::from_values(value.clone(), Missing, Missing, Missing);
            assert_eq!(
                policy.saving(),
                &Saving::Off(SavingOff::FolderNotValid),
                "{value:?}"
            );
            assert_eq!(problems, vec![PolicyProblem(PolicySetting::SaveFolder)]);
        }
    }

    #[test]
    fn f57_saving_is_allowed_or_refused_with_the_policy_reason() {
        assert_eq!(Policy::default().saving().allowed(), Ok(()));
        let (clipboard_only, _) = Policy::from_values(Missing, Number(1), Missing, Missing);
        assert_eq!(
            clipboard_only.saving().allowed(),
            Err(SavingOff::ClipboardOnly)
        );
        let (bad_folder, _) = Policy::from_values(Unreadable, Missing, Missing, Missing);
        assert_eq!(
            bad_folder.saving().allowed(),
            Err(SavingOff::FolderNotValid)
        );
    }

    #[test]
    fn f57_clipboard_only_wins_over_a_folder_and_png() {
        let (policy, _) = Policy::from_values(text(r"C:\Shots"), Number(1), Number(1), Missing);
        assert_eq!(policy.saving(), &Saving::Off(SavingOff::ClipboardOnly));
    }

    const ADMX: &str = include_str!("../../../policy/Cleanshot.admx");
    const ADML: &str = include_str!("../../../policy/en-US/Cleanshot.adml");

    #[test]
    fn f57_the_group_policy_template_writes_each_value_core_reads_with_its_type_and_no_other() {
        assert_eq!(ADMX.matches("<policy ").count(), PolicySetting::ALL.len());
        assert_eq!(ADMX.matches("valueName=").count(), PolicySetting::ALL.len());
        for setting in PolicySetting::ALL {
            let name = setting.value_name();
            let start = ADMX
                .find(&format!("<policy name=\"{name}\""))
                .unwrap_or_else(|| panic!("the template has no {name} policy"));
            let end = start + ADMX[start..].find("</policy>").unwrap();
            let policy = &ADMX[start..end];
            assert!(policy.contains(&format!("key=\"{POLICY_KEY}\"")), "{name}");
            let inside = |open: &str, close: &str| {
                let start = policy.find(open).map(|at| at + open.len())?;
                Some(&policy[start..start + policy[start..].find(close)?])
            };
            match setting {
                PolicySetting::SaveFolder => assert!(
                    policy.contains(&format!(
                        "<text id=\"{name}\" valueName=\"{name}\" expandable=\"true\""
                    )),
                    "{name}"
                ),
                PolicySetting::ClipboardOnly
                | PolicySetting::ForcePng
                | PolicySetting::HideSettings => {
                    assert!(policy.contains(&format!("valueName=\"{name}\"")), "{name}");
                    assert_eq!(
                        inside("<enabledValue>", "</enabledValue>").map(str::trim),
                        Some("<decimal value=\"1\" />"),
                        "{name}"
                    );
                    assert_eq!(
                        inside("<disabledValue>", "</disabledValue>").map(str::trim),
                        Some("<decimal value=\"0\" />"),
                        "{name}"
                    );
                }
            }
        }
    }

    #[test]
    fn f57_every_string_and_presentation_the_template_names_is_in_its_language_file() {
        for (kind, element) in [("string", "string"), ("presentation", "presentation")] {
            let reference = format!("$({kind}.");
            for (at, _) in ADMX.match_indices(&reference) {
                let id = &ADMX[at + reference.len()..];
                let id = &id[..id.find(')').unwrap()];
                assert!(
                    ADML.contains(&format!("<{element} id=\"{id}\">")),
                    "{kind} {id}"
                );
            }
        }
    }
}
