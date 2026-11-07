//! The editor's toolbar as the Windows UI tests work it: through UI Automation and real input.

use std::thread;
use std::time::{Duration, Instant};

use cleanshot_win::accessibility::{ControlKind, Element, controls_in_own_windows, drag, point_in};

// A debug build on a cold runner can take seconds to open a menu; no budget is judged here.
const PATIENCE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);

/// Picks the tool named `tool` as a person does: a click on its group's button opens the
/// group's menu (D-143), and a click on the menu's row of that name picks it.
pub fn pick_tool(tool: &str) -> Result<(), String> {
    let button = radio_buttons(tool, 1)?.remove(0);
    click(&button)?;
    let row = radio_buttons(tool, 2)?.remove(1);
    click(&row)
}

fn click(control: &Element) -> Result<(), String> {
    let middle = point_in(control.bounds, 0.5, 0.5);
    drag(middle, middle).map_err(|error| format!("the click on {} failed: {error}", control.name))
}

/// The radio buttons named `name`, once there are `count` of them, the toolbar's first.
fn radio_buttons(name: &str, count: usize) -> Result<Vec<Element>, String> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let controls = controls_in_own_windows().map_err(|error| error.to_string())?;
        let named: Vec<Element> = controls
            .into_iter()
            .filter(|control| control.kind == ControlKind::RadioButton && control.name == name)
            .collect();
        if named.len() == count {
            return Ok(named);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{count} radio buttons named {name} wanted: {named:?}"
            ));
        }
        thread::sleep(POLL);
    }
}
