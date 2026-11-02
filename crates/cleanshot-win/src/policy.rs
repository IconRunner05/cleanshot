use cleanshot_core::{Policy, PolicyProblem, PolicySetting, PolicyValue};
use windows::Win32::Foundation::ERROR_UNSUPPORTED_TYPE;
use windows::Win32::System::Registry::{HKEY, HKEY_LOCAL_MACHINE};
use windows::core::{PCWSTR, w};

use crate::registry::{read_number, read_text};

// D-86: HKLM only, which only an administrator can write; never HKCU.
const POLICY_KEY: PCWSTR = w!("SOFTWARE\\Policies\\Cleanshot");

/// The organisation's policy under `HKLM\SOFTWARE\Policies\Cleanshot`, and each value that was
/// present but not valid, so its strictest setting applies (F-57, D-86).
#[must_use]
pub fn read_policy() -> (Policy, Vec<PolicyProblem>) {
    read_from(HKEY_LOCAL_MACHINE, POLICY_KEY)
}

fn read_from(root: HKEY, key: PCWSTR) -> (Policy, Vec<PolicyProblem>) {
    let value = |setting| read_value(root, key, value_name(setting));
    Policy::from_values(
        value(PolicySetting::SaveFolder),
        value(PolicySetting::ClipboardOnly),
        value(PolicySetting::ForcePng),
        value(PolicySetting::HideSettings),
    )
}

fn value_name(setting: PolicySetting) -> PCWSTR {
    match setting {
        PolicySetting::SaveFolder => w!("SaveFolder"),
        PolicySetting::ClipboardOnly => w!("ClipboardOnly"),
        PolicySetting::ForcePng => w!("ForcePng"),
        PolicySetting::HideSettings => w!("HideSettings"),
    }
}

// A value Windows cannot read is Unreadable, which takes its strictest setting and is
// reported as a PolicyProblem.
fn read_value(root: HKEY, key: PCWSTR, name: PCWSTR) -> PolicyValue {
    match read_number(root, key, name) {
        Ok(None) => PolicyValue::Missing,
        Ok(Some(number)) => PolicyValue::Number(number),
        Err(ERROR_UNSUPPORTED_TYPE) => match read_text(root, key, name) {
            Ok(None) => PolicyValue::Missing,
            Ok(Some(text)) => PolicyValue::Text(text),
            Err(_) => PolicyValue::Unreadable,
        },
        Err(_) => PolicyValue::Unreadable,
    }
}

#[cfg(test)]
mod tests {
    use cleanshot_core::Saving;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_BINARY, REG_DWORD, REG_EXPAND_SZ, REG_VALUE_TYPE, RegDeleteTreeW,
        RegSetKeyValueW,
    };
    use windows::core::HSTRING;

    use super::*;

    fn set(key: PCWSTR, setting: PolicySetting, kind: REG_VALUE_TYPE, bytes: &[u8]) {
        let size = u32::try_from(bytes.len()).expect("a test value is small");
        // SAFETY: `bytes` lives past the call, which reads `size` bytes of it.
        let set = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key,
                value_name(setting),
                kind.0,
                Some(bytes.as_ptr().cast()),
                size,
            )
        };
        assert_eq!(set, ERROR_SUCCESS, "{setting:?} is written");
    }

    fn wide(text: &str) -> Vec<u8> {
        text.encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    #[test]
    fn f57_each_value_is_read_by_its_type_and_an_expandable_folder_is_expanded() {
        let name = HSTRING::from(format!(
            "Software\\Cleanshot policy test {}",
            std::process::id()
        ));
        let key = PCWSTR(name.as_ptr());
        assert_eq!(
            read_from(HKEY_CURRENT_USER, key),
            (Policy::default(), Vec::new())
        );

        set(
            key,
            PolicySetting::SaveFolder,
            REG_EXPAND_SZ,
            &wide("%SystemDrive%\\Shots"),
        );
        set(
            key,
            PolicySetting::ClipboardOnly,
            REG_DWORD,
            &0u32.to_le_bytes(),
        );
        set(key, PolicySetting::ForcePng, REG_DWORD, &1u32.to_le_bytes());
        set(key, PolicySetting::HideSettings, REG_BINARY, &[1]);
        let (policy, problems) = read_from(HKEY_CURRENT_USER, key);

        let drive = std::env::var("SystemDrive").expect("Windows sets SystemDrive");
        assert_eq!(
            policy.saving(),
            &Saving::Allowed {
                folder: Some(format!("{drive}\\Shots")),
                png_only: true,
            }
        );
        assert!(policy.settings_hidden(), "a switch of another type is on");
        assert_eq!(problems, vec![PolicyProblem(PolicySetting::HideSettings)]);

        // SAFETY: the key is this test's own, made by `set` above.
        let deleted = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key) };
        assert_eq!(deleted, ERROR_SUCCESS, "the test key is removable");
    }
}
