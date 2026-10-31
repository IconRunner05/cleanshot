use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

const FILE_NAME: &str = "cleanshot.log";

/// Why the log could not be opened or written.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("the log file could not be created")]
    Open(#[source] std::io::Error),
    #[error("the log file could not be written")]
    Write(#[source] std::io::Error),
    #[error("Windows did not say when this process started")]
    Clock(#[source] windows::core::Error),
}

/// C-23: the log `--log` turns on, `cleanshot.log` beside the settings file, emptied at each
/// start. Each line is the milliseconds since this process started, then the event.
#[derive(Debug)]
pub struct Log {
    file: File,
}

impl Log {
    /// Creates or empties the log beside `settings`.
    ///
    /// # Errors
    ///
    /// [`LogError::Open`] if the folder or file cannot be created.
    pub fn open(settings: &Path) -> Result<Self, LogError> {
        let path = settings.with_file_name(FILE_NAME);
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder).map_err(LogError::Open)?;
        }
        Ok(Self {
            file: File::create(path).map_err(LogError::Open)?,
        })
    }

    /// Appends `event` as one line. The caller keeps paths and pixels out of it (C-23).
    ///
    /// # Errors
    ///
    /// [`LogError::Clock`] if the process start time cannot be read, [`LogError::Write`] if
    /// the line cannot be written.
    pub fn write(&self, event: &str) -> Result<(), LogError> {
        let at = since_start()?.as_millis();
        writeln!(&self.file, "{at} {event}").map_err(LogError::Write)
    }
}

/// How long ago Windows created this process, which counts the loader's work as well as
/// `main`'s (P-03).
///
/// # Errors
///
/// [`LogError::Clock`] if Windows does not give the process's times.
pub fn since_start() -> Result<Duration, LogError> {
    let (mut created, mut exited, mut kernel, mut user) = Default::default();
    // SAFETY: the pseudo-handle needs no closing, and each output is a writable local.
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    }
    .map_err(LogError::Clock)?;
    // SAFETY: a read of the system clock.
    let now = unsafe { GetSystemTimePreciseAsFileTime() };
    let ticks = ticks(now).saturating_sub(ticks(created));
    Ok(Duration::from_nanos(ticks.saturating_mul(100)))
}

/// A FILETIME's count of 100-nanosecond ticks.
fn ticks(time: FILETIME) -> u64 {
    u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c23_the_log_starts_empty_and_holds_one_timed_line_per_event() {
        let folder = std::env::temp_dir().join(format!("cleanshot-log-{}", std::process::id()));
        let settings = folder.join("cleanshot.toml");
        fs::create_dir_all(&folder).expect("the test folder is created");
        fs::write(folder.join(FILE_NAME), "left from the last run\n").expect("an old log");
        let log = Log::open(&settings).expect("the log opens");
        log.write("perf P-03 tray_ready_ms=1")
            .expect("a line is written");
        log.write("perf P-04 working_set_bytes=2")
            .expect("a line is written");
        drop(log);
        let written = fs::read_to_string(folder.join(FILE_NAME)).expect("the log reads back");
        fs::remove_dir_all(&folder).expect("the test folder is removed");
        let events: Vec<&str> = written
            .lines()
            .map(|line| {
                let (at, event) = line.split_once(' ').expect("a time, then the event");
                at.parse::<u64>().expect("the time is whole milliseconds");
                event
            })
            .collect();
        assert_eq!(
            events,
            ["perf P-03 tray_ready_ms=1", "perf P-04 working_set_bytes=2"]
        );
    }
}
