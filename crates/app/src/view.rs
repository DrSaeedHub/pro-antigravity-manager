//! Turning accounts and quota into what the UI shows. Pure functions, tested without a window.

use accounts::SavedAccount;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use quota::format::{countdown, percent, reset_time};
use quota::{AccountQuota, QuotaBucket, QuotaError};
use slint::{ModelRc, SharedString, VecModel};

use crate::ui::{
    AccountView, CardState, DetailGroup, DetailRow, Details, Gauge, RowKind, TrayAccount, TrayFuel,
};

/// Below this fraction a limit is shown as running low.
pub const LOW: f64 = 0.2;

/// The first letter of the name, upper-cased.
#[must_use]
pub fn initial(name: &str) -> SharedString {
    name.chars().next().map(|c| c.to_ascii_uppercase().to_string()).unwrap_or_default().into()
}

/// The code and detail the UI turns into a sentence.
#[must_use]
pub fn problem(error: &QuotaError) -> (SharedString, SharedString) {
    let detail = match error {
        QuotaError::OAuth { description, .. } => description.clone(),
        QuotaError::Network(reason) => reason.clone(),
        QuotaError::Api { .. } => error.to_string(),
        _ => String::new(),
    };
    (error.code().into(), detail.into())
}

/// One limit on a card.
#[must_use]
#[allow(clippy::cast_possible_truncation, reason = "a fraction for drawing")]
pub fn gauge(fraction: Option<f64>, reset: Option<Timestamp>, now: Timestamp) -> Gauge {
    match fraction {
        None => Gauge::default(),
        Some(value) => Gauge {
            present: true,
            fraction: value.clamp(0.0, 1.0) as f32,
            percent: percent(value).into(),
            resets: countdown(value, reset, now).unwrap_or_default().into(),
            low: value < LOW,
        },
    }
}

fn limit(bucket: Option<&QuotaBucket>, now: Timestamp) -> Gauge {
    gauge(bucket.map(|b| b.fraction), bucket.and_then(|b| b.reset), now)
}

/// A card for `saved`, with its quota when known: the 5-hour and weekly limits of both groups.
/// Without a Gemini group, the recommended Gemini model with the least left stands in for its
/// 5-hour limit (a model's quota follows that limit) and its weekly limit is unknown.
#[must_use]
pub fn account_view(
    saved: &SavedAccount,
    quota: Option<&AccountQuota>,
    active_email: Option<&str>,
    now: Timestamp,
) -> AccountView {
    let email = email_of(saved, quota).unwrap_or_default();
    let active = is_active(saved, quota, active_email);
    let mut view = AccountView {
        name: saved.name.as_str().into(),
        initial: initial(&saved.name),
        email: email.into(),
        active,
        state: CardState::Loading,
        ..AccountView::default()
    };
    let Some(quota) = quota else { return view };
    if let Some(error) = &quota.error {
        let (code, detail) = problem(error);
        view.state = CardState::Failed;
        view.problem_code = code;
        view.problem_detail = detail;
        return view;
    }
    view.state = CardState::Ready;
    view.plan = quota.plan.clone().unwrap_or_default().to_uppercase().into();
    view.claude_five_hour = limit(quota.bucket("5h"), now);
    view.claude_weekly = limit(quota.bucket("weekly"), now);
    let gemini = quota.gemini_group();
    view.gemini_five_hour = if let Some(bucket) = gemini.and_then(|group| group.bucket("5h")) {
        limit(Some(bucket), now)
    } else {
        let model = quota.lowest_gemini();
        gauge(model.map(|m| m.fraction), model.and_then(|m| m.reset), now)
    };
    view.gemini_weekly = limit(gemini.and_then(|group| group.bucket("weekly")), now);
    view
}

