//! The sky's materials from the plan (docs/graphics/retire-d3d-r5.md, "The
//! sky from the plan"): the world's worker resolves the shader of every
//! material of every scenario sky's render model ([`load`]); at the sky's
//! indexed strip the section part's shader has its layer-15 pass evaluated
//! at the render clock ([`evaluate`], `native_render::sky_material`), which
//! the strip's pass draws with and checks against what the guest bound
//! (`IndexedPass::for_sky`, `constants`). The load also makes each
//! layer-15 pass's program and textures from the material
//! (`decorators_load::decorator_look` at layer 15, the `vrtx`'s rigid
//! classification, its bitmap stages decoded), checked against the strip
//! the guest draws ([`look_of`], `SkyLook::check`, `sky_plan_<outcome>`),
//! and reads each sky model's sections from the map
//! (`tags::model::RenderModelGeometry`), checked against the strip's
//! streams and indices ([`section_of`], `SkySection::check`,
//! `sky_geometry_<outcome>`).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::sync::{Arc, OnceLock};

use guest_layout::memory::GuestMemory;
use map::{FourCC, Map, TagId};
use native_render::look::{self, RIGID_CLASSIFICATION, Stage};
use native_render::sky_material::{SKY_LAYER, SkyConstants, SkyLook, SkySection, evaluate as evaluate_pass};
use tags::material::Material;
use tags::shader::AnimatedProperty;

use crate::native_world_glue::{ReadMemory, StageDecoder, StageImages, StagePalettes, State, current_ready};

const SKY: FourCC = FourCC(*b"sky ");
const MODE: FourCC = FourCC(*b"mode");
const SHAD: FourCC = FourCC(*b"shad");
/// `sky ` `+0x0`: the render model's tag reference (its datum at `+4`).
const SKY_MODEL_DATUM: u32 = 4;
/// `mode` `+0x60`: the materials block (count, address) of 0x20-byte
/// records, each with the shader's tag reference at `+0x8` (its datum at
/// `+0xc`). docs/formats: `crates/formats/tags/src/defs/mode.txt`.
const MODE_MATERIALS: u32 = 0x60;
const MATERIAL_SIZE: u32 = 0x20;
const MATERIAL_SHADER_DATUM: u32 = 0xc;
/// A bound on the records read per model.
const MAX_MATERIALS: u32 = 256;

/// One sky shader: its resolved material and the instance's animated
/// properties the layer-15 pass samples.
pub struct SkyShader {
    pub material: Material,
    pub properties: Vec<AnimatedProperty>,
    /// Per layer-15 pass, its program and textures from the material, or
    /// why the load could not make them.
    pub looks: Vec<Result<Arc<SkyLook>, String>>,
}

/// The materials of the map's skies, by shader datum, and the shaders that
/// did not resolve, with why.
#[derive(Default)]
pub struct SkyMaterials {
    pub shaders: HashMap<u32, SkyShader>,
    pub refused: HashMap<u32, String>,
    /// The bitmap images the looks' stages sample, decoded at the load.
    pub images: StageImages,
    /// The device palettes the palettized images expanded through.
    pub palettes: StagePalettes,
    /// Each sky render model's sections, by model datum, or why the model
    /// does not read.
    pub models: HashMap<u32, Result<Vec<Arc<SkySection>>, String>>,
}

/// Raw section blocks over the map's resource files.
struct Files<'a>(&'a mut assets::ResourceFiles);

impl tags::geometry::RawSource for Files<'_> {
    fn read_raw(&mut self, pointer: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.0.read(pointer.file, pointer.offset, size).map_err(|_| tags::Error::MissingResource(pointer.file))
    }
}

/// The render model datum of each `sky ` tag, each once, in tag order.
fn sky_models(skies: &[u32], word: &dyn Fn(u32) -> Option<u32>) -> Vec<u32> {
    let mut seen = HashSet::new();
    skies.iter().filter_map(|&sky| word(sky.wrapping_add(SKY_MODEL_DATUM)))
        .filter(|&model| model != u32::MAX && seen.insert(model)).collect()
}

/// The shader datums of every material of every `sky ` tag's render model,
/// each once, in tag order. `word` reads the tag metadata.
fn sky_shaders(skies: &[u32], mode_root: &dyn Fn(u32) -> Option<u32>, word: &dyn Fn(u32) -> Option<u32>) -> Vec<u32> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for &sky in skies {
        let Some(model) = word(sky.wrapping_add(SKY_MODEL_DATUM)) else { continue };
        let Some(root) = mode_root(model) else { continue };
        let (Some(count), Some(at)) = (word(root + MODE_MATERIALS), word(root + MODE_MATERIALS + 4)) else { continue };
        for k in 0..count.min(MAX_MATERIALS) {
            let Some(shader) = word(at.wrapping_add(k * MATERIAL_SIZE + MATERIAL_SHADER_DATUM)) else { break };
            if shader != u32::MAX && seen.insert(shader) {
                out.push(shader);
            }
        }
    }
    out
}

