use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1Crop, CLSID_D2D1GaussianBlur, D2D1_CROP_PROP_RECT, D2D1_EXTEND_MODE_CLAMP,
    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION, D2D1_IMAGE_BRUSH_PROPERTIES,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_PROPERTY_TYPE_FLOAT, D2D1_PROPERTY_TYPE_VECTOR4,
    D2D1_ROUNDED_RECT, ID2D1Bitmap, ID2D1DeviceContext, ID2D1Effect, ID2D1Image, ID2D1ImageBrush,
    ID2D1SolidColorBrush,
};
use windows::core::Interface;

use crate::palette::Palette;
use crate::text::{Fonts, Label};

pub(crate) mod automation;

/// A control on the capture toolbar (F-05).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Control {
    Copy,
    Save,
    Edit,
    Cancel,
}

/// The toolbar's controls in the order it shows them, Tab visits them and UI Automation lists
/// them.
pub(crate) const CONTROLS: [Control; 4] =
    [Control::Copy, Control::Save, Control::Edit, Control::Cancel];

impl Control {
    /// The control's accessible name.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Control::Copy => "Copy",
            Control::Save => "Save",
            Control::Edit => "Edit",
            Control::Cancel => "Cancel",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Control::Copy | Control::Save | Control::Edit => self.name(),
            Control::Cancel => "\u{d7}",
        }
    }

    fn width(self) -> f32 {
        match self {
            Control::Copy | Control::Save | Control::Edit => TEXT_BUTTON_WIDTH,
            Control::Cancel => ICON_BUTTON_WIDTH,
        }
    }

    /// The control `step` places after this one in Tab order, wrapping at either end.
    pub(crate) fn step(self, step: i32) -> Self {
        let at = CONTROLS
            .iter()
            .position(|&control| control == self)
            .expect("every control is in CONTROLS");
        let count = i32::try_from(CONTROLS.len()).expect("four controls");
        let next = (i32::try_from(at).expect("four controls") + step).rem_euclid(count);
        CONTROLS[usize::try_from(next).expect("rem_euclid is never negative")]
    }
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Layout {
    pub(crate) pill: RECT,
    pub(crate) controls: [(Control, RECT); 4],
}

/// The toolbar centred near the bottom of `monitor`, or near its top when that would cover
/// `selected` (F-05), at `scale` physical pixels per DIP. None when the monitor is too small
/// to hold it.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "toolbar sizes are a few hundred pixels and monitor edges fit in f32"
)]
pub(crate) fn layout(monitor: RECT, selected: Option<RECT>, scale: f32) -> Option<Layout> {
    let px = |dips: f32| (dips * scale).round() as i32;
    let width = px(PADDING * 2.0
        + CONTROLS.iter().map(|control| control.width()).sum::<f32>()
        + GAP * (CONTROLS.len() - 1) as f32);
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
    let covers = |pill: RECT| selected.is_some_and(|selected| overlaps(pill, selected));
    let pill = if covers(bottom) && !covers(top) {
        top
    } else {
        bottom
    };
    let mut x = pill.left + px(PADDING);
    let button_top = pill.top + (height - px(BUTTON_HEIGHT)) / 2;
    let controls = CONTROLS.map(|control| {
        let right = x + px(control.width());
        let placed = RECT {
            left: x,
            top: button_top,
            right,
            bottom: button_top + px(BUTTON_HEIGHT),
        };
        x = right + px(GAP);
        (control, placed)
    });
    Some(Layout { pill, controls })
}

fn overlaps(a: RECT, b: RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

impl Layout {
    /// The control under `point`, if any.
    pub(crate) fn control_at(&self, point: POINT) -> Option<Control> {
        self.controls
            .iter()
            .find(|(_, at)| {
                (at.left..at.right).contains(&point.x) && (at.top..at.bottom).contains(&point.y)
            })
            .map(|&(control, _)| control)
    }
}

/// What one monitor's surface keeps to paint the toolbar: the frozen capture blurred through
/// a crop to the pill, so each paint blurs only what the pill shows (CHARTER 4, "Live blur").
pub(crate) struct Paint {
    context: ID2D1DeviceContext,
    crop: ID2D1Effect,
    backdrop: ID2D1ImageBrush,
    material: ID2D1SolidColorBrush,
    hairline: ID2D1SolidColorBrush,
    label: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    on_accent: ID2D1SolidColorBrush,
    text: Label,
    scale: f32,
}

impl Paint {
    /// The toolbar's paint for a surface drawn by `context`, whose image is `frozen`, in
    /// `palette`.
    pub(crate) fn new(
        context: &ID2D1DeviceContext,
        frozen: &ID2D1Bitmap,
        scale: f32,
        palette: &Palette,
    ) -> windows::core::Result<Self> {
        // SAFETY: reading a bitmap's size touches no memory of ours.
        let size = unsafe { frozen.GetSize() };
        let text = Fonts::inter()?.label(LABEL_SIZE * scale)?.centred()?;
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
            let brush =
                |colour: D2D1_COLOR_F| context.CreateSolidColorBrush(&raw const colour, None);
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
            Ok(Self {
                backdrop,
                material: brush(palette.material)?,
                hairline: brush(palette.hairline)?,
                label: brush(palette.label)?,
                accent: brush(palette.accent)?,
                on_accent: brush(palette.on_accent)?,
                context: context.clone(),
                crop,
                text,
                scale,
            })
        }
    }

    /// Draws the toolbar at `layout`, shifted by `origin` into this surface's pixels, with a
    /// ring round `focus` (CHARTER 4: visible keyboard focus).
    #[expect(clippy::cast_precision_loss, reason = "screen coordinates fit in f32")]
    pub(crate) fn draw(
        &self,
        layout: &Layout,
        origin: POINT,
        focus: Control,
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
        // SAFETY: the context, effect and brushes live with `self` for this call.
        unsafe {
            self.crop.SetValue(
                D2D1_CROP_PROP_RECT.0 as u32,
                D2D1_PROPERTY_TYPE_VECTOR4,
                &cropped.map(f32::to_ne_bytes).concat(),
            )?;
            self.context
                .FillRoundedRectangle(&raw const pill_round, &self.backdrop);
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
                if control == Control::Copy {
                    self.context
                        .FillRoundedRectangle(&raw const button, &self.accent);
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
                let brush = if control == Control::Copy {
                    &self.on_accent
                } else {
                    &self.label
                };
                self.text
                    .draw(&self.context, control.label(), &button.rect, brush);
            }
        }
        Ok(())
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
        layout(MONITOR, selected, 1.0).expect("a 1080p monitor holds the toolbar")
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
    fn f05_a_click_lands_on_the_control_drawn_there_and_tab_wraps() {
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
        assert_eq!(Control::Cancel.step(1), Control::Copy);
        assert_eq!(Control::Copy.step(-1), Control::Cancel);
    }

    #[test]
    fn f05_a_monitor_too_small_for_the_toolbar_shows_none() {
        let small = RECT {
            left: 0,
            top: 0,
            right: 64,
            bottom: 48,
        };
        assert_eq!(layout(small, None, 1.0), None);
        assert!(
            layout(MONITOR, None, 2.0)
                .is_some_and(|placed| placed.pill.bottom - placed.pill.top == 88)
        );
    }
}
