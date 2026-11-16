//! Two co-op peers over a simulated link (docs/live/live-coop.md, phase 0): the
//! pads each machine's guest would read are the same sequence on both,
//! whatever the link does; a frame waits when the other port's pad is late
//! and the game resumes when it comes; the relay carries the game while the
//! direct path is dead; and a differing tick hash stops both machines.

use coop::link::{Conditions, FakeClock, Fallback, Link, Path, SimControl, UdpLink, sim_pair};
use coop::schedule::PAD;
use coop::{Desync, Hold, Pad, Peer};

const FRAME: u64 = 33_333;
const STEP: u64 = 1_000;
const SESSION: u32 = 0x00c0_0b01;

/// What player `port` holds on their `n`th sample: different per port and
/// changing every sample, so a frame shifted by one shows.
fn script(port: usize, n: u32) -> Pad {
    let mut pad = [0u8; PAD];
    pad[0] = port as u8 + 1;
    pad[2..6].copy_from_slice(&n.to_le_bytes());
    pad[6] = (n % 7) as u8;
    pad
}

/// The agreed pads of frame `f` when port `p` runs `delay[p]` frames behind
/// its samples: the answer a link with no latency gives.
fn expected(delay: [u32; 2], frames: u32) -> Vec<[Pad; 2]> {
    (0..frames)
        .map(|f| {
            std::array::from_fn(|p| match f.checked_sub(delay[p]) {
                Some(n) => script(p, n),
                None => [0; PAD],
            })
        })
        .collect()
}

