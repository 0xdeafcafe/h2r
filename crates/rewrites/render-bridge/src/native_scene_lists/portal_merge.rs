//! Merge rectangular windows within a single cluster, at 0x00172c70.
//! The region writer consumes the surviving rectangles, depths and the
//! merge target of each absorbed node. The clip polygons stay in place.
//! After the arm's proof Rust merges every call but one in 64 (`Audit`);
//! that one runs the guest and is compared at the region writer's call,
//! and a difference sends the merge to the guest for the session.

use super::*;
use scene_extract::visibility::{flood, graph, guest};
use std::sync::Mutex;

const NODE: u32 = 0x6c;
const MERGE_RETURN: u32 = 0x0017_32df;
const REGION_RETURN: u32 = 0x0017_32fe;
const STAGE: SceneList = SceneList::PortalWindowMerge;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Window {
    rect: [u32; 4],
    depth: u32,
    into: u32,
}

impl Window {
    fn read(mem: &dyn GuestMemory, node: u32) -> Result<Self> {
        let mut rect = [0; 4];
        for (i, v) in rect.iter_mut().enumerate() {
            *v = mem.u32(at(node, 0xc + i as u32 * 4)?)?;
        }
        Ok(Self {
            rect,
            depth: mem.u32(at(node, 0x24)?)?,
            into: mem.u32(at(node, 0x28)?)?,
        })
    }

    fn grow(&mut self, source: Self, factor: f32) -> bool {
        let args = |w: Self| {
            (
                flood::Rect(w.rect.map(f32::from_bits)),
                f32::from_bits(w.depth),
            )
        };
        if let Some((rect, depth)) =
            scene_extract::visibility::portal_walk::merge_window(args(*self), args(source), factor)
        {
            self.rect = rect.0.map(f32::to_bits);
            self.depth = depth.to_bits();
            return true;
        }
        false
    }
}

#[derive(PartialEq, Eq)]
struct Inputs {
    bsp_epoch: u64,
    factor: u32,
    // Singly reached regions have no candidate pair. Their rectangles
    // and depths cannot affect a merge, even if the camera moves.
    groups: Vec<Vec<(u16, Window)>>,
}

impl Inputs {
    fn merge(&self, windows: &mut [Window]) {
        for group in &self.groups {
            for (i, &(from, _)) in group.iter().enumerate() {
                for &(into, _) in &group[i + 1..] {
                    let source = windows[usize::from(from)];
                    if windows[usize::from(into)].grow(source, f32::from_bits(self.factor)) {
                        windows[usize::from(from)].into = u32::from(into);
                        break;
                    }
                }
            }
        }
    }
}

struct Plan {
    pool: u32,
    chains: u32,
    inputs: Inputs,
    windows: Vec<Window>,
    output: Output,
}

fn node(pool: u32, index: usize) -> Result<u32> {
    at(pool, 4 + index as u32 * NODE)
}

