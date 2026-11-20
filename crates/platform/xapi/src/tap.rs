//! Input recording and replay at the guest's own reads (docs/game/high-framerate.md,
//! phase 1).
//!
//! An [`InputTap`] sees every `XInputGetState` result just before it is
//! written to the guest, and may replace it. [`Recorder`] logs each read's
//! status and packet bytes; [`Replayer`] hands read *n* of a log back as read
//! *n* of a later run. Keying by read index rather than by time makes replay
//! exact regardless of when the host pumped input: the guest reads the same
//! bytes at the same point of its own execution, so with the same schedule
//! it runs the same instructions. Each read can also carry the guest tick it
//! fell in (`lockstep::Recorder::tick_counter`), for people reading the log.
//!
//! The log is ours (button and stick values), not game data, so recordings
//! can be committed. Only `XInputGetState` is tapped: device arrival,
//! capabilities and rumble come from the live backend in both runs, so record
//! and replay over the same kind of pad (a scripted or keyboard pad is
//! always connected).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use input::gamepad::State;

/// Bytes of one `XINPUT_STATE` (packet number and gamepad).
pub const PACKET: usize = State::SIZE;

/// Sees, and may change, what one `XInputGetState` gives the guest.
pub trait InputTap: Send {
    /// `status` is 0 for success or the Win32 error the call returns
    /// (1167 for a pad removed since the handle was opened); `packet` is what
    /// is written to the guest's `XINPUT_STATE`.
    fn read(&mut self, handle: u32, status: &mut u32, packet: &mut [u8; PACKET]);

    /// The live guest view at this read, for input contingent on current
    /// menu focus. Recording and replay need only the packet.
    fn read_with_memory(
        &mut self,
        handle: u32,
        status: &mut u32,
        packet: &mut [u8; PACKET],
        _memory: Option<&dyn crate::GuestMemory>,
    ) {
        self.read(handle, status, packet);
    }

    /// Pump the four controller ports. A film replays device arrivals and
    /// removals here, including a replacement on the same port between reads.
    fn sync(&mut self, input: &mut input::InputManager) {
        input.sync();
    }

    /// Restore any devices supplied by the tap before XAPI reopens its
    /// saved handles. Ordinary read logs only need to seek their cursor.
    fn restore_input(&mut self, cursor: u64, _input: &mut input::InputManager) -> Result<(), String> {
        self.seek(cursor)
    }

    /// The guest opened `handle` on controller `port` (`XInputOpen`), so a
    /// tap that acts per player knows which port a read is for.
    fn opened(&mut self, _handle: u32, _port: u32) {}

    /// The guest closed `handle` (`XInputClose`).
    fn closed(&mut self, _handle: u32) {}

    /// Where this tap is in its log, for a machine snapshot's input cursor
    /// (docs/runtime/snapshots.md): a recorder's read count, a replayer's
    /// next read. `None` for a tap that keeps no log.
    fn cursor(&self) -> Option<u64> {
        None
    }

    /// Carry on from a loaded snapshot's cursor: a replayer answers read
    /// `cursor` next, a recorder forgets the reads after it. Fails when the
    /// log does not reach the cursor. A tap that keeps no log ignores it.
    fn seek(&mut self, _cursor: u64) -> Result<(), String> {
        Ok(())
    }
}

/// One guest read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Read {
    /// Guest ticks completed before this read, when a tick counter was given.
    pub tick: Option<u64>,
    pub handle: u32,
    pub status: u32,
    pub packet: [u8; PACKET],
}

/// Every read of a run, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputLog {
    pub reads: Vec<Read>,
}

/// The first line of an input log.
pub const HEADER: &str = "# h2 input log v1";

impl InputLog {
    /// `read <n> tick=<t|-> handle=<hex> status=<n> packet=<44 hex>` per read.
    pub fn to_text(&self) -> String {
        use std::fmt::Write as _;
        let mut s = format!("{HEADER}\n");
        for (i, r) in self.reads.iter().enumerate() {
            let tick = r.tick.map_or("-".to_owned(), |t| t.to_string());
            let _ = write!(
                s,
                "read {i} tick={tick} handle={:08x} status={} packet=",
                r.handle, r.status
            );
            for b in r.packet {
                let _ = write!(s, "{b:02x}");
            }
            s.push('\n');
        }
        s
    }

