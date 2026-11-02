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

impl ToolbarControl {
    /// The control `step` places after this one in Tab order, wrapping at either end and
    /// passing over a control that is `off` (F-57). Cancel is never off.
    #[must_use]
    pub fn step(self, step: i32, off: impl Fn(Self) -> bool) -> Self {
        let mut next = self.moved(step);
        while off(next) {
            next = next.moved(step.signum());
        }
        next
    }

    fn moved(self, step: i32) -> Self {
        let count = TOOLBAR_CONTROLS.len();
        let mut at = self as usize;
        for _ in 0..step.unsigned_abs() {
            at = (if step > 0 { at + 1 } else { at + count - 1 }) % count;
        }
        TOOLBAR_CONTROLS[at]
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
        assert_eq!(ToolbarControl::Cancel.step(1, on), ToolbarControl::Copy);
        assert_eq!(ToolbarControl::Copy.step(-1, on), ToolbarControl::Cancel);
    }

    #[test]
    fn f57_tab_passes_over_save_while_saving_is_off() {
        let off = |control| control == ToolbarControl::Save;
        assert_eq!(ToolbarControl::Copy.step(1, off), ToolbarControl::Edit);
        assert_eq!(ToolbarControl::Edit.step(-1, off), ToolbarControl::Copy);
        assert_eq!(ToolbarControl::Cancel.step(2, off), ToolbarControl::Edit);
    }
}
