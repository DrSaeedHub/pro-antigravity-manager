//! Reading the Cloud Code API responses. Lenient: missing fields get neutral values.

use std::collections::HashMap;

use kernel::json;
use serde_json::Value;

use crate::model::{ModelQuota, QuotaBucket, QuotaGroup};

/// What `loadCodeAssist` says about the account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeAssist {
    /// `paidTier.name`, else `currentTier.name`.
    pub plan: Option<String>,
    /// `currentTier.id`.
    pub tier: Option<String>,
    /// `cloudaicompanionProject` (a string, or an object's `id`).
    pub project: Option<String>,
}

/// Reads a `loadCodeAssist` response.
#[must_use]
pub fn code_assist(response: &Value) -> CodeAssist {
    let empty = Value::Null;
    let paid = json::get(response, "paidTier").unwrap_or(&empty);
    let current = json::get(response, "currentTier").unwrap_or(&empty);
    let project = match json::get(response, "cloudaicompanionProject") {
        Some(Value::String(id)) => Some(id.clone()),
        Some(node) => json::text(node, "id"),
        None => None,
    };
    CodeAssist {
        plan: json::text(paid, "name").or_else(|| json::text(current, "name")),
        tier: json::text(current, "id"),
        project: project.filter(|id| !id.is_empty()),
    }
}

/// The request body that names the project, or `{}` without one.
#[must_use]
pub fn project_body(project: Option<&str>) -> String {
    match project {
        Some(id) => serde_json::json!({ "project": id }).to_string(),
        None => "{}".to_owned(),
    }
}

fn window_rank(window: &str) -> u8 {
    match window.to_ascii_lowercase().as_str() {
        "5h" => 0,
        "daily" => 1,
        "weekly" => 2,
        _ => 3,
    }
}

/// Reads a `retrieveUserQuotaSummary` response. A missing fraction means nothing is left
/// (Google omits zero values). Buckets are sorted shortest window first.
#[must_use]
pub fn summary(response: &Value) -> Vec<QuotaGroup> {
    json::items(response, "groups")
        .iter()
        .map(|group| {
            let mut buckets: Vec<QuotaBucket> = json::items(group, "buckets")
                .iter()
                .map(|bucket| QuotaBucket {
                    id: json::text(bucket, "bucketId").unwrap_or_default(),
                    name: json::text(bucket, "displayName").unwrap_or_default(),
                    window: json::text(bucket, "window").unwrap_or_default(),
                    fraction: json::number(bucket, "remainingFraction").unwrap_or(0.0),
                    reset: json::time(bucket, "resetTime"),
                })
                .collect();
            buckets.sort_by_key(|bucket| window_rank(&bucket.window));
            QuotaGroup {
                name: json::text(group, "displayName").unwrap_or_default(),
                description: json::text(group, "description").unwrap_or_default(),
                buckets,
            }
        })
        .collect()
}

fn model_order(response: &Value) -> Vec<String> {
    let sorts = json::get(response, "agentModelSorts");
    let groups: Vec<&Value> = match sorts {
        Some(Value::Array(sorts)) => {
            sorts.iter().flat_map(|sort| json::items(sort, "groups")).collect()
        }
        Some(sort @ Value::Object(_)) => json::items(sort, "groups").iter().collect(),
        _ => Vec::new(),
    };
    groups
        .into_iter()
        .flat_map(|group| json::items(group, "modelIds"))
        .filter_map(|id| match id {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        })
        .collect()
}

