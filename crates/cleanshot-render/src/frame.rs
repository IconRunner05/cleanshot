use std::ops::Range;

use cleanshot_core::Frame;
use tiny_skia::{BlendMode, ColorU8, Paint, PathBuilder, Pixmap, Rect, Transform};

use crate::Error;
use crate::shadow::gaussian;

/// `image` inside `frame`: a 1 px border at 20% black, then the shadow cast by image and border
/// together. The image's own pixels are copied unchanged, and the margin is transparent where
/// neither reaches (F-44).
pub(crate) fn framed(image: Pixmap, frame: Frame) -> Result<Pixmap, Error> {
    let margins = frame.margins();
    if frame == Frame::default() {
        return Ok(image);
    }
    let too_large = || Error::TooLarge {
        width: image.width(),
        height: image.height(),
    };
    let width = (image.width())
        .checked_add(margins.left + margins.right)
        .ok_or_else(too_large)?;
    let height = (image.height())
        .checked_add(margins.top + margins.bottom)
        .ok_or_else(too_large)?;
    let mut framed = Pixmap::new(width, height).ok_or_else(too_large)?;
    let border = u32::from(frame.border);
    let block_left = margins.left - border;
    let block_top = margins.top - border;
    let block_width = image.width() + 2 * border;
    let block_height = image.height() + 2 * border;
    if frame.shadow {
        cast_shadow(
            &mut framed,
            (block_left, block_left + block_width),
            (
                block_top + Frame::SHADOW_OFFSET,
                block_top + Frame::SHADOW_OFFSET + block_height,
            ),
            (
                margins.left as usize..(margins.left + image.width()) as usize,
                margins.top as usize..(margins.top + image.height()) as usize,
            ),
        );
    }
    if frame.border {
        draw_border(
            &mut framed,
            block_left,
            block_top,
            block_width,
            block_height,
        );
    }
    let (left, row_length) = (margins.left as usize, image.width() as usize);
    let framed_rows = framed
        .pixels_mut()
        .chunks_exact_mut(width as usize)
        .skip(margins.top as usize);
    for (framed_row, image_row) in framed_rows.zip(image.pixels().chunks_exact(row_length)) {
        framed_row[left..left + row_length].copy_from_slice(image_row);
    }
    Ok(framed)
}

/// Black whose alpha at (x, y) is the box `columns` x `rows` blurred by a Gaussian of standard
/// deviation half of [`Frame::SHADOW_BLUR`]. A blurred box is one blurred edge profile per axis,
/// multiplied. Pixels under `image` are left alone, since the image covers them.
fn cast_shadow(
    pixmap: &mut Pixmap,
    columns: (u32, u32),
    rows: (u32, u32),
    image: (Range<usize>, Range<usize>),
) {
    let across = blurred_span(pixmap.width(), columns);
    let down = blurred_span(pixmap.height(), rows);
    let width = pixmap.width() as usize;
    let (image_columns, image_rows) = image;
    for ((row, y), &down) in pixmap
        .pixels_mut()
        .chunks_exact_mut(width)
        .zip(0..)
        .zip(&down)
    {
        let shaded = if image_rows.contains(&y) {
            [0..image_columns.start, image_columns.end..width]
        } else {
            [0..width, 0..0]
        };
        for x in shaded.into_iter().flatten() {
            let coverage = across[x] * down;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "coverage is in 0..=1, so the product is in 0..=255"
            )]
            let alpha = (f64::from(Frame::SHADOW_ALPHA) * coverage * 255.0).round() as u8;
            row[x] = ColorU8::from_rgba(0, 0, 0, alpha).premultiply();
        }
    }
}

/// For each of `length` pixels, the share of a Gaussian centred on it that falls in `span`.
fn blurred_span(length: u32, span: (u32, u32)) -> Vec<f64> {
    let (reach, weights) = gaussian(Frame::SHADOW_BLUR);
    let reach = i64::from(reach);
    let total: f64 = weights.iter().sum();
    let inside = |pixel: i64| (i64::from(span.0)..i64::from(span.1)).contains(&pixel);
    (0..i64::from(length))
        .map(|pixel| {
            (-reach..=reach)
                .zip(&weights)
                .filter(|&(offset, _)| inside(pixel + offset))
                .map(|(_, weight)| weight)
                .sum::<f64>()
                / total
        })
        .collect()
}

