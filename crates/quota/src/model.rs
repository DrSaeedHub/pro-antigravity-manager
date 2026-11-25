//! The quota of one account.

use jiff::Timestamp;

use crate::error::QuotaError;

/// One limit, such as the 5-hour or the weekly one.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaBucket {
    /// `bucketId`.
    pub id: String,
    /// `displayName`.
    pub name: String,
    /// `window`: `5h`, `daily`, `weekly`…
    pub window: String,
    /// The part left, 0 to 1.
    pub fraction: f64,
    /// When it fills up again.
    pub reset: Option<Timestamp>,
}

/// A group of limits that its models share: Google sends "Gemini Models" and "Claude and GPT
/// models", each with a 5-hour and a weekly limit.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaGroup {
    /// `displayName`.
    pub name: String,
    /// `description`.
    pub description: String,
    /// Shortest window first.
    pub buckets: Vec<QuotaBucket>,
}

impl QuotaGroup {
    /// Whether this is the Gemini models' group: Google names it "Gemini Models" and its limits
    /// `gemini-5h` and `gemini-weekly`. Every other group (today "Claude and GPT models") is not.
    #[must_use]
    pub fn is_gemini(&self) -> bool {
        self.name.to_ascii_lowercase().contains("gemini")
            || self
                .buckets
                .iter()
                .any(|bucket| bucket.id.to_ascii_lowercase().starts_with("gemini"))
    }

    /// The first bucket whose window is `window` (case-insensitive).
    #[must_use]
    pub fn bucket(&self, window: &str) -> Option<&QuotaBucket> {
        self.buckets.iter().find(|bucket| bucket.window.eq_ignore_ascii_case(window))
    }

    /// Of the 5-hour and the weekly limit, the one with less left.
    #[must_use]
    pub fn tightest(&self) -> Option<&QuotaBucket> {
        tighter(self.bucket("5h"), self.bucket("weekly"))
    }
}

/// The limit with less left (`first` on ties), or whichever one is known.
#[must_use]
pub fn tighter<'a>(
    first: Option<&'a QuotaBucket>,
    second: Option<&'a QuotaBucket>,
) -> Option<&'a QuotaBucket> {
    match (first, second) {
        (Some(a), Some(b)) if b.fraction < a.fraction => Some(b),
        (Some(a), _) => Some(a),
        (None, b) => b,
    }
}

/// One model's remaining quota.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelQuota {
    /// The model id.
    pub id: String,
    /// The name people see.
    pub display_name: String,
    /// The part left, 0 to 1.
    pub fraction: f64,
    /// When it fills up again.
    pub reset: Option<Timestamp>,
    /// Whether Antigravity recommends it.
    pub recommended: bool,
    /// Its position in Antigravity's model list.
    pub sort_index: usize,
}

/// Everything known about one account's quota at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountQuota {
    /// The saved account's name.
    pub account: String,
    /// The sign-in's email.
    pub email: Option<String>,
    /// The plan's name.
    pub plan: Option<String>,
    /// The current tier's id.
    pub tier: Option<String>,
    /// When it was read.
    pub fetched_at: Timestamp,
    /// The limit groups.
    pub groups: Vec<QuotaGroup>,
    /// The models, in Antigravity's order.
    pub models: Vec<ModelQuota>,
    /// Why reading failed, if it did.
    pub error: Option<QuotaError>,
}

impl AccountQuota {
    /// An empty result for `account` read at `fetched_at`.
    #[must_use]
    pub fn new(account: &str, fetched_at: Timestamp) -> Self {
        Self {
            account: account.to_owned(),
            email: None,
            plan: None,
            tier: None,
            fetched_at,
            groups: Vec::new(),
            models: Vec::new(),
            error: None,
        }
    }

    /// The Claude and GPT limit for `window` (case-insensitive): the first bucket with that
    /// window in a group that is not the Gemini one.
    #[must_use]
    pub fn bucket(&self, window: &str) -> Option<&QuotaBucket> {
        self.groups.iter().filter(|group| !group.is_gemini()).find_map(|group| group.bucket(window))
    }

    /// The Gemini models' group, when Google sent one.
    #[must_use]
    pub fn gemini_group(&self) -> Option<&QuotaGroup> {
        self.groups.iter().find(|group| group.is_gemini())
    }

    /// What is left for Gemini: the tighter of the Gemini group's 5-hour and weekly limits, or,
    /// without that group, the recommended Gemini model with the least left (its window empty).
    #[must_use]
    pub fn gemini(&self) -> Option<QuotaBucket> {
        if let Some(limit) = self.gemini_group().and_then(QuotaGroup::tightest) {
            return Some(limit.clone());
        }
        self.lowest_gemini().map(|model| QuotaBucket {
            id: model.id.clone(),
            name: model.display_name.clone(),
            window: String::new(),
            fraction: model.fraction,
            reset: model.reset,
        })
    }

