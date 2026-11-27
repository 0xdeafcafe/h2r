//! C and D crossing exits (walks that split and skip the matcher), run
//! ungated against the lifted entry. Every register outside the x87
//! condition codes and empty slots, and every stack byte including the
//! discarded frames, must match.
use super::*;

fn same_exit(case: &str, c: &CpuAbi, ret: TransferAbi, nm: &Ram<'_>, gc: &Cpu, gm: &Ram<'_>) {
    let mut got = ir_cpu(c);
    got.eip = ret.target_pc;
    assert_eq!(got.gpr, gc.gpr, "{case}: general registers");
    assert_eq!(got.eip, gc.eip, "{case}: EIP");
    let both = got.flags_valid & gc.flags_valid;
    assert_eq!(got.eflags & both, gc.eflags & both, "{case}: EFLAGS");
    assert_eq!(got.flags_valid & 0x8d5, gc.flags_valid & 0x8d5, "{case}: defined flags");
    assert_eq!(got.xmm, gc.xmm, "{case}: XMM");
    assert_eq!(got.mxcsr, gc.mxcsr, "{case}: MXCSR");
    assert_eq!(got.x87.control, gc.x87.control, "{case}: x87 control");
    assert_eq!(
        u32::from(got.x87.status) & STATUS_KEPT,
        u32::from(gc.x87.status) & STATUS_KEPT,
        "{case}: x87 status"
    );
    assert_eq!(got.x87.tag, gc.x87.tag, "{case}: x87 tag");
    for (slot, value) in gc.x87.st.iter().enumerate() {
        if gc.x87.tag >> (2 * slot) & 3 != 3 {
            assert_eq!(&got.x87.st[slot], value, "{case}: x87 slot {slot}");
        }
    }
    for p in nm.touched(gm, STACK, (STACK, STACK)) {
        assert_eq!(nm.get(p), gm.get(p), "{case}: byte at {p:#x}");
    }
}

/// `depth` planes on `x = 0 .. depth - 1`. Node `k` holds the next node
/// in front and a leaf behind; with `nested` the planes run backwards and
/// the next node is behind. A segment one way continues in each far
/// child, the other way recurses once per plane.
fn chain(depth: u32, nested: bool, flags: impl Fn(u32) -> u8, solid_end: bool) -> Builder {
    let mut b = Builder::default();
    for k in 0..depth {
        let x = if nested { depth - 1 - k } else { k };
        b.planes.push([1.0, 0.0, 0.0, x as f32]);
        let next = if k + 1 == depth {
            if solid_end { synth::SOLID } else { synth::leaf(depth) }
        } else {
            synth::node(k + 1)
        };
        let leaf = synth::leaf(k);
        b.nodes.push(if nested { (k as u16, next, leaf) } else { (k as u16, leaf, next) });
    }
    for k in 0..=depth {
        b.leaves.push((flags(k), 0, 0));
    }
    b
}

/// One case: run the ungated crossing path and the guest, compare, and
/// check the shadow arm agrees on the same state.
fn run_case(oracle: &mut Oracle<'_>, wrapper: bool, c: &CpuAbi, m: &Ram<'_>, case: &str) -> bool {
    static EPOCH: Epoch = Epoch::new();
    let world = CollisionWorld::new(&EPOCH);
    let (mut nc, mut nm) = (*c, m.clone());
    let got = if wrapper {
        adapter::crossing::vector_test(&mut nc, &mut nm, &world)
    } else {
        adapter::crossing::vector_context(&mut nc, &mut nm, &world)
    }
    .unwrap();
    let Some(ret) = got else {
        assert_eq!(nm.overlay, m.overlay, "{case}: a declined call wrote");
        assert_eq!(nc, *c, "{case}: a declined call moved registers");
        return false;
    };
    let (mut gm, mut gc) = (m.clone(), ir_cpu(c));
    oracle.run(&mut gc, &mut gm, c.eip);
    same_exit(case, &nc, ret, &nm, &gc, &gm);
    let mut entry = m.clone();
    let p = if wrapper {
        adapter::vector_parity::Prediction::gather(c, &mut entry, &world)
    } else {
        adapter::vector_parity::Prediction::gather_context(c, &mut entry, &world)
    }
    .unwrap();
    assert!(p.has_exit(), "{case}: the shadow arm saw no exit");
    let guest = lifted::cpu::from_interp(&gc);
    assert_eq!(p.compare(gc.gpr[0] as u8, &gm).unwrap(), None, "{case}: R");
    assert_eq!(p.compare_exit(&guest, &gm).unwrap(), None, "{case}: shadow exit");
    true
}

