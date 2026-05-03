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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_lowest_chars_for_low_pct() {
        assert_eq!(bar(0), '▁');
        assert_eq!(bar(12), '▁');   // 12*8/101 = 0 → first char
        assert_eq!(bar(13), '▂');   // 13*8/101 = 1 → second char
    }

    #[test]
    fn bar_highest_char_for_full() {
        assert_eq!(bar(100), '█');
        assert_eq!(bar(101), '█');  // clamped via min(100)
        assert_eq!(bar(255), '█');
    }

    #[test]
    fn bar_progression_covers_eight_levels() {
        // Each spark char must appear at least once across 0..=100.
        let chars: std::collections::HashSet<_> = (0..=100).map(bar).collect();
        assert_eq!(chars.len(), 8, "expected all 8 spark levels: got {chars:?}");
    }

    #[test]
    fn ctx_color_grey_below_50() {
        assert_eq!(ctx_color(0), GREY);
        assert_eq!(ctx_color(49), GREY);
    }

    #[test]
    fn ctx_color_yellow_at_50_through_79() {
        assert_eq!(ctx_color(50), YELLOW_BOLD);
        assert_eq!(ctx_color(79), YELLOW_BOLD);
    }

    #[test]
    fn ctx_color_red_at_80_and_above() {
        assert_eq!(ctx_color(80), RED_BOLD);
        assert_eq!(ctx_color(100), RED_BOLD);
    }

    #[test]
    fn pace_color_grey_at_low_delta() {
        assert_eq!(pace_color(-50), GREY);
        assert_eq!(pace_color(0), GREY);
        assert_eq!(pace_color(10), GREY);
    }

    #[test]
    fn pace_color_yellow_above_10_through_30() {
        assert_eq!(pace_color(11), YELLOW_BOLD);
        assert_eq!(pace_color(30), YELLOW_BOLD);
    }

    #[test]
    fn pace_color_red_above_30() {
        assert_eq!(pace_color(31), RED_BOLD);
        assert_eq!(pace_color(100), RED_BOLD);
    }
}
