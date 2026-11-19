//! The screen effects' part of a map load (`native-post`,
//! docs/graphics/native-renderer.md, stage 5, "The screen effects"):
//! every `egor` tag's shader resolved, each implementation of its layer-15
//! template pass looked at (`native_render::screen_look`) and translated
//! for every classification of its vertex shader (which one a pass draws
//! with is `0x00016610`'s to say), and the shader's stage bitmaps decoded.
//! What does not load is counted by reason. Nothing here is drawn yet:
//! the screen effects stay refused (`native_post_glue::missing`) until the
//! GPU pass is in.

use std::collections::HashMap;

use map::{FourCC, Map, TagId};
use native_render::gpu::Image;
use native_render::look;
use native_render::screen_look::{ScreenProgram, ScreenStage, TEMPLATE_LAYER, compile_screen, screen_look};
use tags::material::{Material, ShaderLibrary};

use crate::native_world_glue::decode_image;

/// The `egor` group.
pub const EGOR: FourCC = FourCC(*b"egor");
/// The egor's shader reference's tag id (`engine::screen_effect`: the
/// definition's `+0x44`).
pub const EGOR_SHADER: u32 = 0x44;
/// Most classifications read per vertex shader.
pub const MAX_CLASSIFICATIONS: u32 = 16;

/// One egor's template pass, loaded.
pub struct EffectPrograms {
    pub shader: TagId,
    /// Per (implementation, classification), the translated pass.
    pub programs: HashMap<(usize, u32), ScreenProgram>,
    /// Per implementation, why it did not load (no program for any
    /// classification among them).
    pub refused: HashMap<usize, String>,
}

/// Every egor of a map.
#[derive(Default)]
pub struct ScreenEffects {
    pub effects: HashMap<TagId, EffectPrograms>,
    /// The stage bitmaps (bitmap, image), decoded.
    pub images: HashMap<(TagId, u32), Image>,
    /// Egors that did not load at all, by reason.
    pub refused: HashMap<String, usize>,
}

impl ScreenEffects {
    /// The pass to draw for `egor`'s `implementation` with vertex shader
    /// classification `classification`.
    pub fn program(&self, egor: TagId, implementation: usize, classification: u32) -> Option<&ScreenProgram> {
        self.effects.get(&egor)?.programs.get(&(implementation, classification))
    }
}

/// The implementations of `material`'s layer-15 pass (first level of
/// detail), in order.
pub fn template_implementations(material: &Material) -> Vec<usize> {
    material
        .layers
        .iter()
        .filter(|l| l.lod == 0 && l.layer == TEMPLATE_LAYER)
        .flat_map(|l| l.passes.first())
        .flat_map(|p| p.implementations.iter().map(|i| i.index))
        .collect()
}

