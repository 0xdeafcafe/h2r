//! Mode 2 of the view gather. The PVS chooses an ascending list of
//! clusters, each with one window built from the same input frustum.
//! Compare the original region records on the first call for each input.
use std::sync::Mutex;

use guest_layout::entry::{Cx, EntryResult, GuestCall, Ret};
use scene_extract::visibility::{self, GraphCache, flood, guest};

use super::*;

const GATHER_RETURN: u32 = 0x0013_2157;
const GEOMETRY_RETURN: u32 = 0x0013_219a;
const WINDOW_BUILD: u32 = 0x0016_3db0;
const CAMERA_BYTES: u32 = 0x1bc;
pub(super) const REGION_COUNT: u32 = 0xa6c;
pub(super) const REGIONS: u32 = 0xa6e;
pub(super) const REGION_STRIDE: u32 = 0x1a;
pub(super) const ORDER: u32 = 0x1770;
pub(super) const WINDOW_COUNT: u32 = 0x1970;
pub(super) const WINDOWS: u32 = 0x1974;
const WINDOW_STRIDE: u32 = 0x108;

// The window builder writes the view index, rectangle, then corners,
// bounds, planes and edge vectors. The gaps keep their old contents.
const WINDOW_FIELDS: [(u32, usize); 3] = [(0, 4), (0x10, 0x10), (0x24, 0x108 - 0x24)];

struct Read<'a>(&'a dyn GuestMemory);

impl scene_extract::GuestRead for Read<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Selection {
    start: u32,
    open: flood::OpenPortals,
    clusters: Vec<u16>,
}

fn select(
    mem: &dyn GuestMemory,
    graph: &mut GraphCache,
    bsp: u32,
    epoch: u64,
    start: u32,
) -> Result<Selection> {
    let mut open = flood::OpenPortals::default();
    let clusters = if start == NONE {
        Vec::new()
    } else {
        let invalid = |why| match why {
            visibility::Decline::Unread => Refusal::Memory,
            _ => Refusal::Layout,
        };
        let graph = graph.graph(&Read(mem), epoch, bsp).map_err(invalid)?;
        let id = u16::try_from(start).map_err(|_| Refusal::Layout)?;
        let cluster = graph.clusters.get(usize::from(id)).ok_or(Refusal::Layout)?;
        if cluster.computed_pvs {
            open = guest::open_portals(&Read(mem), graph.portals.len()).map_err(invalid)?;
        }
        flood::potentially_visible(graph, id, &open)
            .map_err(invalid)?
            .clusters
    };
    Ok(Selection {
        start,
        open,
        clusters,
    })
}

struct Plan {
    selection: Selection,
    camera: u32,
    output: u32,
}

