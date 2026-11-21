//! The retail playlist block reader, shared by validation and the weighted
//! match picker (docs/live/live-matchmaking.md §6.5).

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use engine::points::HOPPER_BLOCK_READ;
use hooks::{BuildError, Builder, Feature, Flow, Manifest};
use live_proto::hopper::{BLOCK, is_base_map, retain_block_maps};

/// Return from the picker's block read, before it sums the entry weights.
const PICKER_READ_RETURN: u32 = 0x0019_2ed1;

pub struct MapPackRotation(pub Arc<live_hle::Hle>);

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
        let pending = Arc::new(Mutex::new(BTreeMap::new()));
        let writes = pending.clone();
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
            if filter {
                writes.insert(c.ctx.thread_id, block);
            } else {
                writes.remove(&c.ctx.thread_id);
            }
            Ok(Flow::Continue)
        });
        b.raw().after(&HOPPER_BLOCK_READ, "keep only the base-disc entries when needed", move |c| {
            let block = pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&c.ctx.thread_id);
            let Some(block) = block.filter(|_| c.cpu.eax & 0xff != 0) else {
                return Ok(());
            };
            let mut bytes = vec![0; BLOCK];
            let mut mem = c.ctx.memory();
            mem.read_bytes(block, &mut bytes, 1)?;
            let mut have_base_map = false;
            let changed = retain_block_maps(&mut bytes, |id| {
                let base = is_base_map(id);
                have_base_map |= base;
                base
            });
            match changed {
                Ok(_) if !have_base_map => c.cpu.eax &= !0xff,
                Ok(true) => mem.write_bytes(block, &bytes, 1)?,
                Ok(false) => {}
                Err(e) => {
                    eprintln!("live: invalid hopper rotation: {e}");
                    c.cpu.eax &= !0xff;
                }
            }
            Ok(())
        });
        Ok(())
    }
}