/// Resolves the materials of the map's skies. Called from the world's
/// worker; a shader that does not resolve is named in `refused`, and its
/// strip keeps its guest draw (`shader`).
pub fn load<R: Read + Seek>(map: &mut Map<R>, files: &mut assets::ResourceFiles, decoder: &mut StageDecoder) -> Arc<SkyMaterials> {
    let mut out = SkyMaterials::default();
    let mut lib = match tags::material::ShaderLibrary::read(map) {
        Ok(lib) => lib,
        Err(e) => {
            eprintln!("native-sky: the shader tags do not read ({e}); every sky strip keeps its guest draw (scene_kept_sky_shader)");
            return Arc::new(out);
        }
    };
    let (shaders, models) = {
        let meta = lib.metadata();
        let word = |a: u32| -> Option<u32> { Some(u32::from_le_bytes(meta.bytes(a, 4).ok()?.try_into().ok()?)) };
        let root = |id: TagId, group: FourCC| map.tags().iter().find(|t| t.id == id && t.group == group).and_then(|t| t.location.map(|l| l.address));
        let skies: Vec<u32> = map.tags().iter().filter(|t| t.group == SKY).filter_map(|t| t.location.map(|l| l.address)).collect();
        (sky_shaders(&skies, &|datum| root(TagId(datum), MODE), &word), sky_models(&skies, &word))
    };
    for model in models {
        let read = tags::model::RenderModelGeometry::read(map, TagId(model), &mut Files(&mut *files))
            .map(|geo| geo.sections.iter().map(|s| Arc::new(SkySection::of(&s.geometry))).collect::<Vec<_>>())
            .map_err(|e| e.to_string());
        if let Err(e) = &read {
            eprintln!("native-sky: sky model {model:#010x} does not read ({e}); its strips are counted sky_geometry_model");
        }
        out.models.insert(model, read);
    }
    for datum in shaders {
        let id = TagId(datum);
        if lib.group_of(id) != Some(SHAD) {
            out.refused.insert(datum, "not a shader".into());
            continue;
        }
        let properties = native_render::layer_pass::instance_properties(&lib, id);
        match lib.material(id, None) {
            Ok(material) => {
                let passes = material.layers.iter().find(|l| l.lod == 0 && l.layer == SKY_LAYER).map_or(0, |l| l.passes.len());
                let mut looks = Vec::with_capacity(passes);
                for pass in 0..passes.min(usize::from(u8::MAX)) {
                    let meta = lib.metadata();
                    let mut program = |vrtx: TagId| {
                        let address = map.tag(vrtx).ok()?.location?.address;
                        Some((
                            look::vertex_program(meta, address, RIGID_CLASSIFICATION)?,
                            look::vertex_semantics(meta, address, RIGID_CLASSIFICATION)?,
                        ))
                    };
                    let made = native_render::decorators_load::decorator_look(&material, 0, SKY_LAYER as u8, pass as u8, &mut program)
                        .and_then(|l| {
                            for (n, stage) in l.look.textures.iter().enumerate() {
                                match *stage {
                                    Stage::Empty => {}
                                    Stage::Bitmap { bitmap, image } => {
                                        decoder.decode(map, files, &mut out.images, bitmap, image, n, true)?;
                                    }
                                    _ => return Err(format!("stage {n} samples {stage:?}, which the load does not supply")),
                                }
                            }
                            SkyLook::new(&l.microcode, l.look.textures).map(Arc::new).ok_or_else(|| "the program does not parse".to_owned())
                        });
                    looks.push(made);
                }
                out.shaders.insert(datum, SkyShader { material, properties, looks });
            }
            Err(e) => {
                out.refused.insert(datum, format!("material: {e}"));
            }
        }
    }
    out.palettes = decoder.used().clone();
    if !out.refused.is_empty() {
        eprintln!("native-sky: {} sky shader(s) do not resolve; their strips keep their guest draw (scene_kept_sky_shader)", out.refused.len());
    }
    Arc::new(out)
}

/// The world's state, for the strip's evaluation on the guest thread.
static STATE: OnceLock<State> = OnceLock::new();

pub fn install(state: State) {
    let _ = STATE.set(state);
}

