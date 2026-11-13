use std::ops::RangeInclusive;

use crate::document::{
    BorderWidth, Color, Fill, HighlightStrength, RedactMode, RedactStrength, SpotlightDim,
};
use crate::look::{ANNOTATION_RED, HIGHLIGHTER_YELLOW, Look, Property, WHITE};
use crate::text::{Alignment, DEFAULT_TEXT_SIZE, Horizontal, Vertical};
use crate::tool::Tool;

/// A setting on the property strip, without a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Setting {
    Color,
    StrokeWidth,
    Fill,
    Redact,
    RedactStrength,
    HighlightStrength,
    TextSize,
    /// On or off, shown as one check box, not a choice of values.
    Shadow,
    /// Across and down a bubble's box, picked from a grid the strip opens.
    Alignment,
    /// Across a text's lines only: a text box is as high as its lines.
    HorizontalAlignment,
    /// On or off, shown as one check box beside Shadow.
    Border,
    BorderColor,
    BorderWidth,
    /// On or off, shown as one check box: a box behind a text.
    Background,
    BackgroundColor,
    /// How dark the image is outside every spotlight.
    Dim,
}

impl Setting {
    /// Every setting the strip can show for what `tool` draws, in strip order: none for the
    /// select and crop tools, which draw nothing, and a spotlight's border alone.
    #[must_use]
    pub(crate) fn for_tool(tool: Tool) -> &'static [Self] {
        match tool {
            Tool::Arrow | Tool::Line => &[
                Self::Color,
                Self::StrokeWidth,
                Self::Shadow,
                Self::Border,
                Self::BorderColor,
                Self::BorderWidth,
            ],
            Tool::Pen => &[Self::Color, Self::StrokeWidth],
            Tool::Rectangle | Tool::Ellipse => &[
                Self::Color,
                Self::StrokeWidth,
                Self::Fill,
                Self::Shadow,
                Self::Border,
                Self::BorderColor,
                Self::BorderWidth,
            ],
            Tool::Highlighter => &[
                Self::Color,
                Self::HighlightStrength,
                Self::Border,
                Self::BorderColor,
                Self::BorderWidth,
            ],
            Tool::Redact => &[Self::Redact, Self::RedactStrength],
            Tool::Text => &[
                Self::Color,
                Self::TextSize,
                Self::HorizontalAlignment,
                Self::Background,
                Self::BackgroundColor,
                Self::Shadow,
                Self::Border,
                Self::BorderColor,
                Self::BorderWidth,
            ],
            Tool::Bubble => &[
                Self::Color,
                Self::TextSize,
                Self::Alignment,
                Self::Shadow,
                Self::Border,
                Self::BorderColor,
                Self::BorderWidth,
            ],
            Tool::Step => &[
                Self::Color,
                Self::TextSize,
                Self::Shadow,
                Self::Border,
                Self::BorderColor,
                Self::BorderWidth,
            ],
            Tool::Spotlight => &[
                Self::Dim,
                Self::Border,
                Self::BorderColor,
                Self::BorderWidth,
            ],
            Tool::Select | Tool::Crop => &[],
        }
    }

    /// The settings the strip shows for `tool` drawing in `look`: [`Self::for_tool`]'s, less
    /// those that only set something `look` has switched off, as the border's colour and width
    /// while Border is off.
    #[must_use]
    pub fn shown_for(tool: Tool, look: &Look) -> Vec<Self> {
        Self::for_tool(tool)
            .iter()
            .copied()
            .filter(|setting| setting.is_switched_on(look))
            .collect()
    }

    fn is_switched_on(self, look: &Look) -> bool {
        match self {
            Self::BorderColor | Self::BorderWidth => look.border(),
            Self::BackgroundColor => look.background(),
            Self::Color
            | Self::StrokeWidth
            | Self::Fill
            | Self::Redact
            | Self::RedactStrength
            | Self::HighlightStrength
            | Self::TextSize
            | Self::Shadow
            | Self::Alignment
            | Self::HorizontalAlignment
            | Self::Border
            | Self::Background
            | Self::Dim => true,
        }
    }

    /// What the strip and a screen reader call this setting.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Color => "Colour",
            Self::StrokeWidth => "Stroke width",
            Self::Fill => "Fill",
            Self::Redact => "Redaction",
            Self::RedactStrength | Self::HighlightStrength => "Strength",
            Self::TextSize => "Text size",
            Self::Shadow => "Shadow",
            Self::Alignment | Self::HorizontalAlignment => "Alignment",
            Self::Border => "Border",
            Self::BorderColor => "Border colour",
            Self::BorderWidth => "Border width",
            Self::Background => "Background",
            Self::BackgroundColor => "Background colour",
            Self::Dim => "Dim",
        }
    }

    /// The value the strip lets the user type for this setting beside its choices, holding what
    /// `look` has now, or `None` for a setting with no typed value.
    #[must_use]
    pub fn typed(self, look: &Look) -> Option<TypedValue> {
        match self {
            Self::RedactStrength => Some(TypedValue {
                kind: Typed::RedactStrength,
                shown: f32::from(look.redact_strength().pixels()),
                warning: look
                    .redact_strength()
                    .may_be_readable()
                    .then_some(MAY_BE_READABLE),
            }),
            Self::TextSize => Some(TypedValue {
                kind: Typed::TextSize,
                shown: look.text_size(),
                warning: None,
            }),
            Self::Color
            | Self::StrokeWidth
            | Self::Fill
            | Self::Redact
            | Self::HighlightStrength
            | Self::Shadow
            | Self::Alignment
            | Self::HorizontalAlignment
            | Self::Border
            | Self::BorderColor
            | Self::BorderWidth
            | Self::Background
            | Self::BackgroundColor
            | Self::Dim => None,
        }
    }

    /// The colour this setting holds in `look`, which the colour picker shows and sets, or
    /// `None` for a setting that is not a colour.
    #[must_use]
    pub fn colored(self, look: &Look) -> Option<ColorValue> {
        match self {
            Self::Color => Some(ColorValue {
                kind: Colored::Shape,
                shown: look.style().color,
            }),
            Self::BorderColor => Some(ColorValue {
                kind: Colored::Border,
                shown: look.border_color(),
            }),
            Self::BackgroundColor => Some(ColorValue {
                kind: Colored::Background,
                shown: look.background_color(),
            }),
            Self::StrokeWidth
            | Self::Fill
            | Self::Redact
            | Self::RedactStrength
            | Self::HighlightStrength
            | Self::TextSize
            | Self::Shadow
            | Self::Alignment
            | Self::HorizontalAlignment
            | Self::Border
            | Self::BorderWidth
            | Self::Background
            | Self::Dim => None,
        }
    }

    /// The values the strip offers for this setting, each with the name a screen reader reads,
    /// in strip order.
    #[must_use]
    pub fn choices(self) -> &'static [(Property, &'static str)] {
        match self {
            Self::Color => &COLOR_CHOICES,
            Self::StrokeWidth => &STROKE_WIDTH_CHOICES,
            Self::Fill => &FILL_CHOICES,
            Self::Redact => &REDACT_CHOICES,
            Self::RedactStrength => &REDACT_STRENGTH_CHOICES,
            Self::HighlightStrength => &HIGHLIGHT_STRENGTH_CHOICES,
            Self::TextSize => &TEXT_SIZE_CHOICES,
            Self::Shadow => &SHADOW_CHOICES,
            Self::Alignment => &ALIGNMENT_CHOICES,
            Self::HorizontalAlignment => &HORIZONTAL_ALIGNMENT_CHOICES,
            Self::Border => &BORDER_CHOICES,
            Self::BorderColor => &BORDER_COLOR_CHOICES,
            Self::BorderWidth => &BORDER_WIDTH_CHOICES,
            Self::Background => &BACKGROUND_CHOICES,
            Self::BackgroundColor => &BACKGROUND_COLOR_CHOICES,
            Self::Dim => &DIM_CHOICES,
        }
    }
}

