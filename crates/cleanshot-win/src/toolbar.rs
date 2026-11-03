use std::borrow::Cow;
use std::time::Duration;

use cleanshot_core::{CaptureMode, Toolbar, ToolbarControl};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1Crop, CLSID_D2D1GaussianBlur, D2D1_CROP_PROP_RECT, D2D1_EXTEND_MODE_CLAMP,
    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION, D2D1_IMAGE_BRUSH_PROPERTIES,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_PROPERTY_TYPE_FLOAT, D2D1_PROPERTY_TYPE_VECTOR4,
    D2D1_ROUNDED_RECT, ID2D1Bitmap, ID2D1DeviceContext, ID2D1Effect, ID2D1Image, ID2D1ImageBrush,
    ID2D1SolidColorBrush,
};
use windows::Win32::UI::Controls::{
    ICC_TAB_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, TOOLTIPS_CLASSW, TTDT_INITIAL,
    TTF_SUBCLASS, TTM_ADDTOOLW, TTM_NEWTOOLRECTW, TTM_SETDELAYTIME, TTS_ALWAYSTIP, TTS_NOPREFIX,
    TTTOOLINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, SPI_GETMOUSEHOVERTIME, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    SendMessageW, SystemParametersInfoW, WINDOW_STYLE, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{HSTRING, Interface, PCWSTR, PWSTR};

use crate::capture::{screen_point, screen_rect};
use crate::palette::Palette;
use crate::struct_size;
use crate::text::{Fonts, TextStyle};

pub(crate) mod automation;
pub(crate) mod options;

/// A control's accessible name.
pub(crate) fn name(control: ToolbarControl) -> &'static str {
    match control {
        ToolbarControl::Mode(CaptureMode::Region) => "Region",
        ToolbarControl::Mode(CaptureMode::Window) => "Window",
        ToolbarControl::Mode(CaptureMode::FullScreen) => "Screen",
        ToolbarControl::Timer => "Timer",
        ToolbarControl::Options => "Options",
        ToolbarControl::Capture => "Capture",
        ToolbarControl::Copy => "Copy",
        ToolbarControl::Save => "Save",
        ToolbarControl::Edit => "Edit",
        ToolbarControl::Cancel => "Cancel",
    }
}

/// D-117: the key that chooses `control` on `toolbar`, as its tooltip and UI Automation's
/// `AcceleratorKey` name it, or None when no key does.
pub(crate) fn shortcut(toolbar: Toolbar, control: ToolbarControl) -> Option<&'static str> {
    match (toolbar, control) {
        (Toolbar::Panel | Toolbar::Overlay, ToolbarControl::Cancel) => Some("Esc"),
        (Toolbar::Panel, ToolbarControl::Capture) | (Toolbar::Overlay, ToolbarControl::Copy) => {
            Some("Enter")
        }
        (Toolbar::Overlay, ToolbarControl::Save) => Some("Ctrl+S"),
        (Toolbar::Overlay, ToolbarControl::Edit) => Some("E"),
        (
            Toolbar::Panel,
            ToolbarControl::Mode(_)
            | ToolbarControl::Timer
            | ToolbarControl::Options
            | ToolbarControl::Copy
            | ToolbarControl::Save
            | ToolbarControl::Edit,
        )
        | (
            Toolbar::Overlay,
            ToolbarControl::Mode(_)
            | ToolbarControl::Timer
            | ToolbarControl::Options
            | ToolbarControl::Capture,
        ) => None,
    }
}

/// D-117: the tooltip over `control` on `toolbar`: its name, and its shortcut when it has one.
pub(crate) fn tip(toolbar: Toolbar, control: ToolbarControl) -> Cow<'static, str> {
    match shortcut(toolbar, control) {
        Some(key) => Cow::Owned(format!("{} ({key})", name(control))),
        None => Cow::Borrowed(name(control)),
    }
}

/// The text drawn on a control whose label does not change.
pub(crate) fn label(control: ToolbarControl) -> &'static str {
    match control {
        ToolbarControl::Mode(_)
        | ToolbarControl::Timer
        | ToolbarControl::Options
        | ToolbarControl::Capture
        | ToolbarControl::Copy
        | ToolbarControl::Save
        | ToolbarControl::Edit => name(control),
        ToolbarControl::Cancel => "\u{d7}",
    }
}

