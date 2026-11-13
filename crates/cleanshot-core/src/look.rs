use crate::document::{
    Border, BorderWidth, Color, Fill, HighlightStrength, Opacity, RedactMode, RedactStrength,
    Shape, ShapeEntry, SpotlightDim, Style,
};
use crate::text::{Alignment, DEFAULT_TEXT_SIZE, TextBlock};
use crate::tool::Tool;

pub(crate) const ANNOTATION_RED: Color = Color {
    r: 255,
    g: 59,
    b: 48,
    a: 255,
};

pub(crate) const HIGHLIGHTER_YELLOW: Color = Color {
    r: 255,
    g: 230,
    b: 0,
    a: 255,
};

pub(crate) const WHITE: Color = Color {
    r: 255,
    g: 255,
    b: 255,
    a: 255,
};

/// How a tool draws its next shape: what the property strip sets for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    style: Style,
    fill: Fill,
    redact: RedactMode,
    redact_strength: RedactStrength,
    highlight_strength: HighlightStrength,
    text_size: f32,
    alignment: Alignment,
    shadow: bool,
    border: bool,
    border_color: Color,
    border_width: BorderWidth,
    background: bool,
    background_color: Color,
    opacity: Opacity,
    dim: SpotlightDim,
}

impl Look {
    /// What `tool` draws in until the strip changes it.
    #[must_use]
    pub fn default_for(tool: Tool) -> Self {
        let style = match tool {
            Tool::Highlighter => Style::new(HIGHLIGHTER_YELLOW, 0.0),
            Tool::Select
            | Tool::Arrow
            | Tool::Line
            | Tool::Rectangle
            | Tool::Ellipse
            | Tool::Pen
            | Tool::Redact
            | Tool::Crop
            | Tool::Text
            | Tool::Spotlight
            | Tool::Step
            | Tool::Bubble => Style::new(ANNOTATION_RED, 3.0),
        };
        Self {
            style,
            fill: Fill::Hollow,
            redact: RedactMode::Pixelate,
            redact_strength: RedactStrength::FINE,
            highlight_strength: HighlightStrength::Strong,
            text_size: DEFAULT_TEXT_SIZE,
            alignment: Alignment::default(),
            shadow: false,
            border: false,
            border_color: WHITE,
            border_width: BorderWidth::Thin,
            background: false,
            background_color: WHITE,
            opacity: Opacity::FULL,
            dim: SpotlightDim::Medium,
        }
    }

    /// How dark the image is outside every spotlight, for a spotlight's look.
    #[must_use]
    pub fn dim(&self) -> SpotlightDim {
        self.dim
    }

    /// Whether this look's shapes cast a shadow, where the shape can cast one.
    #[must_use]
    pub fn shadow(&self) -> bool {
        self.shadow
    }

    /// Whether this look's shapes have a border, where the shape can have one.
    #[must_use]
    pub fn border(&self) -> bool {
        self.border
    }

    /// The colour of this look's border, used while [`Self::border`] is on.
    pub(crate) fn border_color(&self) -> Color {
        self.border_color
    }

    /// The width of this look's border, used while [`Self::border`] is on.
    pub(crate) fn border_width(&self) -> BorderWidth {
        self.border_width
    }

    /// Whether this look's text is drawn on a box, where the shape can be.
    #[must_use]
    pub fn background(&self) -> bool {
        self.background
    }

    /// The colour of this look's text box, used while [`Self::background`] is on.
    pub(crate) fn background_color(&self) -> Color {
        self.background_color
    }

    /// How opaque this look's fill is, where the shape has one.
    #[must_use]
    pub fn opacity(&self) -> Opacity {
        self.opacity
    }

    #[must_use]
    pub fn style(&self) -> Style {
        self.style
    }

    /// The font size, in image pixels, always positive.
    #[must_use]
    pub fn text_size(&self) -> f32 {
        self.text_size
    }

    /// Where text sits in its box.
    #[must_use]
    pub fn alignment(&self) -> Alignment {
        self.alignment
    }

