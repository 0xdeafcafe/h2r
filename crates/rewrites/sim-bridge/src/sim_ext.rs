//! The guest side of `SimExt` (docs/engine/sim-rewrite.md section 2, the
//! **Extension** kind): the process's extension records move with the
//! guest's checkpoints and are a snapshot section, so film keyframes
//! (machine snapshots) carry them too. `lockstep::hash_sim` folds them into
//! the arena hash.
//!
//! - Save `0x123e20` (observer before): copy the records into the slot the
//!   guest is about to write.
//! - Revert `0x123f60`: the observer before reads which slot the guest
//!   reverts to; the observer after, first among the observers after, puts
//!   that slot's records back.
//! - Full restore `0x124380` (observer after): the save file has no
//!   records beside it, so they are cleared, with a line when there were
//!   any.
use std::sync::{Arc, Mutex};

use ::sim_ext::State;
use guest_layout::{GuestMemory, points};
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};

pub const ID: &str = "sim-ext";

/// The checkpoint slot the guest last wrote, a `u16`: `u16::MAX` before
/// the first save, then 0 and 1 in turn. A save writes the other slot, and
/// a revert reads the one this names. From the checkpoint acceptance test
/// (`crates/platform/xapi/tests/checkpoints.rs`, which reads it after each
/// commit and sets it before each revert).
pub const CHECKPOINT_SLOT: u32 = 0x004e_3b64;

/// The slot index at [`CHECKPOINT_SLOT`], when readable.
pub fn last_slot(mem: &dyn GuestMemory) -> Option<u16> {
    mem.u16(CHECKPOINT_SLOT).ok()
}

/// The slot a save writes after `last`: the other one, and slot 0 first.
pub fn next_slot(last: u16) -> u16 {
    if last == 0 { 1 } else { 0 }
}

/// A checkpoint save is starting: keep the records with the slot it
/// writes. Returns that slot.
pub fn on_save(mem: &dyn GuestMemory, state: &mut State) -> Result<u16, String> {
    let last = last_slot(mem).ok_or("the checkpoint slot index is unreadable")?;
    let slot = next_slot(last);
    state.checkpoint_saved(slot).map_err(|e| e.to_string())?;
    Ok(slot)
}

/// A revert is starting: the slot it reads, for [`on_revert`].
pub fn reverting(mem: &dyn GuestMemory) -> Option<u16> {
    last_slot(mem)
}

/// A revert has finished: put back the records saved with `slot`.
pub fn on_revert(slot: Option<u16>, state: &mut State) -> Result<(), String> {
    let slot = slot.ok_or("the reverted checkpoint slot is unknown")?;
    state.checkpoint_reverted(slot).map_err(|e| e.to_string())
}

/// A full restore has finished: no records were saved with the file.
/// Returns how many were dropped.
pub fn on_full_restore(state: &mut State) -> usize {
    state.full_restore()
}

/// Always registered: it hooks nothing a run without extension records
/// would notice, and its snapshot section is empty then.
pub struct SimExtFeature;