fn button_width(control: ToolbarControl) -> f32 {
    match control {
        ToolbarControl::Mode(_)
        | ToolbarControl::Timer
        | ToolbarControl::Options
        | ToolbarControl::Capture
        | ToolbarControl::Copy
        | ToolbarControl::Save
        | ToolbarControl::Edit => TEXT_BUTTON_WIDTH,
        ToolbarControl::Cancel => ICON_BUTTON_WIDTH,
    }
}

/// The control filled with the accent: the one Enter presses when the toolbar opens.
pub(crate) fn primary(toolbar: Toolbar) -> ToolbarControl {
    match toolbar {
        Toolbar::Panel => ToolbarControl::Capture,
        Toolbar::Overlay => ToolbarControl::Copy,
    }
}

/// How a control is drawn besides its place: its text, and its state (F-11, F-57).
pub(crate) struct Look {
    pub(crate) label: Cow<'static, str>,
    pub(crate) mark: Mark,
}

/// A control's state as drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    Plain,
    /// Cannot be chosen, so its label is greyed (F-57).
    Off,
    /// The capture panel's chosen mode (F-11).
    Chosen,
}

// CHARTER 4 tokens, in device-independent pixels.
const PILL_HEIGHT: f32 = 44.0;
const BUTTON_HEIGHT: f32 = 32.0;
const PADDING: f32 = 6.0;
const GAP: f32 = 4.0;
const TEXT_BUTTON_WIDTH: f32 = 64.0;
const ICON_BUTTON_WIDTH: f32 = 32.0;
const BUTTON_RADIUS: f32 = 7.0;
const EDGE_MARGIN: f32 = 24.0;
const LABEL_SIZE: f32 = 13.0;
const BLUR_DEVIATION: f32 = 10.0;
const FOCUS_RING: f32 = 2.0;

/// Where the toolbar and each control sit, in physical virtual-desktop pixels.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layout {
    pub(crate) toolbar: Toolbar,
    pub(crate) pill: RECT,
    pub(crate) controls: Vec<(ToolbarControl, RECT)>,
}

/// `toolbar`'s controls centred near the bottom of `monitor`, or near its top when that would
/// cover `selected` (F-05), at `scale` physical pixels per DIP. None when the monitor is too
/// small to hold it.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "toolbar sizes are a few hundred pixels and monitor edges fit in f32"
)]
pub(crate) fn layout(
    toolbar: Toolbar,
    monitor: RECT,
    selected: Option<RECT>,
    scale: f32,
) -> Option<Layout> {
    let px = |dips: f32| (dips * scale).round() as i32;
    let width = px(PADDING * 2.0
        + toolbar
            .controls()
            .iter()
            .map(|&control| button_width(control))
            .sum::<f32>()
        + GAP * (toolbar.controls().len() - 1) as f32);
    let (height, margin) = (px(PILL_HEIGHT), px(EDGE_MARGIN));
    if width + 2 * margin > monitor.right - monitor.left
        || 2 * (height + margin) > monitor.bottom - monitor.top
    {
        return None;
    }
    let left = monitor.left + (monitor.right - monitor.left - width) / 2;
    let at = |top: i32| RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    };
    let bottom = at(monitor.bottom - margin - height);
    let top = at(monitor.top + margin);
    let covers = |pill: RECT| {
        selected.is_some_and(|selected| screen_rect(pill).overlap(screen_rect(selected)).is_some())
    };
    let pill = if covers(bottom) && !covers(top) {
        top
    } else {
        bottom
    };
    let mut x = pill.left + px(PADDING);
    let button_top = pill.top + (height - px(BUTTON_HEIGHT)) / 2;
    let controls = toolbar
        .controls()
        .iter()
        .map(|&control| {
            let right = x + px(button_width(control));
            let placed = RECT {
                left: x,
                top: button_top,
                right,
                bottom: button_top + px(BUTTON_HEIGHT),
            };
            x = right + px(GAP);
            (control, placed)
        })
        .collect();
    Some(Layout {
        toolbar,
        pill,
        controls,
    })
}

