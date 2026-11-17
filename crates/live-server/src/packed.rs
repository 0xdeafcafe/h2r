//! Packed live frames to stream readers (`/v1/stream?frames=packed`),
//! batched once for everyone instead of once a reader.
//!
//! Ingest appends each packed frame to its game's [`Pending`] records.
//! Every [`TICK`] the server takes them all as one [`GameBatch`] a game
//! and publishes a [`Tick`] on [`Hub`]: readers of every game get the
//! tick (one wake a tick, however many games), readers of one game get
//! that game's batch alone (they are not woken by the others). A tick
//! carries its games' records back to back as well, so a reader that may
//! see them all and follows every stream sends that buffer as it is.
//!
//! Each reader keeps its place in each stream ([`Follow`]): a record goes
//! out only if it follows the last one sent of its stream, or is a
//! keyframe after it. So a reader that joins (sent the frames since each
//! keyframe, which the next tick repeats) or falls behind (sent them again)
//! never gets a frame twice, an older frame, or a delta without its base.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::broadcast;

use crate::games::LiveFrame;

/// How often batches go out: what a packed reader's frames wait for.
pub const TICK: Duration = Duration::from_millis(100);

/// Ticks (or one game's batches) a reader may fall behind before it is
/// sent each stream again from its keyframe: six seconds.
const BACKLOG: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Record {
    seq: u64,
    keyframe: bool,
    /// Where it starts (its length prefix) in the batch's bytes.
    at: usize,
}

/// One game's packed frames from one tick, in order, each as
/// [`live_proto::pack::push_record`] frames it.
#[derive(Debug)]
pub struct GameBatch {
    pub game_id: String,
    pub sid: u64,
    records: Vec<Record>,
    /// The records' sequence numbers follow one another.
    contiguous: bool,
    pub bytes: Bytes,
}

/// One tick's batches, every game that had frames, and their bytes back
/// to back (in the same order).
#[derive(Debug)]
pub struct Tick {
    pub games: Vec<Arc<GameBatch>>,
    pub all: Bytes,
}

/// A game's packed frames since the last tick.
#[derive(Default)]
pub(crate) struct Pending {
    bytes: Vec<u8>,
    records: Vec<Record>,
}

impl Pending {
    pub(crate) fn push(&mut self, seq: u64, keyframe: bool, packed: &[u8]) {
        self.records.push(Record {
            seq,
            keyframe,
            at: self.bytes.len(),
        });
        live_proto::pack::push_record(&mut self.bytes, packed);
    }

    /// The batch, if there were frames, leaving this empty.
    pub(crate) fn take(&mut self, game_id: &str, sid: u64) -> Option<GameBatch> {
        if self.records.is_empty() {
            return None;
        }
        let records = std::mem::take(&mut self.records);
        let contiguous = records.windows(2).all(|w| w[1].seq == w[0].seq + 1);
        Some(GameBatch {
            game_id: game_id.to_owned(),
            sid,
            records,
            contiguous,
            bytes: std::mem::take(&mut self.bytes).into(),
        })
    }
}

/// Where ticks and games' batches are published.
pub struct Hub {
    all: broadcast::Sender<Arc<Tick>>,
    games: Mutex<HashMap<String, broadcast::Sender<Arc<GameBatch>>>>,
}

impl Default for Hub {
    fn default() -> Self {
        Hub {
            all: broadcast::channel(BACKLOG).0,
            games: Mutex::default(),
        }
    }
}

impl Hub {
    pub fn subscribe_all(&self) -> broadcast::Receiver<Arc<Tick>> {
        self.all.subscribe()
    }

    pub fn subscribe_game(&self, game_id: &str) -> broadcast::Receiver<Arc<GameBatch>> {
        let mut games = self.games.lock().unwrap();
        games
            .entry(game_id.to_owned())
            .or_insert_with(|| broadcast::channel(BACKLOG).0)
            .subscribe()
    }

    /// Whether anyone reads packed frames (else batches are dropped
    /// unmade).
    pub fn has_readers(&self) -> bool {
        self.all.receiver_count() > 0 || !self.games.lock().unwrap().is_empty()
    }

    /// Sends a tick's batches: the whole tick to readers of every game,
    /// each batch to its game's readers. Games nobody reads alone any
    /// more lose their channel.
    pub fn publish(&self, batches: Vec<GameBatch>) {
        let games: Vec<Arc<GameBatch>> = batches.into_iter().map(Arc::new).collect();
        {
            let mut channels = self.games.lock().unwrap();
            channels.retain(|_, tx| tx.receiver_count() > 0);
            for b in &games {
                if let Some(tx) = channels.get(&b.game_id) {
                    let _ = tx.send(Arc::clone(b));
                }
            }
        }
        if games.is_empty() || self.all.receiver_count() == 0 {
            return;
        }
        let mut all = Vec::with_capacity(games.iter().map(|b| b.bytes.len()).sum());
        for b in &games {
            all.extend_from_slice(&b.bytes);
        }
        let _ = self.all.send(Arc::new(Tick {
            games,
            all: all.into(),
        }));
    }
}

