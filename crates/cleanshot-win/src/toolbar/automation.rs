use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use windows::Win32::Foundation::{
    HWND, LPARAM, LRESULT, POINT, RECT, VARIANT_FALSE, VARIANT_TRUE, WPARAM,
};
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayPutElement};
use windows::Win32::System::Variant::{
    VARENUM, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL, VT_BSTR, VT_I4,
};
use windows::Win32::UI::Accessibility::{
    IInvokeProvider, IInvokeProvider_Impl, IRawElementProviderFragment,
    IRawElementProviderFragment_Impl, IRawElementProviderFragmentRoot,
    IRawElementProviderFragmentRoot_Impl, IRawElementProviderSimple,
    IRawElementProviderSimple_Impl, ISelectionItemProvider, ISelectionItemProvider_Impl,
    IValueProvider, IValueProvider_Impl, NavigateDirection, NavigateDirection_FirstChild,
    NavigateDirection_LastChild, NavigateDirection_NextSibling, NavigateDirection_Parent,
    NavigateDirection_PreviousSibling, NotificationKind_Other, NotificationProcessing_MostRecent,
    ProviderOptions, ProviderOptions_ServerSideProvider, UIA_ButtonControlTypeId,
    UIA_ControlTypePropertyId, UIA_E_ELEMENTNOTAVAILABLE, UIA_E_ELEMENTNOTENABLED,
    UIA_E_INVALIDOPERATION, UIA_HasKeyboardFocusPropertyId, UIA_HelpTextPropertyId,
    UIA_InvokePatternId, UIA_IsEnabledPropertyId, UIA_IsKeyboardFocusablePropertyId,
    UIA_NamePropertyId, UIA_PATTERN_ID, UIA_PROPERTY_ID, UIA_RadioButtonControlTypeId,
    UIA_SelectionItemPatternId, UIA_TextControlTypeId, UIA_ToolBarControlTypeId,
    UIA_ValuePatternId, UiaAppendRuntimeId, UiaHostProviderFromHwnd, UiaRaiseNotificationEvent,
    UiaRect, UiaReturnRawElementProvider, UiaRootObjectId,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{BOOL, BSTR, Error, HRESULT, IUnknown, Interface, PCWSTR, Result, implement};

use cleanshot_core::{CapturePanel, Timer, Toolbar, ToolbarControl};

use super::{Layout, name};

/// Wakes the toolbar's window to act on what UI Automation asked of it. It carries nothing,
/// so a post from another program only makes the window re-read its own [`Mirror`].
pub(crate) const WAKE: u32 = WM_APP;

/// What UI Automation asks the toolbar to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Request {
    Choose(ToolbarControl),
    Focus(ToolbarControl),
}

#[derive(Default)]
struct State {
    shown: Option<(Layout, ToolbarControl)>,
    /// F-11: the capture panel's mode and timer, which only the panel shows.
    choices: Option<CapturePanel>,
    /// F-08: what a window capture would pick, as a screen reader says it, and its frame.
    pick: Option<(String, RECT)>,
    requests: Vec<Request>,
}

/// The toolbar as its window last drew it, with the focused control, and what UI Automation
/// asked of it since. The window's thread writes what it shows and drains the requests; the
/// providers read and ask from whatever thread UI Automation calls them on.
#[derive(Clone)]
pub(crate) struct Mirror {
    toolbar: Toolbar,
    state: Arc<Mutex<State>>,
    saving_off: Option<Arc<str>>,
}

impl Mirror {
    /// `toolbar`, with Save off for the reason `saving_off` says (F-57), or on when `None`.
    pub(crate) fn new(toolbar: Toolbar, saving_off: Option<String>) -> Self {
        Self {
            toolbar,
            state: Arc::default(),
            saving_off: saving_off.map(Arc::from),
        }
    }

    /// Why `control` is off, or `None` when it can be chosen (F-57).
    pub(crate) fn off(&self, control: ToolbarControl) -> Option<&str> {
        match control {
            ToolbarControl::Save => self.saving_off.as_deref(),
            ToolbarControl::Mode(_)
            | ToolbarControl::Timer
            | ToolbarControl::Capture
            | ToolbarControl::Copy
            | ToolbarControl::Edit
            | ToolbarControl::Cancel => None,
        }
    }

