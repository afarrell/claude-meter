//! Usage limits from Claude Code itself: the `rate_limits` object in the
//! statusline stdin payload (https://code.claude.com/docs/en/statusline).
//!
//! It is the documented source, so the weekly total and the five-hour
//! window survive a change to the undocumented usage endpoint behind the
//! cache. It has no per-model cap, so the Fable sparkline still needs the
//! cache, and it is absent for API-key users and before a session's first
//! response.

use chrono::{TimeZone, Utc};
use serde_json::Value;

use crate::cycle::CYCLE_MATCH_TOLERANCE_S;
use crate::{ApiCache, Window};

/// The windows Claude Code reported on stdin. Each is independently absent.
#[derive(Debug, Clone, Default)]
pub struct RateLimits {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
}

/// Guard and parse `rate_limits` from the stdin payload.
///
/// Absent or null (the whole object, or either window) is a legitimate
/// state and gives `None`. A window that is present but doesn't have the
/// shape we read is a broken contract: the error names the field.
pub fn parse_rate_limits(payload: &Value) -> Result<RateLimits, String> {
    let Some(limits) = payload.get("rate_limits").filter(|v| !v.is_null()) else {
        return Ok(RateLimits::default());
    };
    let limits = limits.as_object().ok_or("stdin rate_limits is not an object")?;
    let window = |key: &str| -> Result<Option<Window>, String> {
        let Some(w) = limits.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
        let utilization = w
            .get("used_percentage")
            .and_then(Value::as_f64)
            .ok_or_else(|| format!("stdin rate_limits.{key}.used_percentage is not a number"))?;
        let resets_at = match w.get("resets_at").filter(|v| !v.is_null()) {
            None => None,
            Some(v) => Some(
                v.as_f64()
                    .and_then(|s| Utc.timestamp_opt(s as i64, 0).single())
                    .ok_or_else(|| format!("stdin rate_limits.{key}.resets_at is not epoch seconds"))?,
            ),
        };
        Ok(Some(Window { utilization, resets_at }))
    };
    Ok(RateLimits { five_hour: window("five_hour")?, seven_day: window("seven_day")? })
}

/// One window from its two sources.
///
/// Neither source is always the fresher one: the cache is at most a few
/// minutes old but comes from the endpoint, while the stdin figure dates
/// from this session's last response, which in an idle session can be
/// hours ago. So:
/// - different windows (resets more than the cycle tolerance apart): the
///   one that resets later is the current window;
/// - the same window: usage only rises within a window, so the higher
///   reading is the newer one. The cache's reset time is kept, so the
///   cycle history sees one stable value.
pub fn merge(cached: Option<&Window>, live: Option<&Window>) -> Option<Window> {
    let (c, l) = match (cached, live) {
        (None, None) => return None,
        (Some(w), None) | (None, Some(w)) => return Some(w.clone()),
        (Some(c), Some(l)) => (c, l),
    };
    if let (Some(cr), Some(lr)) = (c.resets_at, l.resets_at)
        && (cr.timestamp() - lr.timestamp()).abs() > CYCLE_MATCH_TOLERANCE_S
    {
        return Some(if lr > cr { l.clone() } else { c.clone() });
    }
    Some(Window { utilization: c.utilization.max(l.utilization), resets_at: c.resets_at.or(l.resets_at) })
}

impl ApiCache {
    /// This cache with the stdin figures merged into its unscoped windows.
    pub fn with_live(&self, live: &RateLimits) -> ApiCache {
        ApiCache {
            five_hour: merge(self.five_hour.as_ref(), live.five_hour.as_ref()),
            seven_day: merge(Some(&self.seven_day), live.seven_day.as_ref()).unwrap_or_else(|| self.seven_day.clone()),
            limits: self.limits.clone(),
        }
    }

