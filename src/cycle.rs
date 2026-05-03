//! Rolling-window cycle math.
//!
//! Anthropic's weekly limit is a rolling 168h window. Each "cycle" in this
//! module is bounded by two consecutive `reset_at` events from the API.
//! Within a cycle we partition time into 7 daily buckets and record each
//! day's peak utilization.
//!
//! The math here is the part of the bash script that had bugs — kept
//! deliberately small and pure so unit tests pin every branch.

use crate::history::{Cycle, History};

/// Cycles are considered "the same" if their reset timestamps are within
/// this many seconds. The Anthropic API drifts the reset timestamp by a
/// second or two between calls; without this slack we'd treat every drift
/// as a brand-new cycle.
pub const CYCLE_MATCH_TOLERANCE_S: i64 = 60;

/// Standard 7-day cycle length in seconds.
pub const SEVEN_DAYS_S: i64 = 7 * 86_400;

/// Find the start of the cycle that contains `reset_ts`.
///
/// Strategy: use the most recent prior cycle's reset as our cycle_start,
/// provided it's "clearly older" (more than CYCLE_MATCH_TOLERANCE_S
/// seconds before `reset_ts`) and within 7 days. Otherwise treat this
/// cycle as a cold start spanning `reset_ts - 7d` to `reset_ts`.
///
/// **Critical:** the `+ TOLERANCE` exclusion prevents the cycle's OWN
/// stored reset (jittered by the API by a second or two) from being
/// selected as "previous." That bug collapsed cycle_start to ~now,
/// pinned bucket index to 0, and combined with the daily-max guard
/// caused the leftmost bar to grow on every render. See the
/// `drift_does_not_collapse_idx_to_zero` outer test.
pub fn cycle_start_for_reset(reset_ts: i64, history: &History) -> i64 {
    let prev = history
        .cycles
        .iter()
        .map(|c| c.reset)
        .filter(|&r| r + CYCLE_MATCH_TOLERANCE_S < reset_ts)
        .max();

    match prev {
        Some(p) if reset_ts - p <= SEVEN_DAYS_S + CYCLE_MATCH_TOLERANCE_S => p,
        _ => reset_ts - SEVEN_DAYS_S,
    }
}

/// Compute the bucket index for `now` within a cycle running from
/// `cycle_start` to `cycle_start + cycle_len`. Returns 0..=6.
pub fn bucket_idx(now: i64, cycle_start: i64, cycle_len: i64) -> usize {
    let bucket_size = (cycle_len / 7).max(60);
    let elapsed = (now - cycle_start).max(0);
    ((elapsed / bucket_size) as usize).min(6)
}

/// Locate the cycle in history matching `reset_ts` within tolerance.
/// Returns the cycle's index in `history.cycles` if found.
pub fn match_cycle(reset_ts: i64, history: &History) -> Option<usize> {
    history
        .cycles
        .iter()
        .position(|c| (c.reset - reset_ts).abs() <= CYCLE_MATCH_TOLERANCE_S)
}

/// Update the bucket at `idx` with `pct` using the daily-peak rule:
/// keep `max(stored, pct)`. Returns the new value stored.
///
/// This is the per-day peak invariant. Multiple renders during a single
/// day can vary in the reported `pct` (rolling-window noise, mid-day
/// dips); we keep only the highest. See the
/// `max_guard_keeps_daily_peak_across_intraday_renders` outer test.
pub fn apply_max_guard(buckets: &mut [Option<u8>; 7], idx: usize, pct: u8) -> u8 {
    let new = match buckets[idx] {
        Some(stored) => stored.max(pct),
        None => pct,
    };
    buckets[idx] = Some(new);
    new
}

/// Append a brand-new cycle entry to history with `pct` written at `idx`.
pub fn append_new_cycle(history: &mut History, reset_ts: i64, idx: usize, pct: u8) {
    let mut buckets: [Option<u8>; 7] = [None; 7];
    buckets[idx] = Some(pct);
    history.cycles.push(Cycle { reset: reset_ts, buckets });
}