    /// Records where the toolbar is and which control has the focus, or that none shows.
    pub(crate) fn show(&self, shown: Option<(Layout, ToolbarControl)>) {
        self.lock().shown = shown;
    }

    /// Records the capture panel's mode and timer as it now shows them (F-11).
    pub(crate) fn show_choices(&self, panel: CapturePanel) {
        self.lock().choices = Some(panel);
    }

    fn choices(&self) -> Result<CapturePanel> {
        self.lock().choices.ok_or_else(gone)
    }

    /// Records what a window capture would pick, or that none is being picked (F-08).
    pub(crate) fn show_pick(&self, pick: Option<(String, RECT)>) {
        self.lock().pick = pick;
    }

    fn pick(&self) -> Result<(String, RECT)> {
        self.lock().pick.clone().ok_or_else(gone)
    }

    fn picking(&self) -> bool {
        self.lock().pick.is_some()
    }

    /// The requests made since the last call, oldest first.
    pub(crate) fn requests(&self) -> Vec<Request> {
        std::mem::take(&mut self.lock().requests)
    }

    fn position(&self, control: ToolbarControl) -> usize {
        self.toolbar
            .controls()
            .iter()
            .position(|&each| each == control)
            .expect("the toolbar shows every control it names")
    }

    fn last_control(&self) -> ToolbarControl {
        *self
            .toolbar
            .controls()
            .last()
            .expect("every toolbar ends with Cancel")
    }

    fn shown(&self) -> Result<(Layout, ToolbarControl)> {
        self.lock().shown.clone().ok_or_else(gone)
    }

    fn ask(&self, window: HWND, request: Request) -> Result<()> {
        self.lock().requests.push(request);
        // SAFETY: posting copies no memory.
        unsafe { PostMessageW(Some(window), WAKE, WPARAM(0), LPARAM(0)) }
    }

    // A panic while the lock is held aborts the release build, so a poisoned lock only
    // happens in a test that is already failing; its state is still whole.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

const TOOLBAR_NAME: &str = "Capture";

/// The answer to `WM_GETOBJECT` for the window that shows the toolbar (F-05: every control has
/// an accessible name).
pub(crate) fn answer(
    window: HWND,
    mirror: &Mirror,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    if i32::try_from(lparam.0) != Ok(UiaRootObjectId) {
        return None;
    }
    let root: IRawElementProviderSimple = ToolbarProvider {
        window,
        mirror: mirror.clone(),
    }
    .into();
    // SAFETY: the parameters are WM_GETOBJECT's own, passed on unfiltered as the API asks.
    Some(unsafe { UiaReturnRawElementProvider(window, wparam, lparam, &root) })
}

/// F-58: has a screen reader say `spoken` now, from the toolbar on `window`, without the focus
/// moving; a newer announcement replaces one not yet said.
pub(crate) fn announce(window: HWND, mirror: &Mirror, spoken: &str) -> Result<()> {
    let root: IRawElementProviderSimple = ToolbarProvider {
        window,
        mirror: mirror.clone(),
    }
    .into();
    // SAFETY: `root` is a live provider for this window, and the strings outlive the call.
    unsafe {
        UiaRaiseNotificationEvent(
            &root,
            NotificationKind_Other,
            NotificationProcessing_MostRecent,
            &BSTR::from(spoken),
            &BSTR::from("Cleanshot.WindowToCapture"),
        )
    }
}

/// Tells UI Automation the toolbar's window is going, so it lets go of its providers.
pub(crate) fn forget(window: HWND) {
    // SAFETY: the documented call for a window that returned providers and is being destroyed.
    unsafe { UiaReturnRawElementProvider(window, WPARAM(0), LPARAM(0), None) };
}

fn gone() -> Error {
    #[expect(clippy::cast_possible_wrap, reason = "an HRESULT is the u32's bits")]
    Error::from_hresult(HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32))
}

fn invalid_operation() -> Error {
    #[expect(clippy::cast_possible_wrap, reason = "an HRESULT is the u32's bits")]
    Error::from_hresult(HRESULT(UIA_E_INVALIDOPERATION as i32))
}