fn fold(state: u64, pads: &[Pad; 2]) -> u64 {
    pads.iter().flatten().fold(state, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// One machine: a peer, and a stand-in guest that reads the frame's pads
/// and folds them into a state it hashes every tick.
struct Machine<L: Link> {
    peer: Peer<L>,
    port: usize,
    due: u64,
    seen: Vec<[Pad; 2]>,
    state: u64,
    /// Flips the state on this tick: a machine that computed something else.
    glitch_at: Option<u32>,
    /// Holds counted when frame 10 started (after the start-up wait).
    holds_at_10: u64,
    stopped: Option<Desync>,
}

impl<L: Link> Machine<L> {
    fn new(link: L, port: usize, delay: u32) -> Self {
        Machine {
            peer: Peer::new(link, SESSION, port, delay),
            port,
            due: 0,
            seen: Vec::new(),
            state: 0xcbf2_9ce4_8422_2325,
            glitch_at: None,
            holds_at_10: 0,
            stopped: None,
        }
    }

    fn step(&mut self, now: u64, frames: u32) {
        self.peer.pump();
        if now < self.due || self.seen.len() as u32 >= frames || self.stopped.is_some() {
            return;
        }
        match self
            .peer
            .begin_frame(script(self.port, self.seen.len() as u32))
        {
            Ok(frame) => {
                assert_eq!(frame as usize, self.seen.len());
                let pads = self.peer.pads().unwrap();
                self.state = fold(self.state, &pads);
                if self.glitch_at == Some(frame) {
                    self.state ^= 1;
                }
                self.peer.report_hash(frame, self.state);
                self.seen.push(pads);
                self.due = now + FRAME;
                if frame == 10 {
                    self.holds_at_10 = self.peer.stats().holds;
                }
            }
            Err(Hold::Waiting { frame }) => assert_eq!(frame as usize, self.seen.len()),
            Err(Hold::Desync(d)) => self.stopped = Some(d),
        }
    }

    fn holds_after_start(&self) -> u64 {
        self.peer.stats().holds - self.holds_at_10
    }
}

struct World<L: Link> {
    clock: FakeClock,
    a: Machine<L>,
    b: Machine<L>,
}

impl<L: Link> World<L> {
    /// Runs until both machines have run `frames` frames or stopped, for at
    /// most `limit` of simulated time; `each` runs every step.
    fn run(
        &mut self,
        frames: u32,
        limit: u64,
        mut each: impl FnMut(u64, &Machine<L>, &Machine<L>),
    ) {
        let done = |m: &Machine<L>| m.seen.len() as u32 >= frames || m.stopped.is_some();
        let end = self.clock.now() + limit;
        while !(done(&self.a) && done(&self.b)) {
            assert!(
                self.clock.now() < end,
                "stuck at frames {} and {}",
                self.a.seen.len(),
                self.b.seen.len()
            );
            self.clock.advance(STEP);
            let now = self.clock.now();
            self.a.step(now, frames);
            self.b.step(now, frames);
            each(now, &self.a, &self.b);
        }
    }
}

type Sim = World<coop::link::SimLink>;

fn sim(conditions: Conditions, delay: [u32; 2], seed: u64) -> (Sim, SimControl) {
    let clock = FakeClock::default();
    let (la, lb, control) = sim_pair(&clock, conditions, seed);
    (
        World {
            clock,
            a: Machine::new(la, 0, delay[0]),
            b: Machine::new(lb, 1, delay[1]),
        },
        control,
    )
}

fn ms(n: u64) -> u64 {
    n * 1000
}

#[test]
fn inside_the_delay_budget_no_frame_waits() {
    // Two frames of delay are 67 ms; the link takes 30 to 40.
    let (mut w, _) = sim(
        Conditions {
            latency: ms(30),
            jitter: ms(10),
            ..Default::default()
        },
        [2, 2],
        1,
    );
    w.run(300, ms(20_000), |_, _, _| {});
    assert_eq!(w.a.seen, expected([2, 2], 300));
    assert_eq!(w.b.seen, w.a.seen);
    assert_eq!((w.a.holds_after_start(), w.b.holds_after_start()), (0, 0));
    // The last frames' hashes are still on the wire when the run ends.
    assert!(w.a.peer.stats().ticks_agreed >= 290 && w.b.peer.stats().ticks_agreed >= 290);
}

#[test]
fn over_the_budget_frames_wait_and_the_stream_does_not_change() {
    let (mut w, _) = sim(
        Conditions {
            latency: ms(150),
            jitter: ms(40),
            ..Default::default()
        },
        [2, 2],
        2,
    );
    w.run(200, ms(60_000), |_, _, _| {});
    assert!(w.a.holds_after_start() > 0 && w.b.holds_after_start() > 0);
    assert_eq!(w.a.seen, expected([2, 2], 200));
    assert_eq!(w.b.seen, w.a.seen);
}

#[test]
fn loss_and_reordering_change_nothing_the_guest_reads() {
    for seed in 1..=8 {
        let (mut w, control) = sim(
            Conditions {
                latency: ms(25),
                jitter: ms(60),
                loss: 0.3,
                reorder: 0.2,
            },
            [2, 2],
            seed,
        );
        w.run(300, ms(60_000), |_, _, _| {});
        assert!(control.dropped() > 100, "the link did lose datagrams");
        assert_eq!(w.a.seen, expected([2, 2], 300), "seed {seed}");
        assert_eq!(w.b.seen, w.a.seen, "seed {seed}");
        assert_eq!(w.a.peer.stats().rejected + w.b.peer.stats().rejected, 0);
        assert_eq!(w.a.peer.desync().or(w.b.peer.desync()), None);
    }
}

#[test]
fn a_dead_link_stalls_both_and_they_resume() {
    let (mut w, control) = sim(
        Conditions {
            latency: ms(20),
            ..Default::default()
        },
        [2, 2],
        5,
    );
    let (mut at_cut, mut at_restore) = ((0, 0), (0, 0));
    w.run(240, ms(60_000), |now, a, b| {
        let frames = (a.seen.len(), b.seen.len());
        if now == ms(2_000) {
            control.set_down(true);
            at_cut = frames;
        }
        if now == ms(5_000) {
            at_restore = frames;
            control.set_down(false);
        }
    });
    // Three seconds are 90 frames. With the link down each machine can run
    // only the frames it already held pads for: its delay's worth.
    assert!(at_cut.0 > 50 && at_cut.1 > 50, "{at_cut:?}");
    assert!(
        at_restore.0 <= at_cut.0 + 4 && at_restore.1 <= at_cut.1 + 4,
        "{at_cut:?} {at_restore:?}"
    );
    assert!(w.a.peer.stats().holds > 2_000, "held for the outage");
    assert_eq!(w.a.seen, expected([2, 2], 240));
    assert_eq!(w.b.seen, w.a.seen);
}

#[test]
fn the_relay_carries_the_game_while_direct_is_dead() {
    let clock = FakeClock::default();
    let fast = Conditions {
        latency: ms(10),
        ..Default::default()
    };
    let slow = Conditions {
        latency: ms(25),
        ..Default::default()
    };
    let (da, db, direct) = sim_pair(&clock, fast, 1);
    let (ra, rb, relay) = sim_pair(&clock, slow, 2);
    direct.set_down(true);
    let timeout = ms(500);
    let mut w = World {
        a: Machine::new(Fallback::new(da, ra, clock.clock(), timeout), 0, 2),
        b: Machine::new(Fallback::new(db, rb, clock.clock(), timeout), 1, 2),
        clock,
    };
    let mut relay_sent_when_direct_took_over = None;
    w.run(300, ms(60_000), |now, a, b| {
        if now == ms(3_000) {
            // Direct never delivered anything: the game is on the relay.
            assert_eq!(direct.delivered(), 0);
            assert!(a.seen.len() > 60 && b.seen.len() > 60);
            assert_eq!(
                (a.peer.link().path(), b.peer.link().path()),
                (Path::Relay, Path::Relay)
            );
            direct.set_down(false);
        }
        if now == ms(4_000) {
            assert_eq!(
                (a.peer.link().path(), b.peer.link().path()),
                (Path::Direct, Path::Direct)
            );
            relay_sent_when_direct_took_over = Some(relay.sent());
        }
    });
    assert_eq!(w.a.seen, expected([2, 2], 300));
    assert_eq!(w.b.seen, w.a.seen);
    assert_eq!(
        Some(relay.sent()),
        relay_sent_when_direct_took_over,
        "the relay went quiet once direct worked"
    );
    assert_eq!(
        w.a.holds_after_start() + w.b.holds_after_start(),
        0,
        "the switch cost no frame"
    );
}

#[test]
fn a_differing_hash_stops_both_machines_at_that_tick() {
    let (mut w, _) = sim(
        Conditions {
            latency: ms(30),
            jitter: ms(20),
            loss: 0.1,
            ..Default::default()
        },
        [2, 2],
        9,
    );
    w.b.glitch_at = Some(40);
    w.run(300, ms(60_000), |_, _, _| {});
    let (da, db) = (w.a.stopped.unwrap(), w.b.stopped.unwrap());
    assert_eq!((da.tick, db.tick), (40, 40));
    assert_eq!((da.local, da.remote), (db.remote, db.local));
    assert_ne!(da.local, da.remote);
    // Every tick before it agreed, and neither machine got far past it:
    // the report takes one exchange, a few frames at this latency.
    assert_eq!(w.a.peer.stats().ticks_agreed, 40);
    assert_eq!(w.b.peer.stats().ticks_agreed, 40);
    assert!(
        w.a.seen.len() < 50 && w.b.seen.len() < 50,
        "{} {}",
        w.a.seen.len(),
        w.b.seen.len()
    );
    // And it stays stopped.
    assert_eq!(w.a.peer.begin_frame([0; PAD]), Err(Hold::Desync(da)));
}

#[test]
fn each_machine_has_its_own_delay_and_may_change_it() {
    let (mut w, _) = sim(
        Conditions {
            latency: ms(40),
            jitter: ms(30),
            loss: 0.05,
            ..Default::default()
        },
        [1, 4],
        11,
    );
    let mut changed = false;
    // Run to frame 100, change A's delay, run on.
    w.run(100, ms(60_000), |_, _, _| {});
    w.a.peer.set_delay(3);
    w.run(260, ms(60_000), |_, a, _| changed |= a.seen.len() > 100);
    assert!(changed);
    assert_eq!(w.b.seen, w.a.seen, "both guests read the same pads");
    // B's port is plain delay 4 throughout.
    let b_port: Vec<Pad> = w.a.seen.iter().map(|p| p[1]).collect();
    let want: Vec<Pad> = expected([1, 4], 260).iter().map(|p| p[1]).collect();
    assert_eq!(b_port, want);
    // A's port: delay 1, then its sample at the change repeated over the
    // two new frames, then delay 3. No frame is missing and none repeats
    // out of order.
    let a_samples: Vec<u32> =
        w.a.seen
            .iter()
            .skip(1)
            .map(|p| u32::from_le_bytes(p[0][2..6].try_into().unwrap()))
            .collect();
    assert!(
        a_samples
            .windows(2)
            .all(|w| w[1] == w[0] || w[1] == w[0] + 1)
    );
    assert_eq!(a_samples.iter().filter(|n| **n == 100).count(), 3);
    assert_eq!(*a_samples.last().unwrap(), 259 - 3);
}

/// A link that also delivers things nobody on the session sent.
struct Noisy<L> {
    inner: L,
    junk: Vec<Vec<u8>>,
}

impl<L: Link> Link for Noisy<L> {
    fn send(&mut self, d: &[u8]) {
        self.inner.send(d)
    }
    fn recv(&mut self) -> Option<Vec<u8>> {
        self.junk.pop().or_else(|| self.inner.recv())
    }
}

#[test]
fn datagrams_that_are_not_this_sessions_are_dropped() {
    let clock = FakeClock::default();
    let (la, lb, _) = sim_pair(&clock, Conditions::default(), 1);
    // A whole, valid datagram of another session, claiming frames and an
    // acknowledgement far ahead; and plain noise.
    let other = coop::wire::Datagram {
        session: SESSION + 1,
        ack_frame: 1000,
        ack_tick: 1000,
        first_frame: 0,
        frames: vec![[0xee; PAD]; 8],
        first_tick: 0,
        hashes: vec![1, 2, 3],
    };
    let junk = vec![other.encode(), b"not co-op".to_vec(), Vec::new()];
    let mut w = World {
        clock,
        a: Machine::new(Noisy { inner: la, junk }, 0, 2),
        b: Machine::new(
            Noisy {
                inner: lb,
                junk: Vec::new(),
            },
            1,
            2,
        ),
    };
    w.run(60, ms(10_000), |_, _, _| {});
    assert_eq!(w.a.peer.stats().rejected, 3);
    assert_eq!(w.a.seen, expected([2, 2], 60));
    assert_eq!(w.b.seen, w.a.seen);
}

#[test]
fn two_peers_over_real_udp_on_loopback() {
    let any: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut la = UdpLink::bind(any, any).unwrap();
    let lb = UdpLink::bind(any, la.local_addr().unwrap()).unwrap();
    la.set_peer(lb.local_addr().unwrap());
    let mut a = Machine::new(la, 0, 2);
    let mut b = Machine::new(lb, 1, 2);
    // No frame timer: as fast as the pads arrive.
    let started = std::time::Instant::now();
    while a.seen.len() < 120 || b.seen.len() < 120 {
        assert!(
            started.elapsed().as_secs() < 20,
            "stuck at {} and {}",
            a.seen.len(),
            b.seen.len()
        );
        a.due = 0;
        b.due = 0;
        a.step(0, 120);
        b.step(0, 120);
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
    assert_eq!(a.seen, expected([2, 2], 120));
    assert_eq!(b.seen, a.seen);
    assert!(a.peer.stats().ticks_agreed.min(b.peer.stats().ticks_agreed) > 100);
}
