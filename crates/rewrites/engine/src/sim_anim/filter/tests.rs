//! The filter step on synthetic guest memory (no image): hand-worked
//! cases down each path — snap, the speed bound, the acceleration bound
//! with either sign, the wrap — the call's stack and register shape, and
//! every decline leaving RAM and the CPU untouched. The lifted `11eed0`
//! is the oracle in `sim_physics_differential::first_person_filter_kernel`.
use super::*;
use runtime::Guest;
use runtime::guest::Permissions;

const RAM: u32 = 0x0080_0000;
const STACK: u32 = 0x007f_0000;
const VEL: u32 = 0x0050_0000;
const POS: u32 = 0x0050_0010;
const SITE: u32 = 0x0016_6f1e;

/// One call: the velocity and position, the speed bound, the wrap flag
/// and the five arguments.
#[derive(Clone, Copy)]
struct Case {
    v: f32,
    p: f32,
    speed: f32,
    wrap: bool,
    target: f32,
    tick: f32,
    accel: f32,
    lo: f32,
    hi: f32,
}

const BASE: Case = Case {
    v: 0.0,
    p: 0.0,
    speed: 4.0,
    wrap: false,
    target: 1.0,
    tick: 1.0,
    accel: 0.5,
    lo: -1.0,
    hi: 1.0,
};

fn machine(k: &Case) -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(0, RAM, Permissions::RW).unwrap();
    let w = |g: &mut Guest, a: u32, v: f32| g.set_u32(a, v.to_bits()).unwrap();
    w(&mut g, HALF, 0.5);
    w(&mut g, TWO, 2.0);
    w(&mut g, VEL, k.v);
    w(&mut g, POS, k.p);
    for (i, v) in [k.target, k.tick, k.accel, k.lo, k.hi].into_iter().enumerate() {
        w(&mut g, STACK + 4 * i as u32, v);
    }
    // Garbage in the locals the call overwrites.
    for i in 1..=4u32 {
        g.set_u32(STACK - 4 * i, 0xdead_0000 + i).unwrap();
    }
    let mut cpu = CpuAbi {
        eax: 0x1234_5678,
        ecx: 0x9abc_def0,
        edx: 0x5555_5500 | u32::from(k.wrap),
        ebx: 0x0bad_f00d,
        esp: STACK,
        ebp: 0x0060_0000,
        esi: VEL,
        edi: POS,
        eflags: 0x202,
        eflags_defined: 0,
        mxcsr: 0x1f80,
        ..CpuAbi::default()
    };
    cpu.x87.control = 0x027f;
    cpu.x87.tag = 0xffff;
    for (i, reg) in cpu.xmm.iter_mut().enumerate() {
        *reg = ((0x1234_u128 + i as u128) << 100).to_le_bytes();
    }
    cpu.xmm[5] = (u128::from(k.speed.to_bits()) | (0x77u128 << 64)).to_le_bytes();
    (cpu, g)
}

fn f(g: &Guest, a: u32) -> f32 {
    f32::from_bits(g.u32(a).unwrap())
}

fn low(c: &CpuAbi, r: usize) -> u32 {
    u32::from_le_bytes(c.xmm[r][..4].try_into().unwrap())
}

/// Run the call with the kernel forced on; it must not decline.
fn call(k: &Case) -> (CpuAbi, Guest, CpuAbi) {
    let (mut c, mut g) = machine(k);
    let before = c;
    assert!(with(true, || run(&mut c, &mut g, SITE)).unwrap(), "declined");
    (c, g, before)
}

/// The shape every completed call leaves: the stack popped past the
/// arguments, the return address and locals written, the callee-saved
/// and untouched registers kept, the flags of `add esp, 0Ch`.
fn shape(c: &CpuAbi, g: &Guest, before: &CpuAbi, k: &Case) {
    assert_eq!(c.esp, STACK + 0x14);
    assert_eq!(g.u32(STACK - 4).unwrap(), SITE);
    assert_eq!(f(g, STACK - 16), k.v, "the velocity local");
    assert_eq!(f(g, STACK - 8), k.speed * k.speed, "the squared speed local");
    for (a, b) in [
        (c.ebx, before.ebx),
        (c.ebp, before.ebp),
        (c.esi, before.esi),
        (c.edi, before.edi),
        (c.edx, before.edx),
    ] {
        assert_eq!(a, b);
    }
    assert_eq!(c.ecx >> 8, before.ecx >> 8);
    assert_eq!(c.xmm[5], before.xmm[5]);
    assert_eq!(u128::from_le_bytes(c.xmm[7]), 0);
    assert_eq!(low(c, 6), k.p.to_bits());
    // `add esp, 0Ch` from `STACK - 16`: no carry, not zero, even parity
    // of 0xfc's low byte... computed, not assumed.
    let mut probe = *before;
    crate::sim_physics::arithmetic(&mut probe, STACK - 16, 12, false);
    assert_eq!(c.eflags, probe.eflags);
    assert_eq!(c.eflags_defined & 0x8d5, 0x8d5);
    // Every push the body made was popped.
    assert_eq!(c.x87.tag, 0xffff);
    assert_eq!(c.x87.status & 0x3800, 0);
    assert_eq!(c.x87.instruction_selector, 8);
    assert_eq!(c.x87.data_selector, 16);
}

