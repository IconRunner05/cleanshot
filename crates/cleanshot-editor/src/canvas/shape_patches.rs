use std::cell::OnceCell;
use std::collections::HashMap;
use std::sync::Arc;

use cleanshot_core::{
    Document, Fill, Label, PointF, RectF, Shape, ShapeEntry, ShapeId, Style, TextMasks, moved,
};
use cleanshot_render::{shape_border, shape_shadow};
use gpui::{App, BoxShadow, Corners, RenderImage, Rgba, Window, hsla, point, px};

use super::previews::{paint_placed, placed};
use super::{Painting, Unpainted};
use crate::placement::Fit;
use crate::text::paint_label;
use crate::{Log, MasksFor};

/// What the canvas shows under each shape of a shadow or border GPUI cannot draw: the pixels
/// the export draws there, made once for each shape rather than every frame.
#[derive(Clone)]
pub(super) struct ShapePatches {
    kind: PatchKind,
    pub(super) previews: HashMap<ShapeId, PatchPreview>,
    log: Log,
}

/// Which of a shape's surroundings a [`ShapePatches`] shows.
#[derive(Clone)]
pub(super) enum PatchKind {
    /// The shadows GPUI cannot draw as a box shadow.
    Shadow,
    /// Every border, a text's grown from the glyph masks this gives.
    Border(MasksFor),
}

impl PatchKind {
    fn wanted(&self, entry: &ShapeEntry) -> bool {
        match self {
            Self::Shadow => entry.style.shadow() && box_shadow(&entry.shape).is_none(),
            Self::Border(_) => entry.style.border().is_some(),
        }
    }

    fn not_made(&self) -> &'static str {
        match self {
            Self::Shadow => "shadow_preview not_made",
            Self::Border(_) => "border_preview not_made",
        }
    }

    fn not_painted(&self) -> &'static str {
        match self {
            Self::Shadow => "shadow_preview not_painted",
            Self::Border(_) => "border_preview not_painted",
        }
    }
}

#[derive(Clone)]
pub(super) struct PatchPreview {
    pub(super) of: ShapeEntry,
    image: Option<(RectF, Arc<RenderImage>)>,
}

impl PatchPreview {
    /// Where this image goes and the image, if it shows `shape` in `style`: where it was made
    /// for its own shape, or shifted by `moved_by` for that shape dragged that far.
    fn placed_for(
        &self,
        shape: &Shape,
        style: Style,
        moved_by: Option<PointF>,
    ) -> Option<(RectF, &Arc<RenderImage>)> {
        let (bounds, image) = self.image.as_ref()?;
        if self.of.style != style {
            return None;
        }
        if self.of.shape == *shape {
            return Some((*bounds, image));
        }
        let by = moved_by.filter(|by| moved(&self.of.shape, *by) == *shape)?;
        let area = RectF {
            x: bounds.x + by.x,
            y: bounds.y + by.y,
            ..*bounds
        };
        Some((area, image))
    }
}

impl ShapePatches {
    pub(crate) fn new(kind: PatchKind, log: Log) -> Self {
        Self {
            kind,
            previews: HashMap::new(),
            log,
        }
    }

    /// Keeps a preview for each such shape in `document`, remaking it when the shape changed and
    /// dropping the ones no longer there. Text masks are made at most once a call, and
    /// only when a bordered text is remade. Returns the images of the previews dropped or remade.
    pub(crate) fn update(&mut self, document: &Document) -> Vec<Arc<RenderImage>> {
        let masks = OnceCell::new();
        let mut kept = HashMap::new();
        for entry in document.shapes() {
            if !self.kind.wanted(entry) {
                continue;
            }
            let preview = match self.previews.remove(&entry.id) {
                Some(preview) if preview.of == *entry => preview,
                Some(_) | None => PatchPreview {
                    of: entry.clone(),
                    image: self.image(entry, document, &masks),
                },
            };
            kept.insert(entry.id, preview);
        }
        std::mem::replace(&mut self.previews, kept)
            .into_values()
            .filter_map(|preview| preview.image.map(|(_, image)| image))
            .collect()
    }

