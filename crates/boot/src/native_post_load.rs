//! The screen effects' part of a map load (`native-post`,
//! docs/graphics/native-renderer.md, stage 5, "The screen effects"):
//! every `egor` tag's shader resolved, each implementation of each of its
//! layer-15 template passes looked at (`native_render::screen_look`) and
//! translated with its vertex shader's first program (`0x00016610` loads
//! it with classification 0, `screen_look::SCREEN_CLASSIFICATION`), and
//! the shader's stage bitmaps decoded.
//! What does not load is counted by reason. Nothing here is drawn yet:
//! the screen effects stay refused (`native_post_glue::missing`) until the
//! GPU pass is in.

use std::collections::HashMap;

use map::{FourCC, Map, TagId};
use native_render::gpu::Image;
use native_render::look;
use native_render::screen_look::{
    SCREEN_CLASSIFICATION, ScreenProgram, ScreenStage, TEMPLATE_LAYER, compile_screen, screen_look,
};
use tags::material::{Material, ShaderLibrary};

use crate::native_world_glue::decode_image;

/// The `egor` group.
pub const EGOR: FourCC = FourCC(*b"egor");
/// The egor's shader reference's tag id (`engine::screen_effect`: the
/// definition's `+0x44`).
pub const EGOR_SHADER: u32 = 0x44;
/// One egor's template pass, loaded.
pub struct EffectPrograms {
    pub shader: TagId,
    /// Per (layer pass, implementation), the translated pass.
    pub programs: HashMap<(usize, usize), ScreenProgram>,
    /// Per (layer pass, implementation), why it did not load.
    pub refused: HashMap<(usize, usize), String>,
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
    /// The pass to draw for `egor`'s layer pass `layer_pass` with
    /// implementation `implementation` (`engine::screen_effect::Pass`'s
    /// `implementation` and `selector`).
    pub fn program(&self, egor: TagId, layer_pass: usize, implementation: usize) -> Option<&ScreenProgram> {
        self.effects.get(&egor)?.programs.get(&(layer_pass, implementation))
    }
}

/// Every (layer pass, implementation) of `material`'s layer 15 (first
/// level of detail), in order.
pub fn template_implementations(material: &Material) -> Vec<(usize, usize)> {
    material
        .layers
        .iter()
        .filter(|l| l.lod == 0 && l.layer == TEMPLATE_LAYER)
        .flat_map(|l| &l.passes)
        .flat_map(|p| p.implementations.iter().map(|i| (p.index, i.index)))
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
        for key @ (layer_pass, implementation) in template_implementations(&material) {
            let look = match screen_look(&material, layer_pass, implementation) {
                Ok(l) => l,
                Err(r) => {
                    e.refused.insert(key, format!("{r:?}"));
                    continue;
                }
            };
            let Some(vrtx) = map.tag(look.vertex_shader).ok().and_then(|t| t.location.as_ref()).map(|l| l.address) else {
                e.refused.insert(key, "no vertex shader tag".into());
                continue;
            };
            let Some(code) = look::vertex_program(lib.metadata(), vrtx, SCREEN_CLASSIFICATION) else {
                e.refused.insert(key, "no vertex program at classification 0".into());
                continue;
            };
            match compile_screen(&look, &code) {
                Ok(p) => {
                    e.programs.insert(key, p);
                }
                Err(w) => {
                    e.refused.insert(key, w);
                    continue;
                }
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
    let mut kept: Vec<&String> = out.effects.values().flat_map(|e| e.refused.values()).collect();
    kept.sort();
    let mut why: Vec<_> = out.refused.iter().collect();
    why.sort();
    eprintln!(
        "native-post: {} of {} screen effects loaded ({passes} translated passes, {} bitmaps; implementations kept on D3D: {kept:?}); not loaded: {why:?}",
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
    fn the_template_implementations_are_every_pass_of_layer_15() {
        let layer = |lod, layer, imps: &[usize]| ResolvedLayer {
            lod,
            layer,
            passes: (0..2)
                .map(|p| ResolvedPass {
                    index: p,
                    pass: TagId(0xe000_0200 + p as u32),
                    implementations: imps.iter().map(|&i| implementation(i)).collect(),
                })
                .collect(),
        };
        let m = Material {
            shader: TagId(0xe000_0100),
            template: TagId(0xe000_0101),
            inputs: EngineInputs::default(),
            layers: vec![layer(0, 3, &[9]), layer(1, TEMPLATE_LAYER, &[7]), layer(0, TEMPLATE_LAYER, &[0, 2])],
        };
        assert_eq!(template_implementations(&m), vec![(0, 0), (0, 2), (1, 0), (1, 2)]);
        assert!(ScreenEffects::default().program(TagId(1), 0, 0).is_none());
    }
}
