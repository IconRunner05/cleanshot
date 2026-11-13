use std::f32::consts::TAU;
use std::rc::Rc;
use std::sync::Arc;

use cleanshot_core::{
    Bubble, Color, Command, Document, Fill, Label, PointF, RectF, RgbaImage, Shape, ShapeId,
    SpotlightDim, Style, TextBlock, areas_outside, arrow_head, box_handles, dimmed_areas, handles,
    step_numbers, text_on,
};
use gpui::{
    App, BorderStyle, Bounds, Canvas, ContentMask, DispatchPhase, Hitbox, HitboxBehavior,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point,
    RenderImage, Rgba, ScrollDelta, ScrollEvent, Window, canvas, fill, point, px, quad, size,
    transparent_black,
};
use smallvec::smallvec;

use crate::placement::{Fit, Placement, fitted_placement, shown_area};
use crate::text::paint_label;
use crate::theme::Theme;

mod previews;
mod shape_patches;

pub(crate) use previews::Previews;
use previews::RedactionPreviews;
use shape_patches::{ShapePatches, paint_border, paint_shadow};

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

/// The pointer on the canvas, in image pixels, clamped to the image.
#[derive(Clone, Copy)]
pub(crate) enum Pointer {
    /// The left button went down over the canvas. `reach` is how far from `at`, in image
    /// pixels, a press still grips a shape or handle.
    /// `position` is on the canvas, and `placement` is where the image was when it was pressed.
    /// `double_click` is a second press within the Windows double-click time and distance.
    Press {
        at: PointF,
        reach: f32,
        position: Point<Pixels>,
        placement: Placement,
        double_click: bool,
    },
    Move {
        at: PointF,
        shift: bool,
        position: Point<Pixels>,
    },
    Release {
        shift: bool,
    },
    /// The right button went down over the canvas at `at`, `position` in the window.
    Menu {
        at: PointF,
        reach: f32,
        position: Point<Pixels>,
    },
    /// The wheel over the canvas without Ctrl: move `placement` by `by`, down or, with Shift
    /// or the tilt wheel, across.
    Scroll {
        by: Point<Pixels>,
        placement: Placement,
    },
    /// Ctrl+wheel over the canvas: scale `placement` by `factor` about `around`.
    Zoom {
        factor: f32,
        around: Point<Pixels>,
        placement: Placement,
    },
}

pub(crate) type OnPointer = Rc<dyn Fn(Pointer, &mut Window, &mut App)>;

pub(crate) type PaintOver = Box<dyn FnOnce(&Fit, &mut Window, &mut App)>;

/// What is drawn over the document: the edit a drag or the text box would make, the document
/// with that edit in it, how far that edit shifts its whole shape if that is all it does, the
/// selected shape's handles, the crop's handles while `crop_handles`, and last `over`, given where
/// the image is (the text box's caret and input).
pub(crate) struct Overlay {
    pub preview: Option<Command>,
    pub shown: Rc<Document>,
    pub moved_by: Option<PointF>,
    pub selected: Option<ShapeId>,
    pub crop_handles: bool,
    pub over: Option<PaintOver>,
}

/// How far from a shape or handle, in screen pixels, a press still grips it.
const POINTER_REACH: f32 = 6.0;
const HANDLE_SIZE: f32 = 8.0;
/// Each wheel line zooms by a fifth; a touchpad reports pixels, read as lines of 20.
const ZOOM_PER_WHEEL_LINE: f32 = 1.2;
const PIXELS_PER_WHEEL_LINE: f32 = 20.0;

