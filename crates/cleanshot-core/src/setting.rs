use crate::document::{Color, Fill, HighlightStrength, RedactMode, RedactStrength};
use crate::look::{ANNOTATION_RED, HIGHLIGHTER_YELLOW, Property};
use crate::text::{Alignment, DEFAULT_TEXT_SIZE, Horizontal, Vertical};
use crate::tool::Tool;

/// A setting on the property strip, without a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Color,
    StrokeWidth,
    Fill,
    Redact,
    RedactStrength,
    HighlightStrength,
    TextSize,
    /// On or off, shown as one check box, not a choice of values (D-114).
    Shadow,
    /// Across and down a bubble's box, picked from a grid the strip opens (D-129).
    Alignment,
    /// Across a text's lines only: a text box is as high as its lines (D-129).
    HorizontalAlignment,
}

impl Setting {
    /// The settings the strip shows for what `tool` draws, in strip order: none for the select
    /// and crop tools, which draw nothing.
    #[must_use]
    pub fn for_tool(tool: Tool) -> &'static [Self] {
        match tool {
            Tool::Arrow | Tool::Line => &[Self::Color, Self::StrokeWidth, Self::Shadow],
            Tool::Pen => &[Self::Color, Self::StrokeWidth],
            Tool::Rectangle | Tool::Ellipse => {
                &[Self::Color, Self::StrokeWidth, Self::Fill, Self::Shadow]
            }
            Tool::Highlighter => &[Self::Color, Self::HighlightStrength],
            Tool::Redact => &[Self::Redact, Self::RedactStrength],
            Tool::Text => &[
                Self::Color,
                Self::TextSize,
                Self::HorizontalAlignment,
                Self::Shadow,
            ],
            Tool::Bubble => &[Self::Color, Self::TextSize, Self::Alignment, Self::Shadow],
            Tool::Step => &[Self::Color, Self::TextSize, Self::Shadow],
            Tool::Select | Tool::Crop | Tool::Spotlight => &[],
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
        }
    }

    /// The values the strip offers for this setting, each with the name a screen reader reads,
    /// in strip order (D-55).
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
        }
    }
}

const fn color(r: u8, g: u8, b: u8) -> Property {
    Property::Color(Color { r, g, b, a: 255 })
}

const COLOR_CHOICES: [(Property, &str); 8] = [
    (Property::Color(ANNOTATION_RED), "Red"),
    (color(255, 149, 0), "Orange"),
    (Property::Color(HIGHLIGHTER_YELLOW), "Yellow"),
    (color(52, 199, 89), "Green"),
    (color(0, 122, 255), "Blue"),
    (color(175, 82, 222), "Purple"),
    (color(0, 0, 0), "Black"),
    (color(255, 255, 255), "White"),
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
    (Property::RedactStrength(RedactStrength::Fine), "16 pixels"),
    (
        Property::RedactStrength(RedactStrength::Medium),
        "24 pixels",
    ),
    (
        Property::RedactStrength(RedactStrength::Coarse),
        "32 pixels",
    ),
    (
        Property::RedactStrength(RedactStrength::Coarsest),
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
