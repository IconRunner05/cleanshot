use std::collections::HashMap;
use std::f32::consts::TAU;
use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{
    Bubble, Color, Command, Document, Fill, Frame, HighlightStrength, Label, PointF, RectF,
    RedactMode, RedactStrength, RgbaImage, SPOTLIGHT_DIM, Shape, ShapeEntry, ShapeId, Style,
    areas_outside, arrow_head, box_handles, dimmed_areas, handles, step_numbers, text_on,
};
use cleanshot_render::{Patch, highlighted, redacted, shape_shadow};
use gpui::{
    App, BorderStyle, Bounds, BoxShadow, Canvas, Corners, DispatchPhase, Hitbox, HitboxBehavior,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point,
    RenderImage, Rgba, ScrollDelta, ScrollEvent, Window, canvas, fill, hsla, point, px, quad, size,
    transparent_black,
};
use smallvec::smallvec;

use crate::Log;
use crate::placement::{Fit, Placement};
use crate::text::paint_label;
use crate::theme::ACCENT;

const ELLIPSE_SEGMENTS: u16 = 64;

/// The base image in the BGRA layout GPUI uploads, made once per window.
pub(crate) fn render_image(image: &RgbaImage) -> Arc<RenderImage> {
    let mut buffer = image::RgbaImage::new(image.width(), image.height());
    let bgra_pixels = buffer.as_chunks_mut::<4>().0.iter_mut();
    for (bgra, rgba) in bgra_pixels.zip(image.pixels().as_chunks::<4>().0) {
        *bgra = [rgba[2], rgba[1], rgba[0], rgba[3]];
    }
    Arc::new(RenderImage::new(smallvec![image::Frame::new(buffer)]))
}

