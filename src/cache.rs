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
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Window {
    pub utilization: f64,
    /// May be absent or null for windows the API hasn't measured yet.
    #[serde(default)]
    pub resets_at: Option<DateTime<Utc>>,
}

impl ApiCache {
    /// Parse a cache JSON string. Used directly by tests; the binary's
    /// `main()` reads the file with the orchestration layer.
    pub fn parse(json: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(json)?)
    }
}
