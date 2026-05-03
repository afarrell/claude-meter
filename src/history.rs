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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_recovers_cycles_and_buckets() {
        let json = r#"{"cycles":[{"reset":1700000000,"buckets":[1,2,3,4,5,6,7]}]}"#;
        let h = History::parse(json).unwrap();
        assert_eq!(h.cycles.len(), 1);
        assert_eq!(h.cycles[0].reset, 1_700_000_000);
        assert_eq!(
            h.cycles[0].buckets,
            [Some(1), Some(2), Some(3), Some(4), Some(5), Some(6), Some(7)]
        );
    }

    #[test]
    fn parse_handles_null_buckets() {
        let json = r#"{"cycles":[{"reset":1,"buckets":[null,2,null,4,null,6,null]}]}"#;
        let h = History::parse(json).unwrap();
        assert_eq!(
            h.cycles[0].buckets,
            [None, Some(2), None, Some(4), None, Some(6), None]
        );
    }

    #[test]
    fn parse_empty_object_is_empty_history() {
        let h = History::parse("{}").unwrap();
        assert!(h.cycles.is_empty());
    }

    #[test]
    fn to_json_round_trips() {
        let h = History {
            cycles: vec![Cycle {
                reset: 1_777_446_000,
                buckets: [Some(12), None, Some(22), None, None, Some(38), Some(54)],
            }],
        };
        let json = h.to_json();
        let h2 = History::parse(&json).unwrap();
        assert_eq!(h, h2);
    }

    #[test]
    fn to_json_emits_cycles_key_with_reset_value() {
        let h = History {
            cycles: vec![Cycle { reset: 42, buckets: [None; 7] }],
        };
        let json = h.to_json();
        assert!(json.contains("\"cycles\""), "json missing 'cycles' key: {json}");
        assert!(json.contains("42"), "json missing reset value: {json}");
    }
}
