//! `engine::sim_anim::graph_walk` against the lifted guest: `1dd1c0` and
//! its two leaves (`1421f0`, `142a60`) run on the IR interpreter with
//! nothing stubbed. Private XBE as the oracle (`H2_XBE`, skipped without
//! it), synthetic graphs (docs/engine/skeleton-walk.md, "The graph walk
//! `1dd1c0`").
//!
//! Three arms from the same entry state, each compared with the guest in
//! full, the dead frame included: the body (`run`, host lanes and the
//! scalar loop), the register-level `reference`, and the arm's plan,
//! which `compare` must find equal to the guest's own result. The first
//! difference is reported by name. A deliberate mismatch, one byte of the
//! guest's result changed, must be named by `compare`.
#![cfg(feature = "differential")]

use engine::points;
use engine::sim_anim::graph_walk as walk;
use ir::interp::{Cpu, FlatMemory};
use lifted::Code;
use lifted::cpu::{diff, from_interp, to_interp};
use lifted::image::{Image, Rng};
use runtime::guest::Permissions;
use runtime::{CpuAbi, Guest, TransferAbi};

const RAM: usize = 8 << 20;
const STACK: u32 = 0x007f_0000;
const EXIT: u32 = lift::harness::HARNESS_EXIT;
const GRAPH: u32 = 0x0060_0000;
const NODE_ARRAY: u32 = 0x0060_1000;
const RECORDS: u32 = 0x0061_0000;
const MATRICES: u32 = 0x0062_0000;
const ROOT: u32 = 0x0063_0000;
const NONE: i16 = -1;

fn word(m: &mut FlatMemory, a: u32, v: u32) {
    assert!(m.load(a, &v.to_le_bytes()), "fixture write at {a:#x}");
}

fn half(m: &mut FlatMemory, a: u32, v: i16) {
    assert!(m.load(a, &v.to_le_bytes()), "fixture write at {a:#x}");
}

/// A magnitude in `[0.25, 4)`, either sign: on the fast path, with sums
/// and products over a few levels that stay on it.
fn value(rng: &mut Rng) -> u32 {
    let r = rng.next_u64();
    let sign = ((r >> 40) as u32 & 1) << 31;
    sign | (125 + (r % 4) as u32) << 23 | (r >> 8) as u32 & 0x7f_ffff
}

fn scale(rng: &mut Rng) -> u32 {
    (126 + rng.below(2)) << 23 | (rng.u32() & 0x7f_ffff)
}

/// One graph: `(first child, next sibling, parent)` per node.
struct Graph {
    count: i32,
    links: Vec<(i16, i16, i16)>,
    initialised: bool,
}

/// A random tree as links; every fifth node unlinked.
fn tree(rng: &mut Rng, count: usize) -> Vec<(i16, i16, i16)> {
    let mut links = vec![(NONE, NONE, NONE); count];
    let mut last: Vec<Option<usize>> = vec![None; count];
    for i in 1..count {
        if rng.below(5) == 0 {
            continue;
        }
        let p = rng.below(i as u32) as usize;
        match last[p] {
            None => links[p].0 = i as i16,
            Some(prev) => links[prev].1 = i as i16,
        }
        last[p] = Some(i);
        links[i].2 = p as i16;
    }
    links
}

fn graph(case: u32, rng: &mut Rng) -> Graph {
    let (count, initialised) = match case % 8 {
        0 => (1, true),
        1 => (5, true),
        2 => (12, false),
        3 => (40, true),
        4 => (0, true),
        5 => (3, true),
        6 => (9, true),
        _ => (24, false),
    };
    let mut links = tree(rng, count);
    match case % 8 {
        // Node 1 queued twice (child and node 0's sibling), naming itself.
        5 => links = vec![(1, 1, NONE), (NONE, NONE, 1), (NONE, NONE, 0)],
        // Parents named ahead of the queue and a node naming itself.
        6 => {
            for (i, l) in links.iter_mut().enumerate().skip(1) {
                if l.2 != NONE && i % 3 == 0 {
                    l.2 = if i % 2 == 0 {
                        i as i16
                    } else {
                        ((i + 1) % count) as i16
                    };
                }
            }
        }
        _ => {}
    }
    Graph {
        count: count as i32,
        links,
        initialised,
    }
}

