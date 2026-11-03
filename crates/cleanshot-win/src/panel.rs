use std::borrow::Cow;
use std::path::PathBuf;

use cleanshot_core::{CapturePanel, OptionsEntry, OptionsItem, PanelOutcome, Timer};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, GetCursorPos};

use crate::Error;
use crate::struct_size;
use crate::toolbar::options;

mod countdown;
mod window;

pub use countdown::{Countdown, CountdownEvent};

/// What happened on the capture panel.
#[derive(Debug)]
pub enum PanelEvent {
    /// Capture or Cancel ended the panel, with the choices as they now stand (D-99). It is
    /// sent once, after the panel hid and gave the focus back.
    Closed {
        panel: CapturePanel,
        outcome: PanelOutcome,
    },
    /// Direct2D could not draw the panel.
    DrawFailed(windows::core::Error),
    /// This item was picked on the Options menu (F-12). The panel stays open.
    Options(OptionsItem),
    /// The Options menu could not show.
    OptionsFailed(windows::core::Error),
}

/// The capture panel (F-11): the toolbar shown before a selection, where the mode and the
/// timer are chosen. It takes the keyboard focus while it shows and is left out of every
/// capture (D-106).
pub struct Panel {
    window: HWND,
}

impl Panel {
    /// Shows `panel`'s choices on the monitor under the pointer, with the keyboard focus on
    /// Capture, and tells `on_event` what happens on it. Its Options button shows the menu
    /// `options` gives as it opens (F-12).
    ///
    /// # Errors
    ///
    /// [`Error::Window`] if the window or its Direct2D surface cannot be made;
    /// [`Error::Monitor`] if the pointer's monitor or its DPI cannot be read;
    /// [`Error::MonitorTooSmall`] if that monitor cannot hold the panel.
    pub fn show(
        panel: CapturePanel,
        options: impl Fn() -> Vec<OptionsEntry> + 'static,
        on_event: impl FnMut(PanelEvent) + 'static,
    ) -> Result<Self, Error> {
        window::register_class()?;
        let monitor = monitor_under_pointer().map_err(Error::Monitor)?;
        let window = window::open_window(monitor, panel, Box::new(options), Box::new(on_event))?;
        Ok(Self { window })
    }

    /// F-12 Save folder...: the folder dialog, owned by the panel, and the folder chosen, or
    /// None when it was cancelled.
    ///
    /// # Errors
    ///
    /// [`Error::Window`] if the dialog cannot show.
    pub fn choose_folder(&self) -> Result<Option<PathBuf>, Error> {
        options::choose_folder(self.window).map_err(Error::Window)
    }

    /// Brings the panel forward with the keyboard focus, as a second Ctrl+Shift+5 does.
    pub fn bring_forward(&self) {
        window::bring_forward(self.window);
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        // SAFETY: the window belongs to this thread and is destroyed once, here.
        let destroyed = unsafe { DestroyWindow(self.window) };
        debug_assert!(destroyed.is_ok(), "the panel window is destroyed once");
    }
}

fn monitor_under_pointer() -> windows::core::Result<RECT> {
    let mut pointer = POINT::default();
    // SAFETY: `pointer` is writable.
    unsafe { GetCursorPos(&raw mut pointer) }?;
    let mut info = MONITORINFO {
        cbSize: struct_size::<MONITORINFO>(),
        ..Default::default()
    };
    // SAFETY: `info` is writable with its size set; MonitorFromPoint always returns a monitor
    // here.
    unsafe {
        GetMonitorInfoW(
            MonitorFromPoint(pointer, MONITOR_DEFAULTTONEAREST),
            &raw mut info,
        )
    }
    .ok()?;
    Ok(info.rcMonitor)
}