/// Reads a `fetchAvailableModels` response: visible models with quota, one per display name
/// (the most limited one wins), in Antigravity's order, then by name.
#[must_use]
pub fn models(response: &Value) -> Vec<ModelQuota> {
    let order = model_order(response);
    let Some(Value::Object(entries)) = json::get(response, "models") else { return Vec::new() };

    let mut by_name: Vec<ModelQuota> = Vec::new();
    let mut index_of: HashMap<String, usize> = HashMap::new();
    for (id, model) in entries {
        let Some(quota) = json::get(model, "quotaInfo") else { continue };
        let Some(display_name) = json::text(model, "displayName").filter(|n| !n.is_empty()) else {
            continue;
        };
        if json::flag(model, "isInternal") {
            continue;
        }
        let item = ModelQuota {
            id: id.clone(),
            display_name: display_name.clone(),
            fraction: json::number(quota, "remainingFraction").unwrap_or(0.0),
            reset: json::time(quota, "resetTime"),
            recommended: json::flag(model, "recommended"),
            sort_index: order.iter().position(|known| known == id).unwrap_or(usize::MAX),
        };
        let key = display_name.to_lowercase();
        if let Some(&slot) = index_of.get(&key) {
            let existing = &mut by_name[slot];
            let sort_index = existing.sort_index.min(item.sort_index);
            let recommended = existing.recommended || item.recommended;
            if item.fraction < existing.fraction {
                *existing = item;
            }
            existing.sort_index = sort_index;
            existing.recommended = recommended;
        } else {
            index_of.insert(key, by_name.len());
            by_name.push(item);
        }
    }
    by_name.sort_by(|a, b| {
        a.sort_index
            .cmp(&b.sort_index)
            .then_with(|| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()))
    });
    by_name
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn code_assist_prefers_the_paid_tier() {
        let response = json!({
            "paidTier": { "name": "Google AI Pro" },
            "currentTier": { "id": "standard-tier", "name": "Standard" },
            "cloudaicompanionProject": "proj-123",
        });
        assert_eq!(
            code_assist(&response),
            CodeAssist {
                plan: Some("Google AI Pro".into()),
                tier: Some("standard-tier".into()),
                project: Some("proj-123".into()),
            }
        );
    }

    #[test]
    fn code_assist_falls_back_and_reads_project_objects() {
        let response = json!({
            "currentTier": { "id": "free-tier", "name": "Free" },
            "cloudaicompanionProject": { "id": "proj-obj" },
        });
        let parsed = code_assist(&response);
        assert_eq!(parsed.plan.as_deref(), Some("Free"));
        assert_eq!(parsed.project.as_deref(), Some("proj-obj"));
        assert_eq!(code_assist(&json!({})), CodeAssist::default());
        assert_eq!(code_assist(&json!({ "cloudaicompanionProject": "" })).project, None);
    }

    #[test]
    fn project_bodies_are_json() {
        assert_eq!(project_body(Some("p-1")), r#"{"project":"p-1"}"#);
        assert_eq!(project_body(Some("a\"b")), r#"{"project":"a\"b"}"#);
        assert_eq!(project_body(None), "{}");
    }

    #[test]
    fn summary_reads_groups_and_sorts_windows() {
        let response = json!({
            "groups": [{
                "displayName": "Claude and GPT models",
                "description": "Shared limits",
                "buckets": [
                    { "bucketId": "w", "displayName": "Weekly", "window": "weekly",
                      "remainingFraction": 0.41, "resetTime": "2026-11-26T08:00:00Z" },
                    { "bucketId": "x", "window": "monthly" },
                    { "bucketId": "d", "window": "DAILY", "remainingFraction": "0.5" },
                    { "bucketId": "f", "displayName": "5 hours", "window": "5h",
                      "remainingFraction": 0.82, "resetTime": "2026-11-23T16:00:00Z" },
                ],
            }, {}],
        });
        let groups = summary(&response);
        assert_eq!(groups.len(), 2);
        let ids: Vec<_> = groups[0].buckets.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["f", "d", "w", "x"]);
        assert_eq!(groups[0].name, "Claude and GPT models");
        assert_eq!(groups[0].description, "Shared limits");
        assert_eq!(groups[0].buckets[0].fraction, 0.82);
        assert_eq!(groups[0].buckets[1].fraction, 0.5);
        assert_eq!(groups[0].buckets[3].fraction, 0.0, "missing means empty");
        assert_eq!(groups[0].buckets[0].reset, Some("2026-11-23T16:00:00Z".parse().unwrap()));
        assert!(groups[1].buckets.is_empty());
        assert!(summary(&json!({})).is_empty());
    }

    #[test]
    fn models_follow_antigravitys_order_and_merge_display_names() {
        let response = json!({
            "agentModelSorts": [{ "groups": [{ "modelIds": ["gemini-3-pro", "claude-4", 7] }] }],
            "models": {
                "claude-4": { "displayName": "Claude 4", "quotaInfo": { "remainingFraction": 0.5 } },
                "zeta": { "displayName": "Zeta", "quotaInfo": {} },
                "alpha": { "displayName": "Alpha", "quotaInfo": { "remainingFraction": 1 } },
                "gemini-3-pro": {
                    "displayName": "Gemini 3 Pro", "recommended": true,
                    "quotaInfo": { "remainingFraction": 0.9, "resetTime": "2026-11-23T20:00:00Z" },
                },
                "gemini-3-pro-high": {
                    "displayName": "gemini 3 pro",
                    "quotaInfo": { "remainingFraction": 0.3 },
                },
                "internal": { "displayName": "Internal", "isInternal": true, "quotaInfo": {} },
                "no-quota": { "displayName": "No quota" },
                "no-name": { "quotaInfo": {} },
            },
        });
        let parsed = models(&response);
        let names: Vec<_> = parsed.iter().map(|m| m.display_name.as_str()).collect();
        assert_eq!(names, ["gemini 3 pro", "Claude 4", "Alpha", "Zeta"]);
        let gemini = &parsed[0];
        assert_eq!(gemini.id, "gemini-3-pro-high", "the most limited id wins");
        assert_eq!(gemini.fraction, 0.3);
        assert!(gemini.recommended, "recommended if either was");
        assert_eq!(gemini.sort_index, 0, "the best position is kept");
        assert_eq!(parsed[3].fraction, 0.0);
    }

    #[test]
    fn a_merge_keeps_the_first_on_ties_and_the_lower_fraction_otherwise() {
        let response = json!({
            "models": {
                "b": { "displayName": "Same", "quotaInfo": { "remainingFraction": 0.5 } },
                "a": { "displayName": "Same", "quotaInfo": { "remainingFraction": 0.5 } },
                "c": { "displayName": "Other", "quotaInfo": { "remainingFraction": 0.2 } },
                "d": { "displayName": "Other", "quotaInfo": { "remainingFraction": 0.7 } },
            },
        });
        let parsed = models(&response);
        assert_eq!(parsed.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["c", "b"]);
    }

    #[test]
    fn a_single_sort_object_also_gives_the_order() {
        let response = json!({
            "agentModelSorts": { "groups": [{ "modelIds": ["b", "a"] }] },
            "models": {
                "a": { "displayName": "A", "quotaInfo": {} },
                "b": { "displayName": "B", "quotaInfo": {} },
            },
        });
        let ids: Vec<_> = models(&response).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["b", "a"]);
        assert!(models(&json!({ "models": [] })).is_empty());
        assert!(models(&json!({ "agentModelSorts": 5, "models": {} })).is_empty());
    }
}
