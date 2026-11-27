//! The GPU transparent sort on a headless device (skips without an
//! adapter): its order is the CPU sort's, equal keys kept in input order.

use native_render::transparent_sort::{GpuSort, SortItem, form_keys};

/// The CPU reference: the guest comparator's order (class, then depth,
/// then tie), stable.
fn reference(items: &[SortItem]) -> Vec<u32> {
    let mut order: Vec<u32> = (0..items.len() as u32).collect();
    order.sort_by(|&a, &b| {
        let (a, b) = (&items[a as usize], &items[b as usize]);
        a.class
            .cmp(&b.class)
            .then(a.depth.partial_cmp(&b.depth).unwrap())
            .then(a.tie.cmp(&b.tie))
    });
    order
}

fn rng(state: &mut u64) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 32) as u32
}

fn item(class: i32, depth: f32, tie: i32) -> SortItem {
    SortItem { class, depth, tie, plane: false }
}

#[test]
fn ten_thousand_random_keys_with_many_ties_sort_as_the_cpu_does() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let gpu = GpuSort::new(&ctx.device);
    let mut state = 0x9e37_79b9_7f4a_7c15;
    let mut ties_seen = 0;
    // Ten lists of 1000 (the guest list's capacity is 0x400): 10,000 keys.
    for _ in 0..10 {
        let items: Vec<_> = (0..1000)
            .map(|_| {
                let depth = (rng(&mut state) % 40) as f32 * 0.5 - 8.0;
                let depth = if depth == 0.0 && rng(&mut state) & 1 == 1 { -0.0 } else { depth };
                item((rng(&mut state) % 4) as i32, depth, (rng(&mut state) % 3) as i32 - 1)
            })
            .collect();
        let got = gpu.sort(&ctx.device, &ctx.queue, &form_keys(&items).unwrap());
        assert_eq!(got.order, reference(&items));
        assert!(got.tied);
        ties_seen += 1;
    }
    assert_eq!(ties_seen, 10);
    // Wide-range distinct depths: no tie flag, same order.
    let items: Vec<_> = (0..1000)
        .map(|i| item((rng(&mut state) % 4) as i32, f32::from_bits(rng(&mut state) & 0x7f7f_ffff) * if i % 2 == 0 { 1.0 } else { -1.0 }, 0))
        .collect();
    let got = gpu.sort(&ctx.device, &ctx.queue, &form_keys(&items).unwrap());
    assert_eq!(got.order, reference(&items));
}

#[test]
fn equal_keys_keep_input_order() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        return;
    };
    let gpu = GpuSort::new(&ctx.device);
    let items = vec![item(1, 3.0, 0); 700];
    let got = gpu.sort(&ctx.device, &ctx.queue, &form_keys(&items).unwrap());
    assert_eq!(got.order, (0..700).collect::<Vec<u32>>());
    assert!(got.tied);
    let mixed = [item(1, 2.0, 0), item(0, 5.0, 0), item(1, 2.0, 0), item(0, 5.0, 0), item(1, 1.0, 0)];
    let got = gpu.sort(&ctx.device, &ctx.queue, &form_keys(&mixed).unwrap());
    assert_eq!(got.order, vec![1, 3, 4, 0, 2]);
}

#[test]
fn an_empty_list_and_a_single_item() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        return;
    };
    let gpu = GpuSort::new(&ctx.device);
    let none = gpu.sort(&ctx.device, &ctx.queue, &form_keys(&[]).unwrap());
    assert!(none.order.is_empty() && !none.tied);
    let one = gpu.sort(&ctx.device, &ctx.queue, &form_keys(&[item(2, -1.5, 0)]).unwrap());
    assert_eq!(one.order, vec![0]);
    assert!(!one.tied);
}
