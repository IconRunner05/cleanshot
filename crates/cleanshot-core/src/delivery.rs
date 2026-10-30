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
