//! Sphere-mode region publication (0x00132d60). Region records follow DFS
//! visit order. A separate index vector sorts them by signed cluster ID.
//! The shared render/sim sphere marks advance only when the flood ran.
use std::collections::VecDeque;
use std::sync::Mutex;

use super::*;
use scene_extract::visibility::{self, GraphCache, graph, guest, sphere, view};

const RETURN_TO_GATHER: u32 = 0x0013_2160;
const GEOMETRY_RETURN: u32 = 0x0013_219a;
const FLOOD_ACTIVE: u32 = 0x004e_7411;
const FLOOD_EPOCH: u32 = 0x004e_7414;
const STAMPS: u32 = 0x004e_7418;
const REGION_COUNT: u32 = 0xa6c;
const REGION: u32 = 0xa6e;
const REGION_BYTES: u32 = 0x1a;
const ORDER: u32 = 0x1770;

// A moving sphere can present a new input every frame. Keep a bounded
// history so recurring stationary lights can still reuse their proof.
const PROOF_LIMIT: usize = 256;

struct Read<'a>(&'a dyn GuestMemory);
impl scene_extract::GuestRead for Read<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

#[derive(PartialEq, Eq)]
enum Input {
    Outside,
    StartOnly(u16),
    Flood {
        start: u16,
        centre: [u32; 3],
        radius: u32,
        axes: sphere::Rows,
    },
}

#[derive(PartialEq, Eq)]
struct Selection {
    input: Input,
    clusters: Vec<u16>,
}

fn fresh_epoch(mem: &dyn GuestMemory, cluster_count: usize) -> Result<u32> {
    let next = mem.u32(FLOOD_EPOCH)?.wrapping_add(1);
    // The guest shares these stamps with sim queries. If a wrap or a
    // restored table makes a cluster already visited, let its walk run.
    if mem.u8(FLOOD_ACTIVE)? != 0
        || mem
            .bytes(STAMPS, cluster_count * 4)?
            .chunks_exact(4)
            .any(|bytes| bytes == next.to_le_bytes())
    {
        return Err(Refusal::Unsupported);
    }
    Ok(next)
}

fn select(
    mem: &dyn GuestMemory,
    vis: u32,
    bsp: u32,
    generation: u64,
    graph: &mut GraphCache,
) -> Result<(Selection, Option<u32>)> {
    let start = mem.u16(at(vis, guest::vis::START_CLUSTER)?)?;
    if start == u16::MAX {
        return Ok((
            Selection {
                input: Input::Outside,
                clusters: vec![],
            },
            None,
        ));
    }
    let radius = mem.f32(at(vis, guest::VIS_SPHERE_RADIUS)?)?;
    if !(radius > mem.f32(guest::SPHERE_FLOOD_MIN)?) {
        return Ok((
            Selection {
                input: Input::StartOnly(start),
                clusters: vec![start],
            },
            None,
        ));
    }
    // The polygon test projects with the currently active collision BSP.
    // The Rust graph stores the plane of the structure's collision BSP.
    if ptr(mem, view::STRUCTURE_BSP)? != ptr(mem, at(bsp, graph::layout::COLLISION_BSP)?)? {
        return Err(Refusal::Unsupported);
    }
    let invalid = |d| match d {
        visibility::Decline::Unread => Refusal::Memory,
        _ => Refusal::Layout,
    };
    let graph = graph.graph(&Read(mem), generation, bsp).map_err(invalid)?;
    let stamp = fresh_epoch(mem, graph.clusters.len())?;
    let centre = mem.vec3(at(vis, guest::VIS_SPHERE_CENTRE)?)?;
    let axes = guest::projection_rows(&Read(mem)).map_err(invalid)?;
    let clusters = sphere::sphere_clusters(graph, &axes, start, centre, radius, sphere::CAPACITY)
        .map_err(invalid)?;
    if clusters.len() > visibility::flood::MAX_REGIONS {
        return Err(Refusal::Layout);
    }
    Ok((
        Selection {
            input: Input::Flood {
                start,
                centre: centre.map(f32::to_bits),
                radius: radius.to_bits(),
                axes,
            },
            clusters,
        },
        Some(stamp),
    ))
}

