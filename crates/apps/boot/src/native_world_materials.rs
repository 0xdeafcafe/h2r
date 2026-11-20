//! Opaque pass choices made from ordinary tag metadata when a BSP loads.
//! The cache holds program words and texture selection recipes. An Xbox
//! bitmap header, its lease and its palette belong to the live render view.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::sync::{Arc, Mutex};

use d3d::{Backend, Device, device::PreparedStageTexture, shader::Microcode};
use engine::memory::GuestMemory;
use engine::render_loop::{world_lightmap, world_walk};
use map::{Map, TagId};
use native_render::world_ranges::SortedPart;
use tags::material::{MAX_MATERIAL_IMPLEMENTATIONS, MAX_MATERIAL_WORK, ShaderLibrary};
use tags::shader::{BitmapParameter, Metadata, TemplatePostprocess};

const OPAQUE_LAYERS: [u8; 5] = [1, 3, 5, 7, 8];
const RASTERIZER: u32 = 0x0048_5a80;
const DEFAULT_BITMAP: u32 = 0x64;
const BITMAP_EPOCH: u32 = 0x004e_6488;
const IMAGE_SIZE: u32 = 0x74;
const STAGE_STATES: u32 = 0x0040_4dd0;
const SLOT_BANK: u32 = engine::render_loop::layer::SETUP_CONTEXT + 0x320;

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct Key {
    shader: u32,
    lod: u8,
    layer: u8,
    pass: u8,
    implementation: u8,
}

#[derive(Clone, Copy)]
enum Source {
    Bitmap(TagId, i32),
    Rasterizer(u32, i32),
    ViewBitmap,
    Slot(u8),
    Lightmap,
    // A render texture takes the ordered surface/alias path in submit_inner.
    Surface,
    Texaccum,
    Inherited,
}

#[derive(Clone, Copy)]
struct Stages {
    sources: [Source; 4],
    signs: [Option<u32>; 4],
    samples: [bool; 4],
}

pub(super) struct Material<'a> {
    pub(super) code: Option<Microcode>,
    stages: Stages,
    textures: &'a Mutex<[Option<CachedStage>; 4]>,
}

#[derive(Default)]
pub(super) struct Library {
    passes: HashMap<Key, Result<Pass, &'static str>>,
    rejected: HashMap<u32, &'static str>,
}

struct Selection {
    key: Key,
    pass: TagId,
    slots: Vec<(u8, Source)>,
}

#[derive(Clone, Copy)]
struct CachedStage {
    resource: (u32, u32, u32),
    hint: PreparedStageTexture,
}

struct Pass {
    textures: Mutex<[Option<CachedStage>; 4]>,
    classes: Arc<[Option<Microcode>]>,
    stages: Stages,
}

type ProgramCache = HashMap<TagId, Arc<[Option<Microcode>]>>;

