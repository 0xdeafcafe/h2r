//! The sky's materials from the plan (docs/graphics/retire-d3d-r5.md, "The
//! sky from the plan"): the world's worker resolves the shader of every
//! material of every scenario sky's render model ([`load`]); at the sky's
//! indexed strip the section part's shader has its layer-15 pass evaluated
//! at the render clock ([`evaluate`], `native_render::sky_material`), which
//! the strip's pass draws with and checks against what the guest bound
//! (`IndexedPass::for_sky`, `constants`).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::sync::{Arc, OnceLock};

use engine::memory::GuestMemory;
use map::{FourCC, Map, TagId};
use native_render::sky_material::{SkyConstants, evaluate as evaluate_pass};
use tags::material::Material;
use tags::shader::AnimatedProperty;

use crate::native_world_glue::{ReadMemory, State, current_ready};

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
}

/// The materials of the map's skies, by shader datum, and the shaders that
/// did not resolve, with why.
#[derive(Default)]
pub struct SkyMaterials {
    pub shaders: HashMap<u32, SkyShader>,
    pub refused: HashMap<u32, String>,
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
pub fn load<R: Read + Seek>(map: &mut Map<R>) -> Arc<SkyMaterials> {
    let mut out = SkyMaterials::default();
    let mut lib = match tags::material::ShaderLibrary::read(map) {
        Ok(lib) => lib,
        Err(e) => {
            eprintln!("native-sky: the shader tags do not read ({e}); every sky strip keeps its guest draw (scene_kept_sky_shader)");
            return Arc::new(out);
        }
    };
    let shaders = {
        let meta = lib.metadata();
        let word = |a: u32| -> Option<u32> { Some(u32::from_le_bytes(meta.bytes(a, 4).ok()?.try_into().ok()?)) };
        let root = |id: TagId, group: FourCC| map.tags().iter().find(|t| t.id == id && t.group == group).and_then(|t| t.location.map(|l| l.address));
        let skies: Vec<u32> = map.tags().iter().filter(|t| t.group == SKY).filter_map(|t| t.location.map(|l| l.address)).collect();
        sky_shaders(&skies, &|datum| root(TagId(datum), MODE), &word)
    };
    for datum in shaders {
        let id = TagId(datum);
        if lib.group_of(id) != Some(SHAD) {
            out.refused.insert(datum, "not a shader".into());
            continue;
        }
        let properties = native_render::layer_pass::instance_properties(&lib, id);
        match lib.material(id, None) {
            Ok(material) => {
                out.shaders.insert(datum, SkyShader { material, properties });
            }
            Err(e) => {
                out.refused.insert(datum, format!("material: {e}"));
            }
        }
    }
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
    }
}