impl Layout {
    /// Where `control` sits, in screen pixels.
    pub(crate) fn area(&self, control: ToolbarControl) -> RECT {
        self.controls
            .iter()
            .find_map(|&(placed, at)| (placed == control).then_some(at))
            .expect("the layout places every control")
    }

    /// The control under `point`, if any.
    pub(crate) fn control_at(&self, point: POINT) -> Option<ToolbarControl> {
        self.controls
            .iter()
            .find(|&&(_, at)| screen_rect(at).holds(screen_point(point)))
            .map(|&(control, _)| control)
    }
}

/// D-117: how long the pointer rests on a control before its tooltip shows, as set in Windows.
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

/// D-117: tooltips over a window's buttons, one per button, each shown after the system's
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

/// D-117: the tooltips for `toolbar`'s buttons, in its order: each one's name and shortcut, or
/// `off`'s reason for a button that is off (F-57).
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

impl Layout {
    /// Where each of the toolbar's buttons sits in the client pixels of a window whose client
    /// area starts at `origin` on the desktop, in the toolbar's order.
    pub(crate) fn client_areas(&self, origin: POINT) -> impl Iterator<Item = RECT> + '_ {
        self.toolbar.controls().iter().map(move |&control| {
            let at = self.area(control);
            RECT {
                left: at.left - origin.x,
                top: at.top - origin.y,
                right: at.right - origin.x,
                bottom: at.bottom - origin.y,
            }
        })
    }
}

/// What a surface keeps to paint the toolbar. Over a frozen capture, the backdrop is that
/// capture blurred through a crop to the pill, so each paint blurs only what the pill shows;
/// a window over the live desktop has the material alone (CHARTER 4, "Live blur").
pub(crate) struct Paint {
    context: ID2D1DeviceContext,
    backdrop: Option<(ID2D1Effect, ID2D1ImageBrush)>,
    chosen: ID2D1SolidColorBrush,
    material: ID2D1SolidColorBrush,
    hairline: ID2D1SolidColorBrush,
    label: ID2D1SolidColorBrush,
    label_off: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    on_accent: ID2D1SolidColorBrush,
    text: TextStyle,
    scale: f32,
}

impl Paint {
    /// The toolbar's paint for a surface drawn by `context`, over `frozen` when the surface
    /// shows a frozen capture, in `palette`.
    pub(crate) fn new(
        context: &ID2D1DeviceContext,
        frozen: Option<&ID2D1Bitmap>,
        scale: f32,
        palette: &Palette,
    ) -> windows::core::Result<Self> {
        let text = Fonts::inter()?.text_style(LABEL_SIZE * scale)?.centred()?;
        let backdrop = frozen
            .map(|frozen| blurred(context, frozen, scale))
            .transpose()?;
        // SAFETY: every call takes owned or borrowed COM objects that outlive it.
        unsafe {
            let brush =
                |colour: D2D1_COLOR_F| context.CreateSolidColorBrush(&raw const colour, None);
            Ok(Self {
                backdrop,
                chosen: brush(palette.hairline)?,
                material: brush(palette.material)?,
                hairline: brush(palette.hairline)?,
                label: brush(palette.label)?,
                label_off: brush(palette.label_off)?,
                accent: brush(palette.accent)?,
                on_accent: brush(palette.on_accent)?,
                context: context.clone(),
                text,
                scale,
            })
        }
    }

