//! The HUD's widget quads drawn from their shader's Rust material, with no
//! guest shader setup (docs/graphics/retire-d3d.md, slice R6;
//! docs/graphics/native-hud-overlay.md, "The material in Rust").
//!
//! A widget quad is predicted from the map: its shader's one pass
//! implementation (`native_render::hud_material::HudShader`), the combiner
//! words with the draw's animated constants through the HUD's function
//! inputs (`render_bridge::native_hud::hud_inputs`) at the render clock,
//! its blend and write state, its stages, and its corners from the quad's
//! own vertices. While a shader is unproven the guest sets it up and the
//! capture (`hud_overlay::draw_tag_quad`) takes the draw; [`check`] then
//! compares the prediction with what the capture took, item field by item
//! field. [`PROOF_DRAWS`] matching quads prove the shader; one that does not
//! match keeps it on the guest's setup for the map. A proven shader's
//! quads are drawn by [`draw`] at `HUD_QUAD_SETUP`, before the guest's
//! setup, which is left out with the guest's draw; one quad in [`RECHECK`]
//! is still set up by the guest and checked again.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use engine::memory::GuestMemory;
use frame_scene::{ClipRect, Overlay, OverlayMaterial, OverlayTexture, OverlayVertex, Program, Sampler};
use map::TagId;
use native_render::hud_material::{HudRefusal, HudShader, HudState};
use render_bridge::native_hud::{HudTagQuad, HudVertices, WalkWidget, hud_inputs};
use tags::material::ShaderLibrary;
use timeline::native::Ui;

/// Matching quads that prove a shader.
pub const PROOF_DRAWS: u32 = 8;
/// A proven shader's quads the guest still sets up: one in this many.
pub const RECHECK: u32 = 32;
/// The render clock `0x0001b230` divides by a property's period: a double.
const RENDER_TIME: u32 = 0x0048_58a0;

/// A quad as the overlay would draw it, but for its target and clip,
/// which the guest's draw gives.
#[derive(Clone, Debug, PartialEq)]
pub struct Predicted {
    pub words: [u32; 60],
    pub state: HudState,
    pub alpha_kill: u8,
    /// The stages the words sample (their texture modes).
    pub sampled: [bool; 4],
    /// Each sampled stage's texture.
    pub stages: [Option<OverlayTexture>; 4],
    /// The first sampled stage's sampler.
    pub sampler: Sampler,
    pub corners: [OverlayVertex; 4],
}

/// Why a captured quad's prediction did not match, by the first part
/// that differed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mismatch {
    Words,
    State,
    Stages,
    Corners,
}

impl Mismatch {
    fn counter(self) -> Ui {
        match self {
            Mismatch::Words => Ui::OverlayHudRustMismatchWords,
            Mismatch::State => Ui::OverlayHudRustMismatchState,
            Mismatch::Stages => Ui::OverlayHudRustMismatchStages,
            Mismatch::Corners => Ui::OverlayHudRustMismatchCorners,
        }
    }
}

/// Why a quad has no prediction.
#[derive(Clone, Debug, PartialEq)]
pub enum Unpredicted {
    /// The map read for the overlay is still loading.
    Loading,
    /// The shader's material is refused.
    Refused(HudRefusal),
    /// The quad's vertices, the HUD's inputs or the clock did not read.
    Inputs,
}

/// One shader's proof on this map.
#[derive(Clone, Debug, Default)]
struct Proof {
    matched: u32,
    /// A captured quad did not match: the guest sets it up for the map.
    failed: bool,
    /// The target and clip of the quads that proved it.
    placed: Option<([u32; 2], ClipRect)>,
    /// Proven quads seen, for the recheck.
    seen: u32,
}

#[derive(Default)]
struct State {
    generation: Option<u64>,
    library: Option<ShaderLibrary>,
    shaders: HashMap<u32, Result<Arc<HudShader>, HudRefusal>>,
    proofs: HashMap<u32, Proof>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);

fn state() -> std::sync::MutexGuard<'static, State> {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let generation = render_bridge::native_map_work::generation();
    if s.generation != Some(generation) {
        *s = State { generation: Some(generation), ..State::default() };
    }
    s
}

