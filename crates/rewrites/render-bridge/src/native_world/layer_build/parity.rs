//! The complete main-view geometry list is built on the side first. Its
//! count, every sorted slot, and the final rejected scratch slot must match
//! at the dispatch loop. Reuse requires the same map, view, caller frame and
//! input bytes, including the inherited sort fields. Foreign callbacks stay
//! with the prefix walk since they may append to the source list.
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use guest_layout::memory::{GuestMemory, MemFault};
use runtime::{CpuAbi, TransferAbi};
use frame_scene::{ViewKind, VisibilitySource, scene::ClusterGather};
use scene_extract::layout::{objects, skip};

use super::{BUILD_CALLER, Build, SLOT, plan};
use scene_extract::layer_sort;

struct Read {
    at: u32,
    len: u8,
    bytes: [u8; 4],
}

struct Inputs<'a> {
    source: &'a dyn GuestMemory,
    reads: RefCell<Vec<Read>>,
}

impl scene_extract::GuestRead for Inputs<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.u32(addr).ok()
    }
}

impl GuestMemory for Inputs<'_> {
    fn read(&self, at: u32, bytes: &mut [u8]) -> Result<(), MemFault> {
        self.source.read(at, bytes)?;
        let mut reads = self.reads.borrow_mut();
        for (i, chunk) in bytes.chunks(4).enumerate() {
            let mut value = [0; 4];
            value[..chunk.len()].copy_from_slice(chunk);
            reads.push(Read { at: at + (i * 4) as u32, len: chunk.len() as u8, bytes: value });
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, bytes: &[u8]) -> Result<(), MemFault> {
        Err(MemFault { addr, len: bytes.len() as u32, write: true })
    }
}

struct Span {
    at: u32,
    bytes: Vec<u8>,
}

impl Inputs<'_> {
    fn spans(self) -> Vec<Span> {
        let mut reads = self.reads.into_inner();
        reads.sort_unstable_by_key(|r| r.at);
        let mut spans: Vec<Span> = Vec::new();
        for read in reads {
            let bytes = &read.bytes[..usize::from(read.len)];
            if let Some(last) = spans.last_mut() {
                let start = (read.at - last.at) as usize;
                if start <= last.bytes.len() {
                    let overlap = last.bytes.len() - start;
                    if overlap < bytes.len() { last.bytes.extend_from_slice(&bytes[overlap..]); }
                    continue;
                }
            }
            spans.push(Span { at: read.at, bytes: bytes.to_vec() });
        }
        spans
    }
}

struct Candidate {
    inputs: Vec<Span>,
    build: Build,
    esp: u32,
    view: u32,
    gather: ClusterGather,
}

impl Candidate {
    fn inputs_hold(&self, m: &dyn GuestMemory, sp: u32) -> bool {
        if self.esp != sp { return false; }
        let mut scratch = [0; 256];
        self.inputs.iter().all(|span| {
            span.bytes.chunks(scratch.len()).enumerate().all(|(i, bytes)| {
                let read = &mut scratch[..bytes.len()];
                m.read(span.at + (i * 256) as u32, read).is_ok() && read == bytes
            })
        })
    }

    fn output_matches(&self, m: &dyn GuestMemory, cpu: &CpuAbi) -> bool {
        cpu.esp == self.esp + 20
            && cpu.edi == self.build.list && cpu.ebx == self.build.layer
            && cpu.eax as usize == self.build.count
            && m.bytes(self.build.buffer, self.build.entries.len() * SLOT)
                .is_ok_and(|bytes| bytes == self.build.entries.as_flattened())
    }
}

#[derive(Default)]
struct Parity {
    generation: u64,
    // The most recently proven list per local view and layer.
    exact: HashMap<(u32, u32), Candidate>,
    pending: Option<Candidate>,
}

thread_local! {
    static PARITY: RefCell<Parity> = RefCell::default();
}

static EQUAL: AtomicU64 = AtomicU64::new(0);
static DIFFER: AtomicU64 = AtomicU64::new(0);
static TAKEN: AtomicU64 = AtomicU64::new(0);

