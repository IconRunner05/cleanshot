//! The CHARTER 8 budgets, each judged here and nowhere else, from the `perf` lines that
//! `cleanshot.exe --log` writes (D-62). P-02 is the exception: its test holds its budget.

use std::fmt::{self, Display};
use std::io;

/// How many hotkeys `--overlays` presses; P-01's p95 is taken over this many overlays.
pub(crate) const OVERLAY_PRESSES: u32 = 20;
const OVERLAY_VISIBLE_BUDGET_MS: u64 = 100;
const TRAY_READY_BUDGET_MS: u64 = 300;
const IDLE_WORKING_SET_BUDGET_BYTES: u64 = 25 * 1024 * 1024;
// P-05's "~0%": under 1% of one core over the 10 s the exe logs.
const IDLE_CPU_BUDGET_MS: u64 = 100;
const EXE_SIZE_BUDGET_BYTES: u64 = 15_000_000;
const EDITOR_FIRST_FRAME_BUDGET_US: u64 = 400_000;
// D-57: P-08's 60 fps is met when the p95 frame interval is at most 1.5 refresh periods.
const MOST_INTERVAL_US_TIMES_HZ: u64 = 1_500_000;

/// One budget's outcome: its P-id, whether it is met, and the measurement against the budget.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Verdict {
    pub(crate) budget: &'static str,
    pub(crate) met: bool,
    pub(crate) detail: String,
}

impl Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let outcome = if self.met { "PASS" } else { "FAIL" };
        write!(f, "{} {outcome}: {}", self.budget, self.detail)
    }
}

/// Where the budgets are measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Machine {
    /// CHARTER 8's reference machine, where every budget is a gate.
    Reference,
    /// A hosted CI runner, a floor: P-01 fails only over its budget times 1.5 (D-75).
    Runner,
}

/// P-01 from a log of [`OVERLAY_PRESSES`] overlays: the p95 (nearest rank) of their
/// `overlay_visible_ms` and the first press, each held to the budget (D-76); it fails unless
/// every press logged one.
pub(crate) fn overlay_visible(log: &str, machine: Machine) -> Verdict {
    let mut times = perf_values(log, "P-01", "overlay_visible_ms");
    let Some(&first) = times.first() else {
        return missing("P-01", "overlay_visible_ms");
    };
    if times.len() != OVERLAY_PRESSES as usize {
        return Verdict {
            budget: "P-01",
            met: false,
            detail: format!(
                "{} of {OVERLAY_PRESSES} hotkeys logged an overlay_visible_ms line",
                times.len()
            ),
        };
    }
    times.sort_unstable();
    let p95 = times[(times.len() * 95).div_ceil(100) - 1];
    let measured = format!(
        "overlay visible p95 {p95} ms over {OVERLAY_PRESSES} hotkeys, first {first} ms \
         (budget under {OVERLAY_VISIBLE_BUDGET_MS} ms for each"
    );
    let slowest = p95.max(first);
    let within_budget = slowest < OVERLAY_VISIBLE_BUDGET_MS;
    let (met, detail) = match machine {
        Machine::Reference => (within_budget, format!("{measured}); sorted {times:?}")),
        Machine::Runner => {
            let floor = OVERLAY_VISIBLE_BUDGET_MS * 3 / 2;
            let budget = if within_budget { "within" } else { "over" };
            (
                slowest <= floor,
                format!(
                    "{measured}, {budget} it; a runner fails only over {floor} ms); \
                     sorted {times:?}"
                ),
            )
        }
    };
    Verdict {
        budget: "P-01",
        met,
        detail,
    }
}

/// P-03, P-04 and P-05 from the log of one idle start.
pub(crate) fn idle(log: &str) -> [Verdict; 3] {
    [
        under(
            log,
            "P-03",
            "tray_ready_ms",
            TRAY_READY_BUDGET_MS,
            "tray ready after",
            "ms",
        ),
        under(
            log,
            "P-04",
            "working_set_bytes",
            IDLE_WORKING_SET_BUDGET_BYTES,
            "idle working set",
            "bytes",
        ),
        under(
            log,
            "P-05",
            "cpu_ms",
            IDLE_CPU_BUDGET_MS,
            "CPU spent idle over 10 s:",
            "ms",
        ),
    ]
}