#[test]
fn a_target_within_reach_snaps() {
    // reach = min(tick² accel, tick speed) = 0.5 >= |0.25|.
    let k = Case {
        target: 0.25,
        ..BASE
    };
    let (c, g, before) = call(&k);
    shape(&c, &g, &before, &k);
    assert_eq!(f(&g, VEL), 0.0);
    assert_eq!(f(&g, POS), 0.25);
    assert_eq!(c.eax, (before.eax & !0xff) | 1);
    assert_eq!(c.ecx & 0xff, 1);
    assert_eq!(f(&g, STACK - 12), 0.5, "the reach local");
    assert_eq!(c.x87.instruction_pointer, SNAP_LAST);
    assert_eq!(c.x87.data_pointer, STACK - 12);
    // The tick argument is left alone on this path.
    assert_eq!(f(&g, STACK + 4), 1.0);
}

/// Observe the stores as they reach the bus, even when a later one
/// replaces the same word or leaves the value unchanged.
struct StoreTrace<'a> {
    guest: &'a mut Guest,
    writes: Vec<(u32, u32)>,
}

impl Memory for StoreTrace<'_> {
    fn physical_read(&self, addr: u32, len: usize) -> Option<u32> {
        Memory::physical_read(&*self.guest, addr, len)
    }

    fn read_bytes(&self, addr: u32, dst: &mut [u8], align: u32) -> Result<(), GuestFault> {
        self.guest.read_bytes(addr, dst, align)
    }

    fn write_bytes(&mut self, addr: u32, data: &[u8], align: u32) -> Result<(), GuestFault> {
        self.guest.write_bytes(addr, data, align)?;
        let word = u32::from_le_bytes(data.try_into().unwrap());
        self.writes.push((addr, word));
        Ok(())
    }
}

fn check_stores(k: Case, expected: &[(u32, f32)], snapped: bool) {
    let (mut c, mut g) = machine(&k);
    let mut trace = StoreTrace {
        guest: &mut g,
        writes: Vec::new(),
    };
    let mut counts = Stats::default();
    assert!(with(true, || run_counted(&mut c, &mut trace, SITE, &mut counts)).unwrap());
    let mut words = vec![(STACK - 4, SITE)];
    words.extend(expected.iter().map(|&(addr, value)| (addr, value.to_bits())));
    assert_eq!(trace.writes, words);
    assert_eq!(counts.native, 1);
    assert_eq!(counts.snapped, u64::from(snapped));
}

#[test]
fn snap_writes_both_reaches_and_leaves_tick_untouched() {
    check_stores(
        Case {
            target: 0.25,
            ..BASE
        },
        &[
            (STACK - 16, 0.0), (STACK - 8, 16.0), (STACK, 0.25),
            (STACK - 12, 4.0), (STACK - 12, 0.5),
            (VEL, 0.0), (POS, 0.25),
        ],
        true,
    );
}

#[test]
fn wrap_and_brake_write_each_intermediate_stack_word() {
    // The short way from -0.75 to 0.75 is -0.5. The stopping speed
    // is 0.5 in magnitude and the acceleration bound is 0.25.
    check_stores(
        Case {
            p: -0.75,
            target: 0.75,
            accel: 0.25,
            wrap: true,
            ..BASE
        },
        &[
            (STACK - 16, 0.0), (STACK - 8, 16.0),
            (STACK, 1.5), (STACK, -0.5),
            (STACK - 12, 4.0), (STACK - 12, 0.25),
            (STACK, 0.5), (STACK, -0.5), (STACK + 4, -0.5),
            (VEL, -0.25), (POS, 0.875),
        ],
        false,
    );
}

#[test]
fn a_snap_clamps_the_target() {
    let k = Case {
        target: 1.25,
        p: 1.0,
        ..BASE
    };
    let (c, g, before) = call(&k);
    shape(&c, &g, &before, &k);
    assert_eq!(f(&g, POS), 1.0);
    let k = Case {
        target: -1.25,
        p: -1.0,
        ..BASE
    };
    let (_, g, _) = call(&k);
    assert_eq!(f(&g, POS), -1.0);
}