pub(crate) fn counters() -> [(&'static str, u64); 3] {
    [
        ("scene_geometry_lists_equal", EQUAL.load(Relaxed)),
        ("scene_geometry_lists_differ", DIFFER.load(Relaxed)),
        ("scene_geometry_lists_taken", TAKEN.load(Relaxed)),
    ]
}

pub(in crate::native_world) fn verifying() -> bool {
    PARITY.with_borrow(|p| p.pending.is_some())
}

pub(in crate::native_world) fn finish(m: &dyn GuestMemory, cpu: &CpuAbi) {
    PARITY.with_borrow_mut(|parity| {
        if let Some(candidate) = parity.pending.take() {
            if candidate.output_matches(m, cpu) {
                EQUAL.fetch_add(1, Relaxed);
                parity.exact.insert((candidate.view, candidate.build.layer), candidate);
            } else {
                DIFFER.fetch_add(1, Relaxed);
            }
        }
    });
}

/// Check each emitted record against this view's Rust visibility. Objects
/// emitted by a visible parent's child walk may lack their own gather row.
fn visible_records(input: &Inputs<'_>, build: &Build, gather: &ClusterGather) -> Option<()> {
    let items = input.u32(build.list.checked_add(0x14)?).ok()?;
    let mut checked = BTreeSet::new();
    for slot in &build.entries[..build.count] {
        let index = u16::from_le_bytes(slot[..2].try_into().ok()?);
        let item = items.checked_add(u32::from(index) * skip::ITEM_SIZE)?;
        let record = (input.u32(item.checked_add(12)?).ok()? >> 8) as u16;
        if !checked.insert(record) { continue; }
        match objects::record_owner(input, record)? {
            objects::RecordOwner::World => {
                let at = skip::RENDER_RECORDS + u32::from(record) * skip::RENDER_RECORD_SIZE;
                if input.u32(at).ok()? & (1 << 12) == 0 {
                    let cluster = ((input.u32(at + 12).ok()? >> 9) & 511) as u16;
                    if !gather.visible.contains(&cluster) { return None; }
                }
                // A placement's record contains no cluster. Its upstream
                // geometry gather walks the placement refs of these regions.
            }
            objects::RecordOwner::Object(mut handle) => {
                let mut ancestors = BTreeSet::new();
                while !gather.objects.contains(&handle) {
                    if !ancestors.insert(handle) { return None; }
                    handle = input.u32(objects::body(input, handle)?.checked_add(0x14)?).ok()?;
                }
            }
            objects::RecordOwner::Unresolved => return None,
        }
    }
    Some(())
}

pub(in crate::native_world) enum Decision {
    Compare,
    Native(TransferAbi, u32),
}

/// None leaves the existing prefix walk in charge. Compare runs the
/// original builder with the per-record replacement suspended.
pub(in crate::native_world) fn complete(m: &mut dyn GuestMemory, cpu: &mut CpuAbi)
    -> Option<Decision>
{
    PARITY.with_borrow_mut(|parity| {
        parity.pending = None;
        let generation = crate::native_map_work::bsp_generation();
        if parity.generation != generation {
            parity.exact.clear();
            parity.generation = generation;
        }
        let layer = crate::native_world_layers::current()?;
        if layer.site != native_render::world_layers::Site::Scene || layer.view >= 4
            || !matches!(layer.number, 1 | 3 | 5 | 6 | 7 | 8 | 16)
        { return None; }
        let sp = cpu.esp;
        if m.u32(sp).ok()? != BUILD_CALLER || m.u32(sp.checked_add(4)?).ok()? != layer.list
            || m.u32(sp.checked_add(12)?).ok()? != layer.number
            || m.u32(sp.checked_add(16)?).ok()? != sp.checked_add(0x1c)?
        { return None; }
        let gather = crate::scene_step::with_open(|open| {
            let (scene, view) = open?;
            if view.key.kind != ViewKind::Game || u32::from(view.key.view) != layer.view { return None; }
            scene.cluster_gathers.last()?.as_ref().filter(|g| g.source == VisibilitySource::Rust).cloned()
        })?;
        parity.attempt(m, cpu, layer.list, layer.number, layer.view, gather)
    })
}

impl Parity {
    fn attempt(&mut self, m: &mut dyn GuestMemory, cpu: &mut CpuAbi, list: u32, layer: u32, view: u32,
        gather: ClusterGather) -> Option<Decision>
    {
        let sp = cpu.esp;
        let key = (view, layer);
        if let Some(candidate) = self.exact.get(&key)
            && candidate.gather == gather && candidate.inputs_hold(m, sp)
        {
            let build = &candidate.build;
            m.write(build.buffer, build.entries.as_flattened()).ok()?;
            TAKEN.fetch_add(1, Relaxed);
            cpu.eax = build.count as u32;
            cpu.esp = sp + 20;
            return Some(Decision::Native(TransferAbi::ret(BUILD_CALLER), build.builds));
        }
        let input = Inputs { source: m, reads: RefCell::default() };
        if input.u32(list.checked_add(4)?).ok()? > input.u32(list).ok()? { return None; }
        let mut build = plan(&input, sp, true)?;
        if build.callback.is_some() {
            // Keep the existing prefix through its first unported callback.
            // When a model occurred in this prefix it has no whole-list proof.
            if build.models { return None; }
            return super::resume(m, cpu, build).map(|(transfer, builds)| Decision::Native(transfer, builds));
        }
        visible_records(&input, &build, &gather)?;
        layer_sort::geometry(&mut build.entries[..build.count], build.layer);
        self.pending = Some(Candidate { inputs: input.spans(), build, esp: sp, view, gather });
        Some(Decision::Compare)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_world::layer_build::tests::{fixture, BUF, HEADER, ITEMS, LIST, SP, TABLE};
    use scene_extract::testing::{BODIES, Mem, lighting, object, record};

    #[test]
    fn a_mixed_list_requires_the_complete_order_and_all_current_inputs() {
        let mut m = fixture();
        let mode = 0x0060_0000;
        let section = mode + 0x200;
        let geometry = section + 0x100;
        let parts = geometry + 0x100;
        // The child's record is generated while walking its visible parent.
        let mut scene = Mem::default();
        let parent = object(&mut scene, 2, 42, 1);
        object(&mut scene, 5, 43, 1);
        scene.w(BODIES + 0x5000 + 0x14, parent);
        record(&mut scene, 1, Some(lighting(12)));
        for (address, word) in scene.0 { m.set_u32(address, word).unwrap(); }

        let model = 0xbaba_0005;
        let record = skip::RENDER_RECORDS + skip::RENDER_RECORD_SIZE;
        let program = HEADER + 0x220;
        let class_table = HEADER + 0x380;
        for (address, word) in [
            (record + 4, model), (record + 12, 2 << 29),
            (TABLE + 5 * 16, u32::from_be_bytes(*b"mode")), (TABLE + 5 * 16 + 8, mode),
            (mode + 0x24, 1), (mode + 0x28, section), (section + 0x34, geometry),
            (geometry, 1), (geometry + 4, parts), (LIST + 4, 2),
            (ITEMS + 24, skip::GEOMETRY_BUILD), (ITEMS + 24 + 8, 1),
            (ITEMS + 24 + 12, 1 << 8), (ITEMS + 24 + 16, 2),
            (HEADER + 0x354, 6), (class_table + 4 * 0x1c + 12, 16),
        ] { m.set_u32(address, word).unwrap(); }
        // Part type 4 uses the second influence count (+0x15); its 2
        // influences select vertex class 4. The first count would yield 3.
        m.set_u16(parts, 4).unwrap();
        m.set_u16(section + 0x14, 0x0201).unwrap();
        m.set_u16(section + 0x18, 3).unwrap();
        let mut cpu = CpuAbi { esp: SP, edi: LIST, ebx: 1, ..Default::default() };
        let gather = ClusterGather {
            visible: vec![0], objects: vec![parent], source: VisibilitySource::Rust, mode: 0, structure: vec![0],
        };
        let mut parity = Parity::default();
        assert!(matches!(parity.attempt(&mut m, &mut cpu, LIST, 1, 0, gather.clone()), Some(Decision::Compare)));
        let candidate = parity.pending.take().unwrap();
        assert_eq!(candidate.build.count, 2);
        assert_eq!(m.u32(BUF + 16).unwrap(), 0, "the proposal is detached");

        // Form the guest result independently from the two synthetic records.
        let mut world = [0; 24];
        world[2..4].copy_from_slice(&1u16.to_le_bytes());
        world[5] = 1;
        world[16..20].copy_from_slice(&program.to_le_bytes());
        let mut object = world;
        object[0] = 1;
        object[5] = 2;
        object[8..10].copy_from_slice(&4u16.to_le_bytes());
        object[12..16].copy_from_slice(&(5u32 << 11).to_le_bytes());
        object[20..24].copy_from_slice(&lighting(12).to_le_bytes());
        let returned = CpuAbi { esp: SP + 20, edi: LIST, ebx: 1, eax: 2, ..Default::default() };
        m.write(BUF, [object, world].as_flattened()).unwrap();
        assert!(!candidate.output_matches(&m, &returned), "a matching set in the wrong order");
        m.write(BUF, [world, object].as_flattened()).unwrap();
        assert!(candidate.output_matches(&m, &returned));
        parity.exact.insert((0, 1), candidate);
        m.write(BUF, &[0; 48]).unwrap();
        assert!(matches!(parity.attempt(&mut m, &mut cpu, LIST, 1, 0, gather.clone()), Some(Decision::Native(_, 2))));
        assert_eq!((cpu.eax, cpu.esp), (2, SP + 20));
        assert_eq!(m.bytes(BUF, 48).unwrap(), [world, object].as_flattened());
        cpu.esp = SP;
        m.write(BUF, &[0; 48]).unwrap();
        m.set_u16(section + 0x14, 0x0101).unwrap();
        assert!(matches!(parity.attempt(&mut m, &mut cpu, LIST, 1, 0, gather.clone()), Some(Decision::Compare)));
        assert_eq!(parity.pending.take().unwrap().build.count, 1, "the new vertex class has no program");
        m.set_u16(section + 0x14, 0x0201).unwrap();
        let invisible = ClusterGather { objects: Vec::new(), ..gather };
        assert!(parity.attempt(&mut m, &mut cpu, LIST, 1, 0, invisible).is_none());
    }
}
