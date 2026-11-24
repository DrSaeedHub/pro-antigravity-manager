//! The settings a person changes in the app, kept in `settings.json` in the app's data folder.
//! Missing or unreadable values fall back to their defaults.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use kernel::json;
use serde_json::{Value, json};

use crate::shell::DesktopError;

/// The allowed "counts as out of quota at" values, in percent.
pub const EMPTY_AT_PERCENT: RangeInclusive<u8> = 1..=50;
/// The allowed warning times before Antigravity restarts, in seconds (0 = no warning).
pub const WARNING_SECONDS: RangeInclusive<u16> = 0..=600;

/// What the person chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preferences {
    /// Switch to the account with the most left when the active one runs out.
    pub auto_switch: bool,
    /// A 5-hour or weekly limit at or below this percentage counts as out of quota.
    pub empty_at_percent: u8,
    /// How long the notice counts down before Antigravity restarts on its own.
    pub warning_seconds: u16,
    /// Whether the "still running in the tray" note was shown already.
    pub tray_hint_shown: bool,
    /// Look for a new version on GitHub every minute.
    pub check_updates: bool,
    /// The newest version the corner notice already announced (empty: none).
    pub notified_version: String,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            auto_switch: true,
            empty_at_percent: 5,
            warning_seconds: 60,
            tray_hint_shown: false,
            check_updates: true,
            notified_version: String::new(),
        }
    }
}

impl Preferences {
    /// The threshold as a fraction (0.05 for 5%).
    #[must_use]
    pub fn threshold(&self) -> f64 {
        f64::from(self.empty_at_percent) / 100.0
    }

    /// The same values, each moved into its allowed range.
    #[must_use]
    pub fn normalized(self) -> Self {
        Self {
            empty_at_percent: self
                .empty_at_percent
                .clamp(*EMPTY_AT_PERCENT.start(), *EMPTY_AT_PERCENT.end()),
            warning_seconds: self
                .warning_seconds
                .clamp(*WARNING_SECONDS.start(), *WARNING_SECONDS.end()),
            ..self
        }
    }

    /// Reads the JSON form; anything missing or wrong keeps its default.
    #[must_use]
    pub fn from_json(text: &str) -> Self {
        let defaults = Self::default();
        let Ok(root) = serde_json::from_str::<Value>(text) else { return defaults };
        let flag = |key: &str, default: bool| match json::get(&root, key) {
            Some(Value::Bool(value)) => *value,
            _ => default,
        };
        let whole = |key: &str| json::number(&root, key).filter(|n| n.fract() == 0.0 && *n >= 0.0);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped right after"
        )]
        let prefs = Self {
            auto_switch: flag("autoSwitch", defaults.auto_switch),
            empty_at_percent: whole("emptyAtPercent")
                .map_or(defaults.empty_at_percent, |n| n.min(255.0) as u8),
            warning_seconds: whole("warningSeconds")
                .map_or(defaults.warning_seconds, |n| n.min(65_535.0) as u16),
            tray_hint_shown: flag("trayHintShown", defaults.tray_hint_shown),
            check_updates: flag("checkUpdates", defaults.check_updates),
            notified_version: match json::get(&root, "notifiedVersion") {
                Some(Value::String(version)) => version.trim().to_owned(),
                _ => String::new(),
            },
        };
        prefs.normalized()
    }

    /// The JSON form.
    #[must_use]
    pub fn to_json(&self) -> String {
        let value = json!({
            "autoSwitch": self.auto_switch,
            "emptyAtPercent": self.empty_at_percent,
            "warningSeconds": self.warning_seconds,
            "trayHintShown": self.tray_hint_shown,
            "checkUpdates": self.check_updates,
            "notifiedVersion": self.notified_version,
        });
        serde_json::to_string_pretty(&value).unwrap_or_default()
    }
}

/// The settings file.
#[derive(Debug, Clone)]
pub struct PreferenceStore {
    path: PathBuf,
}

