use crate::document::{Fill, HighlightStrength, PointF, RedactMode, RedactStrength, Shape};
use crate::drawing::{smoothed_pen, snapped_to_45_degrees, squared};
use crate::history::Command;
use crate::look::Look;
use crate::selection::box_between;
use crate::step::Step;

/// What a press or drag on the canvas does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Select,
    Arrow,
    Line,
    Rectangle,
    Ellipse,
    Pen,
    Highlighter,
    Redact,
    Crop,
    Text,
    Spotlight,
    Step,
    Bubble,
}

impl Tool {
    /// Every tool.
    pub(crate) const ALL: [Self; 13] = [
        Self::Select,
        Self::Arrow,
        Self::Line,
        Self::Rectangle,
        Self::Ellipse,
        Self::Pen,
        Self::Highlighter,
        Self::Redact,
        Self::Crop,
        Self::Text,
        Self::Spotlight,
        Self::Step,
        Self::Bubble,
    ];

    /// The tool that draws shapes like `shape`.
    #[must_use]
    pub fn drawing(shape: &Shape) -> Self {
        match shape {
            Shape::Arrow { .. } => Self::Arrow,
            Shape::Line { .. } => Self::Line,
            Shape::Rect { .. } => Self::Rectangle,
            Shape::Ellipse { .. } => Self::Ellipse,
            Shape::Pen(_) => Self::Pen,
            Shape::Text(_) => Self::Text,
            Shape::Highlight { .. } => Self::Highlighter,
            Shape::Redact { .. } => Self::Redact,
            Shape::Spotlight(_) => Self::Spotlight,
            Shape::Step(_) => Self::Step,
            Shape::Bubble(_) => Self::Bubble,
        }
    }

    /// The key that picks this tool, as its button shows it.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Select => "V",
            Self::Arrow => "A",
            Self::Line => "L",
            Self::Rectangle => "R",
            Self::Ellipse => "O",
            Self::Pen => "P",
            Self::Highlighter => "H",
            Self::Redact => "X",
            Self::Crop => "C",
            Self::Text => "T",
            Self::Spotlight => "S",
            Self::Step => "N",
            Self::Bubble => "B",
        }
    }

    /// The tool a single key picks, case ignored.
    #[must_use]
    pub fn for_key(key: char) -> Option<Self> {
        let key = key.to_ascii_uppercase();
        Self::ALL
            .into_iter()
            .find(|tool| tool.key().starts_with(key))
    }

    /// The edit a finished drag through `path` makes, drawn in `look`, `constrained` while Shift
    /// is held.
    /// `None` for the select, text and bubble tools, which do not draw by dragging, and for a drag
    /// too short to draw anything, such as a click.
    #[must_use]
    pub fn command(self, path: &[PointF], constrained: bool, look: &Look) -> Option<Command> {
        let (&from, &last) = (path.first()?, path.last()?);
        let to = if !constrained {
            last
        } else if matches!(self, Self::Arrow | Self::Line) {
            snapped_to_45_degrees(from, last)
        } else {
            squared(from, last)
        };
        let area = box_between(from, to);
        let has_area = area.width > 0.0 && area.height > 0.0;
        let add = |shape| {
            let (shape, style) = look.dress(shape);
            Command::Add { shape, style }
        };
        match self {
            Self::Select | Self::Text | Self::Bubble => None,
            Self::Arrow => (from != to).then(|| add(Shape::Arrow { from, to })),
            Self::Line => (from != to).then(|| add(Shape::Line { from, to })),
            Self::Pen => {
                let stroke = smoothed_pen(path);
                (stroke.len() > 1).then(|| add(Shape::Pen(stroke)))
            }
            Self::Rectangle => has_area.then(|| {
                add(Shape::Rect {
                    rect: area,
                    fill: Fill::Hollow,
                })
            }),
            Self::Ellipse => has_area.then(|| {
                add(Shape::Ellipse {
                    rect: area,
                    fill: Fill::Hollow,
                })
            }),
            Self::Highlighter => has_area.then(|| {
                add(Shape::Highlight {
                    rect: area,
                    strength: HighlightStrength::Strong,
                })
            }),
            Self::Redact => has_area.then(|| {
                add(Shape::Redact {
                    rect: area,
                    mode: RedactMode::Pixelate,
                    strength: RedactStrength::FINE,
                })
            }),
            Self::Spotlight => has_area.then(|| add(Shape::Spotlight(area))),
            Self::Step => Some(add(Shape::Step(Step::new(last, look.text_size())))),
            Self::Crop => has_area.then_some(Command::SetCrop(Some(area))),
        }
    }
}