/// A packed reader's place in each stream: the last sequence number sent.
#[derive(Default)]
pub struct Follow {
    sent: HashMap<u64, u64>,
}

impl Follow {
    /// Whether a record follows what was sent of its stream: the next one,
    /// or a keyframe after it (the first of a stream must be a keyframe).
    fn follows(&self, sid: u64, seq: u64, keyframe: bool) -> bool {
        match self.sent.get(&sid) {
            Some(last) => seq > *last && (seq == last + 1 || keyframe),
            None => keyframe,
        }
    }

    /// Whether all of `b` would be taken, as it is.
    fn takes_whole(&self, b: &GameBatch) -> bool {
        b.contiguous && self.follows(b.sid, b.records[0].seq, b.records[0].keyframe)
    }

    /// Appends to `out` the records of `b` that follow (all of them,
    /// usually: one copy).
    pub fn take(&mut self, b: &GameBatch, out: &mut Vec<u8>) {
        if self.takes_whole(b) {
            out.extend_from_slice(&b.bytes);
            self.sent
                .insert(b.sid, b.records.last().expect("not empty").seq);
            return;
        }
        for (i, r) in b.records.iter().enumerate() {
            if self.follows(b.sid, r.seq, r.keyframe) {
                let end = b.records.get(i + 1).map_or(b.bytes.len(), |n| n.at);
                out.extend_from_slice(&b.bytes[r.at..end]);
                self.sent.insert(b.sid, r.seq);
            }
        }
    }

    /// What of one game's batch to send: its own bytes when all of it
    /// follows (no copy), else the records that do. `None`: nothing.
    pub fn take_batch(&mut self, b: &GameBatch) -> Option<Bytes> {
        if self.takes_whole(b) {
            self.sent
                .insert(b.sid, b.records.last().expect("not empty").seq);
            return Some(b.bytes.clone());
        }
        let mut out = Vec::new();
        self.take(b, &mut out);
        (!out.is_empty()).then(|| out.into())
    }

    /// What of a tick to send, given which of its games the reader may
    /// see: the tick's own bytes when that is all of it (no copy), else
    /// the records that follow. `None`: nothing.
    pub fn take_tick(&mut self, tick: &Tick, shown: &[bool]) -> Option<Bytes> {
        if shown.iter().all(|s| *s) && tick.games.iter().all(|b| self.takes_whole(b)) {
            for b in &tick.games {
                self.sent
                    .insert(b.sid, b.records.last().expect("not empty").seq);
            }
            return Some(tick.all.clone());
        }
        let mut out = Vec::new();
        for (b, shown) in tick.games.iter().zip(shown) {
            if *shown {
                self.take(b, &mut out);
            }
        }
        (!out.is_empty()).then(|| out.into())
    }

