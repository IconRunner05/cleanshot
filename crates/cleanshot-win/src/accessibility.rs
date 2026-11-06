use std::ffi::c_void;
use std::num::NonZeroIsize;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use cleanshot_core::{Point, Rect};
use windows::Win32::Foundation::{E_INVALIDARG, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationElement6,
    IUIAutomationInvokePattern, IUIAutomationTogglePattern, TreeScope_Subtree,
    UIA_ButtonControlTypeId, UIA_CONTROLTYPE_ID, UIA_CheckBoxControlTypeId, UIA_GroupControlTypeId,
    UIA_ImageControlTypeId, UIA_InvokePatternId, UIA_IsExpandCollapsePatternAvailablePropertyId,
    UIA_IsValuePatternAvailablePropertyId, UIA_RadioButtonControlTypeId, UIA_SliderControlTypeId,
    UIA_TogglePatternId, UIA_ToolBarControlTypeId, UIA_ValueValuePropertyId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, HTCAPTION, HTCLIENT, HTCLOSE, HTMAXBUTTON, HTMINBUTTON,
    PostMessageW, SMTO_ABORTIFHUNG, SMTO_ERRORONEXIT, SendMessageTimeoutW, SetCursorPos, WM_CLOSE,
    WM_MOUSEMOVE, WM_NCHITTEST,
};
use windows::core::{BOOL, BSTR, Interface};

use crate::Error;

/// One element of a window's UI Automation tree, as a screen reader is told it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub kind: ControlKind,
    pub name: String,
    pub enabled: bool,
    /// What a screen reader adds after the name, such as why the control is off.
    pub help_text: String,
    /// What a screen reader reads as the control's longer description (`FullDescription`).
    pub description: String,
    /// Who answers UI Automation for it, as `ProviderDescription` names them.
    pub provider: String,
    /// Whether it opens and closes more controls, such as a menu, as a screen reader is told.
    pub expandable: bool,
    /// What a screen reader reads as its value, for a control that has one.
    pub value: Option<String>,
    /// Where it is on the desktop.
    pub bounds: Rect,
}

/// The point of `bounds` `across` of the way from its left edge and `down` of the way from its
/// top.
///
/// # Panics
///
/// If `across` or `down` is outside 0 to 1, which would name a point off the control.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a fraction from 0 to 1 of a screen's width stays inside i32"
)]
pub fn point_in(bounds: Rect, across: f32, down: f32) -> Point {
    let along = |from: i32, to: i32, part: f32| {
        assert!(
            (0.0..=1.0).contains(&part),
            "a point inside the control is 0 to 1 of the way across it, not {part}"
        );
        from + (f64::from(to - from - 1) * f64::from(part)).round() as i32
    };
    Point {
        x: along(bounds.left, bounds.right, across),
        y: along(bounds.top, bounds.bottom, down),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    ToolBar,
    Group,
    Button,
    RadioButton,
    CheckBox,
    Image,
    Slider,
    /// Any other `UIA_*ControlTypeId`.
    Other(i32),
}

/// Why [`wait_for_container`] found no container, with what a failing test should print.
#[derive(Debug)]
pub enum ContainerMissing {
    /// No signal came that the container's window was shown.
    NotShown(RecvTimeoutError),
    /// The window was shown `shown` after the start, and `walks` walks over the patience
    /// that followed never found the container. `last` is what the last walk reported.
    NotFound {
        walks: u32,
        shown: Duration,
        window_seen: Option<Duration>,
        last: Result<Vec<Element>, Error>,
    },
}

// A debug build on a cold CI runner can take over 10 s to its first frame. No budget is judged
// here, so the wait only bounds a hang.
const PATIENCE: Duration = Duration::from_secs(60);

/// The `kind` controls, in order, that UI Automation reports once a `container` control named
/// `name` (such as the toolbar named Capture) appears under this process's windows (F-05,
/// F-58). The walk starts when `shown` says the window was shown, as no user reaches it before
/// that, and goes on every 250 ms for up to a minute; `start` dates the times in the error.
/// Call it from a thread that owns no window: a window answers UI Automation on its own
/// thread, so a walk from that thread waits on itself.
///
/// # Errors
///
/// [`ContainerMissing`] if the window was not shown, or the container not found, within a minute.
pub fn wait_for_container(
    container: ControlKind,
    name: &str,
    kind: ControlKind,
    start: Instant,
    shown: &Receiver<Instant>,
) -> Result<Vec<Element>, ContainerMissing> {
    let shown = shown
        .recv_timeout(PATIENCE)
        .map_err(ContainerMissing::NotShown)?;
    let deadline = Instant::now() + PATIENCE;
    let mut walks = 0;
    let mut window_seen = None;
    loop {
        walks += 1;
        let walked = controls_in_own_windows();
        if let Ok(controls) = &walked {
            if controls
                .iter()
                .any(|control| control.kind == container && control.name == name)
            {
                return Ok(controls
                    .iter()
                    .filter(|control| control.kind == kind)
                    .cloned()
                    .collect());
            }
            if !controls.is_empty() {
                window_seen.get_or_insert(start.elapsed());
            }
        }
        if Instant::now() >= deadline {
            return Err(ContainerMissing::NotFound {
                walks,
                shown: shown - start,
                window_seen,
                last: walked,
            });
        }
        thread::sleep(Duration::from_millis(250));
    }
}

/// Every element UI Automation reports under this process's top-level windows, the windows
/// themselves included (F-58). Call it from a thread that owns no window, as for
/// [`wait_for_container`].
///
/// # Errors
///
/// [`Error::Accessibility`] if COM or the walk fails.
pub fn controls_in_own_windows() -> Result<Vec<Element>, Error> {
    // SAFETY: COM is initialised for this thread and released below once the walk ends.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(Error::Accessibility)?;
    let controls = walk_own_windows().map_err(Error::Accessibility);
    // SAFETY: balances the successful CoInitializeEx above, after every COM object is dropped.
    unsafe { CoUninitialize() };
    controls
}

/// Presses the first button named `name` under this process's top-level windows through its
/// Invoke pattern, as a screen reader's activate command does. Returns whether one was found.
/// Call it from a thread that owns no window, as for [`wait_for_container`].
///
/// # Errors
///
/// [`Error::Accessibility`] if COM, UI Automation or the button refuses a call.
pub fn invoke_in_own_windows(name: &str) -> Result<bool, Error> {
    act_on_own_control(UIA_ButtonControlTypeId, name, |element| {
        // SAFETY: the element is live, and the pattern asked for is the interface cast to.
        unsafe {
            element
                .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)?
                .Invoke()
        }
    })
}

