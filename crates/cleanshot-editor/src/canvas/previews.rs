use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{
    Color, Document, HighlightStrength, RectF, RedactMode, RedactStrength, RgbaImage, Shape, Style,
};
use cleanshot_render::{Patch, highlighted, redacted};
use gpui::{Bounds, Pixels, RenderImage, Window, fill};

use super::shape_patches::{PatchKind, ShapePatches};
use super::{Overlay, render_image};
use crate::placement::Fit;
use crate::{Log, MasksFor};

/// What the canvas shows as the export draws it, made once rather than every frame, and the log the
/// canvas reports what it could not paint to.
#[derive(Clone)]
pub(crate) struct Previews {
    pub(super) redactions: RedactionPreviews,
    pub(super) shadows: ShapePatches,
    pub(super) borders: ShapePatches,
    highlights: HighlightPreviews,
    pub(super) log: Log,
}

impl Previews {
    pub(crate) fn new(masks: MasksFor, log: Log) -> Self {
        Self {
            redactions: RedactionPreviews::new(Rc::clone(&log)),
            shadows: ShapePatches::new(PatchKind::Shadow, Rc::clone(&log)),
            borders: ShapePatches::new(PatchKind::Border(masks), Rc::clone(&log)),
            highlights: HighlightPreviews::new(Rc::clone(&log)),
            log,
        }
    }

    /// Brings the previews in step with `document` and what `overlay` shows over it, freeing
    /// the atlas space of those it no longer holds.
    pub(crate) fn update(
        &mut self,
        document: &Document,
        overlay: Option<&Overlay>,
        window: &mut Window,
    ) {
        for image in self.replaced_by(document, overlay) {
            if window.drop_image(image).is_err() {
                (self.log)("preview_image not_dropped");
            }
        }
    }

    /// Brings the previews in step, returning the images no preview holds now. A shape being
    /// edited gets the export's highlight, border and shadow at once, except that a moved shape
    /// keeps the shadow and border it has and a redaction waits for release.
    fn replaced_by(
        &mut self,
        document: &Document,
        overlay: Option<&Overlay>,
    ) -> Vec<Arc<RenderImage>> {
        let (shown, moved_by) = overlay.map_or((document, None), |overlay| {
            (&*overlay.shown, overlay.moved_by)
        });
        let outlined = if moved_by.is_some() { document } else { shown };
        let mut replaced = self.redactions.update(document);
        replaced.extend(self.shadows.update(outlined));
        replaced.extend(self.borders.update(outlined));
        replaced.extend(self.highlights.update(shown));
        replaced
    }

    /// Paints each of `document`'s redactions, so the minimap shows nothing they hide.
    pub(crate) fn paint_redactions(&self, document: &Document, fit: &Fit, window: &mut Window) {
        for (rect, mode, strength) in redactions(document) {
            self.redactions.paint(rect, mode, strength, fit, window);
        }
    }

    /// Paints every highlight among `shapes`, under the spotlight dim and before any shape: a
    /// dim and a multiply both scale each channel, so either order gives the export's pixels.
    pub(super) fn paint_highlights<'a>(
        &self,
        shapes: impl Iterator<Item = (&'a Shape, &'a Style)>,
        fit: &Fit,
        window: &mut Window,
    ) {
        for (shape, style) in shapes {
            if let Shape::Highlight { rect, strength } = shape {
                self.highlights
                    .paint(*rect, style.color, *strength, fit, window);
            }
        }
    }
}

fn redactions(
    document: &Document,
) -> impl Iterator<Item = (RectF, RedactMode, RedactStrength)> + '_ {
    document
        .shapes()
        .iter()
        .filter_map(|entry| match entry.shape {
            Shape::Redact {
                rect,
                mode,
                strength,
            } => Some((rect, mode, strength)),
            Shape::Arrow { .. }
            | Shape::Line { .. }
            | Shape::Rect { .. }
            | Shape::Ellipse { .. }
            | Shape::Pen(_)
            | Shape::Text(_)
            | Shape::Step(_)
            | Shape::Bubble(_)
            | Shape::Highlight { .. }
            | Shape::Spotlight(_) => None,
        })
}

fn rect_bits(rect: RectF) -> [u32; 4] {
    [rect.x, rect.y, rect.width, rect.height].map(f32::to_bits)
}