    /// Appends a frame (one kept since its stream's keyframe) if it
    /// follows.
    pub fn take_frame(&mut self, f: &LiveFrame, out: &mut Vec<u8>) {
        let Some(packed) = &f.packed else {
            return;
        };
        if self.follows(f.sid, f.seq, f.keyframe) {
            live_proto::pack::push_record(out, packed);
            self.sent.insert(f.sid, f.seq);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream's records: `(seq, keyframe)`, each packed as its seq.
    fn batch(game: &str, sid: u64, records: &[(u64, bool)]) -> GameBatch {
        let mut p = Pending::default();
        for (seq, key) in records {
            p.push(*seq, *key, &seq.to_le_bytes());
        }
        p.take(game, sid).unwrap()
    }

    fn seqs(bytes: &[u8]) -> Vec<u64> {
        live_proto::pack::records(bytes)
            .map(|r| u64::from_le_bytes(r.unwrap().try_into().unwrap()))
            .collect()
    }

    fn tick(batches: Vec<GameBatch>) -> Arc<Tick> {
        let hub = Hub::default();
        let mut rx = hub.subscribe_all();
        hub.publish(batches);
        rx.try_recv().unwrap()
    }

    #[test]
    fn a_stream_starts_at_a_keyframe_and_never_repeats() {
        let mut f = Follow::default();
        let mut out = Vec::new();
        // Joined mid-keyframe-interval: deltas before the next keyframe
        // have no base.
        f.take(
            &batch("g", 1, &[(4, false), (5, true), (6, false)]),
            &mut out,
        );
        assert_eq!(seqs(&out), [5, 6]);
        // The next batch repeats 6 (sent at join from the keyframe) and
        // skips 8 (not packable): 7 goes, 9 waits for a keyframe.
        out.clear();
        f.take(
            &batch("g", 1, &[(6, false), (7, false), (9, false)]),
            &mut out,
        );
        assert_eq!(seqs(&out), [7]);
        out.clear();
        f.take(
            &batch("g", 1, &[(10, false), (11, true), (12, false)]),
            &mut out,
        );
        assert_eq!(seqs(&out), [11, 12]);
        // An older keyframe is not sent again.
        out.clear();
        f.take(&batch("g", 1, &[(11, true)]), &mut out);
        assert!(out.is_empty());
    }

    /// A reader that fell behind is sent each stream again from its
    /// keyframe, then the ticks it missed come in (some still queued) and
    /// it takes only what is newer: no frame twice, none older.
    #[test]
    fn a_reader_that_fell_behind_picks_up_from_the_keyframe() {
        let mut f = Follow::default();
        let mut out = Vec::new();
        f.take(
            &batch("g", 1, &[(0, true), (1, false), (2, false)]),
            &mut out,
        );
        // Missed 3..=7; the frames since the keyframe at 5 are 5, 6, 7.
        out.clear();
        for seq in 5..=7 {
            let frame = LiveFrame::packed_for_test(1, seq, seq == 5, seq.to_le_bytes().to_vec());
            f.take_frame(&frame, &mut out);
        }
        assert_eq!(seqs(&out), [5, 6, 7]);
        // A tick still queued from before, and the next.
        out.clear();
        f.take(
            &batch("g", 1, &[(6, false), (7, false), (8, false)]),
            &mut out,
        );
        f.take(&batch("g", 1, &[(9, false)]), &mut out);
        assert_eq!(seqs(&out), [8, 9]);
        // A joiner the same way: from the keyframe, then the tick that
        // repeats it.
        let mut joiner = Follow::default();
        out.clear();
        for seq in 5..=9 {
            let frame = LiveFrame::packed_for_test(1, seq, seq == 5, seq.to_le_bytes().to_vec());
            joiner.take_frame(&frame, &mut out);
        }
        joiner.take(
            &batch("g", 1, &[(8, false), (9, false), (10, false)]),
            &mut out,
        );
        assert_eq!(seqs(&out), [5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn a_tick_goes_out_as_it_is_when_the_reader_takes_all_of_it() {
        let mut f = Follow::default();
        let first = tick(vec![
            batch("a", 1, &[(0, true), (1, false)]),
            batch("b", 2, &[(0, true)]),
        ]);
        let sent = f.take_tick(&first, &[true, true]).unwrap();
        assert_eq!(sent.as_ptr(), first.all.as_ptr(), "the tick's own bytes");
        assert_eq!(seqs(&sent), [0, 1, 0]);
        // A hidden game is left out (a copy of the rest).
        let second = tick(vec![
            batch("a", 1, &[(2, false)]),
            batch("b", 2, &[(1, false)]),
        ]);
        let sent = f.take_tick(&second, &[false, true]).unwrap();
        assert_ne!(sent.as_ptr(), second.all.as_ptr());
        assert_eq!(seqs(&sent), [1]);
        // a's 3 does not follow 1 (2 was hidden): it waits for a keyframe.
        let third = tick(vec![batch("a", 1, &[(3, false), (4, true)])]);
        assert_eq!(seqs(&f.take_tick(&third, &[true]).unwrap()), [4]);
        let none = tick(vec![batch("a", 1, &[(4, true)])]);
        assert_eq!(f.take_tick(&none, &[true]), None);
    }

    #[test]
    fn one_games_readers_get_only_its_batches() {
        let hub = Hub::default();
        let mut a = hub.subscribe_game("a");
        let mut all = hub.subscribe_all();
        hub.publish(vec![
            batch("a", 1, &[(0, true)]),
            batch("b", 2, &[(0, true)]),
        ]);
        hub.publish(vec![batch("b", 2, &[(1, false)])]);
        assert_eq!(a.try_recv().unwrap().game_id, "a");
        assert!(a.try_recv().is_err(), "not woken for b");
        assert_eq!(all.try_recv().unwrap().games.len(), 2);
        assert_eq!(all.try_recv().unwrap().games.len(), 1);
        drop(a);
        hub.publish(vec![batch("a", 1, &[(1, false)])]);
        assert!(hub.games.lock().unwrap().is_empty(), "a's channel is gone");
    }
}
