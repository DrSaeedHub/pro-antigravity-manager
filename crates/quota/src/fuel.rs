//! Deciding when to leave an account that ran out of quota, and where to go.
//!
//! Only the 5-hour and weekly limits count. A limit at or below the threshold counts as empty, so an
//! almost-empty account is never chosen over the current one (2% never moves to 1%).

use std::cmp::Ordering;

use jiff::Timestamp;

use crate::model::AccountQuota;

/// What is left of one account, as far as switching is concerned.
#[derive(Debug, Clone, PartialEq)]
pub struct Fuel {
    /// The saved account's name.
    pub account: String,
    /// The 5-hour limit's remaining fraction, when known.
    pub five_hour: Option<f64>,
    /// The weekly limit's remaining fraction, when known.
    pub weekly: Option<f64>,
    /// When the first empty limit refills, if any limit is empty.
    pub refills_at: Option<Timestamp>,
}

impl Fuel {
    /// The fuel of a quota reading; `None` when it failed or knows neither limit.
    #[must_use]
    pub fn from_quota(quota: &AccountQuota, threshold: f64) -> Option<Self> {
        if quota.error.is_some() {
            return None;
        }
        let five_hour = quota.bucket("5h");
        let weekly = quota.bucket("weekly");
        if five_hour.is_none() && weekly.is_none() {
            return None;
        }
        let refills_at = [five_hour, weekly]
            .into_iter()
            .flatten()
            .filter(|bucket| bucket.fraction <= threshold)
            .filter_map(|bucket| bucket.reset)
            .max();
        Some(Self {
            account: quota.account.clone(),
            five_hour: five_hour.map(|b| b.fraction),
            weekly: weekly.map(|b| b.fraction),
            refills_at,
        })
    }

    /// The tighter of the known limits.
    #[must_use]
    pub fn left(&self) -> f64 {
        match (self.five_hour, self.weekly) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => 0.0,
        }
    }

    /// Whether a known limit is at or below `threshold`.
    #[must_use]
    pub fn is_empty(&self, threshold: f64) -> bool {
        [self.five_hour, self.weekly].into_iter().flatten().any(|fraction| fraction <= threshold)
            || (self.five_hour.is_none() && self.weekly.is_none())
    }
}

/// What to do about the active account.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Keep the active account (it has fuel, or its quota is unknown).
    Stay,
    /// Move to this account.
    SwitchTo(String),
    /// The active account and every other known one are out of quota; the earliest known refill.
    AllEmpty(Option<(String, Timestamp)>),
}

fn compare(a: &Fuel, b: &Fuel) -> Ordering {
    a.left()
        .total_cmp(&b.left())
        .then_with(|| a.weekly.unwrap_or(0.0).total_cmp(&b.weekly.unwrap_or(0.0)))
        .then_with(|| b.account.to_lowercase().cmp(&a.account.to_lowercase()))
}