/// At map load (the switch on): every egor of `map`. Says on stderr what
/// loaded.
pub(crate) fn load<R: std::io::Read + std::io::Seek>(map: &mut Map<R>, files: &mut assets::ResourceFiles) -> ScreenEffects {
    let mut out = ScreenEffects::default();
    let egors: Vec<(TagId, u32)> = map
        .tags()
        .iter()
        .filter(|t| t.group == EGOR)
        .filter_map(|t| Some((t.id, t.location.as_ref()?.address)))
        .collect();
    if egors.is_empty() {
        return out;
    }
    let mut lib = match ShaderLibrary::read(map) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("native-post: the shader tags do not read ({e}); every screen effect keeps its D3D draws");
            return out;
        }
    };
    for (egor, address) in egors {
        let shader = match lib.metadata().bytes(address.wrapping_add(EGOR_SHADER), 4) {
            Ok(b) => TagId(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            Err(e) => {
                *out.refused.entry(format!("the egor does not read: {e}")).or_default() += 1;
                continue;
            }
        };
        let material = match lib.material(shader, None) {
            Ok(m) => m,
            Err(e) => {
                *out.refused.entry(format!("material: {e}")).or_default() += 1;
                continue;
            }
        };
        let mut e = EffectPrograms { shader, programs: HashMap::new(), refused: HashMap::new() };
        for implementation in template_implementations(&material) {
            let look = match screen_look(&material, implementation) {
                Ok(l) => l,
                Err(r) => {
                    e.refused.insert(implementation, format!("{r:?}"));
                    continue;
                }
            };
            let Some(vrtx) = map.tag(look.vertex_shader).ok().and_then(|t| t.location.as_ref()).map(|l| l.address) else {
                e.refused.insert(implementation, "no vertex shader tag".into());
                continue;
            };
            let count = look::classification_count(lib.metadata(), vrtx).unwrap_or(0).min(MAX_CLASSIFICATIONS);
            let mut why = None;
            for c in 0..count {
                let Some(code) = look::vertex_program(lib.metadata(), vrtx, c) else { continue };
                match compile_screen(&look, &code) {
                    Ok(p) => {
                        e.programs.insert((implementation, c), p);
                    }
                    Err(w) => why = Some(w),
                }
            }
            if !e.programs.keys().any(|&(i, _)| i == implementation) {
                e.refused.insert(implementation, why.unwrap_or_else(|| "no vertex program".into()));
                continue;
            }
            for t in look.textures {
                if let ScreenStage::Bitmap { bitmap, image } = t
                    && !out.images.contains_key(&(bitmap, image))
                    && let Some(i) = decode_image(map, files, bitmap, image as usize)
                {
                    out.images.insert((bitmap, image), i);
                }
            }
        }
        out.effects.insert(egor, e);
    }
    let passes: usize = out.effects.values().map(|e| e.programs.len()).sum();
    let refused: usize = out.effects.values().map(|e| e.refused.len()).sum();
    let mut why: Vec<_> = out.refused.iter().collect();
    why.sort();
    eprintln!(
        "native-post: {} of {} screen effects loaded ({passes} translated passes, {refused} implementations refused, {} bitmaps); not loaded: {why:?}",
        out.effects.len(),
        out.effects.len() + out.refused.values().sum::<usize>(),
        out.images.len(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tags::material::{EngineInputs, InputReads, ResolvedImplementation, ResolvedLayer, ResolvedPass};

    fn implementation(index: usize) -> ResolvedImplementation {
        ResolvedImplementation {
            index,
            vertex_shader: None,
            textures: [None, None, None, None],
            stage_states: Vec::new(),
            render_states: Vec::new(),
            restore_states: Vec::new(),
            pixel_shader: [0; tags::shader::PSDEF_SIZE],
            variant: false,
            c0_written: 0,
            c1_written: 0,
            pixel_constants_from_engine: false,
            per_draw_inputs: false,
            input_reads: InputReads::default(),
            vector_bindings: Vec::new(),
            scalar_bindings: Vec::new(),
            vertex_constants: Vec::new(),
            vertex_defaults: Vec::new(),
            animated: Vec::new(),
            engine_render_states: Vec::new(),
            engine_stage_states: Vec::new(),
        }
    }

    #[test]
    fn the_template_implementations_are_layer_15s_first_pass() {
        let layer = |lod, layer, imps: &[usize]| ResolvedLayer {
            lod,
            layer,
            passes: vec![ResolvedPass {
                index: 0,
                pass: TagId(0xe000_0200),
                implementations: imps.iter().map(|&i| implementation(i)).collect(),
            }],
        };
        let m = Material {
            shader: TagId(0xe000_0100),
            template: TagId(0xe000_0101),
            inputs: EngineInputs::default(),
            layers: vec![layer(0, 3, &[9]), layer(1, TEMPLATE_LAYER, &[7]), layer(0, TEMPLATE_LAYER, &[0, 2])],
        };
        assert_eq!(template_implementations(&m), vec![0, 2]);
        assert!(ScreenEffects::default().program(TagId(1), 0, 0).is_none());
    }
}
