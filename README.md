# claude-meter

Minimal Claude Code statusline written in Rust. Renders three things, plus
one optional cell:

- **Context bar** — current context-window utilization (one spark char)
- **5-hour bar** — Anthropic's 5h rate-limit window (one spark char, color-coded by pace)
- **7-day sparkline** — rolling weekly utilization across the last 7 days (seven spark chars)
- **Scoped weekly sparkline**: a model-scoped weekly cap (Fable), when your
  plan has one: a second 7-day sparkline, one space after the first

![statusline preview](docs/statusline.svg)

Reading left to right: **model**, then a single char for **context-window
%**, then a single char for the **5-hour rate-limit window**, then the
**7-day rolling sparkline**. In the sparkline, dim cells are the on-pace
baseline projection (no observation yet for that day), the brighter cells
are observed daily peaks, and the brightest cell is today.

### Scoped weekly sparkline (Fable)

Some plans cap a single model at a fraction of the weekly allowance: as of
2026-09, Fable can use half of a subscription's weekly credits. The API
reports this as a `weekly_scoped` entry in its `limits[]` array, alongside
the unscoped `weekly_all` total. When that entry is present, `claude-meter`
records it per day (the `scoped` array of each cycle in the history file)
and renders a **second 7-day sparkline** after the total one, on the same
day grid, so each day's Fable use sits next to that day's total.

The two numbers are independent: an all-Fable week pushes the Fable
sparkline up twice as fast as the total, an all-Opus week leaves it at zero
while the total climbs. Both sparklines follow the same colouring policy:
grey for past days, plain for today when on pace, and a colour only when
today is over the limit for that point in the weekly cycle. The one
difference is the ahead-of-pace warning: **purple** on the Fable sparkline,
yellow on the total. Red at 90% or far ahead is shared by both. Stale
renders as grey dots.

**Narrow terminals.** Claude Code sets `COLUMNS` to the terminal width; the
wrapper script sets `CLAUDE_METER_RESERVE` to the width it appends after the
meter (cwd, branch, hints). When both sparklines don't fit, the Fable one
wraps onto a second line, starting in the same column as the total so the
days line up.

Accounts without a scoped cap render exactly as before.

## Why this exists

