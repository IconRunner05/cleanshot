use cleanshot_core::{HotkeyAction, Pick, Pickable, ToolbarControl, next_window, picked};
use windows::Win32::Foundation::{POINT, RECT};

use super::{Key, OverlayEvent, Shared};
use crate::capture::{screen_point, win32_rect};
use crate::toolbar::automation;

const WINDOW_TO_CAPTURE: &str = "Window to capture";

/// Whether the pointer draws a region or picks a window (F-08).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Mode {
    /// D-122: before any selection, the frame a click would pick is highlighted.
    Region { hovered: Option<Pick> },
    /// The frame the pointer or Tab is on, highlighted until a press picks it.
    Window { hovered: Option<Pick> },
}

impl Shared {
    /// F-08: in window mode Tab moves the highlight through the windows and Enter picks it.
    pub(super) fn press_picking(&self, key: Key, hovered: Option<Pick>) {
        match key {
            Key::SwitchMode => {
                self.mode.set(Mode::Region { hovered: None });
                self.changed();
            }
            Key::Focus(direction) => {
                self.hover(next_window(self.pickable.len(), hovered, direction));
            }
            Key::Press => self.pick(hovered),
            Key::Choose(ToolbarControl::Cancel) => self.send(OverlayEvent::Cancelled),
            Key::Choose(ToolbarControl::Copy | ToolbarControl::Save | ToolbarControl::Edit)
            | Key::Nudge(..)
            | Key::SaveAs => {}
            Key::Choose(
                control @ (ToolbarControl::Mode(_)
                | ToolbarControl::Timer
                | ToolbarControl::Options
                | ToolbarControl::Capture),
            ) => self.choose(control),
        }
    }

    /// What a press at `at` picks in window mode: its window, or else its monitor (D-100).
    pub(super) fn picked_at(&self, at: POINT) -> Option<Pick> {
        picked(&self.pickable, &self.monitors, screen_point(at))
    }

    /// D-122: in region mode with nothing selected, highlights what a click at `at` picks.
    pub(super) fn hover_region(&self, at: POINT) {
        let hovered = match self.selected.get() {
            Some(_) => None,
            None => self.picked_at(at),
        };
        let now = Mode::Region { hovered };
        if self.mode.replace(now) != now {
            self.changed();
        }
    }

    /// F-58: a move of the highlight is said aloud, since the overlay never takes the focus.
    pub(super) fn hover(&self, hovered: Option<Pick>) {
        let now = Mode::Window { hovered };
        if self.mode.replace(now) == now {
            return;
        }
        self.changed();
        if let (Some(pick), Some(window)) = (hovered, self.toolbar_window.get()) {
            // A failed announcement is safe to drop: the highlight has moved, and the toolbar's
            // Text element names the pick for a screen reader that looks.
            let _unannounced = automation::announce(
                window,
                &self.mirror,
                &self.spoken(pick),
                "Cleanshot.WindowToCapture",
            );
        }
    }

    /// What a screen reader says for `pick`: the window's title, or the tray's name for a
    /// capture of the whole screen.
    pub(super) fn spoken(&self, pick: Pick) -> String {
        match pick {
            Pick::Window(at) => match self.window(at).title.as_str() {
                "" => WINDOW_TO_CAPTURE.to_owned(),
                title => format!("{WINDOW_TO_CAPTURE}: {title}"),
            },
            Pick::Monitor(_) => HotkeyAction::FullScreen.name().to_owned(),
        }
    }

    fn window(&self, at: usize) -> &Pickable {
        self.pickable
            .get(at)
            .expect("a pick names a window in the overlay's list")
    }

    /// Selects what `pick` names as a region, which the toolbar then acts on as any selection.
    pub(super) fn pick(&self, pick: Option<Pick>) {
        if let Some(pick) = pick {
            self.mode.set(Mode::Region { hovered: None });
            self.select(Some(self.frame(pick)));
        }
    }

