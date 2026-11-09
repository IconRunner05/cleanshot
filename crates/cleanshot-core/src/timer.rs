use std::time::Duration;

/// How long the capture panel waits before the grab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Timer {
    #[default]
    Off,
    Three,
    Five,
    Ten,
}

impl Timer {
    /// The wait in whole seconds.
    #[must_use]
    pub fn seconds(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::Three => 3,
            Self::Five => 5,
            Self::Ten => 10,
        }
    }

    /// The timer a press on the panel's Timer control moves to: Off, 3, 5, 10, then Off again.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Three,
            Self::Three => Self::Five,
            Self::Five => Self::Ten,
            Self::Ten => Self::Off,
        }
    }

    /// The whole seconds the countdown shows `elapsed` after it started, or `None` once it is
    /// time to grab.
    #[must_use]
    pub fn remaining(self, elapsed: Duration) -> Option<u32> {
        (0..self.seconds())
            .find(|&passed| elapsed < Duration::from_secs((passed + 1).into()))
            .map(|passed| self.seconds() - passed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_steps_the_timer_off_3_5_10_and_back_to_off() {
        let mut timer = Timer::default();
        let mut seen = Vec::new();
        for _ in 0..5 {
            seen.push(timer.seconds());
            timer = timer.next();
        }
        assert_eq!(seen, [0, 3, 5, 10, 0]);
    }

    #[test]
    fn the_countdown_shows_the_whole_seconds_left_and_ends_at_the_wait() {
        let at = |millis| Timer::Three.remaining(Duration::from_millis(millis));
        assert_eq!(at(0), Some(3));
        assert_eq!(at(1), Some(3));
        assert_eq!(at(1000), Some(2));
        assert_eq!(at(2999), Some(1));
        assert_eq!(at(3000), None);
        assert_eq!(at(60_000), None);
    }

    #[test]
    fn with_the_timer_off_the_grab_is_at_once() {
        assert_eq!(Timer::Off.remaining(Duration::ZERO), None);
    }
}