#[test]
fn the_acceleration_bound_takes_the_sign() {
    // distance 1, reach 0.5: stop speed sqrt(2 * 0.5 * 1) = 1 under the
    // speed bound 4; acceleration 1 over the bound 0.5, so +0.5.
    let k = BASE;
    let (c, g, before) = call(&k);
    shape(&c, &g, &before, &k);
    assert_eq!(f(&g, VEL), 0.5);
    assert_eq!(f(&g, POS), 0.75);
    assert_eq!(c.eax, 0, "eax 1, then al 0");
    assert_eq!(c.ecx & 0xff, 0);
    assert_eq!(f(&g, STACK + 4), 1.0, "the raw acceleration over the tick");
    assert_eq!(f(&g, STACK), 1.0, "the stop speed over the target");
    assert_eq!(c.x87.instruction_pointer, MOVE_LAST);
    assert_eq!(c.x87.data_pointer, STACK - 16 + MOVE_OPERAND);

    let k = Case {
        target: -1.0,
        ..BASE
    };
    let (c, g, _) = call(&k);
    assert_eq!(f(&g, VEL), -0.5);
    assert_eq!(f(&g, POS), -0.75);
    assert_eq!(c.eax, 0xffff_ff00, "eax -1, then al 0");
    assert_eq!(f(&g, STACK), -1.0, "the stop speed, toward the target");
}

#[test]
fn the_speed_bound_caps_the_stop_speed() {
    // distance 3, accel 4, speed 1: 2 * 4 * 3 >= 1, so the speed bound;
    // acceleration 1 within the bound 4, so the caller's eax stays.
    let k = Case {
        target: 3.0,
        speed: 1.0,
        accel: 4.0,
        lo: -4.0,
        hi: 4.0,
        ..BASE
    };
    let (c, g, before) = call(&k);
    shape(&c, &g, &before, &k);
    assert_eq!(f(&g, STACK), 1.0);
    assert_eq!(f(&g, VEL), 1.0);
    assert_eq!(f(&g, POS), 1.5);
    assert_eq!(c.eax, before.eax & !0xff);
    // xmm0's upper lanes: loaded by `movss`, so clear.
    assert_eq!(u128::from_le_bytes(c.xmm[0]) >> 32, 0);
}

#[test]
fn the_wrap_takes_the_short_way() {
    // From 0.9 to -0.9 across [-1, 1]: the distance -1.8 wraps to 0.2.
    let k = Case {
        p: 0.9,
        target: -0.9,
        wrap: true,
        speed: 1.0,
        accel: 1.0,
        ..BASE
    };
    let (c, g, before) = call(&k);
    shape(&c, &g, &before, &k);
    assert_eq!(f(&g, STACK), 2.0f32 + (-0.9f32 - 0.9f32));
    // Within reach: the snap takes the unwrapped target.
    assert_eq!(f(&g, POS), -0.9);
    assert_eq!(c.eax & 0xff, 1);
    // The same move unwrapped is out of reach.
    let (c, _, _) = call(&Case { wrap: false, ..k });
    assert_eq!(c.eax & 0xff, 0);
}

#[test]
fn the_wrapped_position_comes_back_into_range() {
    // From 0.9 toward -0.5 across [-1, 1]: the distance -1.4 wraps to
    // 0.6, out of reach 0.25; the speed bound 0.5 and the acceleration
    // bound 0.25 move it 0.375 up, past 1, so it wraps by 2.
    let k = Case {
        p: 0.9,
        target: -0.5,
        wrap: true,
        speed: 0.5,
        accel: 0.25,
        ..BASE
    };
    let (c, g, before) = call(&k);
    shape(&c, &g, &before, &k);
    assert_eq!(c.eax & 0xff, 0);
    assert_eq!(f(&g, STACK), 0.5, "the speed bound");
    assert_eq!(f(&g, VEL), 0.25);
    assert_eq!(f(&g, POS), (0.375f32 + 0.9f32) - 2.0f32);
}

fn ram(g: &Guest) -> Vec<u8> {
    let start = g.mappings()[0].physical_start as usize;
    g.physical_ram()[start..start + RAM as usize].to_vec()
}

/// A decline writes nothing and changes no register, and the call panics
/// with the reason; no guest call runs.
fn declines(k: &Case, edit: impl FnOnce(&mut CpuAbi, &mut Guest), why: Decline) {
    let (mut c, mut g) = machine(k);
    edit(&mut c, &mut g);
    let (cpu, mem) = (c, ram(&g));
    assert_eq!(plan(&c, &g, SITE).err(), Some(why), "{why:?}");
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = with(true, || run(&mut c, &mut g, SITE));
    }))
    .expect_err("a decline panics");
    let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.ends_with(&format!("anim filter: {why}")), "{why:?}: {message}");
    assert_eq!(c, cpu);
    assert!(ram(&g) == mem, "RAM written on a decline");
}

