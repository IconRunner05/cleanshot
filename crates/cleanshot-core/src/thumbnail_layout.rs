use std::time::Duration;

use crate::screen::{Point, Rect};

/// How far in from the work area's bottom-right corner the thumbnail rests (D-96; this and
/// the sizes below are device-independent pixels).
pub const INSET: f32 = 16.0;
/// The height of Save and Dismiss.
pub const BUTTON_HEIGHT: f32 = 24.0;
/// The width of Save and Dismiss.
pub const BUTTON_WIDTH: f32 = 64.0;
/// The gap between the buttons, and between each and the thumbnail's edges.
pub const BUTTON_GAP: f32 = 8.0;
/// The largest preview; a capture is scaled down to fit.
pub const LARGEST_PREVIEW: (f32, f32) = (240.0, 160.0);
/// Room for Save and Dismiss with a gap around each.
pub const SMALLEST_WINDOW: (f32, f32) = (
    2.0 * BUTTON_WIDTH + 3.0 * BUTTON_GAP,
    BUTTON_HEIGHT + 2.0 * BUTTON_GAP,
);
/// How long the thumbnail shows while nothing holds it, the settings file's `thumbnail_ms`: a
/// whole number of milliseconds from [`ThumbnailStay::SHORTEST_MS`] to
/// [`ThumbnailStay::LONGEST_MS`] (F-09, D-165).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThumbnailStay(u32);

impl ThumbnailStay {
    /// The shortest stay, in milliseconds.
    pub const SHORTEST_MS: u32 = 1000;
    /// The longest stay, in milliseconds.
    pub const LONGEST_MS: u32 = 60_000;

    /// `ms` milliseconds, or None outside the range.
    #[must_use]
    pub fn from_ms(ms: u32) -> Option<Self> {
        (Self::SHORTEST_MS..=Self::LONGEST_MS)
            .contains(&ms)
            .then_some(Self(ms))
    }

    /// The stay as a duration.
    #[must_use]
    pub fn duration(self) -> Duration {
        Duration::from_millis(u64::from(self.0))
    }

    /// The stay in milliseconds, as `thumbnail_ms` holds it.
    #[must_use]
    pub fn ms(self) -> u32 {
        self.0
    }
}

impl Default for ThumbnailStay {
    fn default() -> Self {
        Self(5000)
    }
}
/// How long it takes to slide in.
pub const SLIDE: Duration = Duration::from_millis(120);

/// A button the thumbnail shows while the pointer is on it (F-09).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbnailButton {
    Save,
    Dismiss,
}

impl ThumbnailButton {
    /// The button's text, which a screen reader also says.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Save => "Save",
            Self::Dismiss => "Dismiss",
        }
    }
}

/// Where a thumbnail `width` x `height` rests: `inset` in from the bottom-right corner of
/// `work`.
#[must_use]
pub fn place(work: Rect, width: i32, height: i32, inset: i32) -> Rect {
    let (right, bottom) = (work.right - inset, work.bottom - inset);
    Rect {
        left: right - width,
        top: bottom - height,
        right,
        bottom,
    }
}

/// The thumbnail's client size for a `width` x `height` preview at `scale` physical pixels
/// per DIP: the preview's own, but never too small for its buttons.
#[must_use]
pub fn window_size(width: i32, height: i32, scale: f32) -> (i32, i32) {
    (
        width.max(pixels(SMALLEST_WINDOW.0, scale)),
        height.max(pixels(SMALLEST_WINDOW.1, scale)),
    )
}

/// Where a `width` x `height` preview sits in a client `window`: centred.
#[must_use]
pub fn centred((window_width, window_height): (i32, i32), width: i32, height: i32) -> Rect {
    let (left, top) = ((window_width - width) / 2, (window_height - height) / 2);
    Rect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// How far right of its resting place the thumbnail is `elapsed` into a slide of `distance`
/// pixels, or `None` once it has arrived.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a part of `distance`, which is an i32"
)]
pub fn slid(elapsed: Duration, distance: i32) -> Option<i32> {
    let left = SLIDE.checked_sub(elapsed).filter(|left| !left.is_zero())?;
    Some((f64::from(distance) * left.as_secs_f64() / SLIDE.as_secs_f64()) as i32)
}

/// The time the thumbnail has left to show after `elapsed` more, or `None` once it is up.
/// While it is `held`, by the pointer on it or a drag from it, the time stands still (D-96).
#[must_use]
pub fn left_after(left: Duration, elapsed: Duration, held: bool) -> Option<Duration> {
    if held {
        return Some(left);
    }
    left.checked_sub(elapsed).filter(|left| !left.is_zero())
}

