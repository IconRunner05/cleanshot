use toml::{Table, Value};

use super::Reader;
use crate::document::{
    BorderWidth, Color, Fill, HighlightStrength, RedactMode, RedactStrength, SpotlightDim,
};
use crate::look::{Look, Property};
use crate::looks::Looks;
use crate::strip::Setting;
use crate::text::{Alignment, Horizontal, Vertical};
use crate::tool::Tool;

/// `looks` as the settings file's `[looks]` table: for each tool only the values that differ
/// from its default, its colour among them.
pub(super) fn table(looks: &Looks) -> Table {
    let mut table = Table::new();
    for tool in Tool::ALL {
        let (look, default) = (looks.look(tool), Look::default_for(tool));
        let mut values = Table::new();
        for &setting in Setting::for_tool(tool) {
            let current = value(setting, &look);
            if current != value(setting, &default) {
                values.insert(key(setting).into(), current);
            }
        }
        if !values.is_empty() {
            table.insert(tool_name(tool).into(), values.into());
        }
    }
    table
}

impl Reader<'_> {
    /// The `[looks]` table: each value read on its own, so one that is unusable leaves its
    /// tool's default for that value alone, and is reported. A `color` for every tool,
    /// from a file written before each tool had its own, is the colour of each tool whose table
    /// names none.
    pub(super) fn looks(&mut self) -> Looks {
        let mut looks = Looks::default();
        let Some(table) = self.read("looks", |value| value.as_table().cloned()) else {
            return looks;
        };
        let mut reader = Reader {
            table: &table,
            prefix: "looks".into(),
            problems: Vec::new(),
        };
        let every_tool = reader.read("color", |value| value.as_str().and_then(Color::from_hex));
        for tool in Tool::ALL {
            let settings = Setting::for_tool(tool);
            if let Some(color) = every_tool.filter(|_| settings.contains(&Setting::Color)) {
                set(&mut looks, Property::Color(color), tool);
            }
            if settings.is_empty() {
                continue;
            }
            let name = tool_name(tool);
            let Some(values) = reader.read(name, |value| value.as_table().cloned()) else {
                continue;
            };
            let mut tool_reader = Reader {
                table: &values,
                prefix: format!("looks.{name}"),
                problems: Vec::new(),
            };
            for &setting in settings {
                let look = looks.look(tool);
                if let Some(property) =
                    tool_reader.read(key(setting), |value| property(setting, value, &look))
                {
                    set(&mut looks, property, tool);
                }
            }
            reader.problems.append(&mut tool_reader.problems);
        }
        self.problems.append(&mut reader.problems);
        looks
    }
}

fn set(looks: &mut Looks, property: Property, tool: Tool) {
    looks
        .set(property, tool, None)
        .expect("a property read from the file is one Look::with took");
}

/// The table each tool's values are under.
fn tool_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => "select",
        Tool::Arrow => "arrow",
        Tool::Line => "line",
        Tool::Rectangle => "rectangle",
        Tool::Ellipse => "ellipse",
        Tool::Pen => "pen",
        Tool::Highlighter => "highlighter",
        Tool::Redact => "redact",
        Tool::Crop => "crop",
        Tool::Text => "text",
        Tool::Spotlight => "spotlight",
        Tool::Step => "step",
        Tool::Bubble => "bubble",
    }
}

/// The key a setting's value is under in its tool's table.
fn key(setting: Setting) -> &'static str {
    match setting {
        Setting::Color => "color",
        Setting::StrokeWidth => "stroke_width",
        Setting::Fill => "fill",
        Setting::Redact => "redaction",
        Setting::RedactStrength | Setting::HighlightStrength => "strength",
        Setting::TextSize => "text_size",
        Setting::Shadow => "shadow",
        Setting::Alignment | Setting::HorizontalAlignment => "alignment",
        Setting::Border => "border",
        Setting::BorderColor => "border_color",
        Setting::BorderWidth => "border_width",
        Setting::Background => "background",
        Setting::BackgroundColor => "background_color",
        Setting::Dim => "dim",
    }
}

