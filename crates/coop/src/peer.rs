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
//! (docs/high-framerate.md, "In our runtime").

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::link::Link;
use crate::schedule::{Pad, Schedule};
use crate::tap::ScheduleTap;
use crate::wire::{Datagram, MAX_FRAMES, MAX_HASHES};

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
        }
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
        self.stats.received += 1;
        {
            let mut s = self.schedule();
            s.acknowledge(d.ack_frame);
            s.receive(d.first_frame, &d.frames);
        }
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
    /// acknowledgement and the keep-alive.
    pub fn flush(&mut self) {
        let (first_frame, frames, ack_frame) = {
            let s = self.schedule();
            let (first, frames) = s.unacknowledged(MAX_FRAMES);
            (first, frames, s.remote_end())
        };
        let first_tick = self.acked_tick.max(self.local.first);
        let hashes = self
            .local
            .values
            .iter()
            .skip((first_tick - self.local.first) as usize)
            .take(MAX_HASHES)
            .copied()
            .collect();
        let d = Datagram {
            session: self.session,
            ack_frame,
            ack_tick: self.remote.end(),
            first_frame,
            frames,
            first_tick,
            hashes,
        };
        self.link.send(&d.encode());
        self.stats.sent += 1;
    }

    /// Starts the next frame if both ports' pads for it are in hand:
    /// `local` is this machine's pad now. On a hold nothing changes, and
    /// `local` is not consumed.
    pub fn begin_frame(&mut self, local: Pad) -> Result<u32, Hold> {
        if let Some(d) = self.desync {
            return Err(Hold::Desync(d));
        }
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

    /// The hash of this machine's state after `tick`. Ticks are reported in
    /// order from 0; one out of order is ignored.
    pub fn report_hash(&mut self, tick: u32, hash: u64) {
        if tick == self.local.end() {
            self.local.values.push_back(hash);
            self.compare();
        }
    }
}
