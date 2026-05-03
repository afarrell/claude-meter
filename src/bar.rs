//! ANSI bar character + color selection.
//!
//! Pure functions, no I/O. The bash version inlined these — kept separate
//! here so they're trivially unit-testable and reusable.

/// 8 spark characters from low to high.
pub const SPARK_CHARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Pick a spark character for a percentage in 0..=100.
/// Matches the bash mapping: `${BARS:$((v * 8 / 101)):1}`.
pub fn bar(pct: u8) -> char {
    let pct = pct.min(100);
    let idx = (pct as usize * 8) / 101;
    SPARK_CHARS[idx]
}

// ANSI color escape codes.
pub const RESET: &str = "\x1b[0m";
pub const RED_BOLD: &str = "\x1b[1;31m";
pub const YELLOW_BOLD: &str = "\x1b[1;33m";
pub const GREY: &str = "\x1b[90m";
pub const DIM: &str = "\x1b[38;5;238m";
pub const PAST: &str = "\x1b[38;5;241m";

/// Color tier for the context-window bar.
pub fn ctx_color(pct: u8) -> &'static str {
    if pct >= 80 { RED_BOLD }
    else if pct >= 50 { YELLOW_BOLD }
    else { GREY }
}

/// Color tier for pace warnings (delta = utilization - elapsed%).
pub fn pace_color(delta: i32) -> &'static str {
    if delta > 30 { RED_BOLD }
    else if delta > 10 { YELLOW_BOLD }
    else { GREY }
}