fn fixture(m: &mut FlatMemory, cpu: &mut CpuAbi, case: u32) {
    let mut rng = Rng(0x1dd1_c000 + u64::from(case));
    let g = graph(case, &mut rng);
    word(m, STACK, EXIT);
    for (k, v) in [GRAPH, MATRICES, RECORDS, ROOT].into_iter().enumerate() {
        word(m, STACK + 4 + 4 * k as u32, v);
    }
    // Old frame contents, so every dead-stack byte the call writes shows.
    for a in (STACK - 0x500..STACK).step_by(4) {
        word(m, a, rng.u32());
    }
    word(m, GRAPH + walk::NODE_COUNT, g.count as u32);
    word(m, GRAPH + walk::NODES, NODE_ARRAY);
    let flags = 0x0000_0100 | u32::from(g.initialised);
    word(m, engine::sim_native::pose::POSE_INIT_FLAGS, flags);
    word(
        m,
        engine::sim_native::pose::POSE_NORM_THRESHOLD,
        2.5e-4f32.to_bits(),
    );
    let mut matrix = |m: &mut FlatMemory, at: u32| {
        word(m, at, scale(&mut rng));
        for k in 1..13 {
            word(m, at + 4 * k, value(&mut rng));
        }
    };
    matrix(m, ROOT);
    for (i, &(child, sibling, parent)) in g.links.iter().enumerate() {
        let i = i as u32;
        let def = NODE_ARRAY + 0x20 * i;
        half(m, def + walk::FIRST_CHILD, child);
        half(m, def + walk::NEXT_SIBLING, sibling);
        half(m, def + walk::PARENT, parent);
        matrix(m, MATRICES + 0x34 * i);
    }
    for i in 0..g.links.len() as u32 {
        let record = RECORDS + 0x20 * i;
        let zero = rng.below(6) == 0;
        for k in 0..4 {
            let q = if zero { 0 } else { value(&mut rng) };
            word(m, record + 4 * k, q);
        }
        for k in 4..7 {
            word(m, record + 4 * k, value(&mut rng));
        }
        word(m, record + 28, scale(&mut rng));
    }
    *cpu = CpuAbi {
        eax: rng.u32(),
        ecx: rng.u32(),
        edx: rng.u32(),
        ebx: rng.u32(),
        esp: STACK,
        ebp: rng.u32(),
        esi: rng.u32(),
        edi: rng.u32(),
        eip: points::ANIM_GRAPH_MATRICES.base,
        eflags: 0x202 | (rng.u32() & 0x8d5),
        eflags_defined: 0x8d5,
        mxcsr: 0x1fa0,
        ..CpuAbi::default()
    };
    cpu.x87.control = 0x037f;
    for xmm in &mut cpu.xmm {
        for lane in xmm.chunks_exact_mut(4) {
            lane.copy_from_slice(&value(&mut rng).to_le_bytes());
        }
    }
}

/// The image's sections from zero, cut to the fixtures' RAM.
fn image() -> Option<(&'static Image, FlatMemory)> {
    let image = Image::get()?;
    let mut ram = image.ram.clone();
    ram.bytes.truncate(RAM);
    Some((image, ram))
}

fn guest_of(m: &FlatMemory) -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
    g.write_bytes(0, &m.bytes, 1).unwrap();
    g
}

fn ram(g: &Guest) -> &[u8] {
    let start = g.mappings()[0].physical_start as usize;
    &g.physical_ram()[start..start + RAM]
}

/// One arm's result against the guest's: the transfer, every register
/// (named), and every byte (named by the plan).
fn same(
    label: &str,
    plan: &walk::Outcome,
    transfer: TransferAbi,
    cpu: CpuAbi,
    g: &Guest,
    guest_cpu: &Cpu,
    guest: &FlatMemory,
) {
    assert_eq!(transfer, TransferAbi::ret(EXIT), "{label}");
    let regs = to_interp(&CpuAbi { eip: EXIT, ..cpu });
    let differ = diff(&regs, guest_cpu);
    assert!(differ.is_empty(), "{label}: registers {differ:?}");
    if let Some(at) = ram(g).iter().zip(&guest.bytes).position(|(a, b)| a != b) {
        panic!(
            "{label}: first different byte {at:#x} ({})",
            plan.describe(at as u32)
        );
    }
}

