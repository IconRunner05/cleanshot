#![forbid(unsafe_code)]

use std::fmt::{Display, Write as _};
use std::time::Duration;

use cleanshot_win::field_test::{
    Driver, adapters, clipboard_round_trip, d3d11_feature_level, drive_tray, make_dpi_aware,
    monitors, windows_version,
};

const HOTKEY_TIMEOUT: Duration = Duration::from_secs(5);
const CLIPBOARD_MARKER: &str = "cleanshot field test";

fn main() {
    let report_path = match std::env::args().collect::<Vec<_>>().as_slice() {
        [_, flag, path] if flag == "--report" => path.clone(),
        _ => {
            eprintln!("usage: cleanshot-fieldtest --report PATH");
            std::process::exit(2);
        }
    };
    let report = probe();
    print!("{report}");
    std::fs::write(&report_path, report).expect("the report path is writable");
}

fn probe() -> String {
    let mut report = String::from("# Cleanshot probe\n\n");
    line(
        &mut report,
        "DPI awareness (per-monitor V2)",
        outcome(&make_dpi_aware().map(|()| "set")),
    );
    report_windows(&mut report);
    report_display(&mut report);
    report_tray(&mut report);
    report_clipboard(&mut report);
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
    let monitors = monitors();
    line(report, "Monitors", monitors.len());
    for monitor in &monitors {
        let bounds = monitor.bounds;
        let dpi = match &monitor.dpi {
            Ok(dpi) => format!("{dpi} dpi ({}%)", dpi * 100 / 96),
            Err(error) => format!("dpi error: {error}"),
        };
        line(
            report,
            &monitor.device,
            format!(
                "{}x{} at ({}, {}), {dpi}",
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
            for (hotkey, registered) in &run.hotkeys {
                let registered = registered.as_ref().map(|()| "registered");
                line(report, &format!("Hotkey {hotkey:?}"), outcome(&registered));
            }
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

fn line(report: &mut String, name: &str, value: impl Display) {
    writeln!(report, "- {name}: {value}").expect("writing to a String succeeds");
}

fn outcome<T: Display, E: Display>(result: &Result<T, E>) -> String {
    match result {
        Ok(value) => value.to_string(),
        Err(error) => format!("error: {error}"),
    }
}
