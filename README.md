# claude-statusline

Rust port of `~/.claude/tiny-claude-statusline.sh`. Renders a context-window
bar, 5-hour utilization bar, and 7-day rolling sparkline for the Claude Code
statusline.

## Why a port

Two subtle bugs in the bash version (intra-day max not preserved; cycle-start
collapsing under reset_ts drift) cost an afternoon to track down. Both were
the kind of thing a Rust type system + test suite makes hard to reintroduce.
Side benefit: ~20× faster (4ms vs 80ms per render).

## Architecture

```
src/
├── lib.rs       — pure render(input, now, &cache, &mut history) -> String
├── cycle.rs     — cycle_start, bucket_idx, max_guard, forward_fill (the bug zone)
├── bar.rs       — spark chars + ANSI color tiers
├── cache.rs     — ApiCache + Window (deserialize claude-usage.json)
├── history.rs   — Cycle + History (serialize/deserialize claude-usage-history.json)
└── main.rs      — stdin/file/stdout orchestration (only place with I/O)
```

`lib.rs` is intentionally I/O-free: tests build cache + history in memory and
inspect the resulting struct after `render`.

## Tests

```bash
cargo test
```

- `tests/outer.rs` — encodes the bash bugs as integration tests; each test
  fails if the corresponding bug ever returns.
- `src/cycle.rs` `mod tests` — unit coverage for cycle math (drift edges,
  bucket boundaries, max-guard, forward-fill).

## Install

```bash
cargo build --release
cp target/release/claude-statusline ~/.local/bin/
```

Then update the statusline wrapper to invoke the binary instead of the bash
script. Cache refresh (curl + macOS keychain) is intentionally not in this
binary — it stays in the wrapper.
