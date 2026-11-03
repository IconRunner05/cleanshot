use std::path::{Path, PathBuf};

use crate::policy::SavingOff;

/// Where a finished screenshot goes, from the overlay or the editor (F-20, F-21, F-40).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Copy,
    Save,
}

/// What a delivery did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivered {
    Copied,
    Saved(PathBuf),
}

/// Why a delivery failed, one variant per failure the user can act on.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DeliveryError {
    #[error("a {width}x{height} image is larger than the renderer can hold")]
    TooLarge { width: u32, height: u32 },
    #[error("another program is holding the clipboard")]
    ClipboardBusy,
    #[error("Windows could not name or create the screenshots folder")]
    NoSaveFolder,
    #[error("the save folder {configured:?} is not a full path")]
    SaveFolderNotFull { configured: String },
    #[error("every numbered form of the file name {name:?} is taken")]
    NoFreeName { name: String },
    #[error("the file could not be written to {folder:?}")]
    NotWritten { folder: PathBuf },
    #[error("the text could not be drawn")]
    TextNotDrawn,
    #[error("the image could not be made from the capture and its markup")]
    ImageNotMade,
    #[error("the Save As dialog did not open")]
    NoSaveDialog,
    #[error("the organisation's policy turns saving off: {0:?}")]
    SavingOff(SavingOff),
    #[error("the organisation's policy saves only in {folder:?}")]
    OutsidePolicyFolder { folder: String },
}

impl From<SavingOff> for DeliveryError {
    fn from(off: SavingOff) -> Self {
        Self::SavingOff(off)
    }
}

/// The sentence the user sees after a Copy, Save or Save As, from the overlay or the editor (D-48): what
/// happened, naming the file, folder or name it is about.
#[must_use]
pub fn delivery_notice(outcome: &Result<Delivered, DeliveryError>) -> String {
    match outcome {
        Ok(Delivered::Copied) => "Copied to the clipboard".to_owned(),
        Ok(Delivered::Saved(path)) => format!("Saved to {}", path.display()),
        Err(DeliveryError::TooLarge { width, height }) => {
            format!("This {width}x{height} image is too large to export")
        }
        Err(DeliveryError::ClipboardBusy) => {
            "Another program is using the clipboard. Try again.".to_owned()
        }
        Err(DeliveryError::NoSaveFolder) => {
            "Windows could not find or create the Screenshots folder".to_owned()
        }
        Err(DeliveryError::SaveFolderNotFull { configured }) => {
            format!("The save folder \"{configured}\" in the settings is not a full path")
        }
        Err(DeliveryError::NoFreeName { name }) => {
            format!("Every numbered name for \"{name}\" is taken")
        }
        Err(DeliveryError::NotWritten { folder }) => {
            format!("Could not write to {}", folder.display())
        }
        Err(DeliveryError::TextNotDrawn) => {
            "The text could not be drawn, so nothing was copied or saved".to_owned()
        }
        Err(DeliveryError::ImageNotMade) => "The image could not be made. Try again.".to_owned(),
        Err(DeliveryError::NoSaveDialog) => "The Save As dialog did not open".to_owned(),
        Err(DeliveryError::SavingOff(off)) => saving_off_notice(*off),
        Err(DeliveryError::OutsidePolicyFolder { folder }) => {
            format!("Your organisation saves screenshots only in {folder}. Choose a file there.")
        }
    }
}

/// Why a setting is greyed out where the organisation's policy decides it (F-57).
pub const SET_BY_ORGANISATION: &str = "Set by your organisation";

/// Why the policy turned saving off (F-57), for a place that offers no Copy, such as the
/// settings window.
#[must_use]
pub fn saving_off_reason(off: SavingOff) -> &'static str {
    match off {
        SavingOff::ClipboardOnly => "Your organisation allows copying only.",
        SavingOff::FolderNotValid => {
            "Your organisation's save folder is not valid, so saving is off."
        }
    }
}

/// The sentence shown where a refused save sits beside Copy (F-57): the overlay, the
/// thumbnail and the toasts. Why saving is off, and what to do instead.
#[must_use]
pub fn saving_off_notice(off: SavingOff) -> String {
    format!("{} Use Copy.", saving_off_reason(off))
}

