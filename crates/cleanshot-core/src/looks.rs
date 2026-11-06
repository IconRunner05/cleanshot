use std::collections::HashMap;

use crate::document::{Color, ShapeEntry, ShapeId, Style};
use crate::history::Command;
use crate::look::{Look, Property, PropertyError};
use crate::text::TextBlock;
use crate::text_edit::{TextFrame, TextOrigin};
use crate::tool::Tool;

/// The look each tool draws in, as the property strip last set it, with one colour for every
/// tool once one is picked (D-112). It belongs to the editor, not to any document, so undo
/// leaves it alone.
#[derive(Clone, Debug, Default)]
pub struct Looks {
    by_tool: HashMap<Tool, Look>,
    color: Option<Color>,
}

impl Looks {
    /// How `tool` draws its next shape: its default until the strip changes it, in the colour
    /// last picked for any tool.
    #[must_use]
    pub fn look(&self, tool: Tool) -> Look {
        let look = self
            .by_tool
            .get(&tool)
            .copied()
            .unwrap_or_else(|| Look::default_for(tool));
        match self.color {
            Some(color) => Self::colored(look, color),
            None => look,
        }
    }

    fn colored(look: Look, color: Color) -> Look {
        look.with_color(color)
    }

    /// The colour last picked for any tool, if one was (D-112).
    pub(crate) fn picked_color(&self) -> Option<Color> {
        self.color
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
        if let Property::Color(color) = property {
            self.color = Some(color);
        }
        self.by_tool.insert(tool, next);
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
                size,
                style,
                frame,
                alignment,
                ..
            } => {
                self.set(property, frame.tool(), None)?;
                let look = self.look(frame.tool());
                (*size, *style, *alignment) = (
                    look.text_size(),
                    look.style_for(frame.tool()),
                    look.alignment(),
                );
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
                (*block, *style) = (look.dress_text(block), look.style_for(frame.tool()));
                Ok(restyled)
            }
        }
    }
}