    pub(super) fn frame(&self, pick: Pick) -> RECT {
        match pick {
            Pick::Window(at) => win32_rect(self.window(at).frame),
            Pick::Monitor(bounds) => win32_rect(bounds),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use windows::Win32::Foundation::LPARAM;
    use windows::Win32::UI::WindowsAndMessaging::{
        SendMessageW, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    };

    use cleanshot_core::TabDirection;

    use super::*;
    use crate::capture::screen_rect;
    use crate::overlay::tests::{monitor, press, screen};
    use crate::overlay::{Opening, Overlay};

    /// An overlay on `monitor()` picking from `pickable`, and the selections it copies.
    fn picking(frames: Vec<RECT>) -> (Overlay, Rc<RefCell<Vec<RECT>>>) {
        let pickable = frames
            .into_iter()
            .map(|frame| Pickable {
                frame: screen_rect(frame),
                title: String::new(),
            })
            .collect();
        let copied = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&copied);
        let overlay = Overlay::show(
            &[monitor()],
            Opening {
                pickable,
                ..Opening::default()
            },
            Vec::new,
            move |event| {
                if let OverlayEvent::Copy(selected) = event {
                    sink.borrow_mut().push(selected);
                }
            },
        )
        .expect("the overlay opens");
        (overlay, copied)
    }

    #[test]
    fn f08_after_w_a_click_picks_the_window_under_the_pointer_or_else_its_monitor() {
        let _screen = screen();
        let frame = RECT {
            left: 10,
            top: 8,
            right: 40,
            bottom: 30,
        };
        let (overlay, copied) = picking(vec![frame]);
        let window = overlay.windows[0];
        for (x, y) in [(20_isize, 20_isize), (50, 40)] {
            press(window, Key::SwitchMode);
            let at = Some(LPARAM(y * 0x1_0000 + x));
            // SAFETY: each message is handled on this thread by a window created above, and
            // none carries a pointer.
            unsafe {
                SendMessageW(window, WM_MOUSEMOVE, None, at);
                SendMessageW(window, WM_LBUTTONDOWN, None, at);
                SendMessageW(window, WM_LBUTTONUP, None, at);
            }
            press(window, Key::Press);
        }
        drop(overlay);
        assert_eq!(*copied.borrow(), [frame, monitor().bounds]);
    }

    #[test]
    fn f08_an_overlay_told_to_pick_a_window_picks_the_one_clicked_with_no_w() {
        let _screen = screen();
        let frame = RECT {
            left: 10,
            top: 8,
            right: 40,
            bottom: 30,
        };
        let (overlay, copied) = picking(vec![frame]);
        overlay.pick_window();
        let window = overlay.windows[0];
        let at = Some(LPARAM(20 * 0x1_0000 + 20));
        // SAFETY: each message is handled on this thread by a window created above, and none
        // carries a pointer.
        unsafe {
            SendMessageW(window, WM_LBUTTONDOWN, None, at);
            SendMessageW(window, WM_LBUTTONUP, None, at);
        }
        press(window, Key::Press);
        drop(overlay);
        assert_eq!(*copied.borrow(), [frame]);
    }

    /// The selections copied after a press at `down`, moves through `path` and a release at
    /// its last point, in region mode with one window at `frame`.
    fn copied_after(frame: RECT, down: (isize, isize), path: &[(isize, isize)]) -> Vec<RECT> {
        let (overlay, copied) = picking(vec![frame]);
        let window = overlay.windows[0];
        let at = |(x, y): (isize, isize)| Some(LPARAM(y * 0x1_0000 + x));
        let up = path.last().copied().unwrap_or(down);
        // SAFETY: each message is handled on this thread by a window created above, and none
        // carries a pointer.
        unsafe {
            SendMessageW(window, WM_LBUTTONDOWN, None, at(down));
            for &point in path {
                SendMessageW(window, WM_MOUSEMOVE, None, at(point));
            }
            SendMessageW(window, WM_LBUTTONUP, None, at(up));
        }
        press(window, Key::Press);
        drop(overlay);
        copied.take()
    }

    const FRAME: RECT = RECT {
        left: 10,
        top: 8,
        right: 40,
        bottom: 30,
    };

    #[test]
    fn d122_a_click_in_region_mode_picks_the_window_under_the_pointer() {
        let _screen = screen();
        assert_eq!(copied_after(FRAME, (20, 20), &[]), [FRAME]);
    }

    #[test]
    fn d122_a_move_within_the_drag_distance_is_still_a_click() {
        let _screen = screen();
        assert_eq!(copied_after(FRAME, (20, 20), &[(21, 21)]), [FRAME]);
    }

    #[test]
    fn d122_a_drag_past_the_drag_distance_selects_a_region() {
        let _screen = screen();
        let region = RECT {
            left: 20,
            top: 20,
            right: 35,
            bottom: 28,
        };
        assert_eq!(copied_after(FRAME, (20, 20), &[(35, 28)]), [region]);
    }

    #[test]
    fn f08_after_w_tab_and_shift_tab_move_through_every_window_even_two_with_one_frame() {
        let _screen = screen();
        let frame = |left| RECT {
            left,
            top: 0,
            right: left + 10,
            bottom: 10,
        };
        // Two maximised windows share one frame, and Tab still moves past the first. From the
        // last window, Shift+Tab reaches the one before it, never a shared one.
        let (shared, other, last) = (frame(0), frame(20), frame(40));
        let (overlay, copied) = picking(vec![shared, shared, other, last]);
        let window = overlay.windows[0];
        for keys in [
            &[
                Key::Focus(TabDirection::Forward),
                Key::Focus(TabDirection::Forward),
                Key::Focus(TabDirection::Forward),
            ][..],
            &[
                Key::Focus(TabDirection::Back),
                Key::Focus(TabDirection::Back),
            ],
        ] {
            press(window, Key::SwitchMode);
            for &key in keys {
                press(window, key);
            }
            press(window, Key::Press);
            press(window, Key::Press);
        }
        drop(overlay);
        assert_eq!(*copied.borrow(), [other, other]);
    }

    #[test]
    #[cfg(feature = "ui-automation-test")]
    fn f58_ui_automation_names_the_window_tab_puts_the_highlight_on() {
        use std::time::Duration;

        use crate::capture::{BgraFrame, MonitorImage};
        use crate::overlay::tests::pump;

        let _screen = screen();
        let (width, height) = (800, 600);
        let desktop = MonitorImage {
            bounds: RECT {
                left: 0,
                top: 0,
                right: 800,
                bottom: 600,
            },
            frame: BgraFrame::new(width, height, vec![128; 800 * 600 * 4]).expect("800x600 BGRA"),
        };
        // One window over the whole desktop, so the pointer and Tab can only highlight it.
        let pickable = vec![Pickable {
            frame: screen_rect(desktop.bounds),
            title: "Notes".to_owned(),
        }];
        let overlay = Overlay::show(
            &[desktop],
            Opening {
                pickable,
                ..Opening::default()
            },
            Vec::new,
            |_| {},
        )
        .expect("the overlay opens");
        let window = overlay.windows[0];
        for key in [Key::SwitchMode, Key::Focus(TabDirection::Forward)] {
            press(window, key);
        }
        let walk = std::thread::spawn(crate::accessibility::controls_in_own_windows);
        while !walk.is_finished() {
            pump(Duration::from_millis(20));
        }
        let names: Vec<String> = walk
            .join()
            .expect("the walk thread ends")
            .expect("UI Automation walks the overlay")
            .into_iter()
            .map(|control| control.name)
            .collect();
        drop(overlay);
        assert!(
            names.iter().any(|name| name == "Window to capture: Notes"),
            "{names:?}"
        );
    }
}
