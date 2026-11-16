//! Drop the game's cached copy of a map whose package changed while the
//! game runs (docs/runtime/cache-slots.md, "Changed maps").
//!
//! The game copies each map from the DVD into a cache slot and later reuses
//! a slot whose resident header names the same `d:\maps\<name>.map` path,
//! without opening the DVD file or comparing anything else (`0x214a60`
//! compares the path only). So when a package installed mid-session (a
//! Live join, the editor) changes what `maps\<name>.map` serves, a slot
//! still holding the old copy would be served stale. This feature watches
//! once a frame (after `RENDER_FRAME`, on the main thread, where the copy
//! machine `0x2141f0` also runs), at most twice a second: for each
//! multiplayer or campaign slot (3, 4, 5) that is not the current map's,
//! while the copy machine is idle, it asks the VFS whether the map that
//! slot holds would now be served from another package than when it was
//! copied ([`vfs::Vfs::map_changed`]). If so it zeroes the slot's resident
//! header, as the game itself does before reusing a slot (`0x214485`): the
//! next request for that map misses and copies it afresh.
//!
//! Slots 0-2 (shared, single-player shared, main menu) are left alone:
//! other maps' copies check their headers as dependencies, so dropping one
//! mid-session could fail unrelated loads.
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::layout_glue::Gm;

/// The slot records: a file handle at `+0`, then the slot's 0x800-byte
/// resident header copy (docs/runtime/cache-slots.md).
pub const RECORDS: u32 = 0x0055_7c90;
pub const STRIDE: u32 = 0x804;
/// The resident header's offset in a record.
pub const HEADER: u32 = 4;
pub const HEADER_LEN: usize = 0x800;
/// The slot's path, in its header (`+0x20`, 0x100 bytes).
pub const PATH: u32 = 0x20;
pub const PATH_LEN: usize = 0x100;
/// The copy machine's state: 0 when idle.
pub const STATE: u32 = 0x0055_bd04;
/// The slot of the map in use.
pub const CURRENT: u32 = 0x0055_aca8;
/// The slots this may drop: multiplayer (3) and campaign (4, 5).
pub const SLOTS: [u32; 3] = [3, 4, 5];
/// How often the folder is looked at.
pub const EVERY: Duration = Duration::from_millis(500);

fn path_at<M: Gm>(m: &M, slot: u32) -> Option<String> {
    let mut b = [0u8; PATH_LEN];
    if !m.rd(RECORDS + slot * STRIDE + HEADER + PATH, &mut b) {
        return None;
    }
    let end = b.iter().position(|&c| c == 0)?;
    std::str::from_utf8(&b[..end]).ok().map(str::to_owned)
}