/// The Timer control's text: "Timer" while off, else the wait in seconds (D-106).
fn timer_label(timer: Timer) -> Cow<'static, str> {
    match timer {
        Timer::Off => Cow::Borrowed("Timer"),
        Timer::Three | Timer::Five | Timer::Ten => Cow::Owned(format!("{} s", timer.seconds())),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use cleanshot_core::{CaptureMode, ToolbarControl};
    use windows::Win32::Foundation::WPARAM;
    use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_ESCAPE, VK_RETURN, VK_TAB};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetWindowRect, IsWindowVisible, MSG, PM_REMOVE, PeekMessageW,
        SendMessageW, WM_KEYDOWN,
    };

    use super::*;
    use crate::capture::{Method, capture_all, region};
    use crate::overlay::tests::screen;

    type Closed = Rc<RefCell<Option<(CapturePanel, PanelOutcome)>>>;

    fn shown(panel: CapturePanel) -> (Panel, Closed) {
        let closed: Closed = Rc::default();
        let sink = Rc::clone(&closed);
        let shown = Panel::show(panel, Vec::new, move |event| match event {
            PanelEvent::Closed { panel, outcome } => {
                assert!(
                    sink.borrow_mut().replace((panel, outcome)).is_none(),
                    "closed once"
                );
            }
            PanelEvent::DrawFailed(error) => panic!("the panel draws: {error}"),
            PanelEvent::Options(item) => panic!("no Options menu here, yet {item:?} was picked"),
            PanelEvent::OptionsFailed(error) => panic!("no Options menu here: {error}"),
        })
        .expect("the panel opens");
        pump(Duration::from_millis(100));
        (shown, closed)
    }

    fn keys(panel: &Panel, keys: &[VIRTUAL_KEY]) {
        for key in keys {
            // SAFETY: the window belongs to this thread, so this runs its procedure here.
            unsafe { SendMessageW(panel.window, WM_KEYDOWN, Some(WPARAM(key.0.into())), None) };
        }
    }

    fn pump(duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            let mut message = MSG::default();
            // SAFETY: `message` is writable and the panel window belongs to this thread.
            while unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                // SAFETY: `message` was just filled by PeekMessageW.
                unsafe { DispatchMessageW(&raw const message) };
            }
        }
    }

    #[test]
    fn f11_tab_and_enter_choose_the_mode_and_timer_and_capture_hands_them_back() {
        let _screen = screen();
        let (panel, closed) = shown(CapturePanel::default());
        // The focus opens on Capture; Tab goes Cancel, Region, Window, Screen.
        keys(&panel, &[VK_TAB, VK_TAB, VK_TAB, VK_TAB, VK_RETURN]);
        keys(&panel, &[VK_TAB, VK_RETURN]);
        assert!(
            closed.borrow().is_none(),
            "a mode or the timer keeps it open"
        );
        // Timer, then Options, to Capture.
        keys(&panel, &[VK_TAB, VK_TAB, VK_RETURN]);
        let (choices, outcome) = closed.borrow_mut().take().expect("Capture closed it");
        let capture = PanelOutcome::Capture {
            mode: CaptureMode::FullScreen,
            timer: Timer::Three,
        };
        assert_eq!(outcome, capture);
        assert_eq!(
            (choices.mode(), choices.timer()),
            (CaptureMode::FullScreen, Timer::Three)
        );
        // SAFETY: IsWindowVisible only reads the window's style.
        assert!(!unsafe { IsWindowVisible(panel.window) }.as_bool());
    }

    #[test]
    fn f12_options_shows_the_menu_and_tells_the_pick_while_the_panel_stays_open() {
        use std::cell::Cell;

        use windows::Win32::UI::Input::KeyboardAndMouse::VK_DOWN;

        use crate::toolbar::options::answering::answer_menu;

        let _screen = screen();
        let picked = Rc::new(Cell::new(None));
        let sink = Rc::clone(&picked);
        let menu = || {
            vec![OptionsEntry {
                item: OptionsItem::ShowPointer,
                checked: false,
                off: None,
            }]
        };
        let panel = Panel::show(CapturePanel::default(), menu, move |event| {
            if let PanelEvent::Options(item) = event {
                sink.set(Some(item));
            }
        })
        .expect("the panel opens");
        pump(Duration::from_millis(100));
        let answering = answer_menu(&[VK_DOWN, VK_RETURN], || {});
        // From Capture, Tab goes Cancel, Region, Window, Screen, Timer, Options.
        keys(&panel, &[VK_TAB; 6]);
        keys(&panel, &[VK_RETURN]);
        answering.join().expect("the menu was answered");
        assert_eq!(picked.get(), Some(OptionsItem::ShowPointer));
        // SAFETY: IsWindowVisible only reads the window's style.
        assert!(unsafe { IsWindowVisible(panel.window) }.as_bool());
    }

    #[test]
    fn f11_esc_cancels_once_and_keeps_the_choices() {
        let _screen = screen();
        let mut start = CapturePanel::default();
        start.press(ToolbarControl::Timer);
        let (panel, closed) = shown(start);
        keys(&panel, &[VK_TAB, VK_TAB, VK_TAB, VK_RETURN, VK_ESCAPE]);
        let (choices, outcome) = closed.borrow_mut().take().expect("Esc closed it");
        assert_eq!(outcome, PanelOutcome::Cancel);
        assert_eq!(
            (choices.mode(), choices.timer()),
            (CaptureMode::Window, Timer::Three)
        );
        keys(&panel, &[VK_ESCAPE, VK_RETURN]);
        assert!(closed.borrow().is_none(), "nothing after the panel closed");
    }

    #[cfg(feature = "ui-automation-test")]
    #[test]
    fn f11_a_screen_reader_is_told_as_the_focus_the_mode_and_the_timer_change() {
        use crate::accessibility::listening::changes_heard;

        let _screen = screen();
        let (panel, _closed) = shown(CapturePanel::default());
        let heard = changes_heard(
            panel.window,
            || {
                // From Capture: Cancel; then Region, Window, chosen; then Screen, Timer, pressed.
                keys(&panel, &[VK_TAB]);
                keys(&panel, &[VK_TAB, VK_TAB, VK_RETURN]);
                keys(&panel, &[VK_TAB, VK_TAB, VK_RETURN]);
            },
            pump,
        );
        for change in ["focus Cancel", "selected Window", "value 3 seconds"] {
            assert!(
                heard.iter().any(|each| each == change),
                "{change} unheard in {heard:?}"
            );
        }
    }

    // Both copies are taken with the panel up and nothing else changing, so the screen under
    // it is the same; only the panel's own look differs between them.
    #[test]
    fn f11_the_panel_is_left_out_of_a_capture() {
        let _screen = screen();
        let (panel, _closed) = shown(CapturePanel::default());
        let mut at = RECT::default();
        // SAFETY: `at` is writable and the window belongs to this thread.
        unsafe { GetWindowRect(panel.window, &raw mut at) }.expect("the panel has a frame");
        let before = capture_all(Method::GdiBitBlt).expect("the screen copies");
        keys(&panel, &[VK_TAB]);
        pump(Duration::from_millis(100));
        let after = capture_all(Method::GdiBitBlt).expect("the screen copies");
        assert_eq!(
            region(&before, at).expect("the panel's place is on a monitor"),
            region(&after, at).expect("the panel's place is on a monitor"),
            "the panel shows in the capture: its focus ring moved in it"
        );
    }
}
