use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Shell::{
    FOLDERID_Screenshots, KF_FLAG_CREATE, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath,
};
use windows::core::GUID;

use crate::log::LogFailure;

const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];
const MOST_COLLISIONS: u32 = 10_000;

/// A local wall-clock moment, the only input a file-name template reads besides `{n}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

impl Stamp {
    #[must_use]
    pub fn now() -> Self {
        // SAFETY: GetLocalTime only fills the returned struct.
        let time = unsafe { GetLocalTime() };
        Self {
            year: time.wYear,
            month: time.wMonth,
            day: time.wDay,
            hour: time.wHour,
            minute: time.wMinute,
            second: time.wSecond,
        }
    }
}

/// Why a file was not saved.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    /// C-15: the template holds something other than text and `{date}`, `{time}`, `{n}`.
    #[error("the file-name template uses {0}, which is not one of {{date}}, {{time}}, {{n}}")]
    UnknownField(String),
    /// C-15: the expanded name is empty, reserved, or could leave the folder.
    #[error("{0:?} is not a safe file name")]
    UnsafeName(String),
    /// C-15: every numbered name up to the limit is taken.
    #[error("every name from {0:?} to its -{MOST_COLLISIONS} form is taken")]
    NoFreeName(String),
    #[error("the file could not be written")]
    Write(#[source] std::io::Error),
}

