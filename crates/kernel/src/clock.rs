//! An injectable clock, so tests control "now" and the local time zone.

use std::fmt::Debug;
use std::sync::{Mutex, PoisonError};

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

/// The source of the current time and the user's time zone.
pub trait Clock: Send + Sync + Debug {
    /// The current instant.
    fn now(&self) -> Timestamp;
    /// The time zone used to show local times.
    fn time_zone(&self) -> TimeZone;
}

/// The real clock and the Windows time zone.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }

    fn time_zone(&self) -> TimeZone {
        TimeZone::system()
    }
}

/// A clock that only moves when a test moves it.
#[derive(Debug)]
pub struct ManualClock {
    now: Mutex<Timestamp>,
    zone: TimeZone,
}

impl ManualClock {
    /// A clock fixed at `now` in `zone`.
    #[must_use]
    pub fn new(now: Timestamp, zone: TimeZone) -> Self {
        Self { now: Mutex::new(now), zone }
    }

    /// Moves the clock to `now`.
    pub fn set(&self, now: Timestamp) {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner) = now;
    }

    /// Moves the clock forward (or back, with a negative duration).
    pub fn advance(&self, by: SignedDuration) {
        let mut now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        *now = now.saturating_add(by).unwrap_or(*now);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn time_zone(&self) -> TimeZone {
        self.zone.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn system_clock_is_close_to_now() {
        let before = Timestamp::now();
        let now = SystemClock.now();
        assert!(now >= before);
        assert!(now.duration_since(before) < SignedDuration::from_secs(5));
        // The system zone must resolve to something usable for local times.
        let _ = SystemClock.time_zone().to_datetime(now);
    }

    #[test]
    fn manual_clock_reports_what_it_was_set_to() {
        let clock = ManualClock::new(at("2026-11-23T10:00:00Z"), TimeZone::UTC);
        assert_eq!(clock.now(), at("2026-11-23T10:00:00Z"));
        clock.set(at("2026-12-01T00:00:00Z"));
        assert_eq!(clock.now(), at("2026-12-01T00:00:00Z"));
        assert_eq!(clock.time_zone(), TimeZone::UTC);
    }

    #[test]
    fn manual_clock_advances_both_ways() {
        let clock = ManualClock::new(at("2026-11-23T10:00:00Z"), TimeZone::UTC);
        clock.advance(SignedDuration::from_mins(90));
        assert_eq!(clock.now(), at("2026-11-23T11:30:00Z"));
        clock.advance(SignedDuration::from_mins(-30));
        assert_eq!(clock.now(), at("2026-11-23T11:00:00Z"));
    }

    #[test]
    fn manual_clock_stays_put_on_overflow() {
        let clock = ManualClock::new(Timestamp::MAX, TimeZone::UTC);
        clock.advance(SignedDuration::from_hours(1));
        assert_eq!(clock.now(), Timestamp::MAX);
    }
}
