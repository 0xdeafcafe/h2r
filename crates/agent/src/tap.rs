//! Input as a player gives it: timed pad packets per controller, fed to the
//! game through `xapi`'s input tap, the layer `H2_INPUT_RECORD` and
//! `H2_INPUT_REPLAY` use. The guest's `XInputGetState` returns them as it
//! would a pad's, so the game can't tell an agent from a person.
//!
//! [`AgentTap`] is the tap; it shares an [`Input`] with the executor, which
//! queues packets and moves the clock (the game's own tick) at each tick.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use agent_proto::{AtTick, Button, InputMode, PORTS, PadParams, ReplayCounts};
use input::gamepad::{AnalogButton, Buttons, Gamepad};
use xapi::tap::{InputLog, InputTap, PACKET, Recorder, ReplayStats, Replayer};

/// One queued packet.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Packet {
    port: u32,
    /// Holds for reads while `from <= clock < until`.
    from: u64,
    until: u64,
    /// Later packets win when two hold at once.
    seq: u64,
    pad: [u8; Gamepad::SIZE],
}

/// What one handle was last given, to keep its packet numbers moving.
#[derive(Debug, Clone, Copy, Default)]
struct Delivered {
    /// Added to the live packet number from the first agent packet on.
    offset: u32,
    number: u32,
    pad: [u8; Gamepad::SIZE],
    seen: bool,
}

/// Recording or replaying through the tap.
#[derive(Default)]
enum Mode {
    #[default]
    Live,
    Recording {
        name: String,
        path: PathBuf,
        recorder: Recorder,
        log: Arc<Mutex<InputLog>>,
        from_tick: u64,
    },
    Replaying {
        name: String,
        path: PathBuf,
        reads: u64,
        replayer: Replayer,
        stats: Arc<Mutex<ReplayStats>>,
    },
}

/// The queue and what the tap knows of the game's controllers.
#[derive(Default)]
pub struct Input {
    /// The game's tick as last stamped.
    clock: u64,
    /// The same, for the recorder's tick stamps.
    ticks: Arc<AtomicU64>,
    mode: Mode,
    queue: Vec<Packet>,
    next_seq: u64,
    /// Open handles and the port each was opened on.
    ports: BTreeMap<u32, u32>,
    delivered: BTreeMap<u32, Delivered>,
    /// An [`AgentTap`] was made (the game has XAPI input).
    pub(crate) tap: bool,
}

/// A packet the executor accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Queued {
    pub from: u64,
    pub until: u64,
    pub queued: usize,
    pub open: bool,
}

impl Input {
    /// The game's tick moved (or a map loaded and it reset): packets that
    /// are done are dropped.
    pub fn set_clock(&mut self, tick: u64) {
        self.clock = tick;
        self.ticks.store(tick, Ordering::Relaxed);
        self.queue.retain(|p| p.until > tick);
    }

    /// Drops every queued packet, or one player's.
    pub fn clear(&mut self, port: Option<u32>) -> usize {
        let before = self.queue.len();
        self.queue
            .retain(|p| port.is_some_and(|port| p.port != port));
        before - self.queue.len()
    }

    /// Queues a packet; `Err` says what's wrong with it.
    pub fn push(&mut self, p: &PadParams) -> Result<Queued, String> {
        if p.player >= PORTS {
            return Err(format!("player must be 0 to {}", PORTS - 1));
        }
        let hold = p.hold_ticks.unwrap_or(PadParams::DEFAULT_HOLD);
        if !(1..=PadParams::MAX_HOLD).contains(&hold) {
            return Err(format!("hold_ticks must be 1 to {}", PadParams::MAX_HOLD));
        }
        let from = match (p.at_tick, p.delay_ticks) {
            (Some(_), Some(_)) => return Err("give at_tick or delay_ticks, not both".into()),
            (Some(AtTick::Tick(t)), None) => t,
            (None, Some(d)) => self.clock.saturating_add(d),
            (Some(AtTick::Now(_)), None) | (None, None) => self.clock,
        };
        let until = from.saturating_add(hold);
        if until <= self.clock {
            return Err(format!(
                "it would end at tick {until}, and the game is at tick {}",
                self.clock
            ));
        }
        let pad = gamepad(p)?.to_bytes();
        self.next_seq += 1;
        self.queue.push(Packet {
            port: p.player,
            from,
            until,
            seq: self.next_seq,
            pad,
        });
        Ok(Queued {
            from,
            until,
            queued: self.queued(p.player),
            open: self.open_ports().contains(&p.player),
        })
    }

