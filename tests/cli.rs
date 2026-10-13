//! End-to-end tests of the binary's `--json` mode: no stdin, JSON out,
//! history persisted, non-zero exit on failure.

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
