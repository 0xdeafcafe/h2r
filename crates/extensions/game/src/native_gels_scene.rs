//! A light's shader gel drawn from the scene (docs/graphics/native-gels.md,
//! "The gel from the scene"). Under the cut light frame the constants
//! `0x0004a1e0` never run, so neither does their producer `0x0003a5ab`:
//! nothing draws the gel shader into target 13. The light's sequence
//! draws it itself, before its layers, from what the map load made here:
//! each `ligh` tag's gel shader (`+0xe0`), every pass of its template
//! layer 1 with implementation 0 (`0x00016610(shader, 1, -1, 0, 13, 1)`),
//! looked at as a full-target template pass
//! (`native_render::screen_look::template_look`) and translated with its
//! vertex shader's first program, and the stage bitmaps decoded. The quad
//! is the helper `0x00015ec0`'s (`native_render::gels::quad`) at level 0,
//! the level the light's layers sample.

use std::collections::HashMap;
use std::sync::Mutex;

use map::{Map, TagId};
use native_render::gels;
use native_render::gpu::Image;
use native_render::layer_pass::{PassAnimation, instance_properties};
use native_render::look;
use native_render::screen_gpu::{Dest, PostRaster, ScreenDraw, ScreenSet, ScreenTarget, StripVertex};
use native_render::screen_look::{SCREEN_CLASSIFICATION, ScreenProgram, ScreenStage, compile_screen, template_look};
use tags::material::{Material, ShaderLibrary};

use crate::native_world_glue::decode_image;

/// One layer-1 pass of a gel shader: its layer pass, the translated
/// program, and its animated constants, sampled at each draw's clock.
pub struct GelProgram {
    pub layer_pass: usize,
    pub program: ScreenProgram,
    pub animation: Option<PassAnimation>,
}

/// One gel shader's layer-1 passes, in draw order.
pub struct GelPrograms {
    pub passes: Vec<GelProgram>,
}

/// Every gel shader of a map's light definitions.
#[derive(Default)]
pub struct Gels {
    screens: Mutex<ScreenSet>,
    pub shaders: HashMap<u32, GelPrograms>,
    /// Gel shaders that did not load, with why.
    pub refused: HashMap<u32, String>,
    pub images: HashMap<(TagId, u32), Image>,
}

/// Why a light's gel does not draw from the scene.
pub fn refusal(gels: &Gels, shader: u32) -> Option<&'static str> {
    if gels.shaders.contains_key(&shader) {
        None
    } else if gels.refused.contains_key(&shader) {
        Some("the light's gel shader did not translate")
    } else {
        Some("the light's gel shader is not a map light's")
    }
}

/// The datums of the gel shaders of `map`'s light definitions.
fn gel_shaders<R: std::io::Read + std::io::Seek>(map: &Map<R>, lib: &ShaderLibrary) -> Vec<u32> {
    let mut out: Vec<u32> = map
        .tags()
        .iter()
        .filter(|t| t.group.0 == *b"ligh")
        .filter_map(|t| t.location.as_ref().map(|l| l.address))
        .filter_map(|a| {
            let b = lib.metadata().bytes(a.wrapping_add(engine::light_passes::DEFINITION_GEL_SHADER), 4).ok()?;
            Some(u32::from_le_bytes(b.try_into().ok()?))
        })
        .filter(|&d| d != 0 && d != u32::MAX)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// One gel shader's passes, or why one of them does not load.
fn shader_passes<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    lib: &mut ShaderLibrary,
    files: &mut assets::ResourceFiles,
    images: &mut HashMap<(TagId, u32), Image>,
    shader: u32,
) -> Result<GelPrograms, String> {
    let material = lib.material(TagId(shader), None).map_err(|e| format!("material: {e}"))?;
    let layer_passes: Vec<usize> = material
        .layers
        .iter()
        .filter(|l| l.lod == 0 && l.layer == gels::LAYER as usize)
        .flat_map(|l| l.passes.iter().map(|p| p.index))
        .collect();
    let properties = instance_properties(lib, TagId(shader));
    let mut passes = Vec::with_capacity(layer_passes.len());
    for layer_pass in layer_passes {
        let imp = gels::IMPLEMENTATION;
        let externals = crate::native_post_load::vertex_externals(lib, &material, gels::LAYER as usize, layer_pass, imp)?;
        // An animated pass compiles from its baseline (clock 0, targets
        // cleared) and samples its registers at each draw.
        let (material, animation) = baseline(&material, layer_pass, imp, &properties)
            .map_err(|r| format!("pass {layer_pass}: {r}"))?;
        let look = template_look(&material, gels::LAYER as usize, layer_pass, imp, &externals)
            .map_err(|r| format!("pass {layer_pass}: {r:?}"))?;
        // The draw supplies no engine textures.
        if !look.engine_textures().is_empty() {
            return Err(format!("pass {layer_pass}: samples an engine texture"));
        }
        let vrtx = map
            .tag(look.vertex_shader)
            .ok()
            .and_then(|t| t.location.as_ref())
            .map(|l| l.address)
            .ok_or("no vertex shader tag")?;
        let code = look::vertex_program(lib.metadata(), vrtx, SCREEN_CLASSIFICATION)
            .ok_or("no vertex program at classification 0")?;
        let program = compile_screen(&look, &code)?;
        for t in look.textures {
            if let ScreenStage::Bitmap { bitmap, image } = t
                && !images.contains_key(&(bitmap, image))
            {
                let i = decode_image(map, files, bitmap, image as usize).ok_or("a stage bitmap does not decode")?;
                images.insert((bitmap, image), i);
            }
        }
        passes.push(GelProgram { layer_pass, program, animation });
    }
    if passes.is_empty() {
        return Err("no layer-1 pass".into());
    }
    Ok(GelPrograms { passes })
}