/// Why there is no folder to save in.
#[derive(Debug, thiserror::Error)]
pub enum FolderError {
    #[error("Windows could not name or create the screenshots folder")]
    NoFolder(#[source] windows::core::Error),
    /// The configured save folder is not a full path, so it would depend on the working folder.
    #[error("the save folder {0:?} is not a full path")]
    RelativeFolder(String),
}

/// The folder to save in: `configured` (the `save_folder` setting) when it is a full path,
/// otherwise Pictures\Screenshots, created if it does not exist yet (F-21's default).
///
/// # Errors
///
/// [`FolderError::RelativeFolder`] for a configured path that is not full, or
/// [`FolderError::NoFolder`] if the shell cannot name or create the default.
pub fn save_folder(configured: Option<&str>) -> Result<PathBuf, FolderError> {
    match configured {
        Some(folder) if Path::new(folder).is_absolute() => Ok(PathBuf::from(folder)),
        Some(folder) => Err(FolderError::RelativeFolder(folder.to_owned())),
        None => known_folder(FOLDERID_Screenshots, KF_FLAG_CREATE).map_err(FolderError::NoFolder),
    }
}

/// The path of a shell known folder.
pub(crate) fn known_folder(id: GUID, flags: KNOWN_FOLDER_FLAG) -> windows::core::Result<PathBuf> {
    // SAFETY: the shell returns a NUL-terminated path in memory it allocated, read once and
    // freed here.
    unsafe {
        let path = SHGetKnownFolderPath(&raw const id, flags, None)?;
        let folder = OsString::from_wide(path.as_wide());
        CoTaskMemFree(Some(path.0.cast_const().cast()));
        Ok(PathBuf::from(folder))
    }
}

/// Writes `bytes` to `folder` under a name built from `template` and `extension`, never
/// replacing an existing file (C-15), and returns the path written. The file appears whole
/// or not at all (F-21). A partial file that cannot be removed goes to `log`.
///
/// # Errors
///
/// The [`SaveError`] that stopped it.
pub fn save(
    folder: &Path,
    template: &str,
    stamp: Stamp,
    extension: &str,
    bytes: &[u8],
    log: LogFailure,
) -> Result<PathBuf, SaveError> {
    let partial = write_partial(
        folder,
        &offered_name(template, stamp, extension)?,
        bytes,
        log,
    )?;
    let published = publish(folder, template, stamp, extension, &partial);
    remove_partial(&partial, log);
    published
}

/// The first file name `template` gives for `stamp` and `extension`, as Save offers it and
/// Save As suggests it (F-22).
///
/// # Errors
///
/// [`SaveError::UnknownField`] or [`SaveError::UnsafeName`] for a template C-15 refuses.
pub fn offered_name(template: &str, stamp: Stamp, extension: &str) -> Result<String, SaveError> {
    Ok(format!("{}.{extension}", file_name(template, stamp, 1)?))
}

/// F-57 (D-86): whether `path` is in `folder` or a folder under it, comparing the full paths
/// Windows makes of both (`GetFullPathNameW`) one component at a time, ignoring case. The
/// check is on the path as written; a junction inside the folder is not followed. A path
/// Windows cannot make full is not in the folder.
#[must_use]
pub fn is_inside(path: &Path, folder: &Path) -> bool {
    let (Ok(path), Ok(folder)) = (std::path::absolute(path), std::path::absolute(folder)) else {
        return false;
    };
    let lowered = |path: &Path| -> Vec<String> {
        path.components()
            .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    let (path, folder) = (lowered(&path), lowered(&folder));
    path.len() > folder.len() && path.starts_with(&folder)
}

/// F-22: writes `bytes` to `path`, replacing the file there, which the user has confirmed.
/// The file appears whole or not at all: the bytes go to a partial file in the same folder,
/// which `fs::rename` (`MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`) moves over `path`. No
/// partial file is left behind, whether or not it works (C-21); one that cannot be removed
/// goes to `log`.
///
/// # Errors
///
/// [`SaveError::UnsafeName`] for a path with no folder or file name, or
/// [`SaveError::Write`] if the file could not be written or moved into place.
pub fn replace(path: &Path, bytes: &[u8], log: LogFailure) -> Result<(), SaveError> {
    let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(SaveError::UnsafeName(path.display().to_string()));
    };
    let partial = write_partial(folder, &name.to_string_lossy(), bytes, log)?;
    fs::rename(&partial, path).map_err(|error| {
        remove_partial(&partial, log);
        SaveError::Write(error)
    })
}

fn remove_partial(partial: &Path, log: LogFailure) {
    if let Err(error) = fs::remove_file(partial) {
        log("partial_file not_removed", &error);
    }
}

fn write_partial(
    folder: &Path,
    name: &str,
    bytes: &[u8],
    log: LogFailure,
) -> Result<PathBuf, SaveError> {
    for attempt in 1..=MOST_COLLISIONS {
        let partial = folder.join(format!("{name}.partial{attempt}"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)
        {
            Ok(mut file) => {
                return match file.write_all(bytes).and_then(|()| file.sync_all()) {
                    Ok(()) => Ok(partial),
                    Err(error) => {
                        drop(file);
                        remove_partial(&partial, log);
                        Err(SaveError::Write(error))
                    }
                };
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(SaveError::Write(error)),
        }
    }
    Err(SaveError::NoFreeName(name.to_owned()))
}

/// Links the finished partial file under the first free name. A hard link fails if the name
/// exists, so this never replaces a file, even one created after the name was checked.
fn publish(
    folder: &Path,
    template: &str,
    stamp: Stamp,
    extension: &str,
    partial: &Path,
) -> Result<PathBuf, SaveError> {
    let numbered = template.contains("{n}");
    let first = file_name(template, stamp, 1)?;
    for n in 1..=MOST_COLLISIONS {
        let stem = match (numbered, n) {
            (true, _) => file_name(template, stamp, n)?,
            (false, 1) => first.clone(),
            (false, _) => format!("{first}-{n}"),
        };
        let target = folder.join(format!("{stem}.{extension}"));
        match fs::hard_link(partial, &target) {
            Ok(()) => return Ok(target),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(SaveError::Write(error)),
        }
    }
    Err(SaveError::NoFreeName(first))
}

/// Expands `template` for `stamp` and collision number `n`, and checks the result is a
/// plain file name (C-15).
fn file_name(template: &str, stamp: Stamp, n: u32) -> Result<String, SaveError> {
    let mut name = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        name.push_str(&rest[..open]);
        let close = rest[open..]
            .find('}')
            .map(|close| open + close)
            .ok_or_else(|| SaveError::UnknownField(rest[open..].to_owned()))?;
        let field = match &rest[open..=close] {
            "{date}" => format!("{:04}-{:02}-{:02}", stamp.year, stamp.month, stamp.day),
            "{time}" => format!("{:02}.{:02}.{:02}", stamp.hour, stamp.minute, stamp.second),
            "{n}" => n.to_string(),
            unknown => return Err(SaveError::UnknownField(unknown.to_owned())),
        };
        name.push_str(&field);
        rest = &rest[close + 1..];
    }
    name.push_str(rest);
    if is_safe(&name) {
        Ok(name)
    } else {
        Err(SaveError::UnsafeName(name))
    }
}

fn is_safe(name: &str) -> bool {
    let device = name.split('.').next().unwrap_or(name).trim_end();
    !name.trim().is_empty()
        && !name.ends_with(['.', ' '])
        && !name.contains("..")
        && !name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        && !name.chars().any(char::is_control)
        && !RESERVED_NAMES
            .iter()
            .any(|reserved| device.eq_ignore_ascii_case(reserved))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// winnt.h's `FILE_SHARE_READ`: others may read the file, not delete or rename over it.
    const FILE_SHARE_READ: u32 = 1;

    const STAMP: Stamp = Stamp {
        year: 2026,
        month: 10,
        day: 30,
        hour: 9,
        minute: 5,
        second: 7,
    };

    fn empty_folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("cleanshot-{name}-{}", std::process::id()));
        if folder.exists() {
            fs::remove_dir_all(&folder).expect("a stale test folder is removable");
        }
        fs::create_dir(&folder).expect("the test folder is created");
        folder
    }