    /// Among the recommended Gemini models, the one with the least left (the first on ties).
    #[must_use]
    pub fn lowest_gemini(&self) -> Option<&ModelQuota> {
        self.models
            .iter()
            .filter(|model| {
                model.recommended && model.id.to_ascii_lowercase().starts_with("gemini")
            })
            .fold(None, |lowest: Option<&ModelQuota>, model| match lowest {
                Some(current) if current.fraction <= model.fraction => Some(current),
                _ => Some(model),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bucket(window: &str, fraction: f64) -> QuotaBucket {
        QuotaBucket {
            id: window.into(),
            name: window.into(),
            window: window.into(),
            fraction,
            reset: None,
        }
    }

    fn model(id: &str, fraction: f64, recommended: bool) -> ModelQuota {
        ModelQuota {
            id: id.into(),
            display_name: id.into(),
            fraction,
            reset: None,
            recommended,
            sort_index: 0,
        }
    }

    fn group(name: &str, buckets: Vec<QuotaBucket>) -> QuotaGroup {
        QuotaGroup { name: name.into(), description: String::new(), buckets }
    }

    fn gemini_bucket(window: &str, fraction: f64) -> QuotaBucket {
        QuotaBucket { id: format!("gemini-{window}"), ..bucket(window, fraction) }
    }

    #[test]
    fn the_gemini_group_is_known_by_its_name_or_its_limits() {
        assert!(group("Gemini Models", vec![]).is_gemini());
        assert!(group("Other", vec![gemini_bucket("5h", 1.0)]).is_gemini());
        assert!(!group("Claude and GPT models", vec![bucket("5h", 1.0)]).is_gemini());
    }

    #[test]
    fn the_tighter_limit_has_less_left() {
        let (five, weekly) = (bucket("5h", 0.8), bucket("weekly", 0.95));
        assert_eq!(tighter(Some(&five), Some(&weekly)), Some(&five));
        assert_eq!(tighter(Some(&weekly), Some(&five)), Some(&five));
        assert_eq!(
            tighter(Some(&five), Some(&bucket("weekly", 0.8))),
            Some(&five),
            "first on ties"
        );
        assert_eq!(tighter(None, Some(&weekly)), Some(&weekly));
        assert_eq!(tighter(Some(&weekly), None), Some(&weekly));
        assert_eq!(tighter(None, None), None);
        let both = group("g", vec![bucket("5h", 0.9), bucket("weekly", 0.3), bucket("daily", 0.1)]);
        assert_eq!(both.tightest().unwrap().window, "weekly", "only 5-hour and weekly count");
        assert!(group("g", vec![bucket("daily", 0.1)]).tightest().is_none());
    }

    #[test]
    fn claude_limits_never_come_from_the_gemini_group() {
        let mut quota = AccountQuota::new("a", Timestamp::UNIX_EPOCH);
        quota.groups.push(group(
            "Gemini Models",
            vec![gemini_bucket("5h", 0.81), gemini_bucket("weekly", 0.94)],
        ));
        assert!(quota.bucket("5h").is_none());
        quota
            .groups
            .push(group("Claude and GPT models", vec![bucket("5h", 1.0), bucket("weekly", 0.5)]));
        assert_eq!(quota.bucket("5h").unwrap().fraction, 1.0);
        assert_eq!(quota.bucket("weekly").unwrap().fraction, 0.5);
        assert_eq!(quota.gemini_group().unwrap().name, "Gemini Models");
        let gemini = quota.gemini().unwrap();
        assert_eq!((gemini.window.as_str(), gemini.fraction), ("5h", 0.81));
    }

    #[test]
    fn without_a_gemini_group_the_lowest_gemini_model_stands_in() {
        let mut quota = AccountQuota::new("a", Timestamp::UNIX_EPOCH);
        assert!(quota.gemini().is_none());
        quota.models = vec![model("gemini-3", 0.7, true), model("gemini-flash", 0.4, true)];
        let gemini = quota.gemini().unwrap();
        assert_eq!(
            (gemini.id.as_str(), gemini.window.as_str(), gemini.fraction),
            ("gemini-flash", "", 0.4)
        );
    }

    #[test]
    fn finds_buckets_by_window_across_groups() {
        let mut quota = AccountQuota::new("a", Timestamp::UNIX_EPOCH);
        quota.groups.push(QuotaGroup {
            name: "one".into(),
            description: String::new(),
            buckets: vec![bucket("5h", 0.5)],
        });
        quota.groups.push(QuotaGroup {
            name: "two".into(),
            description: String::new(),
            buckets: vec![bucket("WEEKLY", 0.2), bucket("weekly", 0.9)],
        });
        assert_eq!(quota.bucket("5h").unwrap().fraction, 0.5);
        assert_eq!(quota.bucket("weekly").unwrap().fraction, 0.2);
        assert!(quota.bucket("daily").is_none());
    }

    #[test]
    fn the_lowest_gemini_ignores_other_and_unrecommended_models() {
        let mut quota = AccountQuota::new("a", Timestamp::UNIX_EPOCH);
        assert!(quota.lowest_gemini().is_none());
        quota.models = vec![
            model("claude-x", 0.0, true),
            model("gemini-pro", 0.6, true),
            model("Gemini-flash", 0.4, true),
            model("gemini-old", 0.1, false),
            model("gemini-tie", 0.4, true),
        ];
        assert_eq!(quota.lowest_gemini().unwrap().id, "Gemini-flash");
    }
}