    pub fn parse(text: &str) -> Result<InputLog, String> {
        let mut lines = text.lines().enumerate();
        match lines.next() {
            Some((_, h)) if h.trim() == HEADER => {}
            _ => return Err(format!("not an input log (first line is not {HEADER:?})")),
        }
        let mut reads = Vec::new();
        for (n, line) in lines {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let bad = |what: &str| format!("line {}: {what}: {line:?}", n + 1);
            let mut words = line.split_whitespace();
            if words.next() != Some("read") {
                return Err(bad("expected `read`"));
            }
            let index: usize = words
                .next()
                .and_then(|w| w.parse().ok())
                .ok_or_else(|| bad("read number"))?;
            if index != reads.len() {
                return Err(bad("reads out of order"));
            }
            let mut field = |key: &str| -> Result<&str, String> {
                words
                    .next()
                    .and_then(|w| w.strip_prefix(key))
                    .and_then(|w| w.strip_prefix('='))
                    .ok_or_else(|| bad(key))
            };
            let tick = match field("tick")? {
                "-" => None,
                t => Some(t.parse().map_err(|_| bad("tick"))?),
            };
            let handle = u32::from_str_radix(field("handle")?, 16).map_err(|_| bad("handle"))?;
            let status = field("status")?.parse().map_err(|_| bad("status"))?;
            let hex = field("packet")?;
            if hex.len() != PACKET * 2 || !hex.is_ascii() {
                return Err(bad("packet length"));
            }
            let mut packet = [0u8; PACKET];
            for (i, b) in packet.iter_mut().enumerate() {
                *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| bad("packet"))?;
            }
            reads.push(Read {
                tick,
                handle,
                status,
                packet,
            });
        }
        Ok(InputLog { reads })
    }
}

/// Logs every read into a shared [`InputLog`].
pub struct Recorder {
    log: Arc<Mutex<InputLog>>,
    ticks: Option<Arc<AtomicU64>>,
}

impl Recorder {
    /// A recorder and the log it fills.
    pub fn new() -> (Self, Arc<Mutex<InputLog>>) {
        let log = Arc::new(Mutex::new(InputLog::default()));
        (
            Self {
                log: Arc::clone(&log),
                ticks: None,
            },
            log,
        )
    }

    /// A recorder that continues `log`: after loading a machine snapshot,
    /// pass the saved run's reads up to the snapshot's input cursor, and
    /// the new reads follow them with the same numbering.
    pub fn resume(log: InputLog) -> (Self, Arc<Mutex<InputLog>>) {
        let (recorder, shared) = Self::new();
        *shared.lock().unwrap_or_else(|e| e.into_inner()) = log;
        (recorder, shared)
    }

    /// Stamps each read with the current value of `ticks`.
    pub fn with_ticks(mut self, ticks: Arc<AtomicU64>) -> Self {
        self.ticks = Some(ticks);
        self
    }
}

impl InputTap for Recorder {
    fn read(&mut self, handle: u32, status: &mut u32, packet: &mut [u8; PACKET]) {
        let tick = self.ticks.as_ref().map(|t| t.load(Ordering::Relaxed));
        self.log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reads
            .push(Read {
                tick,
                handle,
                status: *status,
                packet: *packet,
            });
    }

    fn cursor(&self) -> Option<u64> {
        Some(
            self.log
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .reads
                .len() as u64,
        )
    }

    fn seek(&mut self, cursor: u64) -> Result<(), String> {
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        let have = log.reads.len();
        if cursor > have as u64 {
            return Err(format!(
                "the recording has {have} reads, the snapshot was taken after {cursor}"
            ));
        }
        log.reads.truncate(cursor as usize);
        Ok(())
    }
}

/// What a replay has done so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayStats {
    /// Reads answered from the log.
    pub replayed: u64,
    /// Reads past the log's end, left live.
    pub past_end: u64,
    /// Replayed reads whose handle differs from the recording's: the runs
    /// opened their pads differently, so they have already parted.
    pub handle_mismatches: u64,
}

/// Answers read *n* with the log's read *n*.
pub struct Replayer {
    log: InputLog,
    next: usize,
    stats: Arc<Mutex<ReplayStats>>,
}

impl Replayer {
    /// A replayer and a handle on its counters.
    pub fn new(log: InputLog) -> (Self, Arc<Mutex<ReplayStats>>) {
        let stats = Arc::new(Mutex::new(ReplayStats::default()));
        (
            Self {
                log,
                next: 0,
                stats: Arc::clone(&stats),
            },
            stats,
        )
    }

    /// A replayer that carries on from a machine snapshot: its first read
    /// is the log's read `next`, the input cursor the snapshot recorded
    /// (the saving run's [`ReplayStats::replayed`], or its recorder's log
    /// length). `replayed` starts at `next`, so it stays the cursor.
    pub fn resume(log: InputLog, next: usize) -> (Self, Arc<Mutex<ReplayStats>>) {
        let (mut replayer, stats) = Self::new(log);
        replayer.next = next;
        stats.lock().unwrap_or_else(|e| e.into_inner()).replayed = next as u64;
        (replayer, stats)
    }
}

