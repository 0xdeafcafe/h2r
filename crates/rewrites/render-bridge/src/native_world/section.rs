//! The main BSP arm of the scene material walk at 0x44c04. Plan a whole
//! record before appending its scene items and sorted geometry entries.
//! An unresolved input returns the complete record to its guest loop.

use std::collections::HashMap;

use guest_layout::memory::GuestMemory;
use native_render::skip;
use frame_scene::Owner;
use crate::native_scene_lists::transparent_bsp::{Bounds, SectionQueue, PLANE_HASH, PLANE_EQUAL};

const SCENE_LIST: u32 = 0x004c_1a48;
const SCREEN_SCALE: u32 = 0x0046_70e4;
const ALPHA_SCALE: u32 = 0x0045_dc10;
const DOUBLE_LOD: u32 = 0x004b_a019;
const LOD_SCALES: u32 = 0x0048_5b28;
const MATERIAL_VIS: u32 = 0x004e_d280;
const ACTIVE_BSP_INDEX: u32 = 0x0046_86c4;
const PART_PLANES: u32 = 0x0051_f400;
pub(super) const FIRST_PART_RETURN: u32 = 0x0004_4c15;

fn material_visible(m: &dyn GuestMemory, material: u32, placement: u32) -> Option<bool> {
    let bit = u32::from(m.u8(material.checked_add(0x1c)?).ok()?);
    if bit == 255 { return Some(true); }
    let bsp = u32::from(m.u16(ACTIVE_BSP_INDEX).ok()?);
    if bsp > i16::MAX as u32 { return None; }
    let masks = m.u32(MATERIAL_VIS).ok()?;
    let offset = if placement == 0x7ff { bsp * 32 + 1 } else { bsp * 1024 + placement + 0x201 };
    let word = m.u32(masks.checked_add(offset + bit/32*4)?).ok()?;
    Some(word & (1 << (bit%32)) != 0)
}

/// Read the live BSP part and material at the layer boundary. Native late
/// programs come from LOD zero. Without a guest item carrying the selected
/// LOD, only a single-LOD shader can supply a display-only part.
pub fn display_late_shader(m: &dyn GuestMemory, cluster: u16, part: u8, layer: u8) -> Option<u16> {
    let bsp = m.u32(super::ACTIVE_BSP).ok()?;
    if bsp == 0 || u32::from(cluster) >= m.u32(bsp.checked_add(0x9c)?).ok()? { return None; }
    let section = m.u32(bsp.checked_add(0xa0)?).ok()?.checked_add(u32::from(cluster)*0xb0)?;
    if m.u8(section.checked_add(0x46)?).ok()? == 0 { return None; }
    let geometry = m.u32(section.checked_add(0x50)?).ok()?;
    if geometry == 0 || u32::from(part) >= m.u32(geometry).ok()? { return None; }
    let p = m.u32(geometry.checked_add(4)?).ok()?.checked_add(u32::from(part)*PART_SIZE as u32)?;
    let kind = m.u16(p).ok()?;
    if matches!(kind, 0 | 5) || (kind == 4) != (layer == 6) || m.u16(p.checked_add(12)?).ok()? == 0 {
        return None;
    }
    let index = u32::from(m.u16(p.checked_add(4)?).ok()?);
    if index >= m.u32(bsp.checked_add(0xa4)?).ok()? { return None; }
    let material = m.u32(bsp.checked_add(0xa8)?).ok()?.checked_add(index*32)?;
    if !material_visible(m, material, 0x7ff)? { return None; }
    let shader = m.u32(material.checked_add(12)?).ok()?;
    let plan = LodPlan::read(m, m.u32(guest_layout::tags::TAG_ENTRIES_GLOBAL).ok()?, shader)?;
    let &[(mask, _)] = plan.levels.as_slice() else { return None; };
    (mask & 1u32.checked_shl(u32::from(layer))? != 0
        && !(mask & 0x8080 != 0 && mask & 0xa == 0 && mask & (1 << 21) != 0)).then_some(shader as u16)
}

/// The layers the display supplement draws for a display-only cluster:
/// layer 3 (the native world's display stage), the water edge blend (6)
/// and the fog (16), each at its own layer boundary. A part that also
/// draws in any other layer (the texture accumulation of layer 1 that a
/// combined layer-3 shader samples, the overlays 5, 7, 8, 20, 2 and 17,
/// a layer-15 pool surface, the sorted 18, 19 and 21) has no display draw
/// there.
pub const DISPLAY_LAYERS: u32 = 1 << 3 | 1 << 6 | 1 << 16;

