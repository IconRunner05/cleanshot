use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationInvokePattern, TreeScope_Subtree,
    UIA_ButtonControlTypeId, UIA_ImageControlTypeId, UIA_InvokePatternId,
    UIA_RadioButtonControlTypeId, UIA_ToolBarControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId};
use windows::core::BOOL;

use crate::Error;

/// One element of a window's UI Automation tree, as a screen reader is told it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Control {
    pub kind: ControlKind,
    pub name: String,
    /// Who answers UI Automation for it, as `ProviderDescription` names them.
    pub provider: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    ToolBar,
    Button,
    RadioButton,
    Image,
    /// Any other `UIA_*ControlTypeId`.
    Other(i32),
}

/// Every element UI Automation reports under this process's top-level windows (F-58).
/// Call it from a thread that owns no window: a window answers UI Automation on its own
/// thread, so a walk from that thread waits on itself.
///
/// # Errors
///
/// [`Error::Accessibility`] if COM, UI Automation or an element refuses a call.
pub fn controls_in_own_windows() -> Result<Vec<Control>, Error> {
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
/// Call it from a thread that owns no window, as for [`controls_in_own_windows`].
///
/// # Errors
///
/// [`Error::Accessibility`] if COM, UI Automation or the button refuses a call.
pub fn invoke_in_own_windows(name: &str) -> Result<bool, Error> {
    // SAFETY: COM is initialised for this thread and released below once the call ends.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(Error::Accessibility)?;
    let invoked = invoke(name).map_err(Error::Accessibility);
    // SAFETY: balances the successful CoInitializeEx above, after every COM object is dropped.
    unsafe { CoUninitialize() };
    invoked
}

fn invoke(name: &str) -> windows::core::Result<bool> {
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
            let (kind, named) = unsafe { (element.CurrentControlType()?, element.CurrentName()?) };
            if kind != UIA_ButtonControlTypeId || named != name {
                continue;
            }
            // SAFETY: the element is live, and the pattern asked for is the interface cast to.
            unsafe {
                element
                    .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)?
                    .Invoke()?;
            }
            return Ok(true);
        }
    }
    Ok(false)
}

fn walk_own_windows() -> windows::core::Result<Vec<Control>> {
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
            let (kind, name, provider) = unsafe {
                let element = found.GetElement(index)?;
                (
                    element.CurrentControlType()?,
                    element.CurrentName()?,
                    element.CurrentProviderDescription()?,
                )
            };
            let kind = [
                (UIA_ToolBarControlTypeId, ControlKind::ToolBar),
                (UIA_ButtonControlTypeId, ControlKind::Button),
                (UIA_RadioButtonControlTypeId, ControlKind::RadioButton),
                (UIA_ImageControlTypeId, ControlKind::Image),
            ]
            .into_iter()
            .find_map(|(id, known)| (id == kind).then_some(known))
            .unwrap_or(ControlKind::Other(kind.0));
            controls.push(Control {
                kind,
                name: name.to_string(),
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
