//! Publish the current portal pass's regions and windows. The caller
//! sorts the resulting ordinal table after all its frusta have run.
use super::*;
use scene_extract::visibility::{
    flood, graph, guest,
    window_record::{SIZE, WindowInputs},
};
use std::sync::Mutex;

const STAGE: SceneList = SceneList::PortalRegions;
const VALIDATE_PLANES: u32 = 0x0047_ff86;
const NODE_SIZE: u32 = 0x6c;
const HEADS: u32 = 0x40;
const LINKS: u32 = 0x440;

struct Read<'a>(&'a dyn GuestMemory);

impl scene_extract::GuestRead for Read<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

#[derive(Clone, Copy)]
struct Frame {
    output: u32,
    nodes: u32,
    chains: u32,
    lookup: u32,
    camera: u32,
    bsp: u32,
}

impl Frame {
    fn read(m: &dyn GuestMemory, cpu: &CpuAbi) -> Result<Self> {
        fp_supported(cpu)?;
        if cpu.x87.control != 0x027f || cpu.x87.tag != u16::MAX || m.u8(VALIDATE_PLANES)? != 0 {
            return Err(Refusal::Unsupported);
        }
        let vis = objects::MAIN_VIS;
        if ptr(m, guest::ACTIVE_VIS)? != vis
            || m.u32(at(vis, objects::VIEW_KIND)?)? != 0
            || m.u32(at(vis, guest::vis::MODE)?)? != 0
            || m.u32(at(cpu.ebp, 8)?)? != 1
            || stack(m, cpu, 12)? != 0
        {
            return Err(Refusal::Unsupported);
        }
        let f = Self {
            output: stack(m, cpu, 16)?,
            camera: stack(m, cpu, 8)?,
            lookup: stack(m, cpu, 4)?,
            nodes: cpu.ebx,
            chains: cpu.edi,
            bsp: ptr(m, guest::ACTIVE_STRUCTURE)?,
        };
        let stack_base = at(cpu.esp, 20)?;
        if f.nodes != at(stack_base, 0x100c0)?
            || f.chains != at(stack_base, 0x220)?
            || f.lookup != at(stack_base, 0x860)?
            || f.camera != ptr(m, at(cpu.ebp, 0xc)?)?
            || f.output != ptr(m, vis)?
            || f.output != ptr(m, at(cpu.ebp, 0x14)?)?
            || m.u16(f.output)? != 1
            || m.u16(at(f.output, regions::REGION_COUNT)?)? != 0
            || m.u16(at(f.output, regions::WINDOW_COUNT)?)? != 0
        {
            return Err(Refusal::Layout);
        }
        Ok(f)
    }

    fn node(self, index: usize) -> Result<u32> {
        at(self.nodes, 4 + index as u32 * NODE_SIZE)
    }
}

// In the guest the pool is walked by ascending node index. A cluster is
// opened only at its chain head, i.e. its last node in pool order. Its
// surviving windows are visited newest first, following the links.
#[derive(Debug, PartialEq, Eq)]
struct Path {
    cluster: u16,
    window: Option<Window>,
}

#[derive(Debug, PartialEq, Eq)]
struct Window {
    inputs: WindowInputs,
    plane: u32,
    depth: u32,
}

#[derive(PartialEq, Eq)]
struct Input {
    generation: u64,
    paths: Vec<Path>,
}

struct Plan {
    input: Input,
    output: Output,
}

