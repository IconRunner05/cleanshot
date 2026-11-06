//! The CHARTER 8 budgets, each judged here and nowhere else, from the `perf` lines that
//! `cleanshot.exe --log` writes (D-62), and P-02 from the `perf` line its `p02_` test prints.

use std::fmt::{self, Display};
use std::io;

/// How many hotkeys `--overlays` presses; P-01's p95 is taken over this many overlays.
pub(crate) const OVERLAY_PRESSES: u32 = 20;
const OVERLAY_VISIBLE_BUDGET_MS: u64 = 100;
const TRAY_READY_BUDGET_MS: u64 = 300;
const IDLE_WORKING_SET_BUDGET_BYTES: u64 = 25 * 1024 * 1024;
const AFTER_USE_PRIVATE_BUDGET_BYTES: u64 = 10 * 1024 * 1024;
const COPY_BUDGET_US: u64 = 150_000;
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
    /// A hosted CI runner, a floor: P-01 and P-02 fail only over their budget times 1.5
    /// (D-75, D-90).
    Runner,
}

const fn runner_floor(budget: u64) -> u64 {
    budget * 3 / 2
}

/// P-01 from a log of [`OVERLAY_PRESSES`] overlays: the p95 (nearest rank) of their
/// `overlay_visible_ms` and the first press, each held to the budget (D-76); it fails unless
/// every press logged one. A runner judges the first press without its `first_show_ms`, which
/// it reports (D-150).
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
    let (judged_first, first_show) = match machine {
        Machine::Reference => (first, String::new()),
        Machine::Runner => {
            let Some(&shown) = perf_values(log, "P-01", "first_show_ms").first() else {
                return missing("P-01", "first_show_ms");
            };
            (
                first.saturating_sub(shown),
                format!(
                    ", {} ms without Windows' first show of its window, which took {shown} ms, \
                     reported, not judged",
                    first.saturating_sub(shown)
                ),
            )
        }
    };
    let measured = format!(
        "overlay visible p95 {p95} ms over {OVERLAY_PRESSES} hotkeys, first {first} \
         ms{first_show} (budget under {OVERLAY_VISIBLE_BUDGET_MS} ms for each"
    );
    let slowest = p95.max(judged_first);
    let within_budget = slowest < OVERLAY_VISIBLE_BUDGET_MS;
    let (met, detail) = match machine {
        Machine::Reference => (within_budget, format!("{measured}); sorted {times:?}")),
        Machine::Runner => {
            let floor = runner_floor(OVERLAY_VISIBLE_BUDGET_MS);
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
        own_start(log),
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

/// P-03 by D-121: Cleanshot's own start, from main entered to tray ready. The time before
/// main is the PC's process start, reported beside it and judged against nothing.
fn own_start(log: &str) -> Verdict {
    let Some(&entered) = perf_values(log, "P-03", "main_entered_ms").last() else {
        return missing("P-03", "main_entered_ms");
    };
    let Some(&ready) = perf_values(log, "P-03", "tray_ready_ms").last() else {
        return missing("P-03", "tray_ready_ms");
    };
    let Some(own) = ready.checked_sub(entered) else {
        return Verdict {
            budget: "P-03",
            met: false,
            detail: format!("tray ready at {ready} ms, before main entered at {entered} ms"),
        };
    };
    Verdict {
        budget: "P-03",
        met: own < TRAY_READY_BUDGET_MS,
        detail: format!(
            "tray ready {own} ms after main entered (budget under {TRAY_READY_BUDGET_MS} ms); \
             {entered} ms before main, not judged (D-121)"
        ),
    }
}

/// P-04 after use from the log of an overlay run: the private working set 10 s after the last
/// capture closed (D-82). Shared pages are not counted.
pub(crate) fn after_use(log: &str) -> Verdict {
    under(
        log,
        "P-04",
        "private_working_set_bytes",
        AFTER_USE_PRIVATE_BUDGET_BYTES,
        "private working set 10 s after the last capture",
        "bytes",
    )
}

/// P-02 from the `p02_` test's `--nocapture` output: the median of five copies of a 4K region.
pub(crate) fn copy(test_output: &str, machine: Machine) -> Verdict {
    let Some(&median) = perf_values(test_output, "P-02", "median_us").last() else {
        return missing("P-02", "median_us");
    };
    let measured =
        format!("median copy of a 4K region {median} us (budget under {COPY_BUDGET_US} us");
    let within_budget = median < COPY_BUDGET_US;
    let (met, detail) = match machine {
        Machine::Reference => (within_budget, format!("{measured})")),
        Machine::Runner => {
            let floor = runner_floor(COPY_BUDGET_US);
            let budget = if within_budget { "within" } else { "over" };
            (
                median <= floor,
                format!("{measured}, {budget} it; a runner fails only over {floor} us)"),
            )
        }
    };
    Verdict {
        budget: "P-02",
        met,
        detail,
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
    let Some(&value) = perf_values(log, budget, key).last() else {
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
pub(crate) fn perf_values(log: &str, budget: &str, key: &str) -> Vec<u64> {
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
        shown_log(times, 0)
    }

    fn shown_log(times: &[u64], first_show: u64) -> String {
        times
            .iter()
            .map(|ms| {
                format!(
                    "1200 perf P-01 overlay_visible_ms={ms}\n\
                     1200 perf P-01 first_show_ms={first_show}\n"
                )
            })
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
    fn d150_a_runner_judges_the_first_press_without_windows_first_show_of_its_window() {
        let after_a_first = |first, shown| {
            let mut times = vec![first];
            times.extend([5; 19]);
            shown_log(&times, shown)
        };
        let shown_slowly = overlay_visible(&after_a_first(300, 200), Machine::Runner);
        assert!(shown_slowly.met, "{}", shown_slowly.detail);
        assert!(
            shown_slowly
                .detail
                .contains("first 300 ms, 100 ms without Windows' first show"),
            "{}",
            shown_slowly.detail
        );
        assert!(!overlay_visible(&after_a_first(300, 149), Machine::Runner).met);
        assert!(!overlay_visible(&after_a_first(300, 200), Machine::Reference).met);
        let unreported = overlay_visible(
            &overlay_log(&[5; 20]).replace("first_show", "x"),
            Machine::Runner,
        );
        assert!(!unreported.met);
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
        let log = "5 perf P-03 main_entered_ms=900\n\
                   12 perf P-03 tray_ready_ms=1199\n\
                   10020 perf P-04 working_set_bytes=26214399\n\
                   10021 perf P-05 cpu_ms=99 over_ms=10000\n";
        assert!(idle(log).iter().all(|verdict| verdict.met));
        let over = "5 perf P-03 main_entered_ms=900\n\
                    12 perf P-03 tray_ready_ms=1200\n\
                    10020 perf P-04 working_set_bytes=26214400\n\
                    10021 perf P-05 cpu_ms=100 over_ms=10000\n";
        assert!(idle(over).iter().all(|verdict| !verdict.met));
    }

    #[test]
    fn d121_p03_counts_from_main_entered_and_reports_the_time_before_main() {
        let [start, ..] =
            idle("887 perf P-03 main_entered_ms=887\n1175 perf P-03 tray_ready_ms=1175\n");
        assert_eq!(
            start.to_string(),
            "P-03 PASS: tray ready 288 ms after main entered (budget under 300 ms); \
             887 ms before main, not judged (D-121)"
        );
    }

    #[test]
    fn a_missing_line_fails_its_budget() {
        let [tray, memory, cpu] = idle(
            "1 perf P-03 main_entered_ms=5\n12 perf P-03 tray_ready_ms=10\n13 perf P-04 cpu_ms=5\n",
        );
        assert!(tray.met);
        assert!(!memory.met);
        assert!(!cpu.met);
        assert_eq!(
            cpu.to_string(),
            "P-05 FAIL: no `perf P-05 cpu_ms=` line in the log"
        );
    }

    #[test]
    fn d82_after_use_holds_the_last_private_working_set_under_10_mb() {
        let log = |bytes: &[u64]| {
            bytes
                .iter()
                .map(|bytes| {
                    format!(
                        "9000 perf P-04 private_working_set_bytes={bytes}
"
                    )
                })
                .collect::<Vec<_>>()
                .concat()
        };
        assert!(after_use(&log(&[10_485_759])).met);
        assert!(!after_use(&log(&[10_485_760])).met);
        assert!(after_use(&log(&[10_485_760, 10_485_759])).met);
        assert!(!after_use(&log(&[10_485_759, 10_485_760])).met);
        let idle_only = "10020 perf P-04 working_set_bytes=1
";
        assert!(!after_use(idle_only).met);
    }

    #[test]
    fn d90_p02_holds_under_150_ms_and_a_runner_fails_only_over_225_ms() {
        let output =
            |us: u64| format!("perf P-02 median_us={us} all=[]\ntest result: ok. 1 passed\n");
        assert!(copy(&output(149_999), Machine::Reference).met);
        assert!(!copy(&output(150_000), Machine::Reference).met);
        let runner = copy(&output(160_000), Machine::Runner);
        assert!(runner.met);
        assert!(runner.detail.contains("over it"), "{}", runner.detail);
        assert!(copy(&output(225_000), Machine::Runner).met);
        assert!(!copy(&output(225_001), Machine::Runner).met);
        assert!(!copy("test result: ok. 0 passed", Machine::Runner).met);
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
