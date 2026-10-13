//! Anthropic OAuth usage cache — parse `~/.cache/claude-usage.json`.
//!
//! This module is deliberately path-agnostic: callers do their own file I/O
//! and pass the parsed string in. That keeps the trust boundary explicit
//! (paths only come from `main()` env config or test fixtures — never from
//! stdin or network) and keeps this module trivially unit-testable.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiCache {
    pub five_hour: Window,
    pub seven_day: Window,
    /// The newer `limits[]` array. Absent on older cache files and on
    /// accounts without model-scoped limits — always optional.
    #[serde(default)]
    pub limits: Vec<Limit>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Window {
    pub utilization: f64,
    /// May be absent or null for windows the API hasn't measured yet.
    #[serde(default)]
    pub resets_at: Option<DateTime<Utc>>,
}

/// One entry of the API's `limits[]` array. Only the fields the renderer
/// needs are modelled; everything else is ignored.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Limit {
    /// `"session"`, `"weekly_all"`, `"weekly_scoped"`, …
    pub kind: String,
    #[serde(default)]
    pub percent: f64,
    #[serde(default)]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scope: Option<Scope>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Scope {
    #[serde(default)]
    pub model: Option<ModelScope>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ModelScope {
    #[serde(default)]
    pub display_name: Option<String>,
}

/// The kind the API uses for a per-model weekly cap (e.g. Fable's
/// half-of-subscription allowance, 2026-09).
pub const WEEKLY_SCOPED_KIND: &str = "weekly_scoped";

impl ApiCache {
    /// Parse a cache JSON string. Used directly by tests; the binary's
    /// `main()` reads the file with the orchestration layer.
    pub fn parse(json: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(json)?)
    }

    /// The first model-scoped weekly limit, as a `Window`, if the account
    /// has one. `None` means "render exactly as before" — the scoped cell
    /// must never appear on accounts without a scoped cap.
    pub fn scoped_weekly(&self) -> Option<Window> {
        self.limits
            .iter()
            .find(|l| l.kind == WEEKLY_SCOPED_KIND)
            .map(|l| Window { utilization: l.percent, resets_at: l.resets_at })
    }

    /// Display name of the model the scoped weekly limit applies to, when
    /// the API reports one (e.g. `"Fable"`).
    pub fn scoped_weekly_label(&self) -> Option<&str> {
        self.limits
            .iter()
            .find(|l| l.kind == WEEKLY_SCOPED_KIND)
            .and_then(|l| l.scope.as_ref())
            .and_then(|s| s.model.as_ref())
            .and_then(|m| m.display_name.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WITH_SCOPED: &str = r#"{
        "five_hour": {"utilization": 36.0, "resets_at": "2026-09-09T12:00:00.133527+00:00"},
        "seven_day": {"utilization": 7.0, "resets_at": "2026-09-16T07:00:00.133553+00:00"},
        "seven_day_opus": null,
        "limits": [
            {"kind": "session", "group": "session", "percent": 36, "severity": "normal",
             "resets_at": "2026-09-09T12:00:00.133527+00:00", "scope": null, "is_active": true},
            {"kind": "weekly_all", "group": "weekly", "percent": 7, "severity": "normal",
             "resets_at": "2026-09-16T07:00:00.133553+00:00", "scope": null, "is_active": false},
            {"kind": "weekly_scoped", "group": "weekly", "percent": 14, "severity": "normal",
             "resets_at": "2026-09-16T07:00:00.133911+00:00",
             "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null},
             "is_active": false}
        ]
    }"#;

    #[test]
    fn scoped_weekly_found_from_limits_array() {
        let c = ApiCache::parse(WITH_SCOPED).unwrap();
        let w = c.scoped_weekly().expect("weekly_scoped present");
        assert_eq!(w.utilization, 14.0);
        assert_eq!(
            w.resets_at.unwrap().timestamp(),
            "2026-09-16T07:00:00+00:00".parse::<DateTime<Utc>>().unwrap().timestamp()
        );
        assert_eq!(c.scoped_weekly_label(), Some("Fable"));
        // Unscoped windows untouched.
        assert_eq!(c.seven_day.utilization, 7.0);
        assert_eq!(c.five_hour.utilization, 36.0);
    }

    #[test]
    fn scoped_weekly_none_when_limits_absent() {
        // Pre-`limits[]` cache shape (and older accounts): must parse and
        // report no scoped window so the layout stays byte-identical.
        let json = r#"{"five_hour":{"utilization":1.0,"resets_at":null},
                       "seven_day":{"utilization":2.0,"resets_at":null}}"#;
        let c = ApiCache::parse(json).unwrap();
        assert!(c.limits.is_empty());
        assert!(c.scoped_weekly().is_none());
        assert!(c.scoped_weekly_label().is_none());
    }

    #[test]
    fn scoped_weekly_none_when_only_unscoped_limits() {
        let json = r#"{"five_hour":{"utilization":1.0},"seven_day":{"utilization":2.0},
            "limits":[{"kind":"session","percent":1},{"kind":"weekly_all","percent":2}]}"#;
        let c = ApiCache::parse(json).unwrap();
        assert!(c.scoped_weekly().is_none());
    }

    #[test]
    fn scoped_weekly_tolerates_missing_scope_and_reset() {
        let json = r#"{"five_hour":{"utilization":1.0},"seven_day":{"utilization":2.0},
            "limits":[{"kind":"weekly_scoped","percent":55}]}"#;
        let c = ApiCache::parse(json).unwrap();
        let w = c.scoped_weekly().unwrap();
        assert_eq!(w.utilization, 55.0);
        assert!(w.resets_at.is_none());
        assert!(c.scoped_weekly_label().is_none());
    }

    #[test]
    fn scoped_weekly_picks_first_scoped_entry() {
        let json = r#"{"five_hour":{"utilization":1.0},"seven_day":{"utilization":2.0},
            "limits":[{"kind":"weekly_scoped","percent":10,"scope":{"model":{"display_name":"A"}}},
                      {"kind":"weekly_scoped","percent":90,"scope":{"model":{"display_name":"B"}}}]}"#;
        let c = ApiCache::parse(json).unwrap();
        assert_eq!(c.scoped_weekly().unwrap().utilization, 10.0);
        assert_eq!(c.scoped_weekly_label(), Some("A"));
    }
}