/// The timer's wait as a screen reader says it (F-11).
fn timer_value(timer: Timer) -> String {
    match timer {
        Timer::Off => "Off".to_owned(),
        Timer::Three | Timer::Five | Timer::Ten => format!("{} seconds", timer.seconds()),
    }
}

fn not_enabled() -> Error {
    #[expect(clippy::cast_possible_wrap, reason = "an HRESULT is the u32's bits")]
    Error::from_hresult(HRESULT(UIA_E_ELEMENTNOTENABLED as i32))
}

/// No element, no pattern or no array: an `S_OK` with a null out-parameter.
fn none<T>() -> Result<T> {
    Err(Error::empty())
}

fn variant(vt: VARENUM, value: VARIANT_0_0_0) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: value,
            }),
        },
    }
}

fn number(value: i32) -> VARIANT {
    variant(VT_I4, VARIANT_0_0_0 { lVal: value })
}

fn text(value: &str) -> VARIANT {
    variant(
        VT_BSTR,
        VARIANT_0_0_0 {
            bstrVal: ManuallyDrop::new(BSTR::from(value)),
        },
    )
}

fn truth(value: bool) -> VARIANT {
    let boolean = if value { VARIANT_TRUE } else { VARIANT_FALSE };
    variant(VT_BOOL, VARIANT_0_0_0 { boolVal: boolean })
}

fn on_screen(at: windows::Win32::Foundation::RECT) -> UiaRect {
    UiaRect {
        left: f64::from(at.left),
        top: f64::from(at.top),
        width: f64::from(at.right - at.left),
        height: f64::from(at.bottom - at.top),
    }
}

#[expect(
    clippy::inline_always,
    clippy::ref_as_ptr,
    reason = "in the code #[implement] generates"
)]
#[expect(
    non_upper_case_globals,
    reason = "the UI Automation constants matched here keep their Windows names"
)]
mod providers {
    use super::{
        BOOL, BSTR, HWND, IInvokeProvider, IInvokeProvider_Impl, IRawElementProviderFragment,
        IRawElementProviderFragment_Impl, IRawElementProviderFragmentRoot,
        IRawElementProviderFragmentRoot_Impl, IRawElementProviderSimple,
        IRawElementProviderSimple_Impl, ISelectionItemProvider, ISelectionItemProvider_Impl,
        IUnknown, IValueProvider, IValueProvider_Impl, Interface, Mirror, NavigateDirection,
        NavigateDirection_FirstChild, NavigateDirection_LastChild, NavigateDirection_NextSibling,
        NavigateDirection_Parent, NavigateDirection_PreviousSibling, PCWSTR, POINT,
        ProviderOptions, ProviderOptions_ServerSideProvider, Request, Result, SAFEARRAY,
        SafeArrayCreateVector, SafeArrayPutElement, TOOLBAR_NAME, ToolbarControl,
        UIA_ButtonControlTypeId, UIA_ControlTypePropertyId, UIA_HasKeyboardFocusPropertyId,
        UIA_HelpTextPropertyId, UIA_InvokePatternId, UIA_IsEnabledPropertyId,
        UIA_IsKeyboardFocusablePropertyId, UIA_NamePropertyId, UIA_PATTERN_ID, UIA_PROPERTY_ID,
        UIA_RadioButtonControlTypeId, UIA_SelectionItemPatternId, UIA_TextControlTypeId,
        UIA_ToolBarControlTypeId, UIA_ValuePatternId, UiaAppendRuntimeId, UiaHostProviderFromHwnd,
        UiaRect, VARIANT, VT_I4, implement, invalid_operation, name, none, not_enabled, number,
        on_screen, text, timer_value, truth,
    };

