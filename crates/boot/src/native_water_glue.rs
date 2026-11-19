//! The native water's load-side census (docs/graphics/native-renderer.md,
//! "Stage 4b: water and the rest"): with the world loaded (the world's
//! worker calls [`census`]), one line per material with a pass in a
//! census layer — water (6), active camouflage (18, 19) and the hologram
//! (21) — naming its template's stage sources: the instance bitmaps, the
//! lightmap, and the engine textures with what each is
//! (`native_render::screen_look::engine_texture`'s table: render targets,
//! the view's bound target, the screen effects' slots), which is the
//! scene-copy and reflection input census. Read-only: nothing is kept
//! and nothing is skipped.
//!
//! The targets' guest surface headers (`0x004b4b58`, stride `0x98`, the
//! surface at `+0x88`) are named in the same lines when a pass would
//! sample them: what a native pass must supply per view, and how (the
//! reservation's `sampled` levels, or a copy of the bound colour for the
//! view's own target).

use std::collections::BTreeSet;

use map::{Map, TagId};
use native_render::screen_look::{engine_texture, EngineTexture};
use native_render::world::World;
use tags::material::{Material, TextureSource};

/// The census layers ([`feature::native_water`]'s).
pub const CENSUS_LAYERS: [usize; 4] = [6, 18, 19, 21];
/// Halo's target table (`0x0001dd60`'s, docs/graphics/native-renderer.md
/// "Stage 5"): `+0x98` per target, the surface header at `+0x88`.
pub const TARGET_TABLE: u32 = 0x004b_4b58;
pub const TARGET_STRIDE: u32 = 0x98;
pub const TARGET_SURFACE: u32 = 0x88;

/// One stage's source, named.
fn source_of(s: TextureSource) -> String {
    match s {
        TextureSource::None => "none".into(),
        TextureSource::Bitmap { bitmap, image, .. } => format!("bitmap {} image {image}", bitmap.0 as u16),
        TextureSource::Default => "the rasterizer default".into(),
        TextureSource::UnsetSlot(n) => format!("unset (slot {n}'s last texture)").into(),
        TextureSource::Lightmap => "the lightmap".into(),
        TextureSource::Engine(id) => {
            let what = match engine_texture(id) {
                EngineTexture::Rasterizer(off) => format!("a rasterizer bitmap (+{off:#x})"),
                EngineTexture::Target(t) => format!("render target {t} (the scene's own at 0x98·{t}+0x88)", t = t),
                EngineTexture::View => "the view's bound target (a scene copy)".into(),
                EngineTexture::Slot(word) => format!("the screen effects' slot at {word:#x}"),
                EngineTexture::Cleared => "a cleared stage".into(),
                EngineTexture::Light => "the light's texture".into(),
                EngineTexture::Dynamic => "chosen at run time".into(),
                EngineTexture::Invalid => format!("invalid id {id}").into(),
            };
            format!("engine texture {id}: {what}")
        }
    }
}

/// Prints the census for `world`'s materials (by BSP index, `shaders` the
/// shader of each) over the census layers. Read-only.
pub fn census<R: std::io::Read + std::io::Seek>(map: &mut Map<R>, shaders: &[TagId], world: &World) {
    let materials: Vec<u16> = world
        .draws
        .iter()
        .map(|d| d.material)
        .chain(world.transparents.iter().map(|t| {
            // The transparents are the water surfaces themselves: census
            // them too (their layer-15 passes may also read the scene).
            t.material
        }))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if materials.is_empty() {
        return;
    }
    let Ok(lib) = tags::material::ShaderLibrary::read(map) else {
        eprintln!("native-water: the shader tags do not read; no census");
        return;
    };
    let mut lines = 0;
    for &m in &materials {
        let Some(&shader) = shaders.get(usize::from(m)) else { continue };
        let Ok(mat) = lib.material(shader, None) else { continue };
        for &layer in &CENSUS_LAYERS {
            let Some(passes) = layer_passes_of(&mat, layer) else { continue };
            for (n, (imp, _)) in passes {
                let stages: Vec<String> = imp
                    .textures
                    .iter()
                    .enumerate()
                    .filter_map(|(s, t)| t.map(|t| format!("stage {s}: {}", source_of(t.source))))
                    .collect();
                eprintln!(
                    "native-water: material {m} (shader {:#010x}) layer {layer} pass {n}: {}",
                    shader.0,
                    stages.join(", ")
                );
                lines += 1;
            }
        }
    }
    if lines == 0 {
        eprintln!("native-water: no material of this world has a pass in layers 6, 18, 19 or 21");
    }
}

/// The layer's passes' implementations that draw (one per pass, with its
/// index), or `None` when the template draws nothing in `layer` at the
/// first level of detail.
fn layer_passes_of(mat: &Material, layer: usize) -> Option<Vec<(&tags::material::ResolvedImplementation, usize)>> {
    let l = mat.layers.iter().find(|l| l.lod == 0 && l.layer == layer)?;
    if l.passes.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for (n, p) in l.passes.iter().enumerate() {
        if let Some(imp) = p.implementations.first() {
            out.push((imp, n));
        }
    }
    (out.is_empty()).then_none()
}

/// The guest surface header of render target `t` (the scene-copy and
/// reflection inputs' attachment), or 0 when it does not read.
pub fn target_header(mem: &dyn engine::memory::GuestMemory, t: u32) -> u32 {
    mem.u32(TARGET_TABLE + TARGET_STRIDE * t + TARGET_SURFACE).unwrap_or(0)
}
