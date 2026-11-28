//! Merge display-only BSP geometry into a private dispatch. The reference
//! index array and the allocator belong to the guest's narrower camera.

use super::*;
use frame_scene::transparent::{
    DisplayTransparentPart, DisplayTransparentQueue, DisplayTransparentSlot,
};

pub(crate) fn install(b: &mut Builder) {
    b.raw()
        .observe_before(
            &points::TRANSPARENT_ORDER_READY,
            "extract display transparent order",
            |c| {
                if fp_supported(c.cpu).is_err() {
                    return;
                }
                let mut mem = c.guest();
                // 0x1e370's saved registers and locals are below its caller.
                if !c
                    .cpu
                    .esp
                    .checked_add(20)
                    .is_some_and(|sp| mem.u32(sp).ok() == Some(0x0002_c19f))
                {
                    return;
                }
                let built = crate::scene_step::display_transparents(|clusters| {
                    queue(&*mem, clusters, |id| {
                        crate::native_world::display_cluster_transparents(
                            &*mem,
                            id,
                            c.cpu.x87.control,
                        )
                    })
                });
                // The head run: before the loop's first entry.
                if let Some(built) = built {
                    crate::native_water::begin_display_runs(&mut *mem, &built, c.cpu.esp);
                }
            },
        )
        .context_only()
        .optional();
    // After each entry of the loop, the one it called or the one it skipped
    // for a null handler: `esi` is the ordinal. `0x0001e3f3`, where the
    // call returns, is a call's continuation, so no hook runs there.
    b.raw()
        .observe_before(
            &points::scene_effects::TRANSPARENT_DISPATCH_NEXT,
            "draw display runs after a sorted entry",
            |c| {
                let mut mem = c.guest();
                crate::native_water::display_runs_after_entry(&mut *mem, c.cpu.esp, c.cpu.esi);
            },
        )
        .context_only()
        .optional();
    b.raw()
        .observe_before(
            &points::scene_effects::SCENE_TRANSPARENTS_END,
            "end display runs",
            |_| crate::native_water::end_display_runs(),
        )
        .context_only()
        .optional();
}

fn queue(
    m: &dyn GuestMemory,
    clusters: &[u16],
    mut parts: impl FnMut(u16) -> Option<Vec<DisplayTransparentPart>>,
) -> Option<DisplayTransparentQueue> {
    let first = m.u32(LIST_FIRST).ok()?;
    let end = m.u32(LIST_END).ok()?;
    if first > end || end > CAPACITY {
        return None;
    }
    let camera = m.vec3(CAMERA).ok()?;
    let mut slots = Vec::new();
    let mut keys = Vec::new();
    if end > first {
        let entries = ptr(m, LIST_ENTRIES).ok()?;
        let order = ptr(m, LIST_ORDER).ok()?;
        // Start with the original insertion order. An entry's payload has
        // already run (or will run) at its sorted reference ordinal.
        let mut ordinals = vec![None; (end - first) as usize];
        for ordinal in 0..end - first {
            let index = u32::from(m.u16(order.checked_add(ordinal * 2)?).ok()?);
            let o = ordinals.get_mut(index.checked_sub(first)? as usize)?;
            if o.replace(ordinal as u16).is_some() {
                return None;
            }
        }
        for (i, ordinal) in ordinals.into_iter().enumerate() {
            keys.push(Key::read(m, entries.checked_add((first + i as u32) * STRIDE)?).ok()?);
            slots.push(DisplayTransparentSlot::Reference(ordinal?));
        }
    }
    let shared = keys.len();
    // Compare the original sort before inserting anything. Include every
    // callback slot, also effect handlers and the slots dispatch skips.
    let mut check: Vec<usize> = (0..shared).collect();
    sort(&mut check, &|a, b| keys[a].after(&keys[b], camera));
    let reference_exact = check
        .iter()
        .enumerate()
        .all(|(ordinal, &index)| slots[index] == DisplayTransparentSlot::Reference(ordinal as u16));
    let mut unresolved_clusters = Vec::new();
    for &cluster in clusters {
        let Some(parts) = parts(cluster) else {
            unresolved_clusters.push(cluster);
            continue;
        };
        if slots.len() + parts.len() > CAPACITY as usize {
            unresolved_clusters.push(cluster);
            continue;
        }
        for part in parts {
            keys.push(Key {
                class: part.class,
                depth: part.depth,
                plane: part.plane,
                position: part.position,
                tie: 0,
            });
            slots.push(DisplayTransparentSlot::Geometry(part));
        }
    }
    let mut order: Vec<usize> = (0..slots.len()).collect();
    sort(&mut order, &|a, b| keys[a].after(&keys[b], camera));
    // New entries can disturb ties in the old sort even when the old keys
    // are equal to one another. Without the wide producer's interleave,
    // only a strict order independent of that interleave is certified.
    let strict =
        |a: usize, b: usize| !keys[a].after(&keys[b], camera) && keys[b].after(&keys[a], camera);
    let order_exact = reference_exact
        && (keys.len() == shared
            || if keys.iter().all(|k| {
                k.plane.is_none() && k.depth.is_finite() && (0..=3).contains(&k.class) && k.tie == 0
            }) {
                order.windows(2).all(|pair| strict(pair[0], pair[1]))
            } else {
                order
                    .iter()
                    .enumerate()
                    .all(|(n, &a)| order[n + 1..].iter().all(|&b| strict(a, b)))
            });
    Some(DisplayTransparentQueue {
        slots: order.into_iter().map(|i| slots[i].clone()).collect(),
        unresolved_clusters,
        order_exact,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    fn part(depth: f32) -> DisplayTransparentPart {
        DisplayTransparentPart {
            cluster: 19,
            part: 1,
            lod: 0,
            shader: 3,
            first_index: 6,
            index_count: 9,
            triangle_list: true,
            first_subpart: 1,
            subpart_count: 3,
            position: [0.; 3],
            depth,
            class: 1,
            plane: None,
        }
    }

    #[test]
    fn wide_water_interleaves_with_reference_callbacks_without_changing_their_queue() {
        let mut m = FlatMemory::new(0x0040_0000, 0x0010_0000);
        let (rows, order) = (0x0047_0000, 0x0048_0000);
        m.set_u32(LIST_ENTRIES, rows).unwrap();
        m.set_u32(LIST_ORDER, order).unwrap();
        m.set_u32(LIST_FIRST, 4).unwrap();
        m.set_u32(LIST_END, 6).unwrap();
        for (i, depth) in [(4, 9.), (5, 3.)] {
            m.set_u32(rows + i * STRIDE, 1).unwrap();
            m.set_f32(rows + i * STRIDE + 4, depth).unwrap();
        }
        m.set_u16(order, 5).unwrap();
        m.set_u16(order + 2, 4).unwrap();
        let before = m.bytes.clone();
        let p = part(6.);
        let q = queue(&m, &[19, 20], |id| (id == 19).then(|| vec![p.clone()])).unwrap();
        assert!(q.order_exact);
        assert_eq!(q.unresolved_clusters, [20]);
        assert_eq!(
            q.slots,
            [
                DisplayTransparentSlot::Reference(0),
                DisplayTransparentSlot::Geometry(p),
                DisplayTransparentSlot::Reference(1)
            ]
        );
        let q = queue(&m, &[19], |_| Some(vec![part(3.)])).unwrap();
        assert!(!q.order_exact);
        assert_eq!(m.bytes, before);
        m.set_u16(order + 2, 5).unwrap();
        assert!(queue(&m, &[], |_| None).is_none());
    }
}
