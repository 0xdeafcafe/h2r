//! The Forge record a built map carries (docs/game/forge-map-variants.md,
//! "The Forge record"): read when the map is activated, into the tables
//! the Forge features play from, so a match on a built variant has its
//! spawners' spare clips and place at start and its starting points'
//! weights with no Forge session.
//!
//! An observer before `GAME_ACTIVATE`, after the features that empty the
//! tables there (`forge_spare_clips`, `forge_spawn_weight`,
//! `forge_place_at_start`), reads the record at the end of the active map's
//! file (`forge_doc::record`) and fills them. It runs before the first
//! round's reset, which is inside `GAME_ACTIVATE`, so place at start holds
//! from the first round. A Forge session's world writes the same tables
//! over it as its document changes.
use std::io::{Read, Seek};
use std::sync::{Arc, Mutex};

use agent::forge::{place_at_start, spare_clips, spawn_weight};
use forge_doc::record::ForgeRecord;

use crate::bots_nav_auto::VfsFile;

/// The feature's id (`features.forge-record`).
pub const FEATURE: &str = "forge-record";

/// Put `record`'s rows in the game's tables.
pub fn apply(record: &ForgeRecord) {
    for (&index, &clips) in &record.spare_clips {
        spare_clips::set(index, Some(clips));
    }
    for (&index, &weight) in &record.weights {
        spawn_weight::set(index, Some(weight));
    }
    for &index in &record.held_back {
        place_at_start::set(index, true);
    }
}

/// The Forge record at the end of `file`: none when it has none, or one
/// that does not read, which is named.
pub fn read<R: Read + Seek>(file: &mut R) -> Option<ForgeRecord> {
    ForgeRecord::read_tail(file).unwrap_or_else(|e| {
        eprintln!("forge record: {e}; the map has none");
        None
    })
}

/// Read the record of the map at guest `path` through the VFS and apply
/// it. A map that is not a file the VFS opens has none.
pub fn activate(vfs: &Arc<Mutex<vfs::Vfs>>, path: Option<&str>) -> Option<ForgeRecord> {
    let mut file = VfsFile::open(Arc::clone(vfs), path?).ok()?;
    let record = read(&mut file)?;
    apply(&record);
    Some(record)
}

/// The feature: reads the active map's record at each activation.
pub struct Record {
    pub vfs: Arc<Mutex<vfs::Vfs>>,
}

impl hooks::Feature for Record {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "A built Forge map's record of its spawners")
            .doc("docs/game/forge-map-variants.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::points::GAME_ACTIVATE;
        let vfs = Arc::clone(&self.vfs);
        b.raw()
            .observe_before(&GAME_ACTIVATE, "forge record: read the map's", move |o| {
                let guest = o.guest();
                let file = crate::map_slots::active_file(&crate::layout_glue::ReadGm(&*guest));
                activate(&vfs, file.as_deref());
            })
            .after_feature(crate::forge_spare_clips::FEATURE)
            .after_feature(crate::forge_spawn_weight::FEATURE)
            .after_feature(crate::forge_place_at_start::FEATURE);
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::io::Cursor;

    use super::*;
    use guest_layout::GuestMemory;
    use guest_layout::testing::FakeGuest;
    use forge_doc::SpawnWeight;
    use guest_layout::global;
    use map::scenario_layout::{BlockKind, block};

    #[test]
    fn a_built_maps_record_reaches_each_feature_with_no_session() {
        // A map whose tail has the record the build writes. The entries
        // are this test's alone in the process's tables.
        let record = ForgeRecord {
            spare_clips: BTreeMap::from([(91, 2)]),
            weights: BTreeMap::from([(3, SpawnWeight::High)]),
            held_back: BTreeSet::from([92]),
            bodies_refused: BTreeMap::new(),
        };
        let mut file = vec![0u8; 32];
        file.extend(record.encode());
        let back = read(&mut Cursor::new(file)).expect("the record");
        assert_eq!(back, record);
        apply(&back);

        // Spare clips: the spawner's hook reads entry 91's clips.
        assert_eq!(spare_clips::get(91), Some(2));

        // Weight: starting location 3 takes High's bias.
        const SCNR: u32 = 0x0070_0000;
        const STARTS: u32 = 0x0071_0000;
        let def = block(BlockKind::StartingLocations);
        let mut g = FakeGuest::new();
        g.put_u32(guest_layout::points::SCENARIO.base, SCNR)
            .map(SCNR + def.offset, 8)
            .put_u32(SCNR + def.offset, 4)
            .put_u32(SCNR + def.offset + 4, STARTS);
        let w = crate::forge_spawn_weight::biased(&g, STARTS + 3 * def.stride, 0.25);
        assert_eq!(w, 0.25 + spawn_weight::BIAS);

        // Place at start: after a round reset entry 92 waits its respawn time.
        const ENGINE: u32 = 0x0080_0000;
        let state = ENGINE + spare_clips::STATE;
        g.put_u32(global::GAME_ENGINE.base, ENGINE)
            .map(state, 100 * spare_clips::STATE_STRIDE);
        for i in 0..100 {
            let e = state + i * spare_clips::STATE_STRIDE;
            g.put_u32(e, u32::MAX).put_u16(e + place_at_start::COUNTDOWN, 1);
        }
        crate::forge_place_at_start::after_reset(&mut g);
        let countdown =
            |g: &FakeGuest, i: u32| g.u16(state + i * spare_clips::STATE_STRIDE + 4).unwrap();
        assert_eq!((countdown(&g, 92), countdown(&g, 91)), (0, 1));

        spare_clips::set(91, None);
        spawn_weight::set(3, None);
        place_at_start::set(92, false);
    }

    #[test]
    fn a_map_with_no_record_or_a_damaged_one_applies_nothing() {
        assert_eq!(read(&mut Cursor::new(vec![0u8; 64])), None);
        let mut bytes = ForgeRecord::default().encode();
        bytes[0] ^= 1;
        assert_eq!(read(&mut Cursor::new(bytes)), None);
    }
}
