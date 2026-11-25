//! Deciding when to leave an account that ran out of quota, and where to go.
//!
//! Each limit group (Gemini, and Claude and GPT) has a 5-hour and a weekly limit, and only those
//! count. A limit at or below the threshold counts as empty, so an almost-empty account is never
//! chosen over the current one (2% never moves to 1%). A group runs down only while its models are
//! used, so the group that ran out on the active account is the one the next account must have:
//! the move goes to the account with the most left in that group.

use std::cmp::Ordering;

use jiff::Timestamp;

use crate::model::AccountQuota;

/// What is left of one limit group of one account.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupFuel {
    /// The group's name in lower case, which identifies it across accounts.
    pub group: String,
    /// The 5-hour limit's remaining fraction, when known.
    pub five_hour: Option<f64>,
    /// The weekly limit's remaining fraction, when known.
    pub weekly: Option<f64>,
    /// When the group can be used again, if a limit is empty (the later reset of its empty ones).
    pub refills_at: Option<Timestamp>,
}

impl GroupFuel {
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
    }
}

/// What is left of one account, as far as switching is concerned.
#[derive(Debug, Clone, PartialEq)]
pub struct Fuel {
    /// The saved account's name.
    pub account: String,
    /// Every group with a 5-hour or weekly limit, in Google's order.
    pub groups: Vec<GroupFuel>,
}

impl Fuel {
    /// The fuel of a quota reading; `None` when it failed or knows no 5-hour or weekly limit.
    #[must_use]
    pub fn from_quota(quota: &AccountQuota, threshold: f64) -> Option<Self> {
        if quota.error.is_some() {
            return None;
        }
        let groups: Vec<GroupFuel> = quota
            .groups
            .iter()
            .filter_map(|group| {
                let five_hour = group.bucket("5h");
                let weekly = group.bucket("weekly");
                if five_hour.is_none() && weekly.is_none() {
                    return None;
                }
                let refills_at = [five_hour, weekly]
                    .into_iter()
                    .flatten()
                    .filter(|bucket| bucket.fraction <= threshold)
                    .filter_map(|bucket| bucket.reset)
                    .max();
                Some(GroupFuel {
                    group: group.name.to_lowercase(),
                    five_hour: five_hour.map(|b| b.fraction),
                    weekly: weekly.map(|b| b.fraction),
                    refills_at,
                })
            })
            .collect();
        (!groups.is_empty()).then(|| Self { account: quota.account.clone(), groups })
    }

    /// The group named `group` (lower case).
    #[must_use]
    pub fn group(&self, group: &str) -> Option<&GroupFuel> {
        self.groups.iter().find(|fuel| fuel.group == group)
    }

    /// The tightest limit of every group.
    #[must_use]
    pub fn left(&self) -> f64 {
        self.groups.iter().map(GroupFuel::left).reduce(f64::min).unwrap_or(0.0)
    }

    /// The tightest limit of the groups named `groups`; 0 when one of them is unknown.
    #[must_use]
    pub fn left_in(&self, groups: &[String]) -> f64 {
        groups
            .iter()
            .map(|name| self.group(name).map_or(0.0, GroupFuel::left))
            .reduce(f64::min)
            .unwrap_or(0.0)
    }

    /// The groups with a limit at or below `threshold`.
    #[must_use]
    pub fn empty_groups(&self, threshold: f64) -> Vec<String> {
        self.groups
            .iter()
            .filter(|fuel| fuel.is_empty(threshold))
            .map(|fuel| fuel.group.clone())
            .collect()
    }

    /// Whether any group has a limit at or below `threshold`.
    #[must_use]
    pub fn is_empty(&self, threshold: f64) -> bool {
        self.groups.iter().any(|fuel| fuel.is_empty(threshold))
    }

    /// Whether every group named `groups` has a known limit here and none of them is empty.
    fn serves(&self, groups: &[String], threshold: f64) -> bool {
        groups.iter().all(|name| {
            self.group(name).is_some_and(|fuel| {
                (fuel.five_hour.is_some() || fuel.weekly.is_some()) && !fuel.is_empty(threshold)
            })
        })
    }

    /// When the groups named `groups` can all be used again; `None` when one of them is unknown
    /// or empty without a known reset.
    fn refills_for(&self, groups: &[String], threshold: f64) -> Option<Timestamp> {
        let mut latest = None;
        for name in groups {
            let fuel = self.group(name)?;
            if fuel.is_empty(threshold) {
                latest = latest.max(Some(fuel.refills_at?));
            }
        }
        latest
    }

