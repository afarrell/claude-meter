//! Outer / integration tests — these encode bugs the bash version had.
//! Each test should fail end-to-end if the bug ever returns.
//!
//! `render` is pure (no I/O), so tests build the cache + history in
//! memory, render N times, and inspect the resulting history struct.

use chrono::{DateTime, TimeZone, Utc};
use claude_statusline::{render, ApiCache, History, StatuslineInput};

fn input(pct: u32) -> StatuslineInput {
    StatuslineInput {
        session_id: "test".into(),
        model_id: "claude-opus-4-7".into(),
        context_pct: pct,
        cwd: "/tmp".into(),
    }
}

fn cache_with_d7(pct: f64, resets_at: &str) -> ApiCache {
    ApiCache {
        five_hour: claude_statusline::Window {
            utilization: 0.0,
            resets_at: Some("2030-01-01T00:00:00+00:00".parse().unwrap()),
        },
        seven_day: claude_statusline::Window {
            utilization: pct,
            resets_at: Some(resets_at.parse().unwrap()),
        },
    }
}

/// Bug 2026-05-03 — "the growing first-day bump."
///
/// API drifts reset_ts forward by +1s past the stored cycle reset.
/// The bash implementation treated the stored value as `prev_reset`,
/// collapsed cycle_start to ~now, pinned idx to 0, and (combined with
/// the daily-max guard) caused bucket[0] to grow on every render.
#[test]
fn drift_does_not_collapse_idx_to_zero() {
    let cycle_reset_ts = 1_778_050_800_i64;
    let drifted_reset = "2026-05-06T07:00:01+00:00";
    let cache = cache_with_d7(51.0, drifted_reset);

    let mut history = History::parse(
        r#"{"cycles":[
            {"reset":1777446000,"buckets":[12,12,null,22,30,38,54]},
            {"reset":1778050800,"buckets":[null,30,44,50,null,null,null]}
        ]}"#,
    )
    .unwrap();

    // NOW = May 3 12:52:23 UTC = ~4.25 days into the cycle (idx should be 4).
    let now: DateTime<Utc> = Utc.with_ymd_and_hms(2026, 5, 3, 12, 52, 23).unwrap();

    for _ in 0..3 {
        render(&input(35), now, &cache, &mut history);
    }

    let last = history.cycles.last().unwrap();
    assert_eq!(last.reset, cycle_reset_ts);

    // bucket[0] must remain null — never written by these renders.
    assert!(
        last.buckets[0].is_none(),
        "bucket[0] grew under drift: expected null, got {:?}",
        last.buckets[0]
    );

    // bucket[4] (today) must hold 51.
    assert_eq!(
        last.buckets[4],
        Some(51),
        "current pct should land in bucket[4], got {:?}",
        last.buckets[4]
    );
}

/// Bug 2026-05-02 — "the impossible dip" / write-time max-guard.
#[test]
fn max_guard_keeps_daily_peak_across_intraday_renders() {
    let mut history = History::parse(
        r#"{"cycles":[
            {"reset":1777446000,"buckets":[54,54,54,54,54,54,54]},
            {"reset":1778050800,"buckets":[null,null,null,null,null,null,null]}
        ]}"#,
    )
    .unwrap();

    // NOW = May 1 14:00 UTC = day 2 of the cycle (idx=2).
    let now = Utc.with_ymd_and_hms(2026, 5, 1, 14, 0, 0).unwrap();

    // Three samples: high, dip, partial recovery.
    for pct in [42.0, 30.0, 38.0] {
        let cache = cache_with_d7(pct, "2026-05-06T07:00:00+00:00");
        render(&input(35), now, &cache, &mut history);
    }

    let last = history.cycles.last().unwrap();
    assert_eq!(
        last.buckets[2],
        Some(42),
        "max-guard failed: bucket[2] should be 42 (peak), got {:?}",
        last.buckets[2]
    );
}

/// Cycle rollover: when reset_ts shifts by more than the 60s tolerance,
/// a brand-new cycle entry must be appended.
#[test]
fn cycle_rollover_appends_new_entry() {
    let old_reset = 1_778_050_800_i64;
    let new_reset = old_reset + 7 * 86_400;

    let new_reset_iso = DateTime::<Utc>::from_timestamp(new_reset, 0)
        .unwrap()
        .to_rfc3339();
    let cache = cache_with_d7(5.0, &new_reset_iso);

    let mut history = History::parse(
        r#"{"cycles":[{"reset":1778050800,"buckets":[50,60,70,80,90,95,99]}]}"#,
    )
    .unwrap();

    // NOW must be within the new cycle's span (after old reset, before new reset).
    let now = Utc.with_ymd_and_hms(2026, 5, 8, 10, 0, 0).unwrap();
    render(&input(35), now, &cache, &mut history);

    assert_eq!(history.cycles.len(), 2, "rollover should append second cycle");
    assert_eq!(history.cycles[1].reset, new_reset);

    let nonnull: Vec<_> = history.cycles[1].buckets.iter().filter(|v| v.is_some()).collect();
    assert_eq!(
        nonnull.len(),
        1,
        "exactly one bucket should be written on first render of new cycle"
    );
    assert_eq!(nonnull[0], &Some(5));

    // Critical: new cycle must NOT inherit prior cycle's high bucket values.
    assert!(
        history.cycles[1].buckets.iter().filter(|v| v.is_some()).all(|v| v.unwrap() < 50),
        "new cycle inherited prior cycle's bucket values"
    );
}

/// Smoke test: the full render output is non-empty and starts with model
/// short-name. (Format details are tested at the unit level.)
#[test]
fn render_produces_nonempty_output_with_model_name() {
    let cache = cache_with_d7(40.0, "2030-01-01T00:00:00+00:00");
    let mut history = History::default();
    let now = Utc.with_ymd_and_hms(2026, 5, 3, 12, 0, 0).unwrap();
    let out = render(&input(35), now, &cache, &mut history);

    assert!(out.contains("opus"), "model short-name missing: {out:?}");
    assert!(!out.is_empty());
}