    fn paint(
        &self,
        id: ShapeId,
        shape: &Shape,
        style: Style,
        painting: &Painting,
        window: &mut Window,
    ) {
        let Some((area, image)) = self
            .previews
            .get(&id)
            .and_then(|preview| preview.placed_for(shape, style, painting.moved_by))
        else {
            return;
        };
        paint_placed(
            area,
            image,
            painting.fit,
            window,
            &self.log,
            self.kind.not_painted(),
        );
    }

    /// `entry`'s image, with `masks` made for `document` the first time a text needs them. A
    /// text whose masks could not be made shows without its border; the maker logged why.
    fn image(
        &self,
        entry: &ShapeEntry,
        document: &Document,
        masks: &OnceCell<Option<TextMasks>>,
    ) -> Option<(RectF, Arc<RenderImage>)> {
        let (width, height) = (document.base().width(), document.base().height());
        let made = match &self.kind {
            PatchKind::Shadow => shape_shadow(entry, width, height),
            PatchKind::Border(masks_for) => {
                let text_masks = if let Shape::Text(_) = entry.shape {
                    let Some(text_masks) = masks.get_or_init(|| masks_for(document)) else {
                        (self.log)("border_preview no_text_masks");
                        return None;
                    };
                    Some(text_masks)
                } else {
                    None
                };
                shape_border(entry, width, height, text_masks)
            }
        };
        match made {
            Ok(Some(patch)) => Some(placed(&patch)),
            Ok(None) => None,
            Err(_) => {
                (self.log)(self.kind.not_made());
                None
            }
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

/// Paints the shadow `shape` casts under it. Bare text's shadow is its glyphs moved
/// down, unblurred, where the export blurs it: a known gap.
#[expect(
    clippy::cast_precision_loss,
    reason = "the shadow's sizes are a few pixels, exact in f32"
)]
pub(super) fn paint_shadow(
    id: ShapeId,
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
    painting.shadows.paint(id, shape, style, painting, window);
    Ok(())
}

/// Paints `shape`'s border under it as the export draws it, in one image.
pub(super) fn paint_border(
    id: ShapeId,
    shape: &Shape,
    style: Style,
    painting: &Painting,
    window: &mut Window,
) {
    if style.border().is_some() {
        painting.borders.paint(id, shape, style, painting, window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::render_image;
    use cleanshot_core::{Color, Command, History, RgbaImage};
    use std::rc::Rc;

    #[test]
    fn a_patch_shows_for_its_shape_and_for_that_shape_moved_by_the_drag_and_nothing_else() {
        let base = RgbaImage::new(64, 48, vec![200; 64 * 48 * 4]).unwrap();
        let shape = Shape::Rect {
            rect: RectF {
                x: 10.0,
                y: 10.0,
                width: 20.0,
                height: 10.0,
            },
            fill: Fill::Hollow,
        };
        let style = Style::new(
            Color {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            2.0,
        );
        let mut history = History::new(Document::new(Arc::new(base)));
        history
            .apply(Command::Add {
                shape: shape.clone(),
                style,
            })
            .unwrap();
        let bounds = RectF {
            x: 8.0,
            y: 8.0,
            width: 24.0,
            height: 14.0,
        };
        let preview = PatchPreview {
            of: history.document().shapes()[0].clone(),
            image: Some((
                bounds,
                render_image(&RgbaImage::new(1, 1, vec![0; 4]).unwrap()),
            )),
        };
        let by = PointF { x: 5.0, y: -2.0 };
        let shifted = moved(&shape, by);
        let area = |shape: &Shape, moved_by| {
            preview
                .placed_for(shape, style, moved_by)
                .map(|(area, _)| area)
        };
        assert_eq!(area(&shape, None), Some(bounds));
        assert_eq!(area(&shape, Some(by)), Some(bounds));
        assert_eq!(
            area(&shifted, Some(by)),
            Some(RectF {
                x: 13.0,
                y: 6.0,
                ..bounds
            })
        );
        assert_eq!(area(&shifted, None), None, "a resize has no patch yet");
        let elsewhere = PointF { x: 1.0, y: 1.0 };
        assert_eq!(area(&shifted, Some(elsewhere)), None);
    }

    #[test]
    fn a_shadow_image_is_made_for_an_arrow_or_hollow_box_kept_until_it_changes_and_not_for_a_solid_box()
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
        let image_of = |previews: &ShapePatches, id: ShapeId| match &previews.previews[&id].image {
            Some((_, image)) => Arc::clone(image),
            None => panic!("a shadow image for {id:?}"),
        };
        let mut previews = ShapePatches::new(PatchKind::Shadow, Rc::new(|_| {}));
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
    fn a_border_image_is_made_once_per_shape_a_texts_from_its_masks_and_none_if_they_fail() {
        use std::cell::{Cell, RefCell};

        use cleanshot_core::{GlyphMask, Look, Property, TextBlock, Tool};

        let image = RgbaImage::new(64, 48, vec![200; 64 * 48 * 4]).unwrap();
        let bordered = |shape: Shape, fill| {
            let (shape, style) = Look::default_for(Tool::drawing(&shape))
                .with(Property::Fill(fill))
                .and_then(|look| look.with(Property::Border(true)))
                .unwrap()
                .dress(shape);
            Command::Add { shape, style }
        };
        let arrow = Shape::Arrow {
            from: PointF { x: 4.0, y: 4.0 },
            to: PointF { x: 40.0, y: 30.0 },
        };
        let solid = Shape::Rect {
            rect: RectF {
                x: 10.0,
                y: 10.0,
                width: 20.0,
                height: 10.0,
            },
            fill: Fill::Solid,
        };
        let text = TextBlock::new(PointF { x: 2.0, y: 2.0 }, "hi".into(), 12.0, 1.0).unwrap();
        let mut history = History::new(Document::new(Arc::new(image)));
        history.apply(bordered(arrow, Fill::Hollow)).unwrap();
        history.apply(bordered(solid, Fill::Solid)).unwrap();
        history
            .apply(bordered(Shape::Text(text), Fill::Hollow))
            .unwrap();
        let document = history.document();
        let ids: Vec<_> = document.shapes().iter().map(|entry| entry.id).collect();
        let calls = Rc::new(Cell::new(0));
        let counted = Rc::clone(&calls);
        let masks: MasksFor = Rc::new(move |document: &Document| {
            counted.set(counted.get() + 1);
            let mut masks = TextMasks::default();
            for entry in document.shapes() {
                if let Shape::Text(_) = entry.shape {
                    let glyph = GlyphMask::new(4, 4, 6, 6, vec![255; 36], None).unwrap();
                    masks.insert(entry.id, vec![glyph]);
                }
            }
            Some(masks)
        });
        let mut previews = ShapePatches::new(PatchKind::Border(masks), Rc::new(|_| {}));
        previews.update(document);
        for id in &ids {
            assert!(
                previews.previews[id].image.is_some(),
                "a border image for {id:?}"
            );
        }
        assert_eq!(calls.get(), 1, "the text's masks are made once");
        previews.update(document);
        assert_eq!(calls.get(), 1, "an unchanged text keeps its border image");
        let logged = Rc::new(RefCell::new(Vec::new()));
        let log = Rc::clone(&logged);
        let mut failing = ShapePatches::new(
            PatchKind::Border(Rc::new(|_| None)),
            Rc::new(move |event| log.borrow_mut().push(event)),
        );
        failing.update(document);
        assert!(failing.previews[&ids[0]].image.is_some());
        assert!(failing.previews[&ids[2]].image.is_none());
        assert_eq!(*logged.borrow(), ["border_preview no_text_masks"]);
    }
}