/// The map name in a slot path `d:\maps\<name>.map`, if it is one.
pub fn map_name(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase();
    let name = lower.strip_prefix("d:\\maps\\")?.strip_suffix(".map")?;
    (!name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then(|| name.to_owned())
}

/// Whether the copy machine is idle, and the current slot.
fn idle<M: Gm>(m: &M) -> Option<u32> {
    (m.u32(STATE)? == 0).then(|| m.u32(CURRENT)).flatten()
}

/// The slots that may be dropped now, with the map each holds: none while
/// a copy is in flight.
pub fn candidates<M: Gm>(m: &M) -> Vec<(u32, String)> {
    let Some(current) = idle(m) else {
        return Vec::new();
    };
    SLOTS
        .iter()
        .filter(|&&s| s != current)
        .filter_map(|&s| Some((s, map_name(&path_at(m, s)?)?)))
        .collect()
}

/// Zero `slot`'s resident header if it still holds `name`, the copy
/// machine is idle and it is not the current map's. True when dropped.
pub fn drop_slot<M: Gm>(m: &mut M, slot: u32, name: &str) -> bool {
    if !SLOTS.contains(&slot) || idle(m).is_none_or(|current| current == slot) {
        return false;
    }
    if path_at(m, slot).and_then(|p| map_name(&p)).as_deref() != Some(name) {
        return false;
    }
    m.wr(RECORDS + slot * STRIDE + HEADER, &[0; HEADER_LEN])
}

pub const FEATURE: &str = "map-slots";

/// The feature: after each `RENDER_FRAME`, at most every [`EVERY`], drop
/// the [`candidates`] whose map the VFS says changed ([`drop_slot`]).
pub struct MapSlots {
    pub vfs: Arc<Mutex<vfs::Vfs>>,
}

impl feature::Feature for MapSlots {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(FEATURE, "Recopy maps whose package changed mid-session")
            .doc("docs/runtime/cache-slots.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        let vfs = Arc::clone(&self.vfs);
        let last: Mutex<Option<Instant>> = Mutex::new(None);
        b.raw().observe_after(
            &feature::points::RENDER_FRAME,
            "drop outdated map slots",
            move |o| {
                {
                    let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
                    if last.is_some_and(|t| t.elapsed() < EVERY) {
                        return;
                    }
                    *last = Some(Instant::now());
                }
                // Guest memory and the VFS are never held together: file
                // I/O takes the VFS, then memory.
                let rt = Arc::clone(o.runtime);
                let candidates = candidates(&*rt.memory());
                if candidates.is_empty() {
                    return;
                }
                let stale: Vec<(u32, String)> = {
                    let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
                    candidates
                        .into_iter()
                        .filter(|(_, n)| {
                            v.map_changed(&format!("{}\\maps\\{n}.map", vfs::CDROM_DEVICE))
                        })
                        .collect()
                };
                if stale.is_empty() {
                    return;
                }
                let mut mem = rt.memory();
                for (slot, name) in stale {
                    if drop_slot(&mut *mem, slot, &name) {
                        eprintln!(
                            "map slots: maps\\{name}.map changed since it was copied; slot {slot} dropped, the next load copies it again"
                        );
                    }
                }
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake(BTreeMap<u32, u8>);
    impl Gm for Fake {
        fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
            let m = &self.0;
            for (i, b) in out.iter_mut().enumerate() {
                match m.get(&(addr + i as u32)) {
                    Some(v) => *b = *v,
                    None => return false,
                }
            }
            true
        }
        fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
            let m = &mut self.0;
            for (i, b) in data.iter().enumerate() {
                m.insert(addr + i as u32, *b);
            }
            true
        }
    }

    /// Six slot records, idle, `current` in use; slot `s` holds `paths[s]`.
    fn world(current: u32, paths: [&str; 6]) -> Fake {
        let mut m = Fake::default();
        for (s, p) in paths.iter().enumerate() {
            let mut header = vec![0x5a; HEADER_LEN];
            header[PATH as usize..][..PATH_LEN].fill(0);
            header[PATH as usize..][..p.len()].copy_from_slice(p.as_bytes());
            m.wr(RECORDS + s as u32 * STRIDE, &[1, 0, 0, 0]);
            m.wr(RECORDS + s as u32 * STRIDE + HEADER, &header);
        }
        m.set_u32(STATE, 0);
        m.set_u32(CURRENT, current);
        m
    }
    const PATHS: [&str; 6] = [
        "d:\\maps\\shared.map",
        "d:\\maps\\single_player_shared.map",
        "d:\\maps\\mainmenu.map",
        "d:\\maps\\lockout.map",
        "d:\\maps\\01a_tutorial.map",
        "",
    ];
    fn header(m: &Fake, s: u32) -> Vec<u8> {
        let mut b = vec![0; HEADER_LEN];
        assert!(m.rd(RECORDS + s * STRIDE + HEADER, &mut b));
        b
    }

    #[test]
    fn slot_paths_name_maps() {
        assert_eq!(
            map_name("d:\\maps\\lockout.map").as_deref(),
            Some("lockout")
        );
        assert_eq!(
            map_name("D:\\MAPS\\Lockout.MAP").as_deref(),
            Some("lockout")
        );
        for not in [
            "",
            "t:\\$c\\x\\lockout.map",
            "d:\\maps\\.map",
            "d:\\maps\\a\\b.map",
        ] {
            assert_eq!(map_name(not), None, "{not}");
        }
    }

    /// What the feature does once: candidates, the VFS's word, drops.
    fn sweep(m: &mut Fake, changed: impl Fn(&str) -> bool) -> Vec<(u32, String)> {
        let stale: Vec<_> = candidates(m)
            .into_iter()
            .filter(|(_, n)| changed(n))
            .collect();
        stale
            .into_iter()
            .filter(|(s, n)| drop_slot(m, *s, n))
            .collect()
    }

    #[test]
    fn a_changed_map_in_an_idle_unused_slot_is_dropped() {
        let mut m = world(2, PATHS);
        // Only slots 3-5 are candidates, and an empty slot is not.
        assert_eq!(
            candidates(&m),
            vec![(3, "lockout".to_owned()), (4, "01a_tutorial".to_owned())]
        );
        let dropped = sweep(&mut m, |n| n == "lockout");
        assert_eq!(dropped, vec![(3, "lockout".to_owned())]);
        assert_eq!(header(&m, 3), vec![0; HEADER_LEN], "header zeroed");
        assert_eq!(m.u32(RECORDS + 3 * STRIDE), Some(1), "handle kept");
        assert_ne!(header(&m, 4), vec![0; HEADER_LEN], "unchanged map kept");
        assert_eq!(candidates(&m), vec![(4, "01a_tutorial".to_owned())]);
    }

    #[test]
    fn the_current_map_a_copy_in_flight_and_the_shared_slots_are_never_dropped() {
        // Lockout in use: kept until the game moves on.
        let mut m = world(3, PATHS);
        assert!(sweep(&mut m, |_| true).iter().all(|(s, _)| *s != 3));
        assert_ne!(header(&m, 3), vec![0; HEADER_LEN]);
        // A copy in flight: nothing at all.
        let mut m = world(2, PATHS);
        m.set_u32(STATE, 7);
        assert!(sweep(&mut m, |_| true).is_empty());
        assert!(!drop_slot(&mut m, 3, "lockout"));
        // Shared, single-player shared and the menu: never.
        let mut m = world(5, PATHS);
        for s in 0..3 {
            let name = map_name(PATHS[s as usize]).unwrap();
            assert!(!drop_slot(&mut m, s, &name));
        }
        // The slot was refilled with another map since the look.
        let mut m = world(2, PATHS);
        assert!(!drop_slot(&mut m, 3, "midship"));
        // Unreadable memory: nothing.
        assert!(candidates(&Fake::default()).is_empty());
    }
}
