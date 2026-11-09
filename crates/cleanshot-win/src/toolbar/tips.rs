use std::borrow::Cow;
use std::time::Duration;

use cleanshot_core::{Toolbar, ToolbarControl};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::Controls::{
    ICC_TAB_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, TOOLTIPS_CLASSW, TTDT_INITIAL,
    TTF_SUBCLASS, TTM_ADDTOOLW, TTM_NEWTOOLRECTW, TTM_SETDELAYTIME, TTS_ALWAYSTIP, TTS_NOPREFIX,
    TTTOOLINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, SPI_GETMOUSEHOVERTIME, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    SendMessageW, SystemParametersInfoW, WINDOW_STYLE, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

use super::tip;
use crate::struct_size;

/// How long the pointer rests on a control before its tooltip shows, as set in Windows.
///
/// # Errors
///
/// The Windows error if the setting cannot be read.
pub fn hover_time() -> Result<Duration, windows::core::Error> {
    let mut hover = 0_u32;
    // SAFETY: SPI_GETMOUSEHOVERTIME writes one u32 to `hover`.
    unsafe {
        SystemParametersInfoW(
            SPI_GETMOUSEHOVERTIME,
            0,
            Some((&raw mut hover).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }?;
    Ok(Duration::from_millis(hover.into()))
}

/// Tooltips over a window's buttons, one per button, each shown after the system's
/// hover time. They belong to the window that shows the buttons, which destroys them with
/// itself.
pub(crate) struct Tips {
    window: HWND,
    owner: HWND,
    count: usize,
}

// The size before lpReserved, which comctl32 v5 accepts as well as v6: a binary without the
// v6 manifest, such as this crate's tests, loads v5 and refuses the whole struct.
#[expect(
    clippy::cast_possible_truncation,
    reason = "an offset inside a small struct"
)]
const TOOL_SIZE: u32 = std::mem::offset_of!(TTTOOLINFOW, lpReserved) as u32;

impl Tips {
    /// A tooltip saying each of `texts` on `owner`, over nothing until [`Tips::place`].
    pub(crate) fn new(owner: HWND, texts: &[Cow<'_, str>]) -> windows::core::Result<Self> {
        let classes = INITCOMMONCONTROLSEX {
            dwSize: struct_size::<INITCOMMONCONTROLSEX>(),
            dwICC: ICC_TAB_CLASSES,
        };
        // SAFETY: `classes` is initialised and outlives the call.
        unsafe { InitCommonControlsEx(&raw const classes) }.ok()?;
        // SAFETY: the tooltip class is the system's; `owner` is a live window of this thread.
        // TTS_ALWAYSTIP shows it over the overlay, which never activates.
        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST,
                TOOLTIPS_CLASSW,
                PCWSTR::null(),
                WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                Some(owner),
                None,
                None,
                None,
            )
        }?;
        let tips = Self {
            window,
            owner,
            count: texts.len(),
        };
        for (id, text) in texts.iter().enumerate() {
            let text = HSTRING::from(text.as_ref());
            let tool = TTTOOLINFOW {
                cbSize: TOOL_SIZE,
                uFlags: TTF_SUBCLASS,
                hwnd: owner,
                uId: id,
                lpszText: PWSTR(text.as_ptr().cast_mut()),
                ..Default::default()
            };
            // SAFETY: the tooltip copies `tool` and its text during the call.
            if tips.send(TTM_ADDTOOLW, 0, std::ptr::from_ref(&tool) as isize) == 0 {
                return Err(windows::core::Error::from_thread());
            }
        }
        // A delay is the message's low word; -1 restores the default for one that does not fit.
        let hover = u16::try_from(hover_time()?.as_millis()).map_or(-1, |delay| {
            isize::try_from(delay).expect("a u16 fits the isize of a 32- or 64-bit target")
        });
        tips.send(TTM_SETDELAYTIME, TTDT_INITIAL as usize, hover);
        Ok(tips)
    }

    /// Moves the tooltips' hover areas onto `areas`, in the owner's client pixels, in the order
    /// of their texts; a tooltip with no area left hovers over nothing.
    pub(crate) fn place(&self, areas: impl IntoIterator<Item = RECT>) {
        let mut areas = areas.into_iter();
        for id in 0..self.count {
            let tool = TTTOOLINFOW {
                cbSize: TOOL_SIZE,
                hwnd: self.owner,
                uId: id,
                rect: areas.next().unwrap_or_default(),
                ..Default::default()
            };
            self.send(TTM_NEWTOOLRECTW, 0, std::ptr::from_ref(&tool) as isize);
        }
    }

    fn send(&self, message: u32, wparam: usize, lparam: isize) -> isize {
        // SAFETY: every message sent here carries a TTTOOLINFOW that outlives the call, or
        // plain numbers.
        unsafe {
            SendMessageW(
                self.window,
                message,
                Some(WPARAM(wparam)),
                Some(LPARAM(lparam)),
            )
        }
        .0
    }
}