    /// The toolbar, the root of the tree its window hands UI Automation.
    #[implement(
        IRawElementProviderSimple,
        IRawElementProviderFragment,
        IRawElementProviderFragmentRoot
    )]
    pub(super) struct ToolbarProvider {
        pub(super) window: HWND,
        pub(super) mirror: Mirror,
    }

    /// One of the toolbar's controls. A capture mode is a radio button whose selection is the
    /// panel's mode, and the timer's value is its wait (F-11).
    #[implement(
        IRawElementProviderSimple,
        IRawElementProviderFragment,
        IInvokeProvider,
        ISelectionItemProvider,
        IValueProvider
    )]
    struct Button {
        window: HWND,
        mirror: Mirror,
        control: ToolbarControl,
    }

    /// F-08: what a window capture would pick, after the buttons while it is being picked.
    #[implement(IRawElementProviderSimple, IRawElementProviderFragment)]
    struct Picking {
        window: HWND,
        mirror: Mirror,
    }

    fn picking(window: HWND, mirror: &Mirror) -> IRawElementProviderFragment {
        Picking {
            window,
            mirror: mirror.clone(),
        }
        .into()
    }

    /// The toolbar's last child: the pick while there is one, else the last button.
    fn last_child(window: HWND, mirror: &Mirror) -> IRawElementProviderFragment {
        if mirror.picking() {
            return picking(window, mirror);
        }
        button(window, mirror, mirror.last_control())
    }

    pub(super) fn button(
        window: HWND,
        mirror: &Mirror,
        control: ToolbarControl,
    ) -> IRawElementProviderFragment {
        Button {
            window,
            mirror: mirror.clone(),
            control,
        }
        .into()
    }

    fn toolbar(window: HWND, mirror: &Mirror) -> ToolbarProvider {
        ToolbarProvider {
            window,
            mirror: mirror.clone(),
        }
    }

    impl IRawElementProviderSimple_Impl for ToolbarProvider_Impl {
        fn ProviderOptions(&self) -> Result<ProviderOptions> {
            Ok(ProviderOptions_ServerSideProvider)
        }

        fn GetPatternProvider(&self, _pattern: UIA_PATTERN_ID) -> Result<IUnknown> {
            none()
        }

        fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
            Ok(match property {
                UIA_ControlTypePropertyId => number(UIA_ToolBarControlTypeId.0),
                UIA_NamePropertyId => text(TOOLBAR_NAME),
                _ => VARIANT::default(),
            })
        }

        fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
            // SAFETY: UI Automation checks the handle is a live window.
            unsafe { UiaHostProviderFromHwnd(self.window) }
        }
    }

    impl IRawElementProviderFragment_Impl for ToolbarProvider_Impl {
        fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
            match direction {
                NavigateDirection_FirstChild => Ok(button(
                    self.window,
                    &self.mirror,
                    self.mirror.toolbar.controls()[0],
                )),
                NavigateDirection_LastChild => Ok(last_child(self.window, &self.mirror)),
                _ => none(),
            }
        }

        fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
            Ok(std::ptr::null_mut())
        }

        fn BoundingRectangle(&self) -> Result<UiaRect> {
            Ok(on_screen(self.mirror.shown()?.0.pill))
        }

        fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
            Ok(std::ptr::null_mut())
        }

        fn SetFocus(&self) -> Result<()> {
            Ok(())
        }

        fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
            Ok(toolbar(self.window, &self.mirror).into())
        }
    }

    impl IRawElementProviderFragmentRoot_Impl for ToolbarProvider_Impl {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "UI Automation passes whole screen pixels"
        )]
        fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
            let at = POINT {
                x: x as i32,
                y: y as i32,
            };
            match self.mirror.shown()?.0.control_at(at) {
                Some(control) => Ok(button(self.window, &self.mirror, control)),
                None => none(),
            }
        }

        fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
            Ok(button(self.window, &self.mirror, self.mirror.shown()?.1))
        }
    }

    impl IRawElementProviderSimple_Impl for Button_Impl {
        fn ProviderOptions(&self) -> Result<ProviderOptions> {
            Ok(ProviderOptions_ServerSideProvider)
        }

        fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID) -> Result<IUnknown> {
            let this = || Button {
                window: self.window,
                mirror: self.mirror.clone(),
                control: self.control,
            };
            match (pattern, self.control) {
                (UIA_InvokePatternId, _) => IInvokeProvider::from(this()).cast(),
                (UIA_SelectionItemPatternId, ToolbarControl::Mode(_)) => {
                    ISelectionItemProvider::from(this()).cast()
                }
                (UIA_ValuePatternId, ToolbarControl::Timer) => IValueProvider::from(this()).cast(),
                _ => none(),
            }
        }

        fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
            Ok(match property {
                UIA_ControlTypePropertyId => match self.control {
                    ToolbarControl::Mode(_) => number(UIA_RadioButtonControlTypeId.0),
                    ToolbarControl::Timer
                    | ToolbarControl::Capture
                    | ToolbarControl::Copy
                    | ToolbarControl::Save
                    | ToolbarControl::Edit
                    | ToolbarControl::Cancel => number(UIA_ButtonControlTypeId.0),
                },
                UIA_NamePropertyId => text(name(self.control)),
                UIA_IsEnabledPropertyId | UIA_IsKeyboardFocusablePropertyId => {
                    truth(self.mirror.off(self.control).is_none())
                }
                UIA_HelpTextPropertyId => match self.mirror.off(self.control) {
                    Some(reason) => text(reason),
                    None => VARIANT::default(),
                },
                UIA_HasKeyboardFocusPropertyId => truth(self.mirror.shown()?.1 == self.control),
                _ => VARIANT::default(),
            })
        }

        fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
            none()
        }
    }

    impl IRawElementProviderFragment_Impl for Button_Impl {
        fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
            let at = self.mirror.position(self.control);
            let sibling = |index: Option<usize>| match index
                .and_then(|index| self.mirror.toolbar.controls().get(index))
            {
                Some(&control) => Ok(button(self.window, &self.mirror, control)),
                None if index == Some(self.mirror.toolbar.controls().len())
                    && self.mirror.picking() =>
                {
                    Ok(picking(self.window, &self.mirror))
                }
                None => none(),
            };
            match direction {
                NavigateDirection_Parent => Ok(toolbar(self.window, &self.mirror).into()),
                NavigateDirection_NextSibling => sibling(at.checked_add(1)),
                NavigateDirection_PreviousSibling => sibling(at.checked_sub(1)),
                _ => none(),
            }
        }

        fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
            let id = [
                i32::try_from(UiaAppendRuntimeId).expect("UiaAppendRuntimeId is 3"),
                i32::try_from(self.mirror.position(self.control)).expect("a few controls"),
            ];
            // SAFETY: a two-element vector of VT_I4, each element written in range; the
            // caller frees the array.
            unsafe {
                let array = SafeArrayCreateVector(VT_I4, 0, 2);
                for (index, value) in (0..).zip(&id) {
                    SafeArrayPutElement(array, &raw const index, std::ptr::from_ref(value).cast())?;
                }
                Ok(array)
            }
        }

        fn BoundingRectangle(&self) -> Result<UiaRect> {
            let (layout, _) = self.mirror.shown()?;
            let (_, at) = layout.controls[self.mirror.position(self.control)];
            Ok(on_screen(at))
        }

        fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
            Ok(std::ptr::null_mut())
        }

        fn SetFocus(&self) -> Result<()> {
            if self.mirror.off(self.control).is_some() {
                return Err(not_enabled());
            }
            self.mirror.ask(self.window, Request::Focus(self.control))
        }

        fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
            Ok(toolbar(self.window, &self.mirror).into())
        }
    }

    impl IRawElementProviderSimple_Impl for Picking_Impl {
        fn ProviderOptions(&self) -> Result<ProviderOptions> {
            Ok(ProviderOptions_ServerSideProvider)
        }

        fn GetPatternProvider(&self, _pattern: UIA_PATTERN_ID) -> Result<IUnknown> {
            none()
        }

        fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
            Ok(match property {
                UIA_ControlTypePropertyId => number(UIA_TextControlTypeId.0),
                UIA_NamePropertyId => text(&self.mirror.pick()?.0),
                _ => VARIANT::default(),
            })
        }

        fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
            none()
        }
    }

    impl IRawElementProviderFragment_Impl for Picking_Impl {
        fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
            match direction {
                NavigateDirection_Parent => Ok(toolbar(self.window, &self.mirror).into()),
                NavigateDirection_PreviousSibling => Ok(button(
                    self.window,
                    &self.mirror,
                    self.mirror.last_control(),
                )),
                _ => none(),
            }
        }

        fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
            let id = [
                i32::try_from(UiaAppendRuntimeId).expect("UiaAppendRuntimeId is 3"),
                i32::try_from(self.mirror.toolbar.controls().len()).expect("a few controls"),
            ];
            // SAFETY: a two-element vector of VT_I4, each element written in range; the
            // caller frees the array.
            unsafe {
                let array = SafeArrayCreateVector(VT_I4, 0, 2);
                for (index, value) in (0..).zip(&id) {
                    SafeArrayPutElement(array, &raw const index, std::ptr::from_ref(value).cast())?;
                }
                Ok(array)
            }
        }

        fn BoundingRectangle(&self) -> Result<UiaRect> {
            Ok(on_screen(self.mirror.pick()?.1))
        }

        fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
            Ok(std::ptr::null_mut())
        }

        fn SetFocus(&self) -> Result<()> {
            Ok(())
        }

        fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
            Ok(toolbar(self.window, &self.mirror).into())
        }
    }

    impl ISelectionItemProvider_Impl for Button_Impl {
        fn Select(&self) -> Result<()> {
            self.mirror.ask(self.window, Request::Choose(self.control))
        }

        fn AddToSelection(&self) -> Result<()> {
            self.Select()
        }

        fn RemoveFromSelection(&self) -> Result<()> {
            Err(invalid_operation())
        }

        fn IsSelected(&self) -> Result<BOOL> {
            let chosen = self.mirror.choices()?.mode();
            Ok((self.control == ToolbarControl::Mode(chosen)).into())
        }

        fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
            Ok(toolbar(self.window, &self.mirror).into())
        }
    }

    impl IValueProvider_Impl for Button_Impl {
        fn SetValue(&self, _value: &PCWSTR) -> Result<()> {
            Err(invalid_operation())
        }

        fn Value(&self) -> Result<BSTR> {
            Ok(BSTR::from(timer_value(self.mirror.choices()?.timer())))
        }

        fn IsReadOnly(&self) -> Result<BOOL> {
            Ok(true.into())
        }
    }

    impl IInvokeProvider_Impl for Button_Impl {
        fn Invoke(&self) -> Result<()> {
            if self.mirror.off(self.control).is_some() {
                return Err(not_enabled());
            }
            self.mirror.ask(self.window, Request::Choose(self.control))
        }
    }
}