/// Forward-fill nulls in past positions for rendering. A null past bucket
/// means we made no observation that day; we inherit the prior day's value
/// as a lower bound rather than rendering a misleading zero.
///
/// Only operates on positions `1..=current_idx`. The "current" and "future"
/// positions are left untouched.
pub fn forward_fill(buckets: &mut [Option<u8>; 7], current_idx: usize) {
    for i in 1..=current_idx.min(6) {
        if buckets[i].is_none() {
            buckets[i] = buckets[i - 1];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Cycle;

    fn h(cycles: Vec<(i64, [Option<u8>; 7])>) -> History {
        History {
            cycles: cycles.into_iter().map(|(reset, buckets)| Cycle { reset, buckets }).collect(),
        }
    }

    // ---------- cycle_start_for_reset ----------

    #[test]
    fn cycle_start_uses_prev_reset_when_within_7d() {
        let history = h(vec![
            (1_777_446_000, [None; 7]), // April 29
        ]);
        // reset_ts = May 6, prev = April 29 (7d apart) → use prev
        let cs = cycle_start_for_reset(1_778_050_800, &history);
        assert_eq!(cs, 1_777_446_000);
    }

    #[test]
    fn cycle_start_falls_back_when_no_prior_cycle() {
        let history = History::default();
        let reset_ts = 1_778_050_800;
        let cs = cycle_start_for_reset(reset_ts, &history);
        assert_eq!(cs, reset_ts - SEVEN_DAYS_S);
    }

    #[test]
    fn cycle_start_falls_back_when_prev_more_than_7d_ago() {
        let history = h(vec![
            (1_700_000_000, [None; 7]), // ancient
        ]);
        let reset_ts = 1_778_050_800;
        let cs = cycle_start_for_reset(reset_ts, &history);
        assert_eq!(cs, reset_ts - SEVEN_DAYS_S);
    }

    /// **The 2026-05-03 bug, isolated to its smallest unit.**
    /// reset_ts drifts +1s past the stored cycle's reset.
    /// MUST NOT pick up the stored reset as "prev".
    #[test]
    fn cycle_start_excludes_stored_reset_within_drift_tolerance() {
        let history = h(vec![
            (1_777_446_000, [None; 7]),                 // real previous cycle
            (1_778_050_800, [None; 7]),                 // current cycle (will drift)
        ]);
        // API now reports reset_ts = stored + 1s drift.
        let drifted = 1_778_050_801;
        let cs = cycle_start_for_reset(drifted, &history);
        // Must use the REAL previous cycle, not the current cycle's own reset.
        assert_eq!(cs, 1_777_446_000);
    }

    #[test]
    fn cycle_start_excludes_stored_reset_at_exactly_60s_drift() {
        let history = h(vec![
            (1_777_446_000, [None; 7]),
            (1_778_050_800, [None; 7]),
        ]);
        // Drift of exactly 60s: still within tolerance, must exclude.
        let drifted = 1_778_050_800 + 60;
        let cs = cycle_start_for_reset(drifted, &history);
        assert_eq!(cs, 1_777_446_000, "60s drift should still exclude same-cycle reset");
    }

    #[test]
    fn cycle_start_includes_reset_just_past_drift_tolerance() {
        let history = h(vec![
            (1_777_446_000, [None; 7]),
            (1_778_050_800, [None; 7]),
        ]);
        // 61s past — outside tolerance, treat as a real prior cycle.
        let reset_ts = 1_778_050_800 + 61;
        let cs = cycle_start_for_reset(reset_ts, &history);
        assert_eq!(cs, 1_778_050_800);
    }

    #[test]
    fn cycle_start_tolerates_7d_gate_drift() {
        // prev_reset is exactly 7d + 30s before reset_ts (slack absorbs drift).
        let prev = 1_700_000_000;
        let reset_ts = prev + SEVEN_DAYS_S + 30;
        let history = h(vec![(prev, [None; 7])]);
        let cs = cycle_start_for_reset(reset_ts, &history);
        assert_eq!(cs, prev, "7d + 30s drift should still use prev_reset");
    }

    #[test]
    fn cycle_start_falls_back_just_past_7d_plus_tolerance() {
        // prev is past the (SEVEN_DAYS_S + CYCLE_MATCH_TOLERANCE_S) gate.
        // Pins the `+` between SEVEN_DAYS_S and CYCLE_MATCH_TOLERANCE_S
        // — a `*` mutation here would silently widen the gate to ~10 years.
        let prev = 1_700_000_000;
        let reset_ts = prev + SEVEN_DAYS_S + CYCLE_MATCH_TOLERANCE_S + 1;
        let history = h(vec![(prev, [None; 7])]);
        let cs = cycle_start_for_reset(reset_ts, &history);
        assert_eq!(
            cs,
            reset_ts - SEVEN_DAYS_S,
            "should cold-start when prev is past 7d + tolerance gate"
        );
    }

    #[test]
    fn cycle_start_uses_prev_at_exact_7d_plus_tolerance() {
        // Boundary: exactly at 7d + tolerance still counts as in-window.
        let prev = 1_700_000_000;
        let reset_ts = prev + SEVEN_DAYS_S + CYCLE_MATCH_TOLERANCE_S;
        let history = h(vec![(prev, [None; 7])]);
        let cs = cycle_start_for_reset(reset_ts, &history);
        assert_eq!(cs, prev, "exact boundary value should still use prev_reset");
    }

    // ---------- bucket_idx ----------

    #[test]
    fn bucket_idx_zero_at_cycle_start() {
        assert_eq!(bucket_idx(0, 0, SEVEN_DAYS_S), 0);
    }

    #[test]
    fn bucket_idx_clamps_to_six_at_cycle_end() {
        assert_eq!(bucket_idx(SEVEN_DAYS_S, 0, SEVEN_DAYS_S), 6);
        assert_eq!(bucket_idx(SEVEN_DAYS_S * 2, 0, SEVEN_DAYS_S), 6);
    }

    #[test]
    fn bucket_idx_clamps_to_zero_when_negative_elapsed() {
        // NOW < cycle_start (clock skew or stale cache): treat as day 0.
        assert_eq!(bucket_idx(-100, 0, SEVEN_DAYS_S), 0);
    }

    #[test]
    fn bucket_idx_partitions_seven_days_correctly() {
        // 4.25 days into a 7-day cycle → day 4 (idx=4).
        let elapsed = 4 * 86_400 + 86_400 / 4;
        assert_eq!(bucket_idx(elapsed, 0, SEVEN_DAYS_S), 4);
    }

    #[test]
    fn bucket_idx_floors_minimum_bucket_size() {
        // Pathological cycle_len=1 — bucket_size clamped to 60.
        // (Verifies the clamp; if removed, idx could overflow.)
        let idx = bucket_idx(120, 0, 1);
        assert_eq!(idx, 2); // 120 / 60 = 2
    }

    // ---------- match_cycle ----------

    #[test]
    fn match_cycle_finds_within_60s_either_direction() {
        let history = h(vec![
            (1_000, [None; 7]),
            (1_000_000, [None; 7]),
        ]);
        assert_eq!(match_cycle(1_000_000, &history), Some(1));
        assert_eq!(match_cycle(1_000_060, &history), Some(1)); // +60s
        assert_eq!(match_cycle(999_940, &history), Some(1));    // -60s
    }

    #[test]
    fn match_cycle_misses_just_outside_tolerance() {
        let history = h(vec![(1_000_000, [None; 7])]);
        assert_eq!(match_cycle(1_000_061, &history), None);
        assert_eq!(match_cycle(999_939, &history), None);
    }

    #[test]
    fn match_cycle_returns_none_for_empty_history() {
        assert_eq!(match_cycle(1_000_000, &History::default()), None);
    }

    // ---------- apply_max_guard ----------

    #[test]
    fn max_guard_writes_pct_when_bucket_empty() {
        let mut buckets: [Option<u8>; 7] = [None; 7];
        apply_max_guard(&mut buckets, 3, 42);
        assert_eq!(buckets[3], Some(42));
    }

    #[test]
    fn max_guard_keeps_higher_stored_value() {
        let mut buckets: [Option<u8>; 7] = [None; 7];
        buckets[3] = Some(50);
        apply_max_guard(&mut buckets, 3, 30);
        assert_eq!(buckets[3], Some(50), "lower pct must NOT clobber higher peak");
    }

    #[test]
    fn max_guard_overwrites_lower_stored_value() {
        let mut buckets: [Option<u8>; 7] = [None; 7];
        buckets[3] = Some(30);
        apply_max_guard(&mut buckets, 3, 50);
        assert_eq!(buckets[3], Some(50));
    }

    #[test]
    fn max_guard_holds_on_equal_value() {
        let mut buckets: [Option<u8>; 7] = [None; 7];
        buckets[3] = Some(42);
        apply_max_guard(&mut buckets, 3, 42);
        assert_eq!(buckets[3], Some(42));
    }

    // ---------- append_new_cycle ----------

    #[test]
    fn append_new_cycle_creates_fresh_entry_at_end() {
        let mut history = h(vec![(1_000_000, [Some(50); 7])]);
        append_new_cycle(&mut history, 2_000_000, 3, 25);
        assert_eq!(history.cycles.len(), 2);
        assert_eq!(history.cycles[1].reset, 2_000_000);
        assert_eq!(history.cycles[1].buckets[3], Some(25));
        assert_eq!(history.cycles[1].buckets[0], None,
            "new cycle must NOT inherit prior cycle's bucket values");
    }

    // ---------- forward_fill ----------

    #[test]
    fn forward_fill_inherits_through_nulls() {
        let mut buckets: [Option<u8>; 7] = [Some(20), None, None, Some(30), None, None, None];
        forward_fill(&mut buckets, 4);
        assert_eq!(buckets, [Some(20), Some(20), Some(20), Some(30), Some(30), None, None]);
    }

    #[test]
    fn forward_fill_does_not_touch_future_positions() {
        let mut buckets: [Option<u8>; 7] = [Some(50); 7];
        buckets[5] = None;
        buckets[6] = None;
        forward_fill(&mut buckets, 3); // current_idx = 3
        assert_eq!(buckets[5], None, "positions past current_idx untouched");
        assert_eq!(buckets[6], None);
    }

    #[test]
    fn forward_fill_leaves_leading_null_alone() {
        let mut buckets: [Option<u8>; 7] = [None, Some(20), None, None, None, None, None];
        forward_fill(&mut buckets, 3);
        // bucket[0] stays None (no prior to inherit from); bucket[1]=20 stays;
        // bucket[2,3] inherit 20.
        assert_eq!(buckets, [None, Some(20), Some(20), Some(20), None, None, None]);
    }
}