    /// What a box-shaped shape is filled with: an outline, or the style colour.
    #[must_use]
    pub fn fill(&self) -> Fill {
        self.fill
    }

    /// How a redaction replaces its pixels.
    #[must_use]
    pub fn redact(&self) -> RedactMode {
        self.redact
    }

    /// How coarse a redaction is.
    #[must_use]
    pub(crate) fn redact_strength(&self) -> RedactStrength {
        self.redact_strength
    }

    /// How strongly a highlight tints what is under it.
    #[must_use]
    pub(crate) fn highlight_strength(&self) -> HighlightStrength {
        self.highlight_strength
    }

    /// `shape` drawn in this look: in its tool's style ([`Self::style_for`]), and with its fill,
    /// redaction, highlight strength, text size or alignment where `shape` has one. New shapes
    /// and restyled ones both take their look through here.
    #[must_use]
    pub fn dress(&self, shape: Shape) -> (Shape, Style) {
        let style = self.style_for(Tool::drawing(&shape));
        let shape = match shape {
            Shape::Rect { rect, .. } => Shape::Rect {
                rect,
                fill: self.fill,
            },
            Shape::Ellipse { rect, .. } => Shape::Ellipse {
                rect,
                fill: self.fill,
            },
            Shape::Redact { rect, .. } => Shape::Redact {
                rect,
                mode: self.redact,
                strength: self.redact_strength,
            },
            Shape::Highlight { rect, .. } => Shape::Highlight {
                rect,
                strength: self.highlight_strength,
            },
            Shape::Text(block) => Shape::Text(self.dress_text(&block)),
            Shape::Step(step) => Shape::Step(step.with_size(self.text_size)),
            Shape::Bubble(bubble) => {
                Shape::Bubble(bubble.with_block(self.dress_text(bubble.block())))
            }
            shape @ (Shape::Arrow { .. }
            | Shape::Line { .. }
            | Shape::Pen(_)
            | Shape::Spotlight(_)) => shape,
        };
        (shape, style)
    }

    /// The style `tool`'s shapes are drawn in: this look's, with its shadow, border, text
    /// background and fill opacity where they take them. Nothing else gives a shape any of them.
    #[must_use]
    pub fn style_for(&self, tool: Tool) -> Style {
        let border = Border {
            color: self.border_color,
            width: self.border_width,
        };
        self.style
            .with_shadow(self.shadow && takes_shadow(tool))
            .with_border((self.border && takes_border(tool)).then_some(border))
            .with_background(
                (self.background && tool == Tool::Text).then_some(self.background_color),
            )
            .with_opacity(if takes_fill(tool) {
                self.opacity
            } else {
                Opacity::FULL
            })
    }

    pub(crate) fn dress_text(&self, block: &TextBlock) -> TextBlock {
        block
            .with_size(self.text_size)
            .with_alignment(self.alignment)
    }

    /// Whether `property` is already this look's value, which the strip shows as checked.
    #[must_use]
    pub fn has(&self, property: Property) -> bool {
        self.with(property) == Ok(*self)
    }

    /// This look with `property` changed.
    ///
    /// # Errors
    ///
    /// [`PropertyError::NotPositive`] for a stroke width or text size that is not a positive
    /// number.
    pub fn with(self, property: Property) -> Result<Self, PropertyError> {
        let positive = |value: f32| {
            (value.is_finite() && value > 0.0)
                .then_some(value)
                .ok_or(PropertyError::NotPositive(value))
        };
        Ok(match property {
            Property::Color(color) => self.with_color(color),
            Property::StrokeWidth(width) => Self {
                style: Style::new(self.style.color, positive(width)?),
                ..self
            },
            Property::Fill(fill) => Self { fill, ..self },
            Property::Redact(redact) => Self { redact, ..self },
            Property::RedactStrength(redact_strength) => Self {
                redact_strength,
                ..self
            },
            Property::HighlightStrength(highlight_strength) => Self {
                highlight_strength,
                ..self
            },
            Property::TextSize(size) => Self {
                text_size: positive(size)?,
                ..self
            },
            Property::Shadow(shadow) => Self { shadow, ..self },
            Property::Border(border) => Self { border, ..self },
            Property::BorderColor(border_color) => Self {
                border_color,
                ..self
            },
            Property::BorderWidth(border_width) => Self {
                border_width,
                ..self
            },
            Property::Alignment(alignment) => Self { alignment, ..self },
            Property::Background(background) => Self { background, ..self },
            Property::BackgroundColor(background_color) => Self {
                background_color,
                ..self
            },
            Property::Opacity(opacity) => Self { opacity, ..self },
            Property::Dim(dim) => Self { dim, ..self },
        })
    }