impl Plan {
    fn build(m: &dyn GuestMemory, cpu: &CpuAbi) -> Result<Self> {
        fp_supported(cpu)?;
        let pool = stack(m, cpu, 4)?;
        let chains = cpu.esi;
        let count = m.u32(pool)? as usize;
        let bsp = ptr(m, guest::ACTIVE_STRUCTURE)?;
        let clusters = m.u32(at(bsp, graph::layout::CLUSTER_COUNT)?)? as usize;
        if count > flood::MAX_NODES || clusters > graph::MAX_CLUSTERS {
            return Err(Refusal::Layout);
        }
        let factor = m.f32(guest::MERGE)?;
        if !factor.is_finite() {
            return Err(Refusal::Unsupported);
        }
        let mut windows = Vec::with_capacity(count);
        let mut by_cluster = vec![Vec::new(); clusters];
        for i in 0..count {
            let n = node(pool, i)?;
            let cluster = usize::from(m.u16(at(n, 8)?)?);
            let group: &mut Vec<(u16, Window)> =
                by_cluster.get_mut(cluster).ok_or(Refusal::Layout)?;
            let w = Window::read(m, n)?;
            if w.into != NONE
                || !w
                    .rect
                    .into_iter()
                    .chain([w.depth])
                    .map(f32::from_bits)
                    .all(f32::is_finite)
            {
                return Err(Refusal::Unsupported);
            }
            // The guest links each new path at the head. Require the
            // exact order it will walk before using our node order.
            let prev = group.last().map_or(u16::MAX, |&(index, _)| index);
            if m.u16(at(chains, 0x440 + 2 * i as u32)?)? != prev {
                return Err(Refusal::Layout);
            }
            group.push((i as u16, w));
            windows.push(w);
        }
        let mut groups = Vec::new();
        for (word, block) in by_cluster.chunks_mut(32).enumerate() {
            let mask = m.u32(at(chains, 4 * word as u32)?)?;
            for (bit, group) in block.iter_mut().enumerate() {
                if (mask & (1 << bit) != 0) == group.is_empty() {
                    return Err(Refusal::Layout);
                }
                if let Some(&(head, _)) = group.last() {
                    let cluster = word * 32 + bit;
                    if m.u16(at(chains, 0x40 + 2 * cluster as u32)?)? != head {
                        return Err(Refusal::Layout);
                    }
                }
                if group.len() > 1 {
                    group.reverse();
                    groups.push(std::mem::take(group));
                }
            }
        }
        let inputs = Inputs {
            bsp_epoch: crate::native_map_work::bsp_generation(),
            factor: factor.to_bits(),
            groups,
        };
        let before = windows.clone();
        inputs.merge(&mut windows);
        let mut output = Output::default();
        for (i, (a, b)) in before.iter().zip(&windows).enumerate() {
            if a == b {
                continue;
            }
            let n = node(pool, i)?;
            if a.rect != b.rect {
                output.array(
                    at(n, 0xc)?,
                    b.rect.into_iter().flat_map(u32::to_le_bytes).collect(),
                );
            }
            if a.depth != b.depth {
                output.word(at(n, 0x24)?, b.depth);
            }
            if a.into != b.into {
                output.word(at(n, 0x28)?, b.into);
            }
        }
        Ok(Self {
            pool,
            chains,
            inputs,
            windows,
            output,
        })
    }