/// One button of the editor's toolbar: a tool, or several behind a menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolGroup {
    Select,
    Shapes,
    Text,
    Attention,
    Redact,
    Crop,
}

impl ToolGroup {
    /// Every group, in toolbar order.
    pub const ALL: [Self; 6] = [
        Self::Select,
        Self::Shapes,
        Self::Text,
        Self::Attention,
        Self::Redact,
        Self::Crop,
    ];

    /// The group's tools, in its menu's order.
    #[must_use]
    pub fn tools(self) -> &'static [Tool] {
        match self {
            Self::Select => &[Tool::Select],
            Self::Shapes => &[
                Tool::Arrow,
                Tool::Line,
                Tool::Rectangle,
                Tool::Ellipse,
                Tool::Pen,
            ],
            Self::Text => &[Tool::Text, Tool::Bubble, Tool::Step],
            Self::Attention => &[Tool::Highlighter, Tool::Spotlight],
            Self::Redact => &[Tool::Redact],
            Self::Crop => &[Tool::Crop],
        }
    }

    /// The group holding `tool`.
    #[must_use]
    pub fn of(tool: Tool) -> Self {
        match tool {
            Tool::Select => Self::Select,
            Tool::Arrow | Tool::Line | Tool::Rectangle | Tool::Ellipse | Tool::Pen => Self::Shapes,
            Tool::Text | Tool::Bubble | Tool::Step => Self::Text,
            Tool::Highlighter | Tool::Spotlight => Self::Attention,
            Tool::Redact => Self::Redact,
            Tool::Crop => Self::Crop,
        }
    }
}

/// The tool each group's button shows: the one last picked from the group, by its button's
/// menu or its key, or the group's first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShownTools([Tool; 6]);

impl Default for ShownTools {
    fn default() -> Self {
        Self(ToolGroup::ALL.map(|group| group.tools()[0]))
    }
}

impl ShownTools {
    /// The tool `group`'s button shows.
    #[must_use]
    pub fn shown(self, group: ToolGroup) -> Tool {
        self.0[group as usize]
    }