/// A value typed on the strip for a setting, as [`Setting::typed`] gives it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypedValue {
    kind: Typed,
    shown: f32,
    warning: Option<Warning>,
}

/// A colour setting's value, as [`Setting::colored`] gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorValue {
    kind: Colored,
    shown: Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Colored {
    Shape,
    Border,
    Background,
}

impl ColorValue {
    /// The colour the look holds now.
    #[must_use]
    pub fn shown(self) -> Color {
        self.shown
    }

    /// What picking `color` sets: the shape's colour, its border's or its text background's.
    #[must_use]
    pub fn property(self, color: Color) -> Property {
        match self.kind {
            Colored::Shape => Property::Color(color),
            Colored::Border => Property::BorderColor(color),
            Colored::Background => Property::BackgroundColor(color),
        }
    }

    /// The palette's colours, each with the name a screen reader reads, in palette order.
    #[must_use]
    pub fn palette() -> &'static [(Color, &'static str)] {
        &PALETTE
    }

    /// The palette's name for `color`, if the palette holds it.
    #[must_use]
    pub fn palette_name(color: Color) -> Option<&'static str> {
        PALETTE
            .iter()
            .find(|&&(entry, _)| entry == color)
            .map(|&(_, name)| name)
    }
}

/// A short warning the strip shows beside a typed value, and the reason its tooltip gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Warning {
    pub label: &'static str,
    pub reason: &'static str,
}

