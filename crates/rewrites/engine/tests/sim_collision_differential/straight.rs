//! C and D return from the unsplit path with no exclusions, including
//! the discarded arguments, local padding and upper XMM lanes.
use super::*;

#[test]
fn straight_exits_match_every_register_and_stack_byte() {
    with_oracle("straight collision vector exits", |oracle, image| {
        let mut b = Builder::default();
        b.planes
            .extend([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.25]]);
        b.nodes.extend([
            (0, synth::node(1), synth::SOLID),
            (1, synth::leaf(0), synth::leaf(1)),
        ]);
        b.leaves.extend([(0, 0, 0), (1, 0, 0)]);
        let mut rng = Rng(0xd9e1_c47b);
        for n in 0..360 {
            let wrapper = n % 2 == 0;
            let point = if wrapper {
                VECTOR.point
            } else {
                CONTEXT_ENTRY.point
            };
            let e = Entry {
                point,
                eax: u32::MAX,
                dead: (0, 0),
            };
            let mut c = entry_cpu(&mut rng, e.point.base);
            (c.mxcsr, c.x87.control, c.x87.tag) = (0x1fa0, 0x027f, 0xffff);
            let mut m = fresh(image);
            write_bsp(&mut m, &b);
            for offset in (4..=96).step_by(4) {
                m.word(STACK - offset, rng.next());
            }
            let count = [0, 23, 255, 256, 309][rng.below(5) as usize];
            record_and_mask(&mut m, &mut rng);
            m.word(RECORD + layout::record::COUNT, count);
            m.float(RECORD + layout::record::FRACTION, 1.0);
            let x = if rng.chance(2) { -2.0 } else { 2.0 };
            let y = if rng.chance(2) { -1.0 } else { 1.0 };
            put_vector(&mut m, START, [x, y, 0.0]);
            put_vector(&mut m, DELTA, [0.375, -0.25, 2.0]);
            let flags = rng.next();
            let limit = [-1.0f32, -0.0, 0.0, 0.25, 1.0, 2.0][rng.below(6) as usize];
            let body: Native = if wrapper {
                c.eax = RECORD;
                c.ecx = flags;
                c.edx = BSP;
                c.xmm[0][..4].copy_from_slice(&limit.to_le_bytes());
                m.word(STACK + 4, rng.next());
                m.word(STACK + 8, MASK);
                m.word(STACK + 12, START);
                m.word(STACK + 16, DELTA);
                |c, m, w| adapter::straight::vector_test(c, m, w)
            } else {
                use layout::context as ctx;
                let node = [0, 1, 0xff80_0001, u32::MAX][rng.below(4) as usize];
                m.word(STACK + 4, CONTEXT);
                m.word(STACK + 8, node);
                m.float(STACK + 12, 0.0);
                m.float(STACK + 16, 1.0);
                m.word(CONTEXT + ctx::FLAGS, flags);
                m.word(CONTEXT + ctx::BSP, BSP);
                m.word(CONTEXT + ctx::BREAKABLE_COUNT, rng.next());
                m.word(CONTEXT + ctx::MASK, MASK);
                m.word(CONTEXT + ctx::START, START);
                m.word(CONTEXT + ctx::DELTA, DELTA);
                m.word(CONTEXT + ctx::RECORD, RECORD);
                m.word(CONTEXT + ctx::PREV_LEAF, u32::MAX);
                m.word(CONTEXT + ctx::PREV_KIND, rng.next() & !0xff);
                m.word(CONTEXT + ctx::PLANE, rng.next());
                |c, m, w| adapter::straight::vector_context(c, m, w)
            };
            assert!(check(
                oracle,
                &e,
                &c,
                &m,
                body,
                None,
                &format!("straight {n}")
            ));
        }
    });
}

#[test]
fn crossing_preserves_the_guest_entry() {
    with_oracle("crossing collision fallback", |oracle, image| {
        let mut b = Builder::default();
        b.planes.push([1.0, 0.0, 0.0, 0.0]);
        b.nodes.push((0, synth::leaf(0), synth::SOLID));
        b.leaves.push((0, 0, 0));
        let mut m = fresh(image);
        write_bsp(&mut m, &b);
        put_vector(&mut m, START, [-1.0, 0.0, 0.0]);
        put_vector(&mut m, DELTA, [2.0, 0.0, 0.0]);
        m.word(STACK + 4, 0);
        m.word(STACK + 8, 0);
        m.word(STACK + 12, START);
        m.word(STACK + 16, DELTA);
        let mut c = entry_cpu(&mut Rng(17), VECTOR.point.base);
        (c.mxcsr, c.x87.control, c.x87.tag) = (0x1fa0, 0x027f, 0xffff);
        c.eax = RECORD;
        c.ecx = 0;
        c.edx = BSP;
        c.xmm[0][..4].copy_from_slice(&1.0f32.to_le_bytes());
        assert!(!check(
            oracle,
            &VECTOR,
            &c,
            &m,
            |c, m, w| adapter::straight::vector_test(c, m, w),
            None,
            "crossing"
        ));
    });
}
