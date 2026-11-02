use std::collections::HashMap;

use crate::document::{Color, Fill, RedactMode, Shape, ShapeEntry, ShapeId, Style};
use crate::history::Command;
use crate::text::{DEFAULT_TEXT_SIZE, TextBlock};
use crate::text_edit::{TextFrame, TextOrigin};
use crate::tool::Tool;

const ANNOTATION_RED: Color = Color {
    r: 255,
    g: 59,
    b: 48,
    a: 255,
};

const HIGHLIGHTER_YELLOW: Color = Color {
    r: 255,
    g: 230,
    b: 0,
    a: 255,
};

/// How a tool draws its next shape: what the property strip sets for it (CHARTER 4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    style: Style,
    fill: Fill,
    redact: RedactMode,
    text_size: f32,
}

impl Look {
    /// What `tool` draws in until the strip changes it (F-31, F-32, F-35, F-36, D-42, D-50).
    #[must_use]
    pub fn default_for(tool: Tool) -> Self {
        let style = match tool {
            Tool::Highlighter => Style {
                color: HIGHLIGHTER_YELLOW,
                stroke_width: 0.0,
            },
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
            | Tool::Bubble => Style {
                color: ANNOTATION_RED,
                stroke_width: 3.0,
            },
        };
        Self {
            style,
            fill: Fill::Hollow,
            redact: RedactMode::Pixelate,
            text_size: DEFAULT_TEXT_SIZE,
        }
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

    /// `shape` drawn in this look: in its style, and with its fill, redaction or text size where
    /// `shape` has one. New shapes and restyled ones both take their look through here.
    #[must_use]
    pub fn dress(&self, shape: Shape) -> (Shape, Style) {
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
            },
            Shape::Text(block) => Shape::Text(self.dress_text(&block)),
            Shape::Step(step) => Shape::Step(step.with_size(self.text_size)),
            Shape::Bubble(bubble) => {
                Shape::Bubble(bubble.with_block(self.dress_text(bubble.block())))
            }
            shape @ (Shape::Arrow { .. }
            | Shape::Line { .. }
            | Shape::Pen(_)
            | Shape::Highlight(_)
            | Shape::Spotlight(_)) => shape,
        };
        (shape, self.style)
    }

    fn dress_text(&self, block: &TextBlock) -> TextBlock {
        block.with_size(self.text_size)
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
            Property::Color(color) => Self {
                style: Style {
                    color,
                    ..self.style
                },
                ..self
            },
            Property::StrokeWidth(width) => Self {
                style: Style {
                    stroke_width: positive(width)?,
                    ..self.style
                },
                ..self
            },
            Property::Fill(fill) => Self { fill, ..self },
            Property::Redact(redact) => Self { redact, ..self },
            Property::TextSize(size) => Self {
                text_size: positive(size)?,
                ..self
            },
        })
    }

    /// The look `entry` is drawn in: its own style, fill, redaction or text size, and the rest
    /// from `rest`.
    fn of(entry: &ShapeEntry, rest: Self) -> Self {
        let look = Self {
            style: entry.style,
            ..rest
        };
        match &entry.shape {
            Shape::Rect { fill, .. } | Shape::Ellipse { fill, .. } => Self {
                fill: *fill,
                ..look
            },
            Shape::Redact { mode, .. } => Self {
                redact: *mode,
                ..look
            },
            Shape::Text(block) => Self {
                text_size: block.size(),
                ..look
            },
            Shape::Step(step) => Self {
                text_size: step.size(),
                ..look
            },
            Shape::Bubble(bubble) => Self {
                text_size: bubble.block().size(),
                ..look
            },
            Shape::Arrow { .. }
            | Shape::Line { .. }
            | Shape::Pen(_)
            | Shape::Highlight(_)
            | Shape::Spotlight(_) => look,
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
    TextSize(f32),
}

/// A setting on the property strip, without a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Color,
    StrokeWidth,
    Fill,
    Redact,
    TextSize,
}