fn value(setting: Setting, look: &Look) -> Value {
    match setting {
        Setting::Color => look.style().color.hex().into(),
        Setting::StrokeWidth => number(look.style().stroke_width),
        Setting::Fill => match look.fill() {
            Fill::Hollow => "outline",
            Fill::Solid => "filled",
        }
        .into(),
        Setting::Redact => match look.redact() {
            RedactMode::Pixelate => "pixelate",
            RedactMode::Blur => "blur",
        }
        .into(),
        Setting::RedactStrength => i64::from(look.redact_strength().pixels()).into(),
        Setting::HighlightStrength => match look.highlight_strength() {
            HighlightStrength::Light => "light",
            HighlightStrength::Medium => "medium",
            HighlightStrength::Strong => "strong",
        }
        .into(),
        Setting::TextSize => number(look.text_size()),
        Setting::Shadow => look.shadow().into(),
        Setting::Alignment => {
            let Alignment {
                horizontal,
                vertical,
            } = look.alignment();
            format!(
                "{}-{}",
                vertical_name(vertical),
                horizontal_name(horizontal)
            )
            .into()
        }
        Setting::HorizontalAlignment => horizontal_name(look.alignment().horizontal).into(),
        Setting::Border => look.border().into(),
        Setting::BorderColor => look.border_color().hex().into(),
        Setting::BorderWidth => i64::from(look.border_width().pixels()).into(),
        Setting::Background => look.background().into(),
        Setting::BackgroundColor => look.background_color().hex().into(),
        Setting::Dim => match look.dim() {
            SpotlightDim::Light => "light",
            SpotlightDim::Medium => "medium",
            SpotlightDim::Dark => "dark",
        }
        .into(),
    }
}

/// The property `value` sets for `setting`, if it is one the property strip offers for it.
fn property(setting: Setting, value: &Value, look: &Look) -> Option<Property> {
    let offered = |property: Property| {
        setting
            .choices()
            .iter()
            .any(|&(choice, _)| choice == property)
            .then_some(property)
    };
    let typed = |value: &Value| {
        let range = setting.typed(look)?.range();
        let number = read_number(value)?;
        (f32::from(*range.start())..=f32::from(*range.end()))
            .contains(&number)
            .then_some(number)
    };
    match setting {
        Setting::Color => Some(Property::Color(Color::from_hex(value.as_str()?)?)),
        Setting::StrokeWidth => offered(Property::StrokeWidth(read_number(value)?)),
        Setting::Fill => offered(Property::Fill(match value.as_str()? {
            "outline" => Fill::Hollow,
            "filled" => Fill::Solid,
            _ => return None,
        })),
        Setting::Redact => offered(Property::Redact(match value.as_str()? {
            "pixelate" => RedactMode::Pixelate,
            "blur" => RedactMode::Blur,
            _ => return None,
        })),
        Setting::RedactStrength => {
            let pixels = u16::try_from(value.as_integer()?).ok()?;
            typed(value)?;
            Some(Property::RedactStrength(RedactStrength::clamped(pixels)))
        }
        Setting::HighlightStrength => {
            offered(Property::HighlightStrength(match value.as_str()? {
                "light" => HighlightStrength::Light,
                "medium" => HighlightStrength::Medium,
                "strong" => HighlightStrength::Strong,
                _ => return None,
            }))
        }
        Setting::TextSize => Some(Property::TextSize(typed(value)?)),
        Setting::Shadow => Some(Property::Shadow(value.as_bool()?)),
        Setting::Alignment => {
            let (vertical, horizontal) = value.as_str()?.split_once('-')?;
            offered(Property::Alignment(Alignment {
                horizontal: horizontal_named(horizontal)?,
                vertical: vertical_named(vertical)?,
            }))
        }
        Setting::HorizontalAlignment => offered(Property::Alignment(Alignment {
            horizontal: horizontal_named(value.as_str()?)?,
            vertical: Vertical::Top,
        })),
        Setting::Border => Some(Property::Border(value.as_bool()?)),
        Setting::BorderColor => Some(Property::BorderColor(Color::from_hex(value.as_str()?)?)),
        Setting::BorderWidth => offered(Property::BorderWidth(match value.as_integer()? {
            2 => BorderWidth::Thin,
            4 => BorderWidth::Medium,
            6 => BorderWidth::Thick,
            _ => return None,
        })),
        Setting::Background => Some(Property::Background(value.as_bool()?)),
        Setting::BackgroundColor => {
            Some(Property::BackgroundColor(Color::from_hex(value.as_str()?)?))
        }
        Setting::Dim => offered(Property::Dim(match value.as_str()? {
            "light" => SpotlightDim::Light,
            "medium" => SpotlightDim::Medium,
            "dark" => SpotlightDim::Dark,
            _ => return None,
        })),
    }
}

