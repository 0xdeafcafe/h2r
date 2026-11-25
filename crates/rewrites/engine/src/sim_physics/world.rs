//! Havok step, island callbacks and the Halo overflow bookkeeping. Every
//! transfer parks the cursor in the caller's real frame. A callback can
//! replace an island's arrays or the current motion's vtable. The two
//! world phases stay guest code: their loops call the island entries
//! below, which the dispatcher answers natively.
use super::*;

const MONITOR_HEAD: u32 = 0x0048_0154;
const MONITOR_END: u32 = 0x0048_0158;
const WORLD: u32 = 0x20;
const GRAVITY: u32 = 0xe0;

/// The step's entry condition: the monitor stream must be off, because
/// step functions append a timer record while `[MONITOR_HEAD] <
/// [MONITOR_END]` (sim-rewrite-havok.md §3.3). A read that faults declines.
pub(super) fn monitor_off(g: &Guest) -> bool {
    g.u32(MONITOR_HEAD)
        .is_ok_and(|head| g.u32(MONITOR_END).is_ok_and(|end| head >= end))
}

pub(super) const WORLD_SITES: &[u32] = &[0x31110e, 0x311116];
pub(super) const ISLAND_SITES: &[u32] = &[0x317d3a, 0x317d4e, 0x317d63, 0x317d69, 0x317d76, 0x317d7c];
pub(super) const INTEGRATE_SITES: &[u32] = &[0x316a48, 0x316aac, 0x316ab4, 0x316b16];

fn advance_stack(c: &mut CpuAbi, n: u32) {
    c.esp = arithmetic(c, c.esp, n, false);
}

fn decrement(c: &mut CpuAbi, x: u32) -> u32 {
    let carry = (c.eflags & 1, c.eflags_defined & 1);
    let out = arithmetic(c, x, 1, true);
    c.eflags = (c.eflags & !1) | carry.0;
    c.eflags_defined = (c.eflags_defined & !1) | carry.1;
    out
}

fn restore_island(c: &mut CpuAbi, g: &Guest, cleanup: u32) -> Result<TransferAbi> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    g.ret(c, cleanup)
}

pub(super) fn step(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    match pc {
        0x311100 => {
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.edi = g.u32(c.esp.wrapping_add(12))?;
            c.esi = c.ecx;
            call_arg(c, g, c.edi, 0x30fcb0, WORLD_SITES[0])
        }
        0x31110e => {
            c.ecx = c.esi;
            call_arg(c, g, c.edi, 0x30fef0, WORLD_SITES[1])
        }
        0x311116 => restore_island(c, g, 4),
        _ => unreachable!(),
    }
}

pub(super) fn collide_island(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    match pc {
        0x317d30 => {
            push(c, g, c.esi)?;
            c.esi = c.ecx;
            push(c, g, c.edi)?;
            call_arg(c, g, c.esi, 0x278e60, ISLAND_SITES[0])
        }
        0x317d3a => {
            c.edi = g.u32(c.esp.wrapping_add(20))?;
            c.eax = g.u32(c.esp.wrapping_add(16))?;
            advance_stack(c, 4);
            push(c, g, c.edi)?;
            c.ecx = c.esi;
            call_arg(c, g, c.eax, 0x3175e0, ISLAND_SITES[1])
        }
        0x317d4e => {
            let world = g.u32(c.esi.wrapping_add(WORLD))?;
            let adaptive = g.u8(world.wrapping_add(0x23e))?;
            c.eax = byte(c.eax, adaptive);
            logic(c, u32::from(adaptive), 0x80);
            c.ecx = c.esi;
            let (target, resume) = if adaptive != 0 {
                (0x317950, ISLAND_SITES[2])
            } else {
                (0x317c00, ISLAND_SITES[4])
            };
            call_arg(c, g, c.edi, target, resume)
        }
        0x317d63 | 0x317d76 => {
            let resume = if pc == ISLAND_SITES[2] { ISLAND_SITES[3] } else { ISLAND_SITES[5] };
            call_arg(c, g, c.esi, 0x278e80, resume)
        }
        0x317d69 | 0x317d7c => {
            advance_stack(c, 4);
            restore_island(c, g, 8)
        }
        _ => unreachable!(),
    }
}

