use crate::delivery::{SET_BY_ORGANISATION, saving_off_reason};
use crate::policy::{Policy, Saving};
use crate::settings::{SaveFormat, Settings};

/// An item on the toolbar's Options menu (F-12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionsItem {
    SaveFolder,
    Format(SaveFormat),
    ShowPointer,
    ShowThumbnail,
    RememberLastSelection,
}

/// One line of the Options menu: its item, whether it shows a check, and why the policy
/// greys it out, if it does (F-57).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OptionsEntry {
    pub item: OptionsItem,
    pub checked: bool,
    pub off: Option<&'static str>,
}

/// What a pick on the Options menu leads to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Picked {
    /// The settings with the pick applied, to be written like any settings change.
    Changed(Settings),
    /// The folder dialog opens; the folder chosen there becomes the save folder.
    ChooseFolder,
}

impl OptionsItem {
    /// The item's name, as the menu shows it and UI Automation reads it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::SaveFolder => "Save folder...",
            Self::Format(format) => format.name(),
            Self::ShowPointer => "Show pointer",
            Self::ShowThumbnail => "Show a thumbnail",
            Self::RememberLastSelection => "Remember last selection",
        }
    }

    /// What picking the item does to `settings`: a switch flips, a format becomes the save
    /// format, and Save folder asks for a folder.
    #[must_use]
    pub fn choose(self, settings: &Settings) -> Picked {
        let mut changed = settings.clone();
        match self {
            Self::SaveFolder => return Picked::ChooseFolder,
            Self::Format(format) => changed.save_format = format,
            Self::ShowPointer => changed.show_pointer = !settings.show_pointer,
            Self::ShowThumbnail => changed.show_thumbnail = !settings.show_thumbnail,
            Self::RememberLastSelection => {
                changed.remember_last_selection = !settings.remember_last_selection;
            }
        }
        Picked::Changed(changed)
    }
}

/// The Options menu for `settings` under `policy`, in the order it is shown: Save folder, the
/// formats, then the switches. A save option the policy decides is greyed out with its reason.
#[must_use]
pub fn options_menu(settings: &Settings, policy: &Policy) -> Vec<OptionsEntry> {
    let (folder_off, format_off, saved_format) = match policy.saving() {
        Saving::Off(off) => {
            let why = Some(saving_off_reason(*off));
            (why, why, settings.save_format)
        }
        Saving::Allowed { folder, png_only } => (
            folder.as_ref().map(|_| SET_BY_ORGANISATION),
            png_only.then_some(SET_BY_ORGANISATION),
            if *png_only {
                SaveFormat::Png
            } else {
                settings.save_format
            },
        ),
    };
    let entry = |item, checked, off| OptionsEntry { item, checked, off };
    let mut menu = vec![entry(OptionsItem::SaveFolder, false, folder_off)];
    menu.extend(
        [SaveFormat::Png, SaveFormat::Jpg, SaveFormat::Bmp].map(|format| {
            entry(
                OptionsItem::Format(format),
                format == saved_format,
                format_off,
            )
        }),
    );
    menu.extend([
        entry(OptionsItem::ShowPointer, settings.show_pointer, None),
        entry(OptionsItem::ShowThumbnail, settings.show_thumbnail, None),
        entry(
            OptionsItem::RememberLastSelection,
            settings.remember_last_selection,
            None,
        ),
    ]);
    menu
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{PolicyValue, SavingOff};

    use PolicyValue::{Missing, Number, Text};

    fn items(menu: &[OptionsEntry]) -> Vec<OptionsItem> {
        menu.iter().map(|entry| entry.item).collect()
    }

    fn off(menu: &[OptionsEntry]) -> Vec<Option<&'static str>> {
        menu.iter().map(|entry| entry.off).collect()
    }

    #[test]
    fn f12_the_menu_lists_save_folder_the_formats_then_the_switches() {
        let menu = options_menu(&Settings::default(), &Policy::default());
        assert_eq!(
            items(&menu),
            [
                OptionsItem::SaveFolder,
                OptionsItem::Format(SaveFormat::Png),
                OptionsItem::Format(SaveFormat::Jpg),
                OptionsItem::Format(SaveFormat::Bmp),
                OptionsItem::ShowPointer,
                OptionsItem::ShowThumbnail,
                OptionsItem::RememberLastSelection,
            ]
        );
        assert_eq!(off(&menu), [None; 7]);
    }

    #[test]
    fn f12_the_checks_show_the_save_format_and_each_switch() {
        let settings = Settings {
            save_format: SaveFormat::Jpg,
            show_pointer: true,
            show_thumbnail: false,
            remember_last_selection: true,
            ..Settings::default()
        };
        let checked: Vec<bool> = options_menu(&settings, &Policy::default())
            .iter()
            .map(|entry| entry.checked)
            .collect();
        assert_eq!(checked, [false, false, true, false, true, false, true]);
    }

    #[test]
    fn f12_a_pick_flips_a_switch_sets_the_format_or_asks_for_a_folder() {
        let settings = Settings::default();
        assert_eq!(
            OptionsItem::SaveFolder.choose(&settings),
            Picked::ChooseFolder
        );
        for (item, expected) in [
            (
                OptionsItem::Format(SaveFormat::Bmp),
                Settings {
                    save_format: SaveFormat::Bmp,
                    ..settings.clone()
                },
            ),
            (
                OptionsItem::ShowPointer,
                Settings {
                    show_pointer: !settings.show_pointer,
                    ..settings.clone()
                },
            ),
            (
                OptionsItem::ShowThumbnail,
                Settings {
                    show_thumbnail: !settings.show_thumbnail,
                    ..settings.clone()
                },
            ),
            (
                OptionsItem::RememberLastSelection,
                Settings {
                    remember_last_selection: !settings.remember_last_selection,
                    ..settings.clone()
                },
            ),
        ] {
            assert_eq!(
                item.choose(&settings),
                Picked::Changed(expected),
                "{item:?}"
            );
        }
    }

    #[test]
    fn f12_f57_saving_off_greys_every_save_option_with_its_reason() {
        let (policy, _) = Policy::from_values(Missing, Number(1), Missing, Missing);
        let why = Some(saving_off_reason(SavingOff::ClipboardOnly));
        assert_eq!(
            off(&options_menu(&Settings::default(), &policy)),
            [why, why, why, why, None, None, None]
        );
    }

    #[test]
    fn f12_f57_a_policy_folder_greys_save_folder_and_png_only_greys_the_formats_on_png() {
        let settings = Settings {
            save_format: SaveFormat::Jpg,
            ..Settings::default()
        };
        let by_policy = Some(SET_BY_ORGANISATION);
        let (folder, _) =
            Policy::from_values(Text(r"D:\Shots".to_owned()), Missing, Missing, Missing);
        assert_eq!(
            off(&options_menu(&settings, &folder)),
            [by_policy, None, None, None, None, None, None]
        );
        let (png_only, _) = Policy::from_values(Missing, Missing, Number(1), Missing);
        let menu = options_menu(&settings, &png_only);
        assert_eq!(
            off(&menu),
            [None, by_policy, by_policy, by_policy, None, None, None]
        );
        assert!(
            menu[1].checked && !menu[2].checked,
            "PNG is what a save uses"
        );
    }
}
