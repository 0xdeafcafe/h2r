//! Machine snapshots (docs/runtime/snapshots.md, phase 4): the `dsound`
//! section, and the mixer's `audio` section with it.
//!
//! [`Hle::save_snapshot`] saves the [`DirectSound`] objects (decisions in
//! `state/snap.rs`), the arena's bookkeeping (its bytes are guest RAM),
//! the event pins by token and object address, and the stream callbacks a
//! host render left for the next override call or the completion DPC
//! (plain data: function, contexts, status). It refuses while event
//! operations are published and not yet delivered (they are delivered
//! before every call returns, so only a call in flight on another thread
//! leaves any) or while a pin cannot name its object.
//!
//! [`Hle::load_snapshot`] decodes and checks both sections and re-makes
//! the pins before it changes anything; a mismatch changes nothing. It
//! must come after the kernel's `kcore` is loaded: the loaded kernel
//! already counts the saved pins, so they are adopted without a new count,
//! and the loading run's own pins are forgotten without a release.
use super::*;
use crate::state::DirectSound;
use snapshot::codec::{Reader, Writer};
use snapshot::{Codec, Snapshot, SnapshotWriter, Tag};

/// The section's tag.
pub const SECTION: Tag = Tag::new("dsound");
const VERSION: u32 = 1;

