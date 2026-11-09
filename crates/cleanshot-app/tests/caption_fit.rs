//! At the editor's smallest width nothing in the caption row is clipped or overlaps,
//! with Copy right after the property strip, and the property strip's groups that do
//! not fit open from "More properties". Run as for `accessibility`: GPUI owns the window's
//! thread, and the checks run on a second thread that ends the process with the verdict.

#[expect(dead_code, reason = "each test uses part of the harness")]
mod on_screen;

use cleanshot_win::accessibility::{ControlKind, Element};
use on_screen::{check_editor, click, grey, pick_tool, poll};

const TEST: &str = "nothing_in_the_caption_row_is_clipped_at_the_smallest_width";
const MORE_PROPERTIES: &str = "More properties";
const ARROW: &str = "Arrow";
const DRAG_SPACE_LEAST: f64 = 40.0;
const ROW_BUTTONS_LEAST: usize = 5;
const CAPTION_BUTTON: f64 = 46.0;

fn main() {
    // A capture smaller than the toolbar opens the editor at its smallest width.
    check_editor(TEST, grey(64, 64), check_row);
}

fn check_row() -> Result<(), String> {
    // The select tool's strip is empty, so the arrow tool fills it.
    pick_tool(ARROW)?;
    let row = settled(|controls| {
        fits(controls).err().or_else(|| {
            named(controls, ControlKind::Button, MORE_PROPERTIES)
                .is_none()
                .then(|| "at the smallest width no group went behind More properties".to_owned())
        })
    })?;
    let more =
        named(&row, ControlKind::Button, MORE_PROPERTIES).expect("settled found the chevron");
    click(more)?;
    settled(|controls| {
        named(controls, ControlKind::Group, MORE_PROPERTIES)
            .is_none()
            .then(|| format!("{MORE_PROPERTIES} opened no panel"))
    })?;
    Ok(())
}

/// The caption row's controls, once `wrong` finds nothing wrong with every control seen, which
/// it must within `PATIENCE`: the strip is fitted on the frame after it is measured.
fn settled(wrong: impl Fn(&[Element]) -> Option<String>) -> Result<Vec<Element>, String> {
    poll(|controls| match wrong(controls) {
        None => Ok(controls.to_vec()),
        Some(problem) => {
            let every: Vec<String> = controls
                .iter()
                .map(|control| {
                    format!("{:?} {:?} {:?}", control.kind, control.name, control.bounds)
                })
                .collect();
            Err(format!("{problem}; every control: {every:#?}"))
        }
    })
}

/// Whether every button in the caption row lies inside the window, clear of every other, and
/// the property strip ends before Copy.
fn fits(controls: &[Element]) -> Result<(), String> {
    let close = named(controls, ControlKind::Button, "Close").ok_or("no Close button")?;
    let copy = named(controls, ControlKind::Button, "Copy").ok_or("no Copy button")?;
    let strip = named(controls, ControlKind::ToolBar, "Properties").ok_or("no Properties strip")?;
    if strip.bounds.right > copy.bounds.left {
        return Err(format!(
            "the strip {:?} runs under Copy {:?}",
            strip.bounds, copy.bounds
        ));
    }
    // Half the drag space at least lies between "..." and Minimize, then Maximize.
    let more = named(controls, ControlKind::Button, "More").ok_or("no More button")?;
    let close_width = f64::from(close.bounds.right - close.bounds.left);
    let least = close_width * (2.0 + DRAG_SPACE_LEAST / 2.0 / CAPTION_BUTTON);
    if f64::from(close.bounds.left - more.bounds.right) < least {
        return Err(format!(
            "the drag space is not between More {:?} and the caption buttons, Close {:?}",
            more.bounds, close.bounds
        ));
    }
    let in_row = |control: &Element| {
        control.bounds.top >= close.bounds.top && control.bounds.bottom <= close.bounds.bottom
    };
    let buttons: Vec<&Element> = controls
        .iter()
        .filter(|control| {
            matches!(
                control.kind,
                ControlKind::Button | ControlKind::RadioButton | ControlKind::CheckBox
            ) && in_row(control)
        })
        .collect();
    if buttons.len() < ROW_BUTTONS_LEAST {
        return Err(format!(
            "the caption row beside Close {:?} holds {} buttons, not Copy, More and the caption buttons",
            close.bounds,
            buttons.len()
        ));
    }
    for (index, button) in buttons.iter().enumerate() {
        if button.bounds.right > close.bounds.right {
            return Err(format!(
                "{} {:?} runs past the window",
                button.name, button.bounds
            ));
        }
        let overlapped = buttons[index + 1..].iter().find(|other| {
            button.bounds.left < other.bounds.right && other.bounds.left < button.bounds.right
        });
        if let Some(other) = overlapped {
            return Err(format!(
                "{} {:?} overlaps {} {:?}",
                button.name, button.bounds, other.name, other.bounds
            ));
        }
    }
    Ok(())
}

/// The first control of `kind` named `name` that takes room on screen: UI Automation also
/// lists a zero-size Minimize, Maximize and Close before the caption row's own.
fn named<'a>(controls: &'a [Element], kind: ControlKind, name: &str) -> Option<&'a Element> {
    controls.iter().find(|control| {
        control.kind == kind
            && control.name == name
            && control.bounds.left < control.bounds.right
            && control.bounds.top < control.bounds.bottom
    })
}
