#![forbid(unsafe_code)]

mod budgets;
mod probe;

use std::cell::RefCell;
use std::fmt::{Display, Write as _};
use std::num::NonZeroU32;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cleanshot_editor::{BenchReport, bench};
use cleanshot_win::field_test::dpi::{copy_at_scale, set_scale};
use cleanshot_win::field_test::{
    copies_while_watched, edit_and_close, exit_running_tray, monitors, open_and_cancel_full_screen,
    open_and_cancel_overlays,
};
use cleanshot_win::log::since_start;

use budgets::Machine;

const BENCH_REDRAWS: NonZeroU32 = NonZeroU32::new(600).expect("600 is not zero");
/// How long Cleanshot may take to close its tray window after Exit.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one overlay may take to show or to close before the run fails.
const OVERLAY_TIMEOUT: Duration = Duration::from_secs(5);
/// The pause after an overlay closes before the next press, as between a user's captures.
const OVERLAY_SETTLE: Duration = Duration::from_millis(500);
/// How long the exe has to log its tray-ready line before `--overlays` fails.
const TRAY_TIMEOUT: Duration = Duration::from_secs(30);
/// The first press comes this long after the tray is ready, so it is not timed during start-up.
const START_SETTLE: Duration = Duration::from_secs(3);
/// How long after the last overlay closes the exe has to log its after-use memory sample, which
/// it takes 10 s after.
const AFTER_USE_TIMEOUT: Duration = Duration::from_secs(15);
/// The same for its later sample, reported only, which it takes 30 s after.
const AFTER_USE_LATE_TIMEOUT: Duration = Duration::from_secs(35);
/// How long after the editor closes the exe has to log its after-editor memory sample, which it
/// takes 15 s after.
const AFTER_EDITOR_TIMEOUT: Duration = Duration::from_secs(20);
/// How long `--editor` keeps the editor open for the exe's idle sample, which it takes 10 s
/// after its tray is ready.
const IDLE_SAMPLE_TIMEOUT: Duration = Duration::from_secs(15);
/// After the after-editor sample, 15 s after the editor closed, how much longer `--editor`
/// waits before its after-idle press.
const AFTER_IDLE_EXTRA: Duration = Duration::from_secs(5);
/// How long one copy may take to be logged before it counts as failed.
const COPY_TIMEOUT: Duration = Duration::from_secs(2);
const SPLIT_STARTS: u32 = 5;

fn main() {
    let report_path = match std::env::args().collect::<Vec<_>>().as_slice() {
        [_, flag, path] if flag == "--report" => path.clone(),
        [_, flag, inputs @ ..] if flag == "--budgets" => std::process::exit(judge(inputs)),
        [_, flag, log] if flag == "--exit" => std::process::exit(exit_from_tray(Path::new(log))),
        [_, flag] if flag == "--entered" => std::process::exit(print_entered()),
        [_, flag] if flag == "--start-split" => std::process::exit(start_split()),
        [_, flag, log] if flag == "--overlays" || flag == "--editor" => {
            std::process::exit(after_start(flag, Path::new(log)))
        }
        [_, flag, times, log] if flag == "--copies-while-watched" => {
            let Ok(times) = times.parse() else {
                eprintln!("--copies-while-watched takes a count, not {times:?}");
                std::process::exit(2);
            };
            let copied = copies_while_watched(times, COPY_TIMEOUT, || {
                perf_lines(Path::new(log), "copy", "copied_ms")
            });
            println!("{copied} of {times} copies reached the clipboard while it was watched");
            if copied < times {
                std::process::exit(1);
            }
            return;
        }
        [_, flag, width, height, step, dpi] if flag == "--set-scale" => {
            let (Ok(width), Ok(height), Ok(step), Ok(dpi)) =
                (width.parse(), height.parse(), step.parse(), dpi.parse())
            else {
                eprintln!("--set-scale takes WIDTH HEIGHT STEP DPI as numbers");
                std::process::exit(2);
            };
            if let Err(error) = set_scale(width, height, step, dpi) {
                eprintln!("FAIL at {dpi} dpi: {error}");
                std::process::exit(1);
            }
            return;
        }
        [_, flag, width, height, dpi, log] if flag == "--copy-at-scale" => {
            let (Ok(width), Ok(height), Ok(dpi)) = (width.parse(), height.parse(), dpi.parse())
            else {
                eprintln!("--copy-at-scale takes WIDTH HEIGHT DPI as numbers, then LOG");
                std::process::exit(2);
            };
            if let Err(error) = copy_at_scale(width, height, dpi, || {
                perf_lines(Path::new(log), "copy", "copied_ms")
            }) {
                eprintln!("FAIL at {dpi} dpi: {error}");
                std::process::exit(1);
            }
            println!("At {dpi} dpi: the {width}x{height} copy holds the pattern pixel for pixel");
            return;
        }
        _ => {
            eprintln!(
                "usage: cleanshot-fieldtest --report PATH | --overlays LOG | --editor LOG | --exit LOG \
                 | --start-split \
                 | --budgets [--runner] [--exe EXE] [--idle LOG] [--overlays LOG] \
                 [--copy COPY_OUTPUT] [--bench REPORT] \
                 | --set-scale WIDTH HEIGHT STEP DPI | --copy-at-scale WIDTH HEIGHT DPI LOG \
                 | --copies-while-watched COUNT LOG"
            );
            std::process::exit(2);
        }
    };
    let mut report = probe::probe();
    report.push_str("\n## GPUI editor bench\n\n");
    // The bench can take the process down with it, so what the probe found is on disk first.
    print!("{report}");
    std::fs::write(&report_path, &report).expect("the report path is writable");
    let finished = Rc::new(RefCell::new(None));
    let result = Rc::clone(&finished);
    let started = cleanshot_editor::run(move |cx| {
        let masks = |document: &_| {
            cleanshot_win::text::masks(document)
                .inspect_err(|error| eprintln!("text_masks not_made: {error}"))
                .ok()
        };
        bench(cx, BENCH_REDRAWS, masks, move |cx, report| {
            *result.borrow_mut() = Some(report);
            cx.quit();
        });
    });
    match (started, finished.take()) {
        (Err(error), _) => line(&mut report, "Editor", format!("error: {error}")),
        (Ok(()), None) => line(&mut report, "Editor", "quit before the bench finished"),
        (Ok(()), Some(Err(error))) => line(&mut report, "Bench", format!("error: {error}")),
        (Ok(()), Some(Ok(timings))) => report_bench(&mut report, &timings),
    }
    print!(
        "{}",
        &report[report.rfind("## GPUI editor bench").unwrap_or(0)..]
    );
    std::fs::write(&report_path, report).expect("the report path is writable");
}

