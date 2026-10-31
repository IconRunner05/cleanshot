use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use windows::Win32::Foundation::{
    HWND, LPARAM, LRESULT, POINT, VARIANT_FALSE, VARIANT_TRUE, WPARAM,
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
    IRawElementProviderSimple_Impl, NavigateDirection, NavigateDirection_FirstChild,
    NavigateDirection_LastChild, NavigateDirection_NextSibling, NavigateDirection_Parent,
    NavigateDirection_PreviousSibling, ProviderOptions, ProviderOptions_ServerSideProvider,
    UIA_ButtonControlTypeId, UIA_ControlTypePropertyId, UIA_E_ELEMENTNOTAVAILABLE,
    UIA_HasKeyboardFocusPropertyId, UIA_InvokePatternId, UIA_IsKeyboardFocusablePropertyId,
    UIA_NamePropertyId, UIA_PATTERN_ID, UIA_PROPERTY_ID, UIA_ToolBarControlTypeId,
    UiaAppendRuntimeId, UiaHostProviderFromHwnd, UiaRect, UiaReturnRawElementProvider,
    UiaRootObjectId,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{BSTR, Error, HRESULT, IUnknown, Interface, Result, implement};

use super::{CONTROLS, Control, Layout};

/// Wakes the toolbar's window to act on what UI Automation asked of it. It carries nothing,
/// so a post from another program only makes the window re-read its own [`Mirror`].
pub(crate) const WAKE: u32 = WM_APP;

/// What UI Automation asks the toolbar to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Request {
    Choose(Control),
    Focus(Control),
}

#[derive(Default)]
struct State {
    shown: Option<(Layout, Control)>,
    requests: Vec<Request>,
}

/// The toolbar as its window last drew it, with the focused control, and what UI Automation
/// asked of it since. The window's thread writes what it shows and drains the requests; the
/// providers read and ask from whatever thread UI Automation calls them on.
#[derive(Clone, Default)]
pub(crate) struct Mirror(Arc<Mutex<State>>);

impl Mirror {
    /// Records where the toolbar is and which control has the focus, or that none shows.
    pub(crate) fn show(&self, shown: Option<(Layout, Control)>) {
        self.lock().shown = shown;
    }

    /// The requests made since the last call, oldest first.
    pub(crate) fn requests(&self) -> Vec<Request> {
        std::mem::take(&mut self.lock().requests)
    }

    fn shown(&self) -> Result<(Layout, Control)> {
        self.lock().shown.ok_or_else(gone)
    }

    fn ask(&self, window: HWND, request: Request) -> Result<()> {
        self.lock().requests.push(request);
        // SAFETY: posting copies no memory.
        unsafe { PostMessageW(Some(window), WAKE, WPARAM(0), LPARAM(0)) }
    }

    // A panic while the lock is held aborts the release build, so a poisoned lock only
    // happens in a test that is already failing; its state is still whole.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
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
    let root: IRawElementProviderSimple = Toolbar {
        window,
        mirror: mirror.clone(),
    }
    .into();
    // SAFETY: the parameters are WM_GETOBJECT's own, passed on unfiltered as the API asks.
    Some(unsafe { UiaReturnRawElementProvider(window, wparam, lparam, &root) })
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

/// No element, no pattern or no array: an `S_OK` with a null out-parameter.
fn none<T>() -> Result<T> {
    Err(Error::empty())
}

fn position(control: Control) -> usize {
    CONTROLS
        .iter()
        .position(|&each| each == control)
        .expect("every control is in CONTROLS")
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
        CONTROLS, Control, HWND, IInvokeProvider, IInvokeProvider_Impl,
        IRawElementProviderFragment, IRawElementProviderFragment_Impl,
        IRawElementProviderFragmentRoot, IRawElementProviderFragmentRoot_Impl,
        IRawElementProviderSimple, IRawElementProviderSimple_Impl, IUnknown, Interface, Mirror,
        NavigateDirection, NavigateDirection_FirstChild, NavigateDirection_LastChild,
        NavigateDirection_NextSibling, NavigateDirection_Parent, NavigateDirection_PreviousSibling,
        POINT, ProviderOptions, ProviderOptions_ServerSideProvider, Request, Result, SAFEARRAY,
        SafeArrayCreateVector, SafeArrayPutElement, TOOLBAR_NAME, UIA_ButtonControlTypeId,
        UIA_ControlTypePropertyId, UIA_HasKeyboardFocusPropertyId, UIA_InvokePatternId,
        UIA_IsKeyboardFocusablePropertyId, UIA_NamePropertyId, UIA_PATTERN_ID, UIA_PROPERTY_ID,
        UIA_ToolBarControlTypeId, UiaAppendRuntimeId, UiaHostProviderFromHwnd, UiaRect, VARIANT,
        VT_I4, implement, none, number, on_screen, position, text, truth,
    };

    /// The toolbar, the root of the tree its window hands UI Automation.
    #[implement(
        IRawElementProviderSimple,
        IRawElementProviderFragment,
        IRawElementProviderFragmentRoot
    )]
    pub(super) struct Toolbar {
        pub(super) window: HWND,
        pub(super) mirror: Mirror,
    }

    /// One of the toolbar's controls.
    #[implement(
        IRawElementProviderSimple,
        IRawElementProviderFragment,
        IInvokeProvider
    )]
    struct Button {
        window: HWND,
        mirror: Mirror,
        control: Control,
    }

    fn button(window: HWND, mirror: &Mirror, control: Control) -> IRawElementProviderFragment {
        Button {
            window,
            mirror: mirror.clone(),
            control,
        }
        .into()
    }

    fn toolbar(window: HWND, mirror: &Mirror) -> Toolbar {
        Toolbar {
            window,
            mirror: mirror.clone(),
        }
    }

    impl IRawElementProviderSimple_Impl for Toolbar_Impl {
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

    impl IRawElementProviderFragment_Impl for Toolbar_Impl {
        fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
            match direction {
                NavigateDirection_FirstChild => Ok(button(self.window, &self.mirror, CONTROLS[0])),
                NavigateDirection_LastChild => Ok(button(
                    self.window,
                    &self.mirror,
                    CONTROLS[CONTROLS.len() - 1],
                )),
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

    impl IRawElementProviderFragmentRoot_Impl for Toolbar_Impl {
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
            match pattern {
                UIA_InvokePatternId => IInvokeProvider::from(Button {
                    window: self.window,
                    mirror: self.mirror.clone(),
                    control: self.control,
                })
                .cast(),
                _ => none(),
            }
        }

        fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
            Ok(match property {
                UIA_ControlTypePropertyId => number(UIA_ButtonControlTypeId.0),
                UIA_NamePropertyId => text(self.control.name()),
                UIA_IsKeyboardFocusablePropertyId => truth(true),
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
            let at = position(self.control);
            let sibling = |index: Option<usize>| match index.and_then(|index| CONTROLS.get(index)) {
                Some(&control) => Ok(button(self.window, &self.mirror, control)),
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
                i32::try_from(position(self.control)).expect("four controls"),
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
            let (_, at) = layout.controls[position(self.control)];
            Ok(on_screen(at))
        }

        fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
            Ok(std::ptr::null_mut())
        }

        fn SetFocus(&self) -> Result<()> {
            self.mirror.ask(self.window, Request::Focus(self.control))
        }

        fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
            Ok(toolbar(self.window, &self.mirror).into())
        }
    }

    impl IInvokeProvider_Impl for Button_Impl {
        fn Invoke(&self) -> Result<()> {
            self.mirror.ask(self.window, Request::Choose(self.control))
        }
    }
}

use providers::Toolbar;