I'd been iterating on a bash status line for months — adding the weekly
sparkline, the per-day bucket history, the pace-color tiers, and all the
cycle math on top of the simple vertical-bar idea from
[`tiny-claude-statusline`](https://github.com/csabapalfi/tiny-claude-statusline)
by [Csaba Palfi](https://github.com/csabapalfi). Over time the cycle math
accumulated two subtle bugs:

1. **Intra-day API noise** clobbered each day's recorded peak (last sample
   wins, even if mid-day the API briefly under-reported).
2. **Reset-timestamp drift** of even one second collapsed the bucket index
   to zero, making the leftmost bar of the weekly sparkline grow on every
   render.

After the second bug, the right answer felt like Rust + a real test suite.
This port preserves the visual style while adding:

- Encoded-as-tests regression coverage for both bugs above.
- A pure `render` function (no I/O in the library) — every behavior is
  unit-testable with deterministic time and in-memory state.
- ~20× faster rendering (≈4 ms vs ≈80 ms per call), which adds up because
  the statusline runs on every interaction.

## Architecture

```
src/
├── lib.rs       — pure render(input, now, &cache, &mut history) -> String
├── state.rs     — pace maths shared by render and --json; the Snapshot contract
├── cycle.rs     — cycle_start, bucket_idx, max_guard, forward_fill (the bug zone)
├── bar.rs       — spark chars + ANSI color tiers
├── cache.rs     — ApiCache + Window (deserialize claude-usage.json)
├── history.rs   — Cycle + History (serialize/deserialize claude-usage-history.json)
└── main.rs      — stdin/file/stdout orchestration (the only place with I/O)
```

The library is intentionally I/O-free — tests build the cache + history in
memory, render, and inspect the resulting struct. This also keeps the
security review surface tiny: filesystem reads/writes happen in exactly one
function in `main.rs`.

## Tests

```bash
cargo test
```

- `tests/outer.rs` — integration tests that encode the bugs as named cases
  (`drift_does_not_collapse_idx_to_zero`,
  `max_guard_keeps_daily_peak_across_intraday_renders`,
  `cycle_rollover_appends_new_entry`,
  `layout_groups_left_meters_and_separates_d7`).
- `src/cycle.rs` `mod tests` — unit coverage for cycle math: drift edges
  (±60s tolerance), bucket boundary clamping, max-guard semantics,
  forward-fill behavior.

If a future change reintroduces either of the bugs above, the tests will
spell it out by name in the failure output.

### Mutation testing

```bash
cargo install cargo-mutants
cargo mutants
```

The library catches 117 of 121 viable mutants (97%). The 4 remaining
mutants all live in `main.rs` (orchestration: `main`, `run`,
`read_stdin`) — they're covered by running the binary against the live
status line, not by unit tests.

## Install

```bash
cargo build --release
cp target/release/claude-meter ~/.local/bin/
```

Then point your Claude Code statusline wrapper at `~/.local/bin/claude-meter`.
This binary consumes the cache file produced by the wrapper — it doesn't
fetch from the Anthropic API itself. Cache refresh (the `curl` to
`/api/oauth/usage` + macOS Keychain token lookup) stays in the wrapper.

## Statusline payload

`claude-meter` reads the standard Claude Code statusline JSON from stdin:

```json
{
  "session_id": "…",
  "model": { "id": "claude-opus-4-7", "display_name": "Opus 4.7" },
  "workspace": { "current_dir": "/path" },
  "context_window": { "used_percentage": 35 }
}
```

It reads `~/.cache/claude-usage.json` (Anthropic API response cache) and
`~/.cache/claude-usage-history.json` (per-day bucket history), updates the
history with the current observation, and prints the rendered ANSI line to
stdout.

## JSON output

`claude-meter --json` ignores stdin and prints the computed meter state
instead, for other front ends (a menu bar app, the Stream Deck plugin) that
want the pace maths without reimplementing it. It reads the same cache and
records today's reading in the history file, just as a statusline render
does. It exits 1 if it can't read or parse the cache; the statusline mode
always exits 0.

```json
{
  "schema": 1,
  "generated_at": "2026-10-04T14:44:58Z",
  "cache_updated_at": "2026-10-04T14:44:29Z",
  "five_hour": { "label": null, "used_pct": 0, "resets_at": "2026-10-04T19:40:00Z",
                 "stale": false, "elapsed_pct": 1, "pace": -1,
                 "cycle_start": null, "today": null, "daily": null },
  "seven_day": { "label": null, "used_pct": 42, "resets_at": "2026-10-07T07:00:00Z",
                 "stale": false, "elapsed_pct": 61, "pace": -19,
                 "cycle_start": "2026-09-30T07:00:00Z", "today": 4,
                 "daily": [21, 30, 37, 37, 42, null, null] },
  "scoped": { "label": "Fable", "used_pct": 41, "...": "same fields as seven_day" }
}
```

- `schema`: contract version. It changes only when a field is renamed,
  removed or changes meaning; new fields can appear without a bump.
- `cache_updated_at`: when the usage cache was last written (file mtime).
- `scoped`: the model-scoped weekly cap, or `null` on accounts without one.
  It runs on the total window's cycle and day grid.
- Every window has the same keys. `used_pct` is always set.
- `stale`: the reset time has passed without a cache refresh, so
  `used_pct` may belong to the previous window; pace fields are `null`.
- `elapsed_pct`: share of the window gone. `pace` is `used_pct - elapsed_pct`,
  so positive means ahead of pace. Both are `null` when the window is stale
  or the API gave no reset time.
- `cycle_start`, `today`, `daily`: weekly windows only. `daily` holds the
  seven per-day readings, forward-filled up to `today` (its index); future
  days are `null`.

## Credits

The vertical-bar spark idea (mapping a 0–100% percentage to one of eight
Unicode bar characters) comes from
[`tiny-claude-statusline`](https://github.com/csabapalfi/tiny-claude-statusline)
by [Csaba Palfi](https://github.com/csabapalfi) — a great little script that
got me started.

## License

MIT — see [LICENSE](LICENSE). Third-party attribution is in [NOTICE](NOTICE).
