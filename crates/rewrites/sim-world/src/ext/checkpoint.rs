//! The extension state beside the arena's checkpoints, and in snapshots.
//!
//! The guest keeps two checkpoint slots (docs/runtime/checkpoints.md).
//! [`State`] keeps one copy of the records per slot: a save at
//! `0x123e20` copies the live records into the slot the guest is writing,
//! and a revert at `0x123f60` puts that slot's copy back. A full restore
//! at `0x124380` reads a save file this side has no copy of, so it clears
//! the records. The hooks are `sim_bridge::sim_ext`; this module knows no
//! guest addresses.
//!
//! The snapshot section ([`State::save_section`]) holds the live records
//! and both slot copies. Film keyframes are machine snapshots, so they
//! carry the same section.
use super::{Error, Reader, SimExt};
use std::sync::{LazyLock, Mutex, MutexGuard};

/// The guest's checkpoint slots.
pub const SLOTS: usize = 2;

/// The snapshot section's layout version.
const SECTION: u8 = 1;

/// The live records and one copy per checkpoint slot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// The live records.
    pub ext: SimExt,
    /// What [`SimExt::save`] gave at each slot's last save; `None` for a
    /// slot no save was seen for.
    slots: [Option<Vec<u8>>; SLOTS],
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    /// The bytes saved with `slot`, if a save was seen for it.
    pub fn slot(&self, slot: u16) -> Option<&[u8]> {
        self.slots.get(usize::from(slot))?.as_deref()
    }

    /// The guest saved a checkpoint into `slot`: keep the live records
    /// with it.
    pub fn checkpoint_saved(&mut self, slot: u16) -> Result<(), Error> {
        let bytes = self.ext.save();
        let saved = self
            .slots
            .get_mut(usize::from(slot))
            .ok_or(Error::Slot(slot))?;
        *saved = Some(bytes);
        Ok(())
    }

    /// The guest reverted to `slot`: put back the records saved with it.
    /// A slot no save was seen for (saved by another process, before a
    /// full restore) clears the records, and says so when there were any;
    /// a slot out of range changes nothing.
    pub fn checkpoint_reverted(&mut self, slot: u16) -> Result<(), Error> {
        let saved = self.slots.get(usize::from(slot)).ok_or(Error::Slot(slot))?;
        match saved {
            Some(bytes) => self.ext.restore(bytes),
            None => {
                if self.ext.clear() > 0 {
                    Err(Error::NoSlot(slot))
                } else {
                    Ok(())
                }
            }
        }
    }

    /// The guest restored a whole game from a save file: no copy of its
    /// records exists, so drop them. Returns how many were dropped.
    pub fn full_restore(&mut self) -> usize {
        self.ext.clear()
    }

    /// The snapshot section: zero bytes when there are no records and no
    /// slot copies, otherwise a version byte, then the live records and
    /// each slot as `u8 present | u32 length | bytes`.
    pub fn save_section(&self) -> Vec<u8> {
        if self.ext.is_empty() && self.slots.iter().all(Option::is_none) {
            return Vec::new();
        }
        let mut out = vec![SECTION];
        put(&mut out, Some(self.ext.save().as_slice()));
        for slot in &self.slots {
            put(&mut out, slot.as_deref());
        }
        out
    }

    /// Load [`State::save_section`]'s bytes. Every part is checked against
    /// the registered kinds first; a refusal changes nothing.
    pub fn load_section(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.is_empty() {
            self.ext.clear();
            self.slots = Default::default();
            return Ok(());
        }
        let mut r = Reader::new(bytes, 0);
        let version = r.u8()?;
        if version != SECTION {
            return Err(Error::Section(format!(
                "version {version}; this build reads {SECTION}"
            )));
        }
        let live = take(&mut r)?.ok_or_else(|| Error::Section("no live records".into()))?;
        self.ext.check(live)?;
        let mut slots: [Option<Vec<u8>>; SLOTS] = Default::default();
        for slot in &mut slots {
            if let Some(b) = take(&mut r)? {
                self.ext.check(b)?;
                *slot = Some(b.to_vec());
            }
        }
        if !r.done() {
            return Err(Error::Trailing);
        }
        self.ext.restore(live)?;
        self.slots = slots;
        Ok(())
    }
}

fn put(out: &mut Vec<u8>, part: Option<&[u8]>) {
    match part {
        None => out.push(0),
        Some(b) => {
            out.push(1);
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(b);
        }
    }
}

fn take<'a>(r: &mut Reader<'a>) -> Result<Option<&'a [u8]>, Error> {
    match r.u8()? {
        0 => Ok(None),
        1 => {
            let len = r.u32()? as usize;
            r.take(len).map(Some)
        }
        flag => Err(Error::Section(format!("part flag {flag}"))),
    }
}

static GLOBAL: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::new()));

/// This process's extension state, the one the hooks, the snapshot
/// section and the sim hash use. Never hold it across a guest call or a
/// call to [`digest`].
pub fn global() -> MutexGuard<'static, State> {
    GLOBAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// The live records' digest, `None` with none: what `lockstep::hash_sim`
/// folds into the arena hash.
pub fn digest() -> Option<u64> {
    global().ext.digest()
}
