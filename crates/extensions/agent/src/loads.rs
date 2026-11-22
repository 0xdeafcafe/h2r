//! `load.timeline`: each level load (or BSP switch, or long stall) from
//! the game's ticks, with what the frames in it counted.
//!
//! A load is a stall (no tick for [`STALL`], or a change of map or BSP
//! between two ticks), then a settle (the next
//! [`LoadRecord::SETTLE_TICKS`] ticks). Each part gets the difference of
//! the frame-stats running totals ([`timeline::phase::totals`]) across it:
//! file bytes read, uploads, shader and pipeline compiles, phase times. The
//! first tick of the run is a `boot` load from when the agent started.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use agent_proto::{LoadParams, LoadRecord, LoadSegment};
use timeline::phase::{Phase, Totals};

/// A gap between ticks at least this long is a stall (the agent's
/// `loading` phase starts at the same point).
pub const STALL: Duration = crate::exec::TICK_STALE;

/// Where running totals come from: frame stats in the game, a fake in tests.
pub trait TotalsSource: Send {
    fn totals(&mut self) -> Option<Totals>;
}

/// The game's frame stats.
pub struct TimelineTotals;

impl TotalsSource for TimelineTotals {
    fn totals(&mut self) -> Option<Totals> {
        timeline::phase::totals()
    }
}

struct LastTick {
    at: Instant,
    map: Option<String>,
    bsp: Option<u32>,
    totals: Option<Totals>,
}

struct Settling {
    rec: LoadRecord,
    from: Instant,
    totals: Option<Totals>,
    ticks: u64,
}

pub struct LoadTracker {
    started: Instant,
    last: Option<LastTick>,
    settling: Option<Settling>,
    done: VecDeque<LoadRecord>,
    /// Frame stats were off at some point a load needed them.
    pub no_frame_stats: bool,
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e5).round() / 100.0
}

fn segment(name: &str, d: Duration, a: Option<&Totals>, b: Option<&Totals>) -> LoadSegment {
    let mut s = LoadSegment {
        name: name.into(),
        ms: ms(d),
        ..Default::default()
    };
    let zero = Totals::default();
    if let Some(b) = b {
        let t = b.since(a.unwrap_or(&zero));
        s.frames = t.frames;
        for p in Phase::ALL {
            let ns = t.phase_ns[p as usize];
            if ns > 0 {
                s.phases_ms
                    .insert(p.label().into(), ms(Duration::from_nanos(ns)));
            }
        }
        for (i, label) in timeline::phase::counter_columns() {
            let n = t.counts[i];
            if n > 0 {
                s.counts.insert(label.into(), n);
            }
        }
    }
    s
}

impl Default for LoadTracker {
    /// Started now: the agent is made before the game runs.
    fn default() -> Self {
        Self::new(Instant::now())
    }
}

impl LoadTracker {
    pub fn new(started: Instant) -> LoadTracker {
        LoadTracker {
            started,
            last: None,
            settling: None,
            done: VecDeque::new(),
            no_frame_stats: false,
        }
    }

    fn finish(&mut self, s: Settling, now: Instant, totals: Option<&Totals>, settled: bool) {
        let mut rec = s.rec;
        let d = now.saturating_duration_since(s.from);
        rec.settle_ms = ms(d);
        rec.settled = settled;
        rec.segments
            .push(segment("settle", d, s.totals.as_ref(), totals));
        if self.done.len() == LoadParams::KEEP {
            self.done.pop_front();
        }
        self.done.push_back(rec);
    }

    /// After each game tick.
    pub fn on_tick(
        &mut self,
        now: Instant,
        map: Option<String>,
        bsp: Option<u32>,
        present: u64,
        totals: Option<Totals>,
    ) {
        if totals.is_none() {
            self.no_frame_stats = true;
        }
        let (kind, from, from_map, from_bsp, before) = match &self.last {
            None => (
                Some("boot"),
                self.started,
                None,
                None,
                Some(Totals::default()),
            ),
            Some(l) => {
                let kind = if l.map != map {
                    Some("map")
                } else if l.bsp != bsp {
                    Some("bsp")
                } else if now.saturating_duration_since(l.at) >= STALL {
                    Some("stall")
                } else {
                    None
                };
                (kind, l.at, l.map.clone(), l.bsp, l.totals)
            }
        };
        if let Some(kind) = kind {
            if let Some(s) = self.settling.take() {
                // Another load before this one settled: it ends at the
                // tick before.
                self.finish(s, from, before.as_ref(), false);
            }
            let stall = now.saturating_duration_since(from);
            let rec = LoadRecord {
                kind: kind.into(),
                from_map,
                map: map.clone(),
                from_bsp,
                bsp,
                present,
                stall_ms: ms(stall),
                segments: vec![segment("stall", stall, before.as_ref(), totals.as_ref())],
                ..Default::default()
            };
            self.settling = Some(Settling {
                rec,
                from: now,
                totals,
                ticks: 0,
            });
        } else if let Some(s) = &mut self.settling {
            s.ticks += 1;
            if s.ticks >= LoadRecord::SETTLE_TICKS {
                let s = self.settling.take().expect("settling");
                self.finish(s, now, totals.as_ref(), true);
            }
        }
        self.last = Some(LastTick {
            at: now,
            map,
            bsp,
            totals,
        });
    }

