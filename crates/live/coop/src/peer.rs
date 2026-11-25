//! One machine's end of a co-op game: its pad schedule, its link and the
//! tick-hash comparison.
//!
//! The runner's loop, per frame it wants to run:
//!
//! ```text
//! peer.pump();                          // as often as it likes
//! match peer.begin_frame(local_pad) {
//!     Ok(frame) => { run the guest for one frame; peer.report_hash(tick, hash) per tick }
//!     Err(Hold::Waiting { .. }) => { do not run the guest; pump and try again }
//!     Err(Hold::Desync(d)) => { stop and report }
//! }
//! ```
//!
//! Holding the guest is free: wall time is outside it
//! (docs/game/high-framerate.md, "In our runtime").

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::cinematic::{CinematicTicks, Stream};
use crate::link::Link;
use crate::schedule::{Pad, Schedule};
use crate::tap::ScheduleTap;
use crate::wire::{BEAT, Datagram, MAX_FRAMES, MAX_HASHES, forge_room};

/// The two machines computed different states for a tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Desync {
    pub tick: u32,
    pub local: u64,
    pub remote: u64,
}

/// Why a frame may not start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    /// The other port's pad for `frame` has not arrived.
    Waiting { frame: u32 },
    /// The machines have parted; no frame runs after that.
    Desync(Desync),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub sent: u64,
    pub received: u64,
    /// Datagrams that were not this session's, or not co-op datagrams.
    pub rejected: u64,
    /// Times a frame was asked for and had to wait.
    pub holds: u64,
    /// Ticks whose hashes were compared and equal.
    pub ticks_agreed: u64,
}

/// A run of per-tick hashes from tick `first` on.
#[derive(Debug, Default)]
struct Hashes {
    first: u32,
    values: VecDeque<u64>,
}

impl Hashes {
    fn end(&self) -> u32 {
        self.first + self.values.len() as u32
    }

    fn get(&self, tick: u32) -> Option<u64> {
        self.values
            .get(tick.checked_sub(self.first)? as usize)
            .copied()
    }

    fn drop_before(&mut self, tick: u32) {
        while self.first < tick && self.values.pop_front().is_some() {
            self.first += 1;
        }
    }
}

/// The most Forge stream bytes waiting for the other machine's
/// acknowledgement: a welcome's document and log, with room to spare.
pub const FORGE_OUT_MAX: usize = 24 << 20;

/// The datagrams one flush sends while the Forge lane has unacknowledged
/// bytes: the first carries the pads, hashes and one run; each of the
/// rest carries another run, so a long stream (a welcome) crosses in a
/// few round trips instead of one a run. The pump's cadence bounds the
/// datagrams a second, which is what the relay allows.
pub const FORGE_WINDOW: usize = 4;

/// The Forge lane (docs/game/forge-design.md, F5): a reliable ordered byte
/// stream each way, in the datagram's Forge section, and the machine's
/// position in the keep-alive. Like the frames, the stream repeats from the
/// other side's acknowledgement on, so a lost datagram costs nothing.
#[derive(Default)]
struct Forge {
    /// Bytes sent and not acknowledged, from stream offset `first`.
    out: VecDeque<u8>,
    first: u32,
    /// The stream bytes taken from the other machine, in order.
    end: u32,
    inbox: Vec<u8>,
    beat_out: Option<[u8; BEAT]>,
    beat_in: Option<[u8; BEAT]>,
}

pub struct Peer<L> {
    link: L,
    session: u32,
    schedule: Arc<Mutex<Schedule>>,
    local: Hashes,
    remote: Hashes,
    /// The other machine has every local hash before this tick.
    acked_tick: u32,
    /// The next tick to compare.
    compared: u32,
    desync: Option<Desync>,
    stats: Stats,
    host: bool,
    cinematic: Stream,
    ack_cinematic: u32,
    forge: Forge,
}

impl<L: Link> Peer<L> {
    /// `session` is a number both machines derive from the Live session;
    /// `local_port` is 0 on the session's host and 1 on the guest; `delay`
    /// is this machine's input delay in frames.
    pub fn new(link: L, session: u32, local_port: usize, delay: u32) -> Peer<L> {
        Peer {
            link,
            session,
            schedule: Arc::new(Mutex::new(Schedule::new(local_port, delay))),
            local: Hashes::default(),
            remote: Hashes::default(),
            acked_tick: 0,
            compared: 0,
            desync: None,
            stats: Stats::default(),
            host: local_port == 0,
            cinematic: Stream::default(),
            ack_cinematic: 0,
            forge: Forge::default(),
        }
    }

