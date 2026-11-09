//! The capture toolbar reaches UI Automation, and its Copy button works from there as
//! Narrator's activate command would press it. The overlay's windows need a message loop on
//! their thread, so GPUI runs it, the walk runs on a second thread, and that thread ends the
//! process with the verdict.

#[expect(dead_code, reason = "each test uses part of the harness")]
mod on_screen;

use cleanshot_win::accessibility::invoke_in_own_windows;
use cleanshot_win::overlay::OverlayEvent;
use on_screen::{PATIENCE, SELECTED, check_toolbar};

const TEST: &str = "ui_automation_names_the_toolbar_buttons_and_copy_presses_copy";
const BUTTONS: [&str; 5] = ["Options", "Copy", "Save", "Edit", "Cancel"];

fn main() {
    check_toolbar(
        TEST,
        None,
        |event| match event {
            OverlayEvent::Copy(selected) => Some(selected),
            OverlayEvent::Cancelled
            | OverlayEvent::Save(_)
            | OverlayEvent::SaveAs(_)
            | OverlayEvent::Edit(_)
            | OverlayEvent::DrawFailed(_)
            | OverlayEvent::Options(_)
            | OverlayEvent::OptionsFailed(_) => None,
        },
        |buttons, choice| {
            let buttons: Vec<String> = buttons.into_iter().map(|button| button.name).collect();
            if buttons != BUTTONS {
                return Err(format!("buttons {buttons:?}, expected {BUTTONS:?}"));
            }
            match invoke_in_own_windows("Copy") {
                Ok(true) => {}
                Ok(false) => return Err(String::from("no button named Copy to invoke")),
                Err(error) => return Err(format!("Copy could not be invoked: {error}")),
            }
            match choice.recv_timeout(PATIENCE) {
                Ok(Some(selected)) if selected == SELECTED => Ok(()),
                Ok(other) => Err(format!("the overlay sent {other:?} for Copy")),
                Err(error) => Err(format!("the overlay sent nothing for Copy: {error}")),
            }
        },
    );
}
