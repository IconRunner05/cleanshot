use cleanshot_core::{Color, HighlightStrength, RectF, RgbaImage};

use crate::flatten::{Covered, highlight};
use crate::{Error, Patch};

/// The pixels a highlight of `rect` in `color` at `strength` leaves of `image`, exactly as an export draws
/// them where no shape is under it (F-35, D-125); `None` if `rect` covers no pixel.
///
/// # Errors
///
/// [`Error::TooLarge`] if the covered area exceeds the renderer's size limit.
pub fn highlighted(
    image: &RgbaImage,
    rect: RectF,
    color: Color,
    strength: HighlightStrength,
) -> Result<Option<Patch>, Error> {
    let Some(mut covered) = Covered::of(image, rect)? else {
        return Ok(None);
    };
    let local = covered.local(rect);
    highlight(&mut covered.pixmap, local, color, strength);
    covered.into_patch().map(Some)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cleanshot_core::{Command, Document, History, Shape, Style, TextMasks};

    use super::*;
    use crate::flatten;

    #[test]
    fn d125_the_editor_preview_of_a_highlight_is_the_exported_pixels() {
        let (width, height) = (40, 30);
        let pixels = (0..width * height)
            .flat_map(|index| {
                let [low, high, ..] = u32::to_le_bytes(index * 7);
                [low, high, low ^ high, 255]
            })
            .collect();
        let image = RgbaImage::new(width, height, pixels).unwrap();
        let rect = RectF {
            x: 3.4,
            y: 2.6,
            width: 20.5,
            height: 11.2,
        };
        let yellow = Color {
            r: 255,
            g: 230,
            b: 0,
            a: 255,
        };
        let mut history = History::new(Document::new(Arc::new(image.clone())));
        history
            .apply(Command::Add {
                shape: Shape::Highlight {
                    rect,
                    strength: HighlightStrength::Strong,
                },
                style: Style::new(yellow, 0.0),
            })
            .unwrap();
        let export = flatten(history.document(), &TextMasks::default()).unwrap();
        let preview = highlighted(&image, rect, yellow, HighlightStrength::Strong)
            .unwrap()
            .unwrap();
        let (left, top) = (preview.left, preview.top);
        assert_eq!(
            (left, top, preview.pixels.width(), preview.pixels.height()),
            (3, 2, 21, 12)
        );
        for y in 0..preview.pixels.height() {
            for x in 0..preview.pixels.width() {
                let at = ((y * preview.pixels.width() + x) * 4) as usize;
                let on_image = (((top + y) * width + left + x) * 4) as usize;
                assert_eq!(
                    preview.pixels.pixels()[at..at + 4],
                    export.image().pixels()[on_image..on_image + 4],
                    "{x},{y}"
                );
            }
        }
    }

    #[test]
    fn d123_each_highlight_strength_tints_at_its_own_opacity() {
        let white = RgbaImage::new(8, 8, vec![255; 8 * 8 * 4]).unwrap();
        let rect = RectF {
            x: 0.0,
            y: 0.0,
            width: 8.0,
            height: 8.0,
        };
        let yellow = Color {
            r: 255,
            g: 230,
            b: 0,
            a: 255,
        };
        let blue_left = |strength| {
            let patch = highlighted(&white, rect, yellow, strength)
                .unwrap()
                .unwrap();
            patch.pixels.pixels()[2]
        };
        let left: Vec<_> = [
            HighlightStrength::Light,
            HighlightStrength::Medium,
            HighlightStrength::Strong,
        ]
        .map(blue_left)
        .into();
        assert_eq!(left, [166, 102, 0]);
    }
}