fn records(base: u32, clusters: &[u16], stamp: Option<u32>) -> Result<Output> {
    let mut out = Output::default();
    out.half(at(base, REGION_COUNT)?, clusters.len() as u16);
    if let Some(stamp) = stamp {
        out.word(FLOOD_EPOCH, stamp);
        out.byte(FLOOD_ACTIVE, 0);
    }
    // The rest of each record, including the other five frustum counts,
    // has no writer in sphere mode and can still hold an earlier view.
    for (i, &id) in clusters.iter().enumerate() {
        let row = at(base, REGION + i as u32 * REGION_BYTES)?;
        out.half(row, id);
        out.half(at(row, 2)?, 0);
        if let Some(stamp) = stamp {
            out.word(STAMPS + u32::from(id) * 4, stamp);
        }
    }
    let mut indices: Vec<u32> = (0..clusters.len() as u32).collect();
    indices.sort_unstable_by_key(|&i| clusters[i as usize] as i16);
    out.array(
        at(base, ORDER)?,
        indices.into_iter().flat_map(u32::to_le_bytes).collect(),
    );
    Ok(out)
}

type Bsp = (u64, u32, u32, u32);

struct Pending {
    geometry_esp: u32,
    visibility: u32,
    regions: u32,
    selection: Selection,
    writes: Output,
}

#[derive(Default)]
struct State {
    bsp: Option<Bsp>,
    graph: GraphCache,
    verified: VecDeque<Selection>,
    pending: HashMap<u32, Pending>,
}

impl State {
    fn enter(&mut self, key: Bsp) {
        if self.bsp != Some(key) {
            *self = Self {
                bsp: Some(key),
                ..Self::default()
            };
        }
    }

    fn observe(
        &mut self,
        m: &dyn GuestMemory,
        selection: Selection,
        output: Output,
    ) -> Result<bool> {
        for Write { addr, bytes } in output.writes {
            if m.bytes(addr, bytes.slice().len())? != bytes.slice() {
                return Ok(false);
            }
        }
        if !self.verified.contains(&selection) {
            if self.verified.len() == PROOF_LIMIT {
                self.verified.pop_front();
            }
            self.verified.push_back(selection);
        }
        Ok(true)
    }
}

const STAGE: SceneList = SceneList::SphereRegions;