fn finish_motion_pass(c: &mut CpuAbi, g: &Guest) -> Result<TransferAbi> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    g.ret(c, 4)
}

fn apply_motion(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    // Entity membership and gravity are live across both virtuals.
    let entities = g.u32(c.edi.wrapping_add(0x3c))?;
    c.edx = g.u32(entities.wrapping_add(c.ebx.wrapping_mul(4)))?;
    let world = g.u32(c.edi.wrapping_add(WORLD))?;
    c.esi = g.u32(c.edx.wrapping_add(0x3c))?;
    c.eax = g.u32(c.esi)?;
    let gravity = arithmetic(c, world, GRAVITY, false);
    push(c, g, gravity)?;
    push(c, g, c.ebp)?;
    c.ecx = c.esi;
    let target = g.u32(c.eax.wrapping_add(8))?;
    g.call(c, target, INTEGRATE_SITES[1])
}

pub(super) fn integrate_island(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    match pc {
        0x316a20 => {
            for word in [c.ecx, c.ebx, c.ebp, c.esi, c.edi] { push(c, g, word)?; }
            c.edi = c.ecx;
            c.esi = g.u32(c.edi.wrapping_add(0x4c))?;
            c.eax = g.u32(c.edi.wrapping_add(0x50))?;
            c.ecx = c.esi;
            c.ebx = c.esi.wrapping_add(c.eax.wrapping_mul(4));
            arithmetic(c, c.esi, c.ebx, true);
            if c.esi != c.ebx {
                let input = g.u32(c.esp.wrapping_add(24))?;
                c.ebp = arithmetic(c, input, 16, false);
            }
        }
        0x316a48 => {
            c.esi = c.esi.wrapping_add(4);
            arithmetic(c, c.esi, c.ebx, true);
        }
        0x316aac => {
            c.edx = g.u32(c.esi)?;
            push(c, g, c.ebp)?;
            c.ecx = c.esi;
            let target = g.u32(c.edx.wrapping_add(4))?;
            return g.call(c, target, INTEGRATE_SITES[2]);
        }
        0x316ab4 => {
            c.ebx = decrement(c, c.ebx);
            if c.ebx as i32 >= 0 { return apply_motion(c, g); }
            c.ecx = g.u32(MONITOR_HEAD)?;
            arithmetic(c, c.ecx, g.u32(MONITOR_END)?, true);
            return finish_motion_pass(c, g);
        }
        0x316b16 => {
            advance_stack(c, 36);
            return finish_motion_pass(c, g);
        }
        _ => unreachable!(),
    }
    // The actions' end pointer is captured once, but each action is read
    // when its turn arrives. Only afterwards choose the constraint path.
    if c.esi != c.ebx {
        c.ecx = g.u32(c.esi)?;
        c.edx = g.u32(c.ecx)?;
        push(c, g, c.ebp)?;
        let target = g.u32(c.edx.wrapping_add(0x14))?;
        return g.call(c, target, INTEGRATE_SITES[0]);
    }
    c.eax = g.u32(c.edi.wrapping_add(0x5c))?;
    logic(c, c.eax, 1 << 31);
    if c.eax != 0 {
        return solve_island(c, g);
    }
    c.ecx = g.u32(MONITOR_HEAD)?;
    arithmetic(c, c.ecx, g.u32(MONITOR_END)?, true);
    let count = g.u32(c.edi.wrapping_add(0x40))?;
    c.ebx = decrement(c, count);
    if (c.ebx as i32) < 0 {
        arithmetic(c, c.ecx, g.u32(MONITOR_END)?, true);
        return finish_motion_pass(c, g);
    }
    let input = g.u32(c.esp.wrapping_add(24))?;
    c.ebp = arithmetic(c, input, 16, false);
    apply_motion(c, g)
}

