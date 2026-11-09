#![allow(
    clippy::unwrap_used,
    reason = "test target: unwrap is allowed in tests"
)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use cleanshot_core::{
    BorderWidth, Bubble, Color, Command, Document, Fill, GlyphMask, HighlightStrength, History,
    Look, PointF, Property, RectF, RedactMode, RedactStrength, RgbaImage, Shape, Style, TextBlock,
    TextMasks, Tool,
};
use cleanshot_render::Format;

const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;

fn main() -> ExitCode {
    let bless = std::env::args().any(|arg| arg == "--bless");
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    let mut failed = 0;
    let shape_scenes = scenes().into_iter().map(|(name, commands)| {
        let steps = commands
            .into_iter()
            .map(|command| (command, None))
            .collect();
        (name, steps)
    });
    for (name, steps) in shape_scenes.chain(text_scenes()) {
        let mut history = History::new(Document::new(Arc::new(gradient())));
        let mut masks = TextMasks::default();
        for (command, layers) in steps {
            history.apply(command).unwrap();
            if let Some(layers) = layers {
                let added = history.document().shapes().last().unwrap().id;
                masks.insert(added, layers);
            }
        }
        let flat = cleanshot_render::flatten(history.document(), &masks).unwrap();
        let png = cleanshot_render::encode(&flat, Format::Png).unwrap();
        let path = directory.join(format!("{name}.png"));
        if bless {
            std::fs::write(&path, &png).unwrap();
            println!("blessed {name}");
            continue;
        }
        match compare(flat.image(), &png, &path) {
            Ok(()) => println!("golden {name} ... ok"),
            Err(reason) => {
                println!("golden {name} ... FAILED: {reason}");
                failed += 1;
            }
        }
    }
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        println!(
            "{failed} golden image(s) differ; run with --bless only if the change is intended"
        );
        ExitCode::FAILURE
    }
}

fn compare(flat: &RgbaImage, png: &[u8], path: &PathBuf) -> Result<(), String> {
    let golden_bytes =
        std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if golden_bytes == png {
        return Ok(());
    }
    let golden = image::open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?
        .into_rgba8();
    if golden.dimensions() != (flat.width(), flat.height()) {
        return Err(format!(
            "size {:?}, golden {:?}",
            (flat.width(), flat.height()),
            golden.dimensions()
        ));
    }
    let differing = flat
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(golden.as_raw().as_chunks::<4>().0)
        .filter(|(ours, theirs)| ours != theirs)
        .count();
    Err(format!("PNG bytes differ; {differing} pixels differ"))
}

/// Opaque, with every pixel distinct enough that a shape drawn in the wrong place shows.
fn gradient() -> RgbaImage {
    let mut pixels = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.extend([
                u8::try_from(x * 255 / WIDTH).unwrap(),
                u8::try_from(y * 255 / HEIGHT).unwrap(),
                160,
                255,
            ]);
        }
    }
    RgbaImage::new(WIDTH, HEIGHT, pixels).unwrap()
}

fn add(shape: Shape, color: Color, stroke_width: f32) -> Command {
    Command::Add {
        shape,
        style: Style::new(color, stroke_width),
    }
}