    /// Shows `tool` on its group's button.
    pub fn pick(&mut self, tool: Tool) {
        self.0[ToolGroup::of(tool) as usize] = tool;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{RectF, point};

    #[test]
    fn every_tool_is_in_the_one_group_that_lists_it() {
        let listed: Vec<Tool> = ToolGroup::ALL
            .iter()
            .flat_map(|&group| group.tools().iter().copied())
            .collect();
        assert_eq!(listed.len(), Tool::ALL.len());
        for tool in Tool::ALL {
            assert!(
                ToolGroup::of(tool).tools().contains(&tool),
                "{tool:?} is not listed by its own group"
            );
        }
    }

    #[test]
    fn a_group_shows_its_first_tool_until_another_of_its_tools_is_picked() {
        let mut shown = ShownTools::default();
        assert_eq!(shown.shown(ToolGroup::Shapes), Tool::Arrow);
        assert_eq!(shown.shown(ToolGroup::Text), Tool::Text);
        shown.pick(Tool::for_key('r').unwrap());
        shown.pick(Tool::Spotlight);
        assert_eq!(shown.shown(ToolGroup::Shapes), Tool::Rectangle);
        assert_eq!(shown.shown(ToolGroup::Attention), Tool::Spotlight);
        assert_eq!(
            shown.shown(ToolGroup::Text),
            Tool::Text,
            "other groups keep theirs"
        );
        shown.pick(Tool::Select);
        assert_eq!(shown.shown(ToolGroup::Shapes), Tool::Rectangle);
    }

    fn added(tool: Tool, path: &[PointF], constrained: bool) -> Shape {
        match tool.command(path, constrained, &Look::default_for(tool)) {
            Some(Command::Add { shape, .. }) => shape,
            other => panic!("{tool:?} made {other:?}"),
        }
    }

    #[test]
    fn each_tool_has_its_key() {
        let tools: Vec<_> = "VALROPHXCTSNB".chars().map(Tool::for_key).collect();
        assert_eq!(
            tools,
            [
                Tool::Select,
                Tool::Arrow,
                Tool::Line,
                Tool::Rectangle,
                Tool::Ellipse,
                Tool::Pen,
                Tool::Highlighter,
                Tool::Redact,
                Tool::Crop,
                Tool::Text,
                Tool::Spotlight,
                Tool::Step,
                Tool::Bubble,
            ]
            .map(Some)
        );
        assert_eq!(Tool::for_key('r'), Some(Tool::Rectangle));
        assert_eq!(Tool::for_key('t'), Some(Tool::Text));
        assert_eq!(Tool::for_key('q'), None);
    }

    #[test]
    fn shift_snaps_a_new_arrow_to_45_degrees() {
        let path = [point(0.0, 0.0), point(100.0, 10.0)];
        assert_eq!(
            added(Tool::Arrow, &path, true),
            Shape::Arrow {
                from: point(0.0, 0.0),
                to: point(100.0, 0.0)
            }
        );
    }

    #[test]
    fn a_new_rectangle_is_hollow_and_shift_makes_it_square() {
        let path = [point(50.0, 50.0), point(10.0, 30.0)];
        assert_eq!(
            added(Tool::Rectangle, &path, false),
            Shape::Rect {
                rect: RectF {
                    x: 10.0,
                    y: 30.0,
                    width: 40.0,
                    height: 20.0
                },
                fill: Fill::Hollow
            }
        );
        let Shape::Ellipse { rect, .. } = added(Tool::Ellipse, &path, true) else {
            panic!("not an ellipse");
        };
        assert_eq!((rect.width, rect.height), (40.0, 40.0));
    }

    #[test]
    fn a_pen_drag_stores_the_smoothed_path() {
        let path = [point(0.0, 0.0), point(16.0, 0.0), point(16.0, 16.0)];
        assert_eq!(
            added(Tool::Pen, &path, false),
            Shape::Pen(smoothed_pen(&path))
        );
    }

    #[test]
    fn highlight_redact_crop_and_spotlight_cover_the_dragged_box() {
        let path = [point(5.0, 5.0), point(25.0, 15.0)];
        let area = RectF {
            x: 5.0,
            y: 5.0,
            width: 20.0,
            height: 10.0,
        };
        assert_eq!(
            added(Tool::Highlighter, &path, false),
            Shape::Highlight {
                rect: area,
                strength: HighlightStrength::Strong
            }
        );
        assert_eq!(
            added(Tool::Redact, &path, false),
            Shape::Redact {
                rect: area,
                mode: RedactMode::Pixelate,
                strength: RedactStrength::FINE,
            }
        );
        assert_eq!(
            Tool::Crop.command(&path, false, &Look::default_for(Tool::Crop)),
            Some(Command::SetCrop(Some(area)))
        );
        assert_eq!(added(Tool::Spotlight, &path, false), Shape::Spotlight(area));
    }

    #[test]
    fn a_click_or_the_select_tool_makes_no_edit() {
        let click = [point(7.0, 7.0), point(7.0, 7.0)];
        let flat = [point(0.0, 5.0), point(40.0, 5.0)];
        for tool in [Tool::Arrow, Tool::Rectangle, Tool::Pen, Tool::Crop] {
            assert_eq!(
                tool.command(&click, false, &Look::default_for(tool)),
                None,
                "{tool:?}"
            );
        }
        assert_eq!(
            Tool::Redact.command(&flat, false, &Look::default_for(Tool::Redact)),
            None
        );
        assert_eq!(
            Tool::Select.command(&flat, false, &Look::default_for(Tool::Select)),
            None
        );
        assert_eq!(
            Tool::Line.command(&[], false, &Look::default_for(Tool::Line)),
            None
        );
    }
}
