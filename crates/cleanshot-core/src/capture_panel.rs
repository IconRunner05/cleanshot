use crate::{CaptureMode, Timer, ToolbarControl};

/// The capture panel's mode and timer, held in memory for the session and never
/// written; it starts at Region and Off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapturePanel {
    mode: CaptureMode,
    timer: Timer,
}

/// What a press on the capture panel leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelOutcome {
    /// The panel stays open, showing its new mode or timer.
    Stay,
    /// The panel closes, and after `timer` the grab runs in `mode`.
    Capture { mode: CaptureMode, timer: Timer },
    /// The panel closes with nothing captured.
    Cancel,
}

impl CapturePanel {
    /// The mode a Capture press grabs in.
    #[must_use]
    pub fn mode(self) -> CaptureMode {
        self.mode
    }

    /// The wait a Capture press counts down before the grab.
    #[must_use]
    pub fn timer(self) -> Timer {
        self.timer
    }

    /// Presses `control`: a mode becomes the panel's mode, Timer steps the timer, and Capture
    /// or Cancel ends the panel. Options, which opens its menu, and a control the panel
    /// does not show change nothing.
    pub fn press(&mut self, control: ToolbarControl) -> PanelOutcome {
        match control {
            ToolbarControl::Mode(mode) => {
                self.mode = mode;
                PanelOutcome::Stay
            }
            ToolbarControl::Timer => {
                self.timer = self.timer.next();
                PanelOutcome::Stay
            }
            ToolbarControl::Capture => PanelOutcome::Capture {
                mode: self.mode,
                timer: self.timer,
            },
            ToolbarControl::Cancel => PanelOutcome::Cancel,
            ToolbarControl::Options
            | ToolbarControl::Copy
            | ToolbarControl::Save
            | ToolbarControl::Edit => PanelOutcome::Stay,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_starts_at_region_with_the_timer_off() {
        let panel = CapturePanel::default();
        assert_eq!(
            (panel.mode(), panel.timer()),
            (CaptureMode::Region, Timer::Off)
        );
    }

    #[test]
    fn capture_grabs_in_the_chosen_mode_after_the_chosen_wait() {
        let mut panel = CapturePanel::default();
        assert_eq!(
            panel.press(ToolbarControl::Mode(CaptureMode::Window)),
            PanelOutcome::Stay
        );
        assert_eq!(panel.press(ToolbarControl::Timer), PanelOutcome::Stay);
        assert_eq!(panel.press(ToolbarControl::Timer), PanelOutcome::Stay);
        assert_eq!(
            panel.press(ToolbarControl::Capture),
            PanelOutcome::Capture {
                mode: CaptureMode::Window,
                timer: Timer::Five,
            }
        );
    }

    #[test]
    fn the_choices_outlast_a_capture_and_a_cancel() {
        let mut panel = CapturePanel::default();
        panel.press(ToolbarControl::Mode(CaptureMode::FullScreen));
        panel.press(ToolbarControl::Timer);
        let chosen = panel;
        panel.press(ToolbarControl::Capture);
        assert_eq!(panel.press(ToolbarControl::Cancel), PanelOutcome::Cancel);
        assert_eq!(panel, chosen);
    }

    #[test]
    fn options_and_a_control_the_panel_does_not_show_change_nothing() {
        for control in [
            ToolbarControl::Options,
            ToolbarControl::Copy,
            ToolbarControl::Save,
            ToolbarControl::Edit,
        ] {
            let mut panel = CapturePanel::default();
            assert_eq!(panel.press(control), PanelOutcome::Stay);
            assert_eq!(panel, CapturePanel::default());
        }
    }
}