#[test]
fn text_across_the_loaders_page_mappings_runs() {
    // Ascension: `anim filter: fault`. The body's text 11eed0..11f0c2
    // crosses 0x11f000, and the loader maps each image page on its own.
    // `map_zeroed` would put the second page back beside the first in
    // their host page, so it gets fresh zeroed storage above the RAM.
    let (mut c, mut g) = machine(&BASE);
    let (mut c0, mut g0) = machine(&BASE);
    let base = g.mappings()[0].physical_start;
    g.unmap(0x11e000, 0x2000).unwrap();
    g.map(0x11e000, 0x1000, base + 0x11e000, Permissions::RX).unwrap();
    g.map(0x11f000, 0x1000, base + RAM + 0x1000, Permissions::RX).unwrap();
    assert!(Memory::physical_read(&g, FILTER, (FILTER_END - FILTER) as usize).is_none());
    assert!(plan(&c, &g, SITE).is_ok());
    assert!(with(true, || run(&mut c, &mut g, SITE)).unwrap());
    assert!(with(true, || run(&mut c0, &mut g0, SITE)).unwrap());
    assert_eq!(c, c0);
    assert_eq!((f(&g, VEL), f(&g, POS)), (f(&g0, VEL), f(&g0, POS)));
}

#[test]
fn unreadable_inputs_decline() {
    let fault = Decline::Fault { addr: RAM + 0x100, len: 4 };
    declines(&BASE, |c, _| c.esi = RAM + 0x100, fault);
    declines(&BASE, |c, _| c.edi = RAM + 0x100, fault);
    assert_eq!(fault.to_string(), format!("fault: read of 4 bytes at {:#010x}", RAM + 0x100));
}

#[test]
fn state_in_the_stack_window_declines() {
    declines(&BASE, |c, _| c.esi = STACK - 8, Decline::Alias);
    declines(&BASE, |c, _| c.edi = STACK + 0x10, Decline::Alias);
    declines(&BASE, |c, _| c.edi = STACK + 0x12, Decline::Alias);
}

#[test]
fn virtual_aliases_of_stack_constants_and_instructions_decline() {
    // Explicitly map each source span at a second, disjoint guest VA.
    const ALIAS: u32 = RAM + 0x1000;
    fn alias(g: &mut Guest, at: u32, len: u32) {
        let phys = Memory::physical_read(g, at, len as usize).unwrap();
        g.map(ALIAS, len, phys, Permissions::RW).unwrap();
    }

    declines(
        &BASE,
        |c, g| {
            alias(g, STACK - 6, 4);
            c.esi = ALIAS;
        },
        Decline::Alias,
    );

    for target in [HALF, TWO] {
        declines(
            &BASE,
            |c, g| {
                alias(g, target, (4 * WINDOW) as u32);
                c.esp = ALIAS + 16;
            },
            Decline::Alias,
        );
    }
    declines(
        &BASE,
        |c, g| {
            alias(g, MOVE_LAST - 1, 4);
            c.edi = ALIAS;
        },
        Decline::Alias,
    );
}

#[test]
fn an_unmasked_exception_declines() {
    // Precision unmasked: the first inexact SSE result would trap.
    let k = Case { v: 0.1, ..BASE };
    declines(&k, |c, _| c.mxcsr = 0x1f80 & !0x1000, Decline::Float);
}

#[test]
fn the_switch_gates_the_kernel() {
    let (mut c, mut g) = machine(&BASE);
    let (cpu, mem) = (c, ram(&g));
    assert!(!with(false, || run(&mut c, &mut g, SITE)).unwrap());
    assert_eq!(c, cpu);
    assert!(ram(&g) == mem);
}

#[test]
fn the_kernel_is_its_own_input_free_function() {
    // `step` on gathered state matches `run` on guest memory.
    let k = BASE;
    let (c0, g0) = machine(&k);
    let mut window = [0u32; WINDOW];
    for (i, v) in window.iter_mut().enumerate() {
        *v = g0.u32(STACK - 16 + 4 * i as u32).unwrap();
    }
    window[RETURN] = SITE;
    let out = step(&In {
        velocity: k.v.to_bits(),
        position: k.p.to_bits(),
        window,
        xmm: c0.xmm.map(u128::from_le_bytes),
        mxcsr: c0.mxcsr,
        fpu: fpu_in(&c0),
        wrap: false,
        half: 0.5f32.to_bits(),
        two: 2.0f32.to_bits(),
    })
    .unwrap();
    let (c, g, _) = call(&k);
    assert_eq!(out.velocity, g.u32(VEL).unwrap());
    assert_eq!(out.position, g.u32(POS).unwrap());
    assert_eq!(out.mxcsr, c.mxcsr);
    assert_eq!(out.xmm, c.xmm.map(u128::from_le_bytes));
}
