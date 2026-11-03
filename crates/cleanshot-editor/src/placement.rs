use cleanshot_core::{PointF, RectF, RgbaImage};
use gpui::{Bounds, Pixels, Point, Size, point, px, size};

/// Zoom stops between these scales, in screen pixels per image pixel (D-46).
const MIN_SCALE: f32 = 0.05;
const MAX_SCALE: f32 = 32.0;
/// The room a fitted image leaves above it for the toolbar and the property strip: their
/// 16 px offset from the top, two 42 px panels, the 8 px gap between them and 16 px below.
const CHROME_TOP: Pixels = px(124.0);
/// The room a fitted image leaves on its other sides.
const MARGIN: Pixels = px(16.0);
/// The smallest the editor window can be made.
pub(crate) const MIN_WINDOW: Size<Pixels> = Size {
    width: px(480.0),
    height: px(320.0),
};

/// D-110: the client size the editor opens at for `image` on a monitor of `scale` physical
/// pixels per DIP with a `work_area` in DIPs: the image at 100%, one image pixel per device
/// pixel, with the room [`Placement::fitted`] keeps round it, shrunk to fit the work area.
#[expect(
    clippy::cast_precision_loss,
    reason = "image sides are far below 2^24, where f32 stops being exact"
)]
pub(crate) fn opening_size(image: &RgbaImage, scale: f32, work_area: Size<Pixels>) -> Size<Pixels> {
    let chrome = size(MARGIN * 2.0, CHROME_TOP + MARGIN);
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
    /// The image placed so its `area`, in image pixels, is centred on a `canvas` of that size
    /// below the toolbar's room, as large as fits but never more than one image pixel per
    /// device pixel (D-41, D-110). The area is the whole image, or its crop (D-113).
    pub fn fitted(canvas: Size<Pixels>, area: RectF, scale_factor: f32) -> Self {
        let room = size(
            canvas.width - MARGIN * 2.0,
            canvas.height - CHROME_TOP - MARGIN,
        );
        let scale = (f32::from(room.width) / area.width)
            .min(f32::from(room.height) / area.height)
            .min(1.0 / scale_factor)
            .max(MIN_SCALE);
        Self {
            corner: point(
                MARGIN + (room.width - px(area.width * scale)) / 2.0 - px(area.x * scale),
                CHROME_TOP + (room.height - px(area.height * scale)) / 2.0 - px(area.y * scale),
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

    /// Moved by `offset` on the canvas (F-39).
    pub fn panned(self, offset: Point<Pixels>) -> Self {
        Self {
            corner: self.corner + offset,
            ..self
        }
    }
}

/// A placement on the screen for one frame: maps image pixels to window pixels and back.
#[derive(Clone, Copy)]
pub(crate) struct Fit {
    pub origin: Point<Pixels>,
    pub scale: f32,
    pub image_bounds: Bounds<Pixels>,
}

impl Fit {
    #[expect(
        clippy::cast_precision_loss,
        reason = "image sides are far below 2^24, where f32 stops being exact"
    )]
    pub fn new(
        canvas: Bounds<Pixels>,
        image: &RgbaImage,
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
        }
    }

    /// The image pixel under `position`, clamped to the image.
    pub fn image_point(&self, position: Point<Pixels>) -> PointF {
        let offset = position - self.origin;
        let image =
            |screen: Pixels, side: Pixels| f32::from(screen.clamp(px(0.0), side)) / self.scale;
        PointF {
            x: image(offset.x, self.image_bounds.size.width),
            y: image(offset.y, self.image_bounds.size.height),
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

    use cleanshot_core::Document;

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
            let fit = Fit::new(canvas, &image, placement, scale_factor);
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
        assert_eq!(small.image_bounds.center(), point(px(500.0), px(354.0)));
    }

    #[test]
    fn f23_a_large_capture_shrinks_to_fit_the_canvas_below_the_toolbar() {
        let large = fit(3872, 1200, 1.0);
        assert_eq!(large.image_bounds.size, size(px(968.0), px(300.0)));
        assert_eq!(large.image_bounds.origin, point(px(16.0), px(204.0)));
    }

    #[test]
    fn d113_a_cropped_capture_is_fitted_by_its_crop_below_the_toolbar() {
        let image = image(2000, 1000);
        let canvas = Bounds::new(point(px(0.0), px(0.0)), CANVAS);
        let shown = |crop: RectF| {
            let fit = Fit::new(canvas, &image, Placement::fitted(CANVAS, crop, 1.0), 1.0);
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
            Bounds::new(point(px(258.0), px(239.0)), size(px(484.0), px(230.0)))
        );
        let large = RectF {
            x: 0.0,
            y: 0.0,
            width: 1936.0,
            height: 920.0,
        };
        assert_eq!(
            shown(large),
            Bounds::new(point(px(16.0), px(124.0)), size(px(968.0), px(460.0)))
        );
    }

    #[test]
    fn d110_the_editor_opens_round_the_capture_at_100_percent_where_it_fits_and_fitted_where_not() {
        let work_area = size(px(1280.0), px(680.0));
        for (width, height, scale) in [(600, 300, 1.0), (900, 600, 1.5), (300, 200, 2.0)] {
            let image = image(width, height);
            let window = opening_size(&image, scale, work_area);
            let fit = Fit::new(
                Bounds::new(point(px(0.0), px(0.0)), window),
                &image,
                Placement::fitted(window, all_of(&image), scale),
                scale,
            );
            assert!(
                (fit.scale * scale - 1.0).abs() < 1e-6,
                "{width}x{height}@{scale}"
            );
            assert!(
                fit.image_bounds.top() >= CHROME_TOP,
                "{width}x{height}@{scale}: under the toolbar"
            );
        }
        assert_eq!(
            opening_size(&image(600, 300), 1.0, work_area),
            size(px(632.0), px(440.0))
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
        let fitted = Placement::fitted(window, all_of(&large), 1.0);
        assert!((fitted.scale - 0.27).abs() < 1e-6, "(680 - 140) / 2000");
    }

    #[test]
    fn f30_a_pointer_outside_the_image_lands_on_its_nearest_edge() {
        let small = fit(100, 50, 2.0);
        let image_point = |x, y| small.image_point(point(px(x), px(y)));
        assert_eq!(image_point(475.0, 341.5), PointF { x: 0.0, y: 0.0 });
        assert_eq!(image_point(500.0, 354.0), PointF { x: 50.0, y: 25.0 });
        assert_eq!(image_point(0.0, 1000.0), PointF { x: 0.0, y: 50.0 });
        assert_eq!(image_point(900.0, -5.0), PointF { x: 100.0, y: 0.0 });
    }

    #[test]
    fn f39_zooming_keeps_the_image_pixel_under_the_pointer() {
        let image = image(400, 300);
        let canvas = Bounds::new(point(px(20.0), px(30.0)), CANVAS);
        let placement = Placement::fitted(CANVAS, all_of(&image), 1.0);
        let pointer = point(px(612.0), px(411.0));
        let before = Fit::new(canvas, &image, placement, 1.0).image_point(canvas.origin + pointer);
        let zoomed = placement.zoomed(3.0, pointer);
        let after = Fit::new(canvas, &image, zoomed, 1.0).image_point(canvas.origin + pointer);
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