fn row(
    label: &str,
    kind: RowKind,
    fraction: f64,
    reset: Option<Timestamp>,
    now: Timestamp,
    zone: &TimeZone,
) -> DetailRow {
    #[allow(clippy::cast_possible_truncation, reason = "a fraction for drawing")]
    let drawn = fraction.clamp(0.0, 1.0) as f32;
    let note = match countdown(fraction, reset, now) {
        Some(left) => {
            reset.map(|at| format!("{left} · {}", reset_time(at, now, zone))).unwrap_or(left)
        }
        None => String::new(),
    };
    DetailRow {
        label: label.into(),
        kind,
        fraction: drawn,
        percent: percent(fraction).into(),
        note: note.into(),
        low: fraction < LOW,
    }
}

fn bucket_label(window: &str, name: &str, id: &str) -> String {
    match window.to_ascii_lowercase().as_str() {
        "5h" => "5-hour limit".into(),
        "weekly" => "Weekly limit".into(),
        _ if !name.is_empty() => name.to_owned(),
        _ => id.to_owned(),
    }
}

fn bucket_kind(window: &str, gemini: bool) -> RowKind {
    match (window.to_ascii_lowercase().as_str(), gemini) {
        ("5h", false) => RowKind::FiveHour,
        ("weekly", false) => RowKind::Weekly,
        ("5h", true) => RowKind::GeminiFiveHour,
        ("weekly", true) => RowKind::GeminiWeekly,
        (_, true) => RowKind::Gemini,
        (_, false) => RowKind::Other,
    }
}

/// The details drawer for `saved`: every group's limits in Google's order, each in its orbit's
/// colour, then every model.
#[must_use]
pub fn details(
    saved: &SavedAccount,
    quota: Option<&AccountQuota>,
    now: Timestamp,
    zone: &TimeZone,
) -> Details {
    let mut view = Details {
        name: saved.name.as_str().into(),
        email: saved.email.clone().unwrap_or_default().into(),
        loading: quota.is_none(),
        ..Details::default()
    };
    let Some(quota) = quota else { return view };
    if let Some(email) = &quota.email {
        view.email = email.as_str().into();
    }
    view.plan = quota.plan.clone().unwrap_or_default().to_uppercase().into();
    view.updated = zone.to_datetime(quota.fetched_at).strftime("%H:%M").to_string().into();
    if let Some(error) = &quota.error {
        let (code, detail) = problem(error);
        view.problem_code = code;
        view.problem_detail = detail;
        return view;
    }
    let groups: Vec<DetailGroup> = quota
        .groups
        .iter()
        .map(|group| DetailGroup {
            name: group.name.as_str().into(),
            rows: ModelRc::new(VecModel::from(
                group
                    .buckets
                    .iter()
                    .map(|b| {
                        row(
                            &bucket_label(&b.window, &b.name, &b.id),
                            bucket_kind(&b.window, group.is_gemini()),
                            b.fraction,
                            b.reset,
                            now,
                            zone,
                        )
                    })
                    .collect::<Vec<_>>(),
            )),
        })
        .collect();
    let models: Vec<DetailRow> = quota
        .models
        .iter()
        .map(|m| {
            let kind = if m.id.to_ascii_lowercase().starts_with("gemini") {
                RowKind::Gemini
            } else {
                RowKind::Other
            };
            row(&m.display_name, kind, m.fraction, m.reset, now, zone)
        })
        .collect();
    view.groups = ModelRc::new(VecModel::from(groups));
    view.models = ModelRc::new(VecModel::from(models));
    view
}

/// The email a saved account signs in with: its latest quota reading's, else the saved one.
#[must_use]
pub fn email_of(saved: &SavedAccount, quota: Option<&AccountQuota>) -> Option<String> {
    quota.and_then(|q| q.email.clone()).or_else(|| saved.email.clone()).filter(|e| !e.is_empty())
}

/// Whether `saved` is the account Antigravity is signed in with.
#[must_use]
pub fn is_active(
    saved: &SavedAccount,
    quota: Option<&AccountQuota>,
    active_email: Option<&str>,
) -> bool {
    match (email_of(saved, quota), active_email) {
        (Some(email), Some(active)) => email.eq_ignore_ascii_case(active),
        _ => false,
    }
}

