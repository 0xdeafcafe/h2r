//! Each BSP cluster references rigid placements by index. Placements can
//! span portals. A placement that meets any of a cluster's windows is
//! visible through that cluster.
use super::{frusta::WindowFrusta, graph, read::*};
use crate::GuestRead;
use engine::render_loop::occlusion::guest::MAIN_VIS;
use std::collections::BTreeSet;

const MAX_PLACEMENTS: u32 = 0x7ff;

#[derive(Debug)]
pub(super) struct Index {
    clusters: Vec<Vec<u16>>,
    spheres: Vec<([f32; 3], f32)>,
}

impl Index {
    pub fn read(m: &dyn GuestRead, bsp: u32) -> Option<Self> {
        let count = u32_at(m, at(bsp, 0x140)?)?;
        let n = u32_at(m, at(bsp, graph::layout::CLUSTER_COUNT)?)?;
        if count > MAX_PLACEMENTS || n as usize > graph::MAX_CLUSTERS { return None; }
        let mut spheres = Vec::with_capacity(count as usize);
        if count > 0 {
            let base = ptr_at(m, at(bsp, 0x144)?)?;
            for i in 0..count {
                let placement = at(base, i * 0x58)?;
                let centre = vec3_at(m, at(placement, 0x3c)?)?;
                let radius = f32_at(m, at(placement, 0x48)?)?;
                if !centre.into_iter().all(f32::is_finite) || !radius.is_finite() || radius < 0. { return None; }
                spheres.push((centre, radius));
            }
        }
        let base = ptr_at(m, at(bsp, graph::layout::CLUSTERS)?)?;
        let mut clusters = Vec::with_capacity(n as usize);
        for i in 0..n {
            let cluster = at(base, i * graph::layout::CLUSTER_STRIDE)?;
            let refs = u32_at(m, at(cluster, 0x98)?)?;
            if refs > MAX_PLACEMENTS { return None; }
            let mut indices = Vec::with_capacity(refs as usize);
            if refs > 0 {
                let indices_at = ptr_at(m, at(cluster, 0x9c)?)?;
                for j in 0..refs {
                    let id = u16_at(m, at(indices_at, j * 2)?)?;
                    if u32::from(id) >= count { return None; }
                    indices.push(id);
                }
            }
            clusters.push(indices);
        }
        Some(Self { clusters, spheres })
    }

    fn visible(&self, windows: &WindowFrusta) -> BTreeSet<u16> {
        let mut ids = BTreeSet::new();
        for &(cluster, ref frustum) in &windows.0 {
            for &id in self.clusters.get(usize::from(cluster)).into_iter().flatten() {
                let (centre, radius) = self.spheres[usize::from(id)];
                if frustum.admits(centre, radius) { ids.insert(id); }
            }
        }
        ids
    }

    pub fn extra(&self, retail: &WindowFrusta, wide: &WindowFrusta, mut excluded: BTreeSet<u16>) -> Vec<u16> {
        excluded.extend(self.visible(retail));
        self.visible(wide).difference(&excluded).copied().collect()
    }
}

/// The live placement list's low word in each 4-byte ID slot, as read
/// by the structure placement record walk.
pub(super) fn submitted(m: &dyn GuestRead) -> Option<BTreeSet<u16>> {
    let list = ptr_at(m, MAIN_VIS + 0x18)?;
    let count = u32::from(u16_at(m, at(list, 4)?)?);
    if count == 0 { return Some(BTreeSet::new()); }
    if count > MAX_PLACEMENTS || count > u32_at(m, list)? { return None; }
    let ids = ptr_at(m, at(list, 0xc)?)?;
    (0..count).map(|i| u16_at(m, at(ids, 4*i)?)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visibility::frusta::Frustum;

    #[test]
    fn references_are_halfwords_and_submitted_placement_slots_are_words() {
        let mut m = crate::testing::Mem::default();
        let (bsp, cluster, instances, refs, list, ids) = (0x1000, 0x2000, 0x3000, 0x4000, 0x5000, 0x6000);
        m.w(bsp + 0x140, 2);
        m.w(bsp + 0x144, instances);
        for i in 0..2 {
            for (o, v) in [(0x3c, 1.), (0x40, 2.), (0x44, 3.), (0x48, 0.5)] {
                m.f(instances + 0x58*i + o, v);
            }
        }
        m.w(bsp + graph::layout::CLUSTER_COUNT, 1);
        m.w(bsp + graph::layout::CLUSTERS, cluster);
        m.w(cluster + 0x98, 2);
        m.w(cluster + 0x9c, refs);
        m.w(refs, 1);
        assert_eq!(Index::read(&m, bsp).unwrap().clusters, [vec![1, 0]]);
        m.w(refs, 2 << 16);
        assert!(Index::read(&m, bsp).is_none());
        m.w(MAIN_VIS + 0x18, list);
        m.w(list, 2);
        m.w(list + 4, 2);
        m.w(list + 0xc, ids);
        m.w(ids, 1 << 16 | 7);
        m.w(ids + 4, 2 << 16 | 9);
        assert_eq!(submitted(&m).unwrap(), [7, 9].into());
    }

    #[test]
    fn shared_placements_enter_once_through_any_open_window() {
        let window = |width: f32| Frustum {
            bounds: [-width, width, -1., 1., -1., 1.],
            planes: [[0., 0., 0., 0.]; 6],
        };
        let index = Index {
            clusters: vec![vec![0, 1, 2], vec![1, 1, 2, 3]],
            spheres: vec![([0., 0., 0.], 0.1), ([1.6, 0., 0.], 0.1), ([2., 0., 0.], 0.1), ([5., 0., 0.], 0.1)],
        };
        let retail = WindowFrusta(vec![(0, window(1.))]);
        let wide = WindowFrusta(vec![(0, window(2.)), (1, window(2.))]);
        assert_eq!(index.extra(&retail, &wide, [2].into()), [1]);
    }
}
