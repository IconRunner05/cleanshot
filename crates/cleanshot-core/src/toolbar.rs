use crate::CaptureMode;

/// A control on the capture toolbar or the capture panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarControl {
    Mode(CaptureMode),
    Timer,
    Options,
    Capture,
    Copy,
    Save,
    Edit,
    Cancel,
}

/// Where the toolbar is shown: on the capture panel before a selection, or on the
/// frozen screen after one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toolbar {
    Panel,
    Overlay,
}

/// Which way Tab moves the toolbar's focus: Tab forward, Shift+Tab back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabDirection {
    Forward,
    Back,
}

const PANEL: [ToolbarControl; 7] = [
    ToolbarControl::Mode(CaptureMode::Region),
    ToolbarControl::Mode(CaptureMode::Window),
    ToolbarControl::Mode(CaptureMode::FullScreen),
    ToolbarControl::Timer,
    ToolbarControl::Options,
    ToolbarControl::Capture,
    ToolbarControl::Cancel,
];

const OVERLAY: [ToolbarControl; 5] = [
    ToolbarControl::Options,
    ToolbarControl::Copy,
    ToolbarControl::Save,
    ToolbarControl::Edit,
    ToolbarControl::Cancel,
];

impl Toolbar {
    /// The controls in the order the toolbar shows them, Tab visits them and UI Automation
    /// lists them. Cancel is in every list.
    #[must_use]
    pub fn controls(self) -> &'static [ToolbarControl] {
        match self {
            Self::Panel => &PANEL,
            Self::Overlay => &OVERLAY,
        }
    }

    /// The control Tab moves to from `from` in `direction`, wrapping at either end and
    /// passing over a control that is `off`. From a control this toolbar does not
    /// show, Tab enters at its first control, Shift+Tab at its last. `off` is never asked
    /// about Cancel, so a step always ends, on Cancel at the latest.
    #[must_use]
    pub fn step(
        self,
        from: ToolbarControl,
        direction: TabDirection,
        off: impl Fn(ToolbarControl) -> bool,
    ) -> ToolbarControl {
        let controls = self.controls();
        let count = controls.len();
        let moved = |at: usize| match direction {
            TabDirection::Forward => (at + 1) % count,
            TabDirection::Back => (at + count - 1) % count,
        };
        let mut at = match (controls.iter().position(|&shown| shown == from), direction) {
            (Some(at), _) => moved(at),
            (None, TabDirection::Forward) => 0,
            (None, TabDirection::Back) => count - 1,
        };
        while controls[at] != ToolbarControl::Cancel && off(controls[at]) {
            at = moved(at);
        }
        controls[at]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY_TOOLBAR: [Toolbar; 2] = [Toolbar::Panel, Toolbar::Overlay];
    const NONE_OFF: fn(ToolbarControl) -> bool = |_| false;

    #[test]
    fn each_toolbar_lists_each_control_once_and_cancel_last() {
        for toolbar in EVERY_TOOLBAR {
            let controls = toolbar.controls();
            for (at, control) in controls.iter().enumerate() {
                assert!(!controls[..at].contains(control), "{control:?} twice");
            }
            assert_eq!(controls.last(), Some(&ToolbarControl::Cancel));
        }
    }

    #[test]
    fn the_panel_shows_the_three_modes_then_timer_options_capture_and_cancel() {
        assert_eq!(
            Toolbar::Panel.controls(),
            [
                ToolbarControl::Mode(CaptureMode::Region),
                ToolbarControl::Mode(CaptureMode::Window),
                ToolbarControl::Mode(CaptureMode::FullScreen),
                ToolbarControl::Timer,
                ToolbarControl::Options,
                ToolbarControl::Capture,
                ToolbarControl::Cancel,
            ]
        );
    }

    #[test]
    fn tab_wraps_at_either_end_of_each_toolbar() {
        for toolbar in EVERY_TOOLBAR {
            let controls = toolbar.controls();
            let (first, last) = (controls[0], controls[controls.len() - 1]);
            assert_eq!(toolbar.step(last, TabDirection::Forward, NONE_OFF), first);
            assert_eq!(toolbar.step(first, TabDirection::Back, NONE_OFF), last);
        }
    }

    #[test]
    fn tab_from_a_control_the_toolbar_does_not_show_enters_at_either_end() {
        let elsewhere = ToolbarControl::Copy;
        assert_eq!(
            Toolbar::Panel.step(elsewhere, TabDirection::Forward, NONE_OFF),
            ToolbarControl::Mode(CaptureMode::Region)
        );
        assert_eq!(
            Toolbar::Panel.step(elsewhere, TabDirection::Back, NONE_OFF),
            ToolbarControl::Cancel
        );
    }

    #[test]
    fn with_every_other_control_off_tab_lands_on_cancel() {
        let off = |control| {
            assert_ne!(control, ToolbarControl::Cancel, "Cancel is never off");
            true
        };
        for toolbar in EVERY_TOOLBAR {
            for &control in toolbar.controls() {
                for direction in [TabDirection::Forward, TabDirection::Back] {
                    assert_eq!(
                        toolbar.step(control, direction, off),
                        ToolbarControl::Cancel
                    );
                }
            }
        }
    }

    #[test]
    fn the_overlay_shows_options_before_copy_save_edit_and_cancel() {
        assert_eq!(
            Toolbar::Overlay.controls(),
            [
                ToolbarControl::Options,
                ToolbarControl::Copy,
                ToolbarControl::Save,
                ToolbarControl::Edit,
                ToolbarControl::Cancel,
            ]
        );
    }

    #[test]
    fn tab_passes_over_save_while_saving_is_off() {
        let off = |control| control == ToolbarControl::Save;
        let overlay = Toolbar::Overlay;
        assert_eq!(
            overlay.step(ToolbarControl::Copy, TabDirection::Forward, off),
            ToolbarControl::Edit
        );
        assert_eq!(
            overlay.step(ToolbarControl::Edit, TabDirection::Back, off),
            ToolbarControl::Copy
        );
    }
}