/// Flips the first check box named `name` under this process's top-level windows through its
/// Toggle pattern, as a screen reader's activate command does. Returns whether one was found.
/// Call it from a thread that owns no window, as for [`wait_for_container`].
///
/// # Errors
///
/// [`Error::Accessibility`] if COM, UI Automation or the check box refuses a call.
pub fn toggle_in_own_windows(name: &str) -> Result<bool, Error> {
    act_on_own_control(UIA_CheckBoxControlTypeId, name, |element| {
        // SAFETY: the element is live, and the pattern asked for is the interface cast to.
        unsafe {
            element
                .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)?
                .Toggle()
        }
    })
}

/// Asks `window` to close as its close button does, by posting it `WM_CLOSE`, and returns
/// without waiting for it to close.
///
/// # Errors
///
/// [`Error::CloseNotPosted`] if Windows refuses the post, as for a window already gone.
pub fn ask_to_close(window: NonZeroIsize) -> Result<(), Error> {
    // SAFETY: WM_CLOSE carries no pointers; a handle to no window fails the post.
    unsafe {
        PostMessageW(
            Some(HWND(window.get() as *mut c_void)),
            WM_CLOSE,
            WPARAM(0),
            LPARAM(0),
        )
    }
    .map_err(Error::CloseNotPosted)
}

/// What part of a window a point is over, as the window answers Windows' `WM_NCHITTEST`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitArea {
    Close,
    Maximize,
    Minimize,
    Caption,
    Client,
    /// Any other `HT` code, as the window returned it.
    Other(isize),
}

/// D-164: whether a window capture offers `window` to pick, as it is now.
#[must_use]
pub fn offered_to_window_pick(window: NonZeroIsize) -> bool {
    crate::capture::pickable(HWND(window.get() as *mut c_void))
}

