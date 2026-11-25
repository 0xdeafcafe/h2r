//! Per-frustum mode-0 portal traversal. Compare the active node pool, polygons,
//! cluster chains and queue at the next call, before any window is merged.
//! Only an identical input to a verified walk may bypass the guest traversal.

use std::cell::RefCell;
use std::sync::Mutex;

use portal_walk::Node;
use scene_extract::GuestRead;
use scene_extract::visibility::{Decline, GraphCache, flood, graph, guest, portal_walk};

use super::*;

/// The only call from the portal pass and its following call to the merger.
const WALK_RETURN: u32 = 0x0017_32cb;
const MERGE_RETURN: u32 = 0x0017_32df;
const NODE_SIZE: u32 = 0x6c;
const CORNER_COUNT: u32 = 4 + NODE_SIZE * flood::MAX_NODES as u32;
const CORNERS: u32 = CORNER_COUNT + 4;
const CHAIN_HEADS: u32 = 0x40;
const CHAIN_LINKS: u32 = 0x440;
const FRUSTUM_SIZE: usize = 0x1bc;

#[derive(Clone, Copy)]
struct Scratch {
    nodes: u32,
    chains: u32,
    queue: u32,
    clusters: usize,
}

impl Scratch {
    /// Match the calling frame, including its freshly seeded queue, root
    /// and projection cache. A light can reach exactly the same return PC.
    fn read(m: &dyn GuestMemory, cpu: &CpuAbi) -> Result<(Self, u32, u16)> {
        fp_supported(cpu)?;
        if cpu.x87.control != 0x027f || cpu.x87.tag != u16::MAX || cpu.eflags & (1 << 10) != 0 {
            return Err(Refusal::Unsupported);
        }
        let structure = ptr(m, guest::ACTIVE_STRUCTURE)?;
        let vis = objects::MAIN_VIS;
        if m.u32(guest::ACTIVE_VIS)? != vis
            || m.u32(at(vis, objects::VIEW_KIND)?)? != 0
            || m.u32(at(vis, guest::vis::MODE)?)? != 0
            || m.u32(at(cpu.ebp, 8)?)? != 1
            || m.u32(at(cpu.ebp, 0x14)?)? != ptr(m, vis)?
            || stack(m, cpu, 4)? != 0
        {
            return Err(Refusal::Unsupported);
        }
        let frame = at(cpu.esp, 24)?;
        let projection = stack(m, cpu, 20)?;
        let s = Self {
            nodes: stack(m, cpu, 8)?,
            chains: stack(m, cpu, 12)?,
            queue: stack(m, cpu, 16)?,
            clusters: m.u32(at(structure, graph::layout::CLUSTER_COUNT)?)? as usize,
        };
        let portals = m.u32(at(structure, graph::layout::PORTAL_COUNT)?)? as usize;
        let frustum = ptr(m, at(cpu.ebp, 0xc)?)?;
        let start = u16::try_from(m.u32(at(cpu.ebp, 0x10)?)?).map_err(|_| Refusal::Layout)?;
        if usize::from(start) >= s.clusters
            || s.clusters > graph::MAX_CLUSTERS
            || portals > graph::MAX_PORTALS
            || s.nodes != at(frame, 0x100c0)?
            || s.chains != at(frame, 0x220)?
            || s.queue != at(frame, 0x18)?
            || projection != at(frame, 0x1060)?
            || m.u32(projection)? != structure
            || m.u32(at(projection, 4)?)? != m.u32(at(structure, graph::layout::COLLISION_BSP)?)?
            || m.u32(at(projection, 8)?)? != frustum
            || m.u32(at(projection, 0x505c)?)? != 0
        {
            return Err(Refusal::Layout);
        }
        // Scratch near plane comes from the caller's negative unit constant.
        for (i, v) in [0, 0, m.u32(guest::FRUSTUM_NEGATIVE_UNIT)?, 0]
            .into_iter()
            .enumerate()
        {
            if m.u32(at(projection, 12 + i as u32 * 4)?)? != v {
                return Err(Refusal::Layout);
            }
        }
        for i in 0..portals.div_ceil(32) {
            if m.u32(at(projection, 0x1c + i as u32 * 4)?)? != 0 {
                return Err(Refusal::Layout);
            }
        }
        Ok((s, frustum, start))
    }