    /// Queue bytes on the Forge lane. They reach the other machine in
    /// order, once. `false` when the other machine is so far behind that
    /// the queue is full; nothing is queued then.
    pub fn forge_send(&mut self, bytes: &[u8]) -> bool {
        if self.forge.out.len() + bytes.len() > FORGE_OUT_MAX {
            return false;
        }
        self.forge.out.extend(bytes);
        true
    }

    /// The Forge bytes that have arrived in order since the last call.
    pub fn forge_recv(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.forge.inbox)
    }

    /// Bytes sent on the Forge lane that the other machine has not
    /// acknowledged.
    pub fn forge_unacked(&self) -> usize {
        self.forge.out.len()
    }

    /// The position this machine reports in every datagram (`None` to stop).
    pub fn forge_set_beat(&mut self, beat: Option<[u8; BEAT]>) {
        self.forge.beat_out = beat;
    }

    /// The other machine's last reported position.
    pub fn forge_beat(&self) -> Option<[u8; BEAT]> {
        self.forge.beat_in
    }

    fn schedule(&self) -> std::sync::MutexGuard<'_, Schedule> {
        self.schedule.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The schedule as an input tap for `xapi` (`port_of` maps an input
    /// handle to its controller port).
    pub fn tap(&self, port_of: impl Fn(u32) -> Option<usize> + Send + 'static) -> ScheduleTap {
        ScheduleTap::new(Arc::clone(&self.schedule), port_of)
    }

    pub fn link(&self) -> &L {
        &self.link
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn desync(&self) -> Option<Desync> {
        self.desync
    }

    pub fn next_frame(&self) -> u32 {
        self.schedule().next_frame()
    }

    /// Both ports' pads in the running frame.
    pub fn pads(&self) -> Option<[Pad; 2]> {
        self.schedule().pads()
    }

    pub fn set_delay(&mut self, delay: u32) {
        self.schedule().set_delay(delay);
    }

    /// Takes everything that has arrived, then sends one datagram.
    pub fn pump(&mut self) {
        self.poll();
        self.flush();
    }

    /// Takes everything that has arrived.
    pub fn poll(&mut self) {
        while let Some(bytes) = self.link.recv() {
            match Datagram::decode(&bytes) {
                Ok(d) if d.session == self.session => self.take(d),
                _ => self.stats.rejected += 1,
            }
        }
        self.compare();
    }

    fn take(&mut self, d: Datagram) {
        if self.host && !d.cinematic.is_empty() {
            self.stats.rejected += 1;
            return;
        }
        if self.host {
            self.ack_cinematic = self.ack_cinematic.max(d.ack_cinematic.min(self.cinematic.end()));
            self.cinematic.discard(self.ack_cinematic.min(self.next_frame()));
        } else {
            self.cinematic.receive(d.first_cinematic, &d.cinematic);
        }
        self.stats.received += 1;
        {
            let mut s = self.schedule();
            s.acknowledge(d.ack_frame);
            s.receive(d.first_frame, &d.frames);
        }
        self.take_forge(&d);
        self.acked_tick = self.acked_tick.max(d.ack_tick.min(self.local.end()));
        // Hashes are contiguous like frames: skip the ones held, ignore a gap.
        let end = self.remote.end();
        if d.first_tick <= end {
            let skip = (end - d.first_tick) as usize;
            self.remote
                .values
                .extend(d.hashes.iter().skip(skip).copied());
        }
    }

    fn take_forge(&mut self, d: &Datagram) {
        let f = &mut self.forge;
        let sent_end = f.first + f.out.len() as u32;
        let acked = d.ack_forge.min(sent_end);
        if acked > f.first {
            f.out.drain(..(acked - f.first) as usize);
            f.first = acked;
        }
        // Bytes are contiguous: skip the ones held, ignore a gap.
        if d.first_forge <= f.end {
            let skip = (f.end - d.first_forge) as usize;
            if let Some(new) = d.forge.get(skip..) {
                f.inbox.extend_from_slice(new);
                f.end += new.len() as u32;
            }
        }
        if d.beat.is_some() {
            f.beat_in = d.beat;
        }
    }

    fn compare(&mut self) {
        while self.desync.is_none() {
            let t = self.compared;
            let (Some(local), Some(remote)) = (self.local.get(t), self.remote.get(t)) else {
                break;
            };
            if local == remote {
                self.stats.ticks_agreed += 1;
                self.compared += 1;
            } else {
                self.desync = Some(Desync {
                    tick: t,
                    local,
                    remote,
                });
            }
        }
        // The remote run is only needed up to the comparison; the local one
        // also until the other machine has it.
        self.remote.drop_before(self.compared);
        self.local.drop_before(self.compared.min(self.acked_tick));
    }

    /// Sends this machine's unacknowledged frames and hashes, and what it
    /// has of the other's. Sent even with nothing new: it is also the
    /// acknowledgement and the keep-alive. The Forge lane's unacknowledged
    /// runs go with it, [`FORGE_WINDOW`] of them, each a datagram that
    /// keeps within the wire cap beside whatever else it carries.
    pub fn flush(&mut self) {
        let (first_frame, frames, ack_frame) = {
            let s = self.schedule();
            let (first, frames) = s.unacknowledged(MAX_FRAMES);
            (first, frames, s.remote_end())
        };
        let first_tick = self.acked_tick.max(self.local.first);
        let hashes: Vec<u64> = self
            .local
            .values
            .iter()
            .skip((first_tick - self.local.first) as usize)
            .take(MAX_HASHES)
            .copied()
            .collect();
        let (first_cinematic, cinematic) = if self.host { self.cinematic.outgoing(MAX_FRAMES) } else { (0, Vec::new()) };
        let beat = self.forge.beat_out;
        let end = self.forge.first + self.forge.out.len() as u32;
        let room = forge_room(frames.len(), hashes.len(), cinematic.len(), beat.is_some());
        let mut at = self.forge.first;
        let d = Datagram {
            first_cinematic, cinematic,
            ack_cinematic: if self.host { 0 } else { self.cinematic.end() },
            session: self.session,
            ack_frame,
            ack_tick: self.remote.end(),
            first_frame,
            frames,
            first_tick,
            hashes,
            ack_forge: self.forge.end,
            first_forge: at,
            forge: self.forge.out.iter().take(room).copied().collect(),
            beat,
        };
        self.link.send(&d.encode());
        self.stats.sent += 1;
        at += d.forge.len() as u32;
        for _ in 1..FORGE_WINDOW {
            if at >= end {
                break;
            }
            // The window's other datagrams carry Forge runs alone: the
            // pads and hashes went with the first, and the runs after the
            // first need none beside them.
            let room = forge_room(0, 0, 0, beat.is_some());
            let d = Datagram {
                first_cinematic: 0,
                cinematic: Vec::new(),
                ack_cinematic: if self.host { 0 } else { self.cinematic.end() },
                session: self.session,
                ack_frame,
                ack_tick: self.remote.end(),
                first_frame,
                frames: Vec::new(),
                first_tick,
                hashes: Vec::new(),
                ack_forge: self.forge.end,
                first_forge: at,
                forge: self
                    .forge
                    .out
                    .iter()
                    .skip((at - self.forge.first) as usize)
                    .take(room)
                    .copied()
                    .collect(),
                beat,
            };
            self.link.send(&d.encode());
            self.stats.sent += 1;
            at += d.forge.len() as u32;
        }
    }

    /// Starts the next frame if both ports' pads for it are in hand:
    /// `local` is this machine's pad now. On a hold nothing changes, and
    /// `local` is not consumed.
    pub fn begin_frame(&mut self, local: Pad) -> Result<u32, Hold> {
        if let Some(d) = self.desync { return Err(Hold::Desync(d)); }
        let mut s = self.schedule();
        match s.begin(local) {
            Some(frame) => Ok(frame),
            None => {
                let frame = s.next_frame();
                drop(s);
                self.stats.holds += 1;
                Err(Hold::Waiting { frame })
            }
        }
    }

    /// Agree the number of extra simulation ticks before releasing the frame.
    /// Both peers call this from frame zero, including frames with zero extra ticks.
    /// A retry repeats the first host decision for that frame.
    pub fn begin_cinematic_frame(&mut self, local: Pad, requested: CinematicTicks) -> Result<(u32, CinematicTicks), Hold> {
        if let Some(d) = self.desync { return Err(Hold::Desync(d)); }
        let frame = self.next_frame();
        if self.host { self.cinematic.receive(frame, &[requested]); }
        let Some(ticks) = self.cinematic.at(frame) else {
            self.stats.holds += 1;
            return Err(Hold::Waiting { frame });
        };
        let frame = self.begin_frame(local)?;
        let end = if self.host { self.ack_cinematic.min(frame + 1) } else { frame + 1 };
        self.cinematic.discard(end);
        Ok((frame, ticks))
    }

    /// The hash of this machine's state after `tick`. Ticks are reported in
    /// order from 0; one out of order is ignored.
    pub fn report_hash(&mut self, tick: u32, hash: u64) {
        if tick == self.local.end() {
            self.local.values.push_back(hash);
            self.compare();
        }
    }
}