/// Decides for the active account `active`, given every account's fuel (unknown ones left out).
#[must_use]
pub fn decide(active: &str, fuels: &[Fuel], threshold: f64) -> Decision {
    let is_active = |fuel: &&Fuel| fuel.account.eq_ignore_ascii_case(active);
    let Some(current) = fuels.iter().find(is_active) else { return Decision::Stay };
    if !current.is_empty(threshold) {
        return Decision::Stay;
    }
    let best = fuels
        .iter()
        .filter(|fuel| !is_active(fuel) && !fuel.is_empty(threshold))
        .max_by(|a, b| compare(a, b));
    match best {
        Some(fuel) => Decision::SwitchTo(fuel.account.clone()),
        None => Decision::AllEmpty(
            fuels
                .iter()
                .filter_map(|fuel| fuel.refills_at.map(|at| (fuel.account.clone(), at)))
                .min_by_key(|(_, at)| *at),
        ),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::error::QuotaError;
    use crate::model::{QuotaBucket, QuotaGroup};

    const T: f64 = 0.05;

    fn fuel(account: &str, five_hour: Option<f64>, weekly: Option<f64>) -> Fuel {
        Fuel { account: account.into(), five_hour, weekly, refills_at: None }
    }

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn quota(account: &str, buckets: &[(&str, f64, Option<&str>)]) -> AccountQuota {
        let mut quota = AccountQuota::new(account, at("2026-11-24T10:00:00Z"));
        quota.groups.push(QuotaGroup {
            name: "g".into(),
            description: String::new(),
            buckets: buckets
                .iter()
                .map(|(window, fraction, reset)| QuotaBucket {
                    id: (*window).into(),
                    name: String::new(),
                    window: (*window).into(),
                    fraction: *fraction,
                    reset: reset.map(at),
                })
                .collect(),
        });
        quota
    }

    #[test]
    fn an_account_with_fuel_stays() {
        let fuels = [fuel("a", Some(0.4), Some(0.9)), fuel("b", Some(1.0), Some(1.0))];
        assert_eq!(decide("a", &fuels, T), Decision::Stay);
    }

    #[test]
    fn an_empty_five_hour_or_weekly_limit_moves_on() {
        let others = fuel("b", Some(0.8), Some(0.8));
        assert_eq!(
            decide("a", &[fuel("a", Some(0.05), Some(0.9)), others.clone()], T),
            Decision::SwitchTo("b".into())
        );
        assert_eq!(
            decide("a", &[fuel("a", Some(0.9), Some(0.01)), others], T),
            Decision::SwitchTo("b".into())
        );
    }

    #[test]
    fn two_percent_never_moves_to_one_percent() {
        let fuels = [fuel("a", Some(0.02), Some(0.5)), fuel("b", Some(0.01), Some(0.9))];
        assert_eq!(decide("a", &fuels, T), Decision::AllEmpty(None));
    }

    #[test]
    fn picks_the_most_fuel() {
        let fuels = [
            fuel("a", Some(0.0), Some(0.3)),
            fuel("b", Some(0.9), Some(0.2)),
            fuel("c", Some(0.6), Some(0.7)),
            fuel("d", Some(0.6), Some(0.9)),
        ];
        assert_eq!(
            decide("a", &fuels, T),
            Decision::SwitchTo("d".into()),
            "tighter limit, then weekly"
        );
        let tie = [
            fuel("a", Some(0.0), None),
            fuel("Zed", Some(0.5), Some(0.5)),
            fuel("bee", Some(0.5), Some(0.5)),
        ];
        assert_eq!(decide("A", &tie, T), Decision::SwitchTo("bee".into()), "then by name");
    }

    #[test]
    fn unknown_quota_is_never_a_target_and_unknown_active_decides_nothing() {
        let fuels = [fuel("a", Some(0.0), Some(0.0)), fuel("b", None, None)];
        assert_eq!(decide("a", &fuels, T), Decision::AllEmpty(None));
        assert_eq!(decide("ghost", &fuels, T), Decision::Stay);
    }

    #[test]
    fn missing_limits() {
        assert_eq!(fuel("a", Some(0.3), None).left(), 0.3);
        assert_eq!(fuel("a", None, Some(0.7)).left(), 0.7);
        assert_eq!(fuel("a", None, None).left(), 0.0);
        assert!(fuel("a", None, None).is_empty(T));
        assert!(!fuel("a", None, Some(0.7)).is_empty(T));
    }

    #[test]
    fn the_earliest_refill_is_reported_when_all_are_empty() {
        let mut a = fuel("a", Some(0.0), Some(0.5));
        a.refills_at = Some(at("2026-11-24T15:00:00Z"));
        let mut b = fuel("b", Some(0.03), Some(0.5));
        b.refills_at = Some(at("2026-11-24T12:00:00Z"));
        assert_eq!(
            decide("a", &[a, b], T),
            Decision::AllEmpty(Some(("b".into(), at("2026-11-24T12:00:00Z"))))
        );
    }

    #[test]
    fn fuel_comes_from_a_quota_reading() {
        let reading = quota(
            "w",
            &[
                ("5h", 0.02, Some("2026-11-24T12:00:00Z")),
                ("weekly", 0.04, Some("2026-11-28T00:00:00Z")),
            ],
        );
        let fuel = Fuel::from_quota(&reading, T).unwrap();
        assert_eq!((fuel.five_hour, fuel.weekly), (Some(0.02), Some(0.04)));
        assert_eq!(
            fuel.refills_at,
            Some(at("2026-11-28T00:00:00Z")),
            "both empty: the later refill frees it"
        );

        let only_five = Fuel::from_quota(
            &quota("x", &[("5h", 0.01, Some("2026-11-24T12:00:00Z")), ("weekly", 0.5, None)]),
            T,
        )
        .unwrap();
        assert_eq!(only_five.refills_at, Some(at("2026-11-24T12:00:00Z")));

        let complete = Fuel::from_quota(&quota("y", &[("5h", 1.0, None)]), T).unwrap();
        assert_eq!(complete.refills_at, None);
        assert_eq!(complete.weekly, None);

        assert_eq!(Fuel::from_quota(&quota("z", &[("daily", 0.5, None)]), T), None);
        let mut failed = quota("e", &[("5h", 1.0, None)]);
        failed.error = Some(QuotaError::SignInExpired);
        assert_eq!(Fuel::from_quota(&failed, T), None);
    }

    proptest! {
        #[test]
        fn never_switches_to_an_empty_account(
            active in (0.0f64..1.0, 0.0f64..1.0),
            others in proptest::collection::vec((0.0f64..1.0, 0.0f64..1.0), 0..6),
            threshold in 0.01f64..0.5,
        ) {
            let mut fuels = vec![fuel("active", Some(active.0), Some(active.1))];
            fuels.extend(others.iter().enumerate().map(|(i, (a, b))| fuel(&format!("o{i}"), Some(*a), Some(*b))));
            match decide("active", &fuels, threshold) {
                Decision::SwitchTo(name) => {
                    let target = fuels.iter().find(|f| f.account == name).unwrap();
                    prop_assert!(!target.is_empty(threshold));
                    prop_assert!(fuels[0].is_empty(threshold));
                    prop_assert!(fuels.iter().filter(|f| f.account != "active" && !f.is_empty(threshold)).all(|f| f.left() <= target.left()));
                }
                Decision::Stay => prop_assert!(!fuels[0].is_empty(threshold)),
                Decision::AllEmpty(_) => prop_assert!(fuels.iter().all(|f| f.is_empty(threshold))),
            }
        }
    }
}
