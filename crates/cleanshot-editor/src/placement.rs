use cleanshot_core::{Document, Frame, PointF, RectF, RgbaImage};
use gpui::{Bounds, Pixels, Point, Size, point, px, size};

/// Zoom stops between these scales, in screen pixels per image pixel (D-46).
const MIN_SCALE: f32 = 0.05;
const MAX_SCALE: f32 = 32.0;
/// The editor's own caption row above the image's card: tools, the property strip, and the
/// window's caption buttons (D-141).
pub(crate) const CAPTION_ROW: Pixels = px(48.0);
/// The window showing round the image's card on its left, right and bottom (D-141).
pub(crate) const CARD_INSET: Pixels = px(12.0);
/// The smallest the editor window can be made.
pub(crate) const MIN_WINDOW: Size<Pixels> = Size {
    width: px(480.0),
    height: px(320.0),
};

/// The canvas inside the image's card in an editor window whose client area is `client`.
pub(crate) fn canvas_size(client: Size<Pixels>) -> Size<Pixels> {
    size(
        client.width - CARD_INSET * 2.0,
        client.height - CAPTION_ROW - CARD_INSET,
    )
}

/// D-110: the client size the editor opens at for `image` on a monitor of `scale` physical
/// pixels per DIP with a `work_area` in DIPs: the image at 100%, one image pixel per device
/// pixel, filling its card, shrunk to fit the work area.
#[expect(
    clippy::cast_precision_loss,
    reason = "image sides are far below 2^24, where f32 stops being exact"
)]
pub(crate) fn opening_size(image: &RgbaImage, scale: f32, work_area: Size<Pixels>) -> Size<Pixels> {
    let chrome = size(CARD_INSET * 2.0, CAPTION_ROW + CARD_INSET);
    let (width, height) = (image.width() as f32 / scale, image.height() as f32 / scale);
    let shrink = (f32::from(work_area.width - chrome.width) / width)
        .min(f32::from(work_area.height - chrome.height) / height)
        .min(1.0);
    size(
        px(width * shrink) + chrome.width,
        px(height * shrink) + chrome.height,
    )
    .max(&MIN_WINDOW)
    .min(&work_area)
}

/// Where the image sits on the canvas: its top-left corner, relative to the canvas, and its
/// scale in screen pixels per image pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    pub corner: Point<Pixels>,
    pub scale: f32,
}

impl Placement {
    /// The image placed so its `area`, in image pixels, is centred on a canvas of size `room`,
    /// as large as fits but never more than one image pixel per device pixel (D-41, D-110). The
    /// area is the whole image or its crop (D-113), with the room its export frame takes
    /// ([`framed`]).
    pub fn fitted(room: Size<Pixels>, area: RectF, scale_factor: f32) -> Self {
        let scale = (f32::from(room.width) / area.width)
            .min(f32::from(room.height) / area.height)
            .min(1.0 / scale_factor)
            .max(MIN_SCALE);
        Self {
            corner: point(
                (room.width - px(area.width * scale)) / 2.0 - px(area.x * scale),
                (room.height - px(area.height * scale)) / 2.0 - px(area.y * scale),
            ),
            scale,
        }
    }

    /// Scaled by `factor` about `around`, a canvas point that stays over the same image pixel
    /// (F-39). The scale stops at the zoom limits.
    pub fn zoomed(self, factor: f32, around: Point<Pixels>) -> Self {
        let scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let ratio = scale / self.scale;
        Self {
            corner: around - (around - self.corner) * ratio,
            scale,
        }
    }

    /// At one image pixel per device pixel, `around`, a canvas point, staying over the same
    /// image pixel (D-144).
    pub fn actual_size(self, around: Point<Pixels>, scale_factor: f32) -> Self {
        self.zoomed(1.0 / scale_factor / self.scale, around)
    }

    /// Moved so the image point `at` sits in the middle of a canvas of `room` (D-145).
    pub fn centred_on(self, at: PointF, room: Size<Pixels>) -> Self {
        Self {
            corner: point(
                room.width / 2.0 - px(at.x * self.scale),
                room.height / 2.0 - px(at.y * self.scale),
            ),
            ..self
        }
    }

