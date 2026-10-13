//! Computed meter state: the pace maths shared by the statusline renderer
//! and the `--json` output, so front ends (menu bar app, Stream Deck) never
//! re-implement the cycle logic.
//!
//! Pure like the rest of the library: `now` is injected and the caller
//! persists the mutated `History`.

use chrono::{DateTime, TimeZone, Utc};
use serde::Serialize;

use crate::cycle;
use crate::{ApiCache, History, Window};

const FIVE_HOURS_S: i64 = 18_000;

/// Version of the `--json` contract. Bump on any breaking change to field
/// names, types or meaning; adding fields is not breaking.
pub const SCHEMA_VERSION: u32 = 1;

/// A weekly window's live state within its cycle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveWeek {
    pub pct: u8,
    pub cycle_start: i64,
    pub reset: i64,
    /// Share of the cycle elapsed, 0..=100.
    pub elapsed_pct: i32,
    /// Index of today's bucket, 0..=6.
    pub today: usize,
    /// Per-day readings, forward-filled up to today; future days are `None`.
    pub daily: [Option<u8>; 7],
}

impl LiveWeek {
    /// Used minus elapsed: positive means ahead of pace (using the
    /// allowance faster than the week is passing).
    pub fn pace(&self) -> i32 {
        self.pct as i32 - self.elapsed_pct
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Weekly {
    /// The API gave no reset time: only the reading is known.
    NoReset { pct: u8 },
    /// The reset is in the past: the cache wasn't refreshed across it, so
    /// the reading may belong to the previous cycle.
    Stale { pct: u8 },
    Live(LiveWeek),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FiveHour {
    NoReset { pct: u8 },
    Stale { pct: u8 },
    Live { pct: u8, elapsed_pct: i32 },
}

/// The total weekly window. Records today's reading in `history`.
pub fn weekly(window: &Window, history: &mut History, now_ts: i64) -> Weekly {
    let pct = window.utilization as u8;
    let reset = match window.resets_at {
        Some(dt) => dt.timestamp(),
        None => return Weekly::NoReset { pct },
    };
    if now_ts > reset {
        return Weekly::Stale { pct };
    }

    let cycle_start = cycle::cycle_start_for_reset(reset, history);
    let cycle_len = (reset - cycle_start).max(1);
    let today = cycle::bucket_idx(now_ts, cycle_start, cycle_len);

    let mut daily = match cycle::match_cycle(reset, history) {
        Some(m) => {
            cycle::record_observation(&mut history.cycles[m].buckets, today, pct);
            history.cycles[m].buckets
        }
        None => {
            cycle::append_new_cycle(history, reset, today, pct);
            history.cycles.last().unwrap().buckets
        }
    };
    cycle::forward_fill(&mut daily, today);

    let elapsed_pct = ((now_ts - cycle_start) * 100 / cycle_len) as i32;
    Weekly::Live(LiveWeek { pct, cycle_start, reset, elapsed_pct, today, daily })
}

/// The model-scoped weekly cap (Fable). It shares the total window's reset,
/// so it uses the same cycle and day grid: `weekly` must run first so the
/// cycle exists. Records today's reading in `history`.
pub fn scoped(d7: &Window, scoped: &Window, history: &mut History, now_ts: i64) -> Weekly {
    let pct = scoped.utilization.clamp(0.0, 100.0) as u8;
    let reset = match d7.resets_at.or(scoped.resets_at) {
        Some(dt) => dt.timestamp(),
        None => return Weekly::NoReset { pct },
    };
    if now_ts > reset {
        return Weekly::Stale { pct };
    }

    let cycle_start = cycle::cycle_start_for_reset(reset, history);
    let cycle_len = (reset - cycle_start).max(1);
    let today = cycle::bucket_idx(now_ts, cycle_start, cycle_len);

    let mut daily = match cycle::match_cycle(reset, history) {
        Some(m) => {
            cycle::record_observation(&mut history.cycles[m].scoped, today, pct);
            history.cycles[m].scoped
        }
        None => {
            let mut b = [None; 7];
            b[today] = Some(pct);
            b
        }
    };
    cycle::forward_fill(&mut daily, today);

    let elapsed_pct = ((now_ts - cycle_start) * 100 / cycle_len) as i32;
    Weekly::Live(LiveWeek { pct, cycle_start, reset, elapsed_pct, today, daily })
}

/// The five-hour window.
pub fn five_hour(window: &Window, now_ts: i64) -> FiveHour {
    let pct = window.utilization as u8;
    let reset = match window.resets_at {
        Some(dt) => dt.timestamp(),
        None => return FiveHour::NoReset { pct },
    };
    if now_ts > reset {
        return FiveHour::Stale { pct };
    }
    let elapsed_pct = ((now_ts - (reset - FIVE_HOURS_S)) * 100 / FIVE_HOURS_S) as i32;
    FiveHour::Live { pct, elapsed_pct }
}

// ---------- `--json` contract ----------

/// Everything a front end needs to draw the meter. Serialised by
/// `claude-meter --json`; see the README for the field reference.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub schema: u32,
    pub generated_at: DateTime<Utc>,
    /// When the usage cache was last written; `None` if unknown.
    pub cache_updated_at: Option<DateTime<Utc>>,
    pub five_hour: WindowJson,
    pub seven_day: WindowJson,
    /// The model-scoped weekly cap, or `None` on accounts without one.
    pub scoped: Option<WindowJson>,
}

/// One usage window. Weekly-only fields are `None` for the five-hour
/// window and whenever the window isn't live (stale or no reset time).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WindowJson {
    /// Model display name, for the scoped cap (e.g. `"Fable"`).
    pub label: Option<String>,
    pub used_pct: u8,
    pub resets_at: Option<DateTime<Utc>>,
    /// The reset has passed without a cache refresh: `used_pct` is suspect.
    pub stale: bool,
    pub elapsed_pct: Option<i32>,
    /// `used_pct - elapsed_pct`: positive is ahead of pace.
    pub pace: Option<i32>,
    pub cycle_start: Option<DateTime<Utc>>,
    /// Index of today's entry in `daily`.
    pub today: Option<usize>,
    /// Per-day readings for the cycle, forward-filled to today; future
    /// days are `null`.
    pub daily: Option<[Option<u8>; 7]>,
}

impl WindowJson {
    fn bare(pct: u8, resets_at: Option<DateTime<Utc>>, stale: bool) -> Self {
        WindowJson {
            label: None,
            used_pct: pct,
            resets_at,
            stale,
            elapsed_pct: None,
            pace: None,
            cycle_start: None,
            today: None,
            daily: None,
        }
    }

