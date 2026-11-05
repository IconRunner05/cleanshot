use std::ffi::c_void;
use std::num::NonZeroIsize;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationTogglePattern, TreeScope_Subtree, UIA_ButtonControlTypeId, UIA_CONTROLTYPE_ID,
    UIA_CheckBoxControlTypeId, UIA_GroupControlTypeId, UIA_ImageControlTypeId, UIA_InvokePatternId,
    UIA_RadioButtonControlTypeId, UIA_TogglePatternId, UIA_ToolBarControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
};
use windows::core::BOOL;

use crate::Error;

/// One element of a window's UI Automation tree, as a screen reader is told it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub kind: ControlKind,
    pub name: String,
    pub enabled: bool,
    /// What a screen reader adds after the name, such as why the control is off.
    pub help_text: String,
    /// Who answers UI Automation for it, as `ProviderDescription` names them.
    pub provider: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    ToolBar,
    Group,
    Button,
    RadioButton,
    CheckBox,
    Image,
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
            // SAFETY: `index` is below the array's length, and the reads are plain properties
            // of a live element.
            let (kind, name, enabled, help_text, provider) = unsafe {
                let element = found.GetElement(index)?;
                (
                    element.CurrentControlType()?,
                    element.CurrentName()?,
                    element.CurrentIsEnabled()?.as_bool(),
                    element.CurrentHelpText()?,
                    element.CurrentProviderDescription()?,
                )
            };
            let kind = [
                (UIA_ToolBarControlTypeId, ControlKind::ToolBar),
                (UIA_GroupControlTypeId, ControlKind::Group),
                (UIA_ButtonControlTypeId, ControlKind::Button),
                (UIA_RadioButtonControlTypeId, ControlKind::RadioButton),
                (UIA_CheckBoxControlTypeId, ControlKind::CheckBox),
                (UIA_ImageControlTypeId, ControlKind::Image),
            ]
            .into_iter()
            .find_map(|(id, known)| (id == kind).then_some(known))
            .unwrap_or(ControlKind::Other(kind.0));
            controls.push(Element {
                kind,
                name: name.to_string(),
                enabled,
                help_text: help_text.to_string(),
                provider: provider.to_string(),
            });
        }
    }
    Ok(controls)
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
