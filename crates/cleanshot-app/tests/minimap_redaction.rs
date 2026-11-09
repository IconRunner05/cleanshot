//! The minimap shows nothing a redaction hides, read from the screen's pixels. Run as
//! for `accessibility`: GPUI owns the window's thread, and the checks run on a second thread
//! that ends the process with the verdict.

#[expect(dead_code, reason = "each test uses part of the harness")]
mod on_screen;

use cleanshot_core::{Rect, RgbaImage};
use cleanshot_win::accessibility::{Element, drag, point_in};
use cleanshot_win::capture::{Method, MonitorImage, capture_all};
use on_screen::{check_editor, click, finish, poll};

const TEST: &str = "the_minimap_shows_nothing_a_redaction_hides";
const SMALLEST_WIDTH: u32 = 1800;
const SMALLEST_HEIGHT: u32 = 1200;
const PAST_THE_MONITOR: u32 = 200;
const BAR_WIDTH: u32 = 32;
const STRONGEST: &str = "48 pixels";
const REDACTED_PERCENT: (i32, i32) = (36, 44);
const OPEN_PERCENT: (i32, i32) = (56, 64);
// The redaction spans at least 306 image pixels, split into near-equal cells at least 41 wide.
// Over 32-pixel bars such a cell is between 9/41 and 32/41 white, a spread of 143 of 255.
const MOST_UNDER_REDACTION: u8 = 160;
const LEAST_IN_THE_OPEN: u8 = 180;

fn main() {
    let (width, height) = image_size().unwrap_or_else(|reason| finish(TEST, Err(reason)));
    let bars = black_and_white_bars(width, height);
    check_editor(TEST, bars, redact_then_read_the_minimap);
}

/// Larger than any monitor, so Actual size always overflows the window and the minimap shows.
fn image_size() -> Result<(u32, u32), String> {
    let monitors = capture_all(Method::GdiBitBlt).map_err(|error| error.to_string())?;
    Ok(monitors.iter().fold(
        (SMALLEST_WIDTH, SMALLEST_HEIGHT),
        |(width, height), monitor| {
            (
                width.max(monitor.frame.width() + PAST_THE_MONITOR),
                height.max(monitor.frame.height() + PAST_THE_MONITOR),
            )
        },
    ))
}

fn black_and_white_bars(width: u32, height: u32) -> RgbaImage {
    let row = (0..width).flat_map(|x| {
        let level = if (x / BAR_WIDTH).is_multiple_of(2) {
            0
        } else {
            255
        };
        [level, level, level, 255]
    });
    let pixels = row.cycle().take((width * height * 4) as usize).collect();
    RgbaImage::new(width, height, pixels).expect("the bars' bytes match their size")
}

fn redact_then_read_the_minimap() -> Result<(), String> {
    click(&named("Redact")?)?;
    click(&strongest_choice()?)?;
    let screenshot = named("Screenshot")?;
    drag(
        point_in(screenshot.bounds, 0.30, 0.35),
        point_in(screenshot.bounds, 0.47, 0.65),
    )
    .map_err(|error| format!("the drag that redacts failed: {error}"))?;
    click(&named("More")?)?;
    click(&named("Actual size")?)?;
    poll(|controls| {
        let minimap = controls
            .iter()
            .find(|control| control.name == "Minimap")
            .ok_or_else(|| "no minimap is shown".to_owned())?;
        let monitors = capture_all(Method::GdiBitBlt).map_err(|error| error.to_string())?;
        let redacted = contrast(&monitors, minimap.bounds, REDACTED_PERCENT)?;
        let open = contrast(&monitors, minimap.bounds, OPEN_PERCENT)?;
        if redacted <= MOST_UNDER_REDACTION && open >= LEAST_IN_THE_OPEN {
            println!("contrast under the redaction {redacted}, beside it {open}");
            Ok(())
        } else {
            Err(format!(
                "contrast under the redaction {redacted}, at most {MOST_UNDER_REDACTION} wanted; \
                 beside it {open}, at least {LEAST_IN_THE_OPEN} wanted"
            ))
        }
    })
}

/// The strip shows the choice itself when the caption row has room, else under More properties.
fn strongest_choice() -> Result<Element, String> {
    named(STRONGEST).or_else(|_| {
        click(&named("More properties")?)?;
        named(STRONGEST)
    })
}

fn named(name: &str) -> Result<Element, String> {
    poll(|controls| {
        controls
            .iter()
            .find(|control| control.name == name)
            .cloned()
            .ok_or_else(|| {
                let every: Vec<&str> = controls.iter().map(|shown| shown.name.as_str()).collect();
                format!("no control named {name} among {every:?}")
            })
    })
}

/// How far the brightest and darkest pixels differ along the middle row of `minimap`, between
/// `percent`'s two distances across it.
fn contrast(monitors: &[MonitorImage], minimap: Rect, percent: (i32, i32)) -> Result<u8, String> {
    let monitor = monitors
        .iter()
        .find(|monitor| {
            monitor.bounds.left <= minimap.left
                && minimap.right <= monitor.bounds.right
                && monitor.bounds.top <= minimap.top
                && minimap.bottom <= monitor.bounds.bottom
        })
        .ok_or_else(|| format!("no monitor holds the minimap at {minimap:?}"))?;
    let index = |offset: i32| {
        usize::try_from(offset)
            .map_err(|_| format!("the minimap at {minimap:?} is off its monitor"))
    };
    let across = |share: i32| minimap.left + (minimap.right - minimap.left) * share / 100;
    let row = index(i32::midpoint(minimap.top, minimap.bottom) - monitor.bounds.top)?;
    let first = index(across(percent.0) - monitor.bounds.left)?;
    let last = index(across(percent.1) - monitor.bounds.left)?;
    let row_start = row * monitor.frame.width() as usize * 4;
    let pixels = monitor.frame.pixels();
    let (darkest, brightest) = (first..last).fold((u8::MAX, u8::MIN), |(darkest, brightest), x| {
        let green = pixels[row_start + x * 4 + 1];
        (darkest.min(green), brightest.max(green))
    });
    Ok(brightest.saturating_sub(darkest))
}