/// What the canvas shows as the export draws it, made once rather than every frame (D-58,
/// D-114, D-125), and the log the canvas reports what it could not paint to (D-120).
#[derive(Clone)]
pub(crate) struct Previews {
    redactions: RedactionPreviews,
    shadows: ShadowPreviews,
    highlights: HighlightPreviews,
    log: Log,
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
    fn paint_highlights<'a>(
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
struct RedactionPreviews {
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
    fn paint(
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
struct ShadowPreviews {
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
fn paint_shadow(
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

/// The pointer on the canvas, in image pixels, clamped to the image.
#[derive(Clone, Copy)]
pub(crate) enum Pointer {
    /// The left button went down over the canvas. `reach` is how far from `at`, in image
    /// pixels, a press still grips a shape or handle.
    /// `position` is on the canvas, and `placement` is where the image was when it was pressed.
    Press {
        at: PointF,
        reach: f32,
        position: Point<Pixels>,
        placement: Placement,
    },
    Move {
        at: PointF,
        shift: bool,
        position: Point<Pixels>,
    },
    Release {
        shift: bool,
    },
    /// The right button went down over the canvas at `at`, `position` in the window (D-115).
    Menu {
        at: PointF,
        reach: f32,
        position: Point<Pixels>,
    },
    /// Ctrl+wheel over the canvas: scale `placement` by `factor` about `around` (F-39).
    Zoom {
        factor: f32,
        around: Point<Pixels>,
        placement: Placement,
    },
}

pub(crate) type OnPointer = Rc<dyn Fn(Pointer, &mut Window, &mut App)>;

pub(crate) type PaintOver = Box<dyn FnOnce(&Fit, &mut Window, &mut App)>;

/// What is drawn over the document: the edit a drag or the text box would make, the selected
/// shape's handles, the crop's handles while `crop_handles` (D-118), and last `over`, given
/// where the image is (the text box's caret and input).
pub(crate) struct Overlay {
    pub preview: Option<Command>,
    pub selected: Option<ShapeId>,
    pub crop_handles: bool,
    pub over: Option<PaintOver>,
}

/// How far from a shape or handle, in screen pixels, a press still grips it.
const POINTER_REACH: f32 = 6.0;
const HANDLE_SIZE: f32 = 8.0;
/// D-46: each wheel line zooms by a fifth; a touchpad reports pixels, read as lines of 20.
const ZOOM_PER_WHEEL_LINE: f32 = 1.2;
const PIXELS_PER_WHEEL_LINE: f32 = 20.0;

/// Draws the document's base image scaled to fit the element, its shapes with `overlay`'s
/// preview on top, and reports the pointer to `on_pointer`, following a drag outside the element.
/// `document`'s shapes in drawing order as the canvas shows them: the one `preview` changes in
/// its changed form, and the one it adds last, without an id.
fn shown_shapes<'a>(
    document: &'a Document,
    preview: Option<&'a Command>,
) -> Vec<(Option<ShapeId>, &'a Shape, &'a Style)> {
    let (replaced, added) = match preview {
        Some(Command::Replace { id, shape, style }) => (Some((*id, shape, style)), None),
        Some(Command::Add { shape, style }) => (None, Some((shape, style))),
        Some(
            Command::Remove(_)
            | Command::Restack { .. }
            | Command::SetCrop(_)
            | Command::SetFrame(_),
        )
        | None => (None, None),
    };
    document
        .shapes()
        .iter()
        .map(|entry| match replaced {
            Some((id, shape, style)) if id == entry.id => (Some(entry.id), shape, style),
            Some(_) | None => (Some(entry.id), &entry.shape, &entry.style),
        })
        .chain(added.map(|(shape, style)| (None, shape, style)))
        .collect()
}

pub(crate) fn document_canvas(
    document: Rc<Document>,
    base: Arc<RenderImage>,
    placement: Option<Placement>,
    previews: Rc<Previews>,
    overlay: Overlay,
    on_pointer: OnPointer,
) -> Canvas<Hitbox> {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, cx| {
            let image = document.base();
            let scale_factor = window.scale_factor();
            // D-118: while the crop is being changed, the view shows what it can grow into.
            let shown = if overlay.crop_handles {
                document.image_area()
            } else {
                document.kept_area()
            };
            let placement =
                placement.unwrap_or_else(|| Placement::fitted(bounds.size, shown, scale_factor));
            let fit = Fit::new(bounds, image, placement, scale_factor);
            paint_frame(document.frame(), fit.image_bounds, fit.scale, window);
            let painted = window.paint_image(
                fit.image_bounds,
                fit.image_bounds,
                0.0.into(),
                base,
                0,
                false,
            );
            if painted.is_err() {
                (previews.log)("base_image not_painted");
            }
            let shown = shown_shapes(&document, overlay.preview.as_ref());
            previews.paint_highlights(
                shown.iter().map(|(_, shape, style)| (*shape, *style)),
                &fit,
                window,
            );
            let dimmed = dimmed_areas(image, shown.iter().map(|(_, shape, _)| *shape));
            paint_dim(&dimmed, fit.image_bounds, &fit, window);
            let mut selected_shape = None;
            let numbers =
                step_numbers(shown.iter().map(|(_, shape, _)| *shape)).collect::<Vec<_>>();
            for ((id, shape, style), number) in shown.into_iter().zip(numbers) {
                let painting = Painting {
                    fit: &fit,
                    redactions: &previews.redactions,
                    shadows: &previews.shadows,
                    dimmed: &dimmed,
                    number,
                };
                let shadow = paint_shadow(id, shape, *style, &painting, window, cx);
                let painted = paint_shape(shape, *style, &painting, window, cx);
                for unpainted in [shadow, painted].into_iter().filter_map(Result::err) {
                    (previews.log)(unpainted.event());
                }
                if id.is_some() && overlay.selected == id {
                    selected_shape = Some(shape);
                }
            }
            if let Some(crop) = document.crop() {
                paint_dim(
                    &areas_outside(image, &[crop]),
                    fit.image_bounds,
                    &fit,
                    window,
                );
            }
            let new_crop = if let Some(Command::SetCrop(Some(crop))) = &overlay.preview {
                paint_outline(fit.bounds(*crop), window);
                Some(*crop)
            } else {
                None
            };
            if overlay.crop_handles
                && let Some(crop) = new_crop.or(document.crop())
            {
                for (_, at) in box_handles(crop) {
                    paint_handle(fit.point(at), window);
                }
            }
            if let Some(shape) = selected_shape {
                for (_, at) in handles(shape) {
                    paint_handle(fit.point(at), window);
                }
            }
            if let Some(over) = overlay.over {
                over(&fit, window, cx);
            }
            listen_for_pointer(&fit, placement, bounds.origin, hitbox, &on_pointer, window);
        },
    )
}

fn listen_for_pointer(
    fit: &Fit,
    placement: Placement,
    canvas_origin: Point<Pixels>,
    hitbox: Hitbox,
    on_pointer: &OnPointer,
    window: &mut Window,
) {
    let fit = *fit;
    let image_point = move |position| fit.image_point(position);
    let reach = POINTER_REACH / fit.scale;
    let zoom_hitbox = hitbox.clone();
    let on_press = Rc::clone(on_pointer);
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble
            && event.button == MouseButton::Left
            && hitbox.is_hovered(window)
        {
            let at = image_point(event.position);
            let position = event.position - canvas_origin;
            let press = Pointer::Press {
                at,
                reach,
                position,
                placement,
            };
            on_press(press, window, cx);
        }
    });
    let on_menu = Rc::clone(on_pointer);
    let menu_hitbox = zoom_hitbox.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble
            && event.button == MouseButton::Right
            && menu_hitbox.is_hovered(window)
        {
            let menu = Pointer::Menu {
                at: image_point(event.position),
                reach,
                position: event.position,
            };
            on_menu(menu, window, cx);
        }
    });
    let on_move = Rc::clone(on_pointer);
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && event.pressed_button == Some(MouseButton::Left) {
            let at = image_point(event.position);
            let shift = event.modifiers.shift;
            let position = event.position - canvas_origin;
            on_move(
                Pointer::Move {
                    at,
                    shift,
                    position,
                },
                window,
                cx,
            );
        }
    });
    let on_release = Rc::clone(on_pointer);
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
            let shift = event.modifiers.shift;
            on_release(Pointer::Release { shift }, window, cx);
        }
    });
    let on_zoom = Rc::clone(on_pointer);
    window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble
            && event.modifiers.control
            && zoom_hitbox.is_hovered(window)
        {
            let lines = match event.delta {
                ScrollDelta::Lines(lines) => lines.y,
                ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / PIXELS_PER_WHEEL_LINE,
            };
            let zoom = Pointer::Zoom {
                factor: ZOOM_PER_WHEEL_LINE.powf(lines),
                around: event.position - canvas_origin,
                placement,
            };
            on_zoom(zoom, window, cx);
        }
    });
}