/// Judges the budgets whose inputs `inputs` names in pairs (`--exe EXE`, `--idle LOG`,
/// `--overlays LOG`, `--copy COPY_OUTPUT`, `--bench REPORT`), after `--runner` for a CI
/// runner, and prints one verdict a line. Returns the exit code: 0 when every budget judged
/// is met, 1 when one is not, 2 for bad arguments.
fn judge(inputs: &[String]) -> i32 {
    let (machine, inputs) = match inputs {
        [flag, rest @ ..] if flag == "--runner" => (Machine::Runner, rest),
        _ => (Machine::Reference, inputs),
    };
    let mut verdicts = Vec::new();
    for pair in inputs.chunks(2) {
        let [input, path] = pair else {
            eprintln!(
                "--budgets takes --exe, --idle, --overlays, --copy or --bench, each with a path"
            );
            return 2;
        };
        match input.as_str() {
            "--exe" => verdicts.push(budgets::exe_size(
                path,
                std::fs::metadata(path).map(|exe| exe.len()),
            )),
            "--idle" => verdicts.extend(budgets::idle(&log_text(path))),
            "--overlays" => {
                let log = log_text(path);
                verdicts.push(budgets::overlay_visible(&log, machine));
                verdicts.push(budgets::after_use(&log));
            }
            "--copy" => verdicts.push(budgets::copy(&log_text(path), machine)),
            "--bench" => verdicts.extend(budgets::editor(&log_text(path))),
            _ => {
                eprintln!("--budgets does not take {input:?}");
                return 2;
            }
        }
    }
    if verdicts.is_empty() {
        eprintln!("--budgets takes at least one input to judge");
        return 2;
    }
    for verdict in &verdicts {
        println!("{verdict}");
    }
    i32::from(!verdicts.iter().all(|verdict| verdict.met))
}

/// Exits the running Cleanshot from its tray and checks its log says the shell removed
/// its icon. Returns the exit code: 0 when it did, 1 otherwise.
fn exit_from_tray(log: &Path) -> i32 {
    if let Err(error) = exit_running_tray(EXIT_TIMEOUT, || icon_removal_logged(log)) {
        eprintln!("FAIL: {error}");
        return 1;
    }
    println!("Cleanshot exited from its tray and logged that the shell removed its icon");
    0
}

