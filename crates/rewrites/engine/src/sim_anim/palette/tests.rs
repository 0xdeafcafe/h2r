//! The adapter against the port's own loops on synthetic guest memory
//! (no image): with the kernel on, the palette step must leave the same
//! RAM, byte for byte, and the same CPU as with it off. The port itself,
//! and with it this path, is held equal to the lifted guest by the
//! first-person differential tests.
use super::*;
use runtime::Guest;
use runtime::guest::Permissions;
use std::sync::Mutex;

/// The kernel switch is process-global; the tests that flip it serialise
/// on this lock so none of them runs another's setting.
static SWITCH: Mutex<()> = Mutex::new(());

const RAM: u32 = 0x0080_0000;
const STACK: u32 = 0x007f_0000;
const FRAME: u32 = 0x0050_0000;
const SLOT: u32 = 0x0051_0000;
const PALETTE_BASE: u32 = 0x0040_0000;
const IDENTITY_AT: u32 = 0x0052_0000;
const MODEL_ONE: u32 = 0x0053_0000;
const MODEL_TWO: u32 = 0x0054_0000;
const NODES_ONE: u32 = 0x0055_0000;
const NODES_TWO: u32 = 0x0056_0000;
const GRAPH: u32 = 0x0057_0000;
const EDI_SAVE: u32 = 0x0058_0000;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// One palette step: the fill count, the two models (node counts and
/// their palette indices), and the seat arguments.
#[derive(Clone)]
struct Case {
    nodes: i32,
    counts: [i32; 2],
    /// `None` for the sentinel: no slot.
    indices: [Vec<Option<u16>>; 2],
    a: u32,
    b: u32,
    seed: u64,
    df: bool,
}

fn case(seed: u64, nodes: i32, counts: [i32; 2]) -> Case {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9) | 1);
    let pick = |rng: &mut Rng| -> Vec<Option<u16>> {
        (0..4)
            .map(|_| match rng.next() % 5 {
                0 => None,
                1 => Some(0x8000),
                _ => Some((rng.next() % 40) as u16),
            })
            .collect()
    };
    Case {
        nodes,
        counts,
        indices: [pick(&mut rng), pick(&mut rng)],
        a: 0,
        b: 0,
        seed,
        df: seed % 3 == 0,
    }
}

/// The palette step's entry state: the port's frame and registers at
/// `POSE_SITES[3]`.
fn machine(c: &Case) -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(0, RAM, Permissions::RW).unwrap();
    let mut rng = Rng(c.seed ^ 0x5bd1_e995);
    let w = |g: &mut Guest, a: u32, v: u32| g.set_u32(a, v).unwrap();

    w(&mut g, PALETTES, PALETTE_BASE);
    w(&mut g, IDENTITY, IDENTITY_AT);
    w(&mut g, ONE, 1.0f32.to_bits());
    for i in 0..8u32 {
        w(&mut g, IDENTITY_AT + 4 * i, 0x2000 + i);
    }
    w(&mut g, FRAME + ARG_B, c.b);
    w(&mut g, FRAME + ARG_A, c.a);
    w(&mut g, FRAME - super::MODEL_ONE, MODEL_ONE);
    w(&mut g, FRAME - super::MODEL_TWO, MODEL_TWO);
    w(&mut g, FRAME - SAVED_EDI, EDI_SAVE);
    w(&mut g, FRAME - super::GRAPH, GRAPH);
    for (i, (at, nodes_at)) in [(MODEL_ONE, NODES_ONE), (MODEL_TWO, NODES_TWO)]
        .into_iter()
        .enumerate()
    {
        w(&mut g, at + MODEL_COUNT, c.counts[i] as u32);
        w(&mut g, at + MODEL_NODES, nodes_at);
        for n in 0..c.counts[i].max(0) {
            for k in 0..24u32 {
                w(&mut g, nodes_at + 96 * n as u32 + 4 * k, rng.next() as u32);
            }
            let index = c.indices[i]
                .get(n as usize)
                .copied()
                .flatten()
                .unwrap_or(u16::MAX);
            let table = if i == 0 {
                SLOT + INDICES[0]
            } else {
                SLOT + INDICES[1]
            };
            let bytes = index.to_le_bytes();
            g.write_bytes(table + 4 * n as u32, &bytes, 1).unwrap();
        }
    }
    // Garbage in the registers the step does not read whole. `esi` is
    // the first model: the step's caller leaves it there, and the first
    // loop reads it before the fill's exit can set it.
    let mut cpu = CpuAbi {
        eax: 0xabcd_0000 | (c.nodes as u16 as u32),
        ecx: rng.next() as u32,
        edx: rng.next() as u32,
        ebx: SLOT,
        esp: STACK,
        ebp: FRAME,
        esi: MODEL_ONE,
        edi: rng.next() as u32,
        eflags: 0x246 | if c.df { 0x400 } else { 0 },
        eflags_defined: 0x8d5,
        mxcsr: 0x1fa0,
        ..CpuAbi::default()
    };
    for (i, reg) in cpu.xmm.iter_mut().enumerate() {
        let lane = 0x3f80_0000u32 + i as u32;
        *reg = lane.to_le_bytes().repeat(4).try_into().unwrap();
    }
    (cpu, g)
}