    pub(crate) fn with_color(self, color: Color) -> Self {
        Self {
            style: Style::new(color, self.style.stroke_width),
            ..self
        }
    }

    /// The look `entry` is drawn in: its own style, shadow, border, text background, fill and its
    /// opacity, redaction, highlight strength, text size or alignment, and the rest from `rest`.
    pub(crate) fn of(entry: &ShapeEntry, rest: Self) -> Self {
        let look = Self {
            style: entry
                .style
                .with_shadow(false)
                .with_border(None)
                .with_background(None)
                .with_opacity(Opacity::FULL),
            shadow: entry.style.shadow(),
            border: entry.style.border().is_some(),
            border_color: entry
                .style
                .border()
                .map_or(rest.border_color, |border| border.color),
            border_width: entry
                .style
                .border()
                .map_or(rest.border_width, |border| border.width),
            background: entry.style.background().is_some(),
            background_color: entry.style.background().unwrap_or(rest.background_color),
            opacity: entry.style.opacity(),
            ..rest
        };
        match &entry.shape {
            Shape::Rect { fill, .. } | Shape::Ellipse { fill, .. } => Self {
                fill: *fill,
                ..look
            },
            Shape::Redact { mode, strength, .. } => Self {
                redact: *mode,
                redact_strength: *strength,
                ..look
            },
            Shape::Highlight { strength, .. } => Self {
                highlight_strength: *strength,
                ..look
            },
            Shape::Text(block) => Self {
                text_size: block.size(),
                alignment: block.alignment(),
                ..look
            },
            Shape::Step(step) => Self {
                text_size: step.size(),
                ..look
            },
            Shape::Bubble(bubble) => Self {
                text_size: bubble.block().size(),
                alignment: bubble.block().alignment(),
                ..look
            },
            Shape::Arrow { .. } | Shape::Line { .. } | Shape::Pen(_) | Shape::Spotlight(_) => look,
        }
    }
}

/// One setting on the property strip, with its new value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Property {
    Color(Color),
    StrokeWidth(f32),
    Fill(Fill),
    Redact(RedactMode),
    RedactStrength(RedactStrength),
    HighlightStrength(HighlightStrength),
    TextSize(f32),
    Shadow(bool),
    Alignment(Alignment),
    Border(bool),
    BorderColor(Color),
    BorderWidth(BorderWidth),
    /// A box behind a text, in its own colour, on or off.
    Background(bool),
    BackgroundColor(Color),
    Opacity(Opacity),
    /// One setting for the whole image, which the next image also starts with.
    Dim(SpotlightDim),
}

/// Whether the shapes `tool` draws can cast a shadow.
pub(crate) fn takes_shadow(tool: Tool) -> bool {
    match tool {
        Tool::Arrow
        | Tool::Line
        | Tool::Rectangle
        | Tool::Ellipse
        | Tool::Text
        | Tool::Step
        | Tool::Bubble => true,
        Tool::Select
        | Tool::Pen
        | Tool::Highlighter
        | Tool::Redact
        | Tool::Crop
        | Tool::Spotlight => false,
    }
}

/// Whether the shapes `tool` draws can have a fill, so a fill opacity.
pub(crate) fn takes_fill(tool: Tool) -> bool {
    match tool {
        Tool::Rectangle | Tool::Ellipse | Tool::Text | Tool::Step | Tool::Bubble => true,
        Tool::Select
        | Tool::Arrow
        | Tool::Line
        | Tool::Pen
        | Tool::Highlighter
        | Tool::Redact
        | Tool::Crop
        | Tool::Spotlight => false,
    }
}