    /// Draws the toolbar at `layout`, shifted by `origin` into this surface's pixels, with a
    /// ring round `focus` (CHARTER 4: visible keyboard focus) and each control as `look` says.
    #[expect(clippy::cast_precision_loss, reason = "screen coordinates fit in f32")]
    pub(crate) fn draw(
        &self,
        layout: &Layout,
        origin: POINT,
        focus: ToolbarControl,
        look: impl Fn(ToolbarControl) -> Look,
    ) -> windows::core::Result<()> {
        let local = |at: RECT| D2D_RECT_F {
            left: (at.left - origin.x) as f32,
            top: (at.top - origin.y) as f32,
            right: (at.right - origin.x) as f32,
            bottom: (at.bottom - origin.y) as f32,
        };
        let pill = local(layout.pill);
        let pill_round = D2D1_ROUNDED_RECT {
            rect: pill,
            radiusX: (pill.bottom - pill.top) / 2.0,
            radiusY: (pill.bottom - pill.top) / 2.0,
        };
        let reach = 3.0 * BLUR_DEVIATION * self.scale;
        let cropped = [
            pill.left - reach,
            pill.top - reach,
            pill.right + reach,
            pill.bottom + reach,
        ];
        let primary = primary(layout.toolbar);
        // SAFETY: the context, effect and brushes live with `self` for this call.
        unsafe {
            if let Some((crop, backdrop)) = &self.backdrop {
                crop.SetValue(
                    D2D1_CROP_PROP_RECT.0 as u32,
                    D2D1_PROPERTY_TYPE_VECTOR4,
                    &cropped.map(f32::to_ne_bytes).concat(),
                )?;
                self.context
                    .FillRoundedRectangle(&raw const pill_round, backdrop);
            }
            self.context
                .FillRoundedRectangle(&raw const pill_round, &self.material);
            self.context
                .DrawRoundedRectangle(&raw const pill_round, &self.hairline, 1.0, None);
            for &(control, at) in &layout.controls {
                let button = D2D1_ROUNDED_RECT {
                    rect: local(at),
                    radiusX: BUTTON_RADIUS * self.scale,
                    radiusY: BUTTON_RADIUS * self.scale,
                };
                let Look { label, mark } = look(control);
                if control == primary {
                    self.context
                        .FillRoundedRectangle(&raw const button, &self.accent);
                } else if mark == Mark::Chosen {
                    self.context
                        .FillRoundedRectangle(&raw const button, &self.chosen);
                }
                if control == focus {
                    let ring = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: button.rect.left - FOCUS_RING * self.scale,
                            top: button.rect.top - FOCUS_RING * self.scale,
                            right: button.rect.right + FOCUS_RING * self.scale,
                            bottom: button.rect.bottom + FOCUS_RING * self.scale,
                        },
                        radiusX: button.radiusX + FOCUS_RING * self.scale,
                        radiusY: button.radiusY + FOCUS_RING * self.scale,
                    };
                    self.context.DrawRoundedRectangle(
                        &raw const ring,
                        &self.label,
                        FOCUS_RING * self.scale,
                        None,
                    );
                }
                let brush = match (control == primary, mark) {
                    (true, _) => &self.on_accent,
                    (false, Mark::Off) => &self.label_off,
                    (false, Mark::Plain | Mark::Chosen) => &self.label,
                };
                self.text.draw(&self.context, &label, &button.rect, brush);
            }
        }
        Ok(())
    }
}

/// `frozen` cropped and blurred, and a brush that paints the result in the surface's pixels.
fn blurred(
    context: &ID2D1DeviceContext,
    frozen: &ID2D1Bitmap,
    scale: f32,
) -> windows::core::Result<(ID2D1Effect, ID2D1ImageBrush)> {
    // SAFETY: reading a bitmap's size touches no memory of ours.
    let size = unsafe { frozen.GetSize() };
    // SAFETY: every call takes owned or borrowed COM objects that outlive it.
    unsafe {
        let crop = context.CreateEffect(&CLSID_D2D1Crop)?;
        crop.SetInput(0, Some(&frozen.cast::<ID2D1Image>()?), true);
        let blur = context.CreateEffect(&CLSID_D2D1GaussianBlur)?;
        blur.SetInput(0, Some(&crop.GetOutput()?), true);
        blur.SetValue(
            D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
            D2D1_PROPERTY_TYPE_FLOAT,
            &(BLUR_DEVIATION * scale).to_ne_bytes(),
        )?;
        let whole = D2D1_IMAGE_BRUSH_PROPERTIES {
            sourceRectangle: D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: size.width,
                bottom: size.height,
            },
            extendModeX: D2D1_EXTEND_MODE_CLAMP,
            extendModeY: D2D1_EXTEND_MODE_CLAMP,
            interpolationMode: D2D1_INTERPOLATION_MODE_LINEAR,
        };
        let backdrop = context.CreateImageBrush(&blur.GetOutput()?, &raw const whole, None)?;
        Ok((crop, backdrop))
    }
}