/// One account in the tray menu: what is left of Claude and GPT (the tighter of its 5-hour and
/// weekly limits) and of Gemini.
#[must_use]
pub fn tray_account(
    saved: &SavedAccount,
    quota: Option<&AccountQuota>,
    active_email: Option<&str>,
    threshold: f64,
) -> TrayAccount {
    let text = |fraction: Option<f64>| fraction.map_or_else(|| "—".to_owned(), percent);
    let mut row = TrayAccount {
        name: saved.name.as_str().into(),
        active: is_active(saved, quota, active_email),
        fuel: TrayFuel::Reading,
        ..TrayAccount::default()
    };
    let Some(quota) = quota else { return row };
    if quota.error.is_some() {
        row.fuel = TrayFuel::Unavailable;
        return row;
    }
    let claude = quota::tighter(quota.bucket("5h"), quota.bucket("weekly")).map(|b| b.fraction);
    row.claude = text(claude).into();
    row.gemini = text(quota.gemini().map(|b| b.fraction)).into();
    row.fuel = match quota::Fuel::from_quota(quota, threshold) {
        Some(fuel) if fuel.is_empty(threshold) => TrayFuel::Empty,
        Some(_) => TrayFuel::Ready,
        None => TrayFuel::Unavailable,
    };
    row
}

#[cfg(test)]
mod tests {
    use quota::{ModelQuota, QuotaBucket, QuotaGroup};
    use slint::Model;

    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn saved(name: &str, email: Option<&str>) -> SavedAccount {
        SavedAccount { name: name.into(), email: email.map(str::to_owned) }
    }

    fn sample() -> AccountQuota {
        let mut quota = AccountQuota::new("work", at("2026-11-23T10:00:00Z"));
        quota.email = Some("work@example.com".into());
        quota.plan = Some("Google AI Pro".into());
        quota.groups.push(QuotaGroup {
            name: "Gemini Models".into(),
            description: String::new(),
            buckets: vec![
                QuotaBucket {
                    id: "gemini-5h".into(),
                    name: String::new(),
                    window: "5h".into(),
                    fraction: 0.95,
                    reset: Some(at("2026-11-23T14:00:00Z")),
                },
                QuotaBucket {
                    id: "gemini-weekly".into(),
                    name: String::new(),
                    window: "weekly".into(),
                    fraction: 0.4,
                    reset: Some(at("2026-11-29T00:00:00Z")),
                },
            ],
        });
        quota.groups.push(QuotaGroup {
            name: "Claude and GPT".into(),
            description: String::new(),
            buckets: vec![
                QuotaBucket {
                    id: "f".into(),
                    name: String::new(),
                    window: "5h".into(),
                    fraction: 0.12,
                    reset: Some(at("2026-11-23T12:15:00Z")),
                },
                QuotaBucket {
                    id: "w".into(),
                    name: String::new(),
                    window: "weekly".into(),
                    fraction: 1.0,
                    reset: Some(at("2026-11-30T00:00:00Z")),
                },
                QuotaBucket {
                    id: "d".into(),
                    name: "Daily".into(),
                    window: "daily".into(),
                    fraction: 0.5,
                    reset: None,
                },
                QuotaBucket {
                    id: "odd".into(),
                    name: String::new(),
                    window: "monthly".into(),
                    fraction: 0.5,
                    reset: None,
                },
            ],
        });
        quota.models = vec![
            ModelQuota {
                id: "gemini-3".into(),
                display_name: "Gemini 3".into(),
                fraction: 0.55,
                reset: None,
                recommended: true,
                sort_index: 0,
            },
            ModelQuota {
                id: "claude".into(),
                display_name: "Claude".into(),
                fraction: 0.3,
                reset: None,
                recommended: true,
                sort_index: 1,
            },
        ];
        quota
    }

    #[test]
    fn initials_are_upper_case_first_letters() {
        assert_eq!(initial("work"), "W");
        assert_eq!(initial("9lives"), "9");
        assert_eq!(initial(""), "");
    }

