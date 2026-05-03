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
        "{}{} {}{}   {}{}",
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

    // Stale: reset is in the past (window has rolled over but cache wasn't refreshed).
    if now_ts > reset_ts {
        let pct = window.utilization as u8;
        if pct == 0 {
            return format!("{}·······", bar::GREY);
        }
        return format!("{}███████", bar::RED_BOLD);
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
    if now_ts > reset_ts {
        if pct == 0 {
            return format!("{}{}", bar::GREY, bar::bar(0));
        }
        return format!("{}{}", bar::RED_BOLD, bar::bar(100));
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

    #[test]
    fn short_model_strips_claude_prefix_and_version() {
        assert_eq!(short_model("claude-opus-4-7"), "opus");
        assert_eq!(short_model("claude-sonnet-4-6"), "sonnet");
        assert_eq!(short_model("claude-haiku-4-5-20251001"), "haiku");
        assert_eq!(short_model("claude-opus-4-7-latest"), "opus");
        assert_eq!(short_model("gpt-4"), "gpt");
        assert_eq!(short_model("custom-model-name"), "custom-model-name");
    }
}