/// Asks `window` what part of it the point `client` is over, in physical pixels from the
/// top-left of its client area, as Windows asks it once the pointer has moved there. It first
/// tells the window the pointer moved to that point, so the window's hover state changes as a
/// real move would change it.
///
/// # Errors
///
/// [`Error::HitTestFailed`] if the point is off any screen Windows can name, or the window
/// does not answer within 5 s.
pub fn hit_test(window: NonZeroIsize, client: (i32, i32)) -> Result<HitArea, Error> {
    let window = HWND(window.get() as *mut c_void);
    let (Ok(client_x), Ok(client_y)) = (i16::try_from(client.0), i16::try_from(client.1)) else {
        return Err(Error::HitTestFailed(E_INVALIDARG.into()));
    };
    // GPUI answers WM_NCHITTEST from the last WM_MOUSEMOVE, not from the point it carries.
    send(window, WM_MOUSEMOVE, packed_point(client_x, client_y))?;
    let mut point = POINT {
        x: client.0,
        y: client.1,
    };
    // SAFETY: `point` is writable; a handle to no window fails the call.
    if !unsafe { ClientToScreen(window, &raw mut point) }.as_bool() {
        return Err(Error::HitTestFailed(windows::core::Error::from_thread()));
    }
    let (Ok(x), Ok(y)) = (i16::try_from(point.x), i16::try_from(point.y)) else {
        return Err(Error::HitTestFailed(E_INVALIDARG.into()));
    };
    let code = send(window, WM_NCHITTEST, packed_point(x, y))?.cast_signed();
    Ok(match u32::try_from(code) {
        Ok(HTCLOSE) => HitArea::Close,
        Ok(HTMAXBUTTON) => HitArea::Maximize,
        Ok(HTMINBUTTON) => HitArea::Minimize,
        Ok(HTCAPTION) => HitArea::Caption,
        Ok(HTCLIENT) => HitArea::Client,
        Ok(_) | Err(_) => HitArea::Other(code),
    })
}

/// Sends `message` with `point` to `window` and returns its answer, waiting as [`hit_test`]
/// does.
fn send(window: HWND, message: u32, point: LPARAM) -> Result<usize, Error> {
    let mut answer = 0usize;
    // SAFETY: both messages carry the point by value; `answer` is writable.
    let sent = unsafe {
        SendMessageTimeoutW(
            window,
            message,
            WPARAM(0),
            point,
            SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT,
            HIT_TEST_TIMEOUT_MS,
            Some(&raw mut answer),
        )
    };
    if sent.0 == 0 {
        return Err(Error::HitTestFailed(windows::core::Error::from_thread()));
    }
    Ok(answer)
}

/// A point as `WM_NCHITTEST`'s and `WM_MOUSEMOVE`'s `lParam` carry it: x in the low word, y
/// in the high word, each a signed 16-bit value, so a monitor left of or above the primary
/// keeps its sign.
fn packed_point(x: i16, y: i16) -> LPARAM {
    LPARAM((isize::from(y) << 16) | (isize::from(x) & 0xFFFF))
}

/// How long [`hit_test`] waits for a window on another thread to answer.
const HIT_TEST_TIMEOUT_MS: u32 = 5_000;

/// Drags with the left button from `from` to `to`, in physical screen pixels, as a person
/// would: the system pointer moves to `from`, the button goes down, the pointer moves to
/// `to` in even steps, and the button comes up there. Windows sends the input to whatever
/// window is under the pointer, so that window must be in front. Call it from a thread
/// that owns no window.
///
/// # Errors
///
/// [`Error::DragRefused`] if Windows does not take a press or a move.
pub fn drag(from: Point, to: Point) -> Result<(), Error> {
    move_pointer(from)?;
    press(MOUSEEVENTF_LEFTDOWN)?;
    let moved = (1..=DRAG_STEPS).try_for_each(|step| {
        move_pointer(Point {
            x: from.x + (to.x - from.x) * step / DRAG_STEPS,
            y: from.y + (to.y - from.y) * step / DRAG_STEPS,
        })
    });
    // Released whatever the moves did, so no later test starts with the button down.
    let released = press(MOUSEEVENTF_LEFTUP);
    moved.and(released)
}

/// How many moves [`drag`] makes between its press and its release.
const DRAG_STEPS: i32 = 8;
/// How long [`drag`] leaves the window to handle each step before the next.
const DRAG_STEP_WAIT: Duration = Duration::from_millis(30);

fn move_pointer(to: Point) -> Result<(), Error> {
    // SAFETY: moving the pointer reads and writes no memory of ours.
    unsafe { SetCursorPos(to.x, to.y) }.map_err(Error::DragRefused)?;
    thread::sleep(DRAG_STEP_WAIT);
    Ok(())
}

fn press(button: MOUSE_EVENT_FLAGS) -> Result<(), Error> {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: button,
                ..Default::default()
            },
        },
    };
    let size = i32::try_from(size_of::<INPUT>()).expect("INPUT fits in i32");
    // SAFETY: `input` is one mouse event of the size passed.
    if unsafe { SendInput(&[input], size) } != 1 {
        return Err(Error::DragRefused(windows::core::Error::from_thread()));
    }
    thread::sleep(DRAG_STEP_WAIT);
    Ok(())
}