    pub fn queued(&self, port: u32) -> usize {
        self.queue.iter().filter(|p| p.port == port).count()
    }

    pub fn queued_by_port(&self) -> BTreeMap<u32, usize> {
        let mut out = BTreeMap::new();
        for p in &self.queue {
            *out.entry(p.port).or_insert(0) += 1;
        }
        out
    }

    pub fn open_ports(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.ports.values().copied().collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Starts logging every read to `path` (written at [`stop`](Self::stop)).
    pub fn record(&mut self, name: &str, path: PathBuf) -> Result<(), String> {
        self.must_be_live()?;
        let (recorder, log) = Recorder::new();
        self.mode = Mode::Recording {
            name: name.to_owned(),
            path,
            recorder: recorder.with_ticks(Arc::clone(&self.ticks)),
            log,
            from_tick: self.clock,
        };
        Ok(())
    }

    /// Answers reads from `log`, read for read, from the next read on.
    pub fn replay(&mut self, name: &str, path: PathBuf, log: InputLog) -> Result<(), String> {
        self.must_be_live()?;
        let reads = log.reads.len() as u64;
        let (replayer, stats) = Replayer::new(log);
        // The log's packet numbers are what the game got: the agent's own
        // numbering starts again on top of them.
        self.delivered.clear();
        self.mode = Mode::Replaying {
            name: name.to_owned(),
            path,
            reads,
            replayer,
            stats,
        };
        Ok(())
    }

    fn must_be_live(&self) -> Result<(), String> {
        match &self.mode {
            Mode::Live => Ok(()),
            Mode::Recording { name, .. } => {
                Err(format!("already recording {name}; input.stop first"))
            }
            Mode::Replaying { name, .. } => {
                Err(format!("already replaying {name}; input.stop first"))
            }
        }
    }

    /// What the tap is doing, as the wire says it.
    pub fn mode(&self) -> InputMode {
        match &self.mode {
            Mode::Live => InputMode::Live,
            Mode::Recording {
                name,
                path,
                log,
                from_tick,
                ..
            } => InputMode::Recording {
                name: name.clone(),
                path: path.display().to_string(),
                reads: log.lock().unwrap_or_else(|e| e.into_inner()).reads.len() as u64,
                from_tick: Some(*from_tick),
            },
            Mode::Replaying {
                name,
                path,
                reads,
                stats,
                ..
            } => {
                let s = *stats.lock().unwrap_or_else(|e| e.into_inner());
                InputMode::Replaying {
                    name: name.clone(),
                    path: path.display().to_string(),
                    counts: ReplayCounts {
                        reads: *reads,
                        replayed: s.replayed,
                        matched: s.replayed - s.handle_mismatches,
                        handle_mismatches: s.handle_mismatches,
                        past_end: s.past_end,
                    },
                }
            }
        }
    }

    /// Ends a recording or replay: what it was, and for a recording the
    /// log to write. `None` when live.
    pub fn stop(&mut self) -> Option<(InputMode, Option<(PathBuf, InputLog)>)> {
        let ended = self.mode();
        match std::mem::take(&mut self.mode) {
            Mode::Live => None,
            Mode::Recording { path, log, .. } => {
                let log = std::mem::take(&mut *log.lock().unwrap_or_else(|e| e.into_inner()));
                Some((ended, Some((path, log))))
            }
            Mode::Replaying { .. } => Some((ended, None)),
        }
    }

    fn holding(&self, port: u32) -> Option<&Packet> {
        self.queue
            .iter()
            .filter(|p| p.port == port && p.from <= self.clock && self.clock < p.until)
            .max_by_key(|p| p.seq)
    }

    /// One guest read of `handle`: a replay answers it, then a holding pad
    /// packet replaces the pad, then a recording logs what the game gets.
    fn read(&mut self, handle: u32, status: &mut u32, packet: &mut [u8; PACKET]) {
        if let Mode::Replaying { replayer, .. } = &mut self.mode {
            replayer.read(handle, status, packet);
        }
        self.pad(handle, packet);
        if let Mode::Recording { recorder, .. } = &mut self.mode {
            recorder.read(handle, status, packet);
        }
    }

    /// A holding packet for the handle's port replaces the pad. The packet
    /// number moves whenever what the game gets changes, as a real pad's
    /// does; reads are left alone until the first packet.
    fn pad(&mut self, handle: u32, packet: &mut [u8; PACKET]) {
        let port = self.ports.get(&handle).copied();
        let pad = port.and_then(|port| self.holding(port)).map(|p| p.pad);
        let d = self.delivered.entry(handle).or_default();
        if let Some(pad) = pad {
            packet[4..].copy_from_slice(&pad);
        }
        let live = u32::from_le_bytes(packet[..4].try_into().unwrap());
        let mut number = live.wrapping_add(d.offset);
        let now: [u8; Gamepad::SIZE] = packet[4..].try_into().unwrap();
        // Until the agent first changes a pad, reads pass untouched.
        let touched = pad.is_some() || d.offset != 0;
        if touched && d.seen && now != d.pad && number == d.number {
            d.offset = d.offset.wrapping_add(1);
            number = number.wrapping_add(1);
        }
        if touched {
            packet[..4].copy_from_slice(&number.to_le_bytes());
        }
        d.number = number;
        d.pad = now;
        d.seen = true;
    }
}

/// The pad a packet describes.
pub fn gamepad(p: &PadParams) -> Result<Gamepad, String> {
    let mut g = Gamepad::default();
    for b in &p.buttons {
        match b {
            Button::A => g.analog[AnalogButton::A.index()] = 255,
            Button::B => g.analog[AnalogButton::B.index()] = 255,
            Button::X => g.analog[AnalogButton::X.index()] = 255,
            Button::Y => g.analog[AnalogButton::Y.index()] = 255,
            Button::Black => g.analog[AnalogButton::Black.index()] = 255,
            Button::White => g.analog[AnalogButton::White.index()] = 255,
            Button::Start => g.buttons.set(Buttons::START, true),
            Button::Back => g.buttons.set(Buttons::BACK, true),
            Button::DpadUp => g.buttons.set(Buttons::DPAD_UP, true),
            Button::DpadDown => g.buttons.set(Buttons::DPAD_DOWN, true),
            Button::DpadLeft => g.buttons.set(Buttons::DPAD_LEFT, true),
            Button::DpadRight => g.buttons.set(Buttons::DPAD_RIGHT, true),
            Button::LeftThumb => g.buttons.set(Buttons::LEFT_THUMB, true),
            Button::RightThumb => g.buttons.set(Buttons::RIGHT_THUMB, true),
        }
    }
    if let Some(s) = p.sticks {
        if let Some([x, y]) = s.left {
            g.thumb_lx = axis(x, "sticks.left")?;
            g.thumb_ly = axis(y, "sticks.left")?;
        }
        if let Some([x, y]) = s.right {
            g.thumb_rx = axis(x, "sticks.right")?;
            g.thumb_ry = axis(y, "sticks.right")?;
        }
    }
    if let Some(t) = p.triggers {
        if let Some(v) = t.left {
            g.analog[AnalogButton::LeftTrigger.index()] = trigger(v, "triggers.left")?;
        }
        if let Some(v) = t.right {
            g.analog[AnalogButton::RightTrigger.index()] = trigger(v, "triggers.right")?;
        }
    }
    Ok(g)
}

fn axis(v: f64, what: &str) -> Result<i16, String> {
    if !(-1.0..=1.0).contains(&v) {
        return Err(format!("{what}: each axis is -1 to 1"));
    }
    Ok((v * 32767.0).round() as i16)
}

fn trigger(v: f64, what: &str) -> Result<u8, String> {
    if !(0.0..=1.0).contains(&v) {
        return Err(format!("{what}: 0 to 1"));
    }
    Ok((v * 255.0).round() as u8)
}

/// Where another tap (`H2_INPUT_RECORD` or `H2_INPUT_REPLAY`'s) goes
/// relative to the agent's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chain {
    /// Before: it stands in for the live pad (a replay), and the agent's
    /// packets apply on top.
    Before,
    /// After: it sees what the game gets (a recording), agent packets
    /// included.
    After,
}

