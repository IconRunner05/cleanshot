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
/// `overlay_visible_ms`, which fails unless every press logged one.
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
         (budget under {OVERLAY_VISIBLE_BUDGET_MS} ms"
    );
    let within_budget = p95 < OVERLAY_VISIBLE_BUDGET_MS;
    let (met, detail) = match machine {
        Machine::Reference => (within_budget, format!("{measured}); sorted {times:?}")),
        Machine::Runner => {
            let floor = OVERLAY_VISIBLE_BUDGET_MS * 3 / 2;
            let budget = if within_budget { "within" } else { "over" };
            (
                p95 <= floor,
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
    fn p06_holds_under_15_mb() {
        assert!(exe_size("cleanshot.exe", Ok(14_999_999)).met);
        assert!(!exe_size("cleanshot.exe", Ok(15_000_000)).met);
        let unreadable = io::Error::from(io::ErrorKind::NotFound);
        assert!(!exe_size("cleanshot.exe", Err(unreadable)).met);
    }
}