    #[test]
    fn a_card_without_quota_is_loading() {
        let view = account_view(
            &saved("work", Some("w@example.com")),
            None,
            None,
            at("2026-11-23T10:00:00Z"),
        );
        assert_eq!(view.state, CardState::Loading);
        assert_eq!(view.email, "w@example.com");
        assert!(!view.active);
    }

    #[test]
    fn a_card_with_quota_shows_its_limits() {
        let now = at("2026-11-23T10:00:00Z");
        let view =
            account_view(&saved("work", None), Some(&sample()), Some("WORK@example.com"), now);
        assert_eq!(view.state, CardState::Ready);
        assert!(view.active, "emails compare case-insensitively");
        assert_eq!(view.plan, "GOOGLE AI PRO");
        assert_eq!(view.email, "work@example.com", "the quota's email wins");
        assert_eq!(view.claude_five_hour.percent, "12%");
        assert_eq!(view.claude_five_hour.resets, "2h 15m");
        assert!(view.claude_five_hour.low);
        assert_eq!(view.claude_weekly.percent, "100%");
        assert_eq!(view.claude_weekly.resets, "", "a full limit shows no countdown");
        assert_eq!(view.gemini_five_hour.percent, "95%", "the group's limit, not a model's");
        assert_eq!(view.gemini_five_hour.resets, "4h 0m");
        assert_eq!(view.gemini_weekly.percent, "40%");
        assert!(view.gemini_weekly.present && view.gemini_five_hour.present);
    }

    #[test]
    fn without_a_gemini_group_the_card_shows_the_lowest_gemini_model() {
        let mut quota = sample();
        quota.groups.remove(0);
        let view =
            account_view(&saved("work", None), Some(&quota), None, at("2026-11-23T10:00:00Z"));
        assert_eq!(view.gemini_five_hour.percent, "55%", "the lowest Gemini model");
        assert!(!view.gemini_weekly.present);
        assert_eq!(view.claude_five_hour.percent, "12%");
    }

    #[test]
    fn missing_limits_are_not_present() {
        let quota = AccountQuota::new("x", at("2026-11-23T10:00:00Z"));
        let view = account_view(&saved("x", None), Some(&quota), None, at("2026-11-23T10:00:00Z"));
        assert!(!view.claude_five_hour.present && !view.claude_weekly.present);
        assert!(!view.gemini_five_hour.present && !view.gemini_weekly.present);
        assert_eq!(view.plan, "");
    }

    #[test]
    fn a_failed_card_carries_a_problem_code() {
        let mut quota = sample();
        quota.error = Some(QuotaError::Network("timed out".into()));
        let view =
            account_view(&saved("work", None), Some(&quota), None, at("2026-11-23T10:00:00Z"));
        assert_eq!(view.state, CardState::Failed);
        assert_eq!(view.problem_code, "quota.network");
        assert_eq!(view.problem_detail, "timed out");
    }

    #[test]
    fn problems_keep_only_useful_details() {
        assert_eq!(
            problem(&QuotaError::SignInExpired),
            ("quota.sign_in_expired".into(), "".into())
        );
        let oauth = QuotaError::OAuth { code: "x".into(), description: "Bad".into() };
        assert_eq!(problem(&oauth).1, "Bad");
        let api = QuotaError::Api { method: "m".into(), status: 500, message: None };
        assert_eq!(problem(&api).1, "m: HTTP 500");
    }

    #[test]
    fn active_needs_both_emails() {
        let view = account_view(
            &saved("x", None),
            None,
            Some("a@example.com"),
            at("2026-11-23T10:00:00Z"),
        );
        assert!(!view.active);
        let view = account_view(&saved("x", Some("")), None, Some(""), at("2026-11-23T10:00:00Z"));
        assert!(!view.active);
    }