impl InputTap for Replayer {
    fn read(&mut self, handle: u32, status: &mut u32, packet: &mut [u8; PACKET]) {
        let mut stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        let Some(r) = self.log.reads.get(self.next) else {
            stats.past_end += 1;
            return;
        };
        self.next += 1;
        stats.replayed += 1;
        if r.handle != handle {
            stats.handle_mismatches += 1;
        }
        *status = r.status;
        *packet = r.packet;
    }

    fn cursor(&self) -> Option<u64> {
        Some(self.next as u64)
    }

    /// As [`Replayer::resume`]: `replayed` becomes the cursor; the other
    /// counters keep counting.
    fn seek(&mut self, cursor: u64) -> Result<(), String> {
        let have = self.log.reads.len();
        if cursor > have as u64 {
            return Err(format!(
                "the replayed log has {have} reads, the snapshot was taken after {cursor}"
            ));
        }
        self.next = cursor as usize;
        self.stats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replayed = cursor;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(seed: u8) -> [u8; PACKET] {
        std::array::from_fn(|i| seed.wrapping_add(i as u8))
    }

    #[test]
    fn logs_round_trip_through_text() {
        let log = InputLog {
            reads: vec![
                Read {
                    tick: Some(0),
                    handle: 0x5800_0000,
                    status: 0,
                    packet: packet(1),
                },
                Read {
                    tick: None,
                    handle: 0x5800_0004,
                    status: 1167,
                    packet: packet(200),
                },
            ],
        };
        assert_eq!(InputLog::parse(&log.to_text()).unwrap(), log);
        assert!(InputLog::parse("read 0").is_err());
        let one = |l: &str| InputLog::parse(&format!("{HEADER}\n{l}"));
        assert!(
            one("read 1 tick=- handle=0 status=0 packet=00").is_err(),
            "order"
        );
        assert!(
            one("read 0 tick=- handle=0 status=0 packet=00").is_err(),
            "length"
        );
        assert!(
            one(&format!(
                "read 0 tick=- handle=0 status=0 packet={}",
                "zz".repeat(PACKET)
            ))
            .is_err()
        );
    }

    #[test]
    fn the_replayer_answers_by_read_index_then_leaves_reads_live() {
        let (mut rec, log) = Recorder::new();
        let ticks = Arc::new(AtomicU64::new(4));
        rec = rec.with_ticks(Arc::clone(&ticks));
        for i in 0..3u8 {
            let (mut status, mut p) = (0, packet(i));
            rec.read(7, &mut status, &mut p);
            ticks.fetch_add(1, Ordering::Relaxed);
        }
        let log = log.lock().unwrap().clone();
        assert_eq!(
            log.reads.iter().map(|r| r.tick).collect::<Vec<_>>(),
            [Some(4), Some(5), Some(6)]
        );
        let (mut play, stats) = Replayer::new(log);
        for i in 0..3u8 {
            let (mut status, mut p) = (99, packet(100));
            play.read(if i == 1 { 8 } else { 7 }, &mut status, &mut p);
            assert_eq!((status, p), (0, packet(i)));
        }
        let (mut status, mut p) = (5, packet(50));
        play.read(7, &mut status, &mut p);
        assert_eq!((status, p), (5, packet(50)), "past the end: live");
        assert_eq!(
            *stats.lock().unwrap(),
            ReplayStats {
                replayed: 3,
                past_end: 1,
                handle_mismatches: 1
            }
        );
    }

    #[test]
    fn replay_and_recording_resume_from_a_snapshot_cursor() {
        let (mut rec, log) = Recorder::new();
        for i in 0..5u8 {
            rec.read(7, &mut 0, &mut packet(i));
        }
        let log = log.lock().unwrap().clone();

        // A run replaying the log is snapshotted after two reads.
        let (mut play, stats) = Replayer::new(log.clone());
        for _ in 0..2 {
            play.read(7, &mut 0, &mut packet(0));
        }
        let cursor = stats.lock().unwrap().replayed as usize;
        assert_eq!(cursor, 2);

        // Loading the snapshot resumes at read 2, as the run would have.
        let (mut resumed, resumed_stats) = Replayer::resume(log.clone(), cursor);
        for i in 2..5u8 {
            let (mut status, mut p) = (9, packet(100));
            resumed.read(7, &mut status, &mut p);
            assert_eq!((status, p), (0, packet(i)));
        }
        assert_eq!(resumed_stats.lock().unwrap().replayed, 5);

        // Recording on from the snapshot keeps the numbering dense.
        let prefix = InputLog {
            reads: log.reads[..cursor].to_vec(),
        };
        let (mut rec, shared) = Recorder::resume(prefix);
        rec.read(7, &mut 0, &mut packet(42));
        let text = shared.lock().unwrap().to_text();
        let reparsed = InputLog::parse(&text).unwrap();
        assert_eq!(reparsed.reads.len(), 3);
        assert_eq!(reparsed.reads[2].packet, packet(42));
    }
}