fn act_on_own_control(
    kind: UIA_CONTROLTYPE_ID,
    name: &str,
    act: impl FnOnce(&IUIAutomationElement) -> windows::core::Result<()>,
) -> Result<bool, Error> {
    // SAFETY: COM is initialised for this thread and released below once the call ends.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(Error::Accessibility)?;
    let acted = find_own_control(kind, name)
        .and_then(|found| found.map(|element| act(&element)).transpose())
        .map(|acted| acted.is_some())
        .map_err(Error::Accessibility);
    // SAFETY: balances the successful CoInitializeEx above, after every COM object is dropped.
    unsafe { CoUninitialize() };
    acted
}

fn find_own_control(
    kind: UIA_CONTROLTYPE_ID,
    name: &str,
) -> windows::core::Result<Option<IUIAutomationElement>> {
    // SAFETY: CUIAutomation is the system class for IUIAutomation, created in process.
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }?;
    for window in own_windows()? {
        // SAFETY: `window` is a top-level window handle; UI Automation checks it is live.
        let found = unsafe {
            let root = automation.ElementFromHandle(window)?;
            root.FindAll(TreeScope_Subtree, &automation.CreateTrueCondition()?)?
        };
        // SAFETY: a read of the array's own length.
        for index in 0..unsafe { found.Length() }? {
            // SAFETY: `index` is below the array's length; the element is live.
            let element = unsafe { found.GetElement(index) }?;
            // SAFETY: plain property reads of a live element.
            let (found_kind, named) =
                unsafe { (element.CurrentControlType()?, element.CurrentName()?) };
            if found_kind == kind && named == name {
                return Ok(Some(element));
            }
        }
    }
    Ok(None)
}

fn walk_own_windows() -> windows::core::Result<Vec<Element>> {
    // SAFETY: CUIAutomation is the system class for IUIAutomation, created in process.
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }?;
    let mut controls = Vec::new();
    for window in own_windows()? {
        // SAFETY: `window` is a top-level window handle; UI Automation checks it is live.
        let found = unsafe {
            let root = automation.ElementFromHandle(window)?;
            root.FindAll(TreeScope_Subtree, &automation.CreateTrueCondition()?)?
        };
        // SAFETY: a read of the array's own length.
        for index in 0..unsafe { found.Length() }? {
            // SAFETY: `index` is below the array's length.
            controls.push(read(&unsafe { found.GetElement(index) }?)?);
        }
    }
    Ok(controls)
}

fn read(element: &IUIAutomationElement) -> windows::core::Result<Element> {
    // SAFETY: plain property reads of a live element.
    let (kind, name, enabled, help_text, description, provider, expandable) = unsafe {
        (
            element.CurrentControlType()?,
            element.CurrentName()?,
            element.CurrentIsEnabled()?.as_bool(),
            element.CurrentHelpText()?,
            element
                .cast::<IUIAutomationElement6>()?
                .CurrentFullDescription()?,
            element.CurrentProviderDescription()?,
            element.GetCurrentPropertyValue(UIA_IsExpandCollapsePatternAvailablePropertyId)?,
        )
    };
    // SAFETY: plain property reads of a live element.
    let (has_value, rect) = unsafe {
        (
            element.GetCurrentPropertyValue(UIA_IsValuePatternAvailablePropertyId)?,
            element.CurrentBoundingRectangle()?,
        )
    };
    let value = if bool::try_from(&has_value)? {
        // SAFETY: a plain property read of a live element that has a value.
        let value = unsafe { element.GetCurrentPropertyValue(UIA_ValueValuePropertyId) }?;
        Some(BSTR::try_from(&value)?.to_string())
    } else {
        None
    };
    let kind = [
        (UIA_ToolBarControlTypeId, ControlKind::ToolBar),
        (UIA_GroupControlTypeId, ControlKind::Group),
        (UIA_ButtonControlTypeId, ControlKind::Button),
        (UIA_RadioButtonControlTypeId, ControlKind::RadioButton),
        (UIA_CheckBoxControlTypeId, ControlKind::CheckBox),
        (UIA_ImageControlTypeId, ControlKind::Image),
        (UIA_SliderControlTypeId, ControlKind::Slider),
    ]
    .into_iter()
    .find_map(|(id, known)| (id == kind).then_some(known))
    .unwrap_or(ControlKind::Other(kind.0));
    Ok(Element {
        kind,
        name: name.to_string(),
        enabled,
        help_text: help_text.to_string(),
        description: description.to_string(),
        provider: provider.to_string(),
        expandable: bool::try_from(&expandable)?,
        value,
        bounds: Rect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        },
    })
}