    /// The part of `area`, in image pixels, that a canvas of `room` shows (D-145).
    pub fn shows(self, room: Size<Pixels>, area: RectF) -> RectF {
        let left = (-f32::from(self.corner.x) / self.scale).max(area.x);
        let top = (-f32::from(self.corner.y) / self.scale).max(area.y);
        let right = ((f32::from(room.width) - f32::from(self.corner.x)) / self.scale)
            .min(area.x + area.width);
        let bottom = ((f32::from(room.height) - f32::from(self.corner.y)) / self.scale)
            .min(area.y + area.height);
        RectF {
            x: left,
            y: top,
            width: (right - left).max(0.0),
            height: (bottom - top).max(0.0),
        }
    }

    /// Where the part of `area`, in image pixels, that a canvas of `room` shows lies on that
    /// canvas: what the image's card backs, so the window shows round a zoomed-out image
    /// (D-158).
    pub fn covers(self, room: Size<Pixels>, area: RectF) -> Bounds<Pixels> {
        let whole = self.shows(room, area).width.min(0.0);
        Bounds::new(point(px(whole), px(whole)), room)
    }

    /// Moved by `offset` on the canvas (F-39).
    pub fn panned(self, offset: Point<Pixels>) -> Self {
        Self {
            corner: self.corner + offset,
            ..self
        }
    }
}

/// `area`, in image pixels, with the room `frame` adds round it in an export (F-44), so a
/// fitted image leaves its frame in view.
#[expect(
    clippy::cast_precision_loss,
    reason = "a frame's margins are a few dozen pixels, exact in f32"
)]
pub(crate) fn framed(area: RectF, frame: Frame) -> RectF {
    let margins = frame.margins();
    RectF {
        x: area.x - margins.left as f32,
        y: area.y - margins.top as f32,
        width: area.width + (margins.left + margins.right) as f32,
        height: area.height + (margins.top + margins.bottom) as f32,
    }
}

/// Where `document`'s image sits on a canvas of `room` until it is zoomed or panned: fitted
/// to what the canvas shows, with its export frame (D-41, D-113, D-141).
pub(crate) fn fitted_placement(
    document: &Document,
    changing_crop: bool,
    room: Size<Pixels>,
    scale_factor: f32,
) -> Placement {
    let shown = shown_area(document, changing_crop);
    Placement::fitted(room, framed(shown, document.frame()), scale_factor)
}

/// The part of `document`'s image the canvas shows: what its crop keeps, or the whole image
/// while the crop is being changed, so the crop has room to grow (D-118, D-127).
pub(crate) fn shown_area(document: &Document, changing_crop: bool) -> RectF {
    if changing_crop {
        document.image_area()
    } else {
        document.kept_area()
    }
}

/// A placement on the screen for one frame: maps image pixels to window pixels and back.
#[derive(Clone, Copy)]
pub(crate) struct Fit {
    pub origin: Point<Pixels>,
    pub scale: f32,
    pub image_bounds: Bounds<Pixels>,
    /// The part of the image the canvas draws, in image pixels (D-127).
    pub shown: RectF,
}