    /// Encode precisely the defined fields. In particular the two bytes
    /// after a node's corner count and heads of absent clusters are stale.
    fn image(self, nodes: &[Node], drained: bool) -> Result<Output> {
        if nodes.is_empty() || nodes.len() > flood::MAX_NODES || self.clusters > graph::MAX_CLUSTERS
        {
            return Err(Refusal::Layout);
        }
        let mut image = Output::default();
        let mut heads = vec![u16::MAX; self.clusters];
        let mut bits = vec![0u32; self.clusters.div_ceil(32)];
        let mut links = Vec::with_capacity(nodes.len() * 2);
        let mut queue_ids = Vec::with_capacity(nodes.len() * 2);
        let mut corners = Vec::new();
        for (i, node) in nodes.iter().enumerate() {
            let ordinal = i as u16;
            let cluster = usize::from(node.cluster);
            let head = heads.get_mut(cluster).ok_or(Refusal::Layout)?;
            links.extend_from_slice(&std::mem::replace(head, ordinal).to_le_bytes());
            bits[cluster / 32] |= 1u32 << (cluster % 32);
            queue_ids.extend_from_slice(&ordinal.to_le_bytes());

            let offset = at(self.nodes, 4 + i as u32 * NODE_SIZE)?;
            let mut fields = Vec::with_capacity(0x1e);
            fields.extend_from_slice(&0u32.to_le_bytes()); // first and only frustum
            for value in [node.parent, node.portal, Some(node.cluster), None] {
                fields.extend_from_slice(&value.unwrap_or(u16::MAX).to_le_bytes());
            }
            for value in node.rect.0 {
                fields.extend_from_slice(&value.to_le_bytes());
            }
            if !(3..=flood::MAX_CORNERS).contains(&node.polygon.len()) {
                return Err(Refusal::Layout);
            }
            fields.extend_from_slice(&(node.polygon.len() as u16).to_le_bytes());
            image.array(offset, fields);
            let mut tail = Vec::with_capacity(0x4c);
            tail.extend_from_slice(&at(self.nodes, CORNERS + corners.len() as u32)?.to_le_bytes());
            tail.extend_from_slice(&node.depth.to_le_bytes());
            tail.extend_from_slice(&NONE.to_le_bytes()); // not merged
            for word in node.crossed {
                tail.extend_from_slice(&word.to_le_bytes());
            }
            image.array(at(offset, 0x20)?, tail);
            for point in &node.polygon {
                for value in point {
                    corners.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        let count = nodes.len() as u32;
        let n_corners = (corners.len() / 8) as u32;
        if n_corners as usize > flood::MAX_WINDOW_CORNERS + flood::MAX_CORNERS {
            return Err(Refusal::Layout);
        }
        image.word(self.nodes, count);
        image.word(at(self.nodes, CORNER_COUNT)?, n_corners);
        image.array(at(self.nodes, CORNERS)?, corners);
        image.array(
            self.chains,
            bits.into_iter().flat_map(u32::to_le_bytes).collect(),
        );
        for (cluster, head) in heads.into_iter().enumerate() {
            if head != u16::MAX {
                image.half(at(self.chains, CHAIN_HEADS + 2 * cluster as u32)?, head);
            }
        }
        image.array(at(self.chains, CHAIN_LINKS)?, links);
        let end = count % flood::MAX_NODES as u32;
        image.word(self.queue, if drained { end } else { 0 });
        image.word(at(self.queue, 4)?, end);
        image.array(at(self.queue, 8)?, queue_ids);
        Ok(image)
    }
}

fn matches_image(m: &dyn GuestMemory, image: &Output) -> Result<bool> {
    let mut scratch = Vec::new();
    for w in &image.writes {
        let wanted = w.bytes.slice();
        scratch.resize(wanted.len(), 0);
        m.read(w.addr, &mut scratch)?;
        if scratch != wanted {
            return Ok(false);
        }
    }
    Ok(true)
}

struct Reads<'a> {
    m: &'a dyn GuestMemory,
    words: RefCell<Option<Vec<(u32, u32)>>>,
}

impl GuestRead for Reads<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        let value = self.m.u32(addr).ok()?;
        if let Some(words) = self.words.borrow_mut().as_mut() {
            words.push((addr, value));
        }
        Some(value)
    }
}