/// The shader's material, resolved from the map once.
fn shader(mem: &dyn GuestMemory, datum: u32) -> Result<Arc<HudShader>, Unpredicted> {
    let mut s = state();
    if let Some(known) = s.shaders.get(&datum) {
        return known.clone().map_err(Unpredicted::Refused);
    }
    if s.library.is_none() {
        let read = crate::hud_overlay::with_art_map(mem, |map| ShaderLibrary::read(map)).ok_or(Unpredicted::Loading)?;
        match read {
            Ok(library) => s.library = Some(library),
            Err(e) => {
                let refused = HudRefusal::Material(e.to_string());
                s.shaders.insert(datum, Err(refused.clone()));
                return Err(Unpredicted::Refused(refused));
            }
        }
    }
    let library = s.library.as_mut().ok_or(Unpredicted::Loading)?;
    let resolved = HudShader::resolve(library, TagId(datum)).map(Arc::new);
    s.shaders.insert(datum, resolved.clone());
    resolved.map_err(Unpredicted::Refused)
}

fn render_time(mem: &dyn GuestMemory) -> Option<f64> {
    let lo = mem.u32(RENDER_TIME).ok()?;
    let hi = mem.u32(RENDER_TIME + 4).ok()?;
    Some(f64::from_bits(u64::from(lo) | u64::from(hi) << 32))
}

/// The stages a definition's texture modes (word 54) sample.
fn sampled_stages(words: &[u32; 60]) -> [bool; 4] {
    std::array::from_fn(|n| shaders::psh::TexMode::from_bits((words[54] >> (5 * n)) & 31).texture_dim().is_some())
}

/// The quad's corners from its vertices (`render_bridge::native_hud`'s
/// order: diffuse, texture coordinate, position).
fn corners(vertices: &HudVertices) -> Option<[OverlayVertex; 4]> {
    let mut out = [OverlayVertex::default(); 4];
    for (corner, vertex) in out.iter_mut().zip(vertices) {
        let [(9, colour), (3, uv), (0, position)] = *vertex else { return None };
        *corner = OverlayVertex { position: [position[0], position[1]], uv: [uv[0], uv[1]], color: colour };
    }
    Some(out)
}

/// The quad from its shader's Rust material and its vertices.
pub fn predict(mem: &dyn GuestMemory, at: &HudTagQuad, vertices: &HudVertices) -> Result<Predicted, Unpredicted> {
    let hud = shader(mem, at.shader)?;
    predict_with(&hud, mem, at, vertices)
}

fn predict_with(hud: &HudShader, mem: &dyn GuestMemory, at: &HudTagQuad, vertices: &HudVertices) -> Result<Predicted, Unpredicted> {
    if !hud_inputs::provider_installed(mem) {
        return Err(Unpredicted::Inputs);
    }
    let corners = corners(vertices).ok_or(Unpredicted::Inputs)?;
    let inputs = |name: u32| hud_inputs::function_input(mem, name);
    let words = hud.words(render_time(mem), &inputs).map_err(Unpredicted::Refused)?;
    let sampled = sampled_stages(&words);
    let mut stages = [None; 4];
    for (n, stage) in stages.iter_mut().enumerate() {
        if sampled[n] {
            *stage = Some(hud.stages[n].overlay(at.bitmap, at.image).ok_or(Unpredicted::Refused(HudRefusal::Texture { stage: n }))?);
        }
    }
    let first = sampled.iter().position(|&v| v);
    Ok(Predicted {
        words,
        state: hud.state,
        alpha_kill: hud.alpha_kill,
        sampled,
        stages,
        sampler: first.map_or_else(Sampler::default, |n| hud.samplers[n]),
        corners,
    })
}

