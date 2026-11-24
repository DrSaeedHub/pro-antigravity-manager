//! Turning fractions and reset times into short text.

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

/// A fraction this close to 1 counts as full; a full limit's reset time is not shown.
pub const FULL: f64 = 0.995;

/// `42%`: clamped to 0–100, halves rounded to even.
#[must_use]
pub fn percent(fraction: f64) -> String {
    let value = (fraction.clamp(0.0, 1.0) * 100.0).round_ties_even();
    format!("{value:.0}%")
}

/// Time until `reset`: `now`, `45m`, `2h 15m` or `3d 4h`.
#[must_use]
pub fn remaining(now: Timestamp, reset: Timestamp) -> String {
    let left = reset.duration_since(now);
    if left < SignedDuration::from_mins(1) {
        return "now".to_owned();
    }
    let total_minutes = left.as_secs() / 60;
    let (days, hours, minutes) =
        (total_minutes / 1440, total_minutes / 60 % 24, total_minutes % 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

/// The countdown shown next to a limit, or nothing when the limit is full or has no reset.
#[must_use]
pub fn countdown(fraction: f64, reset: Option<Timestamp>, now: Timestamp) -> Option<String> {
    reset.filter(|_| fraction < FULL).map(|reset| remaining(now, reset))
}

/// When `reset` happens in local time: `14:30` today, otherwise `Mon 24 Nov 14:30`.
#[must_use]
pub fn reset_time(reset: Timestamp, now: Timestamp, zone: &TimeZone) -> String {
    let local = zone.to_datetime(reset);
    if local.date() == zone.to_datetime(now).date() {
        local.strftime("%H:%M").to_string()
    } else {
        local.strftime("%a %d %b %H:%M").to_string()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn percents_round_halves_to_even_and_clamp() {
        assert_eq!(percent(0.42), "42%");
        assert_eq!(percent(0.125), "12%");
        assert_eq!(percent(0.375), "38%");
        assert_eq!(percent(0.625), "62%");
        assert_eq!(percent(1.0), "100%");
        assert_eq!(percent(1.7), "100%");
        assert_eq!(percent(-0.2), "0%");
        assert_eq!(percent(0.004), "0%");
    }

    #[test]
    fn remaining_time_uses_the_largest_two_units() {
        let now = at("2026-11-23T10:00:00Z");
        assert_eq!(remaining(now, at("2026-11-23T10:00:30Z")), "now");
        assert_eq!(remaining(now, at("2026-11-23T09:00:00Z")), "now");
        assert_eq!(remaining(now, at("2026-11-23T10:01:00Z")), "1m");
        assert_eq!(remaining(now, at("2026-11-23T10:45:59Z")), "45m");
        assert_eq!(remaining(now, at("2026-11-23T12:15:00Z")), "2h 15m");
        assert_eq!(remaining(now, at("2026-11-23T11:00:00Z")), "1h 0m");
        assert_eq!(remaining(now, at("2026-11-26T14:30:00Z")), "3d 4h");
        assert_eq!(remaining(now, at("2026-11-24T10:00:00Z")), "1d 0h");
    }

    #[test]
    fn countdowns_hide_for_full_limits_and_missing_resets() {
        let now = at("2026-11-23T10:00:00Z");
        let reset = Some(at("2026-11-23T12:15:00Z"));
        assert_eq!(countdown(0.5, reset, now).as_deref(), Some("2h 15m"));
        assert_eq!(countdown(0.996, reset, now), None);
        assert_eq!(countdown(0.5, None, now), None);
    }

    #[test]
    fn reset_times_are_local() {
        let tehran = TimeZone::get("Asia/Tehran").unwrap();
        let now = at("2026-11-23T10:00:00Z");
        assert_eq!(reset_time(at("2026-11-23T16:00:00Z"), now, &tehran), "19:30");
        assert_eq!(reset_time(at("2026-11-24T16:00:00Z"), now, &tehran), "Tue 24 Nov 19:30");
        assert_eq!(reset_time(at("2026-11-23T21:00:00Z"), now, &tehran), "Tue 24 Nov 00:30");
        assert_eq!(reset_time(at("2026-11-23T21:00:00Z"), now, &TimeZone::UTC), "21:00");
    }

    proptest! {
        #[test]
        fn percent_is_always_between_0_and_100(fraction in -10.0f64..10.0) {
            let text = percent(fraction);
            let value: u32 = text.trim_end_matches('%').parse().unwrap();
            prop_assert!(value <= 100);
        }

        #[test]
        fn remaining_never_panics(seconds in -1_000_000i64..100_000_000) {
            let now = at("2026-11-23T10:00:00Z");
            let reset = now.checked_add(SignedDuration::from_secs(seconds)).unwrap();
            prop_assert!(!remaining(now, reset).is_empty());
        }
    }
}
