//! Serial, batched and pipelined sampling give the same brains.
use std::sync::{Arc, Mutex};

use guest_layout::testing::FakeGuest;
use input::host::HostPadState;

use super::*;
use crate::brain::{Me, Observations, Perception, Seen};
use crate::cache::MapData;
use crate::decisions::radio::Radio;
use crate::emote::Kill;
use crate::mind::Skill;
use crate::Brain;

pub(crate) mod fixture {
    use super::*;

    pub const BOTS: u32 = 6;
    /// The bot that dies, when, and for how long.
    const VICTIM: u32 = 3;
    const DEATH: u32 = 40;
    const DEAD_FOR: u32 = 15;

    fn position(id: u32, tick: u32) -> [f32; 3] {
        let a = id as f32 * 1.047 + tick as f32 * 0.03 * (1 + id % 3) as f32;
        let r = 4.0 + id as f32;
        [a.cos() * r, a.sin() * r, 0.0]
    }

    fn alive(id: u32, tick: u32) -> bool {
        id != VICTIM || !(DEATH..DEATH + DEAD_FOR).contains(&tick)
    }

    /// What bot `id` sees at `tick` in a two-team game: everyone alive, a
    /// death heard by all at [`DEATH`].
    pub fn frame(id: u32, tick: u32) -> (Perception, Vec<Kill>) {
        let team = (id % 2) as i16;
        let me = alive(id, tick).then(|| {
            let at = position(id, tick);
            let len = (at[0] * at[0] + at[1] * at[1]).sqrt().max(1e-3);
            Me {
                position: at,
                aim: [-at[0] / len, -at[1] / len, 0.0],
                team,
                shield: if tick % 50 < 10 { 0.3 } else { 1.0 },
                ..Me::default()
            }
        });
        let others = (0..BOTS)
            .filter(|&o| o != id && alive(o, tick))
            .map(|o| Seen {
                id: o,
                position: position(o, tick),
                team: (o % 2) as i16,
            })
            .collect();
        let kills = if tick == DEATH {
            vec![Kill {
                seq: 1,
                tick,
                victim: VICTIM,
                attacker: Some(2),
                betrayal: false,
                at: Some(position(VICTIM, tick)),
            }]
        } else {
            vec![]
        };
        (
            Perception {
                observations: Observations {
                    player: Some((id, team)),
                    ..Default::default()
                },
                me,
                others,
                teams: true,
                tick,
                ..Default::default()
            },
            kills,
        )
    }

    /// A brain fed from [`frame`] instead of the guest, keeping a log of
    /// what each think left it doing and saying.
    pub struct Fixture {
        pub id: u32,
        pub brain: Brain,
        pub map: Option<Arc<MapData>>,
        pub log: Arc<Mutex<Vec<String>>>,
    }

    impl PadSource for Fixture {
        fn pad(&mut self, f: &Frame<'_>) -> HostPadState {
            match self.observe(f) {
                Sampled::Pad(pad) => pad,
                Sampled::Think => {
                    let pad = self.think();
                    self.conclude();
                    pad
                }
            }
        }

        fn observe(&mut self, f: &Frame<'_>) -> Sampled {
            let (p, kills) = frame(self.id, f.tick);
            self.brain.observe_fixture(p, &kills, self.map.as_ref())
        }

        fn think(&mut self) -> HostPadState {
            let pad = self.brain.think();
            let m = &self.brain.mind;
            lock(&self.log).push(format!(
                "{:?} {:?} {:?}",
                m.doing, m.decisions.report, pad
            ));
            pad
        }

        fn conclude(&mut self) {
            self.brain.conclude();
        }

        fn reset_input(&mut self) {
            self.brain.reset_input();
        }

        fn brain_view(&self) -> Option<crate::BrainView> {
            Some(self.brain.view())
        }

        fn combat_stats(&self) -> Option<&crate::stats::CombatStats> {
            Some(self.brain.combat_stats())
        }
    }

    pub struct World {
        pub board: &'static Mutex<Radio>,
        pub sources: Vec<Source>,
        pub logs: Vec<Arc<Mutex<Vec<String>>>>,
    }

