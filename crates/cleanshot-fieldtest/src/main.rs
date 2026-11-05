#![forbid(unsafe_code)]

mod budgets;

use std::cell::RefCell;
use std::fmt::{Display, Write as _};
use std::num::NonZeroU32;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cleanshot_editor::{BenchReport, bench};
use cleanshot_win::capture::{Method, capture_all};
use cleanshot_win::field_test::dpi::{copy_at_scale, set_scale};
use cleanshot_win::field_test::{
    Driver, adapters, animations_on, capture_device_costs, clipboard_round_trip,
    copies_while_watched, d3d11_feature_level, drive_tray, exit_running_tray, monitors,
    open_and_cancel_overlays, rendering_params, windows_version,
};
use cleanshot_win::log::since_start;

use budgets::Machine;

const HOTKEY_TIMEOUT: Duration = Duration::from_secs(5);
const CLIPBOARD_MARKER: &str = "cleanshot field test";
const CAPTURE_ATTEMPTS: u32 = 3;
const BENCH_REDRAWS: NonZeroU32 = NonZeroU32::new(600).expect("600 is not zero");
/// How long Cleanshot may take to close its tray window after Exit.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one overlay may take to show or to close before the run fails (D-62).
const OVERLAY_TIMEOUT: Duration = Duration::from_secs(5);
/// The pause after an overlay closes before the next press, as between a user's captures.
const OVERLAY_SETTLE: Duration = Duration::from_millis(500);
/// How long the exe has to log P-03's tray-ready line before `--overlays` fails.
const TRAY_TIMEOUT: Duration = Duration::from_secs(30);
/// D-88: the first press comes this long after the tray is ready, so it is not timed during start-up.
const START_SETTLE: Duration = Duration::from_secs(3);
/// How long after the last overlay closes the exe has to log its P-04 after-use sample, which
/// it takes 10 s after.
const AFTER_USE_TIMEOUT: Duration = Duration::from_secs(15);
/// How long one copy may take to be logged (P-02) before it counts as failed.
const COPY_TIMEOUT: Duration = Duration::from_secs(2);
/// How many times `--start-split` starts this exe.
const SPLIT_STARTS: u32 = 5;

fn main() {
    let report_path = match std::env::args().collect::<Vec<_>>().as_slice() {
        [_, flag, path] if flag == "--report" => path.clone(),
        [_, flag, inputs @ ..] if flag == "--budgets" => std::process::exit(judge(inputs)),
        [_, flag, log] if flag == "--exit" => std::process::exit(exit_from_tray(Path::new(log))),
        [_, flag] if flag == "--entered" => std::process::exit(print_entered()),
        [_, flag] if flag == "--start-split" => std::process::exit(start_split()),
        [_, flag, log] if flag == "--overlays" => {
            if let Err(error) = open_overlays_after_start(Path::new(log)) {
                eprintln!("{error}");
                std::process::exit(1);
            }
            return;
        }
        [_, flag, times, log] if flag == "--copies-while-watched" => {
            let Ok(times) = times.parse() else {
                eprintln!("--copies-while-watched takes a count, not {times:?}");
                std::process::exit(2);
            };
            let copied = copies_while_watched(times, COPY_TIMEOUT, || {
                perf_lines(Path::new(log), "P-02", "copied_ms")
            });
            println!("F-20: {copied} of {times} copies reached the clipboard while it was watched");
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
                eprintln!("FAIL F-01 at {dpi} dpi: {error}");
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
                perf_lines(Path::new(log), "P-02", "copied_ms")
            }) {
                eprintln!("FAIL F-01 at {dpi} dpi: {error}");
                std::process::exit(1);
            }
            println!(
                "F-01 at {dpi} dpi: the {width}x{height} copy holds the pattern pixel for pixel"
            );
            return;
        }
        _ => {
            eprintln!(
                "usage: cleanshot-fieldtest --report PATH | --overlays LOG | --exit LOG \
                 | --start-split \
                 | --budgets [--runner] [--exe EXE] [--idle LOG] [--overlays LOG] \
                 [--copy P02_OUTPUT] [--bench REPORT] \
                 | --set-scale WIDTH HEIGHT STEP DPI | --copy-at-scale WIDTH HEIGHT DPI LOG \
                 | --copies-while-watched COUNT LOG"
            );
            std::process::exit(2);
        }
    };
    let mut report = probe();
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
        (Ok(()), Some(Ok(timings))) => report_bench(&mut report, timings),
    }
    print!(
        "{}",
        &report[report.rfind("## GPUI editor bench").unwrap_or(0)..]
    );
    std::fs::write(&report_path, report).expect("the report path is writable");
}

