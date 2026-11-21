//! The map's object palette is the priority prefix of the existing loader
//! walk. A definition becomes resident only after its GPU upload and its
//! material/geometry coverage are both known.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::sync::atomic::{AtomicU8, Ordering};

use map::{FourCC, TagId};
use tags::geometry::TagView;
use tags::object::{Metadata, OBJECT_TYPES};

/// Read every object palette, including entries without placements.
pub(super) fn manifest(
    meta: &Metadata, root: &[u8], group_of: impl Fn(TagId) -> Option<FourCC>,
) -> tags::Result<Vec<TagId>> {
    let view = TagView::new(&meta.bytes, meta.base);
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    for kind in OBJECT_TYPES {
        let Some((_, field, _)) = kind.placement else { continue };
        for entry in view.block(root, usize::from(field), 0x28)? {
            let id = TagId(u32::from_le_bytes(entry[4..8].try_into().unwrap()));
            if id == TagId::NULL { continue; }
            let expected = kind.group.as_bytes();
            let group = FourCC::from_u32(u32::from_le_bytes(entry[..4].try_into().unwrap()));
            if group.0.as_slice() != expected || group_of(id) != Some(group) {
                return Err(tags::Error::Invalid("scenario object palette has an invalid definition"));
            }
            if seen.insert(id) { ids.push(id); }
        }
    }
    Ok(ids)
}

/// Every implementation named by a material needs its uploaded geometry
/// and program. Record indices outside the native mask cannot be covered.
pub(super) fn implementation_mask(indices: impl IntoIterator<Item = usize>) -> Option<u8> {
    let mut mask = 0u8;
    for index in indices {
        if index >= u8::BITS as usize { return None; }
        mask |= 1 << index;
    }
    (mask != 0).then_some(mask)
}

const PENDING: u8 = 0;
const RESIDENT: u8 = 1;
const FAILED: u8 = 2;
const UNKNOWN: u8 = 3;

/// One immutable set of model-local GPU buffers and material bindings.
/// The loader publishes it once, after validating the uploaded programs.
#[derive(Default)]
pub(super) struct GpuAssets(AtomicU8);

impl GpuAssets {
    pub fn uploaded(&self) -> bool { self.0.load(Ordering::Acquire) == RESIDENT }

    pub fn finish(&self, ok: bool) {
        self.0.store(if ok { RESIDENT } else { FAILED }, Ordering::Release);
    }
}

#[derive(Clone)]
pub(super) enum Definition {
    Pending,
    NoModel,
    Failed,
    Model { gpu: Arc<GpuAssets>, all_parts: bool },
}

#[derive(Default)]
struct Counts {
    pending: u64,
    resident: u64,
    partial: u64,
    failed: u64,
    no_model: u64,
}

impl Counts {
    fn of(defs: &HashMap<TagId, Definition>) -> Self {
        let mut out = Self::default();
        for def in defs.values() {
            match def {
                Definition::Pending => out.pending += 1,
                Definition::NoModel => out.no_model += 1,
                Definition::Failed => out.failed += 1,
                Definition::Model { gpu, all_parts } => match gpu.0.load(Ordering::Acquire) {
                    PENDING => out.pending += 1,
                    RESIDENT if *all_parts => out.resident += 1,
                    RESIDENT => out.partial += 1,
                    _ => out.failed += 1,
                },
            }
        }
        out
    }
}

pub(super) struct Palette {
    defs: Mutex<Option<HashMap<TagId, Definition>>>,
    status: AtomicU8,
}

impl Default for Palette {
    fn default() -> Self {
        Self { defs: Mutex::new(None), status: AtomicU8::new(UNKNOWN) }
    }
}

impl Palette {
    pub fn start(&self, ids: &[TagId]) {
        let mut defs = self.defs.lock().unwrap_or_else(|e| e.into_inner());
        let entries = defs.insert(ids.iter().map(|id| (*id, Definition::Pending)).collect());
        self.publish(entries);
    }

    pub fn resolve(&self, id: TagId, def: Definition) {
        let mut defs = self.defs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(defs) = defs.as_mut() && let Some(slot) = defs.get_mut(&id) {
            *slot = def;
            self.publish(defs);
        }
    }

    /// The table can already contain definitions that share the uploaded
    /// model. An upload that precedes definition decoding is visible when
    /// the definition is installed.
    pub fn refresh(&self) {
        if self.status.load(Ordering::Acquire) == RESIDENT { return; }
        let defs = self.defs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(defs) = defs.as_ref() { self.publish(defs); }
    }

    fn publish(&self, defs: &HashMap<TagId, Definition>) {
        let all_ready = defs.values().all(|d| match d {
            Definition::NoModel => true,
            Definition::Model { gpu, all_parts } => *all_parts && gpu.uploaded(),
            _ => false,
        });
        self.status.store(if all_ready { RESIDENT } else { PENDING }, Ordering::Release);
    }