fn own_windows() -> windows::core::Result<Vec<HWND>> {
    let mut windows: Vec<HWND> = Vec::new();
    let target = LPARAM(std::ptr::from_mut(&mut windows) as isize);
    // SAFETY: the callback runs only during EnumWindows, while `windows` is borrowed here.
    unsafe { EnumWindows(Some(collect_own), target) }?;
    Ok(windows)
}

extern "system" fn collect_own(window: HWND, windows: LPARAM) -> BOOL {
    let mut process = 0;
    // SAFETY: `window` comes from EnumWindows and `process` is writable.
    unsafe { GetWindowThreadProcessId(window, Some(&raw mut process)) };
    // SAFETY: a read of this process's id.
    if process == unsafe { GetCurrentProcessId() } {
        // SAFETY: own_windows passes a pointer to its Vec, alive for the whole EnumWindows
        // call, and nothing else touches it meanwhile. EnumWindows calls this only during our
        // call, with our own LPARAM; no window message carries it, so no other process can
        // supply it.
        // ast-grep-ignore: pointer-from-handle-value
        unsafe { &mut *(windows.0 as *mut Vec<HWND>) }.push(window);
    }
    true.into()
}
/// A UI Automation client for tests that hears the changes a window raises, as a screen
/// reader does.
#[cfg(test)]
#[expect(
    clippy::inline_always,
    clippy::ref_as_ptr,
    reason = "in the code #[implement] generates"
)]
pub(crate) mod listening {
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::thread;
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{
        CUIAutomation8, IUIAutomation, IUIAutomation5, IUIAutomationElement,
        IUIAutomationEventHandler, IUIAutomationEventHandler_Impl,
        IUIAutomationFocusChangedEventHandler, IUIAutomationFocusChangedEventHandler_Impl,
        IUIAutomationNotificationEventHandler, IUIAutomationNotificationEventHandler_Impl,
        IUIAutomationPropertyChangedEventHandler, IUIAutomationPropertyChangedEventHandler_Impl,
        IUIAutomationValuePattern, NotificationKind, NotificationProcessing, TreeScope_Subtree,
        UIA_EVENT_ID, UIA_PROPERTY_ID, UIA_SelectionItem_ElementSelectedEventId,
        UIA_ValuePatternId, UIA_ValueValuePropertyId,
    };
    use windows::core::{BSTR, Interface, Ref, Result, implement};

    type Heard = Arc<Mutex<Vec<String>>>;

