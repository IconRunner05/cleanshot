use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::windows::ffi::OsStringExt;
use std::path::{Component, Path, PathBuf, Prefix};

use windows::Win32::Storage::FileSystem::{MOVE_FILE_FLAGS, MoveFileExW};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Shell::{
    FOLDERID_Screenshots, KF_FLAG_CREATE, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath,
};
use windows::core::{GUID, HSTRING};

use crate::log::LogFailure;

const RESERVED_NAMES: [&str; 28] = [
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
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

/// A file-name template, text with only `{date}`, `{time}` and `{n}` fields in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    Text(String),
    Date,
    Time,
    Number,
}

/// The template holds something other than text and `{date}`, `{time}`, `{n}`.
#[derive(Debug, thiserror::Error)]
#[error("the file-name template uses {0}, which is not one of {{date}}, {{time}}, {{n}}")]
pub struct UnknownField(pub String);

impl Template {
    /// Reads `text` as a file-name template.
    ///
    /// # Errors
    ///
    /// [`UnknownField`] with the first field that is not `{date}`, `{time}` or `{n}`, or an
    /// unclosed `{`.
    pub fn parse(text: &str) -> Result<Self, UnknownField> {
        let mut parts = Vec::new();
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            if open > 0 {
                parts.push(Part::Text(rest[..open].to_owned()));
            }
            let close = rest[open..]
                .find('}')
                .map(|close| open + close)
                .ok_or_else(|| UnknownField(rest[open..].to_owned()))?;
            parts.push(match &rest[open..=close] {
                "{date}" => Part::Date,
                "{time}" => Part::Time,
                "{n}" => Part::Number,
                unknown => return Err(UnknownField(unknown.to_owned())),
            });
            rest = &rest[close + 1..];
        }
        if !rest.is_empty() {
            parts.push(Part::Text(rest.to_owned()));
        }
        Ok(Self { parts })
    }

    fn numbered(&self) -> bool {
        self.parts.contains(&Part::Number)
    }

    fn expand(&self, stamp: Stamp, n: u32) -> String {
        self.parts
            .iter()
            .map(|part| match part {
                Part::Text(text) => text.clone(),
                Part::Date => format!("{:04}-{:02}-{:02}", stamp.year, stamp.month, stamp.day),
                Part::Time => {
                    format!("{:02}.{:02}.{:02}", stamp.hour, stamp.minute, stamp.second)
                }
                Part::Number => n.to_string(),
            })
            .collect()
    }
}

/// Why a file was not saved.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    /// The expanded name is empty, reserved, or could leave the folder.
    #[error("{0:?} is not a safe file name")]
    UnsafeName(String),
    /// Every numbered name up to the limit is taken.
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
/// otherwise Pictures\Screenshots, created if it does not exist yet.
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
/// replacing an existing file, and returns the path written. The file appears whole
/// or not at all. A partial file that cannot be removed goes to `log`.
///
/// # Errors
///
/// The [`SaveError`] that stopped it.
pub fn save(
    folder: &Path,
    template: &Template,
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
    if published.is_err() {
        remove_partial(&partial, log);
    }
    published
}

/// The first file name `template` gives for `stamp` and `extension`, as Save offers it and
/// Save As suggests it.
///
/// # Errors
///
/// [`SaveError::UnsafeName`] for a name that is refused.
pub fn offered_name(
    template: &Template,
    stamp: Stamp,
    extension: &str,
) -> Result<String, SaveError> {
    Ok(format!("{}.{extension}", file_name(template, stamp, 1)?))
}

/// Whether `path` is in `folder` or a folder under it, comparing the full paths
/// Windows makes of both (`GetFullPathNameW`) one component at a time, ignoring case by Unicode
/// lowercasing, which folds a few letters differently from NTFS. The check is on the path as
/// written; a junction inside the folder is not followed. A path
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

/// Writes `bytes` to `path`, replacing the file there, which the user has confirmed.
/// The file appears whole or not at all: the bytes go to a partial file in the same folder,
/// which `fs::rename` (`MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`) moves over `path`. No
/// partial file is left behind, whether or not it works; one that cannot be removed
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

/// Moves the finished partial file to the first free name. The move fails if the name exists,
/// so this never replaces a file, even one created after the name was checked.
fn publish(
    folder: &Path,
    template: &Template,
    stamp: Stamp,
    extension: &str,
    partial: &Path,
) -> Result<PathBuf, SaveError> {
    let numbered = template.numbered();
    let first = file_name(template, stamp, 1)?;
    for n in 1..=MOST_COLLISIONS {
        let stem = match (numbered, n) {
            (true, _) => file_name(template, stamp, n)?,
            (false, 1) => first.clone(),
            (false, _) => format!("{first}-{n}"),
        };
        let target = folder.join(format!("{stem}.{extension}"));
        match move_without_replacing(partial, &target) {
            Ok(()) => return Ok(target),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(SaveError::Write(error)),
        }
    }
    Err(SaveError::NoFreeName(first))
}

