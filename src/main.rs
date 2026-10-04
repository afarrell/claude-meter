//! Claude Code statusline binary.
//!
//! Reads JSON from stdin (Claude Code statusline payload), reads the
//! Anthropic usage cache + bucket history from `$HOME/.cache`, renders the
//! sparkline, persists updated history, and prints the result to stdout.
//!
//! `claude-meter --json` skips stdin and prints the computed meter state as
//! JSON instead (see `state::Snapshot`), for front ends such as the menu bar
//! app and the Stream Deck plugin.
//!
//! Cache refresh from the Anthropic API is the wrapper script's job — this
//! binary just consumes whatever's already in the cache file.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use claude_meter::{render, snapshot, ApiCache, History, StatuslineInput};
use serde_json::Value;
use std::env;
use std::io::{self, Read, Write};

fn main() {
    let json_mode = env::args().skip(1).any(|a| a == "--json");
    if let Err(e) = run(json_mode) {
        eprintln!("claude-meter: {e:#}");
        // A failed render shouldn't break the user's terminal: exit cleanly so
        // the harness still gets *something* on stdout. A `--json` consumer
        // needs to know the read failed, so that mode exits non-zero.
        std::process::exit(if json_mode { 1 } else { 0 });
    }
}

fn run(json_mode: bool) -> Result<()> {
    let home = env::var("HOME").context("HOME not set")?;
    let cache_path = format!("{home}/.cache/claude-usage.json");
    let history_path = format!("{home}/.cache/claude-usage-history.json");

    let cache_json = std::fs::read_to_string(&cache_path).context("reading cache")?;
    let cache = ApiCache::parse(&cache_json).context("parsing cache")?;

    let history_json = std::fs::read_to_string(&history_path).unwrap_or_else(|_| "{}".into());
    let mut history = History::parse(&history_json).unwrap_or_default();

    let output = if json_mode {
        let cache_updated_at = std::fs::metadata(&cache_path)
            .and_then(|m| m.modified())
            .ok()
            .map(DateTime::<Utc>::from);
        let state = snapshot(Utc::now(), &cache, &mut history, cache_updated_at);
        serde_json::to_string(&state)?
    } else {
        let input = parse_input(&read_stdin()?)?;
        render(&input, Utc::now(), &cache, &mut history)
    };

    let tmp_path = format!("{history_path}.tmp.{}", std::process::id());
    std::fs::write(&tmp_path, history.to_json())?;
    std::fs::rename(&tmp_path, &history_path)?;

    io::stdout().write_all(output.as_bytes())?;
    io::stdout().write_all(b"\n")?;
    Ok(())
}

fn read_stdin() -> Result<String> {
    let mut s = String::new();
    io::stdin().read_to_string(&mut s)?;
    Ok(s)
}

/// Parse the Claude Code statusline JSON payload tolerantly — unexpected
/// fields shouldn't break us as the harness adds optional fields over time.
fn parse_input(json: &str) -> Result<StatuslineInput> {
    let v: Value = serde_json::from_str(json).context("parsing stdin JSON")?;
    Ok(StatuslineInput {
        session_id: v
            .get("session_id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        model_id: v
            .get("model")
            .and_then(|m| m.get("id"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        context_pct: v
            .get("context_window")
            .and_then(|c| c.get("used_percentage"))
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0) as u32,
        cwd: v
            .get("workspace")
            .and_then(|w| w.get("current_dir"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        max_width: max_width(),
    })
}

/// Columns this output may use: `COLUMNS` (Claude Code sets it to the
/// terminal width before running the statusline) minus
/// `CLAUDE_METER_RESERVE`, the width the wrapper script appends after us.
fn max_width() -> Option<usize> {
    let cols: usize = env::var("COLUMNS").ok()?.trim().parse().ok()?;
    let reserve: usize = env::var("CLAUDE_METER_RESERVE")
        .ok()
        .and_then(|r| r.trim().parse().ok())
        .unwrap_or(0);
    Some(cols.saturating_sub(reserve))
}