    fn weekly_in(&self, groups: &[String]) -> f64 {
        groups
            .iter()
            .map(|name| self.group(name).and_then(|fuel| fuel.weekly).unwrap_or(0.0))
            .reduce(f64::min)
            .unwrap_or(0.0)
    }
}

/// What to do about the active account.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Keep the active account (it has fuel, or its quota is unknown).
    Stay,
    /// Move to `account`, which has `left` of the groups the active account ran out of.
    SwitchTo {
        /// The account to move to.
        account: String,
        /// The tightest limit of those groups there.
        left: f64,
    },
    /// The active account and every other known one are out of the groups the active account ran
    /// out of; the earliest known refill.
    AllEmpty(Option<(String, Timestamp)>),
}

fn compare(a: &Fuel, b: &Fuel, groups: &[String]) -> Ordering {
    a.left_in(groups)
        .total_cmp(&b.left_in(groups))
        .then_with(|| a.left().total_cmp(&b.left()))
        .then_with(|| a.weekly_in(groups).total_cmp(&b.weekly_in(groups)))
        .then_with(|| b.account.to_lowercase().cmp(&a.account.to_lowercase()))
}

/// Decides for the active account `active`, given every account's fuel (unknown ones left out).
#[must_use]
pub fn decide(active: &str, fuels: &[Fuel], threshold: f64) -> Decision {
    let is_active = |fuel: &&Fuel| fuel.account.eq_ignore_ascii_case(active);
    let Some(current) = fuels.iter().find(is_active) else { return Decision::Stay };
    let needed = current.empty_groups(threshold);
    if needed.is_empty() {
        return Decision::Stay;
    }
    let best = fuels
        .iter()
        .filter(|fuel| !is_active(fuel) && fuel.serves(&needed, threshold))
        .max_by(|a, b| compare(a, b, &needed));
    match best {
        Some(fuel) => {
            Decision::SwitchTo { account: fuel.account.clone(), left: fuel.left_in(&needed) }
        }
        None => Decision::AllEmpty(
            fuels
                .iter()
                .filter_map(|fuel| {
                    fuel.refills_for(&needed, threshold).map(|at| (fuel.account.clone(), at))
                })
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
    const CLAUDE: &str = "claude and gpt models";
    const GEMINI: &str = "gemini models";

    fn group(name: &str, five_hour: Option<f64>, weekly: Option<f64>) -> GroupFuel {
        GroupFuel { group: name.into(), five_hour, weekly, refills_at: None }
    }

    /// An account with only the Claude and GPT group.
    fn fuel(account: &str, five_hour: Option<f64>, weekly: Option<f64>) -> Fuel {
        Fuel { account: account.into(), groups: vec![group(CLAUDE, five_hour, weekly)] }
    }

    /// An account with both groups: (5-hour, weekly) of Gemini, then of Claude and GPT.
    fn both(account: &str, gemini: (f64, f64), claude: (f64, f64)) -> Fuel {
        Fuel {
            account: account.into(),
            groups: vec![
                group(GEMINI, Some(gemini.0), Some(gemini.1)),
                group(CLAUDE, Some(claude.0), Some(claude.1)),
            ],
        }
    }

    fn switch(account: &str, left: f64) -> Decision {
        Decision::SwitchTo { account: account.into(), left }
    }

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    /// A limit: window, fraction left, reset.
    type Limit<'a> = (&'a str, f64, Option<&'a str>);

    fn reading(account: &str, groups: &[(&str, &[Limit<'_>])]) -> AccountQuota {
        let mut quota = AccountQuota::new(account, at("2026-11-24T10:00:00Z"));
        for (name, buckets) in groups {
            quota.groups.push(QuotaGroup {
                name: (*name).into(),
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
        }
        quota
    }

    fn quota(account: &str, buckets: &[Limit<'_>]) -> AccountQuota {
        reading(account, &[("Claude and GPT models", buckets)])
    }

    #[test]
    fn an_account_with_fuel_stays() {
        let fuels = [fuel("a", Some(0.4), Some(0.9)), fuel("b", Some(1.0), Some(1.0))];
        assert_eq!(decide("a", &fuels, T), Decision::Stay);
    }

    #[test]
    fn an_empty_five_hour_or_weekly_limit_moves_on() {
        let others = fuel("b", Some(0.8), Some(0.9));
        assert_eq!(
            decide("a", &[fuel("a", Some(0.05), Some(0.9)), others.clone()], T),
            switch("b", 0.8)
        );
        assert_eq!(decide("a", &[fuel("a", Some(0.9), Some(0.01)), others], T), switch("b", 0.8));
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
        assert_eq!(decide("a", &fuels, T), switch("d", 0.6), "tighter limit, then weekly");
        let tie = [
            fuel("a", Some(0.0), None),
            fuel("Zed", Some(0.5), Some(0.5)),
            fuel("bee", Some(0.5), Some(0.5)),
        ];
        assert_eq!(decide("A", &tie, T), switch("bee", 0.5), "then by name");
    }

    #[test]
    fn the_group_that_ran_out_is_the_one_that_counts() {
        // Gemini ran out: the most Gemini left wins, however much Claude and GPT the others have.
        let fuels = [
            both("a", (0.02, 0.6), (1.0, 1.0)),
            both("b", (0.3, 0.9), (1.0, 1.0)),
            both("c", (0.9, 0.9), (0.1, 0.2)),
        ];
        assert_eq!(decide("a", &fuels, T), switch("c", 0.9));
        // Claude and GPT ran out: an empty Gemini elsewhere does not matter.
        let fuels = [
            both("a", (1.0, 1.0), (0.0, 0.4)),
            both("b", (0.0, 0.0), (0.9, 0.9)),
            both("c", (1.0, 1.0), (0.5, 0.5)),
        ];
        assert_eq!(decide("a", &fuels, T), switch("b", 0.9));
        // Equal in that group: the account with more of everything wins.
        let fuels = [
            both("a", (0.0, 0.5), (1.0, 1.0)),
            both("b", (0.7, 0.7), (0.2, 0.2)),
            both("c", (0.7, 0.7), (0.9, 0.9)),
        ];
        assert_eq!(decide("a", &fuels, T), switch("c", 0.7));
    }

    #[test]
    fn both_groups_empty_need_both_elsewhere() {
        let fuels = [
            both("a", (0.0, 0.5), (0.01, 0.5)),
            both("b", (0.9, 0.9), (0.0, 0.9)),
            both("c", (0.4, 0.4), (0.6, 0.6)),
        ];
        assert_eq!(decide("a", &fuels, T), switch("c", 0.4));
        assert_eq!(decide("a", &fuels[..2], T), Decision::AllEmpty(None));
    }

    #[test]
    fn an_account_without_the_needed_group_is_never_a_target() {
        let fuels = [both("a", (0.0, 0.5), (1.0, 1.0)), fuel("b", Some(1.0), Some(1.0))];
        assert_eq!(decide("a", &fuels, T), Decision::AllEmpty(None));
        let only_claude = [fuel("a", Some(0.0), Some(0.5)), both("b", (0.0, 0.0), (0.8, 0.8))];
        assert_eq!(decide("a", &only_claude, T), switch("b", 0.8));
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
        assert!(!fuel("a", None, None).is_empty(T), "nothing known is nothing empty");
        assert!(!fuel("a", None, Some(0.7)).is_empty(T));
        let none = Fuel { account: "x".into(), groups: Vec::new() };
        assert_eq!(none.left(), 0.0);
        assert_eq!(none.left_in(&[]), 0.0);
        assert_eq!(none.left_in(&[CLAUDE.into()]), 0.0, "an unknown group has nothing left");
        assert_eq!(none.weekly_in(&[]), 0.0);
        assert_eq!(both("x", (0.2, 0.3), (0.6, 0.4)).left(), 0.2);
        assert_eq!(
            both("x", (0.2, 0.3), (0.6, 0.4)).weekly_in(&[GEMINI.into(), CLAUDE.into()]),
            0.3
        );
    }

    #[test]
    fn the_earliest_refill_is_reported_when_all_are_empty() {
        let mut a = fuel("a", Some(0.0), Some(0.5));
        a.groups[0].refills_at = Some(at("2026-11-24T15:00:00Z"));
        let mut b = fuel("b", Some(0.03), Some(0.5));
        b.groups[0].refills_at = Some(at("2026-11-24T12:00:00Z"));
        assert_eq!(
            decide("a", &[a, b], T),
            Decision::AllEmpty(Some(("b".into(), at("2026-11-24T12:00:00Z"))))
        );
    }

    #[test]
    fn a_refill_counts_once_every_needed_group_is_back() {
        let mut a = both("a", (0.0, 0.5), (0.0, 0.5));
        a.groups[0].refills_at = Some(at("2026-11-24T11:00:00Z"));
        a.groups[1].refills_at = Some(at("2026-11-24T13:00:00Z"));
        let mut b = both("b", (0.0, 0.5), (0.9, 0.9));
        b.groups[0].refills_at = Some(at("2026-11-24T12:00:00Z"));
        let c = both("c", (0.0, 0.5), (0.0, 0.5));
        assert_eq!(
            decide("a", &[a, b, c], T),
            Decision::AllEmpty(Some(("b".into(), at("2026-11-24T12:00:00Z")))),
            "a is back at 13:00, b at 12:00, c's refill is not known"
        );
    }

    #[test]
    fn fuel_comes_from_a_quota_reading() {
        let reading_one = quota(
            "w",
            &[
                ("5h", 0.02, Some("2026-11-24T12:00:00Z")),
                ("weekly", 0.04, Some("2026-11-28T00:00:00Z")),
            ],
        );
        let fuel = Fuel::from_quota(&reading_one, T).unwrap();
        let claude = fuel.group(CLAUDE).unwrap();
        assert_eq!((claude.five_hour, claude.weekly), (Some(0.02), Some(0.04)));
        assert_eq!(
            claude.refills_at,
            Some(at("2026-11-28T00:00:00Z")),
            "both empty: the later refill frees it"
        );

        let only_five = Fuel::from_quota(
            &quota("x", &[("5h", 0.01, Some("2026-11-24T12:00:00Z")), ("weekly", 0.5, None)]),
            T,
        )
        .unwrap();
        assert_eq!(only_five.groups[0].refills_at, Some(at("2026-11-24T12:00:00Z")));

        let complete = Fuel::from_quota(&quota("y", &[("5h", 1.0, None)]), T).unwrap();
        assert_eq!(complete.groups[0].refills_at, None);
        assert_eq!(complete.groups[0].weekly, None);

        assert_eq!(Fuel::from_quota(&quota("z", &[("daily", 0.5, None)]), T), None);
        let mut failed = quota("e", &[("5h", 1.0, None)]);
        failed.error = Some(QuotaError::SignInExpired);
        assert_eq!(Fuel::from_quota(&failed, T), None);

        let two = reading(
            "t",
            &[
                ("Gemini Models", &[("weekly", 0.94, None), ("5h", 0.03, None)]),
                ("Daily only", &[("daily", 0.5, None)]),
                ("Claude and GPT models", &[("5h", 1.0, None), ("weekly", 1.0, None)]),
            ],
        );
        let fuel = Fuel::from_quota(&two, T).unwrap();
        let names: Vec<_> = fuel.groups.iter().map(|g| g.group.as_str()).collect();
        assert_eq!(names, [GEMINI, CLAUDE], "groups without a 5-hour or weekly limit are left out");
        assert_eq!(fuel.empty_groups(T), [GEMINI]);
        assert!(fuel.is_empty(T));
    }

    proptest! {
        #[test]
        fn never_switches_to_an_empty_account(
            active in ((0.0f64..1.0, 0.0f64..1.0), (0.0f64..1.0, 0.0f64..1.0)),
            others in proptest::collection::vec(((0.0f64..1.0, 0.0f64..1.0), (0.0f64..1.0, 0.0f64..1.0)), 0..6),
            threshold in 0.01f64..0.5,
        ) {
            let mut fuels = vec![both("active", active.0, active.1)];
            fuels.extend(others.iter().enumerate().map(|(i, (g, c))| both(&format!("o{i}"), *g, *c)));
            let needed = fuels[0].empty_groups(threshold);
            match decide("active", &fuels, threshold) {
                Decision::SwitchTo { account, left } => {
                    let target = fuels.iter().find(|f| f.account == account).unwrap();
                    prop_assert!(!needed.is_empty());
                    prop_assert!(target.serves(&needed, threshold));
                    prop_assert!(left.total_cmp(&target.left_in(&needed)).is_eq());
                    prop_assert!(fuels.iter().filter(|f| f.account != "active" && f.serves(&needed, threshold)).all(|f| f.left_in(&needed) <= left));
                }
                Decision::Stay => prop_assert!(!fuels[0].is_empty(threshold)),
                Decision::AllEmpty(_) => prop_assert!(fuels.iter().all(|f| !f.serves(&needed, threshold))),
            }
        }
    }
}