/// A whole number as an integer, else as a float.
#[expect(
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    reason = "only a number that is exactly whole, and under a billion, is cast"
)]
fn number(number: f32) -> Value {
    let whole = number.trunc();
    if whole == number && whole.abs() < 1e9 {
        return (whole as i64).into();
    }
    f64::from(number).into()
}

/// An integer or a float, as an `f32`.
fn read_number(value: &Value) -> Option<f32> {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "a value out of a strip range is refused after the cast"
    )]
    match value {
        Value::Integer(number) => Some(*number as f32),
        Value::Float(number) => Some(*number as f32),
        Value::String(_)
        | Value::Boolean(_)
        | Value::Datetime(_)
        | Value::Array(_)
        | Value::Table(_) => None,
    }
}

fn horizontal_name(horizontal: Horizontal) -> &'static str {
    match horizontal {
        Horizontal::Left => "left",
        Horizontal::Centre => "center",
        Horizontal::Right => "right",
    }
}

fn horizontal_named(name: &str) -> Option<Horizontal> {
    [Horizontal::Left, Horizontal::Centre, Horizontal::Right]
        .into_iter()
        .find(|&horizontal| horizontal_name(horizontal) == name)
}

fn vertical_name(vertical: Vertical) -> &'static str {
    match vertical {
        Vertical::Top => "top",
        Vertical::Centre => "center",
        Vertical::Bottom => "bottom",
    }
}

