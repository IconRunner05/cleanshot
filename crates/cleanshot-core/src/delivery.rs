use std::path::PathBuf;

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
}

/// The sentence the user sees after a Copy or Save, from the overlay or the editor (D-48): what
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
    }
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
}