/// F-44: the frame the export draws around the image, at the image's scale, so the canvas shows
/// how the export will look. It frames the whole image, as the canvas shows it uncropped.
#[expect(
    clippy::cast_precision_loss,
    reason = "the frame's sizes are a few pixels, exact in f32"
)]
fn paint_frame(frame: Frame, image: Bounds<Pixels>, scale: f32, window: &mut Window) {
    let border = if frame.border { scale } else { 0.0 };
    let block = image.dilate(px(border));
    if frame.shadow {
        window.paint_drop_shadows(
            block,
            Corners::default(),
            &[BoxShadow {
                color: hsla(0.0, 0.0, 0.0, Frame::SHADOW_ALPHA),
                offset: point(px(0.0), px(Frame::SHADOW_OFFSET as f32 * scale)),
                blur_radius: px(Frame::SHADOW_BLUR as f32 * scale),
                spread_radius: px(0.0),
                inset: false,
            }],
        );
    }
    if frame.border {
        window.paint_quad(quad(
            block,
            px(0.0),
            transparent_black(),
            px(border),
            hsla(0.0, 0.0, 0.0, f32::from(Frame::BORDER_ALPHA) / 255.0),
            BorderStyle::Solid,
        ));
    }
}

fn paint_outline(bounds: Bounds<Pixels>, window: &mut Window) {
    window.paint_quad(quad(
        bounds,
        px(0.0),
        transparent_black(),
        px(1.0),
        ACCENT,
        BorderStyle::Dashed,
    ));
}

fn paint_handle(centre: Point<Pixels>, window: &mut Window) {
    let side = px(HANDLE_SIZE);
    let bounds = Bounds::centered_at(centre, size(side, side));
    window.paint_quad(quad(
        bounds,
        px(2.0),
        gpui::white(),
        px(1.0),
        ACCENT,
        BorderStyle::Solid,
    ));
}

/// What a shape is painted against: the fit, the redaction previews, the spotlights' dimmed
/// areas, and the number a step at this place in the drawing order shows.
struct Painting<'a> {
    fit: &'a Fit,
    redactions: &'a RedactionPreviews,
    shadows: &'a ShadowPreviews,
    dimmed: &'a [RectF],
    number: usize,
}

