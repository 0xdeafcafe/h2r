//! A phase 1d segment: the Rust body answers on its turn (`answer`, the
//! `impl.sim.native.anim.manager` switch); otherwise the original handler
//! answers and its outgoing frame is compared with the prediction.
use engine::sim_anim::manager::{self, arm};
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

pub(super) fn run(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    pc: u32,
    resume: bool,
    answer: bool,
) -> Result<TransferAbi, GuestFault> {
    if answer && arm::turn(pc) == Some(arm::Turn::Answer) {
        // A declined segment leaves the frame intact; the guest takes it
        // below and the prediction declines again, counted.
        if let Some(transfer) = manager::run(cpu, &mut ctx.memory(), pc)? {
            arm::answered(pc);
            return Ok(transfer);
        }
    }
    let prediction = arm::predict(cpu, &mut ctx.memory(), pc);
    // A direct-call image can execute a callee inside the caller's handler.
    // Stop the original at the same transfer as the prediction. Dispatch
    // runs the callback after comparison with its actual guest frame.
    let guest = ctx.with_routed_calls(|ctx| crate::sim_objects::original(cpu, ctx, pc, resume));
    if let Some(prediction) = prediction {
        match &guest {
            Ok(transfer) => prediction.compare(cpu, &ctx.memory(), *transfer),
            Err(_) => prediction.guest_error(),
        }
    }
    guest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_native::{ANIM_INVALIDATE, SimNative};
    use engine::{points, sim_anim::manager};
    use hooks::{Base, Registry, Switches};
    use runtime::{Dispatch, Guest, Runtime, guest::Permissions};
    use std::sync::atomic::{AtomicU64, Ordering};

    const FIND_CLIP: u32 = 0x1c6440;
    const GRAPH: u32 = 0x1daea0;
    const RETURN_FROM_GRAPH: u32 = 0x1c6466;
    const RETURN_FROM_PRELOAD: u32 = 0x1daf1f;
    const DATA: u32 = 0x10000;
    const SP: u32 = DATA + 0x800;

    #[test]
    fn fusion_hands_off_to_the_arm_and_the_guest_result_stands() {
        let mut d = Dispatch::default();
        for (point, _) in ANIM_INVALIDATE {
            d.register_generated_rust(point.base, |_, _, _| panic!("observer only"))
                .unwrap();
        }
        d.register_generated_rust(FIND_CLIP, |_, _, _| panic!("expected channel body"))
            .unwrap();
        d.register_generated_rust(GRAPH, |cpu, ctx, pc| {
            assert_eq!(pc, GRAPH);
            assert_eq!(cpu.esp, SP - 8);
            assert_eq!(ctx.memory().u32(SP - 12)?, 0); // predicted save stays private
            cpu.eax = 17;
            ctx.memory().set_u32(SP - 12, 13)?;
            cpu.eip = RETURN_FROM_GRAPH;
            cpu.esp = SP;
            Ok(TransferAbi::ret(RETURN_FROM_GRAPH))
        })
        .unwrap();
        let mut g = Guest::new();
        for body in manager::bodies().filter(|b| matches!(b.entry, FIND_CLIP | GRAPH)) {
            g.map_zeroed(body.entry, body.end - body.entry, Permissions::RX)
                .unwrap();
            for &pc in body.sites {
                d.register_resume(pc, body.entry, pc).unwrap();
            }
        }
        g.map_zeroed(DATA, 0x1000, Permissions::RW).unwrap();
        g.map_zeroed(points::TAG_ENTRIES.base, 4, Permissions::RW)
            .unwrap();
        g.set_u32(points::TAG_ENTRIES.base, DATA + 0x100).unwrap();
        g.set_u32(DATA + 0x108, DATA + 0x200).unwrap();
        g.set_u32(DATA + 0x230, DATA + 0x300).unwrap();
        g.set_u32(DATA + 4, 0x0001ffff).unwrap();
        let rt = Runtime::new(g, d);
        let mut registry = Registry::new(Switches::defaults().with("impl.sim.native.anim", true));
        registry.add(&SimNative::new(&rt)).unwrap();
        registry.build(&Base).unwrap().install(&rt).unwrap();
        let handler = |pc| rt.read_dispatch(|d| d.override_handler(pc)).unwrap();
        let mut ctx = ExecCtx::new(rt.clone(), 1);
        let mut cpu = CpuAbi {
            ecx: DATA,
            esp: SP,
            ebx: 19,
            ..Default::default()
        };
        assert_eq!(
            handler(FIND_CLIP).invoke(&mut cpu, &mut ctx, FIND_CLIP),
            TransferAbi::call(GRAPH, RETURN_FROM_GRAPH)
        );
        let differed = || {
            arm::stats()
                .into_iter()
                .find(|(pc, _)| *pc == GRAPH)
                .map_or(0, |(_, s)| s.differed)
        };
        let before = differed();
        assert_eq!(
            handler(GRAPH).invoke(&mut cpu, &mut ctx, GRAPH),
            TransferAbi::ret(RETURN_FROM_GRAPH)
        );
        assert_eq!(cpu.eax, 17);
        assert_eq!(ctx.memory().u32(SP - 12).unwrap(), 13);
        assert!(differed() > before);
    }

    #[test]
    fn resume_compares_the_generated_owners_selector() {
        const SELECTOR: u32 = 23;
        let mut d = Dispatch::default();
        d.register_generated_rust(GRAPH, |cpu, ctx, selector| {
            assert_eq!(selector, SELECTOR);
            let ram = ctx.memory();
            cpu.eax = cpu.esi;
            cpu.esi = ram.u32(cpu.esp)?;
            cpu.ebx = ram.u32(cpu.esp + 4)?;
            cpu.eip = ram.u32(cpu.esp + 8)?;
            cpu.esp += 16;
            Ok(TransferAbi::ret(cpu.eip))
        })
        .unwrap();
        d.register_resume(RETURN_FROM_PRELOAD, GRAPH, SELECTOR)
            .unwrap();
        let body = manager::bodies().find(|b| b.entry == GRAPH).unwrap();
        let mut g = Guest::new();
        g.map_zeroed(GRAPH, body.end - GRAPH, Permissions::RX)
            .unwrap();
        g.map_zeroed(DATA, 0x1000, Permissions::RW).unwrap();
        for (off, value) in [(0, 3), (4, 5), (8, RETURN_FROM_GRAPH)] {
            g.set_u32(SP + off, value).unwrap();
        }
        let mut ctx = ExecCtx::new(Runtime::new(g, d), 1);
        let mut cpu = CpuAbi {
            esp: SP,
            esi: 7,
            ..Default::default()
        };
        let matched = || {
            arm::stats()
                .into_iter()
                .find(|(pc, _)| *pc == GRAPH)
                .map_or(0, |(_, s)| s.matched)
        };
        let before = matched();
        assert_eq!(
            run(&mut cpu, &mut ctx, RETURN_FROM_PRELOAD, true, false).unwrap(),
            TransferAbi::ret(RETURN_FROM_GRAPH)
        );
        assert_eq!((cpu.eax, cpu.esi, cpu.ebx, cpu.esp), (7, 3, 5, SP + 16));
        assert!(matched() > before);
    }

    #[test]
    fn a_proven_entry_answers_from_rust_and_the_guest_takes_each_audit_slot() {
        // The graph view just after its graph-element callback: save the
        // argument and return PC, then enter the resource lookup with the
        // callback's answer in ESI. No other test here records this entry.
        const VIEW: u32 = 0x1ddb40;
        const AFTER_CALLBACK: u32 = 0x1ddb60;
        const RESOURCE: u32 = 0x1dd7c0;
        const RETURN_TO: u32 = 0x1ddb6a;
        static GUEST: AtomicU64 = AtomicU64::new(0);
        let mut d = Dispatch::default();
        d.register_generated_rust(VIEW, |cpu, ctx, selector| {
            assert_eq!(selector, AFTER_CALLBACK);
            GUEST.fetch_add(1, Ordering::Relaxed);
            let mut ram = ctx.memory();
            ram.set_u32(cpu.esp - 4, cpu.ebx)?;
            ram.set_u32(cpu.esp - 8, RETURN_TO)?;
            cpu.esp -= 8;
            cpu.ecx = cpu.ebp;
            cpu.esi = cpu.eax;
            cpu.eip = RESOURCE;
            Ok(TransferAbi::call(RESOURCE, RETURN_TO))
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
        let mut step = || {
            let mut cpu = entry;
            let t = run(&mut cpu, &mut ctx, AFTER_CALLBACK, true, true).unwrap();
            assert_eq!(t, TransferAbi::call(RESOURCE, RETURN_TO));
            assert_eq!((cpu.esp, cpu.ecx, cpu.esi), (SP - 8, 0x35, 0x123456));
            assert_eq!(ctx.memory().u32(SP - 8).unwrap(), RETURN_TO);
            assert_eq!(ctx.memory().u32(SP - 4).unwrap(), 0xabcdef01);
        };
        while counts().matched < arm::PROVEN_AFTER {
            step();
        }
        assert_eq!(counts().answered, 0);
        let guest = GUEST.load(Ordering::Relaxed);
        let n = arm::AUDIT_EVERY * 4;
        for _ in 0..n {
            step();
        }
        let after = counts();
        assert_eq!(GUEST.load(Ordering::Relaxed) - guest, 4);
        assert_eq!((after.answered, after.differed), (n - 4, 0));
    }

    #[test]
    fn the_manager_switch_alone_installs_the_phase_1d_entries() {
        let switches = Switches::defaults().with("impl.sim.native.anim.manager", true);
        let mut registry = Registry::new(switches);
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for entry in arm::ENTRIES {
            let hook = plan.chains[&entry].implement.as_ref().unwrap();
            assert_eq!(hook.label, crate::sim_native::ANIM, "{entry:#x}");
        }
        for (point, _) in ANIM_INVALIDATE {
            assert!(plan.chains.contains_key(&point.base));
        }
        assert_eq!(plan.chains.len(), arm::ENTRIES.len() + ANIM_INVALIDATE.len());
    }
}