    fn matches(&self, m: &dyn GuestMemory) -> Result<bool> {
        for (i, &window) in self.windows.iter().enumerate() {
            if Window::read(m, node(self.pool, i)?)? != window {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[derive(Default)]
struct State {
    audit: Audit,
    /// The BSP generation the audit's proof belongs to.
    epoch: Option<u64>,
    pending: Option<Plan>,
}

impl State {
    /// The guest merge's result at the region writer's call.
    fn finish(
        &mut self,
        m: &dyn GuestMemory,
        pool: u32,
        chains: u32,
        ret: Option<u32>,
        epoch: u64,
    ) -> Option<bool> {
        let plan = self.pending.take()?;
        if plan.pool != pool
            || plan.chains != chains
            || ret != Some(REGION_RETURN)
            || plan.inputs.bsp_epoch != epoch
        {
            return None;
        }
        let same = plan.matches(m) == Ok(true);
        if self.audit.compared(same) {
            eprintln!(
                "native-scene-lists: the Rust window merge differs from 0x00172c70's; the guest merges from here"
            );
        }
        Some(same)
    }
}

pub(super) fn install(b: &mut Builder) {
    let state = Arc::new(Mutex::new(State::default()));
    let compare = Arc::clone(&state);
    b.raw()
        .observe_before(
            &points::PORTAL_WINDOW_REGIONS,
            "compare merged portal windows",
            move |o| {
                let m = o.guest();
                let mut state = compare.lock().unwrap_or_else(|e| e.into_inner());
                let epoch = crate::native_map_work::bsp_generation();
                match state.finish(&*m, o.cpu.ebx, o.cpu.edi, m.u32(o.cpu.esp).ok(), epoch) {
                    Some(true) => STAGE.count(SceneListResult::Compared),
                    Some(false) => {
                        STAGE.count(SceneListResult::Compared);
                        STAGE.count(SceneListResult::Mismatch);
                    }
                    None => (),
                }
            },
        )
        .optional()
        .context_only();
    super::install(
        b,
        Seam::Function(&points::PORTAL_WINDOW_MERGE, 4),
        move |m, cpu| {
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            state.pending = None;
            let output = (|| {
                if m.u32(cpu.esp)? != MERGE_RETURN {
                    return Err(Refusal::Unsupported);
                }
                let epoch = crate::native_map_work::bsp_generation();
                if state.epoch != Some(epoch) {
                    state.audit.rescope();
                    state.epoch = Some(epoch);
                }
                let guest = Output {
                    guest: true,
                    ..Output::default()
                };
                if !state.audit.compares() {
                    STAGE.count(SceneListResult::Fallback);
                    return Ok(guest);
                }
                let plan = Plan::build(m, cpu)?;
                if state.audit.answers() {
                    return Ok(plan.output);
                }
                // The guest merges this call; the region writer's call
                // compares its result.
                state.pending = Some(plan);
                STAGE.count(SceneListResult::Fallback);
                Ok(guest)
            })();
            (STAGE, output)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(rect: [f32; 4], depth: f32) -> Window {
        Window {
            rect: rect.map(f32::to_bits),
            depth: depth.to_bits(),
            into: NONE,
        }
    }

    #[test]
    fn merges_in_chain_order_and_carries_the_nearest_depth_across_merges() {
        let mut windows = [
            window([1., 4., 0., 2.], 8.),
            window([1., 3., 0., 2.], 6.),
            window([1., 2., 0., 2.], 2.),
            window([8., 9., 0., 2.], 1.),
        ];
        let group = [3, 2, 1, 0].map(|i| (i, windows[usize::from(i)])).to_vec();
        let inputs = Inputs {
            bsp_epoch: 5,
            factor: 1f32.to_bits(),
            groups: vec![group],
        };
        inputs.merge(&mut windows);
        assert_eq!(windows.map(|w| w.into), [NONE, 0, 1, NONE]);
        assert_eq!(windows[0], window([1., 4., 0., 2.], 2.));
    }

    #[test]
    fn the_guest_merge_proves_the_arm_and_a_different_window_falls_it_back() {
        use guest_layout::memory::FlatMemory;
        const POOL: u32 = 0x2000;
        let mut m = FlatMemory::new(0x1000, 0x2000);
        let windows = vec![window([0., 1., 0., 1.], 1.)];
        for (i, w) in windows.iter().enumerate() {
            let n = node(POOL, i).unwrap();
            for (j, v) in w.rect.iter().enumerate() {
                m.set_u32(n + 0xc + 4 * j as u32, *v).unwrap();
            }
            m.set_u32(n + 0x24, w.depth).unwrap();
            m.set_u32(n + 0x28, w.into).unwrap();
        }
        let plan = || Plan {
            pool: POOL,
            chains: 0x1100,
            inputs: Inputs {
                bsp_epoch: 2,
                factor: 1f32.to_bits(),
                groups: Vec::new(),
            },
            windows: windows.clone(),
            output: Output::default(),
        };
        let mut state = State::default();
        // Another pool, return or BSP compares nothing.
        for (pool, ret, epoch) in [(POOL + 4, REGION_RETURN, 2), (POOL, 0, 2), (POOL, REGION_RETURN, 3)] {
            state.pending = Some(plan());
            assert_eq!(state.finish(&m, pool, 0x1100, Some(ret), epoch), None);
        }
        for _ in 0..Audit::PROOF {
            assert!(!state.audit.answers());
            state.pending = Some(plan());
            assert_eq!(state.finish(&m, POOL, 0x1100, Some(REGION_RETURN), 2), Some(true));
        }
        assert!(state.audit.answers());
        m.set_u32(node(POOL, 0).unwrap() + 0x24, 0.5f32.to_bits()).unwrap();
        state.pending = Some(plan());
        assert_eq!(state.finish(&m, POOL, 0x1100, Some(REGION_RETURN), 2), Some(false));
        assert!(state.audit.fallen_back() && !state.audit.answers());
    }

    #[test]
    fn threshold_is_strict_and_signed_zero_bounds_keep_the_selected_operand() {
        let a = window([-0., 1., 0., 1.], 1.);
        let b = window([0., 1., -0., 1.], -0.);
        let mut dest = a;
        assert!(!dest.grow(b, 0.5));
        assert_eq!(dest, a);
        assert!(dest.grow(b, 1.));
        assert_eq!(dest.rect, a.rect);
        assert_eq!(dest.depth, (-0.0f32).to_bits());
    }
}