/// Why a save or load did not happen.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// Something is in flight that a snapshot cannot carry.
    #[error("dsound is not quiescent: {}", .0.join("; "))]
    Busy(Vec<String>),
    /// The section is damaged or missing.
    #[error(transparent)]
    Codec(#[from] snapshot::Error),
    /// The mixer's section did not fit.
    #[error(transparent)]
    Audio(#[from] audio::snap::SnapshotError),
    /// The snapshot was saved by a DirectSound made differently, or its
    /// pins cannot be made again here.
    #[error("dsound snapshot does not fit: {0}")]
    Mismatch(String),
}

fn corrupt(what: &str) -> snapshot::Error {
    snapshot::Error::Corrupt("dsound", what.to_owned())
}

fn put_pairs(w: &mut Writer, v: &[(u32, u32)]) {
    w.list(v, |w, &(a, b)| {
        w.u32(a).u32(b);
    });
}

fn get_pairs(r: &mut Reader<'_>) -> Result<Vec<(u32, u32)>, snapshot::Error> {
    r.list(8, |r| Ok((r.u32()?, r.u32()?)))
}

fn put_arena(w: &mut Writer, a: &Arena) {
    let Arena {
        mapped,
        cursor,
        end,
        live,
        free,
    } = a;
    put_pairs(w, mapped);
    w.u32(*cursor).u32(*end);
    let mut live: Vec<(u32, (u32, u32))> = live.iter().map(|(k, v)| (*k, *v)).collect();
    live.sort_unstable();
    w.list(&live, |w, &(p, (base, len))| {
        w.u32(p).u32(base).u32(len);
    });
    // In order: allocation reuses the first free block that fits.
    put_pairs(w, free);
}

fn get_arena(r: &mut Reader<'_>) -> Result<Arena, snapshot::Error> {
    let mapped = get_pairs(r)?;
    let cursor = r.u32()?;
    let end = r.u32()?;
    let live = r.list(12, |r| Ok((r.u32()?, (r.u32()?, r.u32()?))))?;
    let free = get_pairs(r)?;
    if cursor > end {
        return Err(corrupt("arena cursor past its chunk"));
    }
    Ok(Arena {
        mapped,
        cursor,
        end,
        live: live.into_iter().collect(),
        free,
    })
}

fn put_callback(w: &mut Writer, cb: &StreamCallback) {
    let StreamCallback {
        function,
        stream_context,
        packet_context,
        status,
    } = cb;
    for v in [function, stream_context, packet_context, status] {
        w.u32(*v);
    }
}

fn get_callback(r: &mut Reader<'_>) -> Result<StreamCallback, snapshot::Error> {
    Ok(StreamCallback {
        function: r.u32()?,
        stream_context: r.u32()?,
        packet_context: r.u32()?,
        status: r.u32()?,
    })
}

impl Hle {
    /// What refuses a save now. Empty when [`Hle::save_snapshot`] may run.
    pub fn snapshot_blockers(&self) -> Vec<String> {
        let mut why = Vec::new();
        let batches = lock(&self.outbox).len();
        if batches > 0 {
            why.push(format!("{batches} event batches not yet delivered"));
        }
        let unnamed = lock(&self.pins)
            .values()
            .filter(|p| p.address().is_none())
            .count();
        if unnamed > 0 {
            why.push(format!("{unnamed} event pins cannot name their object"));
        }
        why
    }

    /// Adds the `dsound` and `audio` sections. Call between slices with no
    /// guest memory guard held and nothing rendering the mixer (logical
    /// pacing between `advance_to` calls, or the output halted).
    pub fn save_snapshot(&self, out: &mut SnapshotWriter) -> Result<(), SnapshotError> {
        let busy = self.snapshot_blockers();
        if !busy.is_empty() {
            return Err(SnapshotError::Busy(busy));
        }
        let Hle {
            ds,
            arena,
            // Keep: the loading run's kernel bridge.
            events: _,
            callbacks,
            pins,
            // Quiesce (snapshot_blockers).
            outbox: _,
            delivering: _,
            // Diagnostics.
            stops: _,
            signal_failures: _,
        } = self;
        let ds = lock(ds);
        let mut w = Writer::new();
        ds.put_snapshot(&mut w);
        put_arena(&mut w, &lock(arena));
        let mut pinned: Vec<(u64, u32)> = Vec::new();
        for (token, pin) in lock(pins).iter() {
            let Some(address) = pin.address() else {
                return Err(SnapshotError::Busy(vec![format!(
                    "event pin {token} cannot name its object"
                )]));
            };
            pinned.push((*token, address));
        }
        pinned.sort_unstable();
        w.list(&pinned, |w, &(token, address)| {
            w.u64(token).u32(address);
        });
        let queued: Vec<StreamCallback> = lock(callbacks).iter().copied().collect();
        w.list(&queued, put_callback);
        lock(&**ds.mixer()).save_output_snapshot(out, ds.clock_only())?;
        out.section(SECTION, VERSION, Codec::Zstd, w.into_bytes())?;
        Ok(())
    }

    /// Loads the `dsound` and `audio` sections. Guest RAM and the kernel
    /// (`kcore`) must already be loaded. Everything is decoded and checked,
    /// and the pins made again, before anything changes.
    pub fn load_snapshot(&self, snapshot: &Snapshot<'_>) -> Result<(), SnapshotError> {
        let batches = lock(&self.outbox).len();
        if batches > 0 {
            return Err(SnapshotError::Busy(vec![format!(
                "{batches} event batches not yet delivered"
            )]));
        }
        let bytes = snapshot.require(SECTION, VERSION..=VERSION)?;
        let mut r = Reader::new(&bytes, "dsound");
        let saved = DirectSound::get_snapshot(&mut r)?;
        let arena = get_arena(&mut r)?;
        let pinned = r.list(12, |r| Ok((r.u64()?, r.u32()?)))?;
        let queued = r.list(16, get_callback)?;
        r.finish()?;

        let mut ds = lock(&self.ds);
        ds.check_snapshot(&saved).map_err(SnapshotError::Mismatch)?;
        let mixer = Arc::clone(ds.mixer());
        let mut mixer = lock(&*mixer);
        let loaded = mixer.check_output_snapshot(snapshot, ds.clock_only())?;
        let mut adopted: HashMap<u64, Box<dyn EventRef>> = HashMap::new();
        {
            let mut events = lock(&self.events);
            for &(token, address) in &pinned {
                let why = match events.adopt(address) {
                    Some(Ok(pin)) => {
                        adopted.insert(token, pin);
                        continue;
                    }
                    Some(Err(e)) => e,
                    None => "the event sink cannot make pins again".to_owned(),
                };
                for (_, pin) in adopted {
                    pin.forget();
                }
                return Err(SnapshotError::Mismatch(format!(
                    "event pin {token} at {address:#x}: {why}"
                )));
            }
        }

        mixer.apply_snapshot(loaded);
        drop(mixer);
        ds.apply_snapshot(saved);
        *lock(&self.arena) = arena;
        let old = std::mem::replace(&mut *lock(&self.pins), adopted);
        for (_, pin) in old {
            pin.forget();
        }
        *lock(&self.callbacks) = queued.into();
        Ok(())
    }
}