/// What the canvas shows for each highlight: the image multiplied as an export multiplies it, made
/// once for each rectangle, colour and strength rather than every frame, as GPUI has no multiply
/// blend.
#[derive(Clone)]
struct HighlightPreviews {
    previews: HashMap<HighlightKey, Option<(RectF, Arc<RenderImage>)>>,
    log: Log,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct HighlightKey {
    rect: [u32; 4],
    color: [u8; 4],
    strength: HighlightStrength,
}

impl HighlightPreviews {
    fn new(log: Log) -> Self {
        Self {
            previews: HashMap::new(),
            log,
        }
    }

    fn key(rect: RectF, color: Color, strength: HighlightStrength) -> HighlightKey {
        HighlightKey {
            rect: rect_bits(rect),
            color: [color.r, color.g, color.b, color.a],
            strength,
        }
    }

    /// Keeps a preview for each highlight in `document`, making the ones it lacks and dropping
    /// the ones no longer there, whose images it returns. One that could not be made is logged
    /// and tried again next time.
    fn update(&mut self, document: &Document) -> Vec<Arc<RenderImage>> {
        let mut kept = HashMap::new();
        for entry in document.shapes() {
            let Shape::Highlight { rect, strength } = entry.shape else {
                continue;
            };
            let key = Self::key(rect, entry.style.color, strength);
            if let Some(preview) = self.previews.remove(&key) {
                kept.insert(key, preview);
                continue;
            }
            match highlighted(document.base(), rect, entry.style.color, strength) {
                Ok(patch) => {
                    kept.insert(key, patch.as_ref().map(placed));
                }
                Err(_) => (self.log)("highlight_preview not_made"),
            }
        }
        std::mem::replace(&mut self.previews, kept)
            .into_values()
            .flatten()
            .map(|(_, image)| image)
            .collect()
    }

    /// What the canvas paints for the highlight at `rect` in `color` at `strength`.
    fn painted(&self, rect: RectF, color: Color, strength: HighlightStrength) -> HighlightPaint {
        match self.previews.get(&Self::key(rect, color, strength)) {
            Some(Some((bounds, image))) => HighlightPaint::Image(*bounds, Arc::clone(image)),
            Some(None) | None => HighlightPaint::Nothing,
        }
    }

    fn paint(
        &self,
        rect: RectF,
        color: Color,
        strength: HighlightStrength,
        fit: &Fit,
        window: &mut Window,
    ) {
        match self.painted(rect, color, strength) {
            HighlightPaint::Image(area, image) => {
                paint_placed(
                    area,
                    &image,
                    fit,
                    window,
                    &self.log,
                    "highlight_preview not_painted",
                );
            }
            HighlightPaint::Nothing => {}
        }
    }
}

/// What the canvas paints for one highlight.
enum HighlightPaint {
    /// The export's own pixels, at their place on the image.
    Image(RectF, Arc<RenderImage>),
    /// Nothing: the highlight covers no pixel, or its image could not be made.
    Nothing,
}

/// What the canvas shows for each redaction: the pixels an export leaves there,
/// made once for each rectangle and mode rather than every frame.
#[derive(Clone)]
pub(super) struct RedactionPreviews {
    previews: HashMap<RedactionKey, RedactionPreview>,
    log: Log,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct RedactionKey {
    rect: [u32; 4],
    mode: RedactMode,
    strength: RedactStrength,
}

impl RedactionKey {
    fn new(rect: RectF, mode: RedactMode, strength: RedactStrength) -> Self {
        Self {
            rect: rect_bits(rect),
            mode,
            strength,
        }
    }
}

#[derive(Clone)]
enum RedactionPreview {
    Pixels {
        bounds: RectF,
        image: Arc<RenderImage>,
    },
    CoversNoPixel,
    /// The preview could not be made, so the area is painted over to keep what is under it
    /// hidden.
    Failed,
}

impl RedactionPreviews {
    pub(crate) fn new(log: Log) -> Self {
        Self {
            previews: HashMap::new(),
            log,
        }
    }

    /// Keeps a preview for each redaction in `document`, making the ones it lacks and dropping
    /// the ones no longer there, whose images it returns. A redaction still being dragged is not
    /// in the document, so it has none until release and paints as a solid fill.
    pub(crate) fn update(&mut self, document: &Document) -> Vec<Arc<RenderImage>> {
        let mut kept = HashMap::new();
        for (rect, mode, strength) in redactions(document) {
            let key = RedactionKey::new(rect, mode, strength);
            if kept.contains_key(&key) {
                continue;
            }
            let preview = self.previews.remove(&key).unwrap_or_else(|| {
                redaction_preview(document.base(), rect, mode, strength, &*self.log)
            });
            kept.insert(key, preview);
        }
        std::mem::replace(&mut self.previews, kept)
            .into_values()
            .filter_map(|preview| match preview {
                RedactionPreview::Pixels { image, .. } => Some(image),
                RedactionPreview::CoversNoPixel | RedactionPreview::Failed => None,
            })
            .collect()
    }