impl Feature for SimExtFeature {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ID,
            "Extension sim state saved with checkpoints, snapshots and films",
        )
        .doc("docs/engine/sim-rewrite.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::section(
            || ::sim_ext::global().save_section(),
            |bytes, _| {
                ::sim_ext::global()
                    .load_section(bytes)
                    .map_err(|e| e.to_string())
            },
        ));
        b.raw()
            .observe_before(&points::GAME_STATE_SAVE, "sim-ext-save", |o| {
                let slot = last_slot(&*o.guest());
                let result = match slot {
                    Some(last) => ::sim_ext::global()
                        .checkpoint_saved(next_slot(last))
                        .map_err(|e| e.to_string()),
                    None => Err("the checkpoint slot index is unreadable".into()),
                };
                if let Err(e) = result {
                    eprintln!("sim-ext: checkpoint save: {e}");
                }
            })
            .optional();
        let pending: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));
        let before = pending.clone();
        b.raw()
            .observe_before(
                &points::GAME_STATE_REVERT,
                "sim-ext-revert-slot",
                move |o| {
                    let slot = reverting(&*o.guest());
                    *before.lock().unwrap_or_else(|e| e.into_inner()) = slot;
                },
            )
            .optional();
        b.raw()
            .observe_after(&points::GAME_STATE_REVERT, "sim-ext-revert", move |_| {
                let slot = pending.lock().unwrap_or_else(|e| e.into_inner()).take();
                if let Err(e) = on_revert(slot, &mut ::sim_ext::global()) {
                    eprintln!("sim-ext: checkpoint revert: {e}");
                }
            })
            .first()
            .optional();
        b.raw()
            .observe_after(&points::GAME_STATE_RESTORE, "sim-ext-restore", |_| {
                let dropped = on_full_restore(&mut ::sim_ext::global());
                if dropped > 0 {
                    eprintln!(
                        "sim-ext: full restore: the save file holds no extension records; {dropped} cleared"
                    );
                }
            })
            .first()
            .optional();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::sim_ext::{FORGE, RULES};
    use guest_layout::FlatMemory;

    fn guest(last: u16) -> FlatMemory {
        let mut mem = FlatMemory::new(CHECKPOINT_SLOT & !0xfff, 0x1000);
        mem.set_u16(CHECKPOINT_SLOT, last).unwrap();
        mem
    }

    /// What the guest's own checkpoint code does to the slot index.
    fn guest_saved(mem: &mut FlatMemory) {
        let last = mem.u16(CHECKPOINT_SLOT).unwrap();
        mem.set_u16(CHECKPOINT_SLOT, next_slot(last)).unwrap();
    }

    #[test]
    fn saves_alternate_slots_from_zero() {
        assert_eq!(next_slot(u16::MAX), 0);
        assert_eq!(next_slot(0), 1);
        assert_eq!(next_slot(1), 0);
    }

    #[test]
    fn a_record_survives_checkpoint_save_and_revert_through_the_hooks() {
        let mut mem = guest(u16::MAX);
        let mut s = State::new();

        s.ext.set(&RULES, b"at first save".to_vec()).unwrap();
        s.ext.set(&FORGE, vec![1]).unwrap();
        assert_eq!(on_save(&mem, &mut s), Ok(0));
        guest_saved(&mut mem);
        let first = s.ext.digest();

        s.ext.set(&RULES, b"at second save".to_vec()).unwrap();
        assert_eq!(on_save(&mem, &mut s), Ok(1));
        guest_saved(&mut mem);
        let second = s.ext.digest();
        assert_ne!(first, second, "the record changes the hash");

        s.ext.set(&RULES, b"live".to_vec()).unwrap();
        s.ext.remove(&FORGE);

        // Revert to the latest checkpoint: the slot index names it.
        let slot = reverting(&mem);
        assert_eq!(slot, Some(1));
        on_revert(slot, &mut s).unwrap();
        assert_eq!(s.ext.get(&RULES), Some(&b"at second save"[..]));
        assert_eq!(s.ext.get(&FORGE), Some(&[1][..]));
        assert_eq!(s.ext.digest(), second);

        // Revert to the older slot, as the guest does when told to.
        mem.set_u16(CHECKPOINT_SLOT, 0).unwrap();
        on_revert(reverting(&mem), &mut s).unwrap();
        assert_eq!(s.ext.get(&RULES), Some(&b"at first save"[..]));
        assert_eq!(s.ext.digest(), first);

        // A snapshot taken now, loaded into a fresh process, reverts the same.
        let section = s.save_section();
        let mut loaded = State::new();
        loaded.load_section(&section).unwrap();
        assert_eq!(loaded, s);
        mem.set_u16(CHECKPOINT_SLOT, 1).unwrap();
        on_revert(reverting(&mem), &mut loaded).unwrap();
        assert_eq!(loaded.ext.get(&RULES), Some(&b"at second save"[..]));

        assert_eq!(on_full_restore(&mut loaded), 2);
        assert!(loaded.ext.is_empty());
    }

    #[test]
    fn an_unreadable_slot_index_changes_nothing() {
        let elsewhere = FlatMemory::new(0x1000, 0x10);
        let mut s = State::new();
        s.ext.set(&RULES, vec![5]).unwrap();
        let before = s.clone();
        assert!(on_save(&elsewhere, &mut s).is_err());
        assert!(on_revert(reverting(&elsewhere), &mut s).is_err());
        assert_eq!(s, before);
    }

    #[test]
    fn the_feature_places_its_hooks() {
        let mut r = hooks::Registry::new(hooks::Switches::parse("").unwrap());
        assert!(r.add(&SimExtFeature).unwrap());
        let text = r.build(&hooks::plan::Base).unwrap().text();
        for label in [
            "sim-ext-save",
            "sim-ext-revert-slot",
            "sim-ext-revert",
            "sim-ext-restore",
        ] {
            assert!(text.contains(label), "{label}\n{text}");
        }
    }
}
