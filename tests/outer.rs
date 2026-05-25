//! Outer / integration tests — these encode bugs the bash version had.
//! Each test should fail end-to-end if the bug ever returns.
//!
//! `render` is pure (no I/O), so tests build the cache + history in
//! memory, render N times, and inspect the resulting history struct.

use chrono::{DateTime, TimeZone, Utc};
use claude_meter::{render, ApiCache, History, StatuslineInput};

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
        five_hour: claude_meter::Window {
            utilization: 0.0,
            resets_at: Some("2030-01-01T00:00:00+00:00".parse().unwrap()),
        },
        seven_day: claude_meter::Window {
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

/// Intraday renders track the latest API reading — and write ONLY the
/// current bucket.
///
/// History: this test originally pinned a daily-peak max guard ("the
/// impossible dip", 2026-05-02 bash bug). That guard assumed a rolling
/// window with organic intraday dips. The window is in fact fixed-reset
/// (utilization within a cycle only grows), so a lower reading means
/// Anthropic changed the usage level and the bar must follow immediately
/// (2026-06-10 incident) — there is no organic peak to defend. The
/// invariant still worth pinning from the original bug: renders must
/// never write outside the current bucket.
#[test]
fn intraday_renders_track_latest_reading_in_current_bucket_only() {
    let mut history = History::parse(
        r#"{"cycles":[
            {"reset":1777446000,"buckets":[54,54,54,54,54,54,54]},
            {"reset":1778050800,"buckets":[null,null,null,null,null,null,null]}
        ]}"#,
    )
    .unwrap();

    // NOW = May 1 14:00 UTC = day 2 of the cycle (idx=2).
    let now = Utc.with_ymd_and_hms(2026, 5, 1, 14, 0, 0).unwrap();

    // Three samples: growth, externally-lowered level, regrowth.
    for pct in [42.0, 30.0, 38.0] {
        let cache = cache_with_d7(pct, "2026-05-06T07:00:00+00:00");
        render(&input(35), now, &cache, &mut history);
    }

    let last = history.cycles.last().unwrap();
    assert_eq!(
        last.buckets[2],
        Some(38),
        "bucket[2] should hold the latest reading, got {:?}",
        last.buckets[2]
    );
    let written = last.buckets.iter().filter(|b| b.is_some()).count();
    assert_eq!(written, 1, "renders must only ever write the current bucket");
}

/// Bug 2026-06-10 — "the stuck post-reset bar."
///
/// Anthropic zeroed the weekly usage level mid-cycle (resets_at
/// unchanged). The day's stored reading was 50; the API began reporting
/// 2. The old max guard pinned today's bucket at 50 for the rest of the
/// day, showing the pre-reset high instead of reality. Today's bucket
/// must follow the live value — and past days' buckets must keep their
/// recorded values.
#[test]
fn external_usage_reset_rebaselines_todays_bucket() {
    // Real data from the incident: cycle resets 2026-06-10T07:00Z.
    let mut history = History::parse(
        r#"{"cycles":[
            {"reset":1780470000,"buckets":[10,17,21,null,null,26,18]},
            {"reset":1781074800,"buckets":[15,25,27,28,null,37,50]}
        ]}"#,
    )
    .unwrap();

    // NOW = June 9 23:30 UTC — last day of the cycle (idx=6).
    let now = Utc.with_ymd_and_hms(2026, 6, 9, 23, 30, 0).unwrap();
    let cache = cache_with_d7(2.0, "2026-06-10T06:59:59+00:00");

    render(&input(35), now, &cache, &mut history);

    let last = history.cycles.last().unwrap();
    assert_eq!(
        last.buckets[6],
        Some(2),
        "today's bucket must re-baseline to the post-reset value, got {:?}",
        last.buckets[6]
    );
    assert_eq!(
        &last.buckets[..6],
        &[Some(15), Some(25), Some(27), Some(28), None, Some(37)],
        "past buckets must keep their recorded values"
    );
}

/// A small reset must show just as immediately as a large one: day one
/// of a cycle at 15%, Anthropic resets to 0. There is no drop-size
/// threshold — with a fixed-reset window, genuine usage never decreases
/// within a cycle, so any drop is an external level change.
#[test]
fn small_day_one_reset_drops_bucket_to_zero() {
    let mut history = History::parse(
        r#"{"cycles":[{"reset":1781679600,"buckets":[15,null,null,null,null,null,null]}]}"#,
    )
    .unwrap();

    // NOW = June 10 12:00 UTC — day 0 of the cycle resetting June 17 07:00Z.
    let now = Utc.with_ymd_and_hms(2026, 6, 10, 12, 0, 0).unwrap();
    let cache = cache_with_d7(0.0, "2026-06-17T07:00:00+00:00");

    render(&input(35), now, &cache, &mut history);

    let last = history.cycles.last().unwrap();
    assert_eq!(
        last.buckets[0],
        Some(0),
        "a 15-point day-one reset must drop the bucket to 0, got {:?}",
        last.buckets[0]
    );
}

