use crate::document::{Color, Fill, PointF, RedactMode, Shape, Style};
use crate::drawing::{smoothed_pen, snapped_to_45_degrees, squared};
use crate::history::Command;
use crate::selection::box_between;

/// What a press or drag on the canvas does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

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

impl Tool {
    /// The tool a single key picks, as in the CHARTER feature table: V, A, L, R, O, P, H, X, C, T.
    /// Case is ignored.
    #[must_use]
    pub fn for_key(key: char) -> Option<Self> {
        let tool = match key.to_ascii_lowercase() {
            'v' => Self::Select,
            'a' => Self::Arrow,
            'l' => Self::Line,
            'r' => Self::Rectangle,
            'o' => Self::Ellipse,
            'p' => Self::Pen,
            'h' => Self::Highlighter,
            'x' => Self::Redact,
            'c' => Self::Crop,
            't' => Self::Text,
            _ => return None,
        };
        Some(tool)
    }

    /// The style a new shape from this tool gets (F-31, F-35, D-42).
    #[must_use]
    pub fn style(self) -> Style {
        match self {
            Self::Highlighter => Style {
                color: HIGHLIGHTER_YELLOW,
                stroke_width: 0.0,
            },
            Self::Select
            | Self::Arrow
            | Self::Line
            | Self::Rectangle
            | Self::Ellipse
            | Self::Pen
            | Self::Redact
            | Self::Crop
            | Self::Text => Style {
                color: ANNOTATION_RED,
                stroke_width: 3.0,
            },
        }
    }

    /// The edit a finished drag through `path` makes, `constrained` while Shift is held.
    /// `None` for the select and text tools, which do not draw by dragging, and for a drag too short
    /// to draw anything, such as a click.
    #[must_use]
    pub fn command(self, path: &[PointF], constrained: bool) -> Option<Command> {
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
        let add = |shape| Command::Add {
            shape,
            style: self.style(),
        };
        match self {
            Self::Select | Self::Text => None,
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
            Self::Highlighter => has_area.then(|| add(Shape::Highlight(area))),
            Self::Redact => has_area.then(|| {
                add(Shape::Redact {
                    rect: area,
                    mode: RedactMode::Pixelate,
                })
            }),
            Self::Crop => has_area.then_some(Command::SetCrop(Some(area))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::RectF;

    fn point(x: f32, y: f32) -> PointF {
        PointF { x, y }
    }

    fn added(tool: Tool, path: &[PointF], constrained: bool) -> Shape {
        match tool.command(path, constrained) {
            Some(Command::Add { shape, .. }) => shape,
            other => panic!("{tool:?} made {other:?}"),
        }
    }

    #[test]
    fn f30_each_tool_has_its_key_from_the_charter_table() {
        let tools: Vec<_> = "VALROPHXCT".chars().map(Tool::for_key).collect();
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
            ]
            .map(Some)
        );
        assert_eq!(Tool::for_key('r'), Some(Tool::Rectangle));
        assert_eq!(Tool::for_key('t'), Some(Tool::Text));
        assert_eq!(Tool::for_key('q'), None);
    }

    #[test]
    fn f31_arrows_and_lines_default_to_red_3_px() {
        let style = Tool::Arrow.style();
        assert_eq!((style.color.r, style.color.g, style.color.b), (255, 59, 48));
        assert_eq!(style.stroke_width, 3.0);
        assert_eq!(Tool::Line.style(), style);
    }

    #[test]
    fn f31_shift_snaps_a_new_arrow_to_45_degrees() {
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
    fn f32_a_new_rectangle_is_hollow_and_shift_makes_it_square() {
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
    fn f33_a_pen_drag_stores_the_smoothed_path() {
        let path = [point(0.0, 0.0), point(16.0, 0.0), point(16.0, 16.0)];
        assert_eq!(
            added(Tool::Pen, &path, false),
            Shape::Pen(smoothed_pen(&path))
        );
    }

    #[test]
    fn f35_f36_f37_highlight_redact_and_crop_cover_the_dragged_box() {
        let path = [point(5.0, 5.0), point(25.0, 15.0)];
        let area = RectF {
            x: 5.0,
            y: 5.0,
            width: 20.0,
            height: 10.0,
        };
        assert_eq!(
            added(Tool::Highlighter, &path, false),
            Shape::Highlight(area)
        );
        assert_eq!(
            added(Tool::Redact, &path, false),
            Shape::Redact {
                rect: area,
                mode: RedactMode::Pixelate
            }
        );
        assert_eq!(
            Tool::Crop.command(&path, false),
            Some(Command::SetCrop(Some(area)))
        );
    }

    #[test]
    fn f30_a_click_or_the_select_tool_makes_no_edit() {
        let click = [point(7.0, 7.0), point(7.0, 7.0)];
        let flat = [point(0.0, 5.0), point(40.0, 5.0)];
        for tool in [Tool::Arrow, Tool::Rectangle, Tool::Pen, Tool::Crop] {
            assert_eq!(tool.command(&click, false), None, "{tool:?}");
        }
        assert_eq!(Tool::Redact.command(&flat, false), None);
        assert_eq!(Tool::Select.command(&flat, false), None);
        assert_eq!(Tool::Line.command(&[], false), None);
    }
}