fn ram(g: &Guest) -> &[u8] {
    let start = g.mappings()[0].physical_start as usize;
    &g.physical_ram()[start..start + RAM as usize]
}

fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x != y)
}

/// The step with the kernel on and off: both must agree on every byte
/// and every register, and the kernel must not decline.
fn equal(c: &Case) {
    if c.df {
        // The guest's copies run downward and write below each record;
        // the kernel leaves such a step to the port's loops.
        declines(c, |_, _| {}, Decline::Direction);
        return;
    }
    let _guard = SWITCH.lock().unwrap();
    set_enabled(false);
    let (mut loop_cpu, mut loop_run) = machine(c);
    let loop_transfer =
        crate::sim_physics::first_person::palette(&mut loop_cpu, &mut loop_run).unwrap();
    let was = stats().native;
    set_enabled(true);
    let (mut cpu, mut g) = machine(c);
    let transfer = crate::sim_physics::first_person::palette(&mut cpu, &mut g).unwrap();
    set_enabled(false);
    assert_eq!(
        stats().native,
        was + 1,
        "nodes {} seed {:#x} declined",
        c.nodes,
        c.seed
    );
    assert_eq!(transfer, loop_transfer);
    assert_eq!(cpu, loop_cpu, "nodes {} seed {:#x}", c.nodes, c.seed);
    assert_eq!(
        first_difference(ram(&loop_run), ram(&g)),
        None,
        "nodes {} seed {:#x}: RAM differs",
        c.nodes,
        c.seed
    );
}

#[test]
fn the_kernel_equals_the_loops() {
    for nodes in [-3i32, -1, 0, 1, 2, 5, 17] {
        for counts in [[0i32, 0], [1, 0], [0, 2], [3, 3], [2, 4]] {
            for seed in 1..4u64 {
                equal(&case(
                    seed * 0x9e37 + u64::from(nodes as u16) + counts[0] as u64,
                    nodes,
                    counts,
                ));
            }
        }
    }
}

/// A declined step leaves the port exactly as it is with the kernel off.
fn declines(c: &Case, edit: impl Fn(&mut CpuAbi, &mut Guest), why: Decline) {
    let _guard = SWITCH.lock().unwrap();
    set_enabled(false);
    let (mut loop_cpu, mut loop_run) = machine(c);
    edit(&mut loop_cpu, &mut loop_run);
    let loop_transfer =
        crate::sim_physics::first_person::palette(&mut loop_cpu, &mut loop_run).unwrap();
    set_enabled(true);
    let (mut cpu, mut g) = machine(c);
    edit(&mut cpu, &mut g);
    assert_eq!(plan(&cpu, &g).err(), Some(why), "{why:?}");
    let transfer = crate::sim_physics::first_person::palette(&mut cpu, &mut g).unwrap();
    set_enabled(false);
    assert_eq!(cpu, loop_cpu, "{why:?}");
    assert_eq!(first_difference(ram(&loop_run), ram(&g)), None, "{why:?}");
    assert_eq!(transfer, loop_transfer, "{why:?}");
}

#[test]
fn steps_off_the_gather_decline_untouched() {
    let c = case(5, 3, [2, 2]);
    // A fill count past the bound.
    declines(
        &Case {
            nodes: NODES_MAX as i32 + 1,
            ..case(7, 3, [1, 1])
        },
        |_, _| {},
        Decline::Count,
    );
    // A model node count past the bound.
    declines(
        &c,
        |_, g| {
            g.set_u32(MODEL_TWO + MODEL_COUNT, NODES_MAX as u32 + 1)
                .unwrap()
        },
        Decline::Count,
    );
    // An identity record pointer past the end of RAM. The fill count is
    // zero, so the port never dereferences it: only the kernel's eager
    // gather sees the fault and declines.
    let quiet = case(5, 0, [2, 2]);
    declines(
        &quiet,
        |_, g| g.set_u32(IDENTITY, RAM + 0x1000).unwrap(),
        Decline::Fault,
    );
    // The palette over the first model's nodes: the copies are then
    // order-sensitive through the direction flag.
    declines(
        &Case {
            a: (NODES_ONE - PALETTE_BASE) / 4096,
            ..c.clone()
        },
        |_, _| {},
        Decline::Alias,
    );
}

#[test]
fn the_switch_turns_the_kernel_off() {
    let _guard = SWITCH.lock().unwrap();
    let c = case(7, 4, [2, 1]);
    let (mut cpu, mut g) = machine(&c);
    set_enabled(false);
    let was = stats().native;
    crate::sim_physics::first_person::palette(&mut cpu, &mut g).unwrap();
    assert_eq!(stats().native, was);
    set_enabled(true);
    let (mut cpu, mut g) = machine(&c);
    crate::sim_physics::first_person::palette(&mut cpu, &mut g).unwrap();
    set_enabled(false);
    assert_eq!(stats().native, was + 1);
}