/// The buttons on a thumbnail `width` x `height` at `scale` physical pixels per DIP, in its
/// client pixels: Save and Dismiss side by side, centred along its bottom edge.
#[must_use]
pub fn buttons(width: i32, height: i32, scale: f32) -> [(ThumbnailButton, Rect); 2] {
    let (button, gap) = (pixels(BUTTON_WIDTH, scale), pixels(BUTTON_GAP, scale));
    let top = height - gap - pixels(BUTTON_HEIGHT, scale);
    let left = (width - 2 * button - gap) / 2;
    let at = |left: i32| Rect {
        left,
        top,
        right: left + button,
        bottom: top + pixels(BUTTON_HEIGHT, scale),
    };
    [
        (ThumbnailButton::Save, at(left)),
        (ThumbnailButton::Dismiss, at(left + button + gap)),
    ]
}

/// The button under `point`, in client pixels, if any.
#[must_use]
pub fn button_at(buttons: &[(ThumbnailButton, Rect); 2], point: Point) -> Option<ThumbnailButton> {
    buttons
        .iter()
        .find_map(|&(button, area)| area.holds(point).then_some(button))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the thumbnail's sizes are a few hundred pixels"
)]
fn pixels(dips: f32, scale: f32) -> i32 {
    (dips * scale).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };

    #[test]
    fn f09_the_thumbnail_rests_its_inset_in_from_the_work_areas_bottom_right_corner() {
        assert_eq!(
            place(WORK, 300, 200, 16),
            Rect {
                left: 1604,
                top: 824,
                right: 1904,
                bottom: 1024,
            }
        );
    }

    #[test]
    fn f09_the_slide_starts_its_distance_out_and_arrives_in_120_ms() {
        let at = |millis| slid(Duration::from_millis(millis), 16);
        assert_eq!(at(0), Some(16));
        assert_eq!(at(60), Some(8));
        assert_eq!(at(119), Some(0));
        assert_eq!(at(120), None);
        assert_eq!(at(500), None);
    }

    #[test]
    fn f09_the_pointer_on_the_thumbnail_stops_its_five_seconds_running_out() {
        let second = Duration::from_secs(1);
        let stay = ThumbnailStay::default().duration();
        assert_eq!(left_after(stay, second, true), Some(stay));
        assert_eq!(
            left_after(stay, second, false),
            Some(Duration::from_secs(4))
        );
        assert_eq!(left_after(second, second, false), None);
        assert_eq!(left_after(second, stay, true), Some(second));
    }

    #[test]
    fn f09_a_preview_too_small_for_the_buttons_sits_centred_in_a_window_they_fit() {
        assert_eq!(window_size(30, 20, 1.0), (152, 40));
        assert_eq!(window_size(30, 20, 1.5), (228, 60));
        assert_eq!(window_size(200, 1, 1.0), (200, 40), "a sliver is no strip");
        assert_eq!(window_size(240, 160, 1.0), (240, 160));
        assert_eq!(
            centred((152, 40), 30, 20),
            Rect {
                left: 61,
                top: 10,
                right: 91,
                bottom: 30
            }
        );
        for (_, area) in buttons(152, 40, 1.0) {
            assert!(area.left >= 0 && area.top >= 0, "{area:?}");
            assert!(area.right <= 152 && area.bottom <= 40, "{area:?}");
        }
    }

    #[test]
    fn f09_save_and_dismiss_sit_side_by_side_along_the_bottom_and_nothing_else_is_a_button() {
        let placed = buttons(300, 200, 1.0);
        let [
            (ThumbnailButton::Save, save),
            (ThumbnailButton::Dismiss, dismiss),
        ] = placed
        else {
            panic!("Save then Dismiss: {placed:?}");
        };
        assert_eq!((save.bottom, dismiss.bottom), (192, 192));
        assert_eq!(dismiss.left - save.right, 8);
        assert_eq!(save.left, 300 - dismiss.right);
        let point = |x, y| Point { x, y };
        assert_eq!(
            button_at(&placed, point(save.left, save.top)),
            Some(ThumbnailButton::Save)
        );
        assert_eq!(
            button_at(&placed, point(dismiss.right - 1, dismiss.bottom - 1)),
            Some(ThumbnailButton::Dismiss)
        );
        assert_eq!(button_at(&placed, point(150, 20)), None);
        assert_eq!(button_at(&placed, point(save.right, save.top)), None);
        assert_eq!(button_at(&placed, point(save.right + 1, save.top)), None);
    }
}
