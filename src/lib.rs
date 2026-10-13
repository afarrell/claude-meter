//! Claude Code statusline renderer — D7 sparkline + H5 bar + context bar,
//! plus an optional second sparkline for a model-scoped weekly cap (Fable).
//!
//! Pure rendering core. All file I/O lives in `main.rs` so this library is
//! trivially testable (string-in, string-out) and immune to filesystem
//! surface area in security review.

use chrono::{DateTime, Utc};

pub mod bar;
pub mod cache;
pub mod cycle;
pub mod history;
pub mod state;

pub use cache::{ApiCache, Limit, ModelScope, Scope, Window};
pub use history::{Cycle, History};
pub use state::{snapshot, Snapshot};

use state::{FiveHour, Weekly};

/// Parsed Claude Code statusline stdin payload.
#[derive(Debug, Clone)]
pub struct StatuslineInput {
    pub session_id: String,
    pub model_id: String,
    pub context_pct: u32,
    pub cwd: String,
    /// Columns available to this output (terminal width minus whatever the
    /// wrapper appends). `None` means unknown: always render one line.
    pub max_width: Option<usize>,
}

/// Render the full statusline. Mutates `history` in place to record the
/// current observation; caller is responsible for persisting it.
///
/// `now` is injected for deterministic testing. The binary's `main` calls
/// this with `Utc::now()`.
pub fn render(
    input: &StatuslineInput,
    now: DateTime<Utc>,
    cache: &ApiCache,
    history: &mut History,
) -> String {
    let now_ts = now.timestamp();

    let d7_out = render_d7(&cache.seven_day, history, now_ts);
    let h5_out = render_h5(&cache.five_hour, now_ts);
    // Model-scoped weekly cap (Fable's half-allowance) as its own sparkline.
    // Absent on accounts without one: the line is then the pre-scoped layout.
    let scoped_out = cache
        .scoped_weekly()
        .map(|w| format!("{}{}", render_scoped(&cache.seven_day, &w, history, now_ts), bar::RESET));

    let ctx_pct = input.context_pct.min(100) as u8;
    let model_short = short_model(&input.model_id);
    let ctx_out = format!("{}{} {}", bar::ctx_color(ctx_pct), model_short, bar::bar(ctx_pct));

    let line = format!("{}{}{}{} {}{}", ctx_out, bar::RESET, h5_out, bar::RESET, d7_out, bar::RESET);
    let Some(scoped_out) = scoped_out else { return line };

    // `<model> <ctx><h5> ` precedes the D7 sparkline.
    let d7_col = model_short.chars().count() + 4;
    let one_line_width = d7_col + 7 + 1 + 7;
    match input.max_width {
        // Too narrow for both sparklines: Fable wraps onto a second row,
        // day-aligned under the total sparkline.
        Some(w) if one_line_width > w => format!("{line}\n{}{scoped_out}", " ".repeat(d7_col)),
        _ => format!("{line} {scoped_out}"),
    }
}

/// Emit 7 spark cells: DIM on-pace baseline for days without data, the
/// `today` colour for the current bucket, `past` for earlier observed days.
fn spark_cells(display: &[Option<u8>; 7], idx: usize, today: &str, past: &str) -> String {
    let mut out = String::new();
    for (i, cell) in display.iter().enumerate() {
        match cell {
            None => {
                let expected = ((i + 1) * 100 / 7) as u8;
                out.push_str(bar::DIM);
                out.push(bar::bar(expected));
            }
            Some(v) if i == idx => {
                out.push_str(today);
                out.push(bar::bar(*v));
            }
            Some(v) => {
                out.push_str(past);
                out.push(bar::bar(*v));
            }
        }
    }
    out
}