#[derive(PartialEq, Eq)]
struct Inputs {
    bsp_epoch: u64,
    structure: u32,
    start: u16,
    frustum: Vec<u8>,
    /// Every live word read by the walk (including open portals and the
    /// image's clip/projection tolerances). The tag graph is per BSP epoch.
    words: Vec<(u32, u32)>,
}

struct Comparison {
    thread: u32,
    merge_esp: u32,
    scratch: Scratch,
    inputs: Inputs,
    image: Output,
}

#[derive(Default)]
struct State {
    graph: GraphCache,
    checked: Option<Inputs>,
    pending: Option<Comparison>,
}

fn translate(why: Decline) -> Refusal {
    match why {
        Decline::Unread => Refusal::Memory,
        Decline::Layout | Decline::Overflow => Refusal::Layout,
        _ => Refusal::Unsupported,
    }
}

impl State {
    fn finish(
        &mut self,
        m: &dyn GuestMemory,
        cpu: &CpuAbi,
        thread: u32,
        epoch: u64,
    ) -> Result<Option<bool>> {
        let Some(p) = self.pending.take() else {
            return Ok(None);
        };
        if thread != p.thread
            || cpu.esp != p.merge_esp
            || cpu.esi != p.scratch.chains
            || m.u32(cpu.esp)? != MERGE_RETURN
            || stack(m, cpu, 4)? != p.scratch.nodes
            || p.inputs.bsp_epoch != epoch
        {
            return Ok(None);
        }
        let matched = matches_image(m, &p.image)?;
        self.checked = matched.then_some(p.inputs);
        Ok(Some(matched))
    }

    fn prepare(
        &mut self,
        m: &dyn GuestMemory,
        cpu: &CpuAbi,
        thread: u32,
    ) -> Result<Option<Output>> {
        let (scratch, frustum, start) = Scratch::read(m, cpu)?;
        let epoch = crate::native_map_work::bsp_generation();
        let structure = ptr(m, guest::ACTIVE_STRUCTURE)?;
        let reads = Reads {
            m,
            words: RefCell::default(),
        };
        // Keep graph decoding out of the dynamic key on the first frame too.
        self.graph
            .graph(&reads, epoch, structure)
            .map_err(translate)?;
        *reads.words.borrow_mut() = Some(Vec::new());
        let walk = portal_walk::for_view(&reads, &mut self.graph, epoch, frustum, start)
            .map_err(translate)?;
        let root = walk.nodes.first().ok_or(Refusal::Layout)?;
        if !matches_image(m, &scratch.image(std::slice::from_ref(root), false)?)? {
            return Err(Refusal::Layout);
        }
        let inputs = Inputs {
            bsp_epoch: epoch,
            structure,
            start,
            frustum: m.bytes(frustum, FRUSTUM_SIZE)?,
            words: reads.words.into_inner().unwrap_or_default(),
        };
        let image = scratch.image(&walk.nodes, true)?;
        if self.checked.as_ref() == Some(&inputs) {
            return Ok(Some(image));
        }
        self.checked = None;
        self.pending = Some(Comparison {
            thread,
            merge_esp: at(cpu.esp, 16)?,
            scratch,
            inputs,
            image,
        });
        Ok(None)
    }
}

const STAGE: SceneList = SceneList::PortalWalk;

