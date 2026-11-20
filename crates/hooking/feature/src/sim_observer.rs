//! Trim the inert tail of the observer's cluster resource predictions.
//! The cache requests, camera calculation and visibility walk stay in guest code.
//! See docs/perf/sim-budget.md for the caller and resource-mode trace.

use engine::memory::GuestMemory;
use engine::points;

use crate::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};

const CAMERA_CLUSTER_RETURN: u32 = 0x0003_f54e;
const ENTRY_BYTES: usize = 8;
// Bound the look-back even for a custom BSP. Leaving more inert entries
// in the guest loop is harmless. Both retail measurement maps fit.
const TAIL_ENTRIES: usize = 64;

pub struct ObserverResources;

impl Feature for ObserverResources {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "sim-observer-resources",
            "Trim inactive observer resource predictions",
        )
        .doc("docs/perf/sim-budget.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        if b.switches().implementation("sim.observer-resources", true) {
            b.raw()
                .before(
                    &points::PREDICT_RESOURCES,
                    "trim the current cluster's inactive prediction tail",
                    |c| {
                        let _ = trim_tail(&mut *c.ctx.memory(), c.cpu.esp, c.cpu.edi);
                        Ok(Flow::Continue)
                    },
                )
                .last()
                .optional()
                .context_only();
        }
        Ok(())
    }
}

fn trim_tail(mem: &mut dyn GuestMemory, sp: u32, mode: u32) -> Option<()> {
    if mode as u16 != 1 || mem.u32(sp).ok()? != CAMERA_CLUSTER_RETURN {
        return None;
    }

    let arg = sp.checked_add(4)?;
    let header = mem.u32(arg).ok()?;
    let mut block = [0; 8];
    mem.read(header, &mut block).ok()?;
    let count = i32::from_le_bytes(block[..4].try_into().unwrap());
    if count <= 0 {
        return None;
    }
    let count = count as u32;
    let data = u32::from_le_bytes(block[4..].try_into().unwrap());
    let end = data.checked_add(count.checked_mul(ENTRY_BYTES as u32)?)?;
    let n = count.min(TAIL_ENTRIES as u32) as usize;
    let mut buf = [0; TAIL_ENTRIES * ENTRY_BYTES];
    let tail = &mut buf[..n * ENTRY_BYTES];
    mem.read(end - tail.len() as u32, tail).ok()?;
    // In prediction mode 1 these three cases jump directly to the common
    // success path. Keep every other type and everything preceding it.
    let skipped = tail
        .chunks_exact(ENTRY_BYTES)
        .rev()
        .take_while(|entry| matches!(u16::from_le_bytes([entry[0], entry[1]]), 0 | 7 | 8))
        .count() as u32;
    if skipped == 0 {
        return None;
    }
    block[..4].copy_from_slice(&(count - skipped).to_le_bytes());

    // 3f500 reserves 12 local bytes, saves EBX and EDI, then pushes its
    // argument and calls. At callee entry sp+16 and sp+20 are caller locals
    // first assigned at 3f558 and 3f564, after RET 4. sp+24 is live (PVS).
    // Give the guest a bounded view of the original BSP metadata in those
    // two dead words. Publish the new argument last; any write fault leaves
    // the original prediction list in use.
    let scratch = sp.checked_add(16)?;
    mem.write(scratch, &block).ok()?;
    mem.set_u32(arg, scratch).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Base, Registry, Switches};
    use engine::testing::FakeGuest;

    const SP: u32 = 0x1100;
    const HEADER: u32 = 0x2000;
    const DATA: u32 = 0x3000;

    fn fixture(kinds: &[u16]) -> FakeGuest {
        let mut mem = FakeGuest::new();
        mem.put_u32(SP, CAMERA_CLUSTER_RETURN)
            .put_u32(SP + 4, HEADER)
            .put_u32(HEADER, kinds.len() as u32)
            .put_u32(HEADER + 4, DATA);
        for (i, &kind) in kinds.iter().enumerate() {
            mem.put_u16(DATA + (i * ENTRY_BYTES) as u32, kind);
        }
        mem
    }

    #[test]
    fn only_trailing_mode_one_noops_are_hidden_from_the_original() {
        for (kinds, keep) in [
            (&[3, 7, 0, 4, 6, 0, 8, 7][..], 5),
            (&[0, 0, 7, 8][..], 0),
            (&[3, 0, 99, 7][..], 3),
        ] {
            let mut mem = fixture(kinds);
            for offset in [8, 12, 24, 28] {
                mem.put_u32(SP + offset, offset);
            }
            assert_eq!(trim_tail(&mut mem, SP, 1), Some(()));
            assert_eq!(mem.u32(SP + 4).unwrap(), SP + 16);
            assert_eq!(mem.u32(SP + 16).unwrap(), keep);
            assert_eq!(mem.u32(SP + 20).unwrap(), DATA);
            assert_eq!(mem.u32(HEADER).unwrap(), kinds.len() as u32);
            assert_eq!(mem.u32(SP).unwrap(), CAMERA_CLUSTER_RETURN);
            for offset in [8, 12, 24, 28] {
                assert_eq!(mem.u32(SP + offset).unwrap(), offset);
            }
        }
    }

    #[test]
    fn other_calls_and_unreadable_tails_keep_the_original_header() {
        for (kinds, caller, mode, data) in [
            (&[0][..], CAMERA_CLUSTER_RETURN, 2, DATA),
            (&[0][..], 0x0003_f61e, 1, DATA),
            (&[][..], CAMERA_CLUSTER_RETURN, 1, DATA),
            (&[0, 4][..], CAMERA_CLUSTER_RETURN, 1, DATA),
            (&[0][..], CAMERA_CLUSTER_RETURN, 1, 0xf000),
            (&[0][..], CAMERA_CLUSTER_RETURN, 1, u32::MAX),
        ] {
            let mut mem = fixture(kinds);
            mem.put_u32(SP, caller).put_u32(HEADER + 4, data);
            assert_eq!(trim_tail(&mut mem, SP, mode), None);
            assert_eq!(mem.u32(SP + 4).unwrap(), HEADER);
            assert_eq!(mem.u32(SP + 16).unwrap(), 0);
        }
    }

    #[test]
    fn unreadable_stack_scratch_keeps_the_original_argument() {
        let mut mem = fixture(&[3, 0, 8]);
        let sp = 0x7ff4;
        mem.put_u32(sp, CAMERA_CLUSTER_RETURN)
            .put_u32(sp + 4, HEADER);
        assert_eq!(trim_tail(&mut mem, sp, 1), None);
        assert_eq!(mem.u32(sp + 4).unwrap(), HEADER);
    }

    #[test]
    fn switch_controls_the_prediction_hook() {
        for (setting, enabled) in [("", true), ("-impl.sim.observer-resources", false)] {
            let mut registry = Registry::new(Switches::parse(setting).unwrap());
            registry.add(&ObserverResources).unwrap();
            let plan = registry.build(&Base).unwrap();
            assert_eq!(
                plan.chains.contains_key(&points::PREDICT_RESOURCES.base),
                enabled
            );
        }
    }
}