/// Judges the budgets whose inputs `inputs` names in pairs (`--exe EXE`, `--idle LOG`,
/// `--overlays LOG`, `--copy P02_OUTPUT`, `--bench REPORT`), after `--runner` for a CI
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

/// The text at `path`, or no text when it cannot be read: each budget it holds then fails as
/// missing, and the reason is printed.
/// F-50: exits the running Cleanshot from its tray and checks its log says the shell removed
/// its icon. Returns the exit code: 0 when it did, 1 otherwise.
fn exit_from_tray(log: &Path) -> i32 {
    if let Err(error) = exit_running_tray(EXIT_TIMEOUT, || icon_removal_logged(log)) {
        eprintln!("FAIL F-50: {error}");
        return 1;
    }
    println!("F-50: Cleanshot exited from its tray and logged that the shell removed its icon");
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

/// P-03: starts this exe with `--entered` [`SPLIT_STARTS`] times and prints, for each, how
/// long it took to reach `main` by its own clock and how long the start took to exit as seen
/// from here, beside which cleanshot.exe's `main_entered_ms` says what is the exe and what is
/// the machine. Returns the exit code: 0 when every start answered, 1 otherwise.
fn start_split() -> i32 {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("P-03 split: this exe's path is not known: {error}");
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
                "P-03 split start {start}: fieldtest main_entered_ms={:.1} spawn_to_exit_ms={:.1}",
                Duration::from_micros(micros).as_secs_f64() * 1000.0,
                exited.as_secs_f64() * 1000.0
            ),
            Err(error) => {
                failed += 1;
                println!("P-03 split start {start}: failed: {error}");
            }
        }
    }
    i32::from(failed > 0)
}

fn log_text(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| {
        eprintln!("{path}: {error}");
        String::new()
    })
}

