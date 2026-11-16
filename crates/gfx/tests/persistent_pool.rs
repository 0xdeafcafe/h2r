//! Persistent vertex and index buffers are reused through size classes
//! once a submit has passed: streaming new snapshots through a small
//! budget creates buffers only until the pool is warm. Synthetic data;
//! skipped without a GPU.

use gfx::GfxContext;
use gfx::persistent::PersistentVertices;
use gfx::repack::RepackPlan;
use gfx::vertex::{PrimitiveType, VertexLayout, fvf};

#[test]
fn evicted_buffers_come_back_after_a_submit() {
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let layout = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let plan = RepackPlan::new(&layout, &[20]).streams.remove(0);
    let guest = vec![0x3Fu8; 20 * 64];
    // Room for about three frames of buffers (1280 vertex bytes in a 2 KiB
    // class, 12 index bytes in a 256-byte one).
    let mut p = PersistentVertices::new(128 << 10);
    let mut scratch = Vec::new();
    let mut id = 0;
    let mut frame = |p: &mut PersistentVertices| {
        for _ in 0..16 {
            id += 1;
            let (b, bound) = p.get(&ctx, id, &plan, &guest, 64, &mut scratch);
            assert_eq!(bound, 1280);
            assert!(b.size() >= bound);
            p.put_indices(&ctx, id, PrimitiveType::TriangleList, &[0, 1, 2]);
        }
        ctx.queue.submit([]);
        p.submitted();
    };
    for _ in 0..4 {
        frame(&mut p);
    }
    let before = gfx::churn::snapshot();
    for _ in 0..8 {
        frame(&mut p);
    }
    let c = gfx::churn::snapshot() - before;
    // Without the pool every one of the 256 would be created.
    assert_eq!(c.buffers_created + c.buffers_reused, 8 * 32, "{c:?}");
    assert!(c.buffers_created * 8 <= c.buffers_reused, "{c:?}");
    assert!(p.stats().held_bytes <= 128 << 10);
}