pub(super) fn install(b: &mut Builder) {
    let state = Arc::new(Mutex::new(State::default()));
    let before = state.clone();
    b.raw()
        .before(
            &points::PORTAL_WINDOW_WALK,
            "gather portal windows",
            move |c| {
                let runtime = Arc::clone(c.ctx.runtime());
                let mut mem = runtime.memory();
                let mut state = before.lock().unwrap_or_else(|e| e.into_inner());
                state.pending = None;
                if c.addr != points::PORTAL_WINDOW_WALK.base
                    || mem.u32(c.cpu.esp).ok() != Some(WALK_RETURN)
                    || mem.u32(guest::ACTIVE_VIS).ok() != Some(objects::MAIN_VIS)
                {
                    return Ok(Flow::Continue);
                }
                let image = match state.prepare(&*mem, c.cpu, c.ctx.thread_id) {
                    Ok(Some(image)) => image,
                    Ok(None) => {
                        STAGE.count(SceneListResult::Fallback);
                        return Ok(Flow::Continue);
                    }
                    Err(why) => {
                        STAGE.count(SceneListResult::Fallback);
                        STAGE.count(match why {
                            Refusal::Memory => SceneListResult::Memory,
                            Refusal::Layout => SceneListResult::Layout,
                            _ => SceneListResult::Unsupported,
                        });
                        return Ok(Flow::Continue);
                    }
                };
                if image.check_write(&mem).is_err() {
                    STAGE.count(SceneListResult::Memory);
                    STAGE.count(SceneListResult::Fallback);
                    return Ok(Flow::Continue);
                }
                let transfer = mem.ret(c.cpu, 20)?;
                image.publish(&mut mem, c.addr)?;
                c.cpu.eax = (c.cpu.eax & !0xff) | 1;
                STAGE.count(SceneListResult::Native);
                Ok(Flow::Return(transfer))
            },
        )
        .optional()
        .context_only()
        .last();

    // Observers run before the merger, including when Rust owns that call.
    b.raw()
        .observe_before(
            &points::PORTAL_WINDOW_MERGE,
            "compare portal windows",
            move |obs| {
                let mem = obs.guest();
                let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                if obs.addr != points::PORTAL_WINDOW_MERGE.base {
                    return;
                }
                match state.finish(
                    &*mem,
                    obs.cpu,
                    obs.thread_id,
                    crate::native_map_work::bsp_generation(),
                ) {
                    Ok(Some(matched)) => {
                        STAGE.count(SceneListResult::Compared);
                        if !matched {
                            STAGE.count(SceneListResult::Mismatch);
                        }
                    }
                    Ok(None) => (),
                    Err(_) => STAGE.count(SceneListResult::Memory),
                }
            },
        )
        .optional()
        .context_only();
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;
    use flood::Rect;

    const S: Scratch = Scratch {
        nodes: 0xb000,
        chains: 0x1400,
        queue: 0x1100,
        clusters: 7,
    };

    fn windows() -> Vec<Node> {
        // Synthetic walk 4 -> 6 -> 4. The last path must become the head
        // for 4 and link to the first, even when the windows coincide.
        [4, 6, 4]
            .into_iter()
            .enumerate()
            .map(|(i, cluster)| {
                let mut crossed = [0; 16];
                crossed[0] = (1 << i) - 1;
                let rect = Rect([-1.0, 0.0, -0.0, 1.0]);
                Node {
                    cluster,
                    parent: i.checked_sub(1).map(|v| v as u16),
                    portal: i.checked_sub(1).map(|v| v as u16),
                    rect,
                    polygon: rect.corners(),
                    depth: i as f32 * 0.5,
                    crossed,
                }
            })
            .collect()
    }

    fn apply(m: &mut FlatMemory, output: &Output) {
        for w in &output.writes {
            m.write(w.addr, w.bytes.slice()).unwrap();
        }
    }

    #[test]
    fn publication_preserves_padding_and_links_repeated_clusters_newest_first() {
        let nodes = windows();
        let mut m = FlatMemory::new(0x1000, 0x20000);
        m.bytes.fill(0xbd);
        let image = S.image(&nodes, true).unwrap();
        apply(&mut m, &image);
        assert_eq!(m.u32(S.nodes).unwrap(), 3);
        assert_eq!(m.u32(S.nodes + CORNER_COUNT).unwrap(), 12);
        assert_eq!(m.u32(S.chains).unwrap(), 0x50);
        assert_eq!(m.u16(S.chains + CHAIN_HEADS + 8).unwrap(), 2);
        assert_eq!(m.u16(S.chains + CHAIN_HEADS + 12).unwrap(), 1);
        assert_eq!(m.u16(S.chains + CHAIN_HEADS + 10).unwrap(), 0xbdbd);
        assert_eq!(m.u16(S.chains + CHAIN_LINKS).unwrap(), u16::MAX);
        assert_eq!(m.u16(S.chains + CHAIN_LINKS + 4).unwrap(), 0);
        assert_eq!(
            (m.u32(S.queue).unwrap(), m.u32(S.queue + 4).unwrap()),
            (3, 3)
        );
        assert_eq!(m.u16(S.queue + 12).unwrap(), 2);

        let child = S.nodes + 4 + 2 * NODE_SIZE;
        assert_eq!(m.u16(child + 4).unwrap(), 1);
        assert_eq!(m.u16(child + 6).unwrap(), 1);
        assert_eq!(m.u16(child + 0xa).unwrap(), u16::MAX);
        assert_eq!(m.u16(child + 0x1c).unwrap(), 4);
        assert_eq!(m.u16(child + 0x1e).unwrap(), 0xbdbd);
        assert_eq!(m.u32(child + 0x20).unwrap(), S.nodes + CORNERS + 64);
        assert_eq!(m.f32(child + 0x24).unwrap(), 1.);
        assert_eq!(m.u32(child + 0x2c).unwrap(), 3);
    }

    #[test]
    fn only_the_paired_merge_can_certify_an_input() {
        let mut m = FlatMemory::new(0x1000, 0x20000);
        let nodes = windows();
        let inputs = || Inputs {
            bsp_epoch: 3,
            structure: 0xc00,
            start: 4,
            frustum: vec![3; FRUSTUM_SIZE],
            words: vec![(0x1060, 1)],
        };
        let mut state = State::default();
        let cpu = CpuAbi {
            esp: 0x1000,
            esi: S.chains,
            ..CpuAbi::default()
        };
        m.set_u32(cpu.esp, MERGE_RETURN).unwrap();
        m.set_u32(cpu.esp + 4, S.nodes).unwrap();
        let attempt = || Comparison {
            thread: 5,
            merge_esp: cpu.esp,
            scratch: S,
            inputs: inputs(),
            image: S.image(&nodes, true).unwrap(),
        };
        apply(&mut m, &attempt().image);
        for (thread, epoch) in [(4, 3), (5, 2)] {
            state.pending = Some(attempt());
            assert_eq!(state.finish(&m, &cpu, thread, epoch).unwrap(), None);
            assert!(state.checked.is_none());
        }
        state.pending = Some(attempt());
        assert_eq!(state.finish(&m, &cpu, 5, 3).unwrap(), Some(true));
        assert!(state.checked.as_ref() == Some(&inputs()));
        let mut changed = inputs();
        changed.words[0].1 ^= 1;
        assert!(state.checked.as_ref() != Some(&changed));
        changed = inputs();
        changed.frustum[32] ^= 1;
        assert!(state.checked.as_ref() != Some(&changed));
        m.set_u16(S.chains + CHAIN_LINKS + 4, 1).unwrap();
        state.pending = Some(attempt());
        assert_eq!(state.finish(&m, &cpu, 5, 3).unwrap(), Some(false));
        assert!(state.checked.is_none());
    }

    #[test]
    fn comparison_sees_polygon_bits_and_chain_order_with_equal_cluster_membership() {
        let mut m = FlatMemory::new(0x1000, 0x20000);
        let nodes = windows();
        let image = S.image(&nodes, true).unwrap();
        apply(&mut m, &image);
        assert!(matches_image(&m, &image).unwrap());
        let y = S.nodes + CORNERS + 4;
        assert_eq!(m.f32(y).unwrap().to_bits(), (-0.0f32).to_bits());
        m.set_f32(y, 0.).unwrap();
        assert!(!matches_image(&m, &image).unwrap());
        m.set_f32(y, -0.).unwrap();
        m.set_u16(S.chains + CHAIN_HEADS + 8, 0).unwrap();
        assert!(!matches_image(&m, &image).unwrap());
        let root = S.image(&nodes[..1], false).unwrap();
        apply(&mut m, &root);
        assert_eq!(
            (m.u32(S.queue).unwrap(), m.u32(S.queue + 4).unwrap()),
            (0, 1)
        );
        assert!(matches_image(&m, &root).unwrap());
    }
}
