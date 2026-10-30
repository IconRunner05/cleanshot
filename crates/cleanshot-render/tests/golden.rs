use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use cleanshot_core::{
    Color, Command, Document, Fill, History, PointF, RectF, RedactMode, RgbaImage, Shape, Style,
};
use cleanshot_render::Format;

const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;

fn main() -> ExitCode {
    let bless = std::env::args().any(|arg| arg == "--bless");
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    let mut failed = 0;
    for (name, commands) in scenes() {
        let mut history = History::new(Document::new(Arc::new(gradient())));
        for command in commands {
            history.apply(command).unwrap();
        }
        let flat = cleanshot_render::flatten(history.document()).unwrap();
        let png = cleanshot_render::encode(&flat, Format::Png).unwrap();
        let path = directory.join(format!("{name}.png"));
        if bless {
            std::fs::write(&path, &png).unwrap();
            println!("blessed {name}");
            continue;
        }
        match compare(&flat, &png, &path) {
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
        style: Style {
            color,
            stroke_width,
        },
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
        add(Shape::Highlight(rect(16.0, 20.0, 64.0, 24.0)), YELLOW, 1.0),
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
            },
            WHITE,
            1.0,
        ),
        add(
            Shape::Redact {
                rect: rect(52.0, 6.0, 38.0, 52.0),
                mode: RedactMode::Blur,
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