    #[test]
    fn f57_a_save_as_path_is_inside_the_policy_folder_only_under_it_whatever_its_case() {
        let cases = [
            (r"C:\Shots\a.png", r"C:\Shots", true),
            (r"c:\shots\Team\a.png", r"C:\Shots", true),
            (r"C:\Shots\..\a.png", r"C:\Shots", false),
            (r"C:\Shots-other\a.png", r"C:\Shots", false),
            (r"C:\Shots", r"C:\Shots", false),
            (
                r"\\fileserver\SCREENSHOTS\a.png",
                r"\\fileserver\screenshots",
                true,
            ),
            (
                r"\\fileserver\screenshots\team\a.png",
                r"\\fileserver\screenshots",
                true,
            ),
            (
                r"\\fileserver\screenshots-other\a.png",
                r"\\fileserver\screenshots",
                false,
            ),
        ];
        for (path, folder, inside) in cases {
            assert_eq!(
                is_inside(Path::new(path), Path::new(folder)),
                inside,
                "{path} in {folder}"
            );
        }
    }

    #[test]
    fn c15_the_default_template_names_the_date_and_time_without_colons() {
        let name = file_name("Screenshot {date} {time}", STAMP, 1).expect("a safe name");
        assert_eq!(name, "Screenshot 2026-10-30 09.05.07");
    }

    #[test]
    fn c15_unsafe_templates_are_refused() {
        for template in [
            "..\\{date}",
            "a/b",
            "CON",
            "nul.txt",
            "x:stream",
            "{user}",
            "{date",
            " ",
        ] {
            assert!(
                file_name(template, STAMP, 1).is_err(),
                "{template:?} was accepted"
            );
        }
    }

    #[test]
    fn f21_never_overwrites_existing_file() {
        let folder = empty_folder("collide");
        let first = save(&folder, "Shot", STAMP, "png", b"first", &|_, _| {}).expect("first save");
        let second =
            save(&folder, "Shot", STAMP, "png", b"second", &|_, _| {}).expect("second save");
        let third = save(&folder, "Shot", STAMP, "png", b"third", &|_, _| {}).expect("third save");
        let names: Vec<_> = [&first, &second, &third]
            .map(|path| {
                path.file_name()
                    .expect("a file")
                    .to_string_lossy()
                    .into_owned()
            })
            .into();
        assert_eq!(names, ["Shot.png", "Shot-2.png", "Shot-3.png"]);
        assert_eq!(fs::read(&first).expect("readable"), b"first");
        assert_eq!(
            fs::read_dir(&folder).expect("listable").count(),
            3,
            "a partial file remains"
        );
        fs::remove_dir_all(&folder).expect("the test folder is removable");
    }

    #[test]
    fn f22_replacing_a_file_leaves_only_the_new_file() {
        let folder = empty_folder("replace");
        let path = folder.join("Shot.png");
        fs::write(&path, b"old").expect("the original is written");
        replace(&path, b"new", &|_, _| {}).expect("the file is replaced");
        assert_eq!(fs::read(&path).expect("readable"), b"new");
        assert_eq!(
            fs::read_dir(&folder).expect("listable").count(),
            1,
            "a partial file remains"
        );
        fs::remove_dir_all(&folder).expect("the test folder is removable");
    }

    #[test]
    fn f22_a_replace_that_fails_keeps_the_original_and_leaves_no_partial() {
        use std::os::windows::fs::OpenOptionsExt;

        let folder = empty_folder("replace-held");
        let path = folder.join("Shot.png");
        fs::write(&path, b"old").expect("the original is written");
        let held = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .expect("the original opens without delete sharing");
        assert!(matches!(
            replace(&path, b"new", &|_, _| {}),
            Err(SaveError::Write(_))
        ));
        drop(held);
        assert_eq!(fs::read(&path).expect("readable"), b"old");
        assert_eq!(
            fs::read_dir(&folder).expect("listable").count(),
            1,
            "a partial file remains"
        );
        fs::remove_dir_all(&folder).expect("the test folder is removable");
    }

    #[test]
    fn f21_a_numbered_template_counts_up_instead_of_appending() {
        let folder = empty_folder("numbered");
        save(&folder, "Shot {n}", STAMP, "png", b"a", &|_, _| {}).expect("first save");
        let second =
            save(&folder, "Shot {n}", STAMP, "png", b"b", &|_, _| {}).expect("second save");
        assert_eq!(second.file_name().expect("a file"), "Shot 2.png");
        fs::remove_dir_all(&folder).expect("the test folder is removable");
    }

    #[test]
    fn c15_a_save_folder_that_is_not_a_full_path_is_refused() {
        assert!(matches!(
            save_folder(Some("Screenshots")),
            Err(FolderError::RelativeFolder(_))
        ));
        assert!(matches!(
            save_folder(Some("..\\elsewhere")),
            Err(FolderError::RelativeFolder(_))
        ));
        assert_eq!(
            save_folder(Some("D:\\Shots")).expect("a full path is kept"),
            PathBuf::from("D:\\Shots")
        );
    }
}
