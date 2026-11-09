/// The command Windows runs at sign-in to start the exe at `exe`: its full path in
/// double quotes, so a path with spaces cannot run another program, and no arguments.
#[must_use]
pub fn run_command(exe: &str) -> String {
    format!("\"{exe}\"")
}

/// Whether the Run value `command` starts the exe at `exe`, comparing paths as Windows does,
/// ignoring case. A value naming another copy, or not written by [`run_command`], does not.
#[must_use]
pub fn starts(command: &str, exe: &str) -> bool {
    command
        .strip_prefix('"')
        .and_then(|command| command.strip_suffix('"'))
        .is_some_and(|path| path.to_lowercase() == exe.to_lowercase())
}

/// Why Windows' startup list did not change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StartupEntryError {
    #[error("Cleanshot could not read its own path")]
    NoExePath,
    #[error("Windows refused the change to the Run value: HRESULT {code:#010x}")]
    Refused { code: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_value_quotes_the_exe_and_starts_only_that_exe() {
        let exe = r"D:\Program Tools\Cleanshot\cleanshot.exe";
        let command = run_command(exe);
        assert_eq!(command, r#""D:\Program Tools\Cleanshot\cleanshot.exe""#);
        assert!(starts(&command, exe));
        assert!(starts(
            &command,
            r"d:\program tools\cleanshot\CLEANSHOT.EXE"
        ));
        assert!(!starts(&command, r"E:\Cleanshot\cleanshot.exe"));
        assert!(!starts(exe, exe));
        assert!(!starts(&format!("{command} --hidden"), exe));
        assert!(starts(
            &run_command(r"\\fileserver\tools\cleanshot.exe"),
            r"\\fileserver\tools\cleanshot.exe"
        ));
    }
}
