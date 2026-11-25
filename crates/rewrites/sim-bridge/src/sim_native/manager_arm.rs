//! Compare the outgoing frame while the original handler still owns the answer.
use engine::sim_anim::manager::arm;
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

pub(super) fn run(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    pc: u32,
    resume: bool,
) -> Result<TransferAbi, GuestFault> {
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
            run(&mut cpu, &mut ctx, RETURN_FROM_PRELOAD, true).unwrap(),
            TransferAbi::ret(RETURN_FROM_GRAPH)
        );
        assert_eq!((cpu.eax, cpu.esi, cpu.ebx, cpu.esp), (7, 3, 5, SP + 16));
        assert!(matched() > before);
    }
}
