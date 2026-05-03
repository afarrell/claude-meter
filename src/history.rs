//! Bucket history persistence — `~/.cache/claude-usage-history.json`.
//!
//! Like `cache.rs`, this module takes JSON strings, not paths. Path-handling
//! is the orchestration layer's responsibility.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct History {
    #[serde(default)]
    pub cycles: Vec<Cycle>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Cycle {
    /// Reset timestamp as epoch seconds.
    pub reset: i64,
    /// Per-day buckets. None = no data for that day.
    pub buckets: [Option<u8>; 7],
}

impl History {
    pub fn parse(json: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(json)?)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("history serializes")
    }
}