    #[test]
    fn tray_rows_show_fuel_and_the_active_account() {
        let now = at("2026-11-23T10:00:00Z");
        let _ = now;
        let reading =
            tray_account(&saved("work", Some("w@example.com")), None, Some("W@example.com"), 0.05);
        assert!(reading.active);
        assert_eq!(reading.fuel, TrayFuel::Reading);

        let ready = tray_account(&saved("work", None), Some(&sample()), None, 0.05);
        assert_eq!(ready.fuel, TrayFuel::Ready);
        assert_eq!(ready.claude, "12%", "the tighter Claude and GPT limit");
        assert_eq!(ready.gemini, "40%");
        assert!(!ready.active);

        let empty = tray_account(&saved("work", None), Some(&sample()), None, 0.2);
        assert_eq!(empty.fuel, TrayFuel::Empty, "12% is empty at a 20% threshold");

        let mut failed = sample();
        failed.error = Some(QuotaError::SignInExpired);
        assert_eq!(
            tray_account(&saved("work", None), Some(&failed), None, 0.05).fuel,
            TrayFuel::Unavailable
        );

        let unknown = AccountQuota::new("x", now);
        let row = tray_account(&saved("x", None), Some(&unknown), None, 0.05);
        assert_eq!(row.fuel, TrayFuel::Unavailable);
        assert_eq!(row.claude, "—");
        assert_eq!(row.gemini, "—");
    }

    #[test]
    fn emails_and_activity() {
        assert_eq!(email_of(&saved("x", Some("")), None), None);
        assert_eq!(
            email_of(&saved("x", Some("a@example.com")), Some(&sample())).as_deref(),
            Some("work@example.com")
        );
        assert!(!is_active(&saved("x", None), None, Some("a@example.com")));
    }

    #[test]
    fn details_list_every_limit_and_model() {
        let now = at("2026-11-23T10:00:00Z");
        let view = details(&saved("work", None), Some(&sample()), now, &TimeZone::UTC);
        assert!(!view.loading);
        assert_eq!(view.updated, "10:00");
        assert_eq!(view.email, "work@example.com");
        let gemini = view.groups.row_data(0).unwrap();
        assert_eq!(gemini.name, "Gemini Models", "Google's order");
        let labels: Vec<_> = gemini.rows.iter().map(|r| r.label.to_string()).collect();
        assert_eq!(labels, ["5-hour limit", "Weekly limit"]);
        let kinds: Vec<_> = gemini.rows.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, [RowKind::GeminiFiveHour, RowKind::GeminiWeekly]);
        let group = view.groups.row_data(1).unwrap();
        assert_eq!(group.name, "Claude and GPT");
        let labels: Vec<_> = group.rows.iter().map(|r| r.label.to_string()).collect();
        assert_eq!(labels, ["5-hour limit", "Weekly limit", "Daily", "odd"]);
        let first = group.rows.row_data(0).unwrap();
        assert_eq!(first.kind, RowKind::FiveHour);
        assert_eq!(first.note, "2h 15m · 12:15");
        assert!(first.low);
        assert_eq!(group.rows.row_data(1).unwrap().note, "");
        assert_eq!(group.rows.row_data(2).unwrap().kind, RowKind::Other);
        assert_eq!(view.models.row_count(), 2);
        assert_eq!(view.models.row_data(0).unwrap().kind, RowKind::Gemini);
        assert_eq!(view.models.row_data(1).unwrap().kind, RowKind::Other);
    }

    #[test]
    fn details_without_quota_are_loading_and_failures_show_the_problem() {
        let now = at("2026-11-23T10:00:00Z");
        let view = details(&saved("work", Some("w@example.com")), None, now, &TimeZone::UTC);
        assert!(view.loading);
        assert_eq!(view.email, "w@example.com");

        let mut failed = sample();
        failed.error = Some(QuotaError::ClientNotFound);
        let view = details(&saved("work", None), Some(&failed), now, &TimeZone::UTC);
        assert_eq!(view.problem_code, "quota.client_not_found");
        assert_eq!(view.groups.row_count(), 0);
    }
}