impl Plan {
    fn matches(&self, mem: &dyn GuestMemory) -> Result<bool> {
        let base = self.output;
        let count = self.selection.clusters.len() as u16;
        if mem.u16(base)? != 1
            || mem.u16(at(base, REGION_COUNT)?)? != count
            || mem.u16(at(base, WINDOW_COUNT)?)? != count
        {
            return Ok(false);
        }
        if mem.bytes(self.camera, CAMERA_BYTES as usize)?
            != mem.bytes(at(base, 4)?, CAMERA_BYTES as usize)?
        {
            return Ok(false);
        }
        let rect = at(self.camera, 0xa0)?;
        let has_window = count > 1
            && mem.f32(at(rect, 4)?)? > mem.f32(rect)?
            && mem.f32(at(rect, 12)?)? > mem.f32(at(rect, 8)?)?;
        let first = at(base, WINDOWS)?;
        for (i, &id) in self.selection.clusters.iter().enumerate() {
            let i = i as u32;
            if has_window && i > 0 {
                for (offset, len) in WINDOW_FIELDS {
                    if mem.bytes(at(first, offset)?, len)?
                        != mem.bytes(at(first, i * WINDOW_STRIDE + offset)?, len)?
                    {
                        return Ok(false);
                    }
                }
            }
            let r = at(base, REGIONS + i * REGION_STRIDE)?;
            if mem.u16(r)? != id
                || mem.u16(at(r, 2)?)? != 1
                || mem.u16(at(r, 0xe)?)? != i as u16
                || mem.u32(at(base, ORDER + i * 4)?)? != i
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn check(&self, mem: &Guest) -> Result<()> {
        // Source and destination may be the very same frustum. A partial
        // overlap requires the original word-by-word copy.
        let base = self.output;
        let count = self.selection.clusters.len() as u32;
        let end = at(base, WINDOWS + count * WINDOW_STRIDE)?;
        let camera_end = at(self.camera, CAMERA_BYTES)?;
        if self.camera != at(base, 4)? && self.camera < end && camera_end > base {
            return Err(Refusal::Layout);
        }
        mem.check(self.camera, CAMERA_BYTES as usize, Access::Read, 1)
            .map_err(|_| Refusal::Memory)?;
        mem.check(base, (end - base) as usize, Access::Write, 1)
            .map_err(|_| Refusal::Memory)?;
        Ok(())
    }

    async fn publish(self, cx: Cx) -> EntryResult {
        let base = self.output;
        let count = self.selection.clusters.len() as u16;
        cx.write(|m| -> std::result::Result<(), MemFault> {
            if self.camera != base + 4 {
                let camera = m.bytes(self.camera, CAMERA_BYTES as usize)?;
                m.write(base + 4, &camera)?;
            }
            m.set_u16(base, 1)?;
            m.set_u16(base + REGION_COUNT, count)?;
            m.set_u16(base + WINDOW_COUNT, count)?;
            for (i, &id) in self.selection.clusters.iter().enumerate() {
                let i = i as u32;
                let r = base + REGIONS + i * REGION_STRIDE;
                m.set_u16(r, id)?;
                m.set_u16(r + 2, 1)?;
                m.set_u16(r + 0xe, i as u16)?;
                m.set_u32(base + ORDER + i * 4, i)?;
            }
            Ok(())
        })?;
        if count > 0 {
            let first = base + WINDOWS;
            let result = cx
                .call(
                    GuestCall::new(WINDOW_BUILD)
                        .ecx(self.camera + 0xa0)
                        .edi(first)
                        .args([self.camera, 0]),
                )
                .await?;
            // An empty rectangle exits the leaf without writing. For a
            // valid rectangle the input and view index are identical for
            // every selected region. Its diagnostic flag clears at most once.
            if result.eax as u8 != 0 && count > 1 {
                cx.write(|m| -> std::result::Result<(), MemFault> {
                    for (offset, len) in WINDOW_FIELDS {
                        let bytes = m.bytes(first + offset, len)?;
                        for i in 1..u32::from(count) {
                            m.write(first + i * WINDOW_STRIDE + offset, &bytes)?;
                        }
                    }
                    Ok(())
                })?;
            }
        }
        // GATHER_RETURN discards the volatile registers.
        Ok(Ret::new(0, 4))
    }
}

#[derive(Default)]
struct State {
    bsp: Option<(u64, u32)>,
    graph: GraphCache,
    verified: HashMap<u32, Selection>,
    pending: HashMap<u32, (u32, Plan)>,
}

impl State {
    fn enter_bsp(&mut self, key: (u64, u32)) {
        if self.bsp != Some(key) {
            *self = Self {
                bsp: Some(key),
                ..Self::default()
            };
        }
    }

    fn verified(&self, row: &Selection) -> bool {
        self.verified.get(&row.start) == Some(row)
    }

    fn observe(&mut self, pending: Plan, mem: &dyn GuestMemory) -> Result<bool> {
        let equal = pending.matches(mem)?;
        if equal {
            self.verified
                .insert(pending.selection.start, pending.selection);
        } else {
            self.verified.remove(&pending.selection.start);
        }
        Ok(equal)
    }
}

pub(super) fn install(b: &mut Builder) {
    let state = Arc::new(Mutex::new(State::default()));
    let entry_state = Arc::clone(&state);
    b.raw()
        .before(&points::VIEW_PVS_REGIONS, "build PVS regions", move |c| {
            let runtime = Arc::clone(c.ctx.runtime());
            let plan = {
                let mem = runtime.memory();
                if c.addr != points::VIEW_PVS_REGIONS.base
                    || mem.u32(c.cpu.esp).ok() != Some(GATHER_RETURN)
                {
                    return Ok(Flow::Continue);
                }
                let mut state = entry_state.lock().unwrap_or_else(|e| e.into_inner());
                state.pending.remove(&c.ctx.thread_id);
                let candidate = (|| {
                    if c.cpu.eflags & (1 << 10) != 0 {
                        return Err(Refusal::Unsupported);
                    }
                    let bsp = ptr(&*mem, guest::ACTIVE_STRUCTURE)?;
                    let epoch = crate::native_map_work::bsp_generation();
                    state.enter_bsp((epoch, bsp));
                    let selection = select(&*mem, &mut state.graph, bsp, epoch, c.cpu.eax)?;
                    let plan = Plan {
                        selection,
                        camera: stack(&*mem, c.cpu, 4)?,
                        output: c.cpu.ebx,
                    };
                    plan.check(&mem)?;
                    Ok(plan)
                })();
                let plan = match candidate {
                    Ok(plan) => plan,
                    Err(why) => {
                        SceneList::PvsRegions.count(SceneListResult::Fallback);
                        SceneList::PvsRegions.count(match why {
                            Refusal::Memory => SceneListResult::Memory,
                            Refusal::Layout => SceneListResult::Layout,
                            _ => SceneListResult::Unsupported,
                        });
                        return Ok(Flow::Continue);
                    }
                };
                if !state.verified(&plan.selection) {
                    state.pending.insert(c.ctx.thread_id, (c.cpu.esp, plan));
                    SceneList::PvsRegions.count(SceneListResult::Fallback);
                    return Ok(Flow::Continue);
                }
                plan
            };
            if !plan.selection.clusters.is_empty()
                && !runtime.read_dispatch(|dispatch| {
                    dispatch.has_generated(WINDOW_BUILD)
                        && dispatch.override_name(WINDOW_BUILD).is_none()
                })
            {
                SceneList::PvsRegions.count(SceneListResult::Fallback);
                SceneList::PvsRegions.count(SceneListResult::Layout);
                return Ok(Flow::Continue);
            }
            c.answer_render_async(move |cx| {
                Box::pin(async move {
                    let ret = plan.publish(cx).await?;
                    SceneList::PvsRegions.count(SceneListResult::Native);
                    Ok(ret)
                })
            })
        })
        .optional()
        .context_only();
    b.raw()
        .observe_before(
            &points::SCENE_GEOMETRY_GATHER,
            "compare PVS regions",
            move |o| {
                if o.cpu.esi != objects::MAIN_VIS {
                    return;
                }
                let mem = o.guest();
                if mem.u32(o.cpu.esp).ok() != Some(GEOMETRY_RETURN) {
                    return;
                }
                let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                // Both calls have one stack argument and originate in the
                // same frame. The caller only advances its placement epoch
                // between the region producer and this geometry gather.
                if let Some((esp, plan)) = state.pending.remove(&o.thread_id)
                    && esp == o.cpu.esp
                    && mem.u32(objects::MAIN_VIS).ok() == Some(plan.output)
                    && state
                        .bsp
                        .is_some_and(|(epoch, _)| epoch == crate::native_map_work::bsp_generation())
                {
                    SceneList::PvsRegions.count(SceneListResult::Compared);
                    match state.observe(plan, &*mem) {
                        Ok(true) => (),
                        Ok(false) => SceneList::PvsRegions.count(SceneListResult::Mismatch),
                        Err(_) => SceneList::PvsRegions.count(SceneListResult::Memory),
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
    use guest_layout::entry::{Regs, testing};
    use guest_layout::testing::FakeGuest;

    const BASE: u32 = 0x2000;
    const CAMERA: u32 = 0x1000;

    fn plan(ids: &[u16]) -> Plan {
        Plan {
            selection: Selection {
                start: 2,
                open: flood::OpenPortals(vec![3]),
                clusters: ids.to_vec(),
            },
            camera: CAMERA,
            output: BASE,
        }
    }

    fn published(ids: &[u16]) -> testing::Run {
        publish_view(ids, true)
    }

    fn publish_view(ids: &[u16], valid_rect: bool) -> testing::Run {
        let mut mem = FakeGuest::new();
        mem.put(CAMERA, &vec![0x6b; CAMERA_BYTES as usize]);
        mem.put(BASE, &vec![0xa7; (WINDOWS + 5 * WINDOW_STRIDE) as usize]);
        mem.put_f32(CAMERA + 0xa0, -1.0)
            .put_f32(CAMERA + 0xa4, if valid_rect { 1.0 } else { -1.0 });
        mem.put_f32(CAMERA + 0xa8, -1.5).put_f32(CAMERA + 0xac, 1.5);
        mem.put_u32(BASE + WINDOWS + WINDOW_STRIDE + 4, 0x1234);
        let ids = ids.to_vec();
        testing::run(
            &move |cx| Box::pin(plan(&ids).publish(cx)),
            mem,
            Regs::default(),
            |call, m| {
                // An invented window helper writes one field and leaves its padding.
                assert_eq!(call.target, WINDOW_BUILD);
                assert_eq!(call.ecx, Some(CAMERA + 0xa0));
                assert_eq!(call.args, [CAMERA, 0]);
                if !valid_rect {
                    return Regs::default();
                }
                m.set_u32(call.edi.unwrap(), 177).unwrap();
                Regs {
                    eax: 1,
                    ..Regs::default()
                }
            },
        )
    }

    #[test]
    fn both_pvs_hooks_run_on_the_render_executor() {
        use hooks::{Registry, plan::Base};
        let mut registry = Registry::new(Switches::parse("").unwrap());
        registry.add(&NativeSceneLists).unwrap();
        let plan = registry.build(&Base).unwrap();
        for address in [
            points::VIEW_PVS_REGIONS.base,
            points::SCENE_GEOMETRY_GATHER.base,
        ] {
            let chain = &plan.chains[&address];
            assert!(chain.render_thread().is_ok());
        }
    }

    #[test]
    fn publishes_each_selected_cluster_with_one_window_and_leaves_spare_slots_alone() {
        let run = published(&[1, 4, 9]);
        assert_eq!(run.result, Ok(Ret::new(0, 4)));
        assert_eq!(run.calls.len(), 1);
        assert_eq!(run.calls[0].edi, Some(BASE + WINDOWS));
        let m = run.guest;
        assert_eq!(
            m.bytes(BASE + 4, CAMERA_BYTES as usize).unwrap(),
            m.bytes(CAMERA, CAMERA_BYTES as usize).unwrap()
        );
        assert_eq!(m.u32(BASE + WINDOWS + WINDOW_STRIDE + 4).unwrap(), 0x1234);
        assert_eq!(m.u16(BASE + 2).unwrap(), 0xa7a7);
        assert!(plan(&[1, 4, 9]).matches(&m).unwrap());
        for i in 0..3 {
            assert_eq!(
                m.u16(BASE + REGIONS + i * REGION_STRIDE + 4).unwrap(),
                0xa7a7
            );
            assert_eq!(m.u32(BASE + WINDOWS + i * WINDOW_STRIDE).unwrap(), 177);
        }
        assert_eq!(m.u16(BASE + REGIONS + 3 * REGION_STRIDE).unwrap(), 0xa7a7);
        assert_eq!(m.u32(BASE + ORDER + 3 * 4).unwrap(), 0xa7a7a7a7);
        let invalid = publish_view(&[1, 4], false);
        assert_eq!(invalid.calls.len(), 1);
        assert!(plan(&[1, 4]).matches(&invalid.guest).unwrap());
        assert_eq!(invalid.guest.u32(BASE + WINDOWS).unwrap(), 0xa7a7a7a7);
        assert_eq!(
            invalid
                .guest
                .u32(BASE + WINDOWS + WINDOW_STRIDE + 4)
                .unwrap(),
            0x1234
        );
        let empty = published(&[]);
        assert_eq!(empty.result, Ok(Ret::new(0, 4)));
        assert!(empty.calls.is_empty());
        assert!(plan(&[]).matches(&empty.guest).unwrap());
    }

    #[test]
    fn an_equal_order_and_window_table_admit_only_the_same_inputs_in_this_bsp() {
        let mut state = State::default();
        state.enter_bsp((9, 27));
        let mut mem = published(&[1, 4]).guest;
        let selection = plan(&[1, 4]).selection;
        assert!(!state.verified(&selection));
        assert!(!state.observe(plan(&[4, 1]), &mem).unwrap());
        assert!(state.observe(plan(&[1, 4]), &mem).unwrap());
        assert!(state.verified(&selection));
        let mut closed = selection.clone();
        closed.open.0[0] = 2;
        assert!(!state.verified(&closed));
        mem.set_u32(BASE + ORDER + 4, 0).unwrap();
        assert!(!state.observe(plan(&[1, 4]), &mem).unwrap());
        assert!(!state.verified(&selection));
        mem.set_u32(BASE + ORDER + 4, 1).unwrap();
        mem.set_u16(BASE + REGIONS + 0xe, 1).unwrap();
        assert!(!state.observe(plan(&[1, 4]), &mem).unwrap());
        mem.set_u16(BASE + REGIONS + 0xe, 0).unwrap();
        assert!(state.observe(plan(&[1, 4]), &mem).unwrap());
        mem.set_u32(BASE + WINDOWS + WINDOW_STRIDE + 0x78, 0x55)
            .unwrap();
        assert!(
            !state.observe(plan(&[1, 4]), &mem).unwrap(),
            "a different clipping plane cannot be replicated"
        );
        mem.set_u32(BASE + WINDOWS + WINDOW_STRIDE + 0x78, 0xa7a7a7a7)
            .unwrap();
        assert!(state.observe(plan(&[1, 4]), &mem).unwrap());
        state.enter_bsp((10, 27));
        assert!(!state.verified(&selection));
    }
}