fn point(x: f32, y: f32) -> PointF {
    PointF { x, y }
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> RectF {
    RectF {
        x,
        y,
        width,
        height,
    }
}

const RED: Color = Color {
    r: 230,
    g: 40,
    b: 40,
    a: 255,
};
const WHITE: Color = Color {
    r: 255,
    g: 255,
    b: 255,
    a: 255,
};
const YELLOW: Color = Color {
    r: 255,
    g: 230,
    b: 0,
    a: 255,
};
const BLUE: Color = Color {
    r: 0,
    g: 122,
    b: 255,
    a: 255,
};
const TRANSLUCENT_BLACK: Color = Color {
    r: 0,
    g: 0,
    b: 0,
    a: 128,
};

fn scenes() -> Vec<(&'static str, Vec<Command>)> {
    vec![
        ("lines", lines()),
        ("boxes", boxes()),
        ("highlight_over_shapes", highlight_over_shapes()),
        ("redact", redact()),
        ("crop", crop()),
        ("shadows", shadows()),
        ("borders", borders()),
    ]
}

fn lines() -> Vec<Command> {
    vec![
        add(
            Shape::Line {
                from: point(8.0, 8.0),
                to: point(88.0, 20.0),
            },
            WHITE,
            2.0,
        ),
        add(
            Shape::Arrow {
                from: point(10.0, 56.0),
                to: point(80.0, 32.0),
            },
            RED,
            3.0,
        ),
        add(
            Shape::Pen(vec![
                point(20.0, 30.0),
                point(30.0, 44.0),
                point(44.0, 36.0),
                point(52.0, 50.0),
            ]),
            TRANSLUCENT_BLACK,
            4.0,
        ),
    ]
}

fn boxes() -> Vec<Command> {
    vec![
        add(
            Shape::Rect {
                rect: rect(6.0, 6.0, 36.0, 24.0),
                fill: Fill::Hollow,
            },
            RED,
            2.0,
        ),
        add(
            Shape::Rect {
                rect: rect(50.5, 6.5, 36.0, 24.0),
                fill: Fill::Solid,
            },
            TRANSLUCENT_BLACK,
            2.0,
        ),
        add(
            Shape::Ellipse {
                rect: rect(6.0, 34.0, 36.0, 24.0),
                fill: Fill::Hollow,
            },
            WHITE,
            3.0,
        ),
        add(
            Shape::Ellipse {
                rect: rect(50.0, 34.0, 36.0, 24.0),
                fill: Fill::Solid,
            },
            RED,
            1.0,
        ),
    ]
}

fn highlight_over_shapes() -> Vec<Command> {
    vec![
        add(
            Shape::Line {
                from: point(4.0, 32.0),
                to: point(92.0, 32.0),
            },
            WHITE,
            6.0,
        ),
        add(
            Shape::Highlight {
                rect: rect(16.0, 20.0, 64.0, 24.0),
                strength: HighlightStrength::Strong,
            },
            YELLOW,
            1.0,
        ),
    ]
}

fn redact() -> Vec<Command> {
    vec![
        add(
            Shape::Line {
                from: point(4.0, 32.0),
                to: point(92.0, 32.0),
            },
            WHITE,
            4.0,
        ),
        add(
            Shape::Redact {
                rect: rect(6.0, 6.0, 38.5, 52.0),
                mode: RedactMode::Pixelate,
                strength: RedactStrength::FINE,
            },
            WHITE,
            1.0,
        ),
        add(
            Shape::Redact {
                rect: rect(52.0, 6.0, 38.0, 52.0),
                mode: RedactMode::Blur,
                strength: RedactStrength::FINE,
            },
            WHITE,
            1.0,
        ),
        add(
            Shape::Arrow {
                from: point(20.0, 56.0),
                to: point(76.0, 16.0),
            },
            RED,
            2.0,
        ),
    ]
}

fn crop() -> Vec<Command> {
    vec![
        add(
            Shape::Rect {
                rect: rect(20.0, 16.0, 40.0, 30.0),
                fill: Fill::Hollow,
            },
            RED,
            4.0,
        ),
        Command::SetCrop(Some(rect(12.0, 10.0, 56.5, 40.25))),
    ]
}

/// A command, and for a text shape it adds, the glyph layers DirectWrite would have given it.
type Step = (Command, Option<Vec<GlyphMask>>);

fn text_scenes() -> Vec<(&'static str, Vec<Step>)> {
    vec![
        ("text", text()),
        ("text_redact_crop", text_redact_crop()),
        ("text_shadow", text_shadow()),
        ("text_border", text_border()),
    ]
}

fn text_shape(color: Color) -> Command {
    let block = TextBlock::new(point(0.0, 0.0), "synthetic".into(), 12.0, 1.0).unwrap();
    add(Shape::Text(block), color, 1.0)
}

/// Full coverage over a `width` by `height` box.
fn solid(left: i32, top: i32, width: u32, height: u32, color: Option<Color>) -> GlyphMask {
    let coverage = vec![255; (width * height) as usize];
    GlyphMask::new(left, top, width, height, coverage, color).unwrap()
}

/// Coverage rising from 0 at the left edge to 255 at the right.
fn ramp(left: i32, top: i32, width: u32, height: u32) -> GlyphMask {
    let row: Vec<u8> = (0..width)
        .map(|x| u8::try_from(x * 255 / (width - 1)).unwrap())
        .collect();
    let coverage = row.repeat(height as usize);
    GlyphMask::new(left, top, width, height, coverage, None).unwrap()
}

fn text() -> Vec<Step> {
    vec![
        (text_shape(RED), Some(vec![solid(4, 4, 20, 10, None)])),
        (text_shape(WHITE), Some(vec![ramp(30, 6, 30, 8)])),
        (
            text_shape(RED),
            Some(vec![
                solid(70, 4, 12, 12, None),
                solid(73, 7, 6, 6, Some(YELLOW)),
            ]),
        ),
        (text_shape(WHITE), Some(vec![solid(86, 40, 16, 8, None)])),
        (text_shape(RED), Some(Vec::new())),
        (
            text_shape(TRANSLUCENT_BLACK),
            Some(vec![ramp(-6, 50, 40, 20)]),
        ),
    ]
}

fn text_redact_crop() -> Vec<Step> {
    vec![
        (text_shape(RED), Some(vec![ramp(10, 10, 40, 20)])),
        (
            add(
                Shape::Redact {
                    rect: rect(20.0, 15.0, 30.0, 30.0),
                    mode: RedactMode::Pixelate,
                    strength: RedactStrength::FINE,
                },
                WHITE,
                1.0,
            ),
            None,
        ),
        (text_shape(WHITE), Some(vec![solid(28, 36, 20, 6, None)])),
        (Command::SetCrop(Some(rect(5.0, 5.0, 60.0, 50.0))), None),
    ]
}

/// `shape` in `color` and `stroke_width`, casting a shadow: only a look can give it one.
fn shadowed(shape: Shape, color: Color, stroke_width: f32, fill: Fill) -> Command {
    let (shape, style) = Look::default_for(Tool::drawing(&shape))
        .with(Property::Color(color))
        .and_then(|look| look.with(Property::StrokeWidth(stroke_width)))
        .and_then(|look| look.with(Property::Fill(fill)))
        .and_then(|look| look.with(Property::Shadow(true)))
        .unwrap()
        .dress(shape);
    Command::Add { shape, style }
}

fn shadows() -> Vec<Command> {
    vec![
        shadowed(
            Shape::Line {
                from: point(8.0, 8.0),
                to: point(40.0, 8.0),
            },
            WHITE,
            3.0,
            Fill::Hollow,
        ),
        shadowed(
            Shape::Arrow {
                from: point(52.0, 24.0),
                to: point(88.0, 6.0),
            },
            RED,
            3.0,
            Fill::Hollow,
        ),
        shadowed(
            Shape::Rect {
                rect: rect(8.0, 22.0, 32.0, 20.0),
                fill: Fill::Hollow,
            },
            RED,
            2.0,
            Fill::Hollow,
        ),
        shadowed(
            Shape::Ellipse {
                rect: rect(52.0, 34.0, 32.0, 22.0),
                fill: Fill::Solid,
            },
            WHITE,
            1.0,
            Fill::Solid,
        ),
        shadowed(
            Shape::Highlight {
                rect: rect(8.0, 48.0, 32.0, 10.0),
                strength: HighlightStrength::Strong,
            },
            YELLOW,
            1.0,
            Fill::Hollow,
        ),
    ]
}

fn text_shadow() -> Vec<Step> {
    let block = TextBlock::new(point(0.0, 0.0), "synthetic".into(), 12.0, 1.0).unwrap();
    vec![(
        shadowed(Shape::Text(block), WHITE, 1.0, Fill::Hollow),
        Some(vec![solid(10, 10, 30, 12, None), ramp(50, 30, 30, 16)]),
    )]
}

/// `shape` in `color`, `stroke_width` and `fill`, with a border of `border` and `width`
/// where its tool gives one: only a look can.
fn bordered(
    shape: Shape,
    (color, stroke_width, fill): (Color, f32, Fill),
    border: Color,
    width: BorderWidth,
) -> Command {
    let (shape, style) = Look::default_for(Tool::drawing(&shape))
        .with(Property::Color(color))
        .and_then(|look| look.with(Property::StrokeWidth(stroke_width)))
        .and_then(|look| look.with(Property::Fill(fill)))
        .and_then(|look| look.with(Property::Border(true)))
        .and_then(|look| look.with(Property::BorderColor(border)))
        .and_then(|look| look.with(Property::BorderWidth(width)))
        .unwrap()
        .dress(shape);
    Command::Add { shape, style }
}

fn borders() -> Vec<Command> {
    vec![
        bordered(
            Shape::Line {
                from: point(8.0, 8.0),
                to: point(40.0, 8.0),
            },
            (RED, 3.0, Fill::Hollow),
            WHITE,
            BorderWidth::Thin,
        ),
        bordered(
            Shape::Arrow {
                from: point(52.0, 24.0),
                to: point(88.0, 8.0),
            },
            (RED, 3.0, Fill::Hollow),
            WHITE,
            BorderWidth::Medium,
        ),
        bordered(
            Shape::Rect {
                rect: rect(10.0, 22.0, 28.0, 18.0),
                fill: Fill::Hollow,
            },
            (RED, 2.0, Fill::Hollow),
            BLUE,
            BorderWidth::Thick,
        ),
        bordered(
            Shape::Ellipse {
                rect: rect(52.0, 34.0, 32.0, 22.0),
                fill: Fill::Solid,
            },
            (WHITE, 1.0, Fill::Solid),
            RED,
            BorderWidth::Thin,
        ),
        bordered(
            Shape::Highlight {
                rect: rect(8.0, 48.0, 32.0, 10.0),
                strength: HighlightStrength::Strong,
            },
            (YELLOW, 1.0, Fill::Hollow),
            BLUE,
            BorderWidth::Thick,
        ),
    ]
}

fn text_border() -> Vec<Step> {
    let block = TextBlock::new(point(0.0, 0.0), "synthetic".into(), 12.0, 1.0).unwrap();
    let bubble_text = TextBlock::new(point(60.0, 40.0), "hi".into(), 10.0, 1.0).unwrap();
    vec![
        (
            bordered(
                Shape::Text(block),
                (WHITE, 1.0, Fill::Hollow),
                BLUE,
                BorderWidth::Medium,
            ),
            Some(vec![solid(10, 10, 30, 12, None), ramp(44, 8, 30, 10)]),
        ),
        (
            bordered(
                Shape::Bubble(Bubble::new(bubble_text, point(46.0, 60.0))),
                (BLUE, 1.0, Fill::Hollow),
                WHITE,
                BorderWidth::Medium,
            ),
            Some(vec![solid(62, 42, 10, 8, None)]),
        ),
    ]
}
