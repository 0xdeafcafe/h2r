//! A phase 1d segment: the Rust body answers it. A segment the body
//! declines leaves the frame intact; the original runs it, counted
//! (`manager::arm::declined`).
use engine::sim_anim::manager::{self, arm};
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

pub(super) fn run(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    pc: u32,
    resume: bool,
) -> Result<TransferAbi, GuestFault> {
    if let Some(transfer) = manager::run(cpu, &mut ctx.memory(), pc)? {
        arm::answered(pc);
        return Ok(transfer);
    }
    arm::declined(pc);
    crate::sim_objects::original(cpu, ctx, pc, resume)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_native::{ANIM_INVALIDATE, SimNative};
    use engine::sim_anim::manager;
    use hooks::{Base, Registry, Switches};
    use runtime::{Dispatch, Guest, Runtime, guest::Permissions};
    use std::sync::atomic::{AtomicU64, Ordering};

    const DATA: u32 = 0x10000;
    const SP: u32 = DATA + 0x800;

    #[test]
    fn the_rust_answers_every_segment_and_never_enters_the_guest() {
        // The graph view just after its graph-element callback: save the
        // argument and return PC, then enter the resource lookup with the
        // callback's answer in ESI. No other test here records this entry.
        const VIEW: u32 = 0x1ddb40;
        const AFTER_CALLBACK: u32 = 0x1ddb60;
        const RESOURCE: u32 = 0x1dd7c0;
        const RETURN_TO: u32 = 0x1ddb6a;
        static GUEST: AtomicU64 = AtomicU64::new(0);
        let mut d = Dispatch::default();
        d.register_generated_rust(VIEW, |_, _, _| {
            GUEST.fetch_add(1, Ordering::Relaxed);
            panic!("the guest never runs an answerable segment")
        })
        .unwrap();
        d.register_resume(AFTER_CALLBACK, VIEW, AFTER_CALLBACK).unwrap();
        let body = manager::bodies().find(|b| b.entry == VIEW).unwrap();
        let mut g = Guest::new();
        g.map_zeroed(VIEW, body.end - VIEW, Permissions::RX).unwrap();
        g.map_zeroed(DATA, 0x1000, Permissions::RW).unwrap();
        let mut ctx = ExecCtx::new(Runtime::new(g, d), 1);
        let entry = CpuAbi {
            eip: AFTER_CALLBACK,
            esp: SP,
            eax: 0x123456,
            ebx: 0xabcdef01,
            ebp: 0x35,
            ..Default::default()
        };
        let counts = || {
            arm::stats()
                .into_iter()
                .find(|(pc, _)| *pc == VIEW)
                .map(|(_, s)| s)
                .unwrap_or_default()
        };
        let before = counts();
        for _ in 0..8 {
            let mut cpu = entry;
            let t = run(&mut cpu, &mut ctx, AFTER_CALLBACK, true).unwrap();
            assert_eq!(t, TransferAbi::call(RESOURCE, RETURN_TO));
            assert_eq!((cpu.esp, cpu.ecx, cpu.esi), (SP - 8, 0x35, 0x123456));
            assert_eq!(ctx.memory().u32(SP - 8).unwrap(), RETURN_TO);
            assert_eq!(ctx.memory().u32(SP - 4).unwrap(), 0xabcdef01);
        }
        assert_eq!(GUEST.load(Ordering::Relaxed), 0);
        assert_eq!(counts().answered, before.answered + 8);
    }

    #[test]
    fn the_defaults_install_the_phase_1d_entries() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for entry in arm::ENTRIES {
            let hook = plan.chains[&entry].implement.as_ref().unwrap();
            assert_eq!(hook.label, crate::sim_native::ANIM, "{entry:#x}");
        }
        for (point, _) in ANIM_INVALIDATE {
            assert!(plan.chains.contains_key(&point.base));
        }
    }
}