/// P-02 as its `p02_` test judged it, from the test binary's `--nocapture` output.
pub(crate) fn copy(test_output: &str) -> Verdict {
    let passed = test_output.contains("test result: ok. 1 passed");
    let Some(median) = test_output
        .lines()
        .find(|line| line.starts_with("P-02 median copy"))
    else {
        return Verdict {
            budget: "P-02",
            met: false,
            detail: "the p02_ test printed no median copy time".to_owned(),
        };
    };
    Verdict {
        budget: "P-02",
        met: passed,
        detail: format!("{median} (judged by the p02_ test)"),
    }
}

/// P-07 and P-08 from the probe report's bench lines; P-08 by D-57, against the refresh rate
/// of the monitor the bench ran on.
pub(crate) fn editor(report: &str) -> [Verdict; 2] {
    let first_frame = under(
        report,
        "P-07",
        "first_frame_us",
        EDITOR_FIRST_FRAME_BUDGET_US,
        "editor first frame after",
        "us",
    );
    let Some(&p95) = perf_values(report, "P-08", "p95_us").first() else {
        return [first_frame, missing("P-08", "p95_us")];
    };
    let Some(&hertz) = perf_values(report, "P-08", "refresh_hz")
        .iter()
        .find(|hertz| **hertz > 0)
    else {
        return [
            first_frame,
            Verdict {
                budget: "P-08",
                met: false,
                detail: format!(
                    "frame interval p95 {p95} us, but the bench monitor's refresh rate is \
                     unknown, so D-57 has no bound"
                ),
            },
        ];
    };
    let bound = MOST_INTERVAL_US_TIMES_HZ / hertz;
    let frames = Verdict {
        budget: "P-08",
        met: p95 * hertz <= MOST_INTERVAL_US_TIMES_HZ,
        detail: format!(
            "frame interval p95 {p95} us at {hertz} Hz (D-57: at most 1.5 refresh periods, \
             {bound} us)"
        ),
    };
    [first_frame, frames]
}

/// P-06 from the release exe's size.
pub(crate) fn exe_size(path: &str, bytes: io::Result<u64>) -> Verdict {
    match bytes {
        Ok(bytes) => Verdict {
            budget: "P-06",
            met: bytes < EXE_SIZE_BUDGET_BYTES,
            detail: format!(
                "release exe {bytes} bytes (budget under {EXE_SIZE_BUDGET_BYTES} bytes)"
            ),
        },
        Err(error) => Verdict {
            budget: "P-06",
            met: false,
            detail: format!("{path}: {error}"),
        },
    }
}

fn under(
    log: &str,
    budget: &'static str,
    key: &str,
    limit: u64,
    what: &str,
    unit: &str,
) -> Verdict {
    let Some(&value) = perf_values(log, budget, key).first() else {
        return missing(budget, key);
    };
    Verdict {
        budget,
        met: value < limit,
        detail: format!("{what} {value} {unit} (budget under {limit} {unit})"),
    }
}

fn missing(budget: &'static str, key: &str) -> Verdict {
    Verdict {
        budget,
        met: false,
        detail: format!("no `perf {budget} {key}=` line in the log"),
    }
}