/// The input tap: agent packets over the live (or replayed) pad.
pub struct AgentTap {
    input: Arc<Mutex<Input>>,
    inner: Option<(Box<dyn InputTap>, Chain)>,
}

impl AgentTap {
    pub(crate) fn new(input: Arc<Mutex<Input>>, inner: Option<(Box<dyn InputTap>, Chain)>) -> Self {
        lock(&input).tap = true;
        AgentTap { input, inner }
    }
}

pub(crate) fn lock(input: &Mutex<Input>) -> MutexGuard<'_, Input> {
    input.lock().unwrap_or_else(|e| e.into_inner())
}

impl InputTap for AgentTap {
    fn read(&mut self, handle: u32, status: &mut u32, packet: &mut [u8; PACKET]) {
        if let Some((t, Chain::Before)) = &mut self.inner {
            t.read(handle, status, packet);
        }
        lock(&self.input).read(handle, status, packet);
        if let Some((t, Chain::After)) = &mut self.inner {
            t.read(handle, status, packet);
        }
    }

    fn opened(&mut self, handle: u32, port: u32) {
        {
            let mut i = lock(&self.input);
            i.ports.insert(handle, port);
            i.delivered.remove(&handle);
        }
        if let Some((t, _)) = &mut self.inner {
            t.opened(handle, port);
        }
    }