fn vertical_named(name: &str) -> Option<Vertical> {
    [Vertical::Top, Vertical::Centre, Vertical::Bottom]
        .into_iter()
        .find(|&vertical| vertical_name(vertical) == name)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::settings::{Settings, SettingsProblem};

    /// Every key the `[looks]` table can hold, as a problem names it.
    pub(in crate::settings) fn keys() -> Vec<String> {
        let mut keys = vec!["looks".to_owned(), "looks.color".to_owned()];
        for tool in Tool::ALL {
            let settings = Setting::for_tool(tool);
            if settings.is_empty() {
                continue;
            }
            let name = tool_name(tool);
            keys.push(format!("looks.{name}"));
            keys.extend(
                settings
                    .iter()
                    .map(|&setting| key(setting))
                    .map(|key| format!("looks.{name}.{key}")),
            );
        }
        keys
    }

    /// Looks with every value the file can hold for every tool set away from its default, the
    /// text sizes to ones written with the most digits.
    pub(in crate::settings) fn largest() -> Looks {
        let mut looks = Looks::default();
        for tool in Tool::ALL {
            let default = Look::default_for(tool);
            for &setting in Setting::for_tool(tool) {
                let property = match setting {
                    Setting::TextSize => Property::TextSize(199.9),
                    Setting::RedactStrength => {
                        Property::RedactStrength(RedactStrength::clamped(511))
                    }
                    Setting::Color
                    | Setting::StrokeWidth
                    | Setting::Fill
                    | Setting::Redact
                    | Setting::HighlightStrength
                    | Setting::Shadow
                    | Setting::Alignment
                    | Setting::HorizontalAlignment
                    | Setting::Border
                    | Setting::BorderColor
                    | Setting::BorderWidth
                    | Setting::Background
                    | Setting::BackgroundColor
                    | Setting::Dim => setting
                        .choices()
                        .iter()
                        .rev()
                        .map(|&(property, _)| property)
                        .find(|&property| !default.has(property))
                        .expect("every setting offers a value besides its default"),
                };
                looks.set(property, tool, None).unwrap();
            }
        }
        looks
    }

    fn read(text: &str) -> (Looks, Vec<SettingsProblem>) {
        let (settings, problems) = Settings::from_toml(text.as_bytes());
        (settings.looks, problems)
    }

    fn set_all(sets: &[(Tool, Property)]) -> Looks {
        let mut looks = Looks::default();
        for &(tool, property) in sets {
            looks.set(property, tool, None).unwrap();
        }
        looks
    }

    #[test]
    fn the_settings_file_spells_center_as_color_is_spelt() {
        let center = Alignment {
            horizontal: Horizontal::Centre,
            vertical: Vertical::Centre,
        };
        let looks = set_all(&[
            (Tool::Text, Property::Alignment(center)),
            (Tool::Bubble, Property::Alignment(center)),
        ]);
        let written = Settings {
            looks,
            ..Settings::default()
        }
        .to_toml();
        assert!(
            written.contains("[looks.text]\nalignment = \"center\"")
                && written.contains("[looks.bubble]\nalignment = \"center-center\"")
                && !written.contains("centre"),
            "{written}"
        );
    }

    #[test]
    fn each_tool_s_look_survives_a_restart() {
        let blue = Color::from_hex("#007AFF").unwrap();
        let black = Color::from_hex("#000000").unwrap();
        let bottom_right = Alignment {
            horizontal: Horizontal::Right,
            vertical: Vertical::Bottom,
        };
        let looks = set_all(&[
            (Tool::Pen, Property::Color(black)),
            (Tool::Rectangle, Property::Color(blue)),
            (Tool::Rectangle, Property::StrokeWidth(8.0)),
            (Tool::Rectangle, Property::Fill(Fill::Solid)),
            (Tool::Rectangle, Property::Shadow(true)),
            (Tool::Rectangle, Property::Border(true)),
            (Tool::Rectangle, Property::BorderColor(black)),
            (Tool::Rectangle, Property::BorderWidth(BorderWidth::Thick)),
            (Tool::Redact, Property::Redact(RedactMode::Blur)),
            (
                Tool::Redact,
                Property::RedactStrength(RedactStrength::clamped(100)),
            ),
            (
                Tool::Highlighter,
                Property::HighlightStrength(HighlightStrength::Light),
            ),
            (Tool::Text, Property::TextSize(60.0)),
            (
                Tool::Text,
                Property::Alignment(Alignment {
                    horizontal: Horizontal::Centre,
                    vertical: Vertical::Top,
                }),
            ),
            (Tool::Bubble, Property::Alignment(bottom_right)),
            (Tool::Step, Property::TextSize(36.0)),
            (Tool::Spotlight, Property::Border(true)),
            (Tool::Spotlight, Property::Dim(SpotlightDim::Dark)),
        ]);
        assert_ne!(looks, Looks::default());
        let written = Settings {
            looks: looks.clone(),
            ..Settings::default()
        }
        .to_toml();
        let (restarted, problems) = read(&written);
        assert_eq!(problems, [], "{written}");
        assert_eq!(restarted, looks, "{written}");
        let rectangle = restarted.look(Tool::Rectangle);
        assert_eq!(rectangle.style().stroke_width, 8.0);
        assert_eq!(rectangle.style().color, blue);
        assert_eq!(rectangle.border_width(), BorderWidth::Thick);
        assert_eq!(restarted.look(Tool::Pen).style().color, black);
        assert_eq!(
            restarted.look(Tool::Arrow),
            Look::default_for(Tool::Arrow),
            "{written}"
        );
        assert_eq!(restarted.look(Tool::Bubble).alignment(), bottom_right);
        assert_eq!(
            restarted.look(Tool::Redact).redact_strength(),
            RedactStrength::clamped(100)
        );
    }

    #[test]
    fn every_value_the_strip_can_set_for_any_tool_survives_a_restart() {
        let teal = Color::from_hex("#17A2A8").unwrap();
        for tool in Tool::ALL {
            let default = Look::default_for(tool);
            for &setting in Setting::for_tool(tool) {
                let mut properties: Vec<Property> = setting
                    .choices()
                    .iter()
                    .map(|&(choice, _)| choice)
                    .collect();
                if let Some(typed) = setting.typed(&default) {
                    let range = typed.range();
                    properties
                        .extend([*range.start(), *range.end()].map(|end| typed.property(end)));
                }
                if let Some(colored) = setting.colored(&default) {
                    properties.push(colored.property(teal));
                }
                for property in properties {
                    let looks = set_all(&[(tool, property)]);
                    let written = Settings {
                        looks: looks.clone(),
                        ..Settings::default()
                    }
                    .to_toml();
                    assert_eq!(read(&written), (looks, vec![]), "{tool:?} {property:?}");
                }
            }
        }
    }

    #[test]
    fn a_colour_for_every_tool_from_an_older_file_seeds_each_tool_without_its_own() {
        let blue = Color::from_hex("#007AFF").unwrap();
        let black = Color::from_hex("#000000").unwrap();
        let (looks, problems) =
            read("[looks]\ncolor = '#007AFF'\n[looks.pen]\ncolor = '#000000'\n");
        assert_eq!(problems, []);
        assert_eq!(looks.look(Tool::Pen).style().color, black);
        for tool in [Tool::Arrow, Tool::Rectangle, Tool::Highlighter, Tool::Text] {
            assert_eq!(looks.look(tool).style().color, blue, "{tool:?}");
        }
        let written = Settings {
            looks: looks.clone(),
            ..Settings::default()
        }
        .to_toml();
        assert!(
            written.contains("[looks.arrow]\ncolor = \"#007AFF\"")
                && !written.contains("[looks]\ncolor"),
            "{written}"
        );
        assert_eq!(read(&written).0, looks);
    }

    #[test]
    fn a_damaged_look_value_falls_back_to_its_own_default_alone() {
        let invalid = |key: &str| SettingsProblem::InvalidValue { key: key.into() };
        let (looks, problems) = read(
            "[looks]\ncolor = '#12345'\n\
             [looks.arrow]\nborder = true\nborder_color = 'red'\n\
             [looks.rectangle]\nstroke_width = 7\nfill = 'filled'\nshadow = 'yes'\n\
             [looks.redact]\nredaction = 'blur'\nstrength = 4\n\
             [looks.text]\ntext_size = 0\nalignment = 'middle'\n\
             [looks.ellipse]\nfill = 3\n\
             [looks.highlighter]\nstrength = 'light'\n\
             [looks.pen]\nstroke_width = 5\n\
             [looks.step]\ntext_size = 'large'\n\
             [looks.spotlight]\ndim = 'darkest'\n",
        );
        assert_eq!(
            problems,
            [
                invalid("looks.color"),
                invalid("looks.arrow.border_color"),
                invalid("looks.rectangle.stroke_width"),
                invalid("looks.rectangle.shadow"),
                invalid("looks.ellipse.fill"),
                invalid("looks.redact.strength"),
                invalid("looks.text.text_size"),
                invalid("looks.text.alignment"),
                invalid("looks.spotlight.dim"),
                invalid("looks.step.text_size"),
            ]
        );
        let kept = set_all(&[
            (Tool::Arrow, Property::Border(true)),
            (Tool::Rectangle, Property::Fill(Fill::Solid)),
            (Tool::Pen, Property::StrokeWidth(5.0)),
            (
                Tool::Highlighter,
                Property::HighlightStrength(HighlightStrength::Light),
            ),
            (Tool::Redact, Property::Redact(RedactMode::Blur)),
        ]);
        assert_eq!(looks, kept);
    }

    #[test]
    fn unknown_look_keys_and_tools_are_ignored() {
        let (looks, problems) = read(
            "[looks]\nglow = 3\nselect = 5\n\
             [looks.lasso]\nstroke_width = 2\n\
             [looks.rectangle]\nfill = 'filled'\ncorner_radius = 4\ntext_size = 20\n",
        );
        assert_eq!(problems, []);
        assert_eq!(
            looks,
            set_all(&[(Tool::Rectangle, Property::Fill(Fill::Solid))])
        );
    }
}