/// Whether `captured` (the capture's one-quad overlay) is what `p` draws.
pub fn compare(p: &Predicted, captured: &Overlay) -> Result<(), Mismatch> {
    let item = captured.items.first().ok_or(Mismatch::Corners)?;
    if item.shader_words.as_deref() != Some(&p.words[..]) {
        return Err(Mismatch::Words);
    }
    if item.material.blend != p.state.blend
        || item.blend_constant != p.state.blend_constant
        || item.alpha_test != p.state.alpha_test
        || item.color_write != p.state.color_write
        || item.alpha_kill != p.alpha_kill
    {
        return Err(Mismatch::State);
    }
    let first = p.sampled.iter().position(|&v| v);
    let texture = first.and_then(|n| p.stages[n]).unwrap_or(OverlayTexture::None);
    if (0..4).any(|n| item.shader_stages[n].is_some() != p.sampled[n])
        || item.material.texture != texture
        || item.material.sampler != p.sampler
        || item.glyph_cache != 0
    {
        return Err(Mismatch::Stages);
    }
    if captured.vertices_of(item) != &p.corners[..] {
        return Err(Mismatch::Corners);
    }
    let uv_match = item.stage_uv.as_deref().is_some_and(|uv| {
        (0..4).filter(|&n| p.sampled[n]).all(|n| (0..4).all(|c| uv[c][n] == p.corners[c].uv))
    });
    if !uv_match {
        return Err(Mismatch::Corners);
    }
    Ok(())
}

/// Whether a captured quad of `shader` is still worth predicting: not
/// once a quad of it failed on this map.
pub fn wants_check(shader: u32) -> bool {
    state().proofs.get(&shader).is_none_or(|p| !p.failed)
}

/// A captured widget quad and its prediction: counted, and the shader's
/// proof advanced or failed.
pub fn check(at: &HudTagQuad, predicted: Result<Predicted, Unpredicted>, captured: &Overlay) {
    let predicted = match predicted {
        Ok(p) => p,
        Err(Unpredicted::Loading) => return,
        Err(_) => {
            Ui::OverlayHudRustRefused.count();
            return;
        }
    };
    let result = compare(&predicted, captured);
    let mut s = state();
    let proof = s.proofs.entry(at.shader).or_default();
    let proven = proof.matched >= PROOF_DRAWS;
    match result {
        Ok(()) if !proof.failed => {
            Ui::OverlayHudRustMatched.count();
            let item = &captured.items[0];
            let placed = (captured.size, item.clip);
            if proof.placed.is_some_and(|p| p != placed) {
                // The quads of one shader drawn on two targets or clips
                // (split screen): the skip does not know which.
                proof.failed = true;
                return;
            }
            proof.placed = Some(placed);
            proof.matched += 1;
            if proof.matched == PROOF_DRAWS {
                Ui::OverlayHudRustProven.count();
            }
        }
        Ok(()) => {}
        Err(why) => {
            why.counter().count();
            if proven && !proof.failed {
                Ui::OverlayHudRustRecheckFailed.count();
            }
            proof.failed = true;
        }
    }
}

/// The quad as one overlay item on a target of `size`, inside `clip`.
pub fn build(p: &Predicted, at: &HudTagQuad, size: [u32; 2], clip: ClipRect) -> Option<Overlay> {
    let first = p.sampled.iter().position(|&v| v);
    let material = OverlayMaterial {
        program: Program::Shader { tag: at.shader },
        texture: first.and_then(|n| p.stages[n]).unwrap_or(OverlayTexture::None),
        sampler: p.sampler,
        blend: p.state.blend,
    };
    let mut overlay = Overlay::new(size);
    overlay.push_quad(material, clip, p.corners).ok()?;
    let item = &mut overlay.items[0];
    item.alpha_test = p.state.alpha_test;
    item.alpha_kill = p.alpha_kill;
    item.shader_stages = p.stages;
    item.stage_uv = Some(Box::new(p.corners.map(|c| [c.uv; 4])));
    item.hud_input = at.hud_input;
    item.blend_constant = p.state.blend_constant;
    item.shader_words = Some(Box::new(p.words));
    item.color_write = p.state.color_write;
    Some(overlay)
}