/// A redaction finer than 16 px is allowed, and said to be weak.
const MAY_BE_READABLE: Warning = Warning {
    label: "May be readable",
    reason: "Smaller blocks can leave text readable",
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Typed {
    RedactStrength,
    TextSize,
}

const TYPED_TEXT_SIZES: RangeInclusive<u16> = 8..=200;

impl TypedValue {
    /// What the look holds now, which the strip shows until a value is typed.
    #[must_use]
    pub fn shown(self) -> f32 {
        self.shown
    }

    /// What the strip warns about the value the look holds, if anything.
    #[must_use]
    pub fn warning(self) -> Option<Warning> {
        self.warning
    }

    /// The values a typed one is clamped into.
    #[must_use]
    pub fn range(self) -> RangeInclusive<u16> {
        match self.kind {
            Typed::RedactStrength => {
                RedactStrength::WEAKEST.pixels()..=RedactStrength::STRONGEST.pixels()
            }
            Typed::TextSize => TYPED_TEXT_SIZES,
        }
    }

    /// What typing `typed` sets: the nearest value in [`Self::range`].
    #[must_use]
    pub fn property(self, typed: u16) -> Property {
        let range = self.range();
        let used = typed.clamp(*range.start(), *range.end());
        match self.kind {
            Typed::RedactStrength => Property::RedactStrength(RedactStrength::clamped(used)),
            Typed::TextSize => Property::TextSize(f32::from(used)),
        }
    }
}

const fn opaque(r: u8, g: u8, b: u8) -> Color {
    Color { r, g, b, a: 255 }
}

const PALETTE: [(Color, &str); 11] = [
    (ANNOTATION_RED, "Red"),
    (opaque(255, 149, 0), "Orange"),
    (HIGHLIGHTER_YELLOW, "Yellow"),
    (opaque(52, 199, 89), "Green"),
    (opaque(0, 122, 255), "Blue"),
    (opaque(175, 82, 222), "Purple"),
    (opaque(255, 45, 85), "Pink"),
    (WHITE, "White"),
    (opaque(142, 142, 147), "Grey"),
    (opaque(0, 0, 0), "Black"),
    (opaque(48, 176, 199), "Teal"),
];

const COLOR_CHOICES: [(Property, &str); 11] = {
    let mut choices = [(Property::Color(WHITE), ""); 11];
    let mut index = 0;
    while index < PALETTE.len() {
        choices[index] = (Property::Color(PALETTE[index].0), PALETTE[index].1);
        index += 1;
    }
    choices
};

// A border can be any colour the strip offers a shape.
const BORDER_COLOR_CHOICES: [(Property, &str); 11] = {
    let mut choices = [(Property::BorderColor(WHITE), ""); 11];
    let mut index = 0;
    while index < PALETTE.len() {
        choices[index] = (Property::BorderColor(PALETTE[index].0), PALETTE[index].1);
        index += 1;
    }
    choices
};

// A text background can be any colour the strip offers a shape.
const BACKGROUND_COLOR_CHOICES: [(Property, &str); 11] = {
    let mut choices = [(Property::BackgroundColor(WHITE), ""); 11];
    let mut index = 0;
    while index < PALETTE.len() {
        choices[index] = (
            Property::BackgroundColor(PALETTE[index].0),
            PALETTE[index].1,
        );
        index += 1;
    }
    choices
};

const BACKGROUND_CHOICES: [(Property, &str); 2] = [
    (Property::Background(false), "No background"),
    (Property::Background(true), "Background"),
];

const BORDER_CHOICES: [(Property, &str); 2] = [
    (Property::Border(false), "No border"),
    (Property::Border(true), "Border"),
];

const BORDER_WIDTH_CHOICES: [(Property, &str); 3] = [
    (Property::BorderWidth(BorderWidth::Thin), "2 pixels"),
    (Property::BorderWidth(BorderWidth::Medium), "4 pixels"),
    (Property::BorderWidth(BorderWidth::Thick), "6 pixels"),
];

const DIM_CHOICES: [(Property, &str); 3] = [
    (Property::Dim(SpotlightDim::Light), "Light"),
    (Property::Dim(SpotlightDim::Medium), "Medium"),
    (Property::Dim(SpotlightDim::Dark), "Dark"),
];

const STROKE_WIDTH_CHOICES: [(Property, &str); 4] = [
    (Property::StrokeWidth(2.0), "2 pixels"),
    (Property::StrokeWidth(3.0), "3 pixels"),
    (Property::StrokeWidth(5.0), "5 pixels"),
    (Property::StrokeWidth(8.0), "8 pixels"),
];

const FILL_CHOICES: [(Property, &str); 2] = [
    (Property::Fill(Fill::Hollow), "Outline"),
    (Property::Fill(Fill::Solid), "Filled"),
];

const REDACT_CHOICES: [(Property, &str); 2] = [
    (Property::Redact(RedactMode::Pixelate), "Pixelate"),
    (Property::Redact(RedactMode::Blur), "Blur"),
];

const REDACT_STRENGTH_CHOICES: [(Property, &str); 4] = [
    (Property::RedactStrength(RedactStrength::FINE), "16 pixels"),
    (
        Property::RedactStrength(RedactStrength::MEDIUM),
        "24 pixels",
    ),
    (
        Property::RedactStrength(RedactStrength::COARSE),
        "32 pixels",
    ),
    (
        Property::RedactStrength(RedactStrength::COARSEST),
        "48 pixels",
    ),
];

const HIGHLIGHT_STRENGTH_CHOICES: [(Property, &str); 3] = [
    (
        Property::HighlightStrength(HighlightStrength::Light),
        "Light",
    ),
    (
        Property::HighlightStrength(HighlightStrength::Medium),
        "Medium",
    ),
    (
        Property::HighlightStrength(HighlightStrength::Strong),
        "Strong",
    ),
];

const TEXT_SIZE_CHOICES: [(Property, &str); 4] = [
    (Property::TextSize(16.0), "16 pixels"),
    (Property::TextSize(DEFAULT_TEXT_SIZE), "24 pixels"),
    (Property::TextSize(36.0), "36 pixels"),
    (Property::TextSize(48.0), "48 pixels"),
];

const SHADOW_CHOICES: [(Property, &str); 2] = [
    (Property::Shadow(false), "No shadow"),
    (Property::Shadow(true), "Shadow"),
];

const fn aligned(horizontal: Horizontal, vertical: Vertical) -> Property {
    Property::Alignment(Alignment {
        horizontal,
        vertical,
    })
}

const ALIGNMENT_CHOICES: [(Property, &str); 9] = [
    (aligned(Horizontal::Left, Vertical::Top), "Top left"),
    (aligned(Horizontal::Centre, Vertical::Top), "Top centre"),
    (aligned(Horizontal::Right, Vertical::Top), "Top right"),
    (aligned(Horizontal::Left, Vertical::Centre), "Centre left"),
    (aligned(Horizontal::Centre, Vertical::Centre), "Centre"),
    (aligned(Horizontal::Right, Vertical::Centre), "Centre right"),
    (aligned(Horizontal::Left, Vertical::Bottom), "Bottom left"),
    (
        aligned(Horizontal::Centre, Vertical::Bottom),
        "Bottom centre",
    ),
    (aligned(Horizontal::Right, Vertical::Bottom), "Bottom right"),
];

const HORIZONTAL_ALIGNMENT_CHOICES: [(Property, &str); 3] = [
    (aligned(Horizontal::Left, Vertical::Top), "Left"),
    (aligned(Horizontal::Centre, Vertical::Top), "Centre"),
    (aligned(Horizontal::Right, Vertical::Top), "Right"),
];

/// How many of the strip's groups, `widths` in order, are shown in `room`, `gap` apart; the
/// rest go behind a button `more` wide after them. All of them when they fit without it.
#[must_use]
pub fn groups_that_fit(widths: &[f32], room: f32, gap: f32, more: f32) -> usize {
    let mut used = -gap;
    let ends: Vec<f32> = widths
        .iter()
        .map(|width| {
            used += gap + width;
            used
        })
        .collect();
    if ends.last().is_none_or(|&all| all <= room) {
        return widths.len();
    }
    ends.iter()
        .take_while(|&&end| end + gap + more <= room)
        .count()
}

/// The colour picker's two controls: the shade square and the hue bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorControl {
    Shade,
    Hue,
}