/// F-22: the notice once the Save As dialog closes. `chosen` is the confirmed path, or why the
/// dialog failed; `None` is a cancel, which saves nothing and shows nothing.
#[must_use]
pub fn save_as_notice(
    chosen: Option<Result<PathBuf, DeliveryError>>,
    save_to: impl FnOnce(&Path) -> Result<Delivered, DeliveryError>,
) -> Option<String> {
    let saved = chosen?.and_then(|path| save_to(&path));
    Some(delivery_notice(&saved))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f40_a_delivery_notice_names_the_file_folder_or_name_it_is_about() {
        let notice = |outcome| delivery_notice(&outcome);
        let saved = PathBuf::from(r"C:\Shots\Screenshot 2026-10-30 101500.png");
        assert_eq!(
            notice(Ok(Delivered::Saved(saved.clone()))),
            format!("Saved to {}", saved.display())
        );
        let folder = PathBuf::from(r"D:\Full");
        let not_written = notice(Err(DeliveryError::NotWritten {
            folder: folder.clone(),
        }));
        assert_eq!(
            not_written,
            format!("Could not write to {}", folder.display())
        );
        let configured = "Pictures".to_owned();
        let relative = DeliveryError::SaveFolderNotFull {
            configured: configured.clone(),
        };
        assert!(notice(Err(relative)).contains("\"Pictures\""));
        let name = "Screenshot 2026-10-30 101500".to_owned();
        let taken = DeliveryError::NoFreeName { name: name.clone() };
        assert!(notice(Err(taken)).contains(&name));
        let large = DeliveryError::TooLarge {
            width: 70000,
            height: 9,
        };
        assert!(notice(Err(large)).contains("70000x9"));
    }

    #[test]
    fn d120_an_image_that_could_not_be_made_says_so_and_to_try_again() {
        assert_eq!(
            delivery_notice(&Err(DeliveryError::ImageNotMade)),
            "The image could not be made. Try again."
        );
    }

    #[test]
    fn f57_a_refused_save_reads_the_same_wherever_it_is_shown() {
        for off in [SavingOff::ClipboardOnly, SavingOff::FolderNotValid] {
            let refused: Result<Delivered, DeliveryError> = Err(off.into());
            assert_eq!(delivery_notice(&refused), saving_off_notice(off));
            assert!(
                !saving_off_reason(off).contains("Copy"),
                "the settings window offers no Copy"
            );
        }
    }

    #[test]
    fn f57_a_policy_refusal_says_why_and_what_to_do() {
        let notice = |error| delivery_notice(&Err(error));
        assert_eq!(
            notice(DeliveryError::SavingOff(SavingOff::ClipboardOnly)),
            "Your organisation allows copying only. Use Copy."
        );
        assert_eq!(
            notice(DeliveryError::SavingOff(SavingOff::FolderNotValid)),
            "Your organisation's save folder is not valid, so saving is off. Use Copy."
        );
        assert_eq!(
            notice(DeliveryError::OutsidePolicyFolder {
                folder: r"\\fileserver\screenshots".to_owned()
            }),
            r"Your organisation saves screenshots only in \\fileserver\screenshots. Choose a file there."
        );
    }

    #[test]
    fn f22_save_as_saves_to_the_confirmed_path_and_a_cancel_shows_nothing() {
        let path = PathBuf::from(r"C:\Shots\Mine.png");
        let saved = save_as_notice(Some(Ok(path.clone())), |to| {
            Ok(Delivered::Saved(to.with_extension("jpg")))
        });
        let as_saved = path.with_extension("jpg");
        assert_eq!(saved, Some(format!("Saved to {}", as_saved.display())));
        let refuse = |_: &Path| -> Result<Delivered, DeliveryError> { panic!("nothing to save") };
        assert_eq!(save_as_notice(None, refuse), None);
        assert_eq!(
            save_as_notice(Some(Err(DeliveryError::NoSaveDialog)), refuse),
            Some("The Save As dialog did not open".to_owned())
        );
    }
}
