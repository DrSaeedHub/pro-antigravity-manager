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

/// A group of limits (for example "Claude and GPT models").
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaGroup {
    /// `displayName`.
    pub name: String,
    /// `description`.
    pub description: String,
    /// Shortest window first.
    pub buckets: Vec<QuotaBucket>,
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

    /// The first bucket whose window is `window` (case-insensitive).
    #[must_use]
    pub fn bucket(&self, window: &str) -> Option<&QuotaBucket> {
        self.groups
            .iter()
            .flat_map(|group| &group.buckets)
            .find(|bucket| bucket.window.eq_ignore_ascii_case(window))
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