/// `--overlays LOG`: once the exe's tray is ready and settled (D-88), opens and cancels
/// [`budgets::OVERLAY_PRESSES`] overlays, then waits for the after-use P-04 sample.
fn open_overlays_after_start(log: &Path) -> Result<(), String> {
    let started = Instant::now();
    while !(log.exists() && perf_lines(log, "P-03", "tray_ready_ms") > 0) {
        if started.elapsed() >= TRAY_TIMEOUT {
            return Err(format!(
                "the exe logged no P-03 tray_ready_ms line within {TRAY_TIMEOUT:?}"
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(START_SETTLE);
    let shown = |press| perf_lines(log, "P-01", "overlay_visible_ms") >= press;
    open_and_cancel_overlays(
        budgets::OVERLAY_PRESSES,
        OVERLAY_TIMEOUT,
        OVERLAY_SETTLE,
        shown,
    )
    .map_err(|error| error.to_string())?;
    // P-04: the judge reads the after-use sample, or says it is missing.
    let waited = Instant::now();
    while perf_lines(log, "P-04", "private_working_set_bytes") == 0
        && waited.elapsed() < AFTER_USE_TIMEOUT
    {
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

/// F-50: whether the exe logged that the shell accepted its tray icon's removal.
fn icon_removal_logged(log: &Path) -> bool {
    std::fs::read_to_string(log)
        .expect("cleanshot.exe was started with --log here")
        .lines()
        .any(|line| line.ends_with(" tray icon_removed"))
}

/// How many `budget` perf lines (P-01 for an overlay shown, P-02 for a copy) cleanshot.exe
/// has logged in its `--log` file so far.
fn perf_lines(log: &Path, budget: &str, key: &str) -> u32 {
    let log = std::fs::read_to_string(log).expect("cleanshot.exe was started with --log here");
    let shown = log
        .lines()
        .filter(|line| line.contains(&format!(" perf {budget} {key}=")))
        .count();
    u32::try_from(shown).expect("fewer than 4 billion lines")
}

fn report_bench(report: &mut String, timings: BenchReport) {
    line(report, "Redraws", BENCH_REDRAWS);
    line(
        report,
        "First frame (P-07)",
        format!("{:?}", timings.first_frame),
    );
    line(report, "p50 interval (P-08)", format!("{:?}", timings.p50));
    line(report, "p95 interval (P-08)", format!("{:?}", timings.p95));
    line(report, "Worst interval", format!("{:?}", timings.worst));
    line(
        report,
        "Redaction drag p50 (D-58)",
        format!("{:?}", timings.redaction_drag_p50),
    );
    line(
        report,
        "Redaction drag p95 (D-58)",
        format!("{:?}", timings.redaction_drag_p95),
    );
    line(
        report,
        "fps at p50",
        format!("{:.1}", 1.0 / timings.p50.as_secs_f64()),
    );
    // D-41: the bench opens on the primary monitor, which Windows puts at (0, 0).
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
        "- perf P-07 first_frame_us={}\n- perf P-08 p95_us={}{refresh}",
        timings.first_frame.as_micros(),
        timings.p95.as_micros()
    )
    .expect("writing to a String succeeds");
}

fn probe() -> String {
    let mut report = String::from("# Cleanshot probe\n\n");
    report_windows(&mut report);
    report_display(&mut report);
    report_tray(&mut report);
    report_capture(&mut report);
    report_clipboard(&mut report);
    report_text(&mut report);
    report
}

fn report_windows(report: &mut String) {
    report.push_str("\n## Windows\n\n");
    match windows_version() {
        Ok(version) => {
            line(report, "Product", version.name());
            line(report, "Display version", version.display_version);
            line(
                report,
                "Build",
                format!("{}.{}", version.build, version.revision),
            );
            line(report, "Edition", version.edition);
            line(report, "Installation type", version.installation_type);
        }
        Err(error) => line(report, "Version", format!("error: {error}")),
    }
}

fn report_display(report: &mut String) {
    report.push_str("\n## Display\n\n");
    let animations = animations_on().map(|on| if on { "on" } else { "off" });
    line(
        report,
        "Animation effects (F-13 fade)",
        outcome(&animations),
    );
    let monitors = match monitors() {
        Ok(monitors) => monitors,
        Err(error) => {
            line(report, "Monitors", format!("error: {error}"));
            return;
        }
    };
    line(report, "Monitors", monitors.len());
    for monitor in &monitors {
        let bounds = monitor.bounds;
        let dpi = match &monitor.dpi {
            Ok(dpi) => format!("{dpi} dpi ({}%)", dpi * 100 / 96),
            Err(error) => format!("dpi error: {error}"),
        };
        let refresh = monitor.refresh_hertz.map_or_else(
            || "refresh unknown".to_owned(),
            |hertz| format!("{hertz} Hz"),
        );
        line(
            report,
            &monitor.device,
            format!(
                "{}x{} at ({}, {}), {dpi}, {refresh}",
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                bounds.left,
                bounds.top
            ),
        );
    }
    match adapters() {
        Ok(adapters) => {
            for adapter in adapters {
                line(
                    report,
                    &format!("Adapter {}", adapter.description),
                    format!(
                        "vendor {:#06x} device {:#06x}, {}, {} MB dedicated",
                        adapter.vendor_id,
                        adapter.device_id,
                        if adapter.software {
                            "software"
                        } else {
                            "hardware"
                        },
                        adapter.dedicated_video_memory / 1_000_000
                    ),
                );
            }
        }
        Err(error) => line(report, "Adapters", format!("error: {error}")),
    }
    for driver in [Driver::Hardware, Driver::Warp] {
        let level = d3d11_feature_level(driver).map(|level| format!("feature level {level:#x}"));
        line(report, &format!("D3D11 {driver:?}"), outcome(&level));
    }
}

fn report_tray(report: &mut String) {
    report.push_str("\n## Session, tray and hotkey\n\n");
    match drive_tray(HOTKEY_TIMEOUT) {
        Ok(run) => {
            line(report, "Foreground window", run.foreground_window);
            line(report, "Tray icon shown (NIM_ADD)", run.icon_shown);
            let (hotkey, registered) = &run.hotkey;
            let registered = registered.as_ref().map(|()| "registered");
            line(report, &format!("Hotkey {hotkey}"), outcome(&registered));
            line(report, "SendInput events accepted (of 6)", run.inputs_sent);
            let delivered = run.hotkey_delivered_after.map_or_else(
                || format!("not within {HOTKEY_TIMEOUT:?}"),
                |after| format!("after {after:?}"),
            );
            line(report, "Ctrl+Shift+4 delivered", delivered);
        }
        Err(error) => line(report, "Tray", format!("error: {error}")),
    }
}

/// P-01, P-03, P-04: what a capture device made at start-up would cost.
fn report_device_costs(report: &mut String) {
    match capture_device_costs() {
        Ok(costs) => {
            for cost in costs {
                line(
                    report,
                    &format!("Capture device on adapter {}", cost.adapter_index),
                    format!(
                        "made in {:?}; held: working set {:+} bytes, private {:+} bytes; dropped: working set {:+} bytes, private {:+} bytes",
                        cost.made, cost.held.0, cost.held.1, cost.dropped.0, cost.dropped.1
                    ),
                );
            }
        }
        Err(error) => line(report, "Capture device", format!("error: {error}")),
    }
}

fn report_capture(report: &mut String) {
    report.push_str("\n## Capture\n\n");
    report_device_costs(report);
    for method in [Method::GdiBitBlt, Method::DesktopDuplication] {
        for attempt in 1..=CAPTURE_ATTEMPTS {
            let start = Instant::now();
            let captured = capture_all(method);
            let elapsed = start.elapsed();
            let name = format!("{method:?} attempt {attempt}");
            match captured {
                Ok(monitors) => {
                    let described: Vec<String> = monitors
                        .iter()
                        .map(|monitor| {
                            format!(
                                "{}x{} with {} distinct colours sampled",
                                monitor.frame.width(),
                                monitor.frame.height(),
                                distinct_colours(monitor.frame.pixels())
                            )
                        })
                        .collect();
                    line(
                        report,
                        &name,
                        format!("{elapsed:?}: {}", described.join("; ")),
                    );
                }
                Err(error) => line(report, &name, format!("{elapsed:?}: error: {error}")),
            }
        }
    }
}

/// Distinct colours among every 97th BGRA pixel, whose fourth byte is not read: 1 means the
/// capture is blank.
fn distinct_colours(pixels: &[u8]) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .step_by(97)
        .map(|pixel| &pixel[..3])
        .collect::<std::collections::HashSet<_>>()
        .len()
}

fn report_clipboard(report: &mut String) {
    report.push_str("\n## Clipboard\n\n");
    let read_back = clipboard_round_trip(CLIPBOARD_MARKER).map(|text| {
        if text == CLIPBOARD_MARKER {
            "read back unchanged".to_owned()
        } else {
            format!("read back {text:?}")
        }
    });
    line(report, "Write and read back", outcome(&read_back));
}

fn report_text(report: &mut String) {
    report.push_str("\n## DirectWrite rendering params\n\n");
    match rendering_params() {
        Ok(params) => {
            line(report, "Gamma", format!("{:?}", params.gamma));
            line(
                report,
                "Enhanced contrast",
                format!("{:?}", params.enhanced_contrast),
            );
            line(
                report,
                "Grayscale enhanced contrast",
                format!("{:?}", params.grayscale_enhanced_contrast),
            );
            line(
                report,
                "ClearType level",
                format!("{:?}", params.cleartype_level),
            );
            line(report, "Pixel geometry", params.pixel_geometry.0);
            line(report, "Rendering mode", params.rendering_mode.0);
        }
        Err(error) => line(report, "Rendering params", format!("error: {error}")),
    }
}

fn line(report: &mut String, name: &str, value: impl Display) {
    writeln!(report, "- {name}: {value}").expect("writing to a String succeeds");
}

fn outcome<T: Display, E: Display>(result: &Result<T, E>) -> String {
    match result {
        Ok(value) => value.to_string(),
        Err(error) => format!("error: {error}"),
    }
}
