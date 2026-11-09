use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

use cleanshot_core::{Settings, SettingsSaveError};
use windows::Win32::UI::Shell::{FOLDERID_RoamingAppData, KF_FLAG_DEFAULT};

use crate::log::LogFailure;
use crate::save::{SaveError, known_folder, replace};

const FILE_NAME: &str = "cleanshot.toml";
const URLZONE_INTERNET: u32 = 3;
const LONGEST_ZONE_STREAM: u64 = 4096;

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

/// Why a `cleanshot.toml` beside the exe was not used for this run.
#[derive(Debug)]
pub enum PortableIgnored {
    FromInternet,
    MarkNotRead(std::io::Error),
    MarkNotUnderstood,
}

/// Which settings file this run uses.
#[derive(Debug)]
pub struct SettingsLocation {
    pub path: PathBuf,
    /// Set when a file beside the exe was passed over, so `path` is the `%APPDATA%` one.
    pub ignored: Option<PortableIgnored>,
}

/// `cleanshot.toml` next to the exe if one is there (portable mode) and no browser marked it
/// as downloaded, otherwise `%APPDATA%\Cleanshot\cleanshot.toml`.
///
/// # Errors
///
/// [`SettingsFileError`] if neither folder can be named.
pub fn settings_location() -> Result<SettingsLocation, SettingsFileError> {
    let exe = std::env::current_exe().map_err(SettingsFileError::NoExePath)?;
    let portable = exe.with_file_name(FILE_NAME);
    let ignored = portable.is_file().then(|| why_ignored(&portable));
    if matches!(ignored, Some(None)) {
        return Ok(SettingsLocation {
            path: portable,
            ignored: None,
        });
    }
    let app_data = known_folder(FOLDERID_RoamingAppData, KF_FLAG_DEFAULT)
        .map_err(SettingsFileError::NoAppData)?;
    Ok(SettingsLocation {
        path: app_data.join("Cleanshot").join(FILE_NAME),
        ignored: ignored.flatten(),
    })
}

/// A browser can drop a file beside an exe in Downloads, and the mark it leaves is what
/// tells that file from one the user put there. Unblock in the file's Properties removes it.
fn why_ignored(portable: &Path) -> Option<PortableIgnored> {
    let mut stream = portable.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    let file = match File::open(&stream) {
        Ok(file) => file,
        // A volume with no streams, such as FAT, refuses the name, and can hold no mark.
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::NotFound | ErrorKind::InvalidFilename
            ) =>
        {
            return None;
        }
        Err(error) => return Some(PortableIgnored::MarkNotRead(error)),
    };
    let mut bytes = Vec::new();
    if let Err(error) = file.take(LONGEST_ZONE_STREAM).read_to_end(&mut bytes) {
        return Some(PortableIgnored::MarkNotRead(error));
    }
    match mark_in(&bytes) {
        Mark::Zone(zone) if zone >= URLZONE_INTERNET => Some(PortableIgnored::FromInternet),
        Mark::Zone(_) | Mark::NoZone => None,
        Mark::Garbled => Some(PortableIgnored::MarkNotUnderstood),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Mark {
    Zone(u32),
    NoZone,
    Garbled,
}

/// Reads the `ZoneId=` line of a `Zone.Identifier` stream. Text that cannot be read as the
/// single-byte form Windows writes counts as garbled, so an unusual mark is never taken for none.
fn mark_in(stream: &[u8]) -> Mark {
    if stream.contains(&0) {
        return Mark::Garbled;
    }
    let zone = String::from_utf8_lossy(stream)
        .lines()
        .filter_map(|line| line.split_once('='))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case("ZoneId"))
        .map(|(_, value)| value.trim().parse());
    match zone {
        Some(Ok(zone)) => Mark::Zone(zone),
        Some(Err(_)) => Mark::Garbled,
        None => Mark::NoZone,
    }
}

/// The file's first `limit + 1` bytes, so a caller can tell an over-long file from one
/// exactly `limit` long, or None when there is no file yet.
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