    pub fn world(seed: u64) -> World {
        let board: &'static Mutex<Radio> = Box::leak(Box::new(Mutex::new(Radio::default())));
        let map = Some(Arc::new(MapData::default()));
        let mut sources = Vec::new();
        let mut logs = Vec::new();
        for id in 0..BOTS {
            let log = Arc::new(Mutex::new(Vec::new()));
            logs.push(Arc::clone(&log));
            let skill = Skill::ALL[(id % 4) as usize];
            let brain = Brain::new(skill, seed + u64::from(id)).on_radio(board);
            let fixture = Fixture {
                id,
                brain,
                map: map.clone(),
                log,
            };
            sources.push(Arc::new(Mutex::new(Box::new(fixture) as Box<dyn PadSource>)));
        }
        World {
            board,
            sources,
            logs,
        }
    }

    /// Every seat in seat order, none reset.
    pub fn seats(w: &World, tick: u32) -> Vec<Seat<'_>> {
        w.sources
            .iter()
            .enumerate()
            .map(|(i, source)| Seat {
                key: i as u32,
                generation: 0,
                source,
                frame: u64::from(tick),
                machine: [0xc1, 0xa4, 0, 0, 0, i as u8],
            })
            .collect()
    }
}

use fixture::*;

/// What a run leaves: each tick's pads, each bot's thinks, and the radio.
#[derive(Debug, PartialEq)]
struct Run {
    pads: Vec<Vec<HostPadState>>,
    shot_ranges: Vec<Vec<Option<Option<f32>>>>,
    logs: Vec<Vec<String>>,
    views: Vec<Option<crate::BrainView>>,
    radio: String,
}

const TICKS: u32 = 120;

fn run(mode: Mode) -> Run {
    let w = world(7);
    let mut thinker = Thinker::on_radio(mode, w.board);
    let g = FakeGuest::new();
    let (pads, shot_ranges) = (0..TICKS)
        .map(|tick| {
            thinker
                .sample_reports(tick, &g, &seats(&w, tick))
                .into_iter()
                .map(|report| (report.pad, report.shot_range))
                .unzip::<_, _, Vec<_>, Vec<_>>()
        })
        .unzip();
    thinker.settle();
    Run {
        pads,
        shot_ranges,
        logs: w.logs.iter().map(|l| lock(l).clone()).collect(),
        views: w
            .sources
            .iter()
            .map(|s| lock_source(s).brain_view())
            .collect(),
        radio: format!("{:?}", radio::lock(w.board).digest()),
    }
}

fn batched(threads: usize) -> Mode {
    Mode::Batched {
        threads,
        pipeline: false,
    }
}

fn pipelined(threads: usize) -> Mode {
    Mode::Batched {
        threads,
        pipeline: true,
    }
}

#[test]
fn the_fixture_talks_on_the_radio() {
    // Otherwise the comparisons below would not cover the publish order.
    let serial = run(Mode::Serial);
    let reports = serial
        .logs
        .iter()
        .flatten()
        .filter(|l| l.contains("Some(Call"))
        .count();
    assert!(reports > 0, "no bot reported a sighting");
    assert!(serial.shot_ranges.iter().flatten().any(Option::is_some));
    assert!(serial.pads.iter().flatten().any(|p| *p != HostPadState::default()));
}

#[test]
fn batched_brains_match_serial_at_any_thread_count() {
    let serial = run(Mode::Serial);
    for threads in [0, 1, 2, 3, 8] {
        assert_eq!(run(batched(threads)), serial, "{threads} threads");
    }
}

#[test]
fn pipelined_pads_are_the_serial_pads_one_tick_later() {
    let serial = run(Mode::Serial);
    for threads in [0, 1, 2, 5] {
        let piped = run(pipelined(threads));
        assert_eq!(piped.pads[0], vec![HostPadState::default(); BOTS as usize]);
        for tick in 1..TICKS as usize {
            assert_eq!(piped.pads[tick], serial.pads[tick - 1], "tick {tick}, {threads} threads");
            assert_eq!(piped.shot_ranges[tick], serial.shot_ranges[tick - 1]);
        }
        // The same thinks, the same minds and the same radio: only the
        // pads' timing moved.
        assert_eq!(piped.logs, serial.logs, "{threads} threads");
        assert_eq!(piped.views, serial.views);
        assert_eq!(piped.radio, serial.radio);
    }
}

