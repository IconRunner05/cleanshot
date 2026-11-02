use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW,
    RegSetKeyValueW,
};
use windows::core::{HSTRING, PCWSTR};

/// The `REG_DWORD` named `value` under `key`, or None when it does not exist.
pub(crate) fn read_number(
    root: HKEY,
    key: PCWSTR,
    value: PCWSTR,
) -> Result<Option<u32>, WIN32_ERROR> {
    let mut number = 0u32;
    let mut bytes = u32::try_from(size_of::<u32>()).expect("a u32 is 4 bytes");
    // SAFETY: `number` is writable for `bytes` bytes and RRF_RT_REG_DWORD writes exactly 4.
    let read = unsafe {
        RegGetValueW(
            root,
            key,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut number).cast()),
            Some(&raw mut bytes),
        )
    };
    match read {
        ERROR_SUCCESS => Ok(Some(number)),
        ERROR_FILE_NOT_FOUND => Ok(None),
        refused => Err(refused),
    }
}

/// The `REG_SZ` named `value` under `key`, or a `REG_EXPAND_SZ` with its variables expanded;
/// None when it does not exist.
pub(crate) fn read_text(
    root: HKEY,
    key: PCWSTR,
    value: PCWSTR,
) -> Result<Option<String>, WIN32_ERROR> {
    let mut bytes = 0u32;
    // SAFETY: with no data buffer, RegGetValueW only writes the size it needs to `bytes`.
    let sized = unsafe {
        RegGetValueW(
            root,
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
        refused => return Err(refused),
    }
    let mut text = vec![0u16; usize::try_from(bytes).expect("a registry value fits in memory") / 2];
    // SAFETY: `text` is writable for `bytes` bytes, the size Windows asked for.
    let read = unsafe {
        RegGetValueW(
            root,
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
        refused => return Err(refused),
    }
    // RegGetValueW counts and writes the terminating NUL; the text ends before it.
    let written = usize::try_from(bytes).expect("a registry value fits in memory") / 2;
    let end = text[..written]
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(written);
    Ok(Some(String::from_utf16_lossy(&text[..end])))
}

/// Makes `text` the `REG_SZ` named `value` under `key`, creating the key if needed.
pub(crate) fn write_text(
    root: HKEY,
    key: PCWSTR,
    value: PCWSTR,
    text: &str,
) -> Result<(), WIN32_ERROR> {
    let text = HSTRING::from(text);
    let bytes = u32::try_from((text.len() + 1) * 2).expect("a registry text is shorter than 4 GB");
    // SAFETY: `text` is NUL-terminated and lives past the call, which reads `bytes` bytes of it.
    let written = unsafe {
        RegSetKeyValueW(
            root,
            key,
            value,
            REG_SZ.0,
            Some(text.as_ptr().cast()),
            bytes,
        )
    };
    match written {
        ERROR_SUCCESS => Ok(()),
        refused => Err(refused),
    }
}

/// Removes the value named `value` under `key`; one that does not exist is not an error.
pub(crate) fn remove(root: HKEY, key: PCWSTR, value: PCWSTR) -> Result<(), WIN32_ERROR> {
    // SAFETY: both names are NUL-terminated strings that live past the call.
    match unsafe { RegDeleteKeyValueW(root, key, value) } {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        refused => Err(refused),
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyW};
    use windows::core::w;

    use super::*;

    #[test]
    fn f56_an_entry_reads_back_as_written_and_reads_as_none_once_removed() {
        let name = HSTRING::from(format!("Software\\Cleanshot test {}", std::process::id()));
        let key = PCWSTR(name.as_ptr());
        let value = w!("Cleanshot");
        let command = "\"C:\\Program Files\\Cleanshot\\cleanshot.exe\"";

        assert_eq!(read_text(HKEY_CURRENT_USER, key, value), Ok(None));
        write_text(HKEY_CURRENT_USER, key, value, command).expect("written");
        assert_eq!(
            read_text(HKEY_CURRENT_USER, key, value),
            Ok(Some(command.to_owned()))
        );
        remove(HKEY_CURRENT_USER, key, value).expect("removed");
        assert_eq!(read_text(HKEY_CURRENT_USER, key, value), Ok(None));
        remove(HKEY_CURRENT_USER, key, value).expect("removing no entry is not an error");

        // SAFETY: the key is this test's own, made by `write_text` above.
        let deleted = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, key) };
        assert_eq!(deleted, ERROR_SUCCESS, "the test key is removable");
    }
}