    /// A stand-in for an unusable cache, from the stdin figures alone. It
    /// has no scoped cap. `None` when stdin carried no weekly window.
    pub fn from_live(live: &RateLimits) -> Option<ApiCache> {
        Some(ApiCache { five_hour: live.five_hour.clone(), seven_day: live.seven_day.clone()?, limits: vec![] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;
    use serde_json::json;

    const RESET: i64 = 1_794_380_400;

    fn w(pct: f64, reset: Option<i64>) -> Window {
        Window { utilization: pct, resets_at: reset.map(|r| Utc.timestamp_opt(r, 0).unwrap()) }
    }

    fn reset_of(w: &Window) -> Option<i64> {
        w.resets_at.map(|d: DateTime<Utc>| d.timestamp())
    }

    /// The payload Claude Code 2.1.322 sent on 2026-11-10 for a plan with
    /// no five-hour limit: only `seven_day`.
    #[test]
    fn parses_a_real_payload() {
        let payload = json!({"rate_limits": {"seven_day": {"used_percentage": 45, "resets_at": RESET}}});
        let r = parse_rate_limits(&payload).unwrap();
        assert!(r.five_hour.is_none());
        let d7 = r.seven_day.unwrap();
        assert_eq!((d7.utilization, reset_of(&d7)), (45.0, Some(RESET)));
    }

    #[test]
    fn absent_or_null_is_no_reading_not_an_error() {
        for payload in [json!({}), json!({"rate_limits": null}), json!({"rate_limits": {"seven_day": null}})] {
            let r = parse_rate_limits(&payload).unwrap();
            assert!(r.five_hour.is_none() && r.seven_day.is_none(), "{payload}");
        }
        let no_reset = json!({"rate_limits": {"five_hour": {"used_percentage": 23.5}}});
        let h5 = parse_rate_limits(&no_reset).unwrap().five_hour.unwrap();
        assert_eq!((h5.utilization, h5.resets_at), (23.5, None));
    }

    #[test]
    fn a_changed_shape_names_the_field() {
        let cases = [
            (json!({"rate_limits": []}), "stdin rate_limits is not an object"),
            (json!({"rate_limits": {"seven_day": {"utilization": 45}}}), "stdin rate_limits.seven_day.used_percentage is not a number"),
            (json!({"rate_limits": {"five_hour": {"used_percentage": "12"}}}), "stdin rate_limits.five_hour.used_percentage is not a number"),
            (json!({"rate_limits": {"seven_day": {"used_percentage": 1, "resets_at": "2026-11-11T07:00:00Z"}}}), "stdin rate_limits.seven_day.resets_at is not epoch seconds"),
        ];
        for (payload, want) in cases {
            assert_eq!(parse_rate_limits(&payload).unwrap_err(), want);
        }
    }

    #[test]
    fn merge_takes_the_only_source() {
        assert!(merge(None, None).is_none());
        assert_eq!(merge(Some(&w(40.0, Some(RESET))), None).unwrap().utilization, 40.0);
        assert_eq!(merge(None, Some(&w(45.0, Some(RESET)))).unwrap().utilization, 45.0);
    }

    /// Same window: the higher reading is the newer one, whichever side has
    /// it, and the cache's reset time is kept (the API's drifts by a second).
    #[test]
    fn merge_same_window_takes_the_higher_reading() {
        let live_ahead = merge(Some(&w(40.0, Some(RESET + 1))), Some(&w(45.0, Some(RESET)))).unwrap();
        assert_eq!((live_ahead.utilization, reset_of(&live_ahead)), (45.0, Some(RESET + 1)));
        // An idle session: its stdin figure is older than the cache.
        let cache_ahead = merge(Some(&w(60.0, Some(RESET))), Some(&w(45.0, Some(RESET)))).unwrap();
        assert_eq!(cache_ahead.utilization, 60.0);
        // Exactly at the tolerance is still the same window.
        let edge = merge(Some(&w(1.0, Some(RESET + CYCLE_MATCH_TOLERANCE_S))), Some(&w(2.0, Some(RESET)))).unwrap();
        assert_eq!(edge.utilization, 2.0);
        // No reset on one side: can't tell windows apart, so treat as the same.
        let no_reset = merge(Some(&w(10.0, Some(RESET))), Some(&w(12.0, None))).unwrap();
        assert_eq!((no_reset.utilization, reset_of(&no_reset)), (12.0, Some(RESET)));
    }

    /// Different windows: the later reset is the current window, even when
    /// its reading is lower (a new week starts near zero).
    #[test]
    fn merge_different_windows_takes_the_later_reset() {
        let week = 7 * 86_400;
        let stale_cache = merge(Some(&w(90.0, Some(RESET))), Some(&w(3.0, Some(RESET + week)))).unwrap();
        assert_eq!((stale_cache.utilization, reset_of(&stale_cache)), (3.0, Some(RESET + week)));
        let stale_session = merge(Some(&w(3.0, Some(RESET + week))), Some(&w(90.0, Some(RESET)))).unwrap();
        assert_eq!((stale_session.utilization, reset_of(&stale_session)), (3.0, Some(RESET + week)));
        let just_past = merge(Some(&w(9.0, Some(RESET))), Some(&w(1.0, Some(RESET + CYCLE_MATCH_TOLERANCE_S + 1)))).unwrap();
        assert_eq!(just_past.utilization, 1.0);
    }

    #[test]
    fn with_live_merges_unscoped_windows_and_keeps_the_scoped_cap() {
        let cache = ApiCache::parse(
            r#"{"five_hour":null,"seven_day":{"utilization":40.0,"resets_at":"2026-11-11T07:00:00+00:00"},
                "limits":[{"kind":"weekly_scoped","percent":30}]}"#,
        )
        .unwrap();
        let live = RateLimits { five_hour: Some(w(12.0, None)), seven_day: Some(w(45.0, Some(RESET))) };
        let merged = cache.with_live(&live);
        assert_eq!(merged.seven_day.utilization, 45.0);
        assert_eq!(merged.five_hour.as_ref().unwrap().utilization, 12.0);
        assert_eq!(merged.scoped_weekly().unwrap().utilization, 30.0);

        let untouched = cache.with_live(&RateLimits::default());
        assert_eq!(untouched.seven_day.utilization, 40.0);
        assert!(untouched.five_hour.is_none());
    }

    #[test]
    fn from_live_needs_the_weekly_window() {
        assert!(ApiCache::from_live(&RateLimits::default()).is_none());
        assert!(ApiCache::from_live(&RateLimits { five_hour: Some(w(1.0, None)), seven_day: None }).is_none());
        let c = ApiCache::from_live(&RateLimits { five_hour: None, seven_day: Some(w(45.0, Some(RESET))) }).unwrap();
        assert_eq!(c.seven_day.utilization, 45.0);
        assert!(c.scoped_weekly().is_none());
    }
}