fn paint_shape(
    shape: &Shape,
    style: Style,
    painting: &Painting,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), Unpainted> {
    let Painting {
        fit,
        redactions,
        shadows: _,
        dimmed,
        number,
    } = *painting;
    let color = rgba(style.color);
    let width = px(style.stroke_width * fit.scale);
    let stroke = PathBuilder::stroke(width);
    match shape {
        Shape::Line { from, to } => paint_polygon(
            window,
            stroke,
            &[fit.point(*from), fit.point(*to)],
            false,
            color,
        ),
        Shape::Arrow { from, to } => {
            let line = paint_polygon(
                window,
                PathBuilder::stroke(width),
                &[fit.point(*from), fit.point(*to)],
                false,
                color,
            );
            let head = arrow_head(*from, *to, style.stroke_width).map(|p| fit.point(p));
            line.and(paint_polygon(window, stroke, &head, false, color))
        }
        Shape::Rect {
            rect,
            fill: Fill::Hollow,
        } => {
            window.paint_quad(quad(
                fit.bounds(*rect),
                px(0.0),
                transparent_black(),
                width,
                color,
                BorderStyle::Solid,
            ));
            Ok(())
        }
        Shape::Rect {
            rect,
            fill: Fill::Solid,
        } => {
            window.paint_quad(fill(fit.bounds(*rect), color));
            Ok(())
        }
        Shape::Ellipse {
            rect,
            fill: Fill::Hollow,
        } => paint_polygon(window, stroke, &ellipse(fit.bounds(*rect)), true, color),
        Shape::Ellipse {
            rect,
            fill: Fill::Solid,
        } => paint_polygon(
            window,
            PathBuilder::fill(),
            &ellipse(fit.bounds(*rect)),
            true,
            color,
        ),
        Shape::Pen(points) => {
            let points: Vec<_> = points.iter().map(|p| fit.point(*p)).collect();
            paint_polygon(window, stroke, &points, false, color)
        }
        Shape::Text(block) => paint_label(&Label::Block(block), color, fit, window, cx),
        Shape::Redact {
            rect,
            mode,
            strength,
        } => {
            // D-78: a redaction is the capture with its pixels destroyed, so it dims like the capture.
            if let Some(painted) = redactions.paint(*rect, *mode, *strength, fit, window) {
                paint_dim(dimmed, painted, fit, window);
            }
            Ok(())
        }
        // F-43, D-125: the canvas dims around every spotlight, and paints every highlight, before
        // it paints any shape.
        Shape::Spotlight(_) | Shape::Highlight { .. } => Ok(()),
        Shape::Step(step) => {
            let circle = paint_polygon(
                window,
                PathBuilder::fill(),
                &ellipse(fit.bounds(step.circle())),
                true,
                color,
            );
            circle.and(paint_label(
                &step.label(number),
                rgba(text_on(style.color)),
                fit,
                window,
                cx,
            ))
        }
        Shape::Bubble(bubble) => paint_bubble(bubble, style, fit, window, cx),
    }
}

/// F-42: the rounded box and tail in the style colour, the text on them in the colour that reads.
fn paint_bubble(
    bubble: &Bubble,
    style: Style,
    fit: &Fit,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), Unpainted> {
    let color = rgba(style.color);
    window.paint_quad(quad(
        fit.bounds(bubble.outline()),
        px(bubble.corner_radius() * fit.scale),
        color,
        px(0.0),
        transparent_black(),
        BorderStyle::Solid,
    ));
    let tail = bubble.tail_triangle().map_or(Ok(()), |tail| {
        let tail = tail.map(|point| fit.point(point));
        paint_polygon(window, PathBuilder::fill(), &tail, true, color)
    });
    let text = paint_label(
        &Label::Block(bubble.block()),
        rgba(text_on(style.color)),
        fit,
        window,
        cx,
    );
    tail.and(text)
}

/// Lays the spotlight dim over each of `areas`, cut to `within` (F-43, D-78).
fn paint_dim(areas: &[RectF], within: Bounds<Pixels>, fit: &Fit, window: &mut Window) {
    for area in areas {
        let bounds = fit.bounds(*area).intersect(&within);
        if !bounds.is_empty() {
            window.paint_quad(fill(bounds, rgba(SPOTLIGHT_DIM)));
        }
    }
}

/// What the canvas could not paint this frame (D-120). The document keeps the shape, so the
/// export still has it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Unpainted {
    /// lyon rejected the path: it has no length or too many vertices.
    Path,
    /// A line of text could not be rasterised.
    Glyphs,
}

impl Unpainted {
    fn event(self) -> &'static str {
        match self {
            Self::Path => "path not_painted",
            Self::Glyphs => "glyphs not_painted",
        }
    }
}

fn paint_polygon(
    window: &mut Window,
    mut builder: PathBuilder,
    points: &[Point<Pixels>],
    closed: bool,
    color: Rgba,
) -> Result<(), Unpainted> {
    builder.add_polygon(points, closed);
    let path = builder.build().map_err(|_| Unpainted::Path)?;
    window.paint_path(path, color);
    Ok(())
}

fn ellipse(bounds: Bounds<Pixels>) -> Vec<Point<Pixels>> {
    let centre = bounds.center();
    let radius_x = f32::from(bounds.size.width) / 2.0;
    let radius_y = f32::from(bounds.size.height) / 2.0;
    (0..ELLIPSE_SEGMENTS)
        .map(|segment| {
            let angle = TAU * f32::from(segment) / f32::from(ELLIPSE_SEGMENTS);
            centre + point(px(radius_x * angle.cos()), px(radius_y * angle.sin()))
        })
        .collect()
}

pub(crate) fn rgba(color: Color) -> Rgba {
    Rgba {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: f32::from(color.a) / 255.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cleanshot_core::History;

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