#[test]
fn graph_matrices_matches_the_guest() {
    let Some((image, template)) = image() else {
        return;
    };
    let mut code = image.original();
    for f in [
        points::ANIM_GRAPH_MATRICES.base,
        points::OBJECT_POSE_MATRIX.base,
        points::OBJECT_TRANSFORM_COMPOSE.base,
    ] {
        code.prime(f);
        assert!(code.insn(f).is_some(), "{f:#x}");
    }
    let before = walk::stats().native;
    for case in 0..24 {
        let mut memory = template.clone();
        let mut entry = CpuAbi::default();
        fixture(&mut memory, &mut entry, case);
        let plan = walk::plan(&entry, &guest_of(&memory))
            .unwrap_or_else(|why| panic!("case {case}: the plan declined: {why:?}"));

        // The guest.
        let mut guest_cpu = to_interp(&entry);
        let mut guest = memory.clone();
        lifted::code::run_until(&mut code, &mut guest_cpu, &mut guest, 5_000_000, |c| {
            c.eip == EXIT
        })
        .unwrap_or_else(|e| panic!("case {case}: {e}"));

        // The body, on host lanes and on the scalar loop.
        for simd in [true, false] {
            sim_core::lanes::with(simd, || {
                let mut g = guest_of(&memory);
                let mut cpu = entry;
                let t = walk::run(&mut cpu, &mut g)
                    .unwrap()
                    .unwrap_or_else(|why| panic!("case {case}: declined: {why:?}"));
                same(
                    &format!("body case {case} simd {simd}"),
                    &plan,
                    t,
                    cpu,
                    &g,
                    &guest_cpu,
                    &guest,
                );
            });
        }
        // The register-level reference.
        let mut g = guest_of(&memory);
        let mut cpu = entry;
        let t = walk::reference(&mut cpu, &mut g).unwrap();
        same(
            &format!("reference case {case}"),
            &plan,
            t,
            cpu,
            &g,
            &guest_cpu,
            &guest,
        );

        // The arm: the plan against the guest's own result.
        let after = guest_of(&guest);
        let returned = from_interp(&guest_cpu);
        assert_eq!(
            walk::compare(&plan, &returned, &after),
            None,
            "arm case {case}"
        );
    }
    assert!(walk::stats().native > before);
}

/// The arm must see a guest result that differs from its plan, and name
/// the first byte or register that does.
#[test]
fn the_arm_names_a_deliberate_mismatch() {
    let Some((image, template)) = image() else {
        return;
    };
    let mut code = image.original();
    code.prime(points::ANIM_GRAPH_MATRICES.base);
    let mut memory = template.clone();
    let mut entry = CpuAbi::default();
    fixture(&mut memory, &mut entry, 3);
    let plan = walk::plan(&entry, &guest_of(&memory)).unwrap();
    let mut guest_cpu = to_interp(&entry);
    let mut guest = memory.clone();
    lifted::code::run_until(&mut code, &mut guest_cpu, &mut guest, 5_000_000, |c| {
        c.eip == EXIT
    })
    .unwrap();

    let mut changed = guest.clone();
    // Node 0 is always visited, and its matrix is the first written.
    let at = MATRICES + 0x2c;
    changed.bytes[at as usize] ^= 0x01;
    let m = walk::compare(&plan, &from_interp(&guest_cpu), &guest_of(&changed))
        .expect("a changed byte");
    assert_eq!(m.what, "matrix 0 +0x2c");

    let mut returned = from_interp(&guest_cpu);
    returned.ecx ^= 0x10;
    let m = walk::compare(&plan, &returned, &guest_of(&guest)).expect("a changed register");
    assert_eq!(m.what, "ecx");
}