impl Library {
    pub(super) fn load<R: Read + Seek>(map: &mut Map<R>, shaders: &[TagId]) -> Self {
        let mut out = Self::default();
        let Ok(mut library) = ShaderLibrary::read(map) else {
            out.rejected.extend(shaders.iter().map(|s| (s.0, "unreadable shader tag space")));
            return out;
        };
        let mut programs = ProgramCache::new();
        let mut visited = HashSet::new();
        for &shader in shaders {
            if !visited.insert(shader) { continue; }
            let selections = (|| {
                let instance = library.shader(shader).map_err(|_| "unreadable world shader")?;
                let params = instance.compiled().map_err(|_| "no compiled world shader")?;
                let template = library.template(params.template).map_err(|_| "unreadable shader template")?
                    .compiled().map_err(|_| "no compiled shader template")?;
                selections(shader, &params.bitmaps, template)
            })();
            let selected = match selections {
                Ok(selected) => selected,
                Err(why) => {
                    out.rejected.insert(shader.0, why);
                    continue;
                }
            };
            for row in selected {
                let result = (|| {
                    let pass = library.pass(row.pass).map_err(|_| "unreadable shader pass")?
                        .compiled().map_err(|_| "no compiled shader pass")?;
                    let imp = pass.implementations.get(usize::from(row.key.implementation))
                        .ok_or("missing shader pass implementation")?;
                    // Static and fragment-built definitions both copy the
                    // embedded texture-modes word (PSDEF dword 54).
                    let modes = imp.psdef_word(54);
                    let samples = std::array::from_fn(|n| {
                        shaders::psh::TexMode::from_bits((modes >> (5 * n)) & 31).texture_dim().is_some()
                    });
                    let mut stages = Stages { sources: [Source::Inherited; 4], signs: [None; 4], samples };
                    for binding in pass.textures.get(imp.textures.indices()).ok_or("unreadable stage bindings")? {
                        let stage = stages.sources.get_mut(usize::from(binding.stage)).ok_or("invalid texture stage")?;
                        *stage = if binding.slot == u8::MAX {
                            match binding.source {
                                0 => Source::Inherited,
                                1 => Source::Rasterizer(crate::native_lights_glue::NORMALIZATION_DATUM, 0),
                                2 | 3 => Source::Texaccum,
                                9 => Source::Rasterizer(crate::native_lights_glue::ATTENUATION_DATUM, 0),
                                11 => Source::Lightmap,
                                14 | 15 => Source::Rasterizer(20, i32::from(binding.source == 15)),
                                16 => Source::ViewBitmap,
                                _ => Source::Surface,
                            }
                        } else {
                            // The final write to a parameter slot wins.
                            row.slots.iter().rev().find(|(n, _)| *n == binding.slot)
                                .map_or(Source::Slot(binding.slot), |(_, value)| *value)
                        };
                    }
                    let states = pass.stage_states.get(imp.stage_states.indices()).ok_or("unreadable sampler states")?;
                    for (sign, sampler) in stages.signs.iter_mut().zip(states) {
                        *sign = Some(sampler.color_sign);
                    }
                    let vertex = imp.vertex_shader.id;
                    let classes = programs.entry(vertex).or_insert_with(|| {
                        let Some(address) = map.tag(vertex).ok().and_then(|t| t.location).map(|l| l.address)
                            else { return Arc::default() };
                        let meta = library.metadata();
                        let count = native_render::look::classification_count(meta, address)
                            .filter(|&n| n <= tags::shader::MAX_BLOCK_ELEMENTS).unwrap_or(0);
                        (0..count).map(|class| program(meta, address, class)).collect()
                    });
                    Ok(Pass { classes: Arc::clone(classes), stages, textures: Mutex::default() })
                })();
                out.passes.insert(row.key, result);
            }
        }
        out
    }

    pub(super) fn is_empty(&self) -> bool {
        !self.passes.values().any(Result::is_ok)
    }

    pub(super) fn get(&self, part: &SortedPart) -> Result<Material<'_>, &'static str> {
        let key = Key { shader: part.shader, lod: part.lod, layer: u8::try_from(part.layer)
            .map_err(|_| "invalid opaque layer")?, pass: part.pass, implementation: part.implementation };
        let pass = self.passes.get(&key).ok_or_else(|| self.rejected.get(&part.shader).copied()
            .unwrap_or("opaque material pass absent from BSP"))?.as_ref().map_err(|s| *s)?;
        // A patched class can be valid at the device even without matching
        // map microcode. The section's live stream plan checks drawability.
        let code = pass.classes.get(usize::from(part.classification)).cloned().flatten();
        Ok(Material { code, stages: pass.stages, textures: &pass.textures })
    }
}