/// Whether the shapes `tool` draws can have a border: a redaction never does.
pub(crate) fn takes_border(tool: Tool) -> bool {
    match tool {
        Tool::Arrow
        | Tool::Line
        | Tool::Rectangle
        | Tool::Ellipse
        | Tool::Text
        | Tool::Step
        | Tool::Bubble
        | Tool::Highlighter
        | Tool::Spotlight => true,
        Tool::Select | Tool::Pen | Tool::Redact | Tool::Crop => false,
    }
}

/// Why the strip refused a value.
#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum PropertyError {
    #[error("{0} is not a positive size")]
    NotPositive(f32),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{PointF, RectF};
    use crate::strip::Setting;
    use crate::text::TextBlock;

    const AREA: RectF = RectF {
        x: 1.0,
        y: 2.0,
        width: 30.0,
        height: 20.0,
    };

    fn look(tool: Tool, fill: Fill, redact: RedactMode, text_size: f32) -> Look {
        Look {
            fill,
            redact,
            redact_strength: RedactStrength::COARSE,
            text_size,
            ..Look::default_for(tool)
        }
    }

    #[test]
    fn arrows_and_lines_default_to_red_3_px() {
        let style = Look::default_for(Tool::Arrow).style();
        assert_eq!((style.color.r, style.color.g, style.color.b), (255, 59, 48));
        assert_eq!(style.stroke_width, 3.0);
        assert_eq!(Look::default_for(Tool::Line).style(), style);
    }

    #[test]
    fn the_highlighter_defaults_to_yellow() {
        let color = Look::default_for(Tool::Highlighter).style().color;
        assert_eq!((color.r, color.g, color.b), (255, 230, 0));
    }

    #[test]
    fn a_look_gives_a_shape_its_style_and_its_own_fill_redaction_or_text_size() {
        let solid = look(Tool::Rectangle, Fill::Solid, RedactMode::Blur, 40.0);
        let style = solid.style();
        let hollow_box = Shape::Ellipse {
            rect: AREA,
            fill: Fill::Hollow,
        };
        let solid_box = Shape::Ellipse {
            rect: AREA,
            fill: Fill::Solid,
        };
        assert_eq!(solid.dress(hollow_box), (solid_box, style));
        let pixelated = Shape::Redact {
            rect: AREA,
            mode: RedactMode::Pixelate,
            strength: RedactStrength::FINE,
        };
        let blurred = Shape::Redact {
            rect: AREA,
            mode: RedactMode::Blur,
            strength: RedactStrength::COARSE,
        };
        assert_eq!(solid.dress(pixelated), (blurred, style));
        let at = PointF { x: 5.0, y: 6.0 };
        let small = TextBlock::new(at, "Hi".into(), 12.0, 1.5).unwrap();
        let large = TextBlock::new(at, "Hi".into(), 40.0, 1.5).unwrap();
        assert_eq!(solid.dress(Shape::Text(small)), (Shape::Text(large), style));
        let line = Shape::Line {
            from: at,
            to: PointF { x: 9.0, y: 9.0 },
        };
        assert_eq!(solid.dress(line.clone()), (line, style));
    }

    #[test]
    fn every_strip_choice_is_accepted_and_each_tool_starts_on_one_choice_per_setting() {
        for tool in [
            Tool::Arrow,
            Tool::Line,
            Tool::Rectangle,
            Tool::Ellipse,
            Tool::Pen,
            Tool::Highlighter,
            Tool::Text,
            Tool::Redact,
            Tool::Spotlight,
        ] {
            let look = Look::default_for(tool);
            for setting in Setting::for_tool(tool) {
                let choices = setting.choices();
                assert!(choices.iter().all(|(choice, _)| look.with(*choice).is_ok()));
                let checked = choices
                    .iter()
                    .filter(|(choice, _)| look.has(*choice))
                    .count();
                assert_eq!(checked, 1, "{tool:?} {setting:?}");
            }
        }
    }
}
