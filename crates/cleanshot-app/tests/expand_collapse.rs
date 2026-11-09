//! A screen reader's Expand and Collapse open and close the editor's menus and popups,
//! and the open state UI Automation reports is the one drawn, also after a mouse click. Run as
//! for `accessibility`: GPUI owns the window's thread, and the checks run on a second thread
//! that ends the process with the verdict.

#[expect(dead_code, reason = "each test uses part of the harness")]
mod on_screen;

use cleanshot_win::accessibility::{ControlKind, Element, expand_in_own_windows};
use on_screen::{check_editor, click, grey, pick_tool, poll};

const TEST: &str = "expand_and_collapse_open_and_close_the_editor_popups";
const COLOUR: &str = "Colour";
const PALETTE_SWATCH: &str = "Custom colour";
const MORE: &str = "More";
const MORE_ROW: &str = "Save As";

fn main() {
    check_editor(TEST, grey(1000, 700), check_popups);
}

fn check_popups() -> Result<(), String> {
    pick_tool("Arrow")?;
    expand(COLOUR, true)?;
    shown(PALETTE_SWATCH, true, "Expand on Colour opened no palette")?;
    expand(COLOUR, false)?;
    shown(
        PALETTE_SWATCH,
        false,
        "Collapse on Colour left the palette open",
    )?;
    // UI Automation refuses Collapse on a button it reads as collapsed.
    click(&control(ControlKind::Button, COLOUR)?)?;
    shown(PALETTE_SWATCH, true, "a click on Colour opened no palette")?;
    expand(COLOUR, false).map_err(|reason| {
        format!("after a click opened it, Colour was not reported open: {reason}")
    })?;
    shown(
        PALETTE_SWATCH,
        false,
        "Collapse after a click left the palette open",
    )?;
    expand(MORE, true)?;
    shown(MORE_ROW, true, "Expand on More opened no menu")?;
    expand(MORE, false)?;
    shown(MORE_ROW, false, "Collapse on More left the menu open")
}

fn expand(name: &str, expanded: bool) -> Result<(), String> {
    match expand_in_own_windows(name, expanded) {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!("no button named {name}")),
        Err(error) => Err(format!("expanded {expanded} on {name} failed: {error}")),
    }
}

fn control(kind: ControlKind, name: &str) -> Result<Element, String> {
    poll(|controls| {
        controls
            .iter()
            .find(|control| control.kind == kind && control.name == name)
            .cloned()
            .ok_or_else(|| format!("no {kind:?} named {name}"))
    })
}

/// Waits until a control named `name` is there (`present`) or gone, failing with `otherwise`.
fn shown(name: &str, present: bool, otherwise: &str) -> Result<(), String> {
    poll(|controls| {
        let there = controls.iter().any(|control| control.name == name);
        (there == present)
            .then_some(())
            .ok_or_else(|| otherwise.to_owned())
    })
}