pub(super) fn install(b: &mut Builder) {
    let state = Arc::new(Mutex::new(State::default()));
    let before = state.clone();
    let point = &points::VIEW_SPHERE_REGIONS;
    b.raw()
        .before(point, "build sphere regions", move |c| {
            let runtime = Arc::clone(c.ctx.runtime());
            let mut m = runtime.memory();
            if c.addr != point.base || m.u32(c.cpu.esp).ok() != Some(RETURN_TO_GATHER) {
                return Ok(Flow::Continue);
            }
            let mut state = before.lock().unwrap_or_else(|e| e.into_inner());
            let thread = c.ctx.thread_id;
            state.pending.remove(&thread);
            let plan = (|| {
                fp_supported(c.cpu)?;
                if c.cpu.x87.control != 0x027f || c.cpu.eflags & (1 << 10) != 0 {
                    return Err(Refusal::Unsupported);
                }
                let vis = c.cpu.eax;
                if !matches!(vis, objects::MAIN_VIS | objects::LIGHT_VIS)
                    || ptr(&*m, guest::ACTIVE_VIS)? != vis
                {
                    return Err(Refusal::Layout);
                }
                let bsp = ptr(&*m, guest::ACTIVE_STRUCTURE)?;
                let generation = crate::native_map_work::bsp_generation();
                let mem: &dyn GuestMemory = &*m;
                let key = (
                    generation,
                    bsp,
                    mem.u32(at(bsp, graph::layout::CLUSTER_COUNT)?)?,
                    mem.u32(at(bsp, graph::layout::PORTAL_COUNT)?)?,
                );
                state.enter(key);
                let (selection, stamp) = select(&*m, vis, bsp, generation, &mut state.graph)?;
                let base = ptr(&*m, vis)?;
                let out = records(base, &selection.clusters, stamp)?;
                out.check_write(&m)?;
                Ok((selection, out, base))
            })();
            let (selection, output, base) = match plan {
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
            if !state.verified.contains(&selection) {
                // Geometry has one more stack argument than this call.
                state.pending.insert(
                    thread,
                    Pending {
                        geometry_esp: c.cpu.esp.wrapping_sub(4),
                        visibility: c.cpu.eax,
                        regions: base,
                        selection,
                        writes: output,
                    },
                );
                STAGE.count(SceneListResult::Fallback);
                return Ok(Flow::Continue);
            }
            drop(state);
            // The gather return site ignores scratch registers and flags.
            let transfer = m.ret(c.cpu, 0)?;
            output.publish(&mut m, c.addr)?;
            STAGE.count(SceneListResult::Native);
            Ok(Flow::Return(transfer))
        })
        .optional()
        .context_only()
        .last();

    b.raw()
        .observe_before(
            &points::SCENE_GEOMETRY_GATHER,
            "compare sphere regions",
            move |o| {
                let m = o.guest();
                if m.u32(o.cpu.esp).ok() != Some(GEOMETRY_RETURN) {
                    return;
                }
                let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(p) = state.pending.remove(&o.thread_id)
                    && o.cpu.esp == p.geometry_esp
                    && o.cpu.esi == p.visibility
                    && m.u32(p.visibility).ok() == Some(p.regions)
                    && state.bsp.is_some_and(|(generation, ..)| {
                        generation == crate::native_map_work::bsp_generation()
                    })
                {
                    STAGE.count(SceneListResult::Compared);
                    match state.observe(&*m, p.selection, p.writes) {
                        Ok(true) => (),
                        Ok(false) => STAGE.count(SceneListResult::Mismatch),
                        Err(_) => STAGE.count(SceneListResult::Memory),
                    }
                }
            },
        )
        .optional()
        .context_only();
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const BASE: u32 = 0x10000;

    #[test]
    fn matches_dfs_order_sorted_indices_and_shared_epoch() {
        let mut mem = FakeGuest::new();
        let ids = [5, 2, 4];
        let selection = || Selection {
            input: Input::Flood {
                start: 5,
                centre: [3; 3],
                radius: 9,
                axes: [[0, 1]; 6],
            },
            clusters: ids.to_vec(),
        };
        mem.put_u16(BASE + REGION_COUNT, 3);
        for (i, (&id, idx)) in ids.iter().zip([1, 2, 0]).enumerate() {
            mem.put_u16(BASE + REGION + REGION_BYTES * i as u32, id);
            mem.put_u32(BASE + ORDER + i as u32 * 4, idx);
            mem.put_u32(STAMPS + u32::from(id) * 4, 41);
        }
        mem.put_u32(FLOOD_EPOCH, 41);
        let mut state = State::default();
        let writes = || records(BASE, &ids, Some(41)).unwrap();
        mem.put_u16(BASE + REGION + 4, 923);
        assert!(state.observe(&mem, selection(), writes()).unwrap());
        assert!(state.verified.contains(&selection()));
        state.enter((2, 3, 7, 9));
        assert!(state.verified.is_empty());
        mem.put_u32(BASE + ORDER, 0);
        assert!(!state.observe(&mem, selection(), writes()).unwrap());
        mem.put_u32(BASE + ORDER, 1);
        mem.put_u32(STAMPS + 8, 40);
        assert!(!state.observe(&mem, selection(), writes()).unwrap());
        assert!(state.verified.is_empty());
    }

    #[test]
    fn epoch_collision_is_a_refusal_and_small_or_outside_views_do_not_advance_it() {
        let mut mem = FakeGuest::new();
        mem.put_u32(FLOOD_EPOCH, 28);
        assert_eq!(fresh_epoch(&mem, 8).unwrap(), 29);
        mem.put_u32(STAMPS + 4 * 7, 29);
        assert_eq!(fresh_epoch(&mem, 8), Err(Refusal::Unsupported));
        for ids in [&[][..], &[9][..]] {
            let writes = records(BASE, ids, None).unwrap();
            assert!(
                writes
                    .writes
                    .iter()
                    .all(|w| (BASE..BASE + ORDER + 4).contains(&w.addr))
            );
        }
    }
}
