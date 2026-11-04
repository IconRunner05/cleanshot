use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{
    Color, Document, Fill, HighlightStrength, Label, RectF, RedactMode, RedactStrength, RgbaImage,
    Shape, ShapeEntry, ShapeId, Style,
};
use cleanshot_render::{Patch, highlighted, redacted, shape_shadow};
use gpui::{
    App, Bounds, BoxShadow, Corners, Pixels, RenderImage, Rgba, Window, fill, hsla, point, px,
};

use super::{Painting, Unpainted, render_image, rgba};
use crate::Log;
use crate::placement::Fit;
use crate::text::paint_label;

/// What the canvas shows as the export draws it, made once rather than every frame (D-58,
/// D-114, D-125), and the log the canvas reports what it could not paint to (D-120).
#[derive(Clone)]
pub(crate) struct Previews {
    pub(super) redactions: RedactionPreviews,
    pub(super) shadows: ShadowPreviews,
    highlights: HighlightPreviews,
    pub(super) log: Log,
}

impl Previews {
    pub(crate) fn new(log: Log) -> Self {
        Self {
            redactions: RedactionPreviews::new(Rc::clone(&log)),
            shadows: ShadowPreviews::new(Rc::clone(&log)),
            highlights: HighlightPreviews::new(Rc::clone(&log)),
            log,
        }
    }