// F-59: equal when every tool draws alike, however each look came to be set.
impl PartialEq for Looks {
    fn eq(&self, other: &Self) -> bool {
        self.color == other.color
            && Tool::ALL
                .iter()
                .all(|&tool| self.look(tool) == other.look(tool))
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
    use crate::document::{
        Border, BorderWidth, Document, Fill, HighlightStrength, PointF, RectF, RedactMode,
        RedactStrength, RgbaImage, Shape,
    };
    use crate::drag::{Drag, Start};
    use crate::look::{HIGHLIGHTER_YELLOW, WHITE, takes_border, takes_shadow};
    use crate::strip::Setting;
    use crate::text::{Alignment, DEFAULT_TEXT_SIZE, Horizontal, Vertical};

    const AREA: RectF = RectF {
        x: 1.0,
        y: 2.0,
        width: 30.0,
        height: 20.0,
    };

    #[test]
    fn d123_a_drawn_redaction_takes_a_new_strength_and_keeps_its_mode() {
        let mut looks = Looks::default();
        let blurred = entry(
            Shape::Redact {
                rect: AREA,
                mode: RedactMode::Blur,
                strength: RedactStrength::FINE,
            },
            Look::default_for(Tool::Redact).style(),
        );
        let coarse = Property::RedactStrength(RedactStrength::COARSE);
        let Ok(Some(Command::Replace { shape, .. })) =
            looks.set(coarse, Tool::Select, Some(&blurred))
        else {
            panic!("the redaction is restyled");
        };
        assert_eq!(
            shape,
            Shape::Redact {
                rect: AREA,
                mode: RedactMode::Blur,
                strength: RedactStrength::COARSE,
            }
        );
        assert!(
            looks.look(Tool::Redact).has(coarse),
            "the next redaction is as coarse"
        );
    }

    #[test]
    fn d123_a_drawn_highlight_takes_a_new_strength_and_keeps_its_colour() {
        let mut looks = Looks::default();
        let style = Look::default_for(Tool::Highlighter).style();
        let drawn = entry(
            Shape::Highlight {
                rect: AREA,
                strength: HighlightStrength::Strong,
            },
            style,
        );
        let light = Property::HighlightStrength(HighlightStrength::Light);
        let Ok(Some(Command::Replace {
            shape,
            style: restyled,
            ..
        })) = looks.set(light, Tool::Select, Some(&drawn))
        else {
            panic!("the highlight is restyled");
        };
        assert_eq!(
            shape,
            Shape::Highlight {
                rect: AREA,
                strength: HighlightStrength::Light,
            }
        );
        assert_eq!(restyled.color, style.color);
        assert!(
            looks.look(Tool::Highlighter).has(light),
            "the next highlight is as light"
        );
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
        drag.command(false, |_| 0.0)
    }

    #[test]
    fn d114_only_shapes_that_cast_a_shadow_get_one_and_the_strip_offers_it_only_there() {
        let mut looks = Looks::default();
        for tool in [Tool::Arrow, Tool::Pen, Tool::Highlighter, Tool::Rectangle] {
            looks.set(Property::Shadow(true), tool, None).unwrap();
        }
        let shadow_of = |tool| match drawn(&looks, tool) {
            Some(Command::Add { style, .. }) => style.shadow(),
            other => panic!("{tool:?} drew {other:?}"),
        };
        assert!(shadow_of(Tool::Arrow));
        assert!(shadow_of(Tool::Rectangle));
        assert!(
            !shadow_of(Tool::Line),
            "the line tool's look was left alone"
        );
        assert!(!shadow_of(Tool::Pen), "a pen stroke casts no shadow");
        assert!(!shadow_of(Tool::Highlighter), "nor does a highlight");
        for tool in TOOLS_FOR_TEST {
            let offered = Setting::for_tool(tool).contains(&Setting::Shadow);
            assert_eq!(offered, takes_shadow(tool), "{tool:?}");
        }
        let Some(Command::Add { shape, style }) = drawn(&looks, Tool::Arrow) else {
            panic!("an arrow");
        };
        let arrow = entry(shape, style);
        let (_, shown) = looks.shown(Tool::Select, Some(&arrow));
        assert!(shown.has(Property::Shadow(true)));
        let Ok(Some(Command::Replace { style, .. })) =
            looks.set(Property::Shadow(false), Tool::Select, Some(&arrow))
        else {
            panic!("turning the shadow off restyles the arrow");
        };
        assert!(!style.shadow());
    }

    const TOOLS_FOR_TEST: [Tool; 13] = [
        Tool::Select,
        Tool::Arrow,
        Tool::Line,
        Tool::Rectangle,
        Tool::Ellipse,
        Tool::Pen,
        Tool::Highlighter,
        Tool::Text,
        Tool::Step,
        Tool::Bubble,
        Tool::Redact,
        Tool::Spotlight,
        Tool::Crop,
    ];

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
        assert_eq!(
            style,
            Style::new(BLUE, Look::default_for(Tool::Ellipse).style().stroke_width)
        );
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
    fn d131_a_border_goes_where_a_shadow_can_in_its_own_colour_and_width_one_step_per_change() {
        let mut looks = Looks::default();
        for property in [
            Property::Border(true),
            Property::BorderColor(BLUE),
            Property::BorderWidth(BorderWidth::Thick),
        ] {
            for tool in TOOLS_FOR_TEST {
                looks.set(property, tool, None).unwrap();
            }
        }
        for tool in TOOLS_FOR_TEST {
            let border = looks.look(tool).style_for(tool).border();
            let expected = takes_border(tool).then_some(Border {
                color: BLUE,
                width: BorderWidth::Thick,
            });
            assert_eq!(border, expected, "{tool:?}");
        }
        let style = Look::default_for(Tool::Rectangle).style();
        let plain = entry(
            Shape::Rect {
                rect: AREA,
                fill: Fill::Hollow,
            },
            style,
        );
        let mut fresh = Looks::default();
        let bordered = fresh.set(Property::Border(true), Tool::Select, Some(&plain));
        let Ok(Some(Command::Replace { style: given, .. })) = bordered else {
            panic!("one replace of the rectangle, not {bordered:?}");
        };
        let white_thin = Border {
            color: WHITE,
            width: BorderWidth::Thin,
        };
        assert_eq!(given.border(), Some(white_thin));
        let with_border = entry(plain.shape.clone(), given);
        let (_, shown) = fresh.shown(Tool::Select, Some(&with_border));
        assert!(shown.border() && shown.has(Property::BorderColor(WHITE)));
        let thick = fresh.set(
            Property::BorderWidth(BorderWidth::Thick),
            Tool::Select,
            Some(&with_border),
        );
        let Ok(Some(Command::Replace { style: thicker, .. })) = thick else {
            panic!("one replace, not {thick:?}");
        };
        assert_eq!(
            thicker.border().map(|border| (border.color, border.width)),
            Some((WHITE, BorderWidth::Thick)),
            "the colour is kept"
        );
        assert_eq!(thicker.color, given.color, "the shape's own colour is kept");
    }

    #[test]
    fn d114_a_new_text_or_bubble_casts_the_shadow_its_tool_is_set_to() {
        let mut looks = Looks::default();
        let image = RgbaImage::new(8, 8, vec![0; 8 * 8 * 4]).unwrap();
        let document = Document::new(Arc::new(image));
        let at = PointF { x: 2.0, y: 2.0 };
        for tool in [Tool::Text, Tool::Bubble] {
            looks.set(Property::Shadow(true), tool, None).unwrap();
            let pressed = Drag::press(&document, tool, looks.look(tool), None, at, 4.0);
            let origin = match pressed.start {
                Some(Start::Type(origin)) => Some(origin),
                Some(Start::Drag(drag)) => drag.typed_on_release(tool),
                None => None,
            };
            let Some(TextOrigin::New { style, .. }) = origin else {
                panic!("{tool:?} opens a new box, not {origin:?}");
            };
            assert!(style.shadow(), "{tool:?}");
        }
        let Some(Start::Drag(mut dragged)) = Drag::press(
            &document,
            Tool::Bubble,
            looks.look(Tool::Bubble),
            None,
            at,
            4.0,
        )
        .start
        else {
            panic!("a bubble-tool press on bare image starts a drag");
        };
        dragged.extend(PointF { x: 7.0, y: 7.0 });
        let preview = dragged.preview(false, |_| 0.0).unwrap();
        assert!(
            matches!(preview, Some(Command::Add { style, .. }) if style.shadow()),
            "a bubble shows its shadow while dragged out, not {preview:?}"
        );
        let mut new = TextOrigin::New {
            at,
            size: DEFAULT_TEXT_SIZE,
            style: Look::default_for(Tool::Text).style(),
            frame: TextFrame::Plain,
            box_size: None,
            alignment: Alignment::default(),
        };
        looks
            .restyle_text(Property::Shadow(false), &mut new)
            .unwrap();
        looks
            .restyle_text(Property::Shadow(true), &mut new)
            .unwrap();
        assert!(matches!(new, TextOrigin::New { style, .. } if style.shadow()));
    }

    #[test]
    fn d112_a_colour_picked_for_one_tool_is_the_colour_every_tool_draws_in_next() {
        let mut looks = Looks::default();
        assert_eq!(
            looks.look(Tool::Highlighter).style().color,
            HIGHLIGHTER_YELLOW
        );
        looks
            .set(Property::StrokeWidth(8.0), Tool::Rectangle, None)
            .unwrap();
        looks.set(Property::Color(BLUE), Tool::Arrow, None).unwrap();
        for tool in [Tool::Arrow, Tool::Rectangle, Tool::Highlighter, Tool::Text] {
            assert_eq!(looks.look(tool).style().color, BLUE, "{tool:?}");
        }
        assert_eq!(looks.look(Tool::Rectangle).style().stroke_width, 8.0);
        assert_eq!(looks.look(Tool::Line).style().stroke_width, 3.0);
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
            style: Style::new(BLUE, style.stroke_width),
        };
        assert_eq!(blue, Some(restyled));
        assert_eq!(looks.look(Tool::Rectangle).style().color, BLUE);
        assert_eq!(
            looks.look(Tool::Select),
            Looks::colored(Look::default_for(Tool::Select), BLUE)
        );
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
    fn d129_text_aligns_across_a_bubble_also_down_and_a_pick_restyles_the_selection_in_one_step() {
        assert_eq!(Setting::HorizontalAlignment.choices().len(), 3);
        assert_eq!(Setting::Alignment.choices().len(), 9);
        assert!(Setting::for_tool(Tool::Text).contains(&Setting::HorizontalAlignment));
        assert!(Setting::for_tool(Tool::Bubble).contains(&Setting::Alignment));
        let centred = Alignment {
            horizontal: Horizontal::Centre,
            vertical: Vertical::Bottom,
        };
        let mut looks = Looks::default();
        let text = TextBlock::new(PointF { x: 1.0, y: 1.0 }, "Hi".into(), 24.0, 1.5).unwrap();
        let bubble = entry(
            Shape::Bubble(crate::Bubble::new(text, PointF { x: 0.0, y: 0.0 })),
            Look::default_for(Tool::Bubble).style(),
        );
        let picked = looks.set(Property::Alignment(centred), Tool::Select, Some(&bubble));
        let Ok(Some(Command::Replace {
            shape: Shape::Bubble(restyled),
            ..
        })) = picked
        else {
            panic!("one replace of the bubble, not {picked:?}");
        };
        assert_eq!(restyled.block().alignment(), centred);
        assert_eq!(looks.look(Tool::Bubble).alignment(), centred);
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
        assert_eq!(
            Setting::for_tool(Tool::Redact),
            &[Setting::Redact, Setting::RedactStrength]
        );
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
            box_size: None,
            alignment: Alignment::default(),
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
}