fn selections(shader: TagId, params: &[BitmapParameter], template: &TemplatePostprocess)
    -> Result<Vec<Selection>, &'static str>
{
    let mut selected = Vec::new();
    let mut work = 0usize;
    for (lod, row) in template.lods.iter().take(256).enumerate() {
        for layer in OPAQUE_LAYERS {
            if layer >= row.layers.count { continue; }
            let passes = template.layers.get(usize::from(row.layers.start) + usize::from(layer))
                .ok_or("invalid shader layer span")?;
            let passes = template.passes.get(passes.indices()).ok_or("invalid shader pass span")?;
            for (pi, p) in passes.iter().enumerate() {
                let bindings = template.implementations.get(p.implementations.indices())
                    .ok_or("invalid shader implementation span")?;
                for (i, row) in bindings.iter().enumerate() {
                    let bitmaps = template.bindings.get(row.bitmaps.indices()).ok_or("invalid bitmap binding span")?;
                    work += 1 + bitmaps.len();
                    if selected.len() == MAX_MATERIAL_IMPLEMENTATIONS || work > MAX_MATERIAL_WORK {
                        return Err("shader pass expansion exceeds budget");
                    }
                    let slots = bitmaps.iter().map(|b| {
                        let param = params.get(usize::from(b.source)).ok_or("invalid bitmap parameter index")?;
                        let source = if param.bitmap == TagId::NULL {
                            Source::Rasterizer(DEFAULT_BITMAP, 0)
                        } else { Source::Bitmap(param.bitmap, param.image) };
                        Ok((b.word as u8, source))
                    }).collect::<Result<_, &'static str>>()?;
                    let key = Key { shader: shader.0, layer, lod: lod as u8, pass: pi as u8, implementation: i as u8 };
                    selected.push(Selection { key, pass: p.pass.id, slots });
                }
            }
        }
    }
    Ok(selected)
}

/// The device selects four instruction words per slot, through FINAL. Do
/// not cache the prefix/header or another classification's trailing data.
fn program(meta: &Metadata, address: u32, class: u32) -> Option<Microcode> {
    fn word(meta: &Metadata, at: u32) -> Option<u32> {
        Some(u32::from_le_bytes(meta.bytes(at, 4).ok()?.try_into().ok()?))
    }
    if class >= word(meta, address.checked_add(4)?)? { return None; }
    let desc = word(meta, address.checked_add(8)?)?.checked_add(class.checked_mul(0x1c)?)?;
    let size = word(meta, desc.checked_add(12)?)?;
    let data = word(meta, desc.checked_add(16)?)?;
    let bytes = meta.bytes(data, size as usize).ok()?;
    let header: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    // LoadVertexShaderProgram consumes the third header byte as its count.
    let count = usize::from(header[2]);
    let words = bytes.get(4..4 + count * 16)?.as_chunks::<4>().0;
    let end = words.chunks_exact(4).position(|slot| u32::from_le_bytes(slot[3]) & 1 != 0)?;
    let words: Arc<[u32]> = words[..4 * (end + 1)].iter().copied().map(u32::from_le_bytes).collect();
    Some(Microcode::new(words))
}

fn word(m: &dyn GuestMemory, base: u32, offset: u32) -> Option<u32> {
    m.u32(base.checked_add(offset)?).ok()
}

fn bitmap_root(m: &dyn GuestMemory, datum: TagId) -> Option<u32> {
    if datum == TagId::NULL { return None; }
    let table = m.u32(engine::tags::TAG_ENTRIES_GLOBAL).ok()?;
    let entry = table.checked_add((datum.0 & 0xffff) * 16)?;
    if word(m, entry, 4)? != datum.0 || word(m, entry, 0)? != u32::from_be_bytes(*b"bitm") { return None; }
    word(m, entry, 8).filter(|&r| r != 0)
}

fn rasterizer(m: &dyn GuestMemory, offset: u32) -> Option<TagId> {
    word(m, m.u32(RASTERIZER).ok()?, offset).map(TagId)
}

fn resident(m: &dyn GuestMemory, datum: TagId, frame: i32) -> Option<u32> {
    let mut bitmap = bitmap_root(m, datum)?;
    let count = m.i32(bitmap.checked_add(0x44)?).ok()?;
    let mut index = if count > 0 { frame % count } else { -1 };
    if index < 0 {
        bitmap = bitmap_root(m, rasterizer(m, DEFAULT_BITMAP)?)?;
        index = 0;
    }
    let image = word(m, bitmap, 0x48)?.checked_add((index as u32).checked_mul(IMAGE_SIZE)?)?;
    let streamed = m.u8(image.checked_add(0x0f)?).ok()? & 2 != 0;
    if streamed && m.i32(image.checked_add(0x70)?).ok()? <= m.i32(BITMAP_EPOCH).ok()? {
        return None;
    }
    word(m, image, 0x50).filter(|&h| h != 0)
}