#[test]
fn a_pipelined_think_from_before_a_reset_is_dropped() {
    let w = world(7);
    let mut thinker = Thinker::on_radio(pipelined(2), w.board);
    let g = FakeGuest::new();
    let mut last = Vec::new();
    for tick in 0..12 {
        let mut seats = seats(&w, tick);
        if tick >= 10 {
            // Bot 2's stream was interrupted at tick 10.
            seats[2].generation = 1;
            if tick == 10 {
                lock_source(seats[2].source).reset_input();
            }
        }
        last.push(thinker.sample(tick, &g, &seats));
    }
    assert_eq!(last[10][2], HostPadState::default());
    // The other bots' thinks went on: their tick-10 pads are serial's
    // tick-9 pads (the fixture may still be idle then, so compare, not
    // just look for a press).
    let serial = run(Mode::Serial);
    for i in (0..BOTS as usize).filter(|&i| i != 2) {
        assert_eq!(last[10][i], serial.pads[9][i], "bot {i}");
    }
    // A gap in ticks drops every think in flight.
    let pads = thinker.sample(20, &g, &seats(&w, 20));
    assert!(pads.iter().all(|p| *p == HostPadState::default()));
}

#[test]
fn a_seat_with_its_pad_at_hand_is_not_delayed() {
    let source: Source = Arc::new(Mutex::new(Box::new(|f: &Frame<'_>| HostPadState {
        right_trigger: if f.tick % 2 == 0 { 1.0 } else { 0.0 },
        ..Default::default()
    }) as Box<dyn PadSource>));
    let board: &'static Mutex<Radio> = Box::leak(Box::default());
    let mut thinker = Thinker::on_radio(pipelined(1), board);
    let g = FakeGuest::new();
    for tick in 0..4 {
        let seat = Seat {
            key: 0,
            generation: 0,
            source: &source,
            frame: 0,
            machine: [0; 6],
        };
        let pads = thinker.sample(tick, &g, &[seat]);
        assert_eq!(pads[0].right_trigger, if tick % 2 == 0 { 1.0 } else { 0.0 });
    }
}

fn fault() {
    panic!("a brain fault");
}

#[test]
fn the_pool_runs_every_job_and_carries_a_panic() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for threads in [0, 1, 4] {
        let pool = Pool::new(threads);
        assert_eq!(pool.threads(), threads);
        let done = Arc::new(AtomicUsize::new(0));
        for _ in 0..3 {
            let jobs = (0..50)
                .map(|_| {
                    let done = Arc::clone(&done);
                    Box::new(move || {
                        done.fetch_add(1, Ordering::Relaxed);
                    }) as Job
                })
                .collect();
            pool.submit(jobs);
            pool.wait();
        }
        assert_eq!(done.load(Ordering::Relaxed), 150);
        pool.submit(vec![Box::new(fault) as Job]);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.wait()));
        assert!(caught.is_err(), "{threads} threads");
        // The pool still works after.
        pool.submit(vec![Box::new(|| {}) as Job]);
        pool.wait();
    }
}

#[test]
fn the_mode_comes_from_the_environment_values() {
    assert_eq!(Mode::from_values(None, None), Mode::Serial);
    assert_eq!(Mode::from_values(Some("0"), Some("off")), Mode::Serial);
    assert_eq!(Mode::from_values(Some("off"), None), Mode::Serial);
    assert_eq!(Mode::from_values(Some("3"), None), batched(3));
    assert_eq!(Mode::from_values(Some(" 4 "), Some("on")), pipelined(4));
    assert_eq!(Mode::from_values(Some("999"), None), batched(MAX_THREADS));
    assert_eq!(Mode::from_values(Some("lots"), None), Mode::Serial);
    let auto = Mode::from_values(Some("auto"), Some("1"));
    assert!(matches!(auto, Mode::Batched { threads: 1..=8, pipeline: true }));
    assert!(Mode::from_values(None, Some("yes")).pipelined());
}

#[test]
fn leaving_a_game_discards_a_queued_batch_without_running_it_on_the_caller() {
    let w = world(7);
    let mut thinker = Thinker::on_radio(pipelined(0), w.board);
    let g = FakeGuest::new();
    thinker.sample(0, &g, &seats(&w, 0));
    thinker.end_game();
    assert!(thinker.ready_for_game());
    assert!(thinker.in_flight.is_none());
    assert!(w.logs.iter().all(|log| lock(log).is_empty()));
}

#[test]
fn a_mode_change_settles_the_thinks_in_flight() {
    let w = world(7);
    let mut thinker = Thinker::on_radio(pipelined(2), w.board);
    let g = FakeGuest::new();
    thinker.sample(0, &g, &seats(&w, 0));
    thinker.set_mode(batched(2));
    assert!(thinker.in_flight.is_none());
    // The batch the pipelined tick opened was closed by its conclusion.
    let pads = thinker.sample(1, &g, &seats(&w, 1));
    assert_eq!(pads.len(), BOTS as usize);
}