/// The evaluated constants of pass `pass` of sky shader `shader` at the
/// render clock, or why not, the strip's `scene_kept_sky_<cause>`:
/// `unloaded` (no world, or its sky materials not loaded yet),
/// `shader` (the shader did not resolve), or
/// `native_render::sky_material::evaluate`'s.
pub fn evaluate(mem: &dyn GuestMemory, shader: u32, pass: u32) -> Result<SkyConstants, &'static str> {
    let state = STATE.get().ok_or("unloaded")?;
    let ready = {
        let state = state.lock().unwrap_or_else(|e| e.into_inner());
        current_ready(&state, mem).map(Arc::clone).ok_or("unloaded")?
    };
    let skies = ready.sky.get().ok_or("unloaded")?;
    let sky = skies.shaders.get(&shader).ok_or("shader")?;
    let time = native_render::layer_pass::render_time(&ReadMemory(mem));
    evaluate_pass(&sky.material, &sky.properties, pass, time)
}

/// Pass `pass` of sky shader `shader` as the load made it from the
/// material, or why not: `unloaded`, `shader`, `look` (the load could not
/// make it, logged at the load).
pub fn look_of(mem: &dyn GuestMemory, shader: u32, pass: u32) -> Result<Arc<SkyLook>, &'static str> {
    let state = STATE.get().ok_or("unloaded")?;
    let ready = {
        let state = state.lock().unwrap_or_else(|e| e.into_inner());
        current_ready(&state, mem).map(Arc::clone).ok_or("unloaded")?
    };
    let skies = ready.sky.get().ok_or("unloaded")?;
    let sky = skies.shaders.get(&shader).ok_or("shader")?;
    let look = sky.looks.get(usize::try_from(pass).map_err(|_| "look")?).ok_or("look")?;
    look.as_ref().map(Arc::clone).map_err(|_| "look")
}

/// Section `section` of sky model `model` as the load read it, or why
/// not: `unloaded`, `model` (it did not read, logged at the load),
/// `section`.
pub fn section_of(mem: &dyn GuestMemory, model: u32, section: u32) -> Result<Arc<SkySection>, &'static str> {
    let state = STATE.get().ok_or("unloaded")?;
    let ready = {
        let state = state.lock().unwrap_or_else(|e| e.into_inner());
        current_ready(&state, mem).map(Arc::clone).ok_or("unloaded")?
    };
    let skies = ready.sky.get().ok_or("unloaded")?;
    let sections = skies.models.get(&model).ok_or("model")?.as_ref().map_err(|_| "model")?;
    sections.get(usize::try_from(section).map_err(|_| "section")?).map(Arc::clone).ok_or("section")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two skies over one model and one of their own, a shared shader, a
    /// null material, a sky whose model does not read.
    #[test]
    fn each_skys_model_names_its_shaders_once() {
        let mut words: HashMap<u32, u32> = HashMap::new();
        let (sky_a, sky_b, sky_c, sky_d) = (0x1000, 0x1100, 0x1200, 0x1300);
        let (mode_1, mode_2) = (0x2000, 0x3000);
        let (materials_1, materials_2) = (0x2100, 0x3100);
        words.insert(sky_a + 4, 0xe000_0001);
        words.insert(sky_b + 4, 0xe000_0001);
        words.insert(sky_c + 4, 0xe000_0002);
        words.insert(sky_d + 4, 0xe000_0009);
        words.insert(mode_1 + MODE_MATERIALS, 2);
        words.insert(mode_1 + MODE_MATERIALS + 4, materials_1);
        words.insert(materials_1 + MATERIAL_SHADER_DATUM, 0xe000_0100);
        words.insert(materials_1 + MATERIAL_SIZE + MATERIAL_SHADER_DATUM, u32::MAX);
        words.insert(mode_2 + MODE_MATERIALS, 2);
        words.insert(mode_2 + MODE_MATERIALS + 4, materials_2);
        words.insert(materials_2 + MATERIAL_SHADER_DATUM, 0xe000_0101);
        words.insert(materials_2 + MATERIAL_SIZE + MATERIAL_SHADER_DATUM, 0xe000_0100);
        let word = |a: u32| words.get(&a).copied();
        let root = |datum: u32| match datum {
            0xe000_0001 => Some(mode_1),
            0xe000_0002 => Some(mode_2),
            _ => None,
        };
        let shaders = sky_shaders(&[sky_a, sky_b, sky_c, sky_d], &root, &word);
        assert_eq!(shaders, vec![0xe000_0100, 0xe000_0101]);
        let models = sky_models(&[sky_a, sky_b, sky_c, sky_d, 0x1400], &word);
        assert_eq!(models, vec![0xe000_0001, 0xe000_0002, 0xe000_0009]);
    }
}