/// `material` with layer 1's pass `layer_pass` implementation
/// `implementation` at its animation's baseline
/// (`PassAnimation::baseline`), and the animation; `material` as it is
/// when the pass animates nothing.
fn baseline<'m>(
    material: &'m Material,
    layer_pass: usize,
    implementation: usize,
    properties: &[tags::shader::AnimatedProperty],
) -> Result<(std::borrow::Cow<'m, Material>, Option<PassAnimation>), String> {
    let at = |m: &Material| {
        m.layers
            .iter()
            .position(|l| l.lod == 0 && l.layer == gels::LAYER as usize)
            .and_then(|l| {
                let p = m.layers[l].passes.iter().position(|p| p.index == layer_pass)?;
                let i = m.layers[l].passes[p].implementations.iter().position(|i| i.index == implementation)?;
                Some((l, p, i))
            })
    };
    let Some((l, p, i)) = at(material) else {
        return Ok((std::borrow::Cow::Borrowed(material), None));
    };
    let imp = &material.layers[l].passes[p].implementations[i];
    if imp.animated.is_empty() {
        return Ok((std::borrow::Cow::Borrowed(material), None));
    }
    let (base, animation) =
        PassAnimation::baseline(imp, properties, false, layer_pass).map_err(|r| format!("animation: {r:?}"))?;
    let mut owned = material.clone();
    owned.layers[l].passes[p].implementations[i] = base;
    Ok((std::borrow::Cow::Owned(owned), animation))
}

/// At map load: every gel shader of `map`'s lights. Says on stderr what
/// loaded.
pub fn load<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    lib: &mut ShaderLibrary,
    files: &mut assets::ResourceFiles,
) -> Gels {
    let mut out = Gels::default();
    for shader in gel_shaders(map, lib) {
        match shader_passes(map, lib, files, &mut out.images, shader) {
            Ok(p) => {
                out.shaders.insert(shader, p);
            }
            Err(why) => {
                out.refused.insert(shader, why);
            }
        }
    }
    let mut why: Vec<&String> = out.refused.values().collect();
    why.sort();
    eprintln!(
        "native-lights: {} of {} gel shaders draw from the scene; kept: {why:?}",
        out.shaders.len(),
        out.shaders.len() + out.refused.len(),
    );
    out
}

/// The strip of the quad helper at one level, as the screen pass takes it.
pub fn strip(size: (u32, u32), depth: (f32, f32)) -> Vec<StripVertex> {
    gels::quad(size, depth)
        .into_iter()
        .map(|(position, t)| StripVertex { position, texcoords: [t; 4] })
        .collect()
}

impl Gels {
    /// `shader`'s passes into target 13's level 0 on one copy, prepared
    /// (recording them makes and writes nothing). `guest` is the level's
    /// guest size, `depth` the quad's z and w.
    pub fn prepare(
        &self,
        t: ScreenTarget<'_>,
        shader: u32,
        depth: (f32, f32),
        values: [u32; 4],
        time: Option<f64>,
    ) -> Result<native_render::screen_gpu::ScreenCommands, String> {
        let gel = self.shaders.get(&shader).ok_or("no gel programs")?;
        let guest = t.color.map(|c| c.guest_size).ok_or("no target 13")?;
        let strips = vec![strip(guest, depth)];
        // Each animated pass's registers at this draw's clock over its
        // compiled uniforms.
        let sampled: Vec<std::borrow::Cow<'_, ScreenProgram>> = gel
            .passes
            .iter()
            .map(|p| match &p.animation {
                None => Ok(std::borrow::Cow::Borrowed(&p.program)),
                Some(a) => {
                    let at = a.sample(time).map_err(|r| format!("animation: {r:?}"))?;
                    let mut program = p.program.clone();
                    program.vertex = at.vertex(&program.vertex);
                    program.pixel = at.pixel(&program.pixel);
                    Ok(std::borrow::Cow::Owned(program))
                }
            })
            .collect::<Result<_, String>>()?;
        let draws: Vec<ScreenDraw<'_>> = gel
            .passes
            .iter()
            .zip(&sampled)
            .map(|(p, program)| ScreenDraw {
                key: (shader, p.layer_pass, gels::IMPLEMENTATION),
                program,
                strips: &strips,
                raster: PostRaster::from(program.state),
                dest: Dest::Bound,
                values,
                convolutions: &[],
                convolution_rasters: &[],
            })
            .collect();
        let mut screens = self.screens.lock().unwrap_or_else(|e| e.into_inner());
        screens
            .prepare(t, &draws, None, &mut |s| match s {
                ScreenStage::Bitmap { bitmap, image } => self.images.get(&(bitmap, image)).cloned(),
                _ => None,
            })
            .map_err(|e| format!("{e:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gel_strip_is_the_helpers_quad_on_every_coordinate_set() {
        let s = strip((32, 16), (0.25, 0.5));
        assert_eq!(s.len(), 4);
        assert_eq!(s[3].position, [32.0, 16.0, 0.25, 0.5]);
        assert_eq!(s[1].texcoords, [[1.0, 0.0, 0.0, 1.0]; 4]);
    }

    #[test]
    fn a_gel_not_loaded_is_named() {
        let mut gels = Gels::default();
        gels.refused.insert(7, "x".into());
        assert_eq!(refusal(&gels, 7), Some("the light's gel shader did not translate"));
        assert!(refusal(&gels, 8).is_some());
        gels.shaders.insert(9, GelPrograms { passes: Vec::new() });
        assert_eq!(refusal(&gels, 9), None);
    }
}