/// Test seam: what the tooltips on a window say and where, read back from the control.
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
                let length = text
                    .iter()
                    .position(|&unit| unit == 0)
                    .unwrap_or(text.len());
                (String::from_utf16_lossy(&text[..length]), tool.rect)
            })
            .collect();
        Some((tools, send(TTM_GETDELAYTIME, TTDT_INITIAL as usize, 0)))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: RECT = RECT {
        left: -1920,
        top: 0,
        right: 0,
        bottom: 1080,
    };

    fn layout_at(selected: Option<RECT>) -> Layout {
        layout(Toolbar::Overlay, MONITOR, selected, 1.0).expect("a 1080p monitor holds the toolbar")
    }

    #[test]
    fn f05_the_toolbar_is_centred_near_the_bottom_of_its_monitor() {
        let pill = layout_at(None).pill;
        assert_eq!(pill.left - MONITOR.left, MONITOR.right - pill.right);
        assert_eq!(MONITOR.bottom - pill.bottom, 24);
    }

    #[test]
    fn f05_the_toolbar_moves_to_the_top_rather_than_cover_the_selection() {
        let low = RECT {
            left: -1000,
            top: 900,
            right: -900,
            bottom: 1070,
        };
        let pill = layout_at(Some(low)).pill;
        assert_eq!(pill.top - MONITOR.top, 24);
        let everything = MONITOR;
        assert_eq!(
            layout_at(Some(everything)).pill,
            layout_at(None).pill,
            "a selection under both places keeps the toolbar at the bottom"
        );
    }

    #[test]
    fn f05_on_a_monitor_above_the_primary_the_toolbar_keeps_its_edges() {
        let above = RECT {
            left: 0,
            top: -1080,
            right: 1920,
            bottom: 0,
        };
        let placed = |selected| {
            layout(Toolbar::Overlay, above, selected, 1.0).expect("a 1080p monitor holds it")
        };
        assert_eq!(placed(None).pill.bottom, -24);
        let low = RECT {
            left: 900,
            top: -100,
            right: 1000,
            bottom: -10,
        };
        assert_eq!(placed(Some(low)).pill.top, -1080 + 24);
    }

    #[test]
    fn f05_a_click_lands_on_the_control_drawn_there() {
        let placed = layout_at(None);
        for &(control, at) in &placed.controls {
            let middle = POINT {
                x: i32::midpoint(at.left, at.right),
                y: i32::midpoint(at.top, at.bottom),
            };
            assert_eq!(placed.control_at(middle), Some(control));
        }
        let outside = POINT {
            x: placed.pill.left - 1,
            y: placed.pill.top,
        };
        assert_eq!(placed.control_at(outside), None);
    }

    #[test]
    fn f11_each_toolbar_places_its_own_controls_left_to_right() {
        for toolbar in [Toolbar::Panel, Toolbar::Overlay] {
            let placed = layout(toolbar, MONITOR, None, 1.0).expect("a 1080p monitor holds it");
            let controls: Vec<_> = placed
                .controls
                .iter()
                .map(|&(control, _)| control)
                .collect();
            assert_eq!(controls, toolbar.controls(), "{toolbar:?}");
            assert!(
                placed
                    .controls
                    .windows(2)
                    .all(|pair| pair[0].1.right < pair[1].1.left),
                "{toolbar:?}"
            );
        }
    }

    #[test]
    fn f05_a_monitor_too_small_for_the_toolbar_shows_none() {
        let small = RECT {
            left: 0,
            top: 0,
            right: 64,
            bottom: 48,
        };
        assert_eq!(layout(Toolbar::Overlay, small, None, 1.0), None);
        assert!(
            layout(Toolbar::Overlay, MONITOR, None, 2.0)
                .is_some_and(|placed| placed.pill.bottom - placed.pill.top == 88)
        );
    }
}
