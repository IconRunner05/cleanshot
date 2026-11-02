/// A control on the capture toolbar (F-05).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarControl {
    Copy,
    Save,
    Edit,
    Cancel,
}

/// The toolbar's controls in the order it shows them, Tab visits them and UI Automation lists
/// them.
pub const TOOLBAR_CONTROLS: [ToolbarControl; 4] = [
    ToolbarControl::Copy,
    ToolbarControl::Save,
    ToolbarControl::Edit,
    ToolbarControl::Cancel,
];

/// Which way Tab moves the toolbar's focus: Tab forward, Shift+Tab back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabDirection {
    Forward,
    Back,
}

impl ToolbarControl {
    /// The control Tab moves to from this one in `direction`, wrapping at either end and
    /// passing over a control that is `off` (F-57). `off` is never asked about Cancel, so a
    /// step always ends, on Cancel at the latest.
    #[must_use]
    pub fn step(self, direction: TabDirection, off: impl Fn(Self) -> bool) -> Self {
        let mut next = self.moved(direction);
        while next != Self::Cancel && off(next) {
            next = next.moved(direction);
        }
        next
    }

    fn moved(self, direction: TabDirection) -> Self {
        let count = TOOLBAR_CONTROLS.len();
        let at = self as usize;
        TOOLBAR_CONTROLS[match direction {
            TabDirection::Forward => at + 1,
            TabDirection::Back => at + count - 1,
        } % count]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f05_the_toolbar_lists_each_control_once_in_the_order_it_is_declared() {
        for (at, control) in TOOLBAR_CONTROLS.into_iter().enumerate() {
            assert_eq!(control as usize, at);
        }
    }

    #[test]
    fn f05_tab_wraps_at_either_end() {
        let on = |_| false;
        assert_eq!(
            ToolbarControl::Cancel.step(TabDirection::Forward, on),
            ToolbarControl::Copy
        );
        assert_eq!(
            ToolbarControl::Copy.step(TabDirection::Back, on),
            ToolbarControl::Cancel
        );
    }

    #[test]
    fn f57_with_every_other_control_off_tab_lands_on_cancel() {
        let off = |control| {
            assert_ne!(control, ToolbarControl::Cancel, "Cancel is never off");
            true
        };
        for control in TOOLBAR_CONTROLS {
            assert_eq!(
                control.step(TabDirection::Forward, off),
                ToolbarControl::Cancel
            );
            assert_eq!(
                control.step(TabDirection::Back, off),
                ToolbarControl::Cancel
            );
        }
    }

    #[test]
    fn f57_tab_passes_over_save_while_saving_is_off() {
        let off = |control| control == ToolbarControl::Save;
        assert_eq!(
            ToolbarControl::Copy.step(TabDirection::Forward, off),
            ToolbarControl::Edit
        );
        assert_eq!(
            ToolbarControl::Edit.step(TabDirection::Back, off),
            ToolbarControl::Copy
        );
    }
}