/// Draws the document's base image scaled to fit the element, its shapes with `overlay`'s
/// preview on top, and reports the pointer to `on_pointer`, following a drag outside the element.
pub(crate) fn document_canvas(
    document: Rc<Document>,
    base: Arc<RenderImage>,
    placement: Option<Placement>,
    previews: Rc<Previews>,
    overlay: Overlay,
    theme: Theme,
    on_pointer: OnPointer,
) -> Canvas<Hitbox> {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, cx| {
            let image = document.base();
            let scale_factor = window.scale_factor();
            let shown = shown_area(&document, overlay.crop_handles);
            let placement = placement.unwrap_or_else(|| {
                fitted_placement(&document, overlay.crop_handles, bounds.size, scale_factor)
            });
            let fit = Fit::new(bounds, image, shown, placement, scale_factor);
            let shown_bounds = fit.bounds(shown);
            // Nothing a crop leaves out is drawn.
            let mask = ContentMask {
                bounds: shown_bounds,
            };
            let selected_shape = window.with_content_mask(Some(mask), |window| {
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
                let shapes = overlay.shown.shapes();
                previews.paint_highlights(
                    shapes.iter().map(|entry| (&entry.shape, &entry.style)),
                    &fit,
                    window,
                );
                let dimmed = dimmed_areas(image, shapes.iter().map(|entry| &entry.shape));
                let dim = document.dim().color();
                paint_dim(&dimmed, dim, fit.image_bounds, &fit, window);
                let mut selected_shape = None;
                let numbers =
                    step_numbers(shapes.iter().map(|entry| &entry.shape)).collect::<Vec<_>>();
                for (entry, number) in shapes.iter().zip(numbers) {
                    let (id, shape, style) = (entry.id, &entry.shape, &entry.style);
                    let painting = Painting {
                        fit: &fit,
                        redactions: &previews.redactions,
                        shadows: &previews.shadows,
                        borders: &previews.borders,
                        moved_by: overlay.moved_by,
                        dimmed: &dimmed,
                        dim,
                        number,
                    };
                    let shadow = paint_shadow(id, shape, *style, &painting, window, cx);
                    paint_border(id, shape, *style, &painting, window);
                    let painted = paint_shape(shape, *style, &painting, window, cx);
                    for unpainted in [shadow, painted].into_iter().filter_map(Result::err) {
                        (previews.log)(unpainted.event());
                    }
                    if overlay.selected == Some(id) {
                        selected_shape = Some(shape);
                    }
                }
                if overlay.crop_handles
                    && let Some(crop) = document.crop()
                {
                    paint_dim(
                        &areas_outside(image, &[crop]),
                        OUTSIDE_CROP,
                        fit.image_bounds,
                        &fit,
                        window,
                    );
                }
                selected_shape
            });
            let new_crop = if let Some(Command::SetCrop(Some(crop))) = &overlay.preview {
                paint_outline(fit.bounds(*crop), theme, window);
                Some(*crop)
            } else {
                None
            };
            if overlay.crop_handles
                && let Some(crop) = new_crop.or(document.crop())
            {
                for (_, at) in box_handles(crop) {
                    paint_handle(fit.point(at), theme, window);
                }
            }
            if let Some(shape) = selected_shape {
                for (_, at) in handles(shape) {
                    paint_handle(fit.point(at), theme, window);
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
                double_click: event.click_count > 1,
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
        if phase != DispatchPhase::Bubble || !zoom_hitbox.is_hovered(window) {
            return;
        }
        let by = match event.delta {
            ScrollDelta::Lines(lines) => point(
                px(lines.x * PIXELS_PER_WHEEL_LINE),
                px(lines.y * PIXELS_PER_WHEEL_LINE),
            ),
            ScrollDelta::Pixels(pixels) => pixels,
        };
        let wheeled = if event.modifiers.control {
            Pointer::Zoom {
                factor: ZOOM_PER_WHEEL_LINE.powf(f32::from(by.y) / PIXELS_PER_WHEEL_LINE),
                around: event.position - canvas_origin,
                placement,
            }
        } else {
            Pointer::Scroll { by, placement }
        };
        on_zoom(wheeled, window, cx);
    });
}

fn paint_outline(bounds: Bounds<Pixels>, theme: Theme, window: &mut Window) {
    window.paint_quad(quad(
        bounds,
        px(0.0),
        transparent_black(),
        px(1.0),
        theme.accent,
        BorderStyle::Dashed,
    ));
}

fn paint_handle(centre: Point<Pixels>, theme: Theme, window: &mut Window) {
    let side = px(HANDLE_SIZE);
    let bounds = Bounds::centered_at(centre, size(side, side));
    window.paint_quad(quad(
        bounds,
        px(2.0),
        theme.handle,
        px(1.0),
        theme.accent,
        BorderStyle::Solid,
    ));
}

/// What a shape is painted against: the fit, the redaction, shadow and border previews, how far
/// a drag shifts its shape, the spotlights' dimmed areas, and the number a step at this place in
/// the drawing order shows.
struct Painting<'a> {
    fit: &'a Fit,
    redactions: &'a RedactionPreviews,
    shadows: &'a ShapePatches,
    borders: &'a ShapePatches,
    moved_by: Option<PointF>,
    dimmed: &'a [RectF],
    dim: Color,
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
        dimmed,
        dim,
        number,
        ..
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
        Shape::Text(block) => paint_text(block, style, fit, window, cx),
        Shape::Redact {
            rect,
            mode,
            strength,
        } => {
            // A redaction is the capture with its pixels destroyed, so it dims like the capture.
            if let Some(painted) = redactions.paint(*rect, *mode, *strength, fit, window) {
                paint_dim(dimmed, dim, painted, fit, window);
            }
            Ok(())
        }
        // The canvas dims around every spotlight, and paints every highlight, before
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

/// The text in the style colour, on its background where it has one.
fn paint_text(
    block: &TextBlock,
    style: Style,
    fit: &Fit,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), Unpainted> {
    if let Some(background) = style.background() {
        window.paint_quad(fill(fit.bounds(block.padded_extent()), rgba(background)));
    }
    paint_label(&Label::Block(block), rgba(style.color), fit, window, cx)
}

/// The rounded box and tail in the style colour, the text on them in the colour that reads.
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

/// What the canvas lays over the image outside the crop while it is being changed.
const OUTSIDE_CROP: Color = SpotlightDim::Medium.color();

/// Lays the spotlight dim over each of `areas`, cut to `within`.
fn paint_dim(
    areas: &[RectF],
    color: Color,
    within: Bounds<Pixels>,
    fit: &Fit,
    window: &mut Window,
) {
    for area in areas {
        let bounds = fit.bounds(*area).intersect(&within);
        if !bounds.is_empty() {
            window.paint_quad(fill(bounds, rgba(color)));
        }
    }
}

/// What the canvas could not paint this frame. The document keeps the shape, so the
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