// A hard link would also never replace a file, but FAT and exFAT have none.
fn move_without_replacing(from: &Path, to: &Path) -> io::Result<()> {
    let (from, to) = (
        HSTRING::from(long_form(from)?.as_os_str()),
        HSTRING::from(long_form(to)?.as_os_str()),
    );
    // SAFETY: both strings outlive the call, and no flag asks to replace or copy.
    unsafe { MoveFileExW(&from, &to, MOVE_FILE_FLAGS(0)) }
        // The low 16 bits of a Win32 failure's HRESULT are the code io::ErrorKind reads.
        .map_err(|error| io::Error::from_raw_os_error(error.code().0 & 0xFFFF))
}

/// The `\\?\` form of `path`, which `MoveFileExW` needs past `MAX_PATH` and `std::fs` adds itself.
fn long_form(path: &Path) -> io::Result<OsString> {
    let full = std::path::absolute(path)?;
    let mut components = full.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Ok(full.into_os_string());
    };
    let mut long = OsString::from(r"\\?\");
    match prefix.kind() {
        Prefix::Disk(_) => long.push(&full),
        Prefix::UNC(server, share) => {
            long.push(r"UNC\");
            long.push(server);
            long.push(r"\");
            long.push(share);
            long.push(components.as_path());
        }
        _ => return Ok(full.into_os_string()),
    }
    Ok(long)
}

/// Expands `template` for `stamp` and collision number `n`, and checks the result is a
/// plain file name.
fn file_name(template: &Template, stamp: Stamp, n: u32) -> Result<String, SaveError> {
    let name = template.expand(stamp, n);
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
    fn a_save_as_path_is_inside_the_policy_folder_only_under_it_whatever_its_case() {
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
    fn the_default_template_names_the_date_and_time_without_colons() {
        let template = Template::parse("Screenshot {date} {time}").expect("known fields");
        let name = file_name(&template, STAMP, 1).expect("a safe name");
        assert_eq!(name, "Screenshot 2026-10-30 09.05.07");
    }

    #[test]
    fn unsafe_templates_are_refused() {
        for template in [
            "..\\{date}",
            "a/b",
            "CON",
            "nul.txt",
            "COM\u{b9}",
            "lpt\u{b3}.png",
            "x:stream",
            "{user}",
            "{date",
            " ",
        ] {
            assert!(
                Template::parse(template)
                    .map_or(true, |template| file_name(&template, STAMP, 1).is_err()),
                "{template:?} was accepted"
            );
        }
    }

    #[test]
    fn never_overwrites_existing_file() {
        let folder = empty_folder("collide");
        let shot = Template::parse("Shot").expect("no fields");
        let log: LogFailure = &|event, error| panic!("{event} was logged: {error}");
        let first = save(&folder, &shot, STAMP, "png", b"first", log).expect("first save");
        let second = save(&folder, &shot, STAMP, "png", b"second", log).expect("second save");
        let third = save(&folder, &shot, STAMP, "png", b"third", log).expect("third save");
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
    fn a_save_reaches_a_folder_past_max_path() {
        let folder = empty_folder("long-path");
        let deep = folder
            .join("a".repeat(120))
            .join("b".repeat(120))
            .join("c".repeat(120));
        fs::create_dir_all(&deep).expect("the deep folder is created");
        let shot = Template::parse("Shot").expect("no fields");
        let path = save(&deep, &shot, STAMP, "png", b"deep", &|_, _| {}).expect("the save");
        assert_eq!(fs::read(&path).expect("readable"), b"deep");
        fs::remove_dir_all(&folder).expect("the test folder is removable");
    }

    #[test]
    fn replacing_a_file_leaves_only_the_new_file() {
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
    fn a_replace_that_fails_keeps_the_original_and_leaves_no_partial() {
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
    fn a_numbered_template_counts_up_instead_of_appending() {
        let folder = empty_folder("numbered");
        let numbered = Template::parse("Shot {n}").expect("a known field");
        save(&folder, &numbered, STAMP, "png", b"a", &|_, _| {}).expect("first save");
        let second = save(&folder, &numbered, STAMP, "png", b"b", &|_, _| {}).expect("second save");
        assert_eq!(second.file_name().expect("a file"), "Shot 2.png");
        fs::remove_dir_all(&folder).expect("the test folder is removable");
    }

    #[test]
    fn a_save_folder_that_is_not_a_full_path_is_refused() {
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