/// A proven shader's quad, before the guest's setup: whether it was drawn
/// from the overlay (the setup and the draw are then left out). An
/// unproven shader, the recheck's quad, a quad that does not predict or
/// a pass that is not placed is left to the guest.
pub fn draw<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    mem: &mut dyn GuestMemory,
    at: &HudTagQuad,
    vertices: &HudVertices,
) -> bool {
    let placed = {
        let mut s = state();
        let Some(proof) = s.proofs.get_mut(&at.shader) else { return false };
        if proof.failed || proof.matched < PROOF_DRAWS {
            return false;
        }
        proof.seen = proof.seen.wrapping_add(1);
        if proof.seen % RECHECK == 0 {
            return false;
        }
        proof.placed
    };
    let Some((size, clip)) = placed else { return false };
    let Ok(p) = predict(mem, at, vertices) else { return false };
    let Some(overlay) = build(&p, at, size, clip) else { return false };
    crate::hud_overlay::place_hud(device, mem, overlay)
}

/// A bitmap widget laid out in Rust (`render_bridge::native_hud::walk`),
/// all of it or none: each quad's globals written, its material read as
/// [`draw`] reads it, and the quads placed as one pass under the fit of
/// the widget's anchor group. Its shader must be proven; a quad that
/// does not predict or a pass that is not placed leaves the widget to
/// the guest, which writes the same globals again.
pub fn draw_widget<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    mem: &mut dyn GuestMemory,
    widget: &WalkWidget,
) -> bool {
    let placed = {
        let s = state();
        let Some(proof) = s.proofs.get(&widget.shader) else { return false };
        if proof.failed || proof.matched < PROOF_DRAWS {
            return false;
        }
        proof.placed
    };
    let Some((size, clip)) = placed else { return false };
    let mut all = Overlay::new(size);
    for quad in &widget.quads {
        for &(at, word) in &quad.writes {
            if mem.write(at, &word.to_le_bytes()).is_err() {
                return false;
            }
        }
        let Some(hud_input) = render_bridge::native_hud::hud_overlay::inputs(mem) else { return false };
        let at = HudTagQuad { bitmap: widget.bitmap, image: quad.image, shader: widget.shader, hud_input, widget: true };
        let Ok(p) = predict(mem, &at, &quad.vertices) else { return false };
        let Some(one) = build(&p, &at, size, clip) else { return false };
        if all.append(&one).is_err() {
            return false;
        }
    }
    if all.is_empty() {
        return true;
    }
    // The group's fit, as the guest's anchor call sets it for its quads
    // (`render_aspect`), and the enclosing one back after them.
    let group = crate::render_aspect::hud_group_fit(widget.anchor, widget.point);
    let set = |mem: &mut dyn GuestMemory, fit| {
        device.lock().ok().is_some_and(|mut d| {
            d.set_ui_fit(&mut crate::native_lights_glue::DeviceMemory(mem), fit).is_ok() && d.ui_fit() == fit
        })
    };
    let Some(enclosing) = device.lock().ok().map(|d| d.ui_fit()) else { return false };
    if let Some(fit) = group
        && !set(mem, fit)
    {
        set(mem, enclosing);
        return false;
    }
    let drawn = crate::hud_overlay::place_hud(device, mem, all);
    if group.is_some() && !set(mem, enclosing) {
        eprintln!("hud walk: the enclosing HUD fit was not restored after a widget");
    }
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::OverlayBlend;

    fn vertices() -> HudVertices {
        let corner = |x: f32, y: f32, u: f32, v: f32| [(9, [1.0; 4]), (3, [u, v, 0.0, 1.0]), (0, [x, y, 0.0, 1.0])];
        [corner(10.0, 20.0, 0.0, 0.0), corner(42.0, 20.0, 1.0, 0.0), corner(42.0, 36.0, 1.0, 1.0), corner(10.0, 36.0, 0.0, 1.0)]
    }

    fn quad() -> HudTagQuad {
        HudTagQuad { bitmap: 0xe100_0040, image: 2, shader: 0xe200_0007, hud_input: [0.5, 0.0, 0.0, 0.0], widget: true }
    }

    fn predicted() -> Predicted {
        let mut words = [0u32; 60];
        // Stage 0 projects a 2D texture.
        words[54] = 1;
        Predicted {
            words,
            state: HudState {
                blend: OverlayBlend::On { source: 0x302, dest: 0x303, equation: 0x8006 },
                blend_constant: [0.0; 4],
                alpha_test: None,
                color_write: 0x0101_0101,
            },
            alpha_kill: 0,
            sampled: [true, false, false, false],
            stages: [Some(OverlayTexture::Bitmap { tag: 0xe100_0040, image: 2 }), None, None, None],
            sampler: Sampler { wrap: false, point: false },
            corners: corners(&vertices()).unwrap(),
        }
    }

    /// The capture's item for the same draw: built as the capture builds
    /// it, its sampled stage bound by the device's texture.
    fn captured(p: &Predicted) -> Overlay {
        let mut o = build(p, &quad(), [640, 480], ClipRect::full([640, 480])).unwrap();
        o.items[0].shader_stages = [Some(OverlayTexture::Bound { header: 0x1234 }), None, None, None];
        o
    }

    #[test]
    fn the_quads_vertices_are_its_corners() {
        let c = corners(&vertices()).unwrap();
        assert_eq!(c[2].position, [42.0, 36.0]);
        assert_eq!(c[2].uv, [1.0, 1.0]);
        assert_eq!(c[0].color, [1.0; 4]);
        let mut odd = vertices();
        odd[1][0].0 = 4;
        assert!(corners(&odd).is_none(), "a vertex not in the walk's order");
    }

    #[test]
    fn the_texture_modes_name_the_sampled_stages() {
        let mut words = [0u32; 60];
        assert_eq!(sampled_stages(&words), [false; 4]);
        words[54] = 1 | (1 << 10);
        assert_eq!(sampled_stages(&words), [true, false, true, false]);
    }

    /// The capture of the predicted draw matches; each part that differs
    /// is named, the first first.
    #[test]
    fn a_capture_matches_its_prediction_or_names_what_differs() {
        let p = predicted();
        assert_eq!(compare(&p, &captured(&p)), Ok(()));
        let mut other = p.clone();
        other.words[3] ^= 1;
        assert_eq!(compare(&other, &captured(&p)), Err(Mismatch::Words));
        let mut other = p.clone();
        other.state.color_write = 0x0101_0100;
        assert_eq!(compare(&other, &captured(&p)), Err(Mismatch::State));
        let mut other = p.clone();
        other.sampler.point = true;
        assert_eq!(compare(&other, &captured(&p)), Err(Mismatch::Stages));
        let mut c = captured(&p);
        c.items[0].shader_stages[1] = Some(OverlayTexture::Bound { header: 0x99 });
        assert_eq!(compare(&p, &c), Err(Mismatch::Stages), "a stage the words do not sample");
        let mut c = captured(&p);
        c.vertices[1].position[0] += 0.5;
        assert_eq!(compare(&p, &c), Err(Mismatch::Corners));
        let mut c = captured(&p);
        c.items[0].stage_uv.as_mut().unwrap()[3][0] = [0.0, 0.5];
        assert_eq!(compare(&p, &c), Err(Mismatch::Corners), "the sampled stage's coordinates");
    }

    /// The built item is the overlay's shader quad: the tag shader, the
    /// words, the stages and the widget's inputs.
    #[test]
    fn the_built_item_names_the_shader_and_its_stages() {
        let p = predicted();
        let o = build(&p, &quad(), [640, 480], ClipRect::full([640, 480])).unwrap();
        let item = &o.items[0];
        assert_eq!(item.material.program, Program::Shader { tag: 0xe200_0007 });
        assert_eq!(item.material.texture, OverlayTexture::Bitmap { tag: 0xe100_0040, image: 2 });
        assert_eq!(item.shader_words.as_deref(), Some(&p.words[..]));
        assert_eq!(item.hud_input, [0.5, 0.0, 0.0, 0.0]);
        assert_eq!(o.vertices_of(item), &p.corners[..]);
        assert!(build(&p, &quad(), [640, 480], ClipRect { x: 0, y: 0, width: 0, height: 0 }).is_none(), "an empty clip");
    }
}