fn lightmap(m: &dyn GuestMemory, binding: Option<world_lightmap::Binding>) -> Option<(TagId, i32)> {
    let map = m.u32(world_walk::LIGHTMAP).ok()?;
    let bsp = m.u32(feature::native_world::ACTIVE_BSP).ok()?;
    if map == 0 || bsp == 0 || m.i32(map.checked_add(0x80)?).ok()? <= 0 { return None; }
    let group = word(m, map, 0x84)?;
    let datum = TagId(word(m, group, 0x1c)?);
    let page = binding.map(|b| b.page).or_else(|| m.i32(world_lightmap::PAGE).ok())?;
    (datum != TagId::NULL && page >= 0 && word(m, group, 4)? == word(m, bsp, 8)?).then_some((datum, page))
}

impl Material<'_> {
    pub(super) fn samples_texaccum(&self) -> bool {
        self.stages.sources.iter().zip(self.stages.samples).any(|(source, sampled)| {
            sampled && matches!(source, Source::Texaccum)
        })
    }

    pub(super) fn textures<B: Backend>(&self, m: &dyn GuestMemory, device: &mut Device<B>,
        binding: Option<world_lightmap::Binding>) -> [Option<PreparedStageTexture>; 4]
    {
        let mut cache = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        std::array::from_fn(|stage| {
            if !self.stages.samples[stage] { return None; }
            let header = match self.stages.sources[stage] {
                Source::Bitmap(id, image) => resident(m, id, image)?,
                Source::Rasterizer(offset, image) => resident(m, rasterizer(m, offset)?, image)?,
                Source::ViewBitmap => {
                    const VIEW_BITMAP: u32 = 0x004e_6978;
                    resident(m, TagId(m.u32(VIEW_BITMAP).ok()?), m.i32(VIEW_BITMAP + 4).ok()?)?
                }
                Source::Slot(n) => {
                    let slot = SLOT_BANK.checked_add_signed(i32::from(n as i8) * 64)?;
                    let frame = m.f32(slot.checked_add(0x18)?).ok()?;
                    if !frame.is_finite() { return None; }
                    resident(m, TagId(word(m, slot, 0)?), frame.floor() as i32)?
                }
                Source::Lightmap => {
                    let (id, image) = lightmap(m, binding)?;
                    resident(m, id, image)?
                }
                // Either source may hold a rendered level. Raster work that
                // produces it must precede its texture read or writeback.
                Source::Inherited | Source::Surface | Source::Texaccum => return None,
            };
            let mut palette = device.bound_palette_header(stage)?;
            if stage == 3 && let Some(b) = binding {
                let requested = i32::from(b.palette);
                let last = m.i32(world_lightmap::LAST_PALETTE).ok()?;
                let cached = m.i32(world_lightmap::STAGE_PALETTES + 12).ok()?;
                let count = m.i32(world_lightmap::PALETTE_COUNT).ok()?;
                if let Some(header) = world_lightmap::palette_header(requested, last, cached, count) {
                    palette = header;
                }
            }
            let sign = self.stages.signs[stage].or_else(|| word(m, STAGE_STATES + stage as u32 * 0x80, 0x28))?;
            let resource = (header, palette, sign);
            // Stable materials re-use the hint across sections and frames.
            // The device validates the current data identity, dirty ranges
            // and palette at submission, including after an eviction.
            if let Some(prior) = cache[stage] && prior.resource == resource {
                return Some(prior.hint);
            }
            let hint = device.prepare_stage_texture(&super::ReadMemory(m), header, palette, sign)?;
            cache[stage] = Some(CachedStage { resource, hint });
            Some(hint)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;
    use tags::shader::{ImplementationBindings, IndexRange, ParameterBinding, TagRef, TemplateLod, TemplatePass};

    #[test]
    fn menu_and_later_lods_keep_every_selection_independent() {
        let span = |start, count| IndexRange { start, count };
        let pass = TemplatePass {
            pass: TagRef { group: map::FourCC(*b"spas"), id: TagId(123) },
            implementations: span(0, 4),
        };
        let mut layers = vec![span(0, 0); 18];
        layers[3] = span(0, 1);
        layers[9 + 8] = span(1, 2);
        let template = TemplatePostprocess {
            lods: vec![TemplateLod { layers: span(0, 9), layer_mask: 8, unknown_6: 0 },
                       TemplateLod { layers: span(9, 9), layer_mask: 256, unknown_6: 0 }],
            layers, passes: vec![pass; 3], implementations: vec![
                ImplementationBindings { bitmaps: span(0, 2), ..Default::default() }; 4],
            bindings: vec![ParameterBinding { word: 2, b2: 0, source: 1 },
                           ParameterBinding { word: 2, b2: 0, source: 0 }],
        };
        let params = [BitmapParameter { bitmap: TagId(901), image: 1, lod_offset: 0.0 },
                      BitmapParameter { bitmap: TagId::NULL, image: 0, lod_offset: 0.0 }];
        let plans = selections(TagId(500), &params, &template).unwrap();
        let keys: Vec<_> = plans.iter().map(|p| (p.key.lod, p.key.layer, p.key.pass, p.key.implementation)).collect();
        assert_eq!(keys.len(), 12);
        assert!(keys.contains(&(0, 3, 0, 3)));
        assert!(keys.contains(&(1, 8, 1, 3)));
        assert!(!keys.contains(&(1, 3, 0, 3)));
        assert!(matches!(plans[0].slots[..], [(2, Source::Rasterizer(DEFAULT_BITMAP, 0)), (2, Source::Bitmap(TagId(901), 1))]));
    }

    #[test]
    fn cached_words_match_the_selected_class_through_its_first_final_instruction() {
        let base = 0x1000;
        let mut bytes = vec![0u8; 256];
        let mut set = |off: usize, value: u32| bytes[off..off + 4].copy_from_slice(&value.to_le_bytes());
        set(4, 2);
        set(8, base + 16);
        set(16 + 0x1c + 12, 52);
        set(16 + 0x1c + 16, base + 120);
        set(120, 3 << 16);
        for (i, word) in [23, 54, 81, 0, 17, 88, 47, 1, 321, 65, 71, 1].into_iter().enumerate() {
            set(124 + i * 4, word);
        }
        let meta = Metadata::new(base, bytes);
        assert!(program(&meta, base, 0).is_none());
        let code = program(&meta, base, 1).unwrap();
        assert_eq!(&*code, &[23, 54, 81, 0, 17, 88, 47, 1]);
        assert!(program(&meta, base, 2).is_none());
    }

    #[test]
    fn prepared_bitmap_requires_current_tag_and_strictly_live_lease() {
        let mut m = FakeGuest::new();
        let id = TagId(0xe123_000a);
        let (table, bitmap, images) = (0x1000, 0x2000, 0x3000);
        m.map(engine::tags::TAG_ENTRIES_GLOBAL, 4).map(BITMAP_EPOCH, 4).map(table, 0x3000);
        m.set_u32(engine::tags::TAG_ENTRIES_GLOBAL, table).unwrap();
        m.set_u32(table + 10 * 16, u32::from_be_bytes(*b"bitm")).unwrap();
        m.set_u32(table + 10 * 16 + 4, id.0).unwrap();
        m.set_u32(table + 10 * 16 + 8, bitmap).unwrap();
        m.set_u32(bitmap + 0x44, 2).unwrap();
        m.set_u32(bitmap + 0x48, images).unwrap();
        m.set_u32(BITMAP_EPOCH, 5).unwrap();
        m.set_u8(images + IMAGE_SIZE + 0x0f, 2).unwrap();
        m.set_u32(images + IMAGE_SIZE + 0x50, 0x5678).unwrap();
        m.set_u32(images + IMAGE_SIZE + 0x70, 6).unwrap();
        assert_eq!(resident(&m, id, 5), Some(0x5678));
        assert_eq!(resident(&m, TagId(0xe124_000a), 5), None);
        m.set_u32(BITMAP_EPOCH, 6).unwrap();
        assert_eq!(resident(&m, id, 5), None);
        m.set_u8(images + IMAGE_SIZE + 0x0f, 0).unwrap();
        assert_eq!(resident(&m, id, 5), Some(0x5678));
    }
}
