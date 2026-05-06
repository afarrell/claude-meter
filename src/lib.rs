//! Claude Code statusline renderer — D7 sparkline + H5 bar + context bar.
//!
//! Pure rendering core. All file I/O lives in `main.rs` so this library is
//! trivially testable (string-in, string-out) and immune to filesystem
//! surface area in security review.

use chrono::{DateTime, Utc};

pub mod bar;
pub mod cache;
pub mod cycle;
pub mod history;

pub use cache::{ApiCache, Window};
pub use history::{Cycle, History};

/// Parsed Claude Code statusline stdin payload.
#[derive(Debug, Clone)]
pub struct StatuslineInput {
    pub session_id: String,
    pub model_id: String,
    pub context_pct: u32,
    pub cwd: String,
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

    let ctx_pct = input.context_pct.min(100) as u8;
    let model_short = short_model(&input.model_id);
    let ctx_out = format!("{}{} {}", bar::ctx_color(ctx_pct), model_short, bar::bar(ctx_pct));

    format!(
        "{}{}{}{} {}{}",
        ctx_out,
        bar::RESET,
        h5_out,
        bar::RESET,
        d7_out,
        bar::RESET
    )
}

/// Render the D7 sparkline and update history with the current observation.
fn render_d7(window: &Window, history: &mut History, now_ts: i64) -> String {
    let reset_ts = match window.resets_at {
        Some(dt) => dt.timestamp(),
        None => return format!("{}{}", bar::PAST, bar::bar(window.utilization as u8)),
    };

    // Stale: reset is in the past (cache wasn't refreshed before rollover).
    // Render the dim baseline in GREY — we don't know current bucket state, so
    // showing fabricated full red bars (the prior behavior) misleads the user.
    if now_ts > reset_ts {
        return format!("{}·······", bar::GREY);
    }

    let cycle_start = cycle::cycle_start_for_reset(reset_ts, history);
    let cycle_len = (reset_ts - cycle_start).max(1);
    let idx = cycle::bucket_idx(now_ts, cycle_start, cycle_len);
    let pct = window.utilization as u8;

    let buckets_for_render: [Option<u8>; 7];
    match cycle::match_cycle(reset_ts, history) {
        Some(m) => {
            cycle::apply_max_guard(&mut history.cycles[m].buckets, idx, pct);
            buckets_for_render = history.cycles[m].buckets;
        }
        None => {
            cycle::append_new_cycle(history, reset_ts, idx, pct);
            buckets_for_render = history.cycles.last().unwrap().buckets;
        }
    }

    let mut display = buckets_for_render;
    cycle::forward_fill(&mut display, idx);

    let elapsed_pct = ((now_ts - cycle_start) * 100 / cycle_len) as i32;
    let delta = (pct as i32) - elapsed_pct;
    let current_color = if pct >= 90 || delta > 30 {
        bar::RED_BOLD
    } else if delta > 10 {
        bar::YELLOW_BOLD
    } else {
        "\x1b[0m"
    };

    let mut out = String::new();
    for (i, cell) in display.iter().enumerate() {
        match cell {
            None => {
                let expected = ((i + 1) * 100 / 7) as u8;
                out.push_str(bar::DIM);
                out.push(bar::bar(expected));
            }
            Some(v) if i == idx => {
                out.push_str(current_color);
                out.push(bar::bar(*v));
            }
            Some(v) => {
                out.push_str(bar::PAST);
                out.push(bar::bar(*v));
            }
        }
    }
    out
}

/// Render the H5 five-hour bar with pace coloring.
fn render_h5(window: &Window, now_ts: i64) -> String {
    let pct = window.utilization as u8;
    let reset_ts = match window.resets_at {
        Some(dt) => dt.timestamp(),
        None => return format!("{}{}", bar::PAST, bar::bar(pct)),
    };
    // Stale: window rolled over before the cache could refresh. Render the
    // last known utilization in GREY — honest "stale, may be out of date"
    // signal instead of a fabricated full red bar that lies about usage.
    if now_ts > reset_ts {
        return format!("{}{}", bar::GREY, bar::bar(pct));
    }
    if pct >= 90 {
        return format!("{}{}", bar::RED_BOLD, bar::bar(pct));
    }
    let elapsed = now_ts - (reset_ts - 18_000);
    let elapsed_pct = (elapsed * 100 / 18_000) as i32;
    let delta = (pct as i32) - elapsed_pct;
    let delta_clamped = delta.clamp(0, 100);
    format!("{}{}", bar::pace_color(delta_clamped), bar::bar(pct))
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
            cycles: vec![Cycle { reset, buckets: [None; 7] }],
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
            cycles: vec![Cycle { reset, buckets: [None; 7] }],
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
            cycles: vec![Cycle { reset, buckets: [None; 7] }],
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
            cycles: vec![Cycle { reset, buckets: [None; 7] }],
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
            cycles: vec![Cycle { reset, buckets: [None; 7] }],
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
            cycles: vec![Cycle { reset, buckets: [None; 7] }],
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