    /// Paints the redaction at `rect` and returns where it painted, `None` if nowhere.
    pub(super) fn paint(
        &self,
        rect: RectF,
        mode: RedactMode,
        strength: RedactStrength,
        fit: &Fit,
        window: &mut Window,
    ) -> Option<Bounds<Pixels>> {
        let hide = |window: &mut Window| {
            let bounds = fit.bounds(rect);
            window.paint_quad(fill(bounds, gpui::black()));
            Some(bounds)
        };
        match self.previews.get(&RedactionKey::new(rect, mode, strength)) {
            Some(RedactionPreview::Pixels { bounds, image }) => paint_placed(
                *bounds,
                image,
                fit,
                window,
                &self.log,
                "redaction_preview not_painted",
            )
            .or_else(|| hide(window)),
            Some(RedactionPreview::CoversNoPixel) => None,
            Some(RedactionPreview::Failed) | None => hide(window),
        }
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "image sides are far below 2^24, where f32 stops being exact"
)]
pub(super) fn placed(patch: &Patch) -> (RectF, Arc<RenderImage>) {
    let bounds = RectF {
        x: patch.left as f32,
        y: patch.top as f32,
        width: patch.pixels.width() as f32,
        height: patch.pixels.height() as f32,
    };
    (bounds, render_image(&patch.pixels))
}

/// Paints `image` over `area` of the picture and returns where; a failure logs `not_painted`.
pub(super) fn paint_placed(
    area: RectF,
    image: &Arc<RenderImage>,
    fit: &Fit,
    window: &mut Window,
    log: &Log,
    not_painted: &'static str,
) -> Option<Bounds<Pixels>> {
    let bounds = fit.bounds(area);
    let painted = window.paint_image(bounds, bounds, 0.0.into(), Arc::clone(image), 0, false);
    if painted.is_err() {
        log(not_painted);
        return None;
    }
    Some(bounds)
}

fn redaction_preview(
    image: &RgbaImage,
    rect: RectF,
    mode: RedactMode,
    strength: RedactStrength,
    log: &dyn Fn(&'static str),
) -> RedactionPreview {
    match redacted(image, rect, mode, strength) {
        Ok(Some(patch)) => {
            let (bounds, image) = placed(&patch);
            RedactionPreview::Pixels { bounds, image }
        }
        Ok(None) => RedactionPreview::CoversNoPixel,
        Err(_) => {
            log("redaction_preview not_made");
            RedactionPreview::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cleanshot_core::{Command, Fill, History, PointF, moved};

    fn image_of(previews: &RedactionPreviews, rect: RectF, mode: RedactMode) -> Arc<RenderImage> {
        match previews
            .previews
            .get(&RedactionKey::new(rect, mode, RedactStrength::FINE))
        {
            Some(RedactionPreview::Pixels { image, .. }) => Arc::clone(image),
            Some(RedactionPreview::CoversNoPixel | RedactionPreview::Failed) | None => {
                panic!("a preview of {rect:?}")
            }
        }
    }

    #[test]
    fn the_minimap_is_given_every_redaction_in_the_document() {
        let rect = RectF {
            x: 3.0,
            y: 2.0,
            width: 20.0,
            height: 10.0,
        };
        let image = RgbaImage::new(64, 48, vec![200; 64 * 48 * 4]).unwrap();
        let mut history = History::new(Document::new(Arc::new(image)));
        for mode in [RedactMode::Pixelate, RedactMode::Blur] {
            let shape = Shape::Redact {
                rect,
                mode,
                strength: RedactStrength::FINE,
            };
            let style = Style::new(
                Color {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                },
                1.0,
            );
            history.apply(Command::Add { shape, style }).unwrap();
        }
        let listed: Vec<_> = redactions(history.document()).collect();
        assert_eq!(
            listed,
            [
                (rect, RedactMode::Pixelate, RedactStrength::FINE),
                (rect, RedactMode::Blur, RedactStrength::FINE),
            ]
        );
    }

    #[test]
    fn each_preview_no_longer_in_the_document_gives_back_its_image() {
        use cleanshot_core::{Look, Property, Tool};

        let area = RectF {
            x: 8.0,
            y: 6.0,
            width: 20.0,
            height: 10.0,
        };
        let style = Style::new(
            Color {
                r: 255,
                g: 200,
                b: 0,
                a: 255,
            },
            1.0,
        );
        let hollow = Shape::Rect {
            rect: area,
            fill: Fill::Hollow,
        };
        let (shadowed, shadow_style) = Look::default_for(Tool::drawing(&hollow))
            .with(Property::Fill(Fill::Hollow))
            .and_then(|look| look.with(Property::Shadow(true)))
            .unwrap()
            .dress(hollow);
        let image = RgbaImage::new(64, 48, vec![200; 64 * 48 * 4]).unwrap();
        let mut history = History::new(Document::new(Arc::new(image)));
        for (shape, style) in [
            (
                Shape::Redact {
                    rect: area,
                    mode: RedactMode::Pixelate,
                    strength: RedactStrength::FINE,
                },
                style,
            ),
            (
                Shape::Highlight {
                    rect: area,
                    strength: HighlightStrength::Medium,
                },
                style,
            ),
            (shadowed, shadow_style),
        ] {
            history.apply(Command::Add { shape, style }).unwrap();
        }
        let mut previews = Previews::new(Rc::new(|_| None), Rc::new(|_| {}));
        assert_eq!(previews.replaced_by(history.document(), None).len(), 0);
        assert_eq!(previews.replaced_by(history.document(), None).len(), 0);
        for _ in 0..3 {
            history.undo().unwrap();
        }
        assert_eq!(previews.replaced_by(history.document(), None).len(), 3);
    }

    #[test]
    fn a_resize_remakes_a_border_and_shadow_at_any_size_and_a_move_keeps_them() {
        use cleanshot_core::{BorderWidth, Look, Property, Tool};

        let hollow = |width, height| Shape::Rect {
            rect: RectF {
                x: 10.0,
                y: 10.0,
                width,
                height,
            },
            fill: Fill::Hollow,
        };
        let (shape, style) = Look::default_for(Tool::Rectangle)
            .with(Property::Fill(Fill::Hollow))
            .and_then(|look| look.with(Property::Shadow(true)))
            .and_then(|look| look.with(Property::Border(true)))
            .and_then(|look| look.with(Property::BorderWidth(BorderWidth::Thick)))
            .unwrap()
            .dress(hollow(60.0, 40.0));
        let image = RgbaImage::new(1100, 700, vec![200; 1100 * 700 * 4]).unwrap();
        let mut history = History::new(Document::new(Arc::new(image)));
        history
            .apply(Command::Add {
                shape: shape.clone(),
                style,
            })
            .unwrap();
        let document = history.document();
        let id = document.shapes()[0].id;
        let dragged_to = |shape: &Shape, moved_by| {
            let preview = Command::Replace {
                id,
                shape: shape.clone(),
                style,
            };
            Overlay {
                shown: Rc::new(preview.previewed_on(document)),
                preview: Some(preview),
                moved_by,
                selected: None,
                crop_handles: false,
                over: None,
            }
        };
        let mut previews = Previews::new(Rc::new(|_| None), Rc::new(|_| {}));
        let mut after = |overlay: &Overlay| {
            previews.replaced_by(document, Some(overlay));
            (
                previews.shadows.previews[&id].of.shape.clone(),
                previews.borders.previews[&id].of.shape.clone(),
            )
        };

        let resized = hollow(80.0, 50.0);
        assert_eq!(
            after(&dragged_to(&resized, None)),
            (resized.clone(), resized)
        );
        let by = PointF { x: 30.0, y: 5.0 };
        assert_eq!(
            after(&dragged_to(&moved(&shape, by), Some(by))),
            (shape.clone(), shape),
            "a move paints the images it has"
        );
        let large = hollow(1000.0, 600.0);
        assert_eq!(
            after(&dragged_to(&large, None)),
            (large.clone(), large),
            "a large shape is previewed like any other"
        );
    }

    #[test]
    fn a_redaction_preview_is_made_once_and_dropped_with_its_redaction() {
        let image = RgbaImage::new(64, 48, vec![200; 64 * 48 * 4]).unwrap();
        let rect = RectF {
            x: 3.0,
            y: 2.0,
            width: 20.0,
            height: 10.0,
        };
        let mode = RedactMode::Blur;
        let add = || Command::Add {
            shape: Shape::Redact {
                rect,
                mode,
                strength: RedactStrength::FINE,
            },
            style: Style::new(
                Color {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                },
                1.0,
            ),
        };
        let mut history = History::new(Document::new(Arc::new(image)));
        history.apply(add()).unwrap();
        history.apply(add()).unwrap();
        let mut previews = RedactionPreviews::new(Rc::new(|_| {}));
        previews.update(history.document());
        let first = image_of(&previews, rect, mode);
        history.undo().unwrap();
        previews.update(history.document());
        assert!(Arc::ptr_eq(&first, &image_of(&previews, rect, mode)));
        history.undo().unwrap();
        previews.update(history.document());
        assert!(previews.previews.is_empty());
    }

    #[test]
    fn a_highlight_preview_is_made_once_kept_until_it_changes_and_dropped_after() {
        let image = RgbaImage::new(64, 48, vec![200; 64 * 48 * 4]).unwrap();
        let rect = RectF {
            x: 3.5,
            y: 2.0,
            width: 20.0,
            height: 10.0,
        };
        let yellow = Color {
            r: 255,
            g: 230,
            b: 0,
            a: 255,
        };
        let mut history = History::new(Document::new(Arc::new(image)));
        history
            .apply(Command::Add {
                shape: Shape::Highlight {
                    rect,
                    strength: HighlightStrength::Strong,
                },
                style: Style::new(yellow, 0.0),
            })
            .unwrap();
        let image_of = |previews: &HighlightPreviews, color| match &previews.previews
            [&HighlightPreviews::key(rect, color, HighlightStrength::Strong)]
        {
            Some((bounds, image)) => (*bounds, Arc::clone(image)),
            None => panic!("a highlight image"),
        };
        let mut previews = HighlightPreviews::new(Rc::new(|_| {}));
        previews.update(history.document());
        let (bounds, first) = image_of(&previews, yellow);
        assert_eq!(
            (bounds.x, bounds.y, bounds.width, bounds.height),
            (3.0, 2.0, 21.0, 10.0)
        );
        let pen = Command::Add {
            shape: Shape::Pen(vec![PointF { x: 1.0, y: 1.0 }]),
            style: Style::new(yellow, 2.0),
        };
        history.apply(pen).unwrap();
        previews.update(history.document());
        assert!(Arc::ptr_eq(&first, &image_of(&previews, yellow).1));
        let id = history.document().shapes()[0].id;
        let blue = Color { b: 255, ..yellow };
        history
            .apply(Command::Replace {
                id,
                shape: Shape::Highlight {
                    rect,
                    strength: HighlightStrength::Strong,
                },
                style: Style::new(blue, 0.0),
            })
            .unwrap();
        previews.update(history.document());
        image_of(&previews, blue);
        assert_eq!(previews.previews.len(), 1, "the yellow preview is dropped");
    }

    #[test]
    fn a_highlight_paints_the_exported_pixels() {
        use cleanshot_core::TextMasks;

        let (width, height) = (40, 30);
        let text = (0..width * height)
            .flat_map(|index| {
                if index % 3 == 0 {
                    [20, 20, 20, 255]
                } else {
                    [240, 240, 240, 255]
                }
            })
            .collect();
        let image = RgbaImage::new(width, height, text).unwrap();
        let rect = RectF {
            x: 4.0,
            y: 3.0,
            width: 20.0,
            height: 10.0,
        };
        let yellow = Color {
            r: 255,
            g: 230,
            b: 0,
            a: 255,
        };
        let mut history = History::new(Document::new(Arc::new(image)));
        history
            .apply(Command::Add {
                shape: Shape::Highlight {
                    rect,
                    strength: HighlightStrength::Strong,
                },
                style: Style::new(yellow, 0.0),
            })
            .unwrap();
        let mut previews = HighlightPreviews::new(Rc::new(|_| {}));
        previews.update(history.document());
        let HighlightPaint::Image(bounds, image) =
            previews.painted(rect, yellow, HighlightStrength::Strong)
        else {
            panic!("a drawn highlight paints the export's pixels");
        };
        assert_eq!(
            (bounds.x, bounds.y, bounds.width, bounds.height),
            (4.0, 3.0, 20.0, 10.0)
        );
        let export = cleanshot_render::flatten(history.document(), &TextMasks::default()).unwrap();
        let shown = image.as_bytes(0).unwrap().as_chunks::<4>().0;
        let mut index = 0;
        for y in 3..13 {
            for x in 4..24 {
                let at = ((y * width + x) * 4) as usize;
                let [r, g, b, a] = export.image().pixels()[at..at + 4] else {
                    unreachable!("a pixel is four bytes");
                };
                assert_eq!(shown[index], [b, g, r, a], "{x},{y}");
                index += 1;
            }
        }
    }
}
