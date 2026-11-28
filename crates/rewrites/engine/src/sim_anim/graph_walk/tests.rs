//! The plan and the body against the register-level [`reference`]
//! on synthetic guest memory (no image). The reference runs the exact
//! `sim_native` pose and composition bodies, which
//! `tests/sim_native_differential.rs` holds equal to the lifted guest; the
//! lifted `1dd1c0` itself is `tests/sim_anim_graph_differential.rs`.
//! Every case must leave the same RAM, byte for byte, and the same CPU.
use super::*;
use runtime::Guest;
use std::sync::Mutex;
use runtime::guest::Permissions;

const RAM: u32 = 0x0080_0000;
const STACK: u32 = 0x007f_0000;
const RETURN: u32 = 0x001c_bd73;
const GRAPH: u32 = 0x0060_0000;
const NODE_ARRAY: u32 = 0x0060_1000;
const RECORDS: u32 = 0x0061_0000;
const MATRICES: u32 = 0x0062_0000;
const ROOT: u32 = 0x0063_0000;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// A magnitude in `[0.25, 4)`, either sign.
    fn value(&mut self) -> u32 {
        let r = self.next();
        let sign = ((r >> 40) as u32 & 1) << 31;
        sign | (125 + (r % 4) as u32) << 23 | (r >> 8) as u32 & 0x7f_ffff
    }
    /// A magnitude in `[0.5, 2)`, positive.
    fn scale(&mut self) -> u32 {
        (126 + (self.next() % 2) as u32) << 23 | (self.next() as u32 & 0x7f_ffff)
    }
}

/// One synthetic graph at the walk's entry.
#[derive(Clone)]
struct Case {
    /// The graph's node count word.
    count: i32,
    nodes: Vec<GraphNode>,
    /// `[0x55ebb4]` bit 0 already set.
    initialised: bool,
    mxcsr: u32,
    seed: u64,
}

const NONE: i16 = -1;

/// A random tree turned into child and sibling links. Some nodes are
/// left unlinked (never visited), node 0 sometimes has a sibling (visited
/// twice), and a few parent fields name a later node or the node itself.
fn case(count: usize, seed: u64) -> Case {
    let mut rng = Rng(seed);
    let mut nodes = vec![
        GraphNode {
            first_child: NONE,
            next_sibling: NONE,
            parent: NONE,
        };
        count
    ];
    let mut last_child: Vec<Option<usize>> = vec![None; count];
    for i in 1..count {
        if rng.next() % 11 == 0 {
            continue;
        }
        let p = (rng.next() % i as u64) as usize;
        match last_child[p] {
            None => nodes[p].first_child = i as i16,
            Some(prev) => nodes[prev].next_sibling = i as i16,
        }
        last_child[p] = Some(i);
        nodes[i].parent = match rng.next() % 16 {
            0 => ((i + 1) % count) as i16,
            1 => i as i16,
            _ => p as i16,
        };
    }
    if count > 2 && seed % 3 == 0 {
        nodes[0].next_sibling = (count - 1) as i16;
        if nodes[count - 1].parent == NONE {
            nodes[count - 1].parent = 0;
        }
    }
    Case {
        count: count as i32,
        nodes,
        initialised: seed % 2 == 0,
        mxcsr: 0x1fa0,
        seed,
    }
}