fn plan(m: &dyn GuestMemory, frame: Frame, generation: u64) -> Result<Plan> {
    let count = m.u32(frame.nodes)? as usize;
    let clusters = m.u32(at(frame.bsp, graph::layout::CLUSTER_COUNT)?)? as usize;
    let portals = m.u32(at(frame.bsp, graph::layout::PORTAL_COUNT)?)?;
    if count > flood::MAX_NODES
        || clusters > graph::MAX_CLUSTERS
        || portals as usize > graph::MAX_PORTALS
    {
        return Err(Refusal::Layout);
    }
    let mut paths = Vec::with_capacity(count);
    let mut groups = vec![Vec::new(); clusters];
    for i in 0..count {
        let n = frame.node(i)?;
        let cluster = m.u16(at(n, 8)?)?;
        if m.u32(n)? != 0 || m.u16(at(n, 0xa)?)? != u16::MAX {
            return Err(Refusal::Layout);
        }
        let group = groups
            .get_mut(usize::from(cluster))
            .ok_or(Refusal::Layout)?;
        let prev = group.last().map_or(u16::MAX, |&index| index as u16);
        if m.u16(at(frame.chains, LINKS + 2 * i as u32)?)? != prev {
            return Err(Refusal::Layout);
        }
        group.push(i);
        let window = if m.u32(at(n, 0x28)?)? == NONE {
            let portal = u32::from(m.u16(at(n, 6)?)?);
            let plane = if portal == u32::from(u16::MAX) {
                NONE
            } else {
                if portal >= portals {
                    return Err(Refusal::Layout);
                }
                let table = ptr(m, at(frame.bsp, graph::layout::PORTALS)?)?;
                m.u32(at(
                    table,
                    portal * graph::layout::PORTAL_STRIDE + graph::layout::PORTAL_PLANE,
                )?)?
            };
            let inputs = WindowInputs::read(&Read(m), frame.camera, at(n, 0xc)?, 0).map_err(
                |why| match why {
                    scene_extract::visibility::Decline::Unread => Refusal::Memory,
                    _ => Refusal::Unsupported,
                },
            )?;
            Some(Window {
                inputs,
                depth: m.u32(at(n, 0x24)?)?,
                plane,
            })
        } else {
            None
        };
        paths.push(Path { cluster, window });
    }

    let mut output = Output::default();
    let mut nregions = 0;
    let mut nwindows = 0;
    for (head, path) in paths.iter().enumerate() {
        let cluster = u32::from(path.cluster);
        let group = &groups[cluster as usize];
        if group.last() != Some(&head) {
            continue;
        }
        if m.u32(at(frame.chains, 4 * (cluster / 32))?)? & (1 << (cluster % 32)) == 0
            || u32::from(m.u16(at(frame.chains, HEADS + 2 * cluster)?)?) != head as u32
            || m.u32(at(frame.lookup, 4 * cluster)?)? != NONE
            || nregions >= flood::MAX_REGIONS as u32
        {
            return Err(Refusal::Layout);
        }
        let mut row = [0; regions::REGION_STRIDE as usize];
        row[..2].copy_from_slice(&path.cluster.to_le_bytes());
        row[0xe..].fill(0xff);
        let mut admitted = 0u16;
        for &i in group.iter().rev() {
            let Some(w) = &paths[i].window else {
                continue;
            };
            let slot = at(frame.output, regions::WINDOWS + nwindows * SIZE as u32)?;
            let index = nwindows as u16;
            // The original reserves a slot even for an empty rectangle;
            // it links the node and region only when the helper returns AL=1.
            nwindows += 1;
            let mut image = [0; SIZE];
            if !w
                .inputs
                .build(&mut image)
                .map_err(|_| Refusal::Unsupported)?
            {
                continue;
            }
            image[4..8].copy_from_slice(&nregions.to_le_bytes());
            image[8..12].copy_from_slice(&w.plane.to_le_bytes());
            image[0x20..0x24].copy_from_slice(&w.depth.to_le_bytes());
            output.array(slot, image[..12].to_vec());
            output.array(at(slot, 0x10)?, image[0x10..].to_vec());
            output.half(at(frame.node(i)?, 0xa)?, index);
            if admitted == 0 {
                row[0xe..0x10].copy_from_slice(&index.to_le_bytes());
            }
            admitted += 1;
        }
        row[2..4].copy_from_slice(&admitted.to_le_bytes());
        output.array(
            at(
                frame.output,
                regions::REGIONS + nregions * regions::REGION_STRIDE,
            )?,
            row.to_vec(),
        );
        output.word(at(frame.output, regions::ORDER + 4 * nregions)?, nregions);
        output.word(at(frame.lookup, 4 * cluster)?, nregions);
        nregions += 1;
    }
    output.half(at(frame.output, regions::REGION_COUNT)?, nregions as u16);
    output.half(at(frame.output, regions::WINDOW_COUNT)?, nwindows as u16);
    Ok(Plan {
        input: Input { generation, paths },
        output,
    })
}