impl Setting {
    /// The settings the strip shows for what `tool` draws, in strip order: none for the select
    /// and crop tools, which draw nothing.
    #[must_use]
    pub fn for_tool(tool: Tool) -> &'static [Self] {
        match tool {
            Tool::Arrow | Tool::Line | Tool::Pen => &[Self::Color, Self::StrokeWidth],
            Tool::Rectangle | Tool::Ellipse => &[Self::Color, Self::StrokeWidth, Self::Fill],
            Tool::Highlighter => &[Self::Color],
            Tool::Redact => &[Self::Redact],
            Tool::Text | Tool::Step | Tool::Bubble => &[Self::Color, Self::TextSize],
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
            Self::TextSize => "Text size",
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
            Self::TextSize => &TEXT_SIZE_CHOICES,
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

const TEXT_SIZE_CHOICES: [(Property, &str); 4] = [
    (Property::TextSize(16.0), "16 pixels"),
    (Property::TextSize(DEFAULT_TEXT_SIZE), "24 pixels"),
    (Property::TextSize(36.0), "36 pixels"),
    (Property::TextSize(48.0), "48 pixels"),
];

/// Why the strip refused a value.
#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum PropertyError {
    #[error("{0} is not a positive size")]
    NotPositive(f32),
}

/// The look each tool draws in, as the property strip last set it. It belongs to the editor,
/// not to any document, so undo leaves it alone.
#[derive(Clone, Debug, Default)]
pub struct Looks(HashMap<Tool, Look>);

impl Looks {
    /// How `tool` draws its next shape: its default until the strip changes it.
    #[must_use]
    pub fn look(&self, tool: Tool) -> Look {
        self.0
            .get(&tool)
            .copied()
            .unwrap_or_else(|| Look::default_for(tool))
    }

    /// Sets `property` on `selected` if a shape is selected, otherwise on how `tool` draws its
    /// next shape. The returned command restyles the selection through [`Look::dress`], the
    /// rule new shapes are drawn by, so it is one undo step (F-38); `None` if nothing on the
    /// document changes. The selected shape's tool draws its next shape the same way.
    ///
    /// # Errors
    ///
    /// [`PropertyError`] if `property`'s value is refused; then nothing changes.
    pub fn set(
        &mut self,
        property: Property,
        tool: Tool,
        selected: Option<&ShapeEntry>,
    ) -> Result<Option<Command>, PropertyError> {
        let tool = selected.map_or(tool, |entry| Tool::drawing(&entry.shape));
        let next = self.look(tool).with(property)?;
        let restyled = match selected {
            Some(entry) => {
                let (shape, style) = Look::of(entry, next)
                    .with(property)?
                    .dress(entry.shape.clone());
                (shape != entry.shape || style != entry.style).then_some(Command::Replace {
                    id: entry.id,
                    shape,
                    style,
                })
            }
            None => None,
        };
        self.0.insert(tool, next);
        Ok(restyled)
    }

    /// What the strip shows: the selected shape's tool and its own look, or with nothing
    /// selected, `tool` and how it draws next.
    #[must_use]
    pub fn shown(&self, tool: Tool, selected: Option<&ShapeEntry>) -> (Tool, Look) {
        match selected {
            Some(entry) => {
                let tool = Tool::drawing(&entry.shape);
                (tool, Look::of(entry, self.look(tool)))
            }
            None => (tool, self.look(tool)),
        }
    }

    /// The look an open text box shows: its tool's for a new box, or the shape's own.
    #[must_use]
    pub fn text_look(&self, origin: &TextOrigin) -> Look {
        match origin {
            TextOrigin::New { frame, .. } => self.look(frame.tool()),
            TextOrigin::Existing {
                id,
                block,
                style,
                frame,
            } => {
                self.shown(frame.tool(), Some(&text_entry(*id, block, *style, *frame)))
                    .1
            }
        }
    }

    /// Sets `property` while a text box is open, as on a selection: a new box takes its tool's
    /// look as it now is, and an existing text or bubble is restyled at once by the returned
    /// command, one undo step (F-38), which `origin` then follows.
    ///
    /// # Errors
    ///
    /// [`PropertyError`] if `property`'s value is refused; then nothing changes.
    pub fn restyle_text(
        &mut self,
        property: Property,
        origin: &mut TextOrigin,
    ) -> Result<Option<Command>, PropertyError> {
        match origin {
            TextOrigin::New {
                size, style, frame, ..
            } => {
                self.set(property, frame.tool(), None)?;
                let look = self.look(frame.tool());
                (*size, *style) = (look.text_size, look.style);
                Ok(None)
            }
            TextOrigin::Existing {
                id,
                block,
                style,
                frame,
            } => {
                let entry = text_entry(*id, block, *style, *frame);
                let restyled = self.set(property, frame.tool(), Some(&entry))?;
                let look = Look::of(&entry, self.look(frame.tool())).with(property)?;
                (*block, *style) = (look.dress_text(block), look.style);
                Ok(restyled)
            }
        }
    }
}

fn text_entry(id: ShapeId, block: &TextBlock, style: Style, frame: TextFrame) -> ShapeEntry {
    ShapeEntry {
        id,
        shape: frame.around(block.clone()),
        style,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::document::{Document, PointF, RectF, RgbaImage, ShapeId};
    use crate::drag::{Drag, Start};
    use crate::text::TextBlock;
    use crate::text_edit::TextOrigin;

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
            text_size,
            ..Look::default_for(tool)
        }
    }

