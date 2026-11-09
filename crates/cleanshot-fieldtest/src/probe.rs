use std::fmt::Display;
use std::time::{Duration, Instant};

use cleanshot_win::capture::{Method, capture_all};
use cleanshot_win::field_test::{
    Driver, adapters, animations_on, capture_device_costs, clipboard_round_trip,
    d3d11_feature_level, drive_tray, monitors, prewarm_costs, rendering_params, windows_version,
};

use crate::line;

const HOTKEY_TIMEOUT: Duration = Duration::from_secs(5);
const CLIPBOARD_MARKER: &str = "cleanshot field test";
const CAPTURE_ATTEMPTS: u32 = 3;

pub(crate) fn probe() -> String {
    let mut report = String::from("# Cleanshot probe\n\n");
    // First, so this process has not yet started Direct2D or DirectWrite.
    report_prewarm(&mut report);
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
    line(report, "Animation effects (fade)", outcome(&animations));
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

fn report_prewarm(report: &mut String) {
    match prewarm_costs() {
        Ok(parts) => {
            for part in parts {
                line(
                    report,
                    &format!("Overlay prewarm, {}", part.part),
                    format!("{:?}; private {:+} bytes", part.took, part.private_bytes),
                );
            }
        }
        Err(error) => line(report, "Overlay prewarm", format!("error: {error}")),
    }
}

/// What a capture device made at start-up would cost.
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

fn outcome<T: Display, E: Display>(result: &Result<T, E>) -> String {
    match result {
        Ok(value) => value.to_string(),
        Err(error) => format!("error: {error}"),
    }
}