#[derive(Default)]
struct State {
    verified: Option<Input>,
    pending: HashMap<u32, (u32, u32, Plan)>,
}

pub(super) fn install(b: &mut Builder) {
    let state = Arc::new(Mutex::new(State::default()));
    let compare = Arc::clone(&state);
    let ready = &points::scene_effects::PORTAL_REGIONS_READY;
    b.raw()
        .observe_before(ready, "compare portal regions", move |o| {
            let mem = o.guest();
            let mut state = compare.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((esp, ebp, plan)) = state.pending.remove(&o.thread_id)
                && (esp, ebp) == (o.cpu.esp, o.cpu.ebp)
                && plan.input.generation == crate::native_map_work::bsp_generation()
            {
                STAGE.count(SceneListResult::Compared);
                if plan.output.writes.iter().all(|w| {
                    mem.bytes(w.addr, w.bytes.slice().len()).ok().as_deref()
                        == Some(w.bytes.slice())
                }) {
                    state.verified = Some(plan.input);
                } else {
                    STAGE.count(SceneListResult::Mismatch);
                    state.verified = None;
                }
            }
        })
        .context_only()
        .optional();
    b.raw()
        .before(
            &points::PORTAL_WINDOW_REGIONS,
            "build portal regions",
            move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                if c.addr != points::PORTAL_WINDOW_REGIONS.base
                    || mem.u32(c.cpu.esp).ok() != Some(ready.base)
                {
                    return Ok(Flow::Continue);
                }
                let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                state.pending.remove(&c.ctx.thread_id);
                let candidate = Frame::read(&*mem, c.cpu)
                    .and_then(|frame| plan(&*mem, frame, crate::native_map_work::bsp_generation()))
                    .and_then(|p| p.output.check_write(&mem).map(|()| p));
                let p = match candidate {
                    Ok(p) => p,
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
                if state.verified.as_ref() != Some(&p.input) {
                    state
                        .pending
                        .insert(c.ctx.thread_id, (c.cpu.esp.wrapping_add(20), c.cpu.ebp, p));
                    STAGE.count(SceneListResult::Fallback);
                    return Ok(Flow::Continue);
                }
                let ret = mem.ret(c.cpu, 16)?;
                p.output.publish(&mut mem, c.addr)?;
                STAGE.count(SceneListResult::Native);
                Ok(Flow::Return(ret))
            },
        )
        .context_only()
        .optional()
        .last();
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const FRAME: Frame = Frame {
        output: 0x8000,
        nodes: 0x2000,
        chains: 0x4000,
        lookup: 0x5000,
        camera: 0x6000,
        bsp: 0x1000,
    };

    fn fixture() -> FakeGuest {
        let mut m = FakeGuest::new();
        let f = FRAME;
        let bounds = 0x7300;
        let portals = 0x7000;
        m.put_u32(guest::FRUSTUM_BOUNDS, bounds).map(bounds, 24);
        m.put_f32(guest::FRUSTUM_UNIT, 1.)
            .put_f32(guest::FRUSTUM_NEGATIVE_UNIT, -1.);
        m.put_f32(guest::NORMALIZE_EPSILON, 0.001);
        m.put_f32(f.camera + 0x88, 12.).put_u8(f.camera + 0x6c, 0);
        for idx in [0, 1, 5, 9] {
            m.put_f32(f.camera + 0x38 + idx * 4, 1.);
        }
        m.put_u32(f.bsp + graph::layout::CLUSTER_COUNT, 5);
        m.put_u32(f.bsp + graph::layout::PORTAL_COUNT, 3);
        m.put_u32(f.bsp + graph::layout::PORTALS, portals);
        for portal in 0..3 {
            m.put_u32(
                portals + portal * graph::layout::PORTAL_STRIDE + graph::layout::PORTAL_PLANE,
                63 + portal,
            );
        }
        m.put(f.output, &vec![0xa5; 0x2000]);
        m.put(f.lookup, &[0xff; 20]);
        m.put_u32(f.nodes, 5).put_u32(f.chains, 0b10011);
        let mut heads = [u16::MAX; 5];
        for (i, (cluster, portal, into)) in [
            (4, u16::MAX, NONE),
            (1, 2, NONE),
            (4, 1, NONE),
            (0, 2, NONE),
            (1, 0, 1),
        ]
        .into_iter()
        .enumerate()
        {
            let n = f.node(i).unwrap();
            m.put_u32(n, 0)
                .put_u16(n + 8, cluster)
                .put_u16(n + 6, portal);
            m.put_u16(n + 0xa, u16::MAX).put_u32(n + 0x28, into);
            m.put_f32(n + 0x24, i as f32 * 0.25);
            m.put_f32(n + 0xc, -1.)
                .put_f32(n + 0x10, if i == 3 { -1. } else { 1. });
            m.put_f32(n + 0x14, -2.).put_f32(n + 0x18, 2.);
            let old_head = std::mem::replace(&mut heads[usize::from(cluster)], i as u16);
            m.put_u16(f.chains + LINKS + i as u32 * 2, old_head);
            m.put_u16(f.chains + HEADS + u32::from(cluster) * 2, i as u16);
        }
        m
    }

    #[test]
    fn allocates_by_head_order_and_preserves_window_provenance_and_empty_slots() {
        let f = FRAME;
        let mut m = fixture();
        let proposal = plan(&m, f, 15).unwrap();
        for w in &proposal.output.writes {
            m.write(w.addr, w.bytes.slice()).unwrap();
        }
        assert_eq!(m.u16(f.output + regions::REGION_COUNT), Ok(3));
        assert_eq!(m.u16(f.output + regions::WINDOW_COUNT), Ok(4));
        // These are the raw rows before the caller sorts its index.
        for (i, (cluster, count, first)) in [(4, 2, 0), (0, 0, u16::MAX), (1, 1, 3)]
            .into_iter()
            .enumerate()
        {
            let i = i as u32;
            let row = f.output + regions::REGIONS + i * regions::REGION_STRIDE;
            assert_eq!(m.u16(row), Ok(cluster));
            assert_eq!(m.u16(row + 2), Ok(count));
            assert_eq!(m.u16(row + 0xe), Ok(first));
            assert_eq!(m.u32(f.output + regions::ORDER + 4 * i), Ok(i));
            assert_eq!(m.u32(f.lookup + 4 * u32::from(cluster)), Ok(i));
            assert_eq!(m.bytes(row + 4, 10).unwrap(), [0; 10]);
            assert_eq!(m.bytes(row + 0x10, 10).unwrap(), [0xff; 10]);
        }
        for (index, path, row, plane, depth) in [
            (0, 2, 0, 64, 0.5f32),
            (1, 0, 0, NONE, 0.0),
            (3, 1, 2, 65, 0.25),
        ] {
            let slot = f.output + regions::WINDOWS + SIZE as u32 * index;
            assert_eq!(m.u32(slot), Ok(0));
            assert_eq!(m.u32(slot + 4), Ok(row));
            assert_eq!(m.u32(slot + 8), Ok(plane));
            assert_eq!(m.u32(slot + 0xc), Ok(0xa5a5a5a5));
            assert_eq!(m.u32(slot + 0x20), Ok(depth.to_bits()));
            assert_eq!(m.u16(f.node(path).unwrap() + 0xa), Ok(index as u16));
        }
        assert_eq!(
            m.bytes(f.output + regions::WINDOWS + 2 * SIZE as u32, SIZE)
                .unwrap(),
            vec![0xa5; SIZE]
        );
        assert_eq!(m.u16(f.node(3).unwrap() + 0xa), Ok(u16::MAX));
        assert_eq!(m.u16(f.node(4).unwrap() + 0xa), Ok(u16::MAX));
    }

    #[test]
    fn changed_depth_invalidates_parity_and_a_broken_chain_refuses_before_publication() {
        let f = FRAME;
        let mut m = fixture();
        let first = plan(&m, f, 1).unwrap();
        m.put_f32(f.node(2).unwrap() + 0x24, 88.);
        assert!(plan(&m, f, 1).unwrap().input != first.input);
        m.put_u16(f.chains + LINKS + 4 * 2, 2);
        assert!(matches!(plan(&m, f, 1), Err(Refusal::Layout)));
        assert_eq!(m.u16(f.output + regions::REGION_COUNT), Ok(0xa5a5));
        assert_eq!(m.u32(f.lookup + 4 * 4), Ok(NONE));
    }
}
