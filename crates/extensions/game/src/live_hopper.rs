//! The retail playlist block reader, shared by validation and the weighted
//! match picker (docs/live/live-matchmaking.md §6.5).

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use guest_layout::points::HOPPER_BLOCK_READ;
use hooks::{BuildError, Builder, Feature, Flow, Manifest};
use live_proto::hopper::{BLOCK, is_base_map, retain_block_maps};

/// Return from the picker's block read, before it sums the entry weights.
const PICKER_READ_RETURN: u32 = 0x0019_2ed1;

pub struct MapPackRotation(pub Arc<live_hle::Hle>);

/// The block each guest thread inside the read will filter, by thread.
struct Read {
    hopper: u32,
    block: u32,
    base_only: bool,
    caller: u32,
}

type Pending = Mutex<BTreeMap<u32, Read>>;

/// Last block failure per descriptor (and one slot for invalid indices).
/// Kept for the agent after a dialog has replaced the playlist screen.
#[derive(Clone, serde::Serialize)]
pub(crate) struct Failure {
    hopper: u32,
    caller: u32,
    message: String,
}

static FAILURES: Mutex<BTreeMap<u32, Failure>> = Mutex::new(BTreeMap::new());

pub(crate) fn failures() -> Vec<Failure> {
    FAILURES.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect()
}

fn failed(hopper: u32, caller: u32, message: String) {
    eprintln!("live: playlist {hopper} ({caller:#x}): {message}");
    let slot = hopper.min(live_proto::hopper::HOPPERS as u32);
    FAILURES.lock().unwrap_or_else(|e| e.into_inner())
        .insert(slot, Failure { hopper, caller, message });
}

/// Why a snapshot can't be taken: a thread is between the read's before
/// and after, so the block it will filter is held here. The machine's own
/// check refuses the same moment (the read's after-hook continuation);
/// this names the feature.
fn read_in_flight(pending: &Pending) -> Option<String> {
    let p = pending.lock().unwrap_or_else(|e| e.into_inner());
    let threads: Vec<String> = p.keys().map(u32::to_string).collect();
    (!threads.is_empty()).then(|| {
        format!(
            "a hopper block read is in flight on thread {}",
            threads.join(", ")
        )
    })
}

impl Feature for MapPackRotation {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "live-map-pack-rotation",
            "Gate the matchmaking rotation by the installed maps and the assigned lobby",
        )
        .doc("docs/live/live-matchmaking.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // The reader does not retain its output pointer in a return register.
        // Keep it per guest thread across the native call. The reader's own
        // calls are file I/O and cannot reenter it.
        let pending: Arc<Pending> = Arc::new(Mutex::new(BTreeMap::new()));
        let writes = pending.clone();
        // That map is its only state, and it holds an entry only while a
        // thread is inside the read. The add-on answers are Live's, which
        // the Live HLE's own snapshot check covers.
        let reading = pending.clone();
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            read_in_flight(&reading)
        }));
        let hle = self.0.clone();
        b.raw().before(&HOPPER_BLOCK_READ, "check the caller's map rotation", move |c| {
            let hopper = c.cpu.eax;
            let (caller, block) = {
                let mem = c.ctx.memory();
                (mem.u32(c.cpu.esp)?, mem.u32(c.cpu.esp + 4)?)
            };
            let filter = if caller == PICKER_READ_RETURN {
                !hle.allow_hopper_addons(hopper)
            } else {
                !hle.has_map_pack()
            };
            let mut writes = writes.lock().unwrap_or_else(|e| e.into_inner());
            writes.insert(c.ctx.thread_id, Read { hopper, block, base_only: filter, caller });
            Ok(Flow::Continue)
        });
        b.raw().after(&HOPPER_BLOCK_READ, "keep only the base-disc entries when needed", move |c| {
            let read = pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&c.ctx.thread_id);
            let Some(Read { hopper, block, base_only, caller }) = read else {
                return Ok(());
            };
            if c.cpu.eax & 0xff == 0 {
                failed(hopper, caller, "Could not read the playlist block from multiplayer_hopper.dat.".into());
                return Ok(());
            }
            let mut mem = c.ctx.memory();
            let listed = crate::mod_slots::listed_ids(&*mem);
            if !base_only && listed.is_none() {
                return Ok(());
            }
            let mut bytes = vec![0; BLOCK];
            mem.read_bytes(block, &mut bytes, 1)?;
            let mut eligible = false;
            let mut unavailable = Vec::new();
            let changed = retain_block_maps(&mut bytes, |id| {
                let keep = (!base_only || is_base_map(id))
                    && listed.as_ref().is_none_or(|ids| ids.contains(&id));
                if !keep && !unavailable.contains(&id) {
                    unavailable.push(id);
                }
                eligible |= keep;
                keep
            });
            match changed {
                Ok(_) if !eligible => {
                    failed(hopper, caller, format!("No playable maps in the rotation. Unavailable map IDs: {unavailable:?}."));
                    c.cpu.eax &= !0xff;
                }
                Ok(true) => mem.write_bytes(block, &bytes, 1)?,
                Ok(false) => {}
                Err(e) => {
                    failed(hopper, caller, format!("Invalid rotation in multiplayer_hopper.dat: {e}"));
                    c.cpu.eax &= !0xff;
                }
            }
            Ok(())
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_waits_only_for_a_read_in_flight() {
        let pending: Pending = Mutex::new(BTreeMap::new());
        assert_eq!(read_in_flight(&pending), None);
        pending.lock().unwrap().insert(2, Read { hopper: 0, block: 0x1234, base_only: true, caller: 0 });
        assert_eq!(
            read_in_flight(&pending).as_deref(),
            Some("a hopper block read is in flight on thread 2")
        );
        pending.lock().unwrap().remove(&2);
        assert_eq!(read_in_flight(&pending), None);
    }

    /// Built, it is audited, and idle it blocks nothing.
    #[test]
    fn built_it_blocks_no_snapshot_while_idle() {
        let hle = live_hle::Hle::new(live_hle::LiveConfig {
            server_url: Some("https://live.example".into()),
            ..Default::default()
        })
        .unwrap();
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        assert!(r.add(&MapPackRotation(hle)).unwrap());
        let plan = r.build(&hooks::Base).unwrap();
        let policies: Vec<_> = plan
            .snapshot
            .iter()
            .map(|f| (f.id, f.policy.name()))
            .collect();
        assert_eq!(policies, vec![("live-map-pack-rotation", "forbid while live")]);
        assert!(plan.snapshot_blockers().is_empty());
    }
}
