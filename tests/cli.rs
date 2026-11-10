//! End-to-end tests of the binary. `--json` mode: no stdin, JSON out,
//! history persisted, non-zero exit on failure. Statusline mode: a cache it
//! can't use still renders the context bar and names the reason.

use std::process::{Command, Stdio};

fn meter(home: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_claude-meter"));
    c.arg("--json").env("HOME", home).stdin(Stdio::null());
    c
}

#[test]
fn json_mode_reads_cache_without_stdin_and_records_history() {
    let home = tempfile::tempdir().unwrap();
    let cache_dir = home.path().join(".cache");
    std::fs::create_dir(&cache_dir).unwrap();
    std::fs::write(
        cache_dir.join("claude-usage.json"),
        r#"{
            "five_hour": {"utilization": 12.0, "resets_at": null},
            "seven_day": {"utilization": 42.0, "resets_at": "2099-01-01T00:00:00+00:00"}
        }"#,
    )
    .unwrap();

    let out = meter(home.path()).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["seven_day"]["used_pct"], 42);
    assert_eq!(v["five_hour"]["used_pct"], 12);
    assert!(v["scoped"].is_null());
    assert!(v["cache_updated_at"].is_string(), "cache mtime reported");

    let history = std::fs::read_to_string(cache_dir.join("claude-usage-history.json")).unwrap();
    assert!(history.contains("42"), "today's reading persisted: {history}");
}

#[test]
fn json_mode_fails_loudly_without_a_cache() {
    let home = tempfile::tempdir().unwrap();
    let out = meter(home.path()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
}

fn statusline(home: &std::path::Path) -> std::process::Output {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_claude-meter"))
        .env("HOME", home)
        .env_remove("COLUMNS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"model":{"id":"claude-opus-4-7"},"context_window":{"used_percentage":35}}"#)
        .unwrap();
    child.wait_with_output().unwrap()
}

/// Regression 2026-11: an API shape change made the cache unparseable and
/// the statusline printed nothing, so the wrapper showed a bare fallback
/// with no hint of why. It must say what failed, on stdout.
#[test]
fn statusline_mode_names_the_failure_when_the_cache_changes_shape() {
    let home = tempfile::tempdir().unwrap();
    let cache_dir = home.path().join(".cache");
    std::fs::create_dir(&cache_dir).unwrap();
    std::fs::write(cache_dir.join("claude-usage.json"), r#"{"five_hour": null, "seven_day": null}"#).unwrap();

    let out = statusline(home.path());
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("opus"), "context bar kept: {stdout:?}");
    assert!(stdout.contains("⚠ usage: parsing cache"), "reason shown: {stdout:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("parsing cache"));
}

#[test]
fn statusline_mode_names_a_missing_cache() {
    let home = tempfile::tempdir().unwrap();
    let stdout = String::from_utf8_lossy(&statusline(home.path()).stdout).into_owned();
    assert!(stdout.contains("⚠ usage: reading cache"), "{stdout:?}");
}
