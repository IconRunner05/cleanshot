#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::fmt::{Display, Write as _};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cleanshot_editor::{BenchReport, bench};
use cleanshot_win::capture::{Method, capture_all};
use cleanshot_win::field_test::{
    Driver, adapters, clipboard_round_trip, d3d11_feature_level, drive_tray, monitors,
    open_and_cancel_overlays, rendering_params, windows_version,
};

const HOTKEY_TIMEOUT: Duration = Duration::from_secs(5);
const CLIPBOARD_MARKER: &str = "cleanshot field test";
const CAPTURE_ATTEMPTS: u32 = 3;
const BENCH_REDRAWS: NonZeroU32 = NonZeroU32::new(600).expect("600 is not zero");
const OVERLAY_GAP: Duration = Duration::from_millis(500);

fn main() {
    let report_path = match std::env::args().collect::<Vec<_>>().as_slice() {
        [_, flag, path] if flag == "--report" => path.clone(),
        [_, flag, times] if flag == "--overlays" => {
            let Ok(times) = times.parse() else {
                eprintln!("--overlays takes a count, not {times:?}");
                std::process::exit(2);
            };
            if open_and_cancel_overlays(times, OVERLAY_GAP) {
                return;
            }
            eprintln!("Windows refused a key event");
            std::process::exit(1);
        }
        _ => {
            eprintln!("usage: cleanshot-fieldtest --report PATH | --overlays COUNT");
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
        bench(cx, BENCH_REDRAWS, move |cx, report| {
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
        "fps at p50",
        format!("{:.1}", 1.0 / timings.p50.as_secs_f64()),
    );
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
            line(report, "Product", version.product_name);
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

fn report_capture(report: &mut String) {
    report.push_str("\n## Capture\n\n");
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
                                monitor.image.width(),
                                monitor.image.height(),
                                distinct_colours(monitor.image.pixels())
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

/// Distinct RGBA values among every 97th pixel: 1 means the capture is blank.
fn distinct_colours(pixels: &[u8]) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .step_by(97)
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
    report.push_str(
        "
## DirectWrite rendering params

",
    );
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
