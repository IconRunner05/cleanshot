//! While the organisation's policy turns saving off, UI Automation reports the capture
//! toolbar's Save as not enabled, with the reason as its help text, and refuses to invoke it.
//! Run as for `toolbar_automation`: GPUI owns the overlay's thread and the walk ends the process.

#[expect(dead_code, reason = "each test uses part of the harness")]
mod on_screen;

use std::sync::mpsc;
use std::time::Duration;

use cleanshot_core::{SavingOff, saving_off_notice};
use cleanshot_win::accessibility::invoke_in_own_windows;
use on_screen::check_toolbar;

const TEST: &str = "ui_automation_reports_save_off_with_the_reason_and_refuses_to_press_it";
fn main() {
    let expected = saving_off_notice(SavingOff::ClipboardOnly);
    check_toolbar(
        TEST,
        Some(SavingOff::ClipboardOnly),
        |event| format!("{event:?}"),
        move |buttons, choice| {
            let off: Vec<_> = buttons
                .iter()
                .map(|button| {
                    (
                        button.name.as_str(),
                        button.enabled,
                        button.help_text.as_str(),
                    )
                })
                .collect();
            let wanted = [
                ("Options", true, ""),
                ("Copy", true, ""),
                ("Save", false, expected.as_str()),
                ("Edit", true, ""),
                ("Cancel", true, ""),
            ];
            if off != wanted {
                return Err(format!("buttons {off:?}, expected {wanted:?}"));
            }
            if invoke_in_own_windows("Save").is_ok() {
                return Err(String::from("UI Automation pressed the Save that is off"));
            }
            match choice.recv_timeout(Duration::from_secs(1)) {
                Err(mpsc::RecvTimeoutError::Timeout) => Ok(()),
                Ok(event) => Err(format!("the overlay sent {event} for a refused press")),
                Err(error) => Err(format!("the overlay's channel closed: {error}")),
            }
        },
    );
}
