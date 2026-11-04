pub mod memory;

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2,
};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

const FILE_NAME: &str = "cleanshot.log";
#[expect(
    clippy::cast_possible_truncation,
    reason = "the counters are a few dozen bytes"
)]
const COUNTERS_SIZE: u32 = size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;

/// Why the log could not be opened or written.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("the log file could not be created")]
    Open(#[source] std::io::Error),
    #[error("the log file could not be written")]
    Write(#[source] std::io::Error),
    #[error("Windows did not give this process's times")]
    Clock(#[source] windows::core::Error),
    #[error("Windows did not give this process's memory counters")]
    Memory(#[source] windows::core::Error),
}

/// C-23: the log `--log` turns on, `cleanshot.log` beside the settings file, emptied at each
/// start. Each line is the milliseconds since this process started, then the event.
#[derive(Clone, Debug)]
pub struct Log {
    file: Arc<File>,
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
            file: Arc::new(File::create(path).map_err(LogError::Open)?),
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
        writeln!(&*self.file, "{at} {event}").map_err(LogError::Write)
    }
}

/// Where a failure that is not returned goes: `event` is fixed words naming it, and only
/// `error`'s OS code reaches the log, so no path or user name can (C-23).
pub type LogFailure<'a> = &'a dyn Fn(&'static str, &(dyn std::error::Error + 'static));

/// The code of the first Windows or I/O error in `error`'s chain of sources, itself first.
#[must_use]
pub fn os_code(error: &(dyn std::error::Error + 'static)) -> Option<i32> {
    std::iter::successors(Some(error), |error| error.source()).find_map(|error| {
        error
            .downcast_ref::<windows::core::Error>()
            .map(|error| error.code().0)
            .or_else(|| error.downcast_ref::<std::io::Error>()?.raw_os_error())
    })
}

/// How long ago Windows created this process, which counts the loader's work as well as
/// `main`'s (P-03).
///
/// # Errors
///
/// [`LogError::Clock`] if Windows does not give the process's times.
pub fn since_start() -> Result<Duration, LogError> {
    let times = process_times()?;
    // SAFETY: a read of the system clock.
    let now = unsafe { GetSystemTimePreciseAsFileTime() };
    Ok(duration(ticks(now).saturating_sub(ticks(times.created))))
}

/// What this process holds and has spent so far (P-04, P-05).
#[derive(Clone, Copy, Debug)]
pub struct Usage {
    pub working_set_bytes: usize,
    /// The part of the working set no other process can share: shared DLL and mapped pages
    /// are left out.
    pub private_working_set_bytes: usize,
    /// CPU time in kernel and user mode together.
    pub cpu: Duration,
}

/// This process's working set and CPU time now.
///
/// # Errors
///
/// [`LogError::Clock`] if Windows does not give the process's times, [`LogError::Memory`]
/// if it does not give its memory counters.
pub fn usage() -> Result<Usage, LogError> {
    let times = process_times()?;
    let mut counters = PROCESS_MEMORY_COUNTERS_EX2::default();
    // SAFETY: `counters` is writable and COUNTERS_SIZE is its size, which tells Windows it is the
    // EX2 layout, whose head is PROCESS_MEMORY_COUNTERS; the pseudo-handle needs no closing.
    unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            COUNTERS_SIZE,
        )
    }
    .ok()
    .map_err(LogError::Memory)?;
    Ok(Usage {
        working_set_bytes: counters.WorkingSetSize,
        private_working_set_bytes: counters.PrivateWorkingSetSize,
        cpu: duration(ticks(times.kernel) + ticks(times.user)),
    })
}

struct ProcessTimes {
    created: FILETIME,
    kernel: FILETIME,
    user: FILETIME,
}

fn process_times() -> Result<ProcessTimes, LogError> {
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
    Ok(ProcessTimes {
        created,
        kernel,
        user,
    })
}

fn duration(ticks: u64) -> Duration {
    Duration::from_nanos(ticks.saturating_mul(100))
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

    #[test]
    fn p05_cpu_time_counts_work_this_process_does() {
        let before = usage().expect("the usage reads").cpu;
        let spin = std::time::Instant::now();
        while spin.elapsed() < Duration::from_millis(200) {
            std::hint::black_box(spin.elapsed());
        }
        let after = usage().expect("the usage reads");
        assert!(
            after.cpu.saturating_sub(before) >= Duration::from_millis(100),
            "200 ms of spinning counted as {:?}",
            after.cpu.saturating_sub(before)
        );
        assert!(
            after.working_set_bytes > 0,
            "a running process has a working set"
        );
    }

    #[test]
    fn p04_the_private_working_set_leaves_shared_pages_out() {
        let held = usage().expect("the usage reads");
        assert!(
            (1..held.working_set_bytes).contains(&held.private_working_set_bytes),
            "private working set {} of {}: a running process holds private pages and shares DLL pages",
            held.private_working_set_bytes,
            held.working_set_bytes
        );
    }
}