/// The tooltips for `toolbar`'s buttons, in its order: each one's name and shortcut, or
/// `off`'s reason for a button that is off.
pub(crate) fn toolbar_tips(
    toolbar: Toolbar,
    off: impl Fn(ToolbarControl) -> Option<String>,
) -> Vec<Cow<'static, str>> {
    toolbar
        .controls()
        .iter()
        .map(|&control| off(control).map_or_else(|| tip(toolbar, control), Cow::Owned))
        .collect()
}

#[cfg(test)]
pub(crate) mod reading {
    use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Controls::{
        TTDT_INITIAL, TTM_ENUMTOOLSW, TTM_GETDELAYTIME, TTM_GETTOOLCOUNT, TTTOOLINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumThreadWindows, GW_OWNER, GetClassNameW, GetWindow, SendMessageW,
    };
    use windows::core::{BOOL, PWSTR};

    use super::TOOL_SIZE;

    /// The tooltips `owner` shows, each tool's text and hover area in order, and the hover
    /// delay in ms; None while it has no tooltip control.
    pub(crate) fn tips_of(owner: HWND) -> Option<(Vec<(String, RECT)>, isize)> {
        unsafe extern "system" fn owned(window: HWND, found: LPARAM) -> BOOL {
            // SAFETY: `found` is the (owner, tooltip) pair tips_of lends for this enumeration.
            // EnumThreadWindows calls this only during that call, with our own LPARAM; no window
            // message carries it, so no other process can supply it.
            // ast-grep-ignore: pointer-from-handle-value
            let found = unsafe { &mut *(found.0 as *mut (HWND, Option<HWND>)) };
            let mut class = [0_u16; 32];
            // SAFETY: `class` is writable for its length; both calls only read `window`.
            let (length, owner) = unsafe {
                (
                    GetClassNameW(window, &mut class),
                    GetWindow(window, GW_OWNER),
                )
            };
            let is_tip = String::from_utf16_lossy(&class[..length.unsigned_abs() as usize])
                == "tooltips_class32";
            if is_tip && owner == Ok(found.0) {
                found.1 = Some(window);
            }
            BOOL::from(found.1.is_none())
        }
        let mut found = (owner, None);
        // SAFETY: `found` outlives the enumeration, which runs on this thread.
        let _ = unsafe {
            EnumThreadWindows(
                GetCurrentThreadId(),
                Some(owned),
                LPARAM(std::ptr::from_mut(&mut found) as isize),
            )
        };
        let tip = found.1?;
        // SAFETY: each message sent carries plain numbers, or a TTTOOLINFOW whose 512-unit
        // text buffer outlives the call.
        let send = |message, wparam: usize, lparam: isize| unsafe {
            SendMessageW(tip, message, Some(WPARAM(wparam)), Some(LPARAM(lparam))).0
        };
        let tools = (0..send(TTM_GETTOOLCOUNT, 0, 0).unsigned_abs())
            .map(|index| {
                let mut text = [0_u16; 512];
                let mut tool = TTTOOLINFOW {
                    cbSize: TOOL_SIZE,
                    lpszText: PWSTR(text.as_mut_ptr()),
                    ..Default::default()
                };
                // TTM_ENUMTOOLSW writes the tool and copies its text into `text`, which holds
                // any tip this crate sets.
                send(
                    TTM_ENUMTOOLSW,
                    index,
                    std::ptr::from_mut(&mut tool) as isize,
                );
                (crate::text_before_nul(&text), tool.rect)
            })
            .collect();
        Some((tools, send(TTM_GETDELAYTIME, TTDT_INITIAL as usize, 0)))
    }
}