/// Replaces the file at `path` with `settings`, whole or not at all, creating its folder
/// first. A partial file that cannot be removed goes to `log`.
///
/// # Errors
///
/// [`SettingsSaveError::NotWritten`] with what Windows said, if the folder or the file could not
/// be written.
pub fn write_settings(
    path: &Path,
    settings: &Settings,
    log: LogFailure,
) -> Result<(), SettingsSaveError> {
    let not_written = |kind| SettingsSaveError::NotWritten {
        path: path.to_owned(),
        kind,
    };
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder).map_err(|error| not_written(error.kind()))?;
    }
    replace(path, settings.to_toml().as_bytes(), log).map_err(|error| {
        not_written(match error {
            SaveError::Write(error) => error.kind(),
            SaveError::UnsafeName(_) => ErrorKind::InvalidFilename,
            SaveError::NoFreeName(_) => ErrorKind::Other,
        })
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn a_file_from_the_internet_is_ignored_until_it_is_unblocked() {
        let folder = std::env::temp_dir().join(format!("cleanshot-mark-{}", std::process::id()));
        fs::create_dir_all(&folder).expect("the test folder is created");
        let path = folder.join(FILE_NAME);
        fs::write(&path, "").expect("written");
        let stream = folder.join(format!("{FILE_NAME}:Zone.Identifier"));
        let marked = |text: &str| {
            fs::write(&stream, text).expect("the mark is written");
            why_ignored(&path)
        };
        assert!(why_ignored(&path).is_none(), "a file with no mark is used");
        for zone in [3, 4] {
            assert!(matches!(
                marked(&format!("[ZoneTransfer]\r\nZoneId={zone}\r\n")),
                Some(PortableIgnored::FromInternet)
            ));
        }
        assert!(marked("[ZoneTransfer]\r\nZoneId=2\r\n").is_none());
        assert!(matches!(
            marked("[ZoneTransfer]\r\nZoneId=three\r\n"),
            Some(PortableIgnored::MarkNotUnderstood)
        ));
        fs::remove_file(&stream).expect("the mark is removed, as Unblock does");
        assert!(why_ignored(&path).is_none(), "an unblocked file is used");
        fs::remove_dir_all(&folder).expect("the test folder is removable");
    }

    #[test]
    fn a_mark_not_in_the_expected_form_is_never_read_as_no_mark() {
        assert_eq!(mark_in(b"[ZoneTransfer]\r\nZoneId=3\r\n"), Mark::Zone(3));
        assert_eq!(mark_in(b"[ZoneTransfer]\r\nHostUrl=x\r\n"), Mark::NoZone);
        assert_eq!(mark_in(b""), Mark::NoZone);
        assert_eq!(mark_in(b"Z\0o\0n\0e\0"), Mark::Garbled);
        assert_eq!(mark_in(b"ZoneId=-1"), Mark::Garbled);
    }

    #[test]
    fn a_missing_file_reads_as_none_and_a_present_one_reads_whole() {
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
        assert_eq!(capped.len(), 11, "the read stops one byte past the limit");
        fs::remove_dir_all(&folder).expect("the test folder is removable");
    }

    #[test]
    fn written_settings_create_their_folder_and_read_back() {
        let folder =
            std::env::temp_dir().join(format!("cleanshot-settings-write-{}", std::process::id()));
        let path = folder.join("Cleanshot").join(FILE_NAME);
        let settings = Settings {
            save_folder: Some("D:\\Shots".into()),
            ..Settings::default()
        };
        write_settings(&path, &settings, &|_, _| {})
            .expect("written into a folder that did not exist");
        let bytes = read_settings(&path, cleanshot_core::MAX_SETTINGS_BYTES)
            .expect("readable")
            .expect("present");
        assert_eq!(Settings::from_toml(&bytes), (settings, vec![]));
        fs::remove_dir_all(&folder).expect("the test folder is removable");
    }
}