impl PreferenceStore {
    /// The file at `path`.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Where the file lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The saved settings, or the defaults when there are none.
    #[must_use]
    pub fn load(&self) -> Preferences {
        std::fs::read_to_string(&self.path)
            .map(|text| Preferences::from_json(&text))
            .unwrap_or_default()
    }

    /// Saves `preferences` (written next to the file, then moved over it).
    pub fn save(&self, preferences: &Preferences) -> Result<(), DesktopError> {
        if let Some(folder) = self.path.parent() {
            std::fs::create_dir_all(folder)
                .map_err(|error| DesktopError::file("creating the app folder", folder, &error))?;
        }
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, preferences.clone().normalized().to_json())
            .map_err(|error| DesktopError::file("saving the settings", &temporary, &error))?;
        std::fs::rename(&temporary, &self.path)
            .map_err(|error| DesktopError::file("saving the settings", &self.path, &error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_owners_choices() {
        let prefs = Preferences::default();
        assert!(prefs.auto_switch);
        assert_eq!(prefs.empty_at_percent, 5);
        assert_eq!(prefs.warning_seconds, 60);
        assert!(!prefs.tray_hint_shown);
        assert!(prefs.check_updates);
        assert_eq!(prefs.notified_version, "");
        assert!((prefs.threshold() - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn json_round_trips() {
        let prefs = Preferences {
            auto_switch: false,
            empty_at_percent: 12,
            warning_seconds: 0,
            tray_hint_shown: true,
            check_updates: false,
            notified_version: "1.2.0".into(),
        };
        assert_eq!(Preferences::from_json(&prefs.to_json()), prefs);
        assert!(prefs.to_json().contains("\"checkUpdates\": false"));
        assert!(prefs.to_json().contains("\"emptyAtPercent\": 12"));
    }

    #[test]
    fn bad_values_fall_back_or_clamp() {
        assert_eq!(Preferences::from_json("not json"), Preferences::default());
        assert_eq!(Preferences::from_json("{}"), Preferences::default());
        let odd = Preferences::from_json(
            r#"{"autoSwitch":"yes","emptyAtPercent":99,"warningSeconds":-4,"trayHintShown":1}"#,
        );
        assert_eq!(odd, Preferences { empty_at_percent: 50, ..Preferences::default() });
        assert_eq!(Preferences::from_json(r#"{"emptyAtPercent":0}"#).empty_at_percent, 1);
        assert_eq!(Preferences::from_json(r#"{"emptyAtPercent":2.5}"#).empty_at_percent, 5);
        assert_eq!(Preferences::from_json(r#"{"warningSeconds":99999}"#).warning_seconds, 600);
        assert_eq!(Preferences::from_json(r#"{"warningSeconds":"30"}"#).warning_seconds, 30);
        let odd = Preferences::from_json(r#"{"checkUpdates":"no","notifiedVersion":5}"#);
        assert!(odd.check_updates);
        assert_eq!(odd.notified_version, "");
    }

    #[test]
    fn the_store_saves_loads_and_survives_a_missing_or_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = PreferenceStore::new(dir.path().join("app").join("settings.json"));
        assert_eq!(store.load(), Preferences::default());
        let prefs = Preferences { empty_at_percent: 8, ..Preferences::default() };
        store.save(&prefs).unwrap();
        assert_eq!(store.load(), prefs);
        assert!(!store.path().with_extension("json.tmp").exists());
        std::fs::write(store.path(), b"\xff garbage").unwrap();
        assert_eq!(store.load(), Preferences::default());
    }

    #[test]
    fn saving_reports_problems() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"x").unwrap();
        let store = PreferenceStore::new(blocker.join("settings.json"));
        assert_eq!(store.save(&Preferences::default()).unwrap_err().code(), "desktop.file");
        let folder_in_the_way = PreferenceStore::new(dir.path().to_path_buf());
        assert_eq!(
            folder_in_the_way.save(&Preferences::default()).unwrap_err().code(),
            "desktop.file"
        );
    }
}