    #[test]
    fn f31_arrows_and_lines_default_to_red_3_px() {
        let style = Look::default_for(Tool::Arrow).style();
        assert_eq!((style.color.r, style.color.g, style.color.b), (255, 59, 48));
        assert_eq!(style.stroke_width, 3.0);
        assert_eq!(Look::default_for(Tool::Line).style(), style);
    }

    #[test]
    fn f35_the_highlighter_defaults_to_yellow() {
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
        };
        let blurred = Shape::Redact {
            rect: AREA,
            mode: RedactMode::Blur,
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

    const BLUE: Color = Color {
        r: 0,
        g: 122,
        b: 255,
        a: 255,
    };

    fn entry(shape: Shape, style: Style) -> ShapeEntry {
        ShapeEntry {
            id: ShapeId(7),
            shape,
            style,
        }
    }

    /// What `tool` adds for a drag from (0, 0) to (40, 30), drawn in `looks`.
    fn drawn(looks: &Looks, tool: Tool) -> Option<Command> {
        let image = RgbaImage::new(64, 64, vec![0; 64 * 64 * 4]).unwrap();
        let document = Document::new(Arc::new(image));
        let at = PointF { x: 0.0, y: 0.0 };
        let Some(Start::Drag(mut drag)) =
            Drag::press(&document, tool, looks.look(tool), None, at, 4.0).start
        else {
            panic!("{tool:?} draws by dragging");
        };
        drag.extend(PointF { x: 40.0, y: 30.0 });
        drag.command(false)
    }

    #[test]
    fn f32_f36_with_nothing_selected_the_strip_sets_how_the_tool_draws_its_next_shape() {
        let mut looks = Looks::default();
        let set = |looks: &mut Looks, property, tool| looks.set(property, tool, None).unwrap();
        assert_eq!(
            set(&mut looks, Property::Fill(Fill::Solid), Tool::Rectangle),
            None
        );
        assert_eq!(
            set(&mut looks, Property::Color(BLUE), Tool::Rectangle),
            None
        );
        assert_eq!(
            set(&mut looks, Property::Redact(RedactMode::Blur), Tool::Redact),
            None
        );
        let Some(Command::Add { shape, style }) = drawn(&looks, Tool::Rectangle) else {
            panic!("a rectangle");
        };
        assert!(matches!(
            shape,
            Shape::Rect {
                fill: Fill::Solid,
                ..
            }
        ));
        assert_eq!(style.color, BLUE);
        let Some(Command::Add { shape, .. }) = drawn(&looks, Tool::Redact) else {
            panic!("a redaction");
        };
        assert!(matches!(
            shape,
            Shape::Redact {
                mode: RedactMode::Blur,
                ..
            }
        ));
        let Some(Command::Add { shape, style }) = drawn(&looks, Tool::Ellipse) else {
            panic!("an ellipse");
        };
        assert!(matches!(
            shape,
            Shape::Ellipse {
                fill: Fill::Hollow,
                ..
            }
        ));
        assert_eq!(style, Look::default_for(Tool::Ellipse).style());
        set(&mut looks, Property::TextSize(36.0), Tool::Text);
        set(&mut looks, Property::Color(BLUE), Tool::Text);
        let image = RgbaImage::new(8, 8, vec![0; 8 * 8 * 4]).unwrap();
        let document = Document::new(Arc::new(image));
        let at = PointF { x: 2.0, y: 2.0 };
        let typed = Drag::press(&document, Tool::Text, looks.look(Tool::Text), None, at, 4.0);
        let Some(Start::Type(TextOrigin::New { size, style, .. })) = typed.start else {
            panic!("a new text box");
        };
        assert_eq!((size, style.color), (36.0, BLUE));
    }

    #[test]
    fn f38_a_strip_change_restyles_the_selection_in_one_replace_keeping_its_other_settings() {
        let mut looks = Looks::default();
        let style = Look::default_for(Tool::Rectangle).style();
        let solid = entry(
            Shape::Rect {
                rect: AREA,
                fill: Fill::Solid,
            },
            style,
        );
        let blue = looks
            .set(Property::Color(BLUE), Tool::Select, Some(&solid))
            .unwrap();
        let restyled = Command::Replace {
            id: solid.id,
            shape: solid.shape.clone(),
            style: Style {
                color: BLUE,
                ..style
            },
        };
        assert_eq!(blue, Some(restyled));
        assert_eq!(looks.look(Tool::Rectangle).style().color, BLUE);
        assert_eq!(looks.look(Tool::Select), Look::default_for(Tool::Select));
        let same = looks.set(Property::Fill(Fill::Solid), Tool::Select, Some(&solid));
        assert_eq!(same, Ok(None));
        let text = TextBlock::new(PointF { x: 1.0, y: 1.0 }, "Hi".into(), 24.0, 1.5).unwrap();
        let label = entry(Shape::Text(text.clone()), style);
        let larger = looks.set(Property::TextSize(36.0), Tool::Select, Some(&label));
        let Ok(Some(Command::Replace {
            shape: Shape::Text(block),
            ..
        })) = larger
        else {
            panic!("a larger text, not {larger:?}");
        };
        assert_eq!(
            (block.size(), block.text(), block.at()),
            (36.0, "Hi", text.at())
        );
    }

    #[test]
    fn a_size_that_is_not_positive_is_refused_and_changes_nothing() {
        let mut looks = Looks::default();
        for size in [0.0, -3.0, f32::NAN] {
            for property in [Property::StrokeWidth(size), Property::TextSize(size)] {
                assert!(
                    looks.set(property, Tool::Pen, None).is_err(),
                    "{property:?}"
                );
            }
        }
        assert_eq!(looks.look(Tool::Pen), Look::default_for(Tool::Pen));
    }

    #[test]
    fn the_strip_shows_only_the_settings_what_is_drawn_has() {
        assert_eq!(Setting::for_tool(Tool::Select), &[]);
        assert_eq!(Setting::for_tool(Tool::Crop), &[]);
        assert!(Setting::for_tool(Tool::Rectangle).contains(&Setting::Fill));
        assert!(!Setting::for_tool(Tool::Arrow).contains(&Setting::Fill));
        assert_eq!(Setting::for_tool(Tool::Redact), &[Setting::Redact]);
        assert!(Setting::for_tool(Tool::Text).contains(&Setting::TextSize));
        let style = Look::default_for(Tool::Rectangle).style();
        let solid = entry(
            Shape::Rect {
                rect: AREA,
                fill: Fill::Solid,
            },
            style,
        );
        let (tool, look) = Looks::default().shown(Tool::Select, Some(&solid));
        assert_eq!((tool, look.fill()), (Tool::Rectangle, Fill::Solid));
    }

    #[test]
    fn f34_a_strip_change_while_typing_restyles_the_open_text_box() {
        let mut looks = Looks::default();
        let at = PointF { x: 1.0, y: 1.0 };
        let mut new = TextOrigin::New {
            at,
            size: DEFAULT_TEXT_SIZE,
            style: Look::default_for(Tool::Text).style(),
            frame: TextFrame::Plain,
        };
        let added = looks.restyle_text(Property::TextSize(40.0), &mut new);
        assert_eq!(added, Ok(None));
        assert!(matches!(new, TextOrigin::New { size: 40.0, .. }));
        assert_eq!(looks.look(Tool::Text).text_size(), 40.0);
        let style = Look::default_for(Tool::Text).style();
        let block = TextBlock::new(at, "Hi".into(), 24.0, 1.5).unwrap();
        let mut existing = TextOrigin::Existing {
            id: ShapeId(7),
            block: block.clone(),
            style,
            frame: TextFrame::Plain,
        };
        let restyled = looks.restyle_text(Property::Color(BLUE), &mut existing);
        assert!(
            matches!(restyled, Ok(Some(Command::Replace { .. }))),
            "{restyled:?}"
        );
        let TextOrigin::Existing {
            block: kept,
            style: now,
            ..
        } = &existing
        else {
            panic!("still the existing text");
        };
        assert_eq!((kept.size(), now.color), (24.0, BLUE));
        assert_eq!(looks.text_look(&existing).style().color, BLUE);
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