/// Prints how many microseconds this process took to reach `main`, for `--start-split`.
fn print_entered() -> i32 {
    match since_start() {
        Ok(entered) => {
            println!("{}", entered.as_micros());
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

/// Starts this exe with `--entered` [`SPLIT_STARTS`] times and prints, for each, how
/// long it took to reach `main` by its own clock and how long the start took to exit as seen
/// from here, beside which cleanshot.exe's `main_entered_ms` says what is the exe and what is
/// the machine. Returns the exit code: 0 when every start answered, 1 otherwise.
fn start_split() -> i32 {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("split: this exe's path is not known: {error}");
            return 1;
        }
    };
    let mut failed = 0;
    for start in 1..=SPLIT_STARTS {
        let spawned = Instant::now();
        let output = std::process::Command::new(&exe).arg("--entered").output();
        let exited = spawned.elapsed();
        let entered = match &output {
            Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u64>()
                .map_err(|error| error.to_string()),
            Ok(output) => Err(format!(
                "{}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => Err(error.to_string()),
        };
        match entered {
            Ok(micros) => println!(
                "split start {start}: fieldtest main_entered_ms={:.1} spawn_to_exit_ms={:.1}",
                Duration::from_micros(micros).as_secs_f64() * 1000.0,
                exited.as_secs_f64() * 1000.0
            ),
            Err(error) => {
                failed += 1;
                println!("split start {start}: failed: {error}");
            }
        }
    }
    i32::from(failed > 0)
}

/// The text at `path`, or no text when it cannot be read: each budget it holds then fails as
/// missing, and the reason is printed.
fn log_text(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| {
        eprintln!("{path}: {error}");
        String::new()
    })
}

/// Runs `--overlays` or `--editor` against the exe logging to `log`; the exit code.
fn after_start(flag: &str, log: &Path) -> i32 {
    let run = if flag == "--overlays" {
        open_overlays_after_start
    } else {
        edit_after_start
    };
    run(log).map_or_else(
        |error| {
            eprintln!("{error}");
            1
        },
        |()| 0,
    )
}

/// `--overlays LOG`: once the exe's tray is ready and settled, opens and cancels
/// [`budgets::OVERLAY_PRESSES`] overlays, then waits for the after-use memory samples.
fn open_overlays_after_start(log: &Path) -> Result<(), String> {
    wait_for_tray(log)?;
    let shown = |press| perf_lines(log, "overlay", "overlay_visible_ms") >= press;
    open_and_cancel_overlays(
        budgets::OVERLAY_PRESSES,
        OVERLAY_TIMEOUT,
        OVERLAY_SETTLE,
        shown,
    )
    .map_err(|error| error.to_string())?;
    report_acquires(log)?;
    // The judge reads the after-use samples, or says they are missing.
    let waited = Instant::now();
    for (key, timeout) in [
        ("private_working_set_bytes", AFTER_USE_TIMEOUT),
        ("late_private_working_set_bytes", AFTER_USE_LATE_TIMEOUT),
    ] {
        while perf_lines(log, "memory", key) == 0 && waited.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    Ok(())
}

/// How long the first output and the slowest later one waited for their first frame,
/// over the presses that duplicated two or more outputs.
fn report_acquires(log: &Path) -> Result<(), String> {
    let text =
        std::fs::read_to_string(log).map_err(|error| format!("{}: {error}", log.display()))?;
    let median = |key| {
        let mut values = budgets::perf_values(&text, "overlay", key);
        values.sort_unstable();
        values.get(values.len() / 2).copied()
    };
    match (median("first_acquire_us"), median("later_acquire_us")) {
        (Some(first), Some(later)) => println!(
            "Acquire, reported, not judged: median wait for a first frame {first} us on \
             the first output, {later} us on the slowest later one"
        ),
        _ => println!("Acquire, reported, not judged: no press duplicated two outputs"),
    }
    Ok(())
}

/// Memory after the editor: once the tray is ready, opens an editor on a full-screen capture,
/// closes it once the exe has taken its idle sample with the editor open, and waits for the
/// exe's after-editor sample. Then the overlay after idle: 20 s after the close, opens and cancels one
/// full-screen overlay and prints its time. All are reported, not judged.
fn edit_after_start(log: &Path) -> Result<(), String> {
    wait_for_tray(log)?;
    edit_and_close(
        OVERLAY_TIMEOUT,
        || perf_lines(log, "overlay", "overlay_visible_ms") > 0,
        || perf_lines(log, "editor_open", "first_frame_ms") > 0,
        IDLE_SAMPLE_TIMEOUT,
        || perf_lines(log, "memory", "working_set_bytes") > 0,
    )
    .map_err(|error| error.to_string())?;
    let waited = Instant::now();
    while perf_lines(log, "memory", "after_editor_private_bytes") == 0 {
        if waited.elapsed() >= AFTER_EDITOR_TIMEOUT {
            return Err(format!(
                "the exe logged no memory after_editor_private_bytes line within \
                 {AFTER_EDITOR_TIMEOUT:?} of the editor's close"
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    // GRAPHICS_IDLE let the capture device and Direct2D go long ago, as before a user's next
    // capture minutes later.
    std::thread::sleep(AFTER_IDLE_EXTRA);
    let shown = || perf_lines(log, "overlay", "overlay_visible_ms") >= 2;
    open_and_cancel_full_screen(OVERLAY_TIMEOUT, shown).map_err(|error| error.to_string())?;
    let text =
        std::fs::read_to_string(log).map_err(|error| format!("{}: {error}", log.display()))?;
    let Some(after_idle) = budgets::perf_values(&text, "overlay", "overlay_visible_ms")
        .get(1)
        .copied()
    else {
        return Err("the after-idle press logged no overlay_visible_ms value".to_owned());
    };
    println!(
        "Overlay after idle, reported, not judged: overlay visible {after_idle} ms on a press 20 s \
         after the editor closed, once GRAPHICS_IDLE had let the capture device and Direct2D go"
    );
    Ok(())
}

/// Waits for the exe's tray-ready line, then [`START_SETTLE`], so the first press is not
/// timed during start-up.
fn wait_for_tray(log: &Path) -> Result<(), String> {
    let started = Instant::now();
    while !(log.exists() && perf_lines(log, "start", "tray_ready_ms") > 0) {
        if started.elapsed() >= TRAY_TIMEOUT {
            return Err(format!(
                "the exe logged no start tray_ready_ms line within {TRAY_TIMEOUT:?}"
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(START_SETTLE);
    Ok(())
}

/// Whether the exe logged that the shell accepted its tray icon's removal.
fn icon_removal_logged(log: &Path) -> bool {
    std::fs::read_to_string(log)
        .expect("cleanshot.exe was started with --log here")
        .lines()
        .any(|line| line.ends_with(" tray icon_removed"))
}

/// How many `budget` perf lines (`overlay` for an overlay shown, `copy` for a copy) cleanshot.exe
/// has logged in its `--log` file so far.
fn perf_lines(log: &Path, budget: &str, key: &str) -> u32 {
    let log = std::fs::read_to_string(log).expect("cleanshot.exe was started with --log here");
    let shown = log
        .lines()
        .filter(|line| line.contains(&format!(" perf {budget} {key}=")))
        .count();
    u32::try_from(shown).expect("fewer than 4 billion lines")
}

fn report_bench(report: &mut String, timings: &BenchReport) {
    line(report, "Redraws", BENCH_REDRAWS);
    line(report, "First frame", format!("{:?}", timings.first_frame));
    line(report, "p50 interval", format!("{:?}", timings.p50));
    line(report, "p95 interval", format!("{:?}", timings.p95));
    line(report, "Worst interval", format!("{:?}", timings.worst));
    line(
        report,
        "Redaction drag p50",
        format!("{:?}", timings.redaction_drag_p50),
    );
    line(
        report,
        "Redaction drag p95",
        format!("{:?}", timings.redaction_drag_p95),
    );
    for drag in timings.shape_drags {
        line(
            report,
            drag.name,
            format!("p50 {:?}, p95 {:?}", drag.p50, drag.p95),
        );
    }
    line(
        report,
        "fps at p50",
        format!("{:.1}", 1.0 / timings.p50.as_secs_f64()),
    );
    // The bench opens on the primary monitor, which Windows puts at (0, 0).
    let primary = monitors().map(|monitors| {
        monitors
            .into_iter()
            .find(|monitor| monitor.bounds.left == 0 && monitor.bounds.top == 0)
    });
    let refresh = match primary {
        Ok(Some(monitor)) => {
            line(
                report,
                "Bench monitor",
                format!("{} (primary)", monitor.device),
            );
            monitor.refresh_hertz
        }
        Ok(None) => {
            line(report, "Bench monitor", "no monitor at (0, 0)");
            None
        }
        Err(error) => {
            line(report, "Bench monitor", format!("error: {error}"));
            None
        }
    };
    let refresh = refresh.map_or_else(String::new, |hertz| format!(" refresh_hz={hertz}"));
    writeln!(
        report,
        "- perf editor_open first_frame_us={} graphics_us={} text_masks_us={} previews_us={} glow_us={}\n- perf frames percentile_95_us={}{refresh}",
        timings.first_frame.as_micros(),
        timings.graphics.as_micros(),
        timings.text_masks.as_micros(),
        timings.previews.as_micros(),
        timings.glow.as_micros(),
        timings.p95.as_micros()
    )
    .expect("writing to a String succeeds");
}

fn line(report: &mut String, name: &str, value: impl Display) {
    writeln!(report, "- {name}: {value}").expect("writing to a String succeeds");
}