    fn from_weekly(w: Weekly, resets_at: Option<DateTime<Utc>>) -> Self {
        match w {
            Weekly::NoReset { pct } => Self::bare(pct, None, false),
            Weekly::Stale { pct } => Self::bare(pct, resets_at, true),
            Weekly::Live(l) => WindowJson {
                elapsed_pct: Some(l.elapsed_pct),
                pace: Some(l.pace()),
                cycle_start: Utc.timestamp_opt(l.cycle_start, 0).single(),
                today: Some(l.today),
                daily: Some(l.daily),
                ..Self::bare(l.pct, resets_at, false)
            },
        }
    }

    fn from_five_hour(f: FiveHour, resets_at: Option<DateTime<Utc>>) -> Self {
        match f {
            FiveHour::NoReset { pct } => Self::bare(pct, None, false),
            FiveHour::Stale { pct } => Self::bare(pct, resets_at, true),
            FiveHour::Live { pct, elapsed_pct } => WindowJson {
                elapsed_pct: Some(elapsed_pct),
                pace: Some(pct as i32 - elapsed_pct),
                ..Self::bare(pct, resets_at, false)
            },
        }
    }
}

/// Compute the snapshot and record today's readings in `history`, exactly
/// as a statusline render would.
pub fn snapshot(
    now: DateTime<Utc>,
    cache: &ApiCache,
    history: &mut History,
    cache_updated_at: Option<DateTime<Utc>>,
) -> Snapshot {
    let now_ts = now.timestamp();
    let d7 = &cache.seven_day;
    let seven_day = WindowJson::from_weekly(weekly(d7, history, now_ts), d7.resets_at);
    let five_hour =
        WindowJson::from_five_hour(five_hour(&cache.five_hour, now_ts), cache.five_hour.resets_at);
    let scoped = cache.scoped_weekly().map(|w| WindowJson {
        label: cache.scoped_weekly_label().map(str::to_string),
        // The scoped cap runs on the total window's cycle (see `scoped`).
        ..WindowJson::from_weekly(scoped(d7, &w, history, now_ts), d7.resets_at.or(w.resets_at))
    });
    Snapshot {
        schema: SCHEMA_VERSION,
        generated_at: now,
        cache_updated_at,
        five_hour,
        seven_day,
        scoped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Cycle;
    use crate::{Limit, ModelScope, Scope};

    fn at(ts: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(ts, 0).unwrap()
    }

    fn window(pct: f64, reset: Option<i64>) -> Window {
        Window { utilization: pct, resets_at: reset.map(at) }
    }

    const RESET: i64 = 1_790_000_000;
    const START: i64 = RESET - cycle::SEVEN_DAYS_S;

    fn cache(h5: Window, d7: Window, scoped: Option<(f64, &str)>) -> ApiCache {
        ApiCache {
            five_hour: h5,
            seven_day: d7,
            limits: scoped
                .map(|(pct, name)| Limit {
                    kind: "weekly_scoped".into(),
                    percent: pct,
                    resets_at: Some(at(RESET)),
                    scope: Some(Scope {
                        model: Some(ModelScope { display_name: Some(name.into()) }),
                    }),
                })
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn weekly_live_reports_pace_and_forward_filled_days() {
        // 3.5 days in: elapsed 50%, today idx 3; used 62 → pace +12.
        let now = START + cycle::SEVEN_DAYS_S / 2;
        let mut h = History {
            cycles: vec![Cycle {
                reset: RESET,
                buckets: [Some(10), None, Some(40), None, None, None, None],
                scoped: [None; 7],
            }],
        };
        let Weekly::Live(l) = weekly(&window(62.0, Some(RESET)), &mut h, now) else {
            panic!("expected live");
        };
        assert_eq!((l.elapsed_pct, l.today, l.pace()), (50, 3, 12));
        assert_eq!(l.daily, [Some(10), Some(10), Some(40), Some(62), None, None, None]);
        assert_eq!(h.cycles[0].buckets[3], Some(62), "today recorded in history");
        assert_eq!(h.cycles[0].buckets[1], None, "forward fill is display-only");
    }

    #[test]
    fn weekly_stale_and_no_reset() {
        let mut h = History::default();
        assert_eq!(weekly(&window(42.0, Some(RESET)), &mut h, RESET + 1), Weekly::Stale { pct: 42 });
        assert_eq!(weekly(&window(42.0, None), &mut h, RESET), Weekly::NoReset { pct: 42 });
        assert!(h.cycles.is_empty(), "nothing recorded when the cycle is unknown");
    }

    #[test]
    fn five_hour_states() {
        let reset = RESET;
        assert_eq!(five_hour(&window(30.0, Some(reset)), reset - 9_000), FiveHour::Live { pct: 30, elapsed_pct: 50 });
        assert_eq!(five_hour(&window(30.0, Some(reset)), reset + 1), FiveHour::Stale { pct: 30 });
        assert_eq!(five_hour(&window(0.0, None), reset), FiveHour::NoReset { pct: 0 });
    }

    #[test]
    fn snapshot_live_with_scoped_cap() {
        let now = START + cycle::SEVEN_DAYS_S / 2;
        let c = cache(window(30.0, Some(now + 9_000)), window(62.0, Some(RESET)), Some((25.0, "Fable")));
        let mut h = History::default();
        let s = snapshot(at(now), &c, &mut h, Some(at(now - 60)));

        assert_eq!(s.schema, SCHEMA_VERSION);
        assert_eq!(s.seven_day.pace, Some(12));
        assert_eq!(s.seven_day.today, Some(3));
        assert_eq!(s.seven_day.cycle_start, Some(at(START)));
        assert_eq!(s.seven_day.label, None);
        assert_eq!(s.five_hour.elapsed_pct, Some(50));
        assert_eq!(s.five_hour.pace, Some(-20));
        assert_eq!(s.five_hour.daily, None, "five-hour has no day grid");

        let f = s.scoped.expect("scoped cap present");
        assert_eq!(f.label.as_deref(), Some("Fable"));
        assert_eq!((f.used_pct, f.pace), (25, Some(-25)));
        assert_eq!(f.daily, Some([None, None, None, Some(25), None, None, None]));
        assert_eq!(h.cycles[0].scoped[3], Some(25), "scoped reading recorded");
    }

    #[test]
    fn snapshot_without_scoped_cap_is_null() {
        let c = cache(window(0.0, None), window(10.0, Some(RESET)), None);
        let s = snapshot(at(START + 60), &c, &mut History::default(), None);
        let v = serde_json::to_value(&s).unwrap();
        assert!(v["scoped"].is_null());
        assert!(v["cache_updated_at"].is_null());
        // No reset time: reading only, every pace field null but present.
        assert_eq!(v["five_hour"]["used_pct"], 0);
        assert!(v["five_hour"]["pace"].is_null());
        assert_eq!(v["five_hour"]["stale"], false);
    }

    #[test]
    fn snapshot_stale_week_keeps_reading_and_flags_it() {
        let c = cache(window(5.0, Some(RESET)), window(42.0, Some(RESET)), Some((20.0, "Fable")));
        let s = snapshot(at(RESET + 3_600), &c, &mut History::default(), None);
        assert!(s.seven_day.stale && s.five_hour.stale);
        assert_eq!(s.seven_day.used_pct, 42);
        assert_eq!(s.seven_day.resets_at, Some(at(RESET)));
        assert_eq!(s.seven_day.daily, None);
        assert!(s.scoped.unwrap().stale);
    }

    /// Pins the contract's key set: renaming or dropping a key breaks
    /// front ends, so it must be a deliberate schema bump.
    #[test]
    fn json_keys_are_stable() {
        let c = cache(window(30.0, Some(RESET)), window(62.0, Some(RESET)), Some((25.0, "Fable")));
        let s = snapshot(at(START + 60), &c, &mut History::default(), None);
        let v = serde_json::to_value(&s).unwrap();
        let keys = |o: &serde_json::Value| {
            let mut k: Vec<_> = o.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        assert_eq!(keys(&v), ["cache_updated_at", "five_hour", "generated_at", "schema", "scoped", "seven_day"]);
        let window_keys = [
            "cycle_start", "daily", "elapsed_pct", "label", "pace", "resets_at", "stale", "today", "used_pct",
        ];
        for w in ["five_hour", "seven_day", "scoped"] {
            assert_eq!(keys(&v[w]), window_keys, "{w}");
        }
    }
}