fn solve_island(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    c.ecx = g.u32(c.edi.wrapping_add(0x58))?;
    c.edx = g.u32(c.edi.wrapping_add(0x40))?;
    push(c, g, c.eax)?;
    c.eax = g.u32(c.edi.wrapping_add(0x3c))?;
    push(c, g, c.ecx)?;
    c.ecx = g.u32(c.edi.wrapping_add(WORLD))?;
    push(c, g, c.edx)?;
    push(c, g, c.eax)?;
    let input = g.u32(c.esp.wrapping_add(40))?;
    push(c, g, c.edi)?;
    c.ecx = arithmetic(c, c.ecx, GRAVITY, false);
    push(c, g, c.ecx)?;
    c.edx = input.wrapping_add(0x50);
    push(c, g, c.edx)?;
    c.ecx = input.wrapping_add(0x20);
    push(c, g, c.ecx)?;
    c.eax = arithmetic(c, input, 16, false);
    call_arg(c, g, c.eax, 0x31c100, INTEGRATE_SITES[3])
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    const STACK: u32 = 0x0020_0800;
    const ISLAND: u32 = 0x0021_0000;
    const WORLD_AT: u32 = 0x0022_0000;
    const INPUT: u32 = 0x0023_0000;
    const STEP: u32 = INPUT + 0x10;

    fn guest() -> Guest {
        let mut g = Guest::new();
        for at in [0x0020_0000, ISLAND & !0xfff, WORLD_AT & !0xfff, INPUT & !0xfff, 0x0048_0000] {
            g.map_zeroed(at, 0x2000, Permissions::RW).unwrap();
        }
        // The monitor stream is off: the head sits at the end.
        g.set_u32(MONITOR_HEAD, MONITOR_END).unwrap();
        g.set_u32(MONITOR_END, MONITOR_END).unwrap();
        g
    }

    fn cpu(g: &mut Guest, esp: u32, ret: u32) -> CpuAbi {
        let mut c = CpuAbi {
            ecx: ISLAND,
            ebx: 0x1111_2222,
            ebp: 0x3333_4444,
            esi: 0x5555_6666,
            edi: 0x7777_8888,
            esp,
            ..CpuAbi::default()
        };
        g.set_u32(esp, ret).unwrap();
        c
    }

    fn ret<const N: u32>(c: &mut CpuAbi, g: &mut Guest, pc: u32) {
        // Serve one guest call: pop the resume PC, then `ret N` arguments.
        let _ = g.u32(c.esp).unwrap();
        c.esp += 4 + N;
        c.eip = pc;
    }

    #[test]
    fn the_step_wrapper_calls_both_phases_and_returns_to_the_caller() {
        let mut g = guest();
        let mut c = cpu(&mut g, STACK, 0x1234_5678);
        g.set_u32(STACK + 4, STEP).unwrap();
        assert_eq!(
            step(&mut c, &mut g, 0x311100).unwrap(),
            TransferAbi::call(0x30fcb0, WORLD_SITES[0])
        );
        assert_eq!(g.u32(c.esp).unwrap(), WORLD_SITES[0]);
        assert_eq!(g.u32(c.esp + 4).unwrap(), STEP);
        ret::<4>(&mut c, &mut g, WORLD_SITES[0]);
        assert_eq!(
            step(&mut c, &mut g, WORLD_SITES[0]).unwrap(),
            TransferAbi::call(0x30fef0, WORLD_SITES[1])
        );
        assert_eq!(g.u32(c.esp).unwrap(), WORLD_SITES[1]);
        assert_eq!(g.u32(c.esp + 4).unwrap(), STEP);
        ret::<4>(&mut c, &mut g, WORLD_SITES[1]);
        assert_eq!(step(&mut c, &mut g, WORLD_SITES[1]).unwrap(), TransferAbi::ret(0x1234_5678));
        assert_eq!(c.esp, STACK + 8);
        assert_eq!((c.esi, c.edi), (0x5555_6666, 0x7777_8888));
    }

    #[test]
    fn collide_island_brackets_the_guest_updates_with_the_halo_hooks() {
        let mut g = guest();
        g.set_u32(ISLAND + WORLD, WORLD_AT).unwrap();
        let broadphase = 0x0024_0000;
        let mut c = cpu(&mut g, STACK, 0x1234_5678);
        g.set_u32(STACK + 4, broadphase).unwrap();
        g.set_u32(STACK + 8, INPUT).unwrap();
        assert_eq!(
            collide_island(&mut c, &mut g, 0x317d30).unwrap(),
            TransferAbi::call(0x278e60, ISLAND_SITES[0])
        );
        assert_eq!(g.u32(c.esp + 4).unwrap(), ISLAND);
        ret::<0>(&mut c, &mut g, ISLAND_SITES[0]);
        // The island broadphase update takes (broadphase, input).
        assert_eq!(
            collide_island(&mut c, &mut g, ISLAND_SITES[0]).unwrap(),
            TransferAbi::call(0x3175e0, ISLAND_SITES[1])
        );
        assert_eq!((g.u32(c.esp + 4).unwrap(), g.u32(c.esp + 8).unwrap()), (broadphase, INPUT));
        ret::<8>(&mut c, &mut g, ISLAND_SITES[1]);
        // Adaptive processing is off: the ordinary pair pass runs.
        g.set_u8(WORLD_AT + 0x23e, 0).unwrap();
        assert_eq!(
            collide_island(&mut c, &mut g, ISLAND_SITES[1]).unwrap(),
            TransferAbi::call(0x317c00, ISLAND_SITES[4])
        );
        assert_eq!(g.u32(c.esp + 4).unwrap(), INPUT);
        ret::<4>(&mut c, &mut g, ISLAND_SITES[4]);
        assert_eq!(
            collide_island(&mut c, &mut g, ISLAND_SITES[4]).unwrap(),
            TransferAbi::call(0x278e80, ISLAND_SITES[5])
        );
        assert_eq!(g.u32(c.esp + 4).unwrap(), ISLAND);
        ret::<0>(&mut c, &mut g, ISLAND_SITES[5]);
        assert_eq!(collide_island(&mut c, &mut g, ISLAND_SITES[5]).unwrap(), TransferAbi::ret(0x1234_5678));
        assert_eq!(c.esp, STACK + 12);
        // The adaptive pass takes the other branch and its own sites.
        let mut c = cpu(&mut g, STACK, 0x1234_5678);
        g.set_u32(STACK + 4, broadphase).unwrap();
        g.set_u32(STACK + 8, INPUT).unwrap();
        let _ = collide_island(&mut c, &mut g, 0x317d30).unwrap();
        ret::<0>(&mut c, &mut g, ISLAND_SITES[0]);
        let _ = collide_island(&mut c, &mut g, ISLAND_SITES[0]).unwrap();
        ret::<8>(&mut c, &mut g, ISLAND_SITES[1]);
        g.set_u8(WORLD_AT + 0x23e, 1).unwrap();
        assert_eq!(
            collide_island(&mut c, &mut g, ISLAND_SITES[1]).unwrap(),
            TransferAbi::call(0x317950, ISLAND_SITES[2])
        );
    }

    #[test]
    fn integrate_island_runs_each_action_then_each_motion_in_reverse() {
        let mut g = guest();
        let (action, action_table, entities, entity, motion, motion_table) =
            (0x0024_0000, 0x0025_0000, 0x0026_0000, 0x0027_0000, 0x0028_0000, 0x0029_0000);
        for at in [action, action_table, entities, entity, motion, motion_table, 0x002a_0000, 0x002b_0000] {
            g.map_zeroed(at, 0x1000, Permissions::RW).unwrap();
        }
        g.set_u32(ISLAND + 0x4c, action).unwrap();
        g.set_u32(ISLAND + 0x50, 1).unwrap();
        g.set_u32(action, action_table).unwrap();
        g.set_u32(action_table + 0x14, 0x002a_0000).unwrap();
        g.set_u32(ISLAND + 0x3c, entities).unwrap();
        g.set_u32(ISLAND + 0x40, 1).unwrap();
        g.set_u32(entities, entity).unwrap();
        g.set_u32(entity + 0x3c, motion).unwrap();
        g.set_u32(motion, motion_table).unwrap();
        g.set_u32(motion_table + 8, 0x002a_0000).unwrap();
        g.set_u32(motion_table + 4, 0x002b_0000).unwrap();
        g.set_u32(ISLAND + WORLD, WORLD_AT).unwrap();
        let mut c = cpu(&mut g, STACK, 0x1234_5678);
        g.set_u32(STACK + 4, INPUT).unwrap();
        // The action's virtual +0x14 receives the step at W+0x150.
        assert_eq!(
            integrate_island(&mut c, &mut g, 0x316a20).unwrap(),
            TransferAbi::call(0x002a_0000, INTEGRATE_SITES[0])
        );
        assert_eq!(g.u32(c.esp + 4).unwrap(), STEP);
        ret::<4>(&mut c, &mut g, INTEGRATE_SITES[0]);
        // No constraints: each entity's motion virtuals, pose then forces.
        assert_eq!(
            integrate_island(&mut c, &mut g, INTEGRATE_SITES[0]).unwrap(),
            TransferAbi::call(0x002a_0000, INTEGRATE_SITES[1])
        );
        assert_eq!(g.u32(c.esp + 4).unwrap(), STEP);
        assert_eq!(g.u32(c.esp + 8).unwrap(), WORLD_AT + GRAVITY);
        ret::<4>(&mut c, &mut g, INTEGRATE_SITES[1]);
        assert_eq!(
            integrate_island(&mut c, &mut g, INTEGRATE_SITES[1]).unwrap(),
            TransferAbi::call(0x002b_0000, INTEGRATE_SITES[2])
        );
        assert_eq!(g.u32(c.esp + 4).unwrap(), STEP);
        assert_eq!(g.u32(c.esp + 8).unwrap(), WORLD_AT + GRAVITY);
        ret::<8>(&mut c, &mut g, INTEGRATE_SITES[2]);
        // The last entity consumed: the pass returns to the caller.
        assert_eq!(
            integrate_island(&mut c, &mut g, INTEGRATE_SITES[2]).unwrap(),
            TransferAbi::ret(0x1234_5678)
        );
        assert_eq!(c.esp, STACK + 8);
        assert_eq!(
            (c.ecx, c.ebx, c.ebp, c.esi, c.edi),
            (ISLAND, 0x1111_2222, 0x3333_4444, 0x5555_6666, 0x7777_8888)
        );
    }

    #[test]
    fn a_constrained_island_passes_nine_solver_arguments_in_guest_order() {
        let mut g = guest();
        g.set_u32(ISLAND + 0x50, 0).unwrap();
        g.set_u32(ISLAND + 0x5c, 3).unwrap();
        g.set_u32(ISLAND + 0x58, 0x0024_0000).unwrap();
        g.set_u32(ISLAND + 0x3c, 0x0025_0000).unwrap();
        g.set_u32(ISLAND + 0x40, 7).unwrap();
        g.set_u32(ISLAND + WORLD, WORLD_AT).unwrap();
        g.map_zeroed(0x0024_0000, 0x2000, Permissions::RW).unwrap();
        let mut c = cpu(&mut g, STACK, 0x1234_5678);
        g.set_u32(STACK + 4, INPUT).unwrap();
        assert_eq!(
            integrate_island(&mut c, &mut g, 0x316a20).unwrap(),
            TransferAbi::call(0x31c100, INTEGRATE_SITES[3])
        );
        for (at, word) in [
            (c.esp + 4, STEP),
            (c.esp + 8, INPUT + 0x20),
            (c.esp + 12, INPUT + 0x50),
            (c.esp + 16, WORLD_AT + GRAVITY),
            (c.esp + 20, ISLAND),
            (c.esp + 24, 0x0025_0000),
            (c.esp + 28, 7),
            (c.esp + 32, 0x0024_0000),
            (c.esp + 36, 3),
        ] {
            assert_eq!(g.u32(at).unwrap(), word, "{at:#x}");
        }
        // The solve is cdecl: it pops only its return; the caller drops
        // the 0x24 argument bytes at the resume.
        ret::<0>(&mut c, &mut g, INTEGRATE_SITES[3]);
        assert_eq!(
            integrate_island(&mut c, &mut g, INTEGRATE_SITES[3]).unwrap(),
            TransferAbi::ret(0x1234_5678)
        );
        assert_eq!(c.esp, STACK + 8);
    }

    #[test]
    fn the_step_declines_while_the_monitor_stream_is_on() {
        let mut g = Guest::new();
        g.map_zeroed(0x0048_0000, 0x1000, Permissions::RW).unwrap();
        assert!(monitor_off(&g));
        g.set_u32(MONITOR_HEAD, 0).unwrap();
        assert!(!monitor_off(&g));
        g.set_u32(MONITOR_HEAD, MONITOR_END + 4).unwrap();
        assert!(monitor_off(&g));
        let unmapped = Guest::new();
        assert!(!monitor_off(&unmapped));
    }
}