/// Inter-day complement to the intra-day max-guard test above.
///
/// `apply_max_guard` keeps the *peak* within a single day's bucket, but it
/// must never reach across buckets. When usage genuinely drops from one day
/// to the next — e.g. a window reset, or a limit change that raises the
/// token ceiling so the same consumption reports a lower percentage — a
/// later day's bar must render *shorter* than an earlier, higher day's bar.
///
/// Concretely: 40% on day 0, then 20% on day 1, must render a tall bar
/// followed by a short bar — and the day-0 value must NOT be rewritten.
///
/// The regression this guards against: a future "fix" that makes the
/// sparkline monotonic (forward-fill or max-guard reaching across buckets),
/// turning it into a ratchet that can only ever climb.
#[test]
fn lower_usage_on_later_day_renders_shorter_bar_without_rewriting_history() {
    // Cycle: 2026-05-01T07:00 → 2026-05-08T07:00 (7 daily buckets).
    // reset epoch 1778223600 == 2026-05-08T07:00:00Z (must match cache reset).
    let reset = "2026-05-08T07:00:00+00:00";
    let cache = cache_with_d7(20.0, reset); // today's (lower) reading

    // Day 0 already recorded a 40% peak; days 1..6 unobserved.
    let mut history = History::parse(
        r#"{"cycles":[{"reset":1778223600,"buckets":[40,null,null,null,null,null,null]}]}"#,
    )
    .unwrap();

    // NOW = 2026-05-02T12:00 → ~1.2 days into the cycle → bucket idx 1.
    let now = Utc.with_ymd_and_hms(2026, 5, 2, 12, 0, 0).unwrap();
    let out = render(&input(35), now, &cache, &mut history);

    // History invariant: day 0's peak is preserved (no back-rewrite), and
    // day 1 records the genuinely lower reading rather than inheriting 40.
    let cyc = &history.cycles[0];
    assert_eq!(cyc.buckets[0], Some(40), "day 0 peak must not be rewritten");
    assert_eq!(
        cyc.buckets[1],
        Some(20),
        "day 1 must record the lower reading, not ratchet up to day 0"
    );

    // Render invariant: the earlier (higher) day's bar is taller than the
    // later (lower) day's bar. Compare spark-char heights, not raw chars.
    use claude_meter::bar::SPARK_CHARS;
    let d7 = strip_ansi(&out).rsplit(' ').next().unwrap().to_string();
    let sparks: Vec<char> = d7.chars().collect();
    let height = |c: char| SPARK_CHARS.iter().position(|&s| s == c).unwrap();
    assert_eq!(sparks[0], claude_meter::bar::bar(40), "day 0 char = bar(40)");
    assert_eq!(sparks[1], claude_meter::bar::bar(20), "day 1 char = bar(20)");
    assert!(
        height(sparks[0]) > height(sparks[1]),
        "earlier higher day must render taller than later lower day: {d7:?}"
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

/// Layout pin: `<model> <ctx-bar><h5-bar> <d7-sparkline>`.
/// Two spaces total — between model and the left meter pair, and between
/// that pair and the weekly sparkline. The ctx-bar and h5-bar sit flush
/// together so they read as a unified "current limits" group.
#[test]
fn layout_groups_left_meters_and_separates_d7() {
    let cache = cache_with_d7(40.0, "2030-01-01T00:00:00+00:00");
    let mut history = History::default();
    let now = Utc.with_ymd_and_hms(2026, 5, 3, 12, 0, 0).unwrap();
    let out = render(&input(35), now, &cache, &mut history);

    let plain = strip_ansi(&out);

    let parts: Vec<&str> = plain.split(' ').collect();
    assert_eq!(parts.len(), 3, "expected 3 space-separated groups, got {parts:?}");
    assert_eq!(parts[0], "opus");
    assert_eq!(
        parts[1].chars().count(),
        2,
        "left meter pair (ctx + h5, no space): {:?}",
        parts[1]
    );
    assert_eq!(parts[2].chars().count(), 7, "d7 sparkline is 7 chars: {:?}", parts[2]);

    assert!(
        !plain.contains("  "),
        "layout has stray double-space: {plain:?}"
    );
}

/// Strip ANSI escape sequences (color codes) from a string for layout testing.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}