/// Whether the display supplement completes every drawn part of
/// `cluster`: each visible material's layers, over all its LODs, lie in
/// [`DISPLAY_LAYERS`]. A cluster it cannot complete is not drawn at all,
/// rather than drawn without the layers it lacks (a terrain without its
/// accumulation pass, a bay floor without its pool surface). `None` when
/// the BSP, a part or a shader does not read.
pub fn display_cluster_complete(m: &dyn GuestMemory, cluster: u16) -> Option<bool> {
    let bsp = m.u32(super::ACTIVE_BSP).ok()?;
    if bsp == 0 || u32::from(cluster) >= m.u32(bsp.checked_add(0x9c)?).ok()? { return None; }
    let section = m.u32(bsp.checked_add(0xa0)?).ok()?.checked_add(u32::from(cluster)*0xb0)?;
    let geometry = m.u32(section.checked_add(0x50)?).ok()?;
    if geometry == 0 { return None; }
    let parts = m.u32(geometry).ok()?;
    if parts > 256 { return None; }
    let first = m.u32(geometry.checked_add(4)?).ok()?;
    let materials = m.u32(bsp.checked_add(0xa4)?).ok()?;
    let table = m.u32(guest_layout::tags::TAG_ENTRIES_GLOBAL).ok()?;
    for part in 0..parts {
        let p = first.checked_add(part*PART_SIZE as u32)?;
        if matches!(m.u16(p).ok()?, 0 | 5) || m.u16(p.checked_add(12)?).ok()? == 0 { continue; }
        let index = u32::from(m.u16(p.checked_add(4)?).ok()?);
        if index >= materials { return None; }
        let material = m.u32(bsp.checked_add(0xa8)?).ok()?.checked_add(index*32)?;
        if !material_visible(m, material, 0x7ff)? { continue; }
        let shader = m.u32(material.checked_add(12)?).ok()?;
        let layers = LodPlan::read(m, table, shader)?.levels.iter().fold(0, |all, &(mask, _)| all | mask);
        if layers & !DISPLAY_LAYERS != 0 { return Some(false); }
    }
    Some(true)
}

fn part_visible(m: &dyn GuestMemory, part: &Part, mask: u32, masked: bool) -> Option<bool> {
    if !masked { return Some(true); }
    let first = u32::from(part.first_subpart);
    let count = u32::from(part.subparts);
    if first + count > i16::MAX as u32 || mask == 0 { return None; }
    for i in first..first+count {
        if m.u32(mask.checked_add(i/32*4)?).ok()? & (1 << (i%32)) != 0 { return Some(true); }
    }
    Some(false)
}

// Validate the source spans independently: a level edit or a relocated
// template invalidates this material even if its shader datum is unchanged.
struct LodPlan {
    checked: Option<u64>,
    source: Vec<(u32, Vec<u8>)>,
    classification: u32,
    levels: Vec<(u32, f32)>,
}

impl LodPlan {
    fn read(m: &dyn GuestMemory, table: u32, shader: u32) -> Option<Self> {
        let mut source = Vec::new();
        let mut block = |address: u32, len: usize| -> Option<Vec<u8>> {
            let data = m.bytes(address, len).ok()?;
            source.push((address, data.clone()));
            Some(data)
        };
        let word = |data: &[u8], i: usize| u32::from_le_bytes(data[i..i+4].try_into().unwrap());
        let entry = block(table.checked_add((shader & 0xffff)*16)?, 12)?;
        if shader as u16 > i16::MAX as u16 || word(&entry, 0) != u32::from_be_bytes(*b"shad") { return None; }
        let tag = word(&entry, 8);
        let instance = word(&block(tag.checked_add(0x24)?, 4)?, 0);
        let class = block(tag.checked_add(0x3c)?, 2)?;
        let classification = u32::from(u16::from_le_bytes(class.try_into().ok()?));
        if classification > 5 { return None; }
        let template_id = word(&block(instance, 4)?, 0);
        let template = word(&block(table.checked_add((template_id & 0xffff)*16 + 8)?, 4)?, 0);
        let header = word(&block(template.checked_add(0x5c)?, 4)?, 0);
        if header == 0 { return None; }
        let header = block(header, 8)?;
        let count = word(&header, 0);
        if !(1..=256).contains(&count) { return None; }
        let data = block(word(&header, 4), count as usize * 10)?;
        let levels = data.chunks_exact(10).map(|row| (word(row, 2), f32::from_bits(word(row, 6)))).collect();
        Some(Self { checked: None, source, classification, levels })
    }

    fn current(&self, m: &dyn GuestMemory, scratch: &mut Vec<u8>) -> bool {
        self.source.iter().all(|(addr, saved)| {
            scratch.resize(saved.len(), 0);
            m.read(*addr, scratch).is_ok() && scratch.as_slice() == saved.as_slice()
        })
    }

    fn select(&self, m: &dyn GuestMemory, mut pixels: f32) -> Option<(u8, u32, u32)> {
        let mut level = 0;
        if self.classification != 5 {
            if m.u8(DOUBLE_LOD).ok()? != 0 { pixels *= m.f32(engine::light_constants::constant::TWO).ok()?; }
            pixels *= m.f32(LOD_SCALES + self.classification*4).ok()?;
            if !pixels.is_finite() { return None; }
            while level + 1 < self.levels.len() {
                let threshold = self.levels[level].1;
                if !threshold.is_finite() { return None; }
                if pixels >= threshold { break; }
                level += 1;
            }
        }
        Some((level as u8, self.levels[level].0, self.levels[0].0))
    }
}

#[derive(Default)]
struct Materials {
    shaders: HashMap<(u32, u32), LodPlan>,
    scratch: Vec<u8>,
}

impl Materials {
    fn lod(&mut self, m: &dyn GuestMemory, shader: u32, pixels: f32, watch: &mut Sources)
        -> Option<((u8, u32, u32), bool)> {
        let table = m.u32(guest_layout::tags::TAG_ENTRIES_GLOBAL).ok()?;
        let key = (table, shader);
        let hit = self.shaders.get(&key).is_some_and(|p| {
            watch.clean(p.checked) || p.current(m, &mut self.scratch)
        });
        if !hit {
            self.shaders.insert(key, LodPlan::read(m, table, shader)?);
        }
        let plan = self.shaders.get_mut(&key)?;
        if watch.stamp.is_some() && plan.checked.is_none() {
            for (addr, data) in &plan.source { watch.add(*addr, data.len()); }
        }
        plan.checked = watch.stamp;
        Some((plan.select(m, pixels)?, hit))
    }
}