/// Render the D7 sparkline and update history with the current observation.
fn render_d7(window: &Window, history: &mut History, now_ts: i64) -> String {
    match state::weekly(window, history, now_ts) {
        Weekly::NoReset { pct } => format!("{}{}", bar::PAST, bar::bar(pct)),
        // Stale: reset is in the past (cache wasn't refreshed before rollover).
        // Render the dim baseline in GREY — we don't know current bucket state, so
        // showing fabricated full red bars (the prior behavior) misleads the user.
        Weekly::Stale { .. } => format!("{}·······", bar::GREY),
        Weekly::Live(w) => {
            let delta = w.pace();
            let current_color = if w.pct >= 90 || delta > 30 {
                bar::RED_BOLD
            } else if delta > 10 {
                bar::YELLOW_BOLD
            } else {
                "\x1b[0m"
            };
            spark_cells(&w.daily, w.today, current_color, bar::PAST)
        }
    }
}

/// Render the H5 five-hour bar with pace coloring.
fn render_h5(window: &Window, now_ts: i64) -> String {
    match state::five_hour(window, now_ts) {
        FiveHour::NoReset { pct } => format!("{}{}", bar::PAST, bar::bar(pct)),
        // Stale: window rolled over before the cache could refresh. Render the
        // last known utilization in GREY — honest "stale, may be out of date"
        // signal instead of a fabricated full red bar that lies about usage.
        FiveHour::Stale { pct } => format!("{}{}", bar::GREY, bar::bar(pct)),
        FiveHour::Live { pct, .. } if pct >= 90 => format!("{}{}", bar::RED_BOLD, bar::bar(pct)),
        FiveHour::Live { pct, elapsed_pct } => {
            let delta_clamped = (pct as i32 - elapsed_pct).clamp(0, 100);
            format!("{}{}", bar::pace_color(delta_clamped), bar::bar(pct))
        }
    }
}

/// Render the model-scoped weekly cap (Fable) as a 7-day sparkline and
/// record today's observation in `history`.
///
/// Shares the D7 cycle and day grid (`render_d7` must run first so the
/// cycle exists); see `state::scoped`.
fn render_scoped(d7: &Window, scoped: &Window, history: &mut History, now_ts: i64) -> String {
    match state::scoped(d7, scoped, history, now_ts) {
        Weekly::NoReset { pct } => format!("{}{}", bar::PAST, bar::bar(pct)),
        // Stale (cache not refreshed across the reset): same honesty rule as D7.
        Weekly::Stale { .. } => format!("{}·······", bar::GREY),
        // Same colouring policy as the total: grey past days, plain today unless
        // ahead of pace. Purple (not yellow) marks ahead-of-pace for Fable.
        Weekly::Live(w) => spark_cells(&w.daily, w.today, bar::scoped_color(w.pct, w.pace()), bar::PAST),
    }
}

