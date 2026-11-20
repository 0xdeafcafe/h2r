//! Two toy guests (`lockstep::synthetic`) played in step over a bad link:
//! each machine's guest reads its pads through the schedule tap, the
//! simulation hash of every tick goes to the other machine, and the
//! whole-RAM traces of the two runs are identical. Then one machine's
//! memory is changed behind the game's back, and both stop at that tick.

use std::sync::Arc;

use coop::link::{Conditions, FakeClock, SimLink, sim_pair};
use coop::schedule::PAD;
use coop::tap::ScheduleTap;
use coop::{Desync, Hold, Pad, Peer};
use lockstep::synthetic::{self, Toy};
use lockstep::{Recorder, Trace, Verdict, compare, hash_sim, install_tick_observer};
use xapi::tap::InputTap;

const H: [u32; 2] = [0x5800_0000, 0x5800_0004];
const CROUCH: u16 = 0x40;
const FRAME: u64 = 33_333;

fn port_of(handle: u32) -> Option<usize> {
    H.iter().position(|h| *h == handle)
}

/// A pad as `XINPUT_GAMEPAD` bytes: buttons, the A pressure, left stick X.
fn pad(buttons: u16, a: u8, lx: i16) -> Pad {
    let state = synthetic::packet(0, buttons, a, lx);
    state[4..].try_into().unwrap()
}

/// Player one walks and crouches, player two jumps: together they are the
/// walk that lands the toy's super bounce (lockstep's own test), so one
/// pad a frame late anywhere sends the run somewhere else.
fn play(port: usize, n: u32) -> Pad {
    match port {
        0 => pad(if (58..78).contains(&n) { CROUCH } else { 0 }, 0, 20_000),
        _ => pad(0, if n % 24 < 3 { 255 } else { 0 }, 0),
    }
}

struct Machine {
    peer: Peer<SimLink>,
    tap: ScheduleTap,
    toy: Toy,
    recorder: Arc<Recorder>,
    port: usize,
    frames: u32,
    due: u64,
    peak: i32,
    stopped: Option<Desync>,
    /// Pokes the toy's memory after this frame's tick.
    corrupt_at: Option<u32>,
}

impl Machine {
    fn new(link: SimLink, port: usize) -> Machine {
        let peer = Peer::new(link, 7, port, 2);
        let tap = peer.tap(port_of);
        let toy = Toy::new();
        let recorder = Recorder::new(synthetic::HOOK.game_time);
        install_tick_observer(&toy.runtime, synthetic::HOOK, Arc::clone(&recorder)).unwrap();
        Machine {
            peer,
            tap,
            toy,
            recorder,
            port,
            frames: 0,
            due: 0,
            peak: 0,
            stopped: None,
            corrupt_at: None,
        }
    }

    fn step(&mut self, now: u64, frames: u32) {
        self.peer.pump();
        if now < self.due || self.frames >= frames || self.stopped.is_some() {
            return;
        }
        match self.peer.begin_frame(play(self.port, self.frames)) {
            Ok(frame) => {
                // The guest's two XInputGetState calls, through the tap; a
                // live device that would say something else entirely.
                let mut read = |handle| {
                    let (mut status, mut packet) = (0, [0x55; PAD + 4]);
                    self.tap.read(handle, &mut status, &mut packet);
                    assert_eq!(status, 0);
                    packet
                };
                let (p0, p1) = (read(H[0]), read(H[1]));
                // The toy has one pad: player one's buttons and stick,
                // player two's A button.
                let mut packet = p0;
                packet[6] = p1[6];
                self.toy.set_input(&packet);
                self.toy.frame();
                self.peak = self.peak.max(self.toy.height());
                if self.corrupt_at == Some(frame) {
                    let x = self.toy.runtime.memory().u32(synthetic::PLAYER).unwrap();
                    memory_set(&self.toy, x ^ 0x100);
                }
                let hash = hash_sim(&self.toy.runtime.memory(), synthetic::SIM).unwrap();
                self.peer.report_hash(frame, hash);
                self.frames += 1;
                self.due = now + FRAME;
            }
            Err(Hold::Waiting { .. }) => {}
            Err(Hold::Desync(d)) => self.stopped = Some(d),
        }
    }

    fn trace(&self) -> Trace {
        self.recorder.trace()
    }
}

fn memory_set(toy: &Toy, x: u32) {
    toy.runtime
        .memory()
        .write_bytes(synthetic::PLAYER, &x.to_le_bytes(), 1)
        .unwrap();
}

fn run(corrupt: Option<u32>, frames: u32) -> (Machine, Machine) {
    let clock = FakeClock::default();
    let (la, lb, _) = sim_pair(
        &clock,
        Conditions {
            latency: 35_000,
            jitter: 45_000,
            loss: 0.2,
            reorder: 0.1,
        },
        3,
    );
    let (mut a, mut b) = (Machine::new(la, 0), Machine::new(lb, 1));
    b.corrupt_at = corrupt;
    let done = |m: &Machine| m.frames >= frames || m.stopped.is_some();
    while !(done(&a) && done(&b)) {
        assert!(
            clock.now() < 60_000_000,
            "stuck at {} and {}",
            a.frames,
            b.frames
        );
        clock.advance(1_000);
        a.step(clock.now(), frames);
        b.step(clock.now(), frames);
    }
    (a, b)
}

#[test]
fn two_toy_guests_stay_identical_over_a_bad_link() {
    let (a, b) = run(None, 90);
    assert_eq!(
        a.tap.starved().load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert!(a.peer.stats().holds > 0, "the link made frames wait");
    let (ta, tb) = (a.trace(), b.trace());
    assert_eq!(ta.ticks(), 90);
    assert!(compare(&ta, &tb).identical(), "{:?}", compare(&ta, &tb));
    assert_eq!((a.peer.desync(), b.peer.desync()), (None, None));
    // Both players' inputs reached the game on the frames they were meant
    // for: the walk and the jumps together land the bounce.
    assert_eq!(a.peak, b.peak);
    assert!(a.peak > 100 * 256, "no bounce: peak {}", a.peak);
}

#[test]
fn a_changed_word_on_one_machine_stops_both_at_that_tick() {
    let (a, b) = run(Some(30), 90);
    let (da, db) = (a.stopped.unwrap(), b.stopped.unwrap());
    assert_eq!((da.tick, db.tick), (30, 30));
    assert_eq!((da.local, da.remote), (db.remote, db.local));
    assert!(a.frames < 45 && b.frames < 45, "{} {}", a.frames, b.frames);
    // The whole-RAM traces agree: the tick observer hashes when the tick
    // returns, so the change made after tick 30 first shows in tick 31.
    match compare(&a.trace(), &b.trace()) {
        Verdict::Diverged { index, chunks, .. } => {
            assert_eq!(index, 31);
            assert_eq!(chunks.len(), 1, "one MiB of RAM differs");
        }
        v => panic!("the traces should diverge: {v:?}"),
    }
}