const PART_SIZE: usize = 0x48;

struct Part {
    kind: u16,
    material: u16,
    first_subpart: u16,
    subparts: u16,
    size: f32,
}

struct Parts {
    checked: Option<u64>,
    source: Vec<u8>,
    parts: Vec<Part>,
}

#[derive(Default)]
pub(super) struct SectionCache {
    parts: SectionParts,
    materials: Materials,
    outputs: Vec<u8>,
    sources: Sources,
    compare_transparents: bool,
}

/// Clean watched pages let the scene walk use decoded resources directly.
/// If any watched page changes, verify the actual spans on their next use.
#[derive(Default)]
struct Sources {
    subscription: Option<(u64, u64, runtime::WatchId)>, // BSP generation, virtual layout, watch
    revision: u64,
    stamp: Option<u64>,
    pending: Vec<(u32, usize)>,
}

impl Sources {
    fn clean(&self, checked: Option<u64>) -> bool { checked.is_some() && self.stamp == checked }

    fn add(&mut self, addr: u32, len: usize) {
        if self.stamp.is_some() { self.pending.push((addr, len)); }
    }
}

impl SectionCache {
    pub(super) fn append_guest(&mut self, m: &mut runtime::Guest, sp: u32, index: u32)
        -> Option<(u32, bool)> {
        self.compare_transparents = true;
        crate::native_scene_lists::transparent_bsp::parity::reset_visit();
        const OWNER: &str = "native-world-section-sources";
        let generation = crate::native_map_work::bsp_generation();
        let (layout, revision) = m.write_watch_epoch();
        let id = match self.sources.subscription {
            Some((g, l, id)) if (g, l) == (generation, layout) => id,
            _ => {
                // Watch IDs are local to a Guest. A render-thread RAM view or
                // restored runtime can have a different owner at the same ID.
                if let Some((id, _)) = m.write_watch_owners().into_iter().find(|(_, name)| *name == OWNER) {
                    m.unsubscribe_writes(id);
                }
                for section in self.parts.sections.values_mut() { section.checked = None; }
                for shader in self.materials.shaders.values_mut() { shader.checked = None; }
                let id = m.subscribe_writes(OWNER);
                self.sources.subscription = Some((generation, layout, id));
                id
            }
        };
        if m.has_dirty(id) || self.sources.revision != revision || m.has_trap() {
            m.take_dirty(id).ok()?;
            self.sources.stamp = Some(self.sources.stamp.unwrap_or(0).wrapping_add(1));
        } else {
            self.sources.stamp.get_or_insert(0);
        }
        self.sources.revision = revision;
        let result = append_cached(m, sp, index, self, generation);
        if !self.sources.pending.is_empty() {
            // Verify newly watched data once. The caller may have aliased the
            // output list with a source before its pages were subscribed.
            self.sources.stamp = self.sources.stamp.map(|stamp| stamp.wrapping_add(1));
        }
        for (addr, len) in self.sources.pending.drain(..) {
            if m.watch_virtual_read(id, addr, len).is_err() {
                // Outputs were resolved from live bytes. Retry registration
                // with empty caches on the next call.
                self.sources.subscription = None;
                break;
            }
        }
        result
    }
}

#[derive(Default)]
struct SectionParts {
    scope: Option<(u64, u32)>,
    sections: HashMap<u32, Parts>,
    scratch: Vec<u8>,
}

impl SectionParts {
    // Compare the live block in one read. Resource streaming and tag edits
    // can overwrite a section in place, including on a render-thread RAM view.
    fn resolve(&mut self, m: &dyn GuestMemory, generation: u64, bsp: u32, address: u32, count: u32, watch: &mut Sources)
        -> Option<(&[Part], bool)> {
        if self.scope != Some((generation, bsp)) {
            self.sections.clear();
            self.scope = Some((generation, bsp));
        }
        let len = count as usize * PART_SIZE;
        let cached = self.sections.entry(address).or_insert_with(|| Parts {
            source: Vec::new(), parts: Vec::new(), checked: None,
        });
        if cached.source.len() == len && watch.clean(cached.checked) { return Some((&cached.parts, true)); }
        self.scratch.resize(len, 0);
        m.read(address, &mut self.scratch).ok()?;
        let hit = cached.source == self.scratch;
        if !hit || cached.checked.is_none() { watch.add(address, len); }
        cached.checked = watch.stamp;
        if !hit {
            std::mem::swap(&mut cached.source, &mut self.scratch);
            cached.parts.clear();
            for row in cached.source.chunks_exact(PART_SIZE) {
                let word = |at: usize| u16::from_le_bytes([row[at], row[at+1]]);
                cached.parts.push(Part {
                    kind: word(0), material: word(4), first_subpart: word(10), subparts: word(12),
                    size: f32::from_le_bytes(row[0x2c..0x30].try_into().unwrap()),
                });
            }
        }
        Some((&cached.parts, hit))
    }
}