/// Strip "claude-" prefix and version suffix from a full model ID.
/// Matches bash: `sed 's/claude-//;s/-[0-9].*//;s/-latest//'`.
pub fn short_model(id: &str) -> String {
    let s = id.strip_prefix("claude-").unwrap_or(id);
    // Truncate at the first "-<digit>" we find — that begins the version tail.
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == b'-' && bytes[i + 1].is_ascii_digit() {
            return s[..i].trim_end_matches("-latest").to_string();
        }
    }
    s.trim_end_matches("-latest").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn short_model_strips_claude_prefix_and_version() {
        assert_eq!(short_model("claude-opus-4-7"), "opus");
        assert_eq!(short_model("claude-sonnet-4-6"), "sonnet");
        assert_eq!(short_model("claude-haiku-4-5-20251001"), "haiku");
        assert_eq!(short_model("claude-opus-4-7-latest"), "opus");
        assert_eq!(short_model("gpt-4"), "gpt");
        assert_eq!(short_model("custom-model-name"), "custom-model-name");
    }

    fn window(pct: f64, resets_at: Option<DateTime<Utc>>) -> Window {
        Window { utilization: pct, resets_at }
    }

    fn ts(y: i32, m: u32, d: u32, h: u32) -> i64 {
        Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap().timestamp()
    }

    // ---------- render_d7 stale branch ----------

    #[test]
    fn render_d7_stale_idle_renders_dim_dots() {
        let reset = ts(2026, 5, 1, 0);
        let w = window(0.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let mut h = History::default();
        // now > reset and pct == 0 → dim "·······"
        let out = render_d7(&w, &mut h, reset + 3600);
        assert!(out.contains('·'), "expected dim dots for stale+idle: {out:?}");
        assert!(out.starts_with(bar::GREY), "expected GREY prefix: {out:?}");
    }

    #[test]
    fn render_d7_stale_with_pct_renders_grey_dots_not_red_full() {
        let reset = ts(2026, 5, 1, 0);
        let w = window(42.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let mut h = History::default();
        // now > reset → dim dots in GREY regardless of pct (no fake-max bar).
        let out = render_d7(&w, &mut h, reset + 3600);
        assert!(out.contains('·'), "expected dim dots for stale: {out:?}");
        assert!(!out.contains('█'), "must not render fake-max blocks: {out:?}");
        assert!(out.starts_with(bar::GREY), "expected GREY prefix: {out:?}");
        assert!(!out.contains(bar::RED_BOLD), "must not be RED: {out:?}");
    }

    #[test]
    fn render_d7_at_exact_reset_is_not_stale() {
        // Boundary: now == reset is NOT stale (uses '>' not '>=').
        let reset = ts(2026, 5, 1, 0);
        let w = window(50.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let mut h = History::default();
        let out = render_d7(&w, &mut h, reset);
        assert!(!out.contains('·'), "now == reset should not trigger stale path: {out:?}");
    }

    // ---------- render_d7 current vs past coloring ----------

    /// Verifies the `i == idx` match guard: only the current bucket gets the
    /// "current" color tier; past buckets get PAST grey; nulls get DIM.
    /// A `true` mutation would color every bucket as current; `false` would
    /// color even today's bucket as past.
    #[test]
    fn render_d7_only_current_bucket_uses_current_color() {
        let cycle_start = ts(2026, 4, 29, 0);
        let reset = cycle_start + 7 * 86_400;
        let now = cycle_start + 3 * 86_400 + 3600; // ~day 3 of cycle, idx=3

        let w = window(40.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let mut h = History {
            cycles: vec![Cycle {
                reset,
                buckets: [Some(10), Some(20), Some(30), Some(40), None, None, None],
                scoped: [None; 7],
            }],
        };
        let out = render_d7(&w, &mut h, now);

        // PAST grey (241) appears for buckets 0,1,2 (the past).
        let past_count = out.matches(bar::PAST).count();
        assert_eq!(past_count, 3, "expected exactly 3 PAST-colored cells: {out:?}");

        // DIM (238) appears for buckets 4,5,6 (future/null).
        let dim_count = out.matches(bar::DIM).count();
        assert_eq!(dim_count, 3, "expected exactly 3 DIM cells (future): {out:?}");
    }

    // ---------- render_d7 pace / current_color tiers ----------

    #[test]
    fn render_d7_current_color_red_when_pct_at_or_above_90() {
        let cycle_start = ts(2026, 4, 29, 0);
        let reset = cycle_start + 7 * 86_400;
        let now = cycle_start + 86_400;
        let w = window(90.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let mut h = History {
            cycles: vec![Cycle { reset, buckets: [None; 7], scoped: [None; 7] }],
        };
        let out = render_d7(&w, &mut h, now);
        assert!(out.contains(bar::RED_BOLD),
            "pct=90 should produce RED current_color: {out:?}");
    }

    #[test]
    fn render_d7_current_color_red_when_delta_above_30() {
        // pct=50 very early in cycle → delta > 30 → RED.
        let cycle_start = ts(2026, 4, 29, 0);
        let reset = cycle_start + 7 * 86_400;
        let now = cycle_start + 60_000;
        let w = window(50.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let mut h = History {
            cycles: vec![Cycle { reset, buckets: [None; 7], scoped: [None; 7] }],
        };
        let out = render_d7(&w, &mut h, now);
        assert!(out.contains(bar::RED_BOLD), "delta>30 should be RED: {out:?}");
    }

    #[test]
    fn render_d7_current_color_yellow_when_delta_in_11_to_30() {
        // 4d in: elapsed_pct ~ 57. pct = 75 → delta = 18 (yellow).
        let cycle_start = ts(2026, 4, 29, 0);
        let reset = cycle_start + 7 * 86_400;
        let now = cycle_start + 4 * 86_400;
        let w = window(75.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let mut h = History {
            cycles: vec![Cycle { reset, buckets: [None; 7], scoped: [None; 7] }],
        };
        let out = render_d7(&w, &mut h, now);
        assert!(out.contains(bar::YELLOW_BOLD),
            "delta in (10, 30] should be YELLOW: {out:?}");
        assert!(!out.contains(bar::RED_BOLD), "should not be RED: {out:?}");
    }

    #[test]
    fn render_d7_current_color_default_when_on_pace() {
        let cycle_start = ts(2026, 4, 29, 0);
        let reset = cycle_start + 7 * 86_400;
        let now = cycle_start + 3 * 86_400 + 12 * 3600; // ~50% through cycle
        let w = window(50.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let mut h = History {
            cycles: vec![Cycle { reset, buckets: [None; 7], scoped: [None; 7] }],
        };
        let out = render_d7(&w, &mut h, now);
        assert!(!out.contains(bar::RED_BOLD), "on-pace should not be RED: {out:?}");
        assert!(!out.contains(bar::YELLOW_BOLD), "on-pace should not be YELLOW: {out:?}");
    }

    // ---------- render_h5 ----------

    #[test]
    fn render_h5_stale_idle_renders_grey_lowest_bar() {
        let reset = ts(2026, 5, 1, 0);
        let w = window(0.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let out = render_h5(&w, reset + 3600);
        assert_eq!(out, format!("{}{}", bar::GREY, bar::bar(0)));
    }

    #[test]
    fn render_h5_stale_with_pct_renders_last_known_in_grey() {
        // Stale + pct=42 must render the last-known bar height in GREY,
        // not a fabricated red full bar at 100%. Lying about utilization
        // (the prior behavior) panicked users when their actual usage was low.
        let reset = ts(2026, 5, 1, 0);
        let w = window(42.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let out = render_h5(&w, reset + 3600);
        assert_eq!(out, format!("{}{}", bar::GREY, bar::bar(42)));
        assert!(!out.contains(bar::RED_BOLD), "must not be RED: {out:?}");
        assert!(!out.contains('█'), "must not fake-max the bar: {out:?}");
    }

    #[test]
    fn render_h5_at_exact_reset_is_not_stale() {
        let reset = ts(2026, 5, 1, 0);
        let w = window(42.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let out = render_h5(&w, reset);
        assert!(!out.contains('█'),
            "now == reset should not be stale (no full bar): {out:?}");
    }

    #[test]
    fn render_h5_red_at_pct_90() {
        let reset = ts(2026, 5, 1, 5);
        let w = window(90.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let out = render_h5(&w, reset - 1800);
        assert!(out.starts_with(bar::RED_BOLD), "pct=90 must be RED: {out:?}");
    }

    #[test]
    fn render_h5_below_90_uses_pace_color_not_red() {
        // pct=89 with negative delta (overran the window): GREY.
        let reset = ts(2026, 5, 1, 5);
        let now = reset - 60;
        let w = window(89.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let out = render_h5(&w, now);
        assert!(!out.starts_with(bar::RED_BOLD),
            "pct=89 with on-pace delta should NOT be RED: {out:?}");
        assert!(out.starts_with(bar::GREY), "expected GREY pace color: {out:?}");
    }

    #[test]
    fn render_h5_yellow_when_pace_delta_above_10() {
        // 15 min into 5h window → elapsed_pct=5; pct=30 → delta=25 (yellow).
        let reset = ts(2026, 5, 1, 5);
        let now = reset - 18_000 + 900;
        let w = window(30.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let out = render_h5(&w, now);
        assert!(out.starts_with(bar::YELLOW_BOLD),
            "delta in (10, 30] should be YELLOW: {out:?}");
    }

    #[test]
    fn render_h5_no_resets_at_renders_past_color() {
        let w = window(25.0, None);
        let out = render_h5(&w, 0);
        assert_eq!(out, format!("{}{}", bar::PAST, bar::bar(25)));
    }

    // ---------- additional mutation-killing boundary tests ----------

    /// Pins the exact dim-baseline characters for null past/future buckets.
    /// Each null cell renders `bar((i+1) * 100 / 7)` — kills all five
    /// arithmetic mutations on that line.
    ///
    /// Setup: idx=0, so position 0 is the current bucket (not baseline) and
    /// positions 1..=6 are the dim-baseline cells we're verifying.
    #[test]
    fn render_d7_baseline_uses_expected_per_cell_progression() {
        let cycle_start = ts(2026, 4, 29, 0);
        let reset = cycle_start + 7 * 86_400;
        let now = cycle_start;
        let w = window(0.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let mut h = History::default();
        let out = render_d7(&w, &mut h, now);

        // Positions 1..=6 inclusive should be baseline-projected.
        // bar((1+1)*100/7), bar((2+1)*100/7) ... = bar(28), bar(42), bar(57),
        // bar(71), bar(85), bar(100) = ▃ ▄ ▅ ▆ ▇ █
        let expected_baseline: String = (1..7)
            .map(|i| bar::bar(((i + 1) * 100 / 7) as u8))
            .collect();
        let plain = strip_ansi(&out);
        assert!(
            plain.contains(&expected_baseline),
            "expected baseline chars {expected_baseline:?} (positions 1..=6) in: {plain:?}"
        );
    }

    /// Exact-equality boundary on render_d7's stale check. Kills `> → >=`.
    /// At now == reset, original code goes to the non-stale render path.
    /// The mutant would return one of the two stale-signature strings.
    #[test]
    fn render_d7_at_exact_reset_does_not_emit_stale_signature() {
        let reset = ts(2026, 5, 1, 0);
        let w = window(50.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let mut h = History::default();
        let out = render_d7(&w, &mut h, reset);
        assert_ne!(out, format!("{}███████", bar::RED_BOLD));
        assert_ne!(out, format!("{}·······", bar::GREY));
    }

    /// Exact-equality boundary on `delta > 30`. Kills `> → >=`.
    /// At delta == 30, original: NOT red (falls through to yellow).
    /// Mutant: red.
    #[test]
    fn render_d7_at_delta_exactly_30_is_yellow_not_red() {
        // Construct: pct=80 (below 90), elapsed_pct=50 → delta=30.
        // 3.5d into 7d cycle → elapsed_pct = 50.
        let cycle_start = ts(2026, 4, 29, 0);
        let reset = cycle_start + 7 * 86_400;
        let now = cycle_start + 7 * 86_400 / 2;
        let w = window(80.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let mut h = History {
            cycles: vec![Cycle { reset, buckets: [None; 7], scoped: [None; 7] }],
        };
        let out = render_d7(&w, &mut h, now);
        assert!(out.contains(bar::YELLOW_BOLD), "delta==30 → YELLOW: {out:?}");
        assert!(!out.contains(bar::RED_BOLD), "delta==30 → not RED: {out:?}");
    }

    /// Exact-equality boundary on `delta > 10`. Kills `> → >=`.
    /// At delta == 10, original: NOT yellow (falls through to default).
    /// Mutant: yellow.
    #[test]
    fn render_d7_at_delta_exactly_10_is_default_not_yellow() {
        // Construct: pct=60, elapsed_pct=50 → delta=10.
        let cycle_start = ts(2026, 4, 29, 0);
        let reset = cycle_start + 7 * 86_400;
        let now = cycle_start + 7 * 86_400 / 2;
        let w = window(60.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let mut h = History {
            cycles: vec![Cycle { reset, buckets: [None; 7], scoped: [None; 7] }],
        };
        let out = render_d7(&w, &mut h, now);
        assert!(!out.contains(bar::YELLOW_BOLD), "delta==10 → default not YELLOW: {out:?}");
        assert!(!out.contains(bar::RED_BOLD), "delta==10 → not RED: {out:?}");
    }

    /// render_h5: pin the elapsed_pct division (kills `/` → `%`).
    /// At elapsed=9000 (2.5h into 5h window), original elapsed_pct=50.
    /// Mutant `% 18000` would produce 0 (since 9000*100=900000, %18000=0).
    /// Choose pct=50 so original delta=0 (GREY) but mutant delta=50 (RED).
    #[test]
    fn render_h5_elapsed_pct_division_pinned() {
        let reset = ts(2026, 5, 1, 5);
        let now = reset - 9000; // elapsed = 9000 (2.5h into 5h)
        let w = window(50.0, Some(Utc.timestamp_opt(reset, 0).unwrap()));
        let out = render_h5(&w, now);
        assert!(out.starts_with(bar::GREY), "elapsed_pct=50, pct=50 → on-pace GREY: {out:?}");
        assert!(!out.starts_with(bar::RED_BOLD), "should not be RED: {out:?}");
    }

    // ---------- render_scoped (Fable sparkline) ----------

    /// Render D7 first (it creates the cycle), then the scoped sparkline,
    /// exactly as `render` does.
    fn scoped_at(d7_pct: f64, pct: f64, reset: i64, now: i64, h: &mut History) -> String {
        let at = Some(Utc.timestamp_opt(reset, 0).unwrap());
        let d7 = window(d7_pct, at);
        render_d7(&d7, h, now);
        render_scoped(&d7, &window(pct, at), h, now)
    }

    #[test]
    fn render_scoped_no_resets_at_renders_single_past_cell() {
        let mut h = History::default();
        let out = render_scoped(&window(10.0, None), &window(25.0, None), &mut h, 0);
        assert_eq!(out, format!("{}{}", bar::PAST, bar::bar(25)));
    }

    #[test]
    fn render_scoped_stale_renders_grey_dots() {
        let reset = ts(2026, 9, 16, 7);
        let mut h = History::default();
        let out = scoped_at(40.0, 42.0, reset, reset + 1, &mut h);
        assert_eq!(out, format!("{}·······", bar::GREY));
    }

    #[test]
    fn render_scoped_at_exact_reset_is_not_stale() {
        let reset = ts(2026, 9, 16, 7);
        let mut h = History::default();
        let out = scoped_at(40.0, 42.0, reset, reset, &mut h);
        assert!(!out.contains('·'), "now == reset is not stale: {out:?}");
    }

    #[test]
    fn render_scoped_records_today_in_scoped_buckets_only() {
        let reset = ts(2026, 9, 16, 7);
        let now = reset - cycle::SEVEN_DAYS_S + 3 * 86_400 + 60; // idx 3
        let mut h = History::default();
        scoped_at(30.0, 55.0, reset, now, &mut h);
        assert_eq!(h.cycles.len(), 1);
        assert_eq!(h.cycles[0].scoped[3], Some(55));
        assert_eq!(h.cycles[0].buckets[3], Some(30), "total buckets untouched");
    }

    #[test]
    fn render_scoped_past_days_grey_future_dim() {
        let reset = ts(2026, 9, 16, 7);
        let now = reset - cycle::SEVEN_DAYS_S + 3 * 86_400 + 60; // idx 3
        let mut h = History {
            cycles: vec![Cycle {
                reset,
                buckets: [Some(5), Some(10), Some(20), None, None, None, None],
                scoped: [Some(10), Some(20), Some(40), None, None, None, None],
            }],
        };
        let out = scoped_at(30.0, 50.0, reset, now, &mut h);
        assert_eq!(out.matches(bar::PAST).count(), 3, "past days grey like the total: {out:?}");
        assert!(!out.contains(bar::FABLE_WARN), "on-pace Fable must not be purple: {out:?}");
        assert_eq!(out.matches(bar::DIM).count(), 3, "{out:?}");
        assert_eq!(out.chars().filter(|c| bar::SPARK_CHARS.contains(c)).count(), 7);
    }

    #[test]
    fn render_scoped_today_plain_when_on_pace() {
        // 3.5d into the week → elapsed_pct=50; pct=50 → delta=0 → plain.
        let reset = ts(2026, 9, 16, 7);
        let now = reset - cycle::SEVEN_DAYS_S / 2;
        let mut h = History::default();
        let out = scoped_at(10.0, 50.0, reset, now, &mut h);
        assert!(out.contains(&format!("{}{}", bar::RESET, bar::bar(50))), "{out:?}");
        assert!(!out.contains(bar::FABLE_WARN), "{out:?}");
    }

    #[test]
    fn render_scoped_today_purple_when_ahead_of_pace() {
        // Day 0 (elapsed_pct=0); pct=20 → delta=20 → FABLE_WARN.
        let reset = ts(2026, 9, 16, 7);
        let now = reset - cycle::SEVEN_DAYS_S;
        let mut h = History::default();
        let out = scoped_at(10.0, 20.0, reset, now, &mut h);
        assert!(out.starts_with(&format!("{}{}", bar::FABLE_WARN, bar::bar(20))), "{out:?}");
    }

    #[test]
    fn render_scoped_red_when_far_ahead_or_at_90() {
        let reset = ts(2026, 9, 16, 7);
        let mut h = History::default();
        let out = scoped_at(10.0, 31.0, reset, reset - cycle::SEVEN_DAYS_S, &mut h);
        assert!(out.starts_with(bar::RED_BOLD), "delta 31 → RED: {out:?}");
        let mut h = History::default();
        let out = scoped_at(10.0, 90.0, reset, reset - 60, &mut h);
        assert!(out.contains(&format!("{}{}", bar::RED_BOLD, bar::bar(90))), "pct 90 → RED: {out:?}");
    }

    #[test]
    fn render_scoped_elapsed_pct_division_pinned() {
        // 1.75d in (25% elapsed) with pct=25 → delta=0 → plain. A `/`→`%`
        // mutant makes elapsed_pct=0 → delta=25 → FABLE_WARN.
        let reset = ts(2026, 9, 16, 7);
        let now = reset - cycle::SEVEN_DAYS_S + cycle::SEVEN_DAYS_S / 4;
        let mut h = History::default();
        let out = scoped_at(10.0, 25.0, reset, now, &mut h);
        assert!(!out.contains(bar::FABLE_WARN), "on-pace must not be purple: {out:?}");
        assert!(out.contains(&format!("{}{}", bar::RESET, bar::bar(25))), "{out:?}");
    }

    #[test]
    fn render_scoped_clamps_out_of_range_utilization() {
        let mut h = History::default();
        assert!(render_scoped(&window(0.0, None), &window(250.0, None), &mut h, 0).ends_with('█'));
        assert!(render_scoped(&window(0.0, None), &window(-5.0, None), &mut h, 0).ends_with('▁'));
    }

    /// Strip ANSI escape sequences for plain-text assertions.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' && chars.peek() == Some(&'[') {
                chars.next();
                for c2 in chars.by_ref() {
                    if c2.is_ascii_alphabetic() { break; }
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}