    fn closed(&mut self, handle: u32) {
        {
            let mut i = lock(&self.input);
            i.ports.remove(&handle);
            i.delivered.remove(&handle);
        }
        if let Some((t, _)) = &mut self.inner {
            t.closed(handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::{Now, Sticks, Triggers};
    use input::gamepad::State;

    const H0: u32 = 0x5800_0000;
    const H1: u32 = 0x5800_0004;

    fn live(number: u32, lx: i16) -> [u8; PACKET] {
        State {
            packet_number: number,
            gamepad: Gamepad {
                thumb_lx: lx,
                ..Default::default()
            },
        }
        .to_bytes()
    }

    fn read(tap: &mut AgentTap, handle: u32, from: [u8; PACKET]) -> (u32, Gamepad) {
        let (mut status, mut p) = (0, from);
        tap.read(handle, &mut status, &mut p);
        let number = u32::from_le_bytes(p[..4].try_into().unwrap());
        (number, Gamepad::from_bytes(p[4..].try_into().unwrap()))
    }

    fn tap() -> (Arc<Mutex<Input>>, AgentTap) {
        let input = Arc::new(Mutex::new(Input::default()));
        let mut t = AgentTap::new(Arc::clone(&input), None);
        t.opened(H0, 0);
        t.opened(H1, 1);
        (input, t)
    }

    #[test]
    fn a_packet_maps_buttons_sticks_and_triggers_to_the_xbox_pad() {
        let g = gamepad(&PadParams {
            buttons: vec![
                Button::A,
                Button::Start,
                Button::DpadLeft,
                Button::RightThumb,
            ],
            sticks: Some(Sticks {
                left: Some([1.0, -1.0]),
                right: Some([0.5, 0.0]),
            }),
            triggers: Some(Triggers {
                left: None,
                right: Some(1.0),
            }),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(g.analog(AnalogButton::A), 255);
        assert_eq!(g.analog(AnalogButton::B), 0);
        assert_eq!(g.analog(AnalogButton::RightTrigger), 255);
        assert_eq!(
            g.buttons,
            Buttons::START | Buttons::DPAD_LEFT | Buttons::RIGHT_THUMB
        );
        assert_eq!((g.thumb_lx, g.thumb_ly), (32767, -32767));
        assert_eq!((g.thumb_rx, g.thumb_ry), (16384, 0));
        for bad in [
            PadParams {
                sticks: Some(Sticks {
                    left: Some([1.5, 0.0]),
                    right: None,
                }),
                ..Default::default()
            },
            PadParams {
                triggers: Some(Triggers {
                    left: Some(-0.1),
                    right: None,
                }),
                ..Default::default()
            },
        ] {
            assert!(gamepad(&bad).is_err());
        }
    }

    #[test]
    fn a_packet_holds_for_its_ticks_on_its_player_only_then_the_pad_is_live_again() {
        let (input, mut t) = tap();
        lock(&input).set_clock(10);
        let q = lock(&input)
            .push(&PadParams {
                player: 1,
                hold_ticks: Some(2),
                buttons: vec![Button::A],
                ..Default::default()
            })
            .unwrap();
        assert_eq!((q.from, q.until, q.queued, q.open), (10, 12, 1, true));
        // Player 0 is untouched, byte for byte.
        let (mut s, mut p) = (0, live(7, 300));
        t.read(H0, &mut s, &mut p);
        assert_eq!(p, live(7, 300));
        // Player 1 gets the packet for ticks 10 and 11.
        for tick in [10, 11] {
            lock(&input).set_clock(tick);
            let (_, g) = read(&mut t, H1, live(7, 300));
            assert_eq!(g.analog(AnalogButton::A), 255, "tick {tick}");
            assert_eq!(g.thumb_lx, 0, "the whole pad is the packet's");
        }
        lock(&input).set_clock(12);
        let (_, g) = read(&mut t, H1, live(7, 300));
        assert_eq!((g.analog(AnalogButton::A), g.thumb_lx), (0, 300));
        assert_eq!(lock(&input).queued(1), 0, "a finished packet is dropped");
    }

    #[test]
    fn packets_wait_for_their_tick_and_the_later_one_wins_an_overlap() {
        let (input, mut t) = tap();
        lock(&input).set_clock(5);
        let mut i = lock(&input);
        i.push(&PadParams {
            at_tick: Some(AtTick::Tick(8)),
            hold_ticks: Some(10),
            buttons: vec![Button::X],
            ..Default::default()
        })
        .unwrap();
        let q = i
            .push(&PadParams {
                delay_ticks: Some(4),
                buttons: vec![Button::Y],
                ..Default::default()
            })
            .unwrap();
        assert_eq!((q.from, q.until), (9, 10));
        assert!(
            i.push(&PadParams {
                at_tick: Some(AtTick::Tick(1)),
                hold_ticks: Some(2),
                ..Default::default()
            })
            .is_err(),
            "already over"
        );
        assert!(
            i.push(&PadParams {
                at_tick: Some(AtTick::Now(Now::Now)),
                delay_ticks: Some(1),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            i.push(&PadParams {
                player: 4,
                ..Default::default()
            })
            .is_err()
        );
        drop(i);
        let pressed = |t: &mut AgentTap, tick| {
            lock(&input).set_clock(tick);
            let (_, g) = read(t, H0, live(1, 0));
            (g.analog(AnalogButton::X), g.analog(AnalogButton::Y))
        };
        assert_eq!(pressed(&mut t, 7), (0, 0));
        assert_eq!(pressed(&mut t, 8), (255, 0));
        assert_eq!(pressed(&mut t, 9), (0, 255), "the later packet wins");
        assert_eq!(pressed(&mut t, 10), (255, 0));
        assert_eq!(lock(&input).clear(Some(0)), 1);
        assert_eq!(pressed(&mut t, 11), (0, 0));
    }

    #[test]
    fn the_packet_number_moves_when_the_agent_changes_the_pad() {
        let (input, mut t) = tap();
        // The live pad's number doesn't move (nobody touches it), but the
        // pad the game gets does: so must the number.
        assert_eq!(read(&mut t, H0, live(40, 0)).0, 40);
        lock(&input)
            .push(&PadParams {
                hold_ticks: Some(1),
                buttons: vec![Button::B],
                ..Default::default()
            })
            .unwrap();
        let (n1, _) = read(&mut t, H0, live(40, 0));
        assert_eq!(n1, 41);
        let (n2, _) = read(&mut t, H0, live(40, 0));
        assert_eq!(n2, 41, "same pad, same number");
        lock(&input).set_clock(1);
        let (n3, g) = read(&mut t, H0, live(40, 0));
        assert_eq!((n3, g.analog(AnalogButton::B)), (42, 0), "released");
        assert_eq!(read(&mut t, H0, live(43, 5)).0, 45, "live moves keep it");
    }

    #[test]
    fn a_recording_replays_read_for_read_with_the_agent_packets_in_it() {
        let (input, mut t) = tap();
        lock(&input).set_clock(20);
        lock(&input)
            .record("fight", "/c/fight.input".into())
            .unwrap();
        assert!(lock(&input).record("again", "/c/x".into()).is_err());
        lock(&input)
            .push(&PadParams {
                hold_ticks: Some(2),
                buttons: vec![Button::A],
                ..Default::default()
            })
            .unwrap();
        let mut seen = Vec::new();
        for tick in 20..24 {
            lock(&input).set_clock(tick);
            seen.push(read(&mut t, H0, live(3, tick as i16)));
            seen.push(read(&mut t, H1, live(3, -1)));
        }
        let InputMode::Recording {
            reads, from_tick, ..
        } = lock(&input).mode()
        else {
            panic!("recording")
        };
        assert_eq!((reads, from_tick), (8, Some(20)));
        let (ended, file) = lock(&input).stop().unwrap();
        assert!(matches!(ended, InputMode::Recording { reads: 8, .. }));
        let (path, log) = file.unwrap();
        assert_eq!(path, PathBuf::from("/c/fight.input"));
        assert_eq!(log.reads[0].tick, Some(20), "stamped with the game tick");
        assert!(lock(&input).stop().is_none(), "live again");

        // Replayed over a live pad that does something else entirely.
        let log = InputLog::parse(&log.to_text()).unwrap();
        lock(&input)
            .replay("fight", "/c/fight.input".into(), log)
            .unwrap();
        let mut got = Vec::new();
        for _ in 20..24 {
            got.push(read(&mut t, H0, live(77, 999)));
            got.push(read(&mut t, H1, live(77, 999)));
        }
        assert_eq!(got, seen);
        read(&mut t, H0, live(77, 999));
        let InputMode::Replaying { counts, .. } = lock(&input).mode() else {
            panic!("replaying")
        };
        assert_eq!(
            counts,
            ReplayCounts {
                reads: 8,
                replayed: 8,
                matched: 8,
                handle_mismatches: 0,
                past_end: 1
            }
        );
    }

    #[test]
    fn a_chained_replay_comes_first_and_a_chained_recording_sees_the_agent() {
        use xapi::tap::{InputLog, Read, Recorder, Replayer};
        let input = Arc::new(Mutex::new(Input::default()));
        let (rec, log) = Recorder::new();
        let mut t = AgentTap::new(Arc::clone(&input), Some((Box::new(rec), Chain::After)));
        t.opened(H0, 0);
        lock(&input)
            .push(&PadParams {
                buttons: vec![Button::A],
                ..Default::default()
            })
            .unwrap();
        read(&mut t, H0, live(1, 0));
        let got = log.lock().unwrap().reads[0].packet;
        assert_eq!(
            Gamepad::from_bytes(got[4..].try_into().unwrap()).analog[0],
            255
        );

        let replay = InputLog {
            reads: vec![Read {
                tick: None,
                handle: H0,
                status: 0,
                packet: live(9, 1234),
            }],
        };
        let (rp, _) = Replayer::new(replay);
        let input = Arc::new(Mutex::new(Input::default()));
        let mut t = AgentTap::new(Arc::clone(&input), Some((Box::new(rp), Chain::Before)));
        t.opened(H0, 0);
        let (n, g) = read(&mut t, H0, live(1, 0));
        assert_eq!((n, g.thumb_lx), (9, 1234), "the replay stands in for live");
    }
}