fn machine(c: &Case) -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(0, RAM, Permissions::RW).unwrap();
    let mut rng = Rng(c.seed ^ 0x5bd1_e995);
    let w = |g: &mut Guest, a: u32, v: u32| g.set_u32(a, v).unwrap();
    w(&mut g, ONE, 1.0f32.to_bits());
    w(&mut g, SMALL, 1e-4f32.to_bits());
    for (i, v) in [0x8000_0000u32, 0, 0x8000_0000, 0].into_iter().enumerate() {
        w(&mut g, SIGNS + 4 * i as u32, v);
    }
    w(
        &mut g,
        POSE_INIT_FLAGS,
        if c.initialised { 0x101 } else { 0x100 },
    );
    w(
        &mut g,
        POSE_NORM_THRESHOLD,
        if c.initialised { 2e-4f32.to_bits() } else { 0 },
    );

    w(&mut g, STACK, RETURN);
    for (k, v) in [GRAPH, MATRICES, RECORDS, ROOT].into_iter().enumerate() {
        w(&mut g, STACK + 4 + 4 * k as u32, v);
    }
    w(&mut g, GRAPH + NODE_COUNT, c.count as u32);
    w(&mut g, GRAPH + NODES, NODE_ARRAY);
    // Old frame contents, so the plan's dead-stack bytes are visible.
    for a in (STACK - 0x500..STACK).step_by(4) {
        let v = rng.next() as u32;
        w(&mut g, a, v);
    }
    let matrix = |g: &mut Guest, rng: &mut Rng, at: u32| {
        let s = rng.scale();
        g.set_u32(at, s).unwrap();
        for k in 1..13 {
            let v = rng.value();
            g.set_u32(at + 4 * k, v).unwrap();
        }
    };
    matrix(&mut g, &mut rng, ROOT);
    for (i, n) in c.nodes.iter().enumerate() {
        let i = i as u32;
        let def = NODE_ARRAY + 0x20 * i;
        g.set_u16(def + FIRST_CHILD, n.first_child as u16).unwrap();
        g.set_u16(def + NEXT_SIBLING, n.next_sibling as u16)
            .unwrap();
        g.set_u16(def + PARENT, n.parent as u16).unwrap();
        let record = RECORDS + 0x20 * i;
        // Every eighth rotation is zero: the identity branch.
        let zero = rng.next() % 8 == 0;
        for k in 0..4 {
            let q = if zero { 0 } else { rng.value() };
            w(&mut g, record + 4 * k, q);
        }
        for k in 4..7 {
            let t = rng.value();
            w(&mut g, record + 4 * k, t);
        }
        let s = rng.scale();
        w(&mut g, record + 28, s);
        matrix(&mut g, &mut rng, MATRICES + 0x34 * i);
    }
    let mut cpu = CpuAbi {
        eax: rng.next() as u32,
        ecx: rng.next() as u32,
        edx: rng.next() as u32,
        ebx: rng.next() as u32,
        esp: STACK,
        ebp: rng.next() as u32,
        esi: rng.next() as u32,
        edi: rng.next() as u32,
        eip: ENTRY,
        eflags: 0x246,
        eflags_defined: 0x8d5,
        mxcsr: c.mxcsr,
        ..CpuAbi::default()
    };
    for reg in cpu.xmm.iter_mut() {
        *reg = unlanes(std::array::from_fn(|_| rng.value()));
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

/// The reference and the body from the same state; both must agree, and
/// the body must not decline.
fn equal(c: &Case) {
    for simd in [true, false] {
        sim_core::lanes::with(simd, || {
            let (mut ref_cpu, mut ref_g) = machine(c);
            let want = reference(&mut ref_cpu, &mut ref_g).unwrap();
            let (mut cpu, mut g) = machine(c);
            let label = format!("count {} seed {:#x} simd {simd}", c.count, c.seed);
            let got = run(&mut cpu, &mut g)
                .unwrap()
                .unwrap_or_else(|why| panic!("{label}: declined: {why:?}"));
            assert_eq!(got, want, "{label}");
            if let Some(at) = first_difference(ram(&ref_g), ram(&g)) {
                let (entry, sim) = machine(c);
                let o = plan(&entry, &sim).unwrap();
                panic!(
                    "{label}: RAM differs at {at:#x} ({})",
                    o.describe(at as u32)
                );
            }
            assert_eq!(cpu, ref_cpu, "{label}");
        });
    }
}

#[test]
fn the_body_equals_the_reference() {
    let _s = serial();
    for count in [1usize, 2, 3, 9, 31, 64, 90, 200] {
        for seed in 1..7u64 {
            equal(&case(count, seed * 0x9e37_79b9 + count as u64));
        }
    }
}

#[test]
fn an_empty_or_negative_count_saves_two_registers_and_returns() {
    let _s = serial();
    for count in [0, -1, i32::MIN] {
        let c = Case {
            count,
            ..case(4, 11)
        };
        equal(&c);
        let (before, _) = machine(&c);
        let (mut cpu, mut g) = machine(&c);
        run(&mut cpu, &mut g).unwrap().unwrap();
        assert_eq!(
            (cpu.eax, cpu.ecx, cpu.edx),
            (GRAPH, count as u32, before.edx)
        );
        assert_eq!(cpu.xmm, before.xmm);
        assert_eq!(
            g.u32(MATRICES).unwrap(),
            machine(&c).1.u32(MATRICES).unwrap()
        );
    }
}

/// 0 -> 1 -> (3, 2), 2 -> 4; node 5 is linked from nothing.
fn fixed(seed: u64) -> Case {
    let mut c = case(6, seed);
    let link = |first_child, next_sibling, parent| GraphNode {
        first_child,
        next_sibling,
        parent,
    };
    c.nodes = vec![
        link(1, NONE, NONE),
        link(3, NONE, 0),
        link(4, NONE, 1),
        link(NONE, 2, 1),
        link(NONE, NONE, 2),
        link(NONE, NONE, 0),
    ];
    c
}

#[test]
fn the_queue_order_and_unvisited_nodes() {
    let _s = serial();
    let c = fixed(2);
    equal(&c);
    let (cpu, g) = machine(&c);
    let o = plan(&cpu, &g).unwrap();
    assert_eq!(o.posed, 5);
    let (mut cpu, mut g) = machine(&c);
    let untouched = g.load::<0x34>(MATRICES + 0x34 * 5).unwrap();
    run(&mut cpu, &mut g).unwrap().unwrap();
    for (i, want) in [0u32, 1, 3, 2, 4].into_iter().enumerate() {
        assert_eq!(g.u32(STACK - QUEUE + 4 * i as u32).unwrap(), want);
    }
    assert_eq!(g.u32(STACK - LENGTH).unwrap(), 5);
    assert_eq!(g.load::<0x34>(MATRICES + 0x34 * 5).unwrap(), untouched);
    // The last node, 4, has no links: ECX is its parent's matrix and EDX
    // the scratch.
    assert_eq!(
        (cpu.eax, cpu.ecx, cpu.edx),
        (5, MATRICES + 0x34 * 2, STACK - SCRATCH)
    );
}

#[test]
fn a_node_queued_twice_is_walked_twice() {
    let _s = serial();
    let mut c = case(3, 5);
    c.nodes = vec![
        GraphNode {
            first_child: 1,
            next_sibling: 1,
            parent: NONE,
        },
        GraphNode {
            first_child: NONE,
            next_sibling: NONE,
            parent: 1,
        },
        GraphNode {
            first_child: NONE,
            next_sibling: NONE,
            parent: 0,
        },
    ];
    equal(&c);
}

#[test]
fn an_uninitialised_cutoff_is_set_by_the_walk() {
    let _s = serial();
    let mut c = case(12, 4);
    c.initialised = false;
    equal(&c);
    let (mut cpu, mut g) = machine(&c);
    run(&mut cpu, &mut g).unwrap().unwrap();
    assert_eq!(g.u32(POSE_INIT_FLAGS).unwrap(), 0x101);
    assert_eq!(g.u32(POSE_NORM_THRESHOLD).unwrap(), 1e-4f32.to_bits());
}

/// A declined call writes nothing and leaves the CPU as it was.
fn declines(c: &Case, edit: impl Fn(&mut CpuAbi, &mut Guest), why: Decline) {
    let (mut before_cpu, mut before) = machine(c);
    edit(&mut before_cpu, &mut before);
    let (mut cpu, mut g) = machine(c);
    edit(&mut cpu, &mut g);
    assert_eq!(plan(&cpu, &g).err(), Some(why), "{why:?}");
    assert_eq!(run(&mut cpu, &mut g).unwrap(), Err(why), "{why:?}");
    assert_eq!(cpu, before_cpu, "{why:?}");
    assert_eq!(first_difference(ram(&before), ram(&g)), None, "{why:?}");
}

#[test]
fn calls_outside_the_fast_path_decline_untouched() {
    let _s = serial();
    let c = case(10, 6);
    let def = |i: u32, field: u32| NODE_ARRAY + 0x20 * i + field;
    // PE not yet sticky; rounding toward zero.
    for mxcsr in [0x1f80, 0x7fa0] {
        declines(&Case { mxcsr, ..c.clone() }, |_, _| {}, Decline::Float);
    }
    // A denormal in node 0's quaternion: node 0 is always visited.
    declines(
        &c,
        |_, g| g.set_u32(RECORDS + 4, 1).unwrap(),
        Decline::Float,
    );
    // An infinity in a dead lane of XMM1, which the first pose adds.
    declines(
        &c,
        |cpu, _| cpu.xmm[1][12..].copy_from_slice(&0x7f80_0000u32.to_le_bytes()),
        Decline::Float,
    );
    // A link past the nodes, a negative one other than -1, and a cycle
    // that fills the queue.
    declines(
        &c,
        move |_, g| g.set_u16(def(0, FIRST_CHILD), 10).unwrap(),
        Decline::Queue,
    );
    declines(
        &c,
        move |_, g| g.set_u16(def(0, FIRST_CHILD), 0xfffe).unwrap(),
        Decline::Queue,
    );
    declines(
        &c,
        move |_, g| g.set_u16(def(0, NEXT_SIBLING), 0).unwrap(),
        Decline::Queue,
    );
    // A visited node's parent past the nodes, and a negative one.
    let mut chain = c.clone();
    chain.nodes = (0..10)
        .map(|i| GraphNode {
            first_child: if i < 9 { i + 1 } else { NONE },
            next_sibling: NONE,
            parent: i - 1,
        })
        .collect();
    for parent in [10u16, 0xffff] {
        declines(
            &chain,
            move |_, g| g.set_u16(def(4, PARENT), parent).unwrap(),
            Decline::Parent,
        );
    }
    // Node counts the plan does not take.
    declines(
        &c,
        |_, g| g.set_u32(GRAPH + NODE_COUNT, MAX_NODES + 1).unwrap(),
        Decline::Count,
    );
    // The matrices over the records, over the frame, and over the root;
    // records past the end of RAM.
    declines(
        &c,
        |_, g| g.set_u32(STACK + 8, RECORDS + 0x40).unwrap(),
        Decline::Alias,
    );
    declines(
        &c,
        |_, g| g.set_u32(STACK + 8, STACK - 0x200).unwrap(),
        Decline::Alias,
    );
    declines(
        &c,
        |_, g| g.set_u32(STACK + 16, MATRICES + 0x34).unwrap(),
        Decline::Alias,
    );
    declines(
        &c,
        |_, g| g.set_u32(STACK + 12, RAM - 0x20).unwrap(),
        Decline::Alias,
    );
}

#[test]
fn a_full_queue_is_taken_and_one_more_declines() {
    let _s = serial();
    // Node 0 and its children in one sibling chain: one node more is one
    // queue entry more, at a depth the scales keep on the fast path.
    let chain = |count: usize| {
        let mut c = case(count, 7);
        c.nodes = (0..count as i16)
            .map(|i| GraphNode {
                first_child: if i == 0 { 1 } else { NONE },
                next_sibling: if i > 0 && (i as usize) < count - 1 {
                    i + 1
                } else {
                    NONE
                },
                parent: if i == 0 { NONE } else { 0 },
            })
            .collect();
        c
    };
    equal(&chain(::sim_anim::graph_walk::QUEUE));
    declines(
        &chain(::sim_anim::graph_walk::QUEUE + 1),
        |_, _| {},
        Decline::Queue,
    );
}

/// `stats()` is process-wide: every test that makes the body answer
/// holds this and runs one at a time.
static BODY_TESTS: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    BODY_TESTS.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn eflags_compare_only_where_both_sides_define_them() {
    let c = case(5, 13);
    let (mut cpu, mut g) = machine(&c);
    let o = plan(&cpu, &g).unwrap();
    reference(&mut cpu, &mut g).unwrap();
    let mut undefined = cpu;
    undefined.eflags ^= 0x40;
    undefined.eflags_defined &= !0x40;
    assert_eq!(compare(&o, &undefined, &g), None);
    let mut wrong = cpu;
    wrong.eflags ^= 0x40;
    assert_eq!(compare(&o, &wrong, &g).unwrap().what, "eflags");
}

#[test]
fn bytes_are_named_by_region() {
    let (cpu, g) = machine(&case(4, 15));
    let o = plan(&cpu, &g).unwrap();
    for (addr, name) in [
        (MATRICES + 0x34 * 3 + 0x30, "matrix 3 +0x30"),
        (POSE_INIT_FLAGS + 1, "pose cutoff flag"),
        (POSE_NORM_THRESHOLD, "pose cutoff"),
        (STACK - 4, "stack entry-0x4 (queue entry 254)"),
        (STACK - LENGTH, "stack entry-0x434 (queue length)"),
        (STACK - 0x438, "stack entry-0x438 (saved ebx)"),
        (STACK - 0x444, "stack entry-0x444 (saved edi)"),
        (STACK - 0x454, "stack entry-0x454 (pose saved ebp)"),
        (STACK + 4, "0x007f0004"),
    ] {
        assert_eq!(o.describe(addr), name);
    }
}