#[test]
fn crossing_exits_match_every_register_and_stack_byte() {
    with_oracle("crossing collision vector exits", |oracle, image| {
        let mut rng = Rng(0x5be0_cd19_137e_2179);
        let mut answered = [0u32; 2];
        const CASES: u32 = 480;
        for n in 0..CASES {
            let wrapper = n % 2 == 0;
            let depth = 1 + rng.below(6);
            let nested = rng.chance(2);
            let solid_end = rng.chance(3);
            // Two-sided leaves next to each other would run the matcher.
            let two_sided = rng.below(depth + 1);
            let b = chain(depth, nested, |k| if k == two_sided { 1 } else { 0 }, solid_end);
            let mut m = fresh(image);
            write_bsp(&mut m, &b);
            for offset in (4..=0x400).step_by(4) {
                m.word(STACK - offset, rng.next());
            }
            record_and_mask(&mut m, &mut rng);
            let (from, to) = (-0.5f32, depth as f32 - 0.5);
            let (x, dx) = if rng.chance(2) { (to, from - to) } else { (from, to - from) };
            put_vector(&mut m, START, [x, 0.25, -0.5]);
            put_vector(&mut m, DELTA, [dx, 0.125, 0.0]);
            // Query bits without 1 (enter solid) and 4 (two-sided) mostly
            // skip the matcher; the rest decline the exit.
            let flags = [0u32, 2, 4, 8, 0x10, 0x1c, 1, 5][rng.below(8) as usize] | (rng.next() & !0x1f);
            let mut c = entry_cpu(&mut rng, if wrapper { VECTOR.point.base } else { CONTEXT_ENTRY.point.base });
            c.mxcsr = 0x1fa0;
            c.x87.control = [0x027f, 0x007f, 0x037f][rng.below(3) as usize];
            let top = u16::try_from(rng.below(8)).unwrap();
            c.x87.status = (top << 11) | if rng.chance(2) { 0x20 } else { 0 };
            c.x87.tag = if rng.chance(2) { 0xffff } else { 0xffff & !(3 << (2 * top)) };
            if wrapper {
                c.eax = RECORD;
                c.ecx = flags;
                c.edx = BSP;
                let limit = [1.0f32, 2.0, 0.75, 0.5, 1.0e30][rng.below(5) as usize];
                c.xmm[0][..4].copy_from_slice(&limit.to_le_bytes());
                m.word(STACK + 4, [0x100, 8, 0][rng.below(3) as usize] | (rng.next() << 16));
                m.word(STACK + 8, if rng.chance(2) { MASK } else { 0 });
                m.word(STACK + 12, START);
                m.word(STACK + 16, DELTA);
            } else {
                use layout::context as ctx;
                let fraction = [1.0f32, 0.5, 0.75, 0.25][rng.below(4) as usize];
                m.float(RECORD + layout::record::FRACTION, fraction);
                let listed = [0u32, 5, 0xfe, 0xff, 0x100, 0x150][rng.below(6) as usize];
                m.word(RECORD + layout::record::COUNT, listed);
                m.word(CONTEXT + ctx::FLAGS, flags);
                m.word(CONTEXT + ctx::BSP, BSP);
                m.word(CONTEXT + ctx::BREAKABLE_COUNT, rng.next());
                m.word(CONTEXT + ctx::MASK, MASK);
                m.word(CONTEXT + ctx::START, START);
                m.word(CONTEXT + ctx::DELTA, DELTA);
                m.word(CONTEXT + ctx::RECORD, RECORD);
                m.word(CONTEXT + ctx::PREV_LEAF, if rng.chance(2) { u32::MAX } else { 0 });
                m.word(CONTEXT + ctx::PREV_KIND, [0u32, 1, 3][rng.below(3) as usize] | (rng.next() & !0xff));
                m.word(CONTEXT + ctx::PLANE, rng.next());
                let (t0, t1) = [(0.0f32, 1.0f32), (0.125, 0.875), (0.0, 0.5)][rng.below(3) as usize];
                m.word(STACK + 4, CONTEXT);
                m.word(STACK + 8, 0);
                m.float(STACK + 12, t0);
                m.float(STACK + 16, t1);
            }
            let case = format!("crossing {n}: depth {depth} nested {nested} flags {flags:#x}");
            if run_case(oracle, wrapper, &c, &m, &case) {
                answered[usize::from(!wrapper)] += 1;
            }
        }
        eprintln!("crossing exits: C {}, D {} of {CASES}", answered[0], answered[1]);
        assert!(
            answered.iter().all(|&k| k * 4 > CASES / 2),
            "too few crossing exits: {answered:?}"
        );
    });
}

/// A crossing that ends by running the matcher keeps the lifted body and
/// leaves the entry untouched.
#[test]
fn crossing_into_the_matcher_has_no_exit() {
    with_oracle("crossing collision matcher fallback", |oracle, image| {
        let b = chain(2, false, |_| 0, true);
        let mut m = fresh(image);
        write_bsp(&mut m, &b);
        record_and_mask(&mut m, &mut Rng(5));
        put_vector(&mut m, START, [-0.5, 0.0, 0.0]);
        put_vector(&mut m, DELTA, [2.0, 0.0, 0.0]);
        m.word(STACK + 4, 0);
        m.word(STACK + 8, 0);
        m.word(STACK + 12, START);
        m.word(STACK + 16, DELTA);
        let mut c = entry_cpu(&mut Rng(17), VECTOR.point.base);
        (c.mxcsr, c.x87.control, c.x87.tag) = (0x1fa0, 0x027f, 0xffff);
        c.eax = RECORD;
        c.ecx = 1;
        c.edx = BSP;
        c.xmm[0][..4].copy_from_slice(&1.0f32.to_le_bytes());
        assert!(!run_case(oracle, true, &c, &m, "matcher"));
    });
}