/// How far one arrow key moves the hue, in degrees, or the saturation or brightness.
const HUE_STEP: f32 = 5.0;
const SHADE_STEP: f32 = 0.05;
const MOST_HUE: f32 = 359.9;

/// A colour as the picker moves it: `hue` in degrees from 0 up to 360, `saturation` and
/// `value` from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsv {
    pub hue: f32,
    pub saturation: f32,
    pub value: f32,
}

impl Hsv {
    /// `color` as the picker shows it; its alpha is not kept.
    #[must_use]
    pub fn of(color: Color) -> Self {
        let top = color.r.max(color.g).max(color.b);
        let [r, g, b] = [color.r, color.g, color.b].map(|channel| f32::from(channel) / 255.0);
        let most = f32::from(top) / 255.0;
        let range = most - r.min(g).min(b);
        let hue = if range == 0.0 {
            0.0
        } else if top == color.r {
            60.0 * ((g - b) / range).rem_euclid(6.0)
        } else if top == color.g {
            60.0 * ((b - r) / range + 2.0)
        } else {
            60.0 * ((r - g) / range + 4.0)
        };
        Self {
            hue,
            saturation: if most == 0.0 { 0.0 } else { range / most },
            value: most,
        }
    }

    /// This colour in red, green and blue, fully opaque.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "each channel is rounded from 0 to 255 before the cast"
    )]
    #[must_use]
    pub fn color(self) -> Color {
        let chroma = self.value * self.saturation;
        let sector = (self.hue.rem_euclid(360.0)) / 60.0;
        let second = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
        let (r, g, b) = match sector as u8 {
            0 => (chroma, second, 0.0),
            1 => (second, chroma, 0.0),
            2 => (0.0, chroma, second),
            3 => (0.0, second, chroma),
            4 => (second, 0.0, chroma),
            _ => (chroma, 0.0, second),
        };
        let lift = self.value - chroma;
        let channel = |part: f32| ((part + lift).clamp(0.0, 1.0) * 255.0).round() as u8;
        Color {
            r: channel(r),
            g: channel(g),
            b: channel(b),
            a: 255,
        }
    }

    /// This colour with the part `control` sets moved to `across` and `down`, each a fraction
    /// from 0 to 1 of the control.
    #[must_use]
    pub fn at(self, control: ColorControl, across: f32, down: f32) -> Self {
        match control {
            ColorControl::Shade => Self {
                saturation: across,
                value: 1.0 - down,
                ..self
            },
            ColorControl::Hue => Self {
                hue: (across * 360.0).min(MOST_HUE),
                ..self
            },
        }
    }

    /// This colour after the arrow `key` on `control`: Left and Right step the hue, or the
    /// saturation on the shade square, where Up and Down step the brightness. `None` for any
    /// other key.
    #[must_use]
    pub fn stepped(self, control: ColorControl, key: &str) -> Option<Self> {
        let shade = |saturation: f32, value: f32| Self {
            saturation: (self.saturation + saturation).clamp(0.0, 1.0),
            value: (self.value + value).clamp(0.0, 1.0),
            ..self
        };
        let hue = |by: f32| Self {
            hue: (self.hue + by).clamp(0.0, MOST_HUE),
            ..self
        };
        match control {
            ColorControl::Hue => match key {
                "left" => Some(hue(-HUE_STEP)),
                "right" => Some(hue(HUE_STEP)),
                _ => None,
            },
            ColorControl::Shade => match key {
                "left" => Some(shade(-SHADE_STEP, 0.0)),
                "right" => Some(shade(SHADE_STEP, 0.0)),
                "up" => Some(shade(0.0, SHADE_STEP)),
                "down" => Some(shade(0.0, -SHADE_STEP)),
                _ => None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_shows_the_groups_that_fit_before_its_more_button_and_all_when_all_fit() {
        let widths = [100.0, 60.0, 80.0];
        assert_eq!(groups_that_fit(&widths, 256.0, 8.0, 28.0), 3);
        assert_eq!(groups_that_fit(&widths, 255.0, 8.0, 28.0), 2);
        assert_eq!(groups_that_fit(&widths, 204.0, 8.0, 28.0), 2);
        assert_eq!(groups_that_fit(&widths, 203.0, 8.0, 28.0), 1);
        assert_eq!(groups_that_fit(&widths, 135.0, 8.0, 28.0), 0);
        assert_eq!(groups_that_fit(&[], 0.0, 8.0, 28.0), 0);
    }

    #[test]
    fn a_typed_strength_or_text_size_is_clamped_into_its_range_and_shown_once_set() {
        let redact = Look::default_for(Tool::Redact);
        let strength = Setting::RedactStrength.typed(&redact).unwrap();
        assert_eq!(strength.shown(), 16.0);
        let used = |typed: TypedValue, value| match typed.property(value) {
            Property::RedactStrength(strength) => f32::from(strength.pixels()),
            Property::TextSize(size) => size,
            other => panic!("a strength or a size, not {other:?}"),
        };
        assert_eq!(
            [4, 64, 999].map(|value| used(strength, value)),
            [8.0, 64.0, 512.0]
        );
        let text = Look::default_for(Tool::Text);
        let size = Setting::TextSize.typed(&text).unwrap();
        assert_eq!(size.shown(), DEFAULT_TEXT_SIZE);
        assert_eq!(
            [2, 60, 999].map(|value| used(size, value)),
            [8.0, 60.0, 200.0]
        );
        let set = text.with(size.property(999)).unwrap();
        assert_eq!(
            Setting::TextSize.typed(&set).map(TypedValue::shown),
            Some(200.0)
        );
        assert_eq!(Setting::Color.typed(&text), None);
    }

    #[test]
    fn the_border_colour_and_width_show_only_while_border_is_ticked() {
        let border_rows = [Setting::BorderColor, Setting::BorderWidth];
        for tool in [
            Tool::Arrow,
            Tool::Rectangle,
            Tool::Text,
            Tool::Bubble,
            Tool::Step,
        ] {
            let off = Look::default_for(tool);
            assert!(!off.border(), "{tool:?}");
            let shown = Setting::shown_for(tool, &off);
            assert!(shown.contains(&Setting::Border), "{tool:?}");
            assert!(
                !border_rows.iter().any(|row| shown.contains(row)),
                "{tool:?}"
            );
            let on = off.with(Property::Border(true)).unwrap();
            let shown = Setting::shown_for(tool, &on);
            assert!(
                border_rows.iter().all(|row| shown.contains(row)),
                "{tool:?}"
            );
        }
    }

    #[test]
    fn the_palette_holds_eleven_colours_and_a_picked_colour_sets_the_shape_or_its_border() {
        let look = Look::default_for(Tool::Rectangle);
        let palette = Setting::Color.choices();
        assert_eq!(palette.len(), 11);
        assert!(palette.iter().any(|&(property, _)| look.has(property)));
        let teal = Color::from_hex_digits("17a2a8").unwrap();
        let shape = Setting::Color.colored(&look).unwrap();
        assert_eq!(shape.shown(), look.style().color);
        assert_eq!(shape.property(teal), Property::Color(teal));
        let border = Setting::BorderColor.colored(&look).unwrap();
        assert_eq!(border.property(teal), Property::BorderColor(teal));
        let bordered = look.with(border.property(teal)).unwrap();
        assert_eq!(
            Setting::BorderColor.colored(&bordered).unwrap().shown(),
            teal
        );
        assert_eq!(Setting::StrokeWidth.colored(&look), None);
        assert_eq!(ColorValue::palette_name(WHITE), Some("White"));
        assert_eq!(ColorValue::palette_name(teal), None);
    }

    #[test]
    fn a_strength_under_16_px_is_kept_and_warned_about_and_16_or_more_is_not() {
        let redact = Look::default_for(Tool::Redact);
        let strength = Setting::RedactStrength.typed(&redact).unwrap();
        let warned = |value| {
            let look = redact.with(strength.property(value)).unwrap();
            let typed = Setting::RedactStrength.typed(&look).unwrap();
            (typed.shown(), typed.warning())
        };
        assert_eq!(warned(8), (8.0, Some(MAY_BE_READABLE)));
        assert_eq!(warned(15), (15.0, Some(MAY_BE_READABLE)));
        assert_eq!(warned(16), (16.0, None));
        assert_eq!(strength.warning(), None, "the default is not warned about");
        let text = Look::default_for(Tool::Text);
        assert_eq!(Setting::TextSize.typed(&text).unwrap().warning(), None);
    }

    #[test]
    fn a_colour_comes_back_from_the_picker_as_it_went_in() {
        for hex in [
            "17A2A8", "FF3B30", "000000", "FFFFFF", "8E8E93", "AF52DE", "123456",
        ] {
            let color = Color::from_hex_digits(hex).unwrap();
            assert_eq!(Hsv::of(color).color(), color, "{hex}");
        }
        let teal = Hsv::of(Color::from_hex_digits("17A2A8").unwrap());
        assert!((teal.hue - 182.5).abs() < 0.1, "{teal:?}");
    }

    #[test]
    fn the_arrow_keys_step_the_hue_bar_and_the_shade_square() {
        let draft = Hsv {
            hue: 120.0,
            saturation: 0.5,
            value: 0.98,
        };
        let step = |control, key| draft.stepped(control, key);
        assert_eq!(
            step(ColorControl::Hue, "right"),
            Some(Hsv {
                hue: 125.0,
                ..draft
            })
        );
        assert_eq!(
            step(ColorControl::Hue, "left"),
            Some(Hsv {
                hue: 115.0,
                ..draft
            })
        );
        assert_eq!(step(ColorControl::Hue, "up"), None);
        let up = step(ColorControl::Shade, "up").unwrap();
        assert_eq!((up.saturation, up.value), (0.5, 1.0));
        let right = step(ColorControl::Shade, "right").unwrap();
        assert!((right.saturation - 0.55).abs() < 1e-6 && (right.value - draft.value).abs() < 1e-6);
        assert_eq!(step(ColorControl::Shade, "a"), None);
    }
}