/// Every `key`'s value on a `perf <budget> ...` line, in log order.
fn perf_values(log: &str, budget: &str, key: &str) -> Vec<u64> {
    log.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace().skip_while(|word| *word != "perf");
            words.next()?;
            if words.next()? != budget {
                return None;
            }
            words
                .filter_map(|word| word.split_once('='))
                .find(|(name, _)| *name == key)
                .and_then(|(_, value)| value.parse().ok())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay_log(times: &[u64]) -> String {
        times
            .iter()
            .map(|ms| format!("1200 perf P-01 overlay_visible_ms={ms}\n"))
            .collect::<Vec<_>>()
            .concat()
    }

    #[test]
    fn p01_is_the_19th_fastest_of_20_overlays() {
        let mut times: Vec<u64> = (1..=18).collect();
        times.extend([99, 500]);
        assert!(overlay_visible(&overlay_log(&times), Machine::Reference).met);
        times[18] = 100;
        assert!(!overlay_visible(&overlay_log(&times), Machine::Reference).met);
    }

    #[test]
    fn d75_a_runner_fails_p01_only_over_one_and_a_half_times_the_budget() {
        let mut times = vec![5; 18];
        times.extend([150, 150]);
        let runner = overlay_visible(&overlay_log(&times), Machine::Runner);
        assert!(runner.met);
        assert!(runner.detail.contains("over it"), "{}", runner.detail);
        assert!(!overlay_visible(&overlay_log(&times), Machine::Reference).met);
        times[18..].fill(151);
        assert!(!overlay_visible(&overlay_log(&times), Machine::Runner).met);
    }

    #[test]
    fn d76_the_first_press_is_held_to_the_p01_budget_like_the_p95() {
        let after_a_first = |first| {
            let mut times = vec![first];
            times.extend([5; 19]);
            overlay_log(&times)
        };
        assert!(overlay_visible(&after_a_first(99), Machine::Reference).met);
        assert!(!overlay_visible(&after_a_first(100), Machine::Reference).met);
        assert!(overlay_visible(&after_a_first(150), Machine::Runner).met);
        assert!(!overlay_visible(&after_a_first(151), Machine::Runner).met);
    }

    #[test]
    fn p01_fails_unless_every_hotkey_logged_an_overlay() {
        let verdict = overlay_visible(&overlay_log(&[5; 19]), Machine::Reference);
        assert!(!verdict.met);
        assert_eq!(
            verdict.detail,
            "19 of 20 hotkeys logged an overlay_visible_ms line"
        );
        assert!(!overlay_visible("", Machine::Reference).met);
    }

    #[test]
    fn idle_budgets_are_read_from_the_exe_log_lines() {
        let log = "12 perf P-03 tray_ready_ms=299\n\
                   10020 perf P-04 working_set_bytes=26214399\n\
                   10021 perf P-05 cpu_ms=99 over_ms=10000\n";
        assert!(idle(log).iter().all(|verdict| verdict.met));
        let over = "12 perf P-03 tray_ready_ms=300\n\
                    10020 perf P-04 working_set_bytes=26214400\n\
                    10021 perf P-05 cpu_ms=100 over_ms=10000\n";
        assert!(idle(over).iter().all(|verdict| !verdict.met));
    }

    #[test]
    fn a_missing_line_fails_its_budget() {
        let [tray, memory, cpu] = idle("12 perf P-03 tray_ready_ms=10\n13 perf P-04 cpu_ms=5\n");
        assert!(tray.met);
        assert!(!memory.met);
        assert!(!cpu.met);
        assert_eq!(
            cpu.to_string(),
            "P-05 FAIL: no `perf P-05 cpu_ms=` line in the log"
        );
    }

    #[test]
    fn p02_is_the_verdict_of_its_test() {
        let median = "P-02 median copy of a 4K region 61.2ms, all [60ms, 61.2ms]\n";
        assert!(copy(&format!("{median}test result: ok. 1 passed; 0 failed")).met);
        assert!(!copy(&format!("{median}test result: FAILED. 0 passed; 1 failed")).met);
        assert!(!copy("test result: ok. 0 passed; 0 failed").met);
    }

    #[test]
    fn p07_holds_under_400_ms() {
        let report = |us: u64| format!("- perf P-07 first_frame_us={us}\n");
        assert!(editor(&report(399_999))[0].met);
        assert!(!editor(&report(400_000))[0].met);
    }

    #[test]
    fn d57_p08_holds_at_most_one_and_a_half_refresh_periods() {
        let report =
            |p95: u64, hertz: u64| format!("- perf P-08 p95_us={p95} refresh_hz={hertz}\n");
        assert!(editor(&report(25_000, 60))[1].met);
        assert!(!editor(&report(25_001, 60))[1].met);
        assert!(editor(&report(10_416, 144))[1].met);
        assert!(!editor(&report(10_417, 144))[1].met);
    }

    #[test]
    fn p08_fails_without_the_bench_monitors_refresh_rate() {
        assert!(!editor("- perf P-08 p95_us=1000\n")[1].met);
        assert!(!editor("- perf P-08 p95_us=1000 refresh_hz=0\n")[1].met);
        assert!(!editor("")[1].met);
    }

    #[test]
    fn p06_holds_under_15_mb() {
        assert!(exe_size("cleanshot.exe", Ok(14_999_999)).met);
        assert!(!exe_size("cleanshot.exe", Ok(15_000_000)).met);
        let unreadable = io::Error::from(io::ErrorKind::NotFound);
        assert!(!exe_size("cleanshot.exe", Err(unreadable)).met);
    }
}
