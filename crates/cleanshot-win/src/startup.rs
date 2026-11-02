use windows::Win32::Foundation::WIN32_ERROR;
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::core::{PCWSTR, w};

use cleanshot_core::StartupEntryError;

use crate::registry::{read_text, remove, write_text};

// F-56: only this per-user value; never HKLM, and never StartupApproved, which is the user's.
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Cleanshot");

/// The command in Cleanshot's start-with-Windows entry, or None when there is no entry
/// (F-56).
///
/// # Errors
///
/// [`StartupEntryError::Refused`] if the entry exists but cannot be read.
pub fn run_value() -> Result<Option<String>, StartupEntryError> {
    read_text(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE).map_err(entry_error)
}

/// Makes `command` Cleanshot's start-with-Windows entry, replacing any there (F-56).
///
/// # Errors
///
/// [`StartupEntryError::Refused`] if Windows does not write it.
pub fn set_run_value(command: &str) -> Result<(), StartupEntryError> {
    write_text(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, command).map_err(entry_error)
}

/// Removes Cleanshot's start-with-Windows entry; no entry is not an error (F-56).
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