/// Layer-15 parts of a cluster reached by the display portal flood. No
/// guest render record or visible-subpart mask exists for this owner.
pub(crate) fn display_cluster_transparents(m: &dyn GuestMemory, id: u16, cw: u16)
    -> Option<Vec<frame_scene::transparent::DisplayTransparentPart>> {
    use frame_scene::transparent::DisplayTransparentPart;
    let bsp = m.u32(super::ACTIVE_BSP).ok()?;
    if bsp == 0 || u32::from(id) >= m.u32(bsp.checked_add(0x9c)?).ok()? || id >= 512 { return None; }
    let base = m.u32(bsp.checked_add(0xa0)?).ok()?;
    let section = base.checked_add(u32::from(id)*0xb0)?;
    if m.u16(section.checked_add(0x24)?).ok()? == 0 { return Some(Vec::new()); }
    if m.u8(section.checked_add(0x46)?).ok()? == 0 { return None; }
    let geometry = m.u32(section.checked_add(0x50)?).ok()?;
    if geometry == 0 { return None; }
    let parts = m.u32(geometry).ok()?;
    if parts > 256 { return None; }
    let part_base = m.u32(geometry.checked_add(4)?).ok()?;
    let mat_count = m.u32(bsp.checked_add(0xa4)?).ok()?;
    let mat_base = m.u32(bsp.checked_add(0xa8)?).ok()?;
    let planes = m.u32(PART_PLANES).ok()?;
    let table = m.u32(guest_layout::tags::TAG_ENTRIES_GLOBAL).ok()?;
    let pixels = engine::render_loop::world_walk::cluster_pixel_size(m, section, cw)?;
    let scale = m.f32(SCREEN_SCALE).ok()?;
    let alpha = m.f32(ALPHA_SCALE).ok()?;
    let mut found = Vec::new();
    let mut materials = HashMap::new();
    for part in 0..parts {
        let at = part_base.checked_add(part*PART_SIZE as u32)?;
        let kind = m.u16(at).ok()?;
        if matches!(kind, 0 | 5) { continue; }
        let material = u32::from(m.u16(at.checked_add(4)?).ok()?);
        if material >= mat_count || material > i16::MAX as u32 { return None; }
        let mat = mat_base.checked_add(material*32)?;
        if !material_visible(m, mat, 0x7ff)? { continue; }
        let shader = m.u32(mat.checked_add(12)?).ok()?;
        if shader == u32::MAX { continue; }
        let size = m.f32(at.checked_add(0x2c)?).ok()?;
        let size = if size > 0. { size } else { m.f32(engine::light_constants::constant::ONE).ok()? };
        let plan = match materials.entry(shader) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => e.insert(LodPlan::read(m, table, shader)?),
        };
        let (lod, layers, base_layers) = plan.select(m, ((f32::from(pixels)*size)*scale)*alpha)?;
        if layers & (1<<15) == 0 || layers & 0xa == 0 && base_layers & (1<<21) != 0 { continue; }
        let bounds = Bounds::read(m, bsp, planes, Owner::Cluster(u32::from(id)), part as u8, at)?;
        let tag = m.u32(table.checked_add((shader & 0xffff)*16+8)?).ok()?;
        let flags = m.u8(tag.checked_add(0x16)?).ok()?;
        let first_subpart = m.u16(at.checked_add(10)?).ok()?;
        let subpart_count = m.u16(at.checked_add(12)?).ok()?;
        let total = m.u16(section.checked_add(0x24)?).ok()?;
        if subpart_count == 0 { continue; }
        if first_subpart.checked_add(subpart_count)? > total { return None; }
        let [class, depth] = crate::native_scene_lists::display_sort_key(m, bounds.position, flags)?;
        found.push(DisplayTransparentPart {
            cluster: id, part: part as u8, lod, shader, position: bounds.position,
            class: class as i32, depth: f32::from_bits(depth),
            plane: bounds.plane.map(|[x,y,z,d]| ([x,y,z],d)),
            first_index: m.u16(at.checked_add(6)?).ok()?,
            index_count: m.u16(at.checked_add(8)?).ok()?,
            triangle_list: m.u16(at.checked_add(2)?).ok()? & 8 != 0,
            first_subpart, subpart_count,
        });
    }
    Some(found)
}

#[cfg(test)]
fn append(m: &mut dyn GuestMemory, sp: u32, index: u32) -> Option<u32> {
    append_cached(m, sp, index, &mut SectionCache::default(), 0).map(|(count, _)| count)
}

