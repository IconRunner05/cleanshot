//! The editor as the Windows UI tests work it: through UI Automation and real input.

use std::error::Error as _;
use std::thread;
use std::time::{Duration, Instant};

use cleanshot_win::accessibility::{ControlKind, Element, controls_in_own_windows, drag, point_in};

/// How long a check waits for the editor: a debug build on a cold runner can take seconds to
/// lay out a frame or open a menu, and no budget is judged here.
pub const PATIENCE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);

/// Reads the controls in this process's windows until `found` answers with a value, within
/// `PATIENCE`, or returns the last answer's reason. A read UI Automation refuses while a
/// window changes is tried again, as a not-yet answer is.
pub fn poll<T>(mut found: impl FnMut(&[Element]) -> Result<T, String>) -> Result<T, String> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let answer = match controls_in_own_windows() {
            Ok(controls) => found(&controls),
            Err(error) => Err(format!("{error}: {:?}", error.source())),
        };
        match answer {
            Ok(value) => return Ok(value),
            Err(reason) if Instant::now() >= deadline => return Err(reason),
            Err(_not_yet) => thread::sleep(POLL),
        }
    }
}

/// Picks the tool named `tool` as a person does: a click on its group's button opens the
/// group's menu (D-143), and a click on the menu's row of that name picks it.
pub fn pick_tool(tool: &str) -> Result<(), String> {
    let button = radio_buttons(tool, 1)?.remove(0);
    click(&button)?;
    let row = radio_buttons(tool, 2)?.remove(1);
    click(&row)
}

/// Clicks the middle of `control` with the left button.
pub fn click(control: &Element) -> Result<(), String> {
    let middle = point_in(control.bounds, 0.5, 0.5);
    drag(middle, middle).map_err(|error| format!("the click on {} failed: {error}", control.name))
}

/// The radio buttons named `name`, once there are `count` of them, the toolbar's first.
fn radio_buttons(name: &str, count: usize) -> Result<Vec<Element>, String> {
    poll(|controls| {
        let named: Vec<Element> = controls
            .iter()
            .filter(|control| control.kind == ControlKind::RadioButton && control.name == name)
            .cloned()
            .collect();
        if named.len() == count {
            Ok(named)
        } else {
            Err(format!(
                "{count} radio buttons named {name} wanted: {named:?}"
            ))
        }
    })
}