use providers::ToolbarProvider;

#[cfg(test)]
mod tests {
    use cleanshot_core::CaptureMode;
    use windows::Win32::Foundation::RECT;

    use super::providers::button;
    use super::*;

    fn patterns(mirror: &Mirror, control: ToolbarControl) -> IRawElementProviderSimple {
        button(HWND::default(), mirror, control)
            .cast()
            .expect("a button is a simple provider")
    }

    fn selected(mirror: &Mirror, control: ToolbarControl) -> Result<bool> {
        // SAFETY: in-process calls on a provider this test owns.
        unsafe {
            let item: ISelectionItemProvider = patterns(mirror, control)
                .GetPatternProvider(UIA_SelectionItemPatternId)?
                .cast()?;
            Ok(item.IsSelected()?.as_bool())
        }
    }

    fn value(mirror: &Mirror) -> Result<String> {
        // SAFETY: in-process calls on a provider this test owns.
        unsafe {
            let timer: IValueProvider = patterns(mirror, ToolbarControl::Timer)
                .GetPatternProvider(UIA_ValuePatternId)?
                .cast()?;
            Ok(timer.Value()?.to_string())
        }
    }

    #[test]
    fn f11_a_screen_reader_hears_the_chosen_mode_as_selected_and_the_timer_as_its_wait() {
        let mirror = Mirror::new(Toolbar::Panel, None);
        let layout = super::super::layout(
            Toolbar::Panel,
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            None,
            1.0,
        )
        .expect("a 1080p monitor holds the panel");
        mirror.show(Some((layout, ToolbarControl::Capture)));
        let mut panel = CapturePanel::default();
        panel.press(ToolbarControl::Mode(CaptureMode::Window));
        panel.press(ToolbarControl::Timer);
        panel.press(ToolbarControl::Timer);
        mirror.show_choices(panel);
        let modes = [
            CaptureMode::Region,
            CaptureMode::Window,
            CaptureMode::FullScreen,
        ]
        .map(|mode| selected(&mirror, ToolbarControl::Mode(mode)).expect("a mode is selectable"));
        assert_eq!(modes, [false, true, false]);
        assert_eq!(value(&mirror).expect("the timer has a value"), "5 seconds");
    }
}