    /// Brings the previews in step with `document`.
    pub(crate) fn update(&mut self, document: &Document) {
        self.redactions.update(document);
        self.shadows.update(document);
        self.highlights.update(document);
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

fn rect_bits(rect: RectF) -> [u32; 4] {
    [rect.x, rect.y, rect.width, rect.height].map(f32::to_bits)
}

/// What the canvas shows for each highlight: the image multiplied as an export multiplies it
/// (F-35, D-125), made once for each rectangle, colour and strength rather than every frame. GPUI has no
/// multiply blend, so a highlight still being dragged is a translucent fill until release.
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
    /// the ones no longer there. One that could not be made is logged and tried again next time.
    fn update(&mut self, document: &Document) {
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
        self.previews = kept;
    }

    /// What the canvas paints for the highlight at `rect` in `color` at `strength`.
    fn painted(&self, rect: RectF, color: Color, strength: HighlightStrength) -> HighlightPaint {
        match self.previews.get(&Self::key(rect, color, strength)) {
            Some(Some((bounds, image))) => HighlightPaint::Image(*bounds, Arc::clone(image)),
            Some(None) => HighlightPaint::Nothing,
            None => HighlightPaint::Fill(Rgba {
                a: DRAGGED_HIGHLIGHT_ALPHA * f32::from(strength.alpha()) / 255.0,
                ..rgba(color)
            }),
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
            HighlightPaint::Image(bounds, image) => {
                let bounds = fit.bounds(bounds);
                let painted = window.paint_image(bounds, bounds, 0.0.into(), image, 0, false);
                if painted.is_err() {
                    (self.log)("highlight_preview not_painted");
                }
            }
            HighlightPaint::Fill(color) => window.paint_quad(fill(fit.bounds(rect), color)),
            HighlightPaint::Nothing => {}
        }
    }
}

/// What the canvas paints for one highlight.
enum HighlightPaint {
    /// The export's own pixels, at their place on the image.
    Image(RectF, Arc<RenderImage>),
    /// A translucent fill, for a highlight still being dragged, which has no image yet.
    Fill(Rgba),
    /// Nothing: the highlight covers no pixel.
    Nothing,
}

const DRAGGED_HIGHLIGHT_ALPHA: f32 = 0.4;

/// What the canvas shows for each redaction: the pixels an export leaves there (F-36, D-58),
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
    /// the ones no longer there. A redaction still being dragged is not in the document, so it
    /// has none until release and paints as a solid fill (D-58).
    pub(crate) fn update(&mut self, document: &Document) {
        let mut kept = HashMap::new();
        for entry in document.shapes() {
            let Shape::Redact {
                rect,
                mode,
                strength,
            } = entry.shape
            else {
                continue;
            };
            let key = RedactionKey::new(rect, mode, strength);
            if kept.contains_key(&key) {
                continue;
            }
            let preview = self.previews.remove(&key).unwrap_or_else(|| {
                redaction_preview(document.base(), rect, mode, strength, &*self.log)
            });
            kept.insert(key, preview);
        }
        self.previews = kept;
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
            Some(RedactionPreview::Pixels { bounds, image }) => {
                let bounds = fit.bounds(*bounds);
                let painted =
                    window.paint_image(bounds, bounds, 0.0.into(), Arc::clone(image), 0, false);
                if painted.is_ok() {
                    return Some(bounds);
                }
                (self.log)("redaction_preview not_painted");
                hide(window)
            }
            Some(RedactionPreview::CoversNoPixel) => None,
            Some(RedactionPreview::Failed) | None => hide(window),
        }
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "image sides are far below 2^24, where f32 stops being exact"
)]
fn placed(patch: &Patch) -> (RectF, Arc<RenderImage>) {
    let bounds = RectF {
        x: patch.left as f32,
        y: patch.top as f32,
        width: patch.pixels.width() as f32,
        height: patch.pixels.height() as f32,
    };
    (bounds, render_image(&patch.pixels))
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

/// What the canvas shows under each shadowed shape whose shadow GPUI cannot draw as a box: the
/// shadow the export draws there (D-114), made once for each shape rather than every frame.
#[derive(Clone)]
pub(super) struct ShadowPreviews {
    previews: HashMap<ShapeId, ShadowPreview>,
    log: Log,
}

#[derive(Clone)]
struct ShadowPreview {
    of: ShapeEntry,
    image: Option<(RectF, Arc<RenderImage>)>,
}

impl ShadowPreviews {
    pub(crate) fn new(log: Log) -> Self {
        Self {
            previews: HashMap::new(),
            log,
        }
    }

    /// Keeps a preview for each such shape in `document`, remaking it when the shape changed and
    /// dropping the ones no longer there. A shape being dragged is not in the document, so its
    /// shadow shows again on release.
    pub(crate) fn update(&mut self, document: &Document) {
        let (width, height) = (document.base().width(), document.base().height());
        let mut kept = HashMap::new();
        for entry in document.shapes() {
            if !entry.style.shadow() || box_shadow(&entry.shape).is_some() {
                continue;
            }
            let preview = match self.previews.remove(&entry.id) {
                Some(preview) if preview.of == *entry => preview,
                Some(_) | None => ShadowPreview {
                    of: entry.clone(),
                    image: shadow_image(entry, width, height, &*self.log),
                },
            };
            kept.insert(entry.id, preview);
        }
        self.previews = kept;
    }

    fn paint(&self, id: ShapeId, shape: &Shape, style: Style, fit: &Fit, window: &mut Window) {
        let Some(ShadowPreview {
            of,
            image: Some((bounds, image)),
        }) = self.previews.get(&id)
        else {
            return;
        };
        if of.shape != *shape || of.style != style {
            return;
        }
        let bounds = fit.bounds(*bounds);
        let painted = window.paint_image(bounds, bounds, 0.0.into(), Arc::clone(image), 0, false);
        if painted.is_err() {
            (self.log)("shadow_preview not_painted");
        }
    }
}

fn shadow_image(
    entry: &ShapeEntry,
    width: u32,
    height: u32,
    log: &dyn Fn(&'static str),
) -> Option<(RectF, Arc<RenderImage>)> {
    match shape_shadow(entry, width, height) {
        Ok(Some(patch)) => Some(placed(&patch)),
        Ok(None) => None,
        Err(_) => {
            log("shadow_preview not_made");
            None
        }
    }
}

/// The box and corner radius, in image pixels, that cast `shape`'s shadow when GPUI can draw it
/// as a box shadow: a solid rectangle, a numbered step's circle, a speech bubble's box.
fn box_shadow(shape: &Shape) -> Option<(RectF, f32)> {
    match shape {
        Shape::Rect {
            rect,
            fill: Fill::Solid,
        } => Some((*rect, 0.0)),
        Shape::Step(step) => {
            let circle = step.circle();
            Some((circle, circle.width / 2.0))
        }
        Shape::Bubble(bubble) => Some((bubble.outline(), bubble.corner_radius())),
        Shape::Rect {
            fill: Fill::Hollow, ..
        }
        | Shape::Line { .. }
        | Shape::Arrow { .. }
        | Shape::Ellipse { .. }
        | Shape::Pen(_)
        | Shape::Text(_)
        | Shape::Highlight { .. }
        | Shape::Redact { .. }
        | Shape::Spotlight(_) => None,
    }
}

/// Paints the shadow `shape` casts under it (D-114). Bare text's shadow is its glyphs moved
/// down, unblurred, where the export blurs it: a known gap, recorded in D-114.
#[expect(
    clippy::cast_precision_loss,
    reason = "the shadow's sizes are a few pixels, exact in f32"
)]
pub(super) fn paint_shadow(
    id: Option<ShapeId>,
    shape: &Shape,
    style: Style,
    painting: &Painting,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), Unpainted> {
    if !style.shadow() {
        return Ok(());
    }
    let fit = painting.fit;
    let offset = Style::SHADOW_OFFSET as f32;
    if let Some((rect, radius)) = box_shadow(shape) {
        window.paint_drop_shadows(
            fit.bounds(rect),
            Corners::all(px(radius * fit.scale)),
            &[BoxShadow {
                color: hsla(0.0, 0.0, 0.0, Style::SHADOW_ALPHA),
                offset: point(px(0.0), px(offset * fit.scale)),
                blur_radius: px(Style::SHADOW_BLUR as f32 * fit.scale),
                spread_radius: px(0.0),
                inset: false,
            }],
        );
        return Ok(());
    }
    if let Shape::Text(block) = shape {
        let below = Fit {
            origin: fit.origin + point(px(0.0), px(offset * fit.scale)),
            ..*fit
        };
        let black = Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: Style::SHADOW_ALPHA,
        };
        return paint_label(&Label::Block(block), black, &below, window, cx);
    }
    if let Some(id) = id {
        painting.shadows.paint(id, shape, style, fit, window);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cleanshot_core::{Command, History, PointF};

    fn image_of(previews: &RedactionPreviews, rect: RectF, mode: RedactMode) -> Arc<RenderImage> {
        match previews
            .previews
            .get(&RedactionKey::new(rect, mode, RedactStrength::Fine))
        {
            Some(RedactionPreview::Pixels { image, .. }) => Arc::clone(image),
            Some(RedactionPreview::CoversNoPixel | RedactionPreview::Failed) | None => {
                panic!("a preview of {rect:?}")
            }
        }
    }

    #[test]
    fn f36_a_redaction_preview_is_made_once_and_dropped_with_its_redaction() {
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
                strength: RedactStrength::Fine,
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
    fn d114_a_shadow_image_is_made_for_an_arrow_or_hollow_box_kept_until_it_changes_and_not_for_a_solid_box()
     {
        use cleanshot_core::{Look, Property, Tool, moved};

        let image = RgbaImage::new(64, 48, vec![200; 64 * 48 * 4]).unwrap();
        let shadowed = |shape: Shape, fill| {
            let (shape, style) = Look::default_for(Tool::drawing(&shape))
                .with(Property::Fill(fill))
                .and_then(|look| look.with(Property::Shadow(true)))
                .unwrap()
                .dress(shape);
            Command::Add { shape, style }
        };
        let arrow = Shape::Arrow {
            from: PointF { x: 4.0, y: 4.0 },
            to: PointF { x: 40.0, y: 30.0 },
        };
        let rect = |fill| Shape::Rect {
            rect: RectF {
                x: 10.0,
                y: 10.0,
                width: 20.0,
                height: 10.0,
            },
            fill,
        };
        let mut history = History::new(Document::new(Arc::new(image)));
        history.apply(shadowed(arrow, Fill::Hollow)).unwrap();
        history
            .apply(shadowed(rect(Fill::Solid), Fill::Solid))
            .unwrap();
        history
            .apply(shadowed(rect(Fill::Hollow), Fill::Hollow))
            .unwrap();
        let image_of = |previews: &ShadowPreviews, id: ShapeId| match &previews.previews[&id].image
        {
            Some((_, image)) => Arc::clone(image),
            None => panic!("a shadow image for {id:?}"),
        };
        let mut previews = ShadowPreviews::new(Rc::new(|_| {}));
        previews.update(history.document());
        let ids: Vec<_> = history
            .document()
            .shapes()
            .iter()
            .map(|entry| entry.id)
            .collect();
        let arrow_id = ids[0];
        assert!(
            !previews.previews.contains_key(&ids[1]),
            "the solid box's shadow is GPUI's"
        );
        image_of(&previews, ids[2]);
        let first = image_of(&previews, arrow_id);
        let pen = Command::Add {
            shape: Shape::Pen(vec![PointF { x: 1.0, y: 1.0 }]),
            style: Style::new(history.document().shapes()[0].style.color, 2.0),
        };
        history.apply(pen).unwrap();
        previews.update(history.document());
        assert!(Arc::ptr_eq(&first, &image_of(&previews, arrow_id)));
        let entry = history.document().shapes()[0].clone();
        history
            .apply(Command::Replace {
                id: arrow_id,
                shape: moved(&entry.shape, PointF { x: 5.0, y: 0.0 }),
                style: entry.style,
            })
            .unwrap();
        previews.update(history.document());
        assert!(!Arc::ptr_eq(&first, &image_of(&previews, arrow_id)));
    }

    #[test]
    fn d125_a_highlight_preview_is_made_once_kept_until_it_changes_and_dropped_after() {
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
    fn d125_a_drawn_highlight_paints_the_exported_pixels_and_a_dragged_one_a_translucent_fill() {
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
        let dragged = RectF { x: 1.0, ..rect };
        let HighlightPaint::Fill(fill) =
            previews.painted(dragged, yellow, HighlightStrength::Strong)
        else {
            panic!("a dragged highlight paints a fill");
        };
        assert_eq!(fill.a, DRAGGED_HIGHLIGHT_ALPHA);
    }
}
