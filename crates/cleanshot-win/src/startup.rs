use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::{HSTRING, PCWSTR, w};

use cleanshot_core::StartupEntryError;

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
    read(RUN_KEY, RUN_VALUE)
}

/// Makes `command` Cleanshot's start-with-Windows entry, replacing any there (F-56).
///
/// # Errors
///
/// [`StartupEntryError::Refused`] if Windows does not write it.
pub fn set_run_value(command: &str) -> Result<(), StartupEntryError> {
    write(RUN_KEY, RUN_VALUE, command)
}

/// Removes Cleanshot's start-with-Windows entry; no entry is not an error (F-56).
///
/// # Errors
///
/// [`StartupEntryError::Refused`] if the entry exists and Windows does not remove it.
pub fn remove_run_value() -> Result<(), StartupEntryError> {
    remove(RUN_KEY, RUN_VALUE)
}

fn read(key: PCWSTR, value: PCWSTR) -> Result<Option<String>, StartupEntryError> {
    let mut bytes = 0u32;
    // SAFETY: with no data buffer, RegGetValueW only writes the size it needs to `bytes`.
    let sized = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&raw mut bytes),
        )
    };
    match sized {
        ERROR_SUCCESS => {}
        ERROR_FILE_NOT_FOUND => return Ok(None),
        refused => return Err(entry_error(refused)),
    }
    let mut text = vec![0u16; usize::try_from(bytes).expect("a registry value fits in memory") / 2];
    // SAFETY: `text` is writable for `bytes` bytes, the size Windows asked for.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            RRF_RT_REG_SZ,
            None,
            Some(text.as_mut_ptr().cast()),
            Some(&raw mut bytes),
        )
    };
    match read {
        ERROR_SUCCESS => {}
        ERROR_FILE_NOT_FOUND => return Ok(None),
        refused => return Err(entry_error(refused)),
    }
    // RegGetValueW counts and writes the terminating NUL; the text ends before it.
    let written = usize::try_from(bytes).expect("a registry value fits in memory") / 2;
    let end = text[..written]
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(written);
    Ok(Some(String::from_utf16_lossy(&text[..end])))
}

fn write(key: PCWSTR, value: PCWSTR, command: &str) -> Result<(), StartupEntryError> {
    let text = HSTRING::from(command);
    let with_nul = text.len() + 1;
    let bytes = u32::try_from(with_nul * 2).expect("a command is shorter than 4 GB");
    // SAFETY: `text` is NUL-terminated and lives past the call, which reads `bytes` bytes of it.
    let written = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            REG_SZ.0,
            Some(text.as_ptr().cast()),
            bytes,
        )
    };
    match written {
        ERROR_SUCCESS => Ok(()),
        refused => Err(entry_error(refused)),
    }
}

fn remove(key: PCWSTR, value: PCWSTR) -> Result<(), StartupEntryError> {
    // SAFETY: both names are NUL-terminated strings that live past the call.
    match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key, value) } {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        refused => Err(entry_error(refused)),
    }
}

fn entry_error(code: WIN32_ERROR) -> StartupEntryError {
    StartupEntryError::Refused {
        code: code.to_hresult().0.cast_unsigned(),
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::System::Registry::RegDeleteKeyW;

    use super::*;

    #[test]
    fn f56_an_entry_reads_back_as_written_and_reads_as_none_once_removed() {
        let name = HSTRING::from(format!("Software\\Cleanshot test {}", std::process::id()));
        let key = PCWSTR(name.as_ptr());
        let command = "\"C:\\Program Files\\Cleanshot\\cleanshot.exe\"";

        assert_eq!(read(key, RUN_VALUE).expect("an absent entry reads"), None);
        write(key, RUN_VALUE, command).expect("written");
        assert_eq!(
            read(key, RUN_VALUE).expect("readable").as_deref(),
            Some(command)
        );
        remove(key, RUN_VALUE).expect("removed");
        assert_eq!(read(key, RUN_VALUE).expect("readable"), None);
        remove(key, RUN_VALUE).expect("removing no entry is not an error");

        // SAFETY: the key is this test's own, made by `write` above.
        let deleted = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, key) };
        assert_eq!(deleted, ERROR_SUCCESS, "the test key is removable");
    }
}