fn append_cached(m: &mut dyn GuestMemory, sp: u32, index: u32, cache: &mut SectionCache,
    generation: u64) -> Option<(u32, bool)> {
    if index >= 0x352 || m.u32(sp).ok()? != FIRST_PART_RETURN ||
        m.u32(sp.checked_add(4)?).ok()? != 0 || m.u32(sp.checked_add(0x60)?).ok()? != 0 ||
        m.u32(sp.checked_add(0x68)?).ok()? != 0x1f ||
        m.u32(sp.checked_add(0x1c)?).ok()? != u32::MAX { return None; }
    let record = skip::RENDER_RECORDS + index*skip::RENDER_RECORD_SIZE;
    if m.u32(record+4).ok()? != u32::MAX { return None; }
    let planes = m.u32(PART_PLANES).ok()?;
    if planes == 0 { return None; }
    // In opaque entries the computed clipping plane and centre are private
    // scratch. Only bypass the two known pure callbacks of the plane lookup.
    if m.u32(planes.checked_add(0x2c)?).ok()? != PLANE_HASH ||
        m.u32(planes.checked_add(0x30)?).ok()? != PLANE_EQUAL { return None; }
    let flags = m.u32(record).ok()?;
    let mask = m.u32(record+20).ok()?;
    let placement = m.u32(record+12).ok()? >> 18 & 0x7ff;
    let geometry = m.u32(sp.checked_add(0x28)?).ok()?;
    let count = m.u32(geometry).ok()?;
    if geometry == 0 || !(1..=256).contains(&count) { return None; }
    let part_base = m.u32(geometry.checked_add(4)?).ok()?;
    let bsp = m.u32(super::ACTIVE_BSP).ok()?;
    let materials = m.u32(bsp.checked_add(0xa8)?).ok()?;
    let material_count = m.u32(bsp.checked_add(0xa4)?).ok()?;
    let pixels = m.u8(sp.checked_add(0x24)?).ok()?;
    let scale = m.f32(SCREEN_SCALE).ok()?;
    let alpha_scale = m.f32(ALPHA_SCALE).ok()?;
    let first = m.u32(SCENE_LIST+4).ok()?;
    let capacity = m.u32(SCENE_LIST).ok()?;
    if first > capacity || capacity > skip::MAX_ITEMS { return None; }
    let dst = m.u32(SCENE_LIST+20).ok()?.checked_add(first * skip::ITEM_SIZE)?;
    let mut all_layers = m.u32(SCENE_LIST+8).ok()?;
    if cache.parts.scope != Some((generation, bsp)) { cache.materials.shaders.clear(); }
    let outputs = &mut cache.outputs;
    outputs.clear();
    let (parts, mut hit) = cache.parts.resolve(m, generation, bsp, part_base, count, &mut cache.sources)?;
    let mut queue = SectionQueue::default();
    for (n, part) in parts.iter().enumerate() {
        let material_no = u32::from(part.material);
        if material_no >= material_count || material_no > i16::MAX as u32 { return None; }
        let mat = materials.checked_add(material_no*32)?;
        if !material_visible(m, mat, placement)? || !part_visible(m, part, mask, flags & 64 != 0)? { continue; }
        let shader = m.u32(mat.checked_add(12)?).ok()?;
        let kind = part.kind;
        if shader == u32::MAX || matches!(kind, 0 | 5) { continue; }
        let size = part.size;
        let size = if size > 0.0 { size } else { m.f32(engine::light_constants::constant::ONE).ok()? };
        let ((level, layers, base_layers), material_hit) = cache.materials.lod(m, shader,
            ((f32::from(pixels)*size)*scale)*alpha_scale, &mut cache.sources)?;
        hit &= material_hit;
        // The standard scene caller passes the same shader as its override.
        if layers & 0x8080 != 0 && layers & 0xa == 0 && base_layers & (1 << 21) != 0 { continue; }
        let argument = u32::from(pixels)<<24 | index<<8 | n as u32;
        if layers & (1<<15) != 0 {
            let owner = if flags & (1<<12) != 0 {
                Owner::Instance(placement)
            } else {
                Owner::Cluster((m.u32(record+12).ok()? >> 9) & 0x1ff)
            };
            let bounds = Bounds::read(m, bsp, planes, owner, n as u8, part_base.checked_add(n as u32 * PART_SIZE as u32)?)?;
            queue.push(m, bounds, shader, argument)?;
        }
        // The base scene skips the ordinary material append if the only
        // selected layer is the sorted handler (0x412a3).
        if layers & !(1 << 15) == 0 || first + (outputs.len() / 24) as u32 == capacity { continue; }
        let mut item = [0; 24];
        let item_at = dst.checked_add(outputs.len() as u32)?;
        m.read(item_at, &mut item).ok()?;
        item[0..4].copy_from_slice(&skip::GEOMETRY_BUILD.to_le_bytes());
        item[4..8].copy_from_slice(&skip::GEOMETRY_DRAW.to_le_bytes());
        item[8..12].copy_from_slice(&shader.to_le_bytes());
        item[12..16].copy_from_slice(&argument.to_le_bytes());
        item[16..20].copy_from_slice(&layers.to_le_bytes());
        item[20] = 0;
        item[21] = level;
        all_layers |= layers;
        outputs.extend_from_slice(&item);
    }
    if let Some((rows_at, rows, count_at, next_count)) = queue.pending() {
        let mut header = [0; 8];
        header[..4].copy_from_slice(&(first + (outputs.len() / 24) as u32).to_le_bytes());
        header[4..].copy_from_slice(&all_layers.to_le_bytes());
        let mut writes: Vec<(u32, &[u8])> = vec![(rows_at, rows), (count_at, &next_count)];
        if !outputs.is_empty() {
            writes.extend([(dst, outputs.as_slice()), (SCENE_LIST + 4, header.as_slice())]);
        }
        if cache.compare_transparents {
            // Check the opaque count and mask even for a section that should
            // add only transparents. Scratch bytes outside both live tails
            // have no consumer.
            let mut verify = writes.clone();
            if outputs.is_empty() { verify.push((SCENE_LIST+4, &header)); }
            if !crate::native_scene_lists::transparent_bsp::parity::admit(
                generation, geometry, m.u32(record+12).ok()?, sp.checked_add(12)?, index, &verify,
            ) { return None; }
        }
        // A later read-only destination must leave both queues intact for
        // the guest to retry the entire section. GuestMemory writes each
        // individual span atomically.
        let old = writes.iter().map(|(addr, bytes)| m.bytes(*addr, bytes.len()).ok())
            .collect::<Option<Vec<_>>>()?;
        for (i, &(addr, bytes)) in writes.iter().enumerate() {
            if m.write(addr, bytes).is_err() {
                for (&(prior, _), saved) in writes[..i].iter().zip(&old).rev() {
                    m.write(prior, saved).expect("restore material lists on refusal");
                }
                return None;
            }
        }
    } else if !outputs.is_empty() {
        let mut old_header = [0; 8];
        m.read(SCENE_LIST+4, &mut old_header).ok()?;
        let mut header = [0; 8];
        header[..4].copy_from_slice(&(first+(outputs.len()/24) as u32).to_le_bytes());
        header[4..].copy_from_slice(&all_layers.to_le_bytes());
        m.write(SCENE_LIST+4, &header).ok()?;
        if m.write(dst, outputs).is_err() {
            m.write(SCENE_LIST+4, &old_header).expect("restore scene list on refusal");
            return None;
        }
    }
    Some((count, hit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    const SP: u32 = 0x520000;
    const BSP: u32 = SP+0x1000;
    const ITEMS: u32 = SP+0x2000;
    const GEOM: u32 = BSP+0x400;
    const PARTS: u32 = GEOM+0x100;
    const MAT: u32 = BSP+0x800;
    const HEADER: u32 = SP+0x3000;
    const TABLE: u32 = HEADER+0x400;

    fn fixture() -> FlatMemory {
        let mut m = FlatMemory::new(0x400000, 0x140000);
        for (at, v) in [
            (SP, FIRST_PART_RETURN), (SP+0x68, 0x1f), (SP+0x1c, u32::MAX),
            (SP+0x28, GEOM), (skip::RENDER_RECORDS+4, u32::MAX),
            (skip::RENDER_RECORDS+12, 0x7ff << 18), (GEOM, 2), (GEOM+4, PARTS),
            (super::super::ACTIVE_BSP, BSP), (BSP+0xa4, 2), (BSP+0xa8, MAT),
            (MAT+12, 1), (MAT+32+12, 1), (SCENE_LIST, 8), (SCENE_LIST+20, ITEMS),
            (PART_PLANES, BSP+0xb00), (BSP+0xb2c, PLANE_HASH), (BSP+0xb30, PLANE_EQUAL),
            (guest_layout::tags::TAG_ENTRIES_GLOBAL, TABLE), (TABLE+16, u32::from_be_bytes(*b"shad")),
            (TABLE+24, HEADER+0x100), (HEADER+0x124, HEADER+0x180), (HEADER+0x180, 2),
            (TABLE+40, HEADER+0x200), (HEADER+0x25c, HEADER), (HEADER, 1),
            (HEADER+4, HEADER+0x40), (HEADER+0x42, (1<<3)|(1<<16)),
        ] { m.set_u32(at, v).unwrap(); }
        m.set_u8(MAT+28, 255).unwrap();
        m.set_u8(MAT+32+28, 255).unwrap();
        m.set_u16(PARTS, 2).unwrap();
        m.set_u16(PARTS+0x48, 2).unwrap();
        m.set_u16(PARTS+0x48+4, 1).unwrap();
        m
    }

    #[test]
    fn display_only_cluster_supplies_water_and_fog_at_their_live_layer() {
        use scene_extract::visibility::frusta::{Frustum, WindowFrusta};
        let mut m = fixture();
        m.bytes.resize(0x170000, 0);
        let section = BSP + 0xd00;
        m.set_u32(BSP + 0x9c, 1).unwrap();
        m.set_u32(BSP + 0xa0, section).unwrap();
        m.set_u32(section + 0x50, GEOM).unwrap();
        m.set_u16(section + 0x24, 2).unwrap();
        m.set_u8(section + 0x46, 1).unwrap();
        for (n, v) in [1.10, 1.25, -0.2, 0.2, 0.1, 0.5].into_iter().enumerate() {
            m.set_f32(section + 0x54 + 4*n as u32, v).unwrap();
        }
        m.set_u16(PARTS, 4).unwrap();
        m.set_u16(PARTS + 12, 1).unwrap();
        m.set_u16(PARTS + 0x48 + 12, 1).unwrap();
        m.set_u32(HEADER + 0x42, 1 << 6 | 1 << 16).unwrap();

        let window = |right: f32| WindowFrusta(vec![(0, Frustum {
            bounds: [-right, right, -1., 1., 0., 1.],
            planes: [[0.; 4]; 6],
        })]);
        let bounds = crate::native_scene_lists::late_cluster_bounds(&m, 0).unwrap();
        assert!(!window(1.).contains_cluster(0, bounds));
        assert!(!window(1.2).contains_cluster(0, bounds), "only part of the section is visible");
        assert!(window(4./3.).contains_cluster(0, bounds));
        assert_eq!(display_late_shader(&m, 0, 0, 6), Some(1));
        assert_eq!(display_late_shader(&m, 0, 1, 16), Some(1));
        m.set_u32(0x004b_9ef0, 3).unwrap(); // fog view
        m.set_u32(0x004b_9f8c, u32::MAX).unwrap();
        let flags = crate::native_scene_lists::display_cluster_fog_flags(&m, 0).unwrap();
        assert_eq!(native_render::fog::layer_implementation(flags, false), Some(0));

        m.set_u8(MAT + 32 + 28, 0).unwrap();
        assert_eq!(display_late_shader(&m, 0, 1, 16), None, "hidden material");
        m.set_u32(HEADER, 2).unwrap();
        assert_eq!(display_late_shader(&m, 0, 0, 6), None, "part needs a per-view LOD");
    }

    /// The left edge of a wide Delta Towers view: a display-only cluster
    /// whose shaders also draw in layers the supplement has no display
    /// draw for is left out whole, not drawn as a flat, unaccumulated or
    /// uncovered slab.
    #[test]
    fn a_display_only_cluster_is_drawn_only_when_every_layer_of_it_is() {
        let mut m = fixture();
        m.bytes.resize(0x170000, 0);
        let section = BSP + 0xd00;
        m.set_u32(BSP + 0x9c, 1).unwrap();
        m.set_u32(BSP + 0xa0, section).unwrap();
        m.set_u32(section + 0x50, GEOM).unwrap();
        m.set_u16(PARTS + 12, 1).unwrap();
        m.set_u16(PARTS + 0x48 + 12, 1).unwrap();
        for (layers, complete) in [
            (1 << 3, true),
            (1 << 3 | 1 << 16, true),
            (1 << 6 | 1 << 16, true),
            (1 << 1 | 1 << 3, false),  // layer 3 samples layer 1's accumulation
            (1 << 3 | 1 << 7, false),
            (1 << 15, false),          // a pool surface: layer 15
            (1 << 3 | 1 << 18, false),
        ] {
            m.set_u32(HEADER + 0x42, layers).unwrap();
            assert_eq!(display_cluster_complete(&m, 0), Some(complete), "layers {layers:#x}");
        }
        // Every LOD counts: a far level that adds a layer refuses too.
        m.set_u32(HEADER + 0x42, 1 << 3).unwrap();
        m.set_u32(HEADER, 2).unwrap();
        m.set_u32(HEADER + 0x40 + 10 + 2, 1 << 3 | 1 << 1).unwrap();
        assert_eq!(display_cluster_complete(&m, 0), Some(false));
        // A part with no subparts draws nothing and does not count.
        m.set_u32(HEADER, 1).unwrap();
        m.set_u16(PARTS + 0x48 + 12, 0).unwrap();
        m.set_u16(PARTS + 0x48 + 4, 0).unwrap();
        assert_eq!(display_cluster_complete(&m, 0), Some(true));
        assert_eq!(display_cluster_complete(&m, 1), None, "no such cluster");
    }

    #[test]
    fn all_opaque_parts_publish_in_order_including_the_other_layers_bits() {
        let mut m = fixture();
        m.set_u8(SP+0x24, 19).unwrap();
        m.set_u16(ITEMS+22, 0xbeef).unwrap();
        assert_eq!(append(&mut m, SP, 0), Some(2));
        assert_eq!(m.u32(SCENE_LIST+4).unwrap(), 2);
        assert_eq!(m.u32(SCENE_LIST+8).unwrap(), (1<<3)|(1<<16));
        assert_eq!(m.u32(ITEMS+12).unwrap(), 19<<24);
        assert_eq!(m.u32(ITEMS+24+12).unwrap(), (19<<24)|1);
        assert_eq!(m.u16(ITEMS+22).unwrap(), 0xbeef);
    }

    #[test]
    fn a_second_frame_reuses_sections_and_reads_live_visibility_and_materials() {
        let mut m = fixture();
        let mut cache = SectionCache::default();
        let mut frame = |m: &mut FlatMemory, generation| {
            m.set_u32(SCENE_LIST+4, 0).unwrap();
            m.set_u32(SCENE_LIST+8, 0).unwrap();
            append_cached(m, SP, 0, &mut cache, generation)
        };
        assert_eq!(frame(&mut m, 1), Some((2, false)));
        let first = m.bytes(ITEMS, 48).unwrap();
        assert_eq!(frame(&mut m, 1), Some((2, true)));
        assert_eq!(m.bytes(ITEMS, 48).unwrap(), first);
        m.set_u32(skip::RENDER_RECORDS, 64).unwrap();
        m.set_u32(skip::RENDER_RECORDS+20, BSP+0x40).unwrap();
        assert_eq!(frame(&mut m, 1), Some((2, true)));
        assert_eq!(m.u32(SCENE_LIST+4).unwrap(), 0);
        m.set_u32(skip::RENDER_RECORDS, 0).unwrap();
        m.set_u32(HEADER+0x42, 1<<7).unwrap();
        assert_eq!(frame(&mut m, 1), Some((2, false)));
        assert_eq!(m.u32(ITEMS+16).unwrap(), 1<<7);
        m.set_u16(PARTS, 0).unwrap();
        assert_eq!(frame(&mut m, 1), Some((2, false)));
        assert_eq!(m.u32(SCENE_LIST+4).unwrap(), 1);
        assert_eq!(frame(&mut m, 2), Some((2, false)));
    }

    #[test]
    fn watched_sections_follow_alias_writes_and_refreshed_render_views() {
        use runtime::guest::Permissions;
        let src = fixture();
        let mut game = runtime::Guest::new();
        let len = src.bytes.len() as u32;
        let alias = src.base + len;
        game.map(src.base, len, 0, Permissions::RW).unwrap();
        game.map(alias, len, 0, Permissions::RW).unwrap();
        game.write_bytes(src.base, &src.bytes, 1).unwrap();
        let (mut render, _) = game.cow_snapshot();
        let mut cache = SectionCache::default();
        let mut frame = |mem: &mut runtime::Guest| {
            mem.set_u32(SCENE_LIST+4, 0).unwrap();
            cache.append_guest(mem, SP, 0)
        };
        assert_eq!(frame(&mut render), Some((2, false)));
        assert_eq!(frame(&mut render), Some((2, true)));
        render.set_u16(PARTS + alias - src.base, 0).unwrap();
        assert_eq!(frame(&mut render), Some((2, false)));
        assert_eq!(render.u32(SCENE_LIST+4).unwrap(), 1);
        // cow_refresh clears dirty pages, but can change the contents of any
        // watched range while retaining the identical virtual layout.
        game.set_u32(HEADER+0x42, 1<<7).unwrap();
        render.cow_refresh(&game);
        assert_eq!(frame(&mut render), Some((2, false)));
        assert_eq!(render.u32(SCENE_LIST+4).unwrap(), 2);
        assert_eq!(render.u32(ITEMS+16).unwrap(), 1<<7);
    }

    #[test]
    fn section_build_publishes_layer_15_in_part_order_and_mixed_parts_to_both_lists() {
        const END: u32 = 0x004b_6280;
        const ENTRIES: u32 = 0x004b_6288;
        const ROWS: u32 = SP+0x4000;
        let mut m = fixture();
        m.set_u32(BSP+0xb20, 1).unwrap();
        m.set_u32(ENTRIES, ROWS).unwrap();
        m.set_u32(END, 1).unwrap();
        m.set_u8(SP+0x24, 17).unwrap();
        m.set_u32(HEADER+0x42, 1<<15).unwrap();
        m.set_u8(HEADER+0x116, 1).unwrap();
        m.set_f32(0x0048_5624, 1.).unwrap();
        m.set_f32(0x0048_5618, 3.).unwrap();
        m.set_f32(PARTS+0x10, 9.).unwrap();
        m.set_f32(PARTS+0x48+0x10, -2.).unwrap();
        m.set_u8(ROWS+0x70+0x3a, 37).unwrap();
        assert_eq!(append(&mut m, SP, 0), Some(2));
        assert_eq!(m.u32(SCENE_LIST+4), Ok(0));
        assert_eq!(m.u32(END), Ok(3));
        for (i, depth) in [(0, -6.), (1, 5.)] {
            let row = ROWS+(i+1)*0x70;
            assert_eq!(m.u32(row), Ok(1));
            assert_eq!(m.f32(row+4), Ok(depth));
            assert_eq!(m.u32(row+0x20), Ok(guest_layout::points::TRANSPARENT_GEOMETRY.base));
            assert_eq!(m.u32(row+0x24), Ok(1));
            assert_eq!(m.u32(row+0x28), Ok((17<<24) | i));
        }
        assert_eq!(m.u8(ROWS+0x70+0x3a), Ok(37));
        m.set_u32(HEADER+0x42, (1<<15) | (1<<6)).unwrap();
        assert_eq!(append(&mut m, SP, 0), Some(2));
        assert_eq!(m.u32(SCENE_LIST+4), Ok(2));
        assert_eq!(m.u32(ITEMS+16), Ok((1<<15) | (1<<6)));
        assert_eq!(m.u32(END), Ok(5));

        m.set_u32(END, 0x3ff).unwrap();
        let before = m.bytes.clone();
        assert_eq!(append(&mut m, SP, 0), None);
        assert_eq!(m.bytes, before);
    }

    #[test]
    fn unknown_last_material_or_transparent_pass_leaves_the_whole_section_for_the_guest() {
        let mut m = fixture();
        m.set_u32(MAT+32+12, 7).unwrap();
        let before = m.bytes.clone();
        assert_eq!(append(&mut m, SP, 0), None);
        assert_eq!(m.bytes, before);
        m.set_u32(MAT+32+12, 1).unwrap();
        m.set_u32(HEADER+0x42, 1<<15).unwrap();
        assert_eq!(append(&mut m, SP, 0), None);
        assert_eq!(m.u32(SCENE_LIST+4).unwrap(), 0);
    }

    #[test]
    fn scene_items_use_the_selected_lod_and_leave_a_full_list_alone() {
        let mut m = fixture();
        m.set_u32(HEADER, 2).unwrap();
        m.set_f32(HEADER+0x46, 10.0).unwrap();
        m.set_u32(HEADER+0x4c, 1<<7).unwrap();
        m.set_u32(SCENE_LIST, 1).unwrap();
        assert_eq!(append(&mut m, SP, 0), Some(2));
        assert_eq!(m.u32(SCENE_LIST+4).unwrap(), 1);
        assert_eq!(m.u32(SCENE_LIST+8).unwrap(), 1<<7);
        assert_eq!(m.u8(ITEMS+21).unwrap(), 1);
        let before = m.bytes.clone();
        assert_eq!(append(&mut m, SP, 0), Some(2));
        assert_eq!(m.bytes, before);
    }

    #[test]
    fn hidden_subparts_do_not_need_a_native_material_or_an_item() {
        let mut m = fixture();
        m.set_u32(skip::RENDER_RECORDS, 64).unwrap();
        m.set_u32(skip::RENDER_RECORDS+20, BSP+0x40).unwrap();
        m.set_u32(BSP+0x40, 1).unwrap();
        m.set_u16(PARTS+12, 1).unwrap();
        m.set_u16(PARTS+0x48+10, 1).unwrap();
        m.set_u16(PARTS+0x48+12, 1).unwrap();
        m.set_u32(MAT+32+12, 7).unwrap();
        assert_eq!(append(&mut m, SP, 0), Some(2));
        assert_eq!(m.u32(SCENE_LIST+4).unwrap(), 1);
    }
}
