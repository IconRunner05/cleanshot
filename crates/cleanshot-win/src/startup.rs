use windows::Win32::Foundation::WIN32_ERROR;
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::core::{PCWSTR, w};

use cleanshot_core::StartupEntryError;

use crate::registry::{read_text, remove, write_text};

// Only this per-user value; never HKLM, and never StartupApproved, which is the user's.
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Cleanshot");

/// Why [`exe_path`] has no path to give.
#[derive(Debug, thiserror::Error)]
pub enum ExePathError {
    #[error("Windows did not give this exe's path")]
    Unreadable(#[source] std::io::Error),
    #[error("this exe's path is not Unicode text")]
    NotUnicode,
}

/// This exe's full path, as the start-with-Windows entry's command names it.
///
/// # Errors
///
/// [`ExePathError`] if Windows cannot give the path or it is not Unicode text.
pub fn exe_path() -> Result<String, ExePathError> {
    std::env::current_exe()
        .map_err(ExePathError::Unreadable)?
        .into_os_string()
        .into_string()
        .map_err(|_unlogged_path| ExePathError::NotUnicode)
}

/// The command in Cleanshot's start-with-Windows entry, or None when there is no entry.
///
/// # Errors
///
/// [`StartupEntryError::Refused`] if the entry exists but cannot be read.
pub fn run_value() -> Result<Option<String>, StartupEntryError> {
    read_text(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE).map_err(entry_error)
}

/// Makes `command` Cleanshot's start-with-Windows entry, replacing any there.
///
/// # Errors
///
/// [`StartupEntryError::Refused`] if Windows does not write it.
pub fn set_run_value(command: &str) -> Result<(), StartupEntryError> {
    write_text(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, command).map_err(entry_error)
}

/// Removes Cleanshot's start-with-Windows entry; no entry is not an error.
///
/// # Errors
///
/// [`StartupEntryError::Refused`] if the entry exists and Windows does not remove it.
pub fn remove_run_value() -> Result<(), StartupEntryError> {
    remove(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE).map_err(entry_error)
}

fn entry_error(code: WIN32_ERROR) -> StartupEntryError {
    StartupEntryError::Refused {
        code: code.to_hresult().0.cast_unsigned(),
    }
}