impl Fit {
    #[expect(
        clippy::cast_precision_loss,
        reason = "image sides are far below 2^24, where f32 stops being exact"
    )]
    pub fn new(
        canvas: Bounds<Pixels>,
        image: &RgbaImage,
        shown: RectF,
        placement: Placement,
        scale_factor: f32,
    ) -> Self {
        // D-41: the shapes start on the device pixel GPUI draws the image from.
        let origin = (canvas.origin + placement.corner)
            .map(|at| px((f32::from(at) * scale_factor).round() / scale_factor));
        let image_size = size(
            px(image.width() as f32 * placement.scale),
            px(image.height() as f32 * placement.scale),
        );
        Self {
            origin,
            scale: placement.scale,
            image_bounds: Bounds::new(origin, image_size),
            shown,
        }
    }

    /// The image pixel under `position`, clamped to the part of the image shown.
    pub fn image_point(&self, position: Point<Pixels>) -> PointF {
        let offset = position - self.origin;
        let image = |screen: Pixels, start: f32, side: f32| {
            (f32::from(screen) / self.scale).clamp(start, start + side)
        };
        PointF {
            x: image(offset.x, self.shown.x, self.shown.width),
            y: image(offset.y, self.shown.y, self.shown.height),
        }
    }

    pub fn point(&self, point_f: PointF) -> Point<Pixels> {
        self.origin + point(px(point_f.x * self.scale), px(point_f.y * self.scale))
    }

    pub fn bounds(&self, rect: RectF) -> Bounds<Pixels> {
        Bounds::new(
            self.point(PointF {
                x: rect.x,
                y: rect.y,
            }),
            size(px(rect.width * self.scale), px(rect.height * self.scale)),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cleanshot_core::{Command, History};

    use super::*;

    const CANVAS: Size<Pixels> = Size {
        width: px(1000.0),
        height: px(600.0),
    };

    fn image(width: u32, height: u32) -> RgbaImage {
        RgbaImage::new(width, height, vec![0; width as usize * height as usize * 4]).unwrap()
    }

    fn all_of(image: &RgbaImage) -> RectF {
        Document::new(Arc::new(image.clone())).kept_area()
    }

    fn fit(image_width: u32, image_height: u32, scale_factor: f32) -> Fit {
        let image = image(image_width, image_height);
        let canvas = Bounds::new(point(px(0.0), px(0.0)), CANVAS);
        Fit::new(
            canvas,
            &image,
            all_of(&image),
            Placement::fitted(CANVAS, all_of(&image), scale_factor),
            scale_factor,
        )
    }

    #[test]
    fn d41_at_every_display_scaling_a_fitted_capture_has_each_pixel_on_its_own_device_pixel() {
        let image = image(301, 157);
        let canvas = Bounds::new(point(px(0.3), px(41.7)), size(px(997.0), px(603.0)));
        for scale_factor in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let placement = Placement::fitted(canvas.size, all_of(&image), scale_factor);
            let fit = Fit::new(canvas, &image, all_of(&image), placement, scale_factor);
            assert!(
                (fit.scale * scale_factor - 1.0).abs() < 1e-6,
                "{scale_factor}"
            );
            for (x, y) in [(0.0, 0.0), (1.0, 1.0), (150.0, 77.0), (301.0, 157.0)] {
                let corner = fit.point(PointF { x, y });
                let device = |at: Pixels| f32::from(at) * scale_factor;
                let (left, top) = (device(corner.x), device(corner.y));
                assert!(
                    (left - left.round()).abs() < 1e-3 && (top - top.round()).abs() < 1e-3,
                    "{scale_factor}: pixel corner {x},{y} at device {left},{top}"
                );
            }
        }
    }

    #[test]
    fn f23_a_small_capture_is_drawn_one_image_pixel_per_device_pixel() {
        let small = fit(100, 50, 2.0);
        assert_eq!(small.scale, 0.5);
        assert_eq!(small.image_bounds.size, size(px(50.0), px(25.0)));
        assert_eq!(small.image_bounds.center(), point(px(500.0), px(300.0)));
    }

    #[test]
    fn f23_a_large_capture_shrinks_to_fit_its_card() {
        let large = fit(4000, 1200, 1.0);
        assert_eq!(large.image_bounds.size, size(px(1000.0), px(300.0)));
        assert_eq!(large.image_bounds.origin, point(px(0.0), px(150.0)));
    }

    #[test]
    fn d113_a_cropped_capture_is_fitted_by_its_crop() {
        let image = image(2000, 1000);
        let canvas = Bounds::new(point(px(0.0), px(0.0)), CANVAS);
        let shown = |crop: RectF| {
            let fit = Fit::new(
                canvas,
                &image,
                crop,
                Placement::fitted(CANVAS, crop, 1.0),
                1.0,
            );
            fit.bounds(crop)
        };
        let small = RectF {
            x: 400.0,
            y: 300.0,
            width: 484.0,
            height: 230.0,
        };
        assert_eq!(
            shown(small),
            Bounds::new(point(px(258.0), px(185.0)), size(px(484.0), px(230.0)))
        );
        let large = RectF {
            x: 0.0,
            y: 0.0,
            width: 2000.0,
            height: 920.0,
        };
        assert_eq!(
            shown(large),
            Bounds::new(point(px(0.0), px(70.0)), size(px(1000.0), px(460.0)))
        );
    }

    #[test]
    fn d110_the_editor_opens_round_the_capture_at_100_percent_where_it_fits_and_fitted_where_not() {
        let work_area = size(px(1280.0), px(680.0));
        for (width, height, scale) in [(600, 300, 1.0), (900, 600, 1.5), (300, 200, 2.0)] {
            let image = image(width, height);
            let canvas = canvas_size(opening_size(&image, scale, work_area));
            let fit = Fit::new(
                Bounds::new(point(px(0.0), px(0.0)), canvas),
                &image,
                all_of(&image),
                Placement::fitted(canvas, all_of(&image), scale),
                scale,
            );
            assert!(
                (fit.scale * scale - 1.0).abs() < 1e-6,
                "{width}x{height}@{scale}"
            );
            assert!(
                fit.image_bounds.left() >= px(0.0) && fit.image_bounds.top() >= px(0.0),
                "{width}x{height}@{scale}: inside its card"
            );
        }
        assert_eq!(
            opening_size(&image(600, 300), 1.0, work_area),
            size(px(624.0), px(360.0)),
            "D-141: the image fills its card, 12 px inside the window and below the 48 px row"
        );
        assert_eq!(
            opening_size(&image(100, 50), 1.0, work_area),
            MIN_WINDOW,
            "never smaller than the toolbar needs"
        );
        let large = image(3000, 2000);
        let window = opening_size(&large, 1.0, work_area);
        assert_eq!(window.height, work_area.height);
        assert!(window.width <= work_area.width);
        let fitted = Placement::fitted(canvas_size(window), all_of(&large), 1.0);
        assert!((fitted.scale - 0.31).abs() < 1e-6, "(680 - 60) / 2000");
    }

    #[test]
    fn d141_a_fitted_image_fills_its_card_and_leaves_its_export_frame_in_view() {
        let image = image(1000, 600);
        let canvas = Bounds::new(point(px(0.0), px(0.0)), CANVAS);
        let area = all_of(&image);
        let fitted = |frame| {
            let placement = Placement::fitted(CANVAS, framed(area, frame), 1.0);
            Fit::new(canvas, &image, area, placement, 1.0)
        };
        assert_eq!(fitted(Frame::default()).image_bounds, canvas);
        let frame = Frame {
            border: true,
            shadow: true,
        };
        let fit = fitted(frame);
        let margins = frame.margins();
        let room = |side: u32| px(f32::from(u16::try_from(side).unwrap()) * fit.scale);
        let image = fit.image_bounds;
        let reach = Bounds::from_corners(
            point(
                image.left() - room(margins.left),
                image.top() - room(margins.top),
            ),
            point(
                image.right() + room(margins.right),
                image.bottom() + room(margins.bottom),
            ),
        );
        let device_pixel = px(0.5);
        assert!(
            reach.left() >= -device_pixel
                && reach.top() >= -device_pixel
                && reach.right() <= canvas.right() + device_pixel
                && reach.bottom() <= canvas.bottom() + device_pixel,
            "the frame reaches {reach:?} on a {canvas:?} card"
        );
    }

    #[test]
    fn d144_actual_size_puts_one_image_pixel_on_each_device_pixel_about_the_centre() {
        let image = image(4000, 3000);
        let scale_factor = 1.5;
        let fitted = Placement::fitted(CANVAS, all_of(&image), scale_factor);
        let centre = point(CANVAS.width / 2.0, CANVAS.height / 2.0);
        let image_point = |placement: Placement| {
            let fit = Fit::new(
                Bounds::new(point(px(0.0), px(0.0)), CANVAS),
                &image,
                all_of(&image),
                placement,
                scale_factor,
            );
            fit.image_point(centre)
        };
        let actual = fitted.actual_size(centre, scale_factor);
        assert!(
            (actual.scale * scale_factor - 1.0).abs() < 1e-6,
            "{} device pixels per image pixel",
            actual.scale * scale_factor
        );
        let (before, after) = (image_point(fitted), image_point(actual));
        assert!(
            (before.x - after.x).abs() < 1e-2 && (before.y - after.y).abs() < 1e-2,
            "{before:?} moved to {after:?}"
        );
    }

    #[test]
    fn f30_a_pointer_outside_the_image_lands_on_its_nearest_edge() {
        let small = fit(100, 50, 2.0);
        let image_point = |x, y| small.image_point(point(px(x), px(y)));
        assert_eq!(image_point(475.0, 287.5), PointF { x: 0.0, y: 0.0 });
        assert_eq!(image_point(500.0, 300.0), PointF { x: 50.0, y: 25.0 });
        assert_eq!(image_point(0.0, 1000.0), PointF { x: 0.0, y: 50.0 });
        assert_eq!(image_point(900.0, -5.0), PointF { x: 100.0, y: 0.0 });
    }

    #[test]
    fn d127_a_cropped_canvas_shows_and_takes_the_pointer_only_inside_the_crop_until_crop_is_picked()
    {
        let image = Arc::new(image(400, 300));
        let crop = RectF {
            x: 100.0,
            y: 50.0,
            width: 200.0,
            height: 100.0,
        };
        let mut history = History::new(Document::new(Arc::clone(&image)));
        history.apply(Command::SetCrop(Some(crop))).unwrap();
        let document = history.document();
        assert_eq!(shown_area(document, false), crop);
        assert_eq!(shown_area(document, true), all_of(&image));
        let canvas = Bounds::new(point(px(0.0), px(0.0)), CANVAS);
        let fit = Fit::new(
            canvas,
            &image,
            crop,
            Placement::fitted(CANVAS, crop, 1.0),
            1.0,
        );
        let corner = fit.point(PointF { x: 0.0, y: 0.0 });
        assert_eq!(
            fit.image_point(corner),
            PointF { x: 100.0, y: 50.0 },
            "a press over the cut-off top-left lands on the crop's corner"
        );
        assert_eq!(
            fit.image_point(canvas.bottom_right()),
            PointF { x: 300.0, y: 150.0 }
        );
    }

    #[test]
    fn d158_the_card_backs_only_the_part_of_the_canvas_the_image_covers() {
        let area = all_of(&image(400, 200));
        let room = size(px(800.0), px(400.0));
        let fitted = Placement::fitted(room, area, 0.5);
        assert_eq!(
            fitted.covers(room, area),
            Bounds::new(point(px(0.0), px(0.0)), room)
        );
        let zoomed_out = fitted.zoomed(0.5, point(px(400.0), px(200.0)));
        assert_eq!(
            zoomed_out.covers(room, area),
            Bounds::new(point(px(200.0), px(100.0)), size(px(400.0), px(200.0)))
        );
        let zoomed_in = fitted.zoomed(2.0, point(px(0.0), px(0.0)));
        assert_eq!(
            zoomed_in.covers(room, area),
            Bounds::new(point(px(0.0), px(0.0)), room)
        );
    }

    #[test]
    fn f39_zooming_keeps_the_image_pixel_under_the_pointer() {
        let image = image(400, 300);
        let canvas = Bounds::new(point(px(20.0), px(30.0)), CANVAS);
        let placement = Placement::fitted(CANVAS, all_of(&image), 1.0);
        let pointer = point(px(612.0), px(411.0));
        let before = Fit::new(canvas, &image, all_of(&image), placement, 1.0)
            .image_point(canvas.origin + pointer);
        let zoomed = placement.zoomed(3.0, pointer);
        let after = Fit::new(canvas, &image, all_of(&image), zoomed, 1.0)
            .image_point(canvas.origin + pointer);
        assert_eq!(zoomed.scale, 3.0);
        assert!((after.x - before.x).abs() < 1e-3 && (after.y - before.y).abs() < 1e-3);
    }

    #[test]
    fn f39_zoom_stops_at_its_limits_and_a_pan_moves_the_image() {
        let start = Placement {
            corner: point(px(10.0), px(10.0)),
            scale: 1.0,
        };
        assert_eq!(
            start.zoomed(1000.0, point(px(0.0), px(0.0))).scale,
            MAX_SCALE
        );
        assert_eq!(
            start.zoomed(0.0001, point(px(0.0), px(0.0))).scale,
            MIN_SCALE
        );
        let panned = start.panned(point(px(-30.0), px(5.0)));
        assert_eq!(panned.corner, point(px(-20.0), px(15.0)));
        assert_eq!(panned.scale, 1.0);
    }
}