fn draw_border(pixmap: &mut Pixmap, left: u32, top: u32, width: u32, height: u32) {
    let mut paint = Paint {
        anti_alias: false,
        blend_mode: BlendMode::SourceOver,
        ..Paint::default()
    };
    paint.set_color_rgba8(0, 0, 0, Frame::BORDER_ALPHA);
    #[expect(clippy::cast_precision_loss, reason = "image sides are under 2^24")]
    let edges = {
        let (left, top, width, height) = (left as f32, top as f32, width as f32, height as f32);
        [
            Rect::from_xywh(left, top, width, 1.0),
            Rect::from_xywh(left, top + height - 1.0, width, 1.0),
            Rect::from_xywh(left, top + 1.0, 1.0, height - 2.0),
            Rect::from_xywh(left + width - 1.0, top + 1.0, 1.0, height - 2.0),
        ]
    };
    for edge in edges.into_iter().flatten() {
        pixmap.fill_path(
            &PathBuilder::from_rect(edge),
            &paint,
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: Frame = Frame {
        border: true,
        shadow: true,
    };

    fn grey(width: u32, height: u32) -> Pixmap {
        let mut image = Pixmap::new(width, height).unwrap();
        image.fill(tiny_skia::Color::from_rgba8(90, 120, 150, 255));
        image
    }

    fn alpha(pixmap: &Pixmap, x: u32, y: u32) -> u8 {
        pixmap.pixel(x, y).unwrap().alpha()
    }

    #[test]
    fn f44_each_frame_grows_the_image_by_its_margins() {
        let size = |frame| {
            let framed = framed(grey(40, 30), frame).unwrap();
            (framed.width(), framed.height())
        };
        assert_eq!(size(Frame::default()), (40, 30));
        assert_eq!(
            size(Frame {
                border: true,
                shadow: false
            }),
            (42, 32)
        );
        assert_eq!(
            size(Frame {
                border: false,
                shadow: true
            }),
            (88, 78)
        );
        assert_eq!(size(BOTH), (90, 80));
    }

    #[test]
    fn f44_the_frame_leaves_every_image_pixel_as_it_was() {
        let image = grey(40, 30);
        let framed = framed(image.clone(), BOTH).unwrap();
        let inside = tiny_skia::IntRect::from_xywh(25, 17, 40, 30).unwrap();
        assert_eq!(framed.clone_rect(inside).unwrap(), image);
    }

    #[test]
    fn d80_the_border_is_one_pixel_of_black_at_20_percent_around_the_image() {
        let framed = framed(
            grey(40, 30),
            Frame {
                border: true,
                shadow: false,
            },
        )
        .unwrap();
        for (x, y) in [(0, 0), (41, 0), (0, 31), (41, 31), (20, 0), (0, 15)] {
            let pixel = framed.pixel(x, y).unwrap();
            assert_eq!(
                (pixel.red(), pixel.alpha()),
                (0, Frame::BORDER_ALPHA),
                "({x}, {y})"
            );
        }
    }

    #[test]
    fn d80_the_shadow_falls_below_the_image_and_fades_to_nothing_at_the_edge() {
        let framed = framed(
            grey(200, 100),
            Frame {
                border: false,
                shadow: true,
            },
        )
        .unwrap();
        let (middle, below, above) = (124, 16 + 100, 15);
        assert!(
            (44..=52).contains(&alpha(&framed, middle, below)),
            "{}",
            alpha(&framed, middle, below)
        );
        assert!(alpha(&framed, middle, below) > 2 * alpha(&framed, middle, above));
        assert_eq!(alpha(&framed, 0, 0), 0);
        assert!(alpha(&framed, middle, framed.height() - 1) <= 2);
        assert_eq!(
            alpha(&framed, 23, 60),
            alpha(&framed, 224, 60),
            "left and right alike"
        );
    }
}