    #[implement(
        IUIAutomationFocusChangedEventHandler,
        IUIAutomationEventHandler,
        IUIAutomationPropertyChangedEventHandler,
        IUIAutomationNotificationEventHandler
    )]
    struct Listener {
        heard: Heard,
    }

    impl Listener {
        fn note(&self, what: &str, sender: &IUIAutomationElement) -> Result<()> {
            // SAFETY: a property read of a live element.
            let name = unsafe { sender.CurrentName() }?;
            self.heard
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("{what} {name}"));
            Ok(())
        }
    }

    impl IUIAutomationFocusChangedEventHandler_Impl for Listener_Impl {
        fn HandleFocusChangedEvent(&self, sender: Ref<IUIAutomationElement>) -> Result<()> {
            self.note("focus", sender.ok()?)
        }
    }

    impl IUIAutomationEventHandler_Impl for Listener_Impl {
        fn HandleAutomationEvent(
            &self,
            sender: Ref<IUIAutomationElement>,
            event: UIA_EVENT_ID,
        ) -> Result<()> {
            if event == UIA_SelectionItem_ElementSelectedEventId {
                self.note("selected", sender.ok()?)?;
            }
            Ok(())
        }
    }

    impl IUIAutomationNotificationEventHandler_Impl for Listener_Impl {
        fn HandleNotificationEvent(
            &self,
            _sender: Ref<IUIAutomationElement>,
            _kind: NotificationKind,
            _processing: NotificationProcessing,
            spoken: &BSTR,
            _activity: &BSTR,
        ) -> Result<()> {
            self.heard
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("notice {spoken}"));
            Ok(())
        }
    }

    impl IUIAutomationPropertyChangedEventHandler_Impl for Listener_Impl {
        fn HandlePropertyChangedEvent(
            &self,
            sender: Ref<IUIAutomationElement>,
            property: UIA_PROPERTY_ID,
            _new: &VARIANT,
        ) -> Result<()> {
            if property != UIA_ValueValuePropertyId {
                return Ok(());
            }
            let sender = sender.ok()?;
            // SAFETY: the element is live, and the pattern asked for is the interface cast to.
            let value = unsafe {
                sender
                    .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)?
                    .CurrentValue()
            }?;
            self.heard
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("value {value}"));
            Ok(())
        }
    }

    /// What a UI Automation client heard from `window` while `act` ran, as "focus <name>",
    /// "selected <name>", "value <value>" and "notice <spoken>". The client runs on its own thread, and `pump`
    /// runs `window`'s messages for the time given, so the window can answer it.
    pub(crate) fn changes_heard(
        window: HWND,
        act: impl FnOnce(),
        pump: impl Fn(Duration),
    ) -> Vec<String> {
        let heard: Heard = Arc::default();
        let listening = Arc::clone(&heard);
        let (ready, is_ready) = channel();
        let (stop, stopped) = channel::<()>();
        let address = window.0 as usize;
        let client = thread::spawn(move || -> Result<()> {
            // SAFETY: COM is initialised for this thread and released below.
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()?;
            let listened = (|| -> Result<()> {
                // SAFETY: CUIAutomation8 is the system class for IUIAutomation and its later
                // versions; IUIAutomation5 is cast to below for notifications.
                let automation: IUIAutomation =
                    unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }?;
                let listener = Listener { heard: listening };
                let events: IUIAutomationEventHandler = listener.into();
                // SAFETY: the window is live while the test pumps it, and every handler is
                // removed before this thread ends.
                unsafe {
                    let root = automation.ElementFromHandle(HWND(address as *mut _))?;
                    automation.AddFocusChangedEventHandler(
                        None,
                        &events.cast::<IUIAutomationFocusChangedEventHandler>()?,
                    )?;
                    automation.AddAutomationEventHandler(
                        UIA_SelectionItem_ElementSelectedEventId,
                        &root,
                        TreeScope_Subtree,
                        None,
                        &events,
                    )?;
                    automation.AddPropertyChangedEventHandlerNativeArray(
                        &root,
                        TreeScope_Subtree,
                        None,
                        &events.cast::<IUIAutomationPropertyChangedEventHandler>()?,
                        &[UIA_ValueValuePropertyId],
                    )?;
                    automation
                        .cast::<IUIAutomation5>()?
                        .AddNotificationEventHandler(
                            &root,
                            TreeScope_Subtree,
                            None,
                            &events.cast::<IUIAutomationNotificationEventHandler>()?,
                        )?;
                    ready.send(()).expect("the test waits for the client");
                    stopped.recv().expect("the test stops the client");
                    automation.RemoveAllEventHandlers()
                }
            })();
            // SAFETY: balances the CoInitializeEx above, after every COM object is dropped.
            unsafe { CoUninitialize() };
            listened
        });
        let deadline = Instant::now() + Duration::from_secs(30);
        while is_ready.try_recv().is_err() {
            assert!(
                Instant::now() < deadline,
                "the client never started listening"
            );
            assert!(
                !client.is_finished(),
                "the client stopped before listening: {:?}",
                client.join()
            );
            pump(Duration::from_millis(50));
        }
        act();
        pump(Duration::from_millis(1_000));
        stop.send(()).expect("the client is waiting");
        while !client.is_finished() {
            pump(Duration::from_millis(50));
        }
        client
            .join()
            .expect("the client thread ends")
            .expect("the client listens");
        Arc::try_unwrap(heard)
            .expect("the client let go of what it heard")
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d141_a_hit_test_point_above_and_left_of_the_primary_keeps_its_sign() {
        for (x, y) in [
            (-2, -1440),
            (3449, -1),
            (-3440, 1439),
            (0, 0),
            (i16::MAX, i16::MIN),
        ] {
            let LPARAM(packed) = packed_point(x, y);
            let low = u16::try_from(packed & 0xFFFF).unwrap();
            let high = u16::try_from((packed >> 16) & 0xFFFF).unwrap();
            assert_eq!(
                (
                    i16::from_ne_bytes(low.to_ne_bytes()),
                    i16::from_ne_bytes(high.to_ne_bytes())
                ),
                (x, y),
                "GET_X_LPARAM and GET_Y_LPARAM read back the point"
            );
        }
    }
}