    /// The last `n` loads, oldest first; and the stall going on now, if
    /// the game hasn't ticked for [`STALL`].
    pub fn report(
        &self,
        n: usize,
        now: Instant,
        totals: Option<Totals>,
    ) -> (Vec<LoadRecord>, Option<LoadRecord>) {
        let mut loads: Vec<LoadRecord> = self.done.iter().rev().take(n).cloned().collect();
        loads.reverse();
        let current = match &self.last {
            Some(l) if now.saturating_duration_since(l.at) >= STALL => {
                let d = now.saturating_duration_since(l.at);
                Some(LoadRecord {
                    kind: "stall".into(),
                    from_map: l.map.clone(),
                    from_bsp: l.bsp,
                    stall_ms: ms(d),
                    segments: vec![segment("stall", d, l.totals.as_ref(), totals.as_ref())],
                    ..Default::default()
                })
            }
            None => {
                let d = now.saturating_duration_since(self.started);
                Some(LoadRecord {
                    kind: "boot".into(),
                    stall_ms: ms(d),
                    segments: vec![segment(
                        "stall",
                        d,
                        Some(&Totals::default()),
                        totals.as_ref(),
                    )],
                    ..Default::default()
                })
            }
            _ => None,
        };
        (loads, current)
    }
}

#[cfg(test)]
mod tests {
    use timeline::phase::Counter;

    use super::*;

    fn totals(frames: u64, read: u64, shaders: u64) -> Option<Totals> {
        let mut t = Totals {
            frames,
            ..Default::default()
        };
        t.counts[Counter::ReadBytes as usize] = read;
        t.counts[Counter::Shaders as usize] = shaders;
        t.phase_ns[Phase::Guest as usize] = frames * 1_000_000;
        Some(t)
    }

    #[test]
    fn boot_then_a_map_load_in_two_segments() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut l = LoadTracker::new(t0);
        // The first tick, 2 s after start: the boot load.
        l.on_tick(
            at(2000),
            Some("mainmenu".into()),
            None,
            50,
            totals(50, 9000, 4),
        );
        for i in 1..=30 {
            l.on_tick(
                at(2000 + i * 33),
                Some("mainmenu".into()),
                None,
                50 + i,
                totals(50 + i, 9000, 4),
            );
        }
        // No tick for 3 s, then the level, which reads 5 MB and compiles
        // shaders as it settles.
        let t = 2000 + 30 * 33;
        l.on_tick(
            at(t + 3000),
            Some("01a_tutorial".into()),
            Some(0),
            200,
            totals(150, 5_009_000, 10),
        );
        for i in 1..=29 {
            l.on_tick(
                at(t + 3000 + i * 33),
                Some("01a_tutorial".into()),
                Some(0),
                200 + i,
                totals(150 + i, 5_009_000, 10),
            );
        }
        let (loads, current) = l.report(8, at(t + 3000 + 29 * 33 + 10), None);
        assert!(current.is_none());
        assert_eq!(loads.len(), 1, "the level hasn't settled yet");
        let boot = &loads[0];
        assert_eq!(
            (boot.kind.as_str(), boot.stall_ms, boot.settled),
            ("boot", 2000.0, true)
        );
        assert_eq!(boot.segments[0].counts["read_bytes"], 9000);
        l.on_tick(
            at(t + 3000 + 30 * 33),
            Some("01a_tutorial".into()),
            Some(0),
            230,
            totals(181, 5_009_100, 12),
        );
        let (loads, _) = l.report(8, at(t + 5000), None);
        let lvl = &loads[1];
        assert_eq!(lvl.kind, "map");
        assert_eq!(lvl.from_map.as_deref(), Some("mainmenu"));
        assert_eq!(
            (lvl.map.as_deref(), lvl.bsp),
            (Some("01a_tutorial"), Some(0))
        );
        assert_eq!(lvl.stall_ms, 3000.0);
        assert!(lvl.settled);
        assert_eq!(lvl.settle_ms, 990.0);
        let stall = &lvl.segments[0];
        assert_eq!((stall.name.as_str(), stall.frames), ("stall", 70));
        assert_eq!(stall.counts["read_bytes"], 5_000_000);
        assert_eq!(stall.counts["shaders"], 6);
        let settle = &lvl.segments[1];
        assert_eq!(settle.counts["read_bytes"], 100);
        assert_eq!(settle.counts["shaders"], 2);
        assert_eq!(l.report(1, at(t + 5000), None).0.len(), 1, "last n");
    }

    #[test]
    fn a_bsp_switch_and_a_stall_now() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut l = LoadTracker::new(t0);
        l.on_tick(at(10), Some("03a".into()), Some(0), 1, None);
        // The BSP changes after 100 ms: a load though short; then a second
        // switch before it settles.
        l.on_tick(at(110), Some("03a".into()), Some(1), 2, None);
        l.on_tick(at(140), Some("03a".into()), Some(2), 3, None);
        let (loads, current) = l.report(8, at(150), None);
        assert_eq!(loads.len(), 2);
        assert_eq!(
            (loads[1].kind.as_str(), loads[1].from_bsp, loads[1].bsp),
            ("bsp", Some(0), Some(1))
        );
        assert!(!loads[1].settled, "cut short by the next switch");
        assert_eq!(loads[1].stall_ms, 100.0);
        assert!(current.is_none());
        assert!(l.no_frame_stats);
        let (_, current) = l.report(8, at(1140), None);
        let c = current.unwrap();
        assert_eq!(
            (c.kind.as_str(), c.stall_ms, c.from_bsp),
            ("stall", 1000.0, Some(2))
        );
    }

    #[test]
    fn before_the_first_tick_the_boot_is_current() {
        let t0 = Instant::now();
        let l = LoadTracker::new(t0);
        let (loads, current) = l.report(8, t0 + Duration::from_millis(400), totals(3, 10, 0));
        assert!(loads.is_empty());
        let c = current.unwrap();
        assert_eq!((c.kind.as_str(), c.stall_ms), ("boot", 400.0));
        assert_eq!(c.segments[0].counts["read_bytes"], 10);
    }
}