    fn ready(&self) -> Option<bool> {
        match self.status.load(Ordering::Acquire) {
            RESIDENT => Some(true),
            PENDING => Some(false),
            _ => None,
        }
    }
}

static ACTIVE: Mutex<Option<(u64, Weak<Palette>)>> = Mutex::new(None);

pub(super) fn clear() {
    *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub(super) fn activate(generation: u64, palette: &Arc<Palette>) {
    *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some((generation, Arc::downgrade(palette)));
}

fn active() -> Option<Arc<Palette>> {
    let guard = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    let (generation, palette) = guard.as_ref()?;
    (*generation == render_bridge::native_map_work::generation()).then(|| palette.upgrade()).flatten()
}

pub(crate) fn palette_ready() -> Option<bool> { active()?.ready() }

pub(crate) fn palette_counters() -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    if let Some(palette) = active() {
        let defs = palette.defs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(defs) = defs.as_ref() {
            let c = Counts::of(defs);
            for (name, n) in [
                ("definitions", defs.len() as u64), ("pending", c.pending), ("resident", c.resident),
                ("partial", c.partial), ("failed", c.failed), ("no_model", c.no_model),
            ] {
                out.insert(format!("scene_palette_{name}"), n);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpublished_pending_partial_invalid_and_resident_are_distinct() {
        let p = Palette::default();
        let one = TagId(0x50001);
        let two = TagId(0x50002);
        let gpu = Arc::new(GpuAssets::default());
        assert_eq!(p.ready(), None);
        p.start(&[one, two]);
        p.resolve(one, Definition::Model { gpu: Arc::clone(&gpu), all_parts: true });
        p.resolve(two, Definition::NoModel);
        assert_eq!(p.ready(), Some(false));
        gpu.finish(true);
        p.refresh();
        assert_eq!(p.ready(), Some(true));
        p.resolve(two, Definition::Model { gpu, all_parts: false });
        assert_eq!(p.ready(), Some(false));
        p.resolve(two, Definition::Failed);
        let defs = p.defs.lock().unwrap();
        let c = Counts::of(defs.as_ref().unwrap());
        assert_eq!((c.resident, c.failed, c.pending), (1, 1, 0));
    }

    #[test]
    fn upload_can_finish_before_the_loader_resolves_shared_palette_definitions() {
        let palette = Palette::default();
        let ids = [TagId(7), TagId(8)];
        palette.start(&ids);
        let model = Arc::new(GpuAssets::default());
        model.finish(true);
        palette.refresh();
        assert_eq!(palette.ready(), Some(false));
        for id in ids {
            palette.resolve(id, Definition::Model { gpu: Arc::clone(&model), all_parts: true });
        }
        assert_eq!(palette.ready(), Some(true));
        let failed = Arc::new(GpuAssets::default());
        palette.resolve(ids[1], Definition::Model { gpu: Arc::clone(&failed), all_parts: true });
        failed.finish(false);
        palette.refresh();
        assert_eq!(palette.ready(), Some(false));
        assert_eq!(Counts::of(palette.defs.lock().unwrap().as_ref().unwrap()).failed, 1);
    }

    #[test]
    fn material_coverage_requires_every_declared_implementation() {
        assert_eq!(implementation_mask([0, 2, 4]), Some(0b10101));
        assert_eq!(implementation_mask([0, 8]), None);
        assert_eq!(implementation_mask([]), None);
    }

    #[test]
    fn palette_prefetch_includes_unplaced_entries_and_rejects_invalid_identity_or_pointer() {
        let mut bytes = vec![0; 0x500];
        let field = OBJECT_TYPES.iter().find(|k| k.group == "scen").unwrap().placement.unwrap().1 as usize;
        bytes[field..field + 4].copy_from_slice(&3u32.to_le_bytes());
        bytes[field + 4..field + 8].copy_from_slice(&0x1400u32.to_le_bytes());
        let id = TagId(0x50017);
        let group = FourCC(*b"scen");
        for (index, key) in [id, TagId::NULL, id].into_iter().enumerate() {
            let offset = 0x400 + index * 0x28;
            bytes[offset..offset + 4].copy_from_slice(&group.as_u32().to_le_bytes());
            bytes[offset + 4..offset + 8].copy_from_slice(&key.0.to_le_bytes());
        }
        let mut meta = Metadata { base: 0x1000, bytes };
        assert_eq!(manifest(&meta, &meta.bytes[..0x400], |k| (k == id).then_some(group)).unwrap(), [id]);
        assert!(manifest(&meta, &meta.bytes[..0x400], |_| None).is_err());
        meta.bytes[field + 4..field + 8].copy_from_slice(&0xffff_ff00u32.to_le_bytes());
        assert!(manifest(&meta, &meta.bytes[..0x400], |_| Some(group)).is_err());
    }
}
