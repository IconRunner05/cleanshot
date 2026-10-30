use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

use windows::Win32::UI::Shell::{FOLDERID_RoamingAppData, KF_FLAG_DEFAULT};

use crate::save::known_folder;

const FILE_NAME: &str = "cleanshot.toml";

/// Why the settings file could not be found or read.
#[derive(Debug, thiserror::Error)]
pub enum SettingsFileError {
    #[error("Windows could not name the roaming application data folder")]
    NoAppData(#[source] windows::core::Error),
    #[error("the running exe's own path is unknown")]
    NoExePath(#[source] std::io::Error),
    #[error("{0} could not be read")]
    Read(PathBuf, #[source] std::io::Error),
}

/// C-05: `cleanshot.toml` next to the exe if one is there (portable mode), otherwise
/// `%APPDATA%\Cleanshot\cleanshot.toml`.
///
/// # Errors
///
/// [`SettingsFileError`] if neither folder can be named.
pub fn settings_path() -> Result<PathBuf, SettingsFileError> {
    let exe = std::env::current_exe().map_err(SettingsFileError::NoExePath)?;
    let portable = exe.with_file_name(FILE_NAME);
    if portable.is_file() {
        return Ok(portable);
    }
    let app_data = known_folder(FOLDERID_RoamingAppData, KF_FLAG_DEFAULT)
        .map_err(SettingsFileError::NoAppData)?;
    Ok(app_data.join("Cleanshot").join(FILE_NAME))
}

/// The file's first `limit + 1` bytes, so a caller can tell an over-long file from one
/// exactly `limit` long (C-13), or None when there is no file yet.
///
/// # Errors
///
/// [`SettingsFileError::Read`] for any failure other than the file not existing.
pub fn read_settings(path: &Path, limit: usize) -> Result<Option<Vec<u8>>, SettingsFileError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(SettingsFileError::Read(path.to_owned(), error)),
    };
    let mut bytes = Vec::new();
    let cap = u64::try_from(limit).map_or(u64::MAX, |limit| limit.saturating_add(1));
    file.take(cap)
        .read_to_end(&mut bytes)
        .map_err(|error| SettingsFileError::Read(path.to_owned(), error))?;
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn f54_a_missing_file_reads_as_none_and_a_present_one_reads_whole() {
        let folder =
            std::env::temp_dir().join(format!("cleanshot-settings-{}", std::process::id()));
        fs::create_dir_all(&folder).expect("the test folder is created");
        let path = folder.join(FILE_NAME);
        assert!(
            read_settings(&path, 100)
                .expect("a missing file is not an error")
                .is_none()
        );
        fs::write(&path, "save_format = \"bmp\"\n").expect("written");
        let read = read_settings(&path, 100)
            .expect("readable")
            .expect("present");
        assert_eq!(read, b"save_format = \"bmp\"\n");
        fs::write(&path, "x".repeat(50)).expect("rewritten");
        let capped = read_settings(&path, 10)
            .expect("readable")
            .expect("present");
        assert_eq!(
            capped.len(),
            11,
            "C-13: the read stops one byte past the limit"
        );
        fs::remove_dir_all(&folder).expect("the test folder is removable");
    }
}
