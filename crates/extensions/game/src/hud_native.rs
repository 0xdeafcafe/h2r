//! The HUD's widget quads drawn from their shader's Rust material, with no
//! guest shader setup (docs/graphics/retire-d3d.md, slice R6;
//! docs/graphics/native-hud-overlay.md, "The material in Rust").
//!
//! A widget quad is predicted from the map: its shader's one pass
//! implementation (`native_render::hud_material::HudShader`), the combiner
//! words with the draw's animated constants through the HUD's function
//! inputs (`render_bridge::native_hud::hud_inputs`) at the render clock,
//! its blend and write state, its stages, and its corners from the quad's
//! own vertices. Every shader's quads are drawn this way, the HUD's and
//! the menus' (a menu shader bitmap's quad is predicted as a HUD one is),
//! on the UI's 640 by 480 guest frame, with no guest shader setup and no
//! proof against a guest draw. A quad that does not predict, or a pass
//! that is not placed, panics by name ([`refuse`]). The guest's setup
//! and draw are left out at `HUD_QUAD_SETUP`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex};

use guest_layout::memory::GuestMemory;
use frame_scene::{ClipRect, Overlay, OverlayGlyph, OverlayMaterial, OverlayTexture, OverlayVertex, Program, Sampler};
use map::TagId;
use native_render::hud_material::{HudRefusal, HudShader, HudState, StageTexture};
use render_bridge::native_hud::strings::StringRun;
use render_bridge::native_hud::walk::GlyphQuad;
use render_bridge::native_hud::{HudTagQuad, HudVertices, WalkText, WalkWidget, hud_inputs};
use tags::material::ShaderLibrary;
use timeline::native::Ui;

/// The UI's guest frame: every guest UI coordinate is 640 x 480
/// (`render_aspect`), and its quads are clipped to the whole of it.
const FRAME: [u32; 2] = [640, 480];
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

impl Unpredicted {
    /// Its name in `native_ui_overlay_hud_rust_refused_<cause>`; `None`
    /// while the map loads, which is no refusal.
    pub fn cause(&self) -> Option<&'static str> {
        match self {
            Unpredicted::Loading => None,
            Unpredicted::Refused(r) => Some(r.name()),
            Unpredicted::Inputs => Some("inputs"),
        }
    }
}

static REFUSALS: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

/// A captured quad whose prediction is refused: counted, all together
/// and by its cause.
fn refused(why: &Unpredicted) {
    let Some(cause) = why.cause() else { return };
    Ui::OverlayHudRustRefused.count();
    *REFUSALS.lock().unwrap_or_else(|e| e.into_inner()).entry(cause).or_default() += 1;
}

/// `native_ui_overlay_hud_rust_refused_<cause>` for every refusal counted.
pub fn refusal_counters() -> Vec<(String, u64)> {
    let counts = REFUSALS.lock().unwrap_or_else(|e| e.into_inner());
    counts.iter().map(|(cause, &n)| (format!("native_ui_overlay_hud_rust_refused_{cause}"), n)).collect()
}

#[derive(Default)]
struct State {
    generation: Option<u64>,
    library: Option<ShaderLibrary>,
    shaders: HashMap<u32, Result<Arc<HudShader>, HudRefusal>>,
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
        let read = crate::hud_overlay::with_art_map(mem, ShaderLibrary::read).ok_or(Unpredicted::Loading)?;
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

/// A named refusal: a quad whose shader the Rust material cannot draw, or
/// a pass that is not placed. The guest's draw has no device behind it.
fn refuse(what: &str, why: &dyn std::fmt::Debug) -> ! {
    unimplemented!("hud: {what}: {why:?}")
}

/// A shader quad, before the guest's setup: drawn from the overlay (the
/// setup and the draw are then left out). The quad is the HUD's
/// ([`HudTagQuad::widget`]) or a menu shader bitmap's.
pub fn draw(
    mem: &mut dyn GuestMemory,
    at: &HudTagQuad,
    vertices: &HudVertices,
) -> bool {
    let p = match predict(mem, at, vertices) {
        Ok(p) => p,
        Err(why) => {
            refused(&why);
            refuse(&format!("quad of shader {:#x} does not predict", at.shader), &why)
        }
    };
    let Some(overlay) = build(&p, at, FRAME, ClipRect::full(FRAME)) else {
        refuse(&format!("quad of shader {:#x} does not build", at.shader), &"overlay")
    };
    if at.widget {
        if !crate::hud_overlay::place_hud(mem, overlay) {
            refuse(&format!("quad of shader {:#x} is not placed", at.shader), &"hud");
        }
    } else if !crate::hud_overlay::place_ui(mem, overlay) {
        refuse(&format!("menu quad of shader {:#x} is not placed", at.shader), &"ui");
    }
    true
}

/// A text shader's material for a glyph draw: what [`Predicted`] holds
/// but for the corners and the stages' textures, which are the glyph
/// pages'.
#[derive(Clone, Debug, PartialEq)]
pub struct TextMaterial {
    pub words: [u32; 60],
    pub state: HudState,
    pub alpha_kill: u8,
    pub sampled: [bool; 4],
    pub sampler: Sampler,
}

/// The material of text shader `datum` from the map. The glyphs take
/// stage 0 alone, the text stage's place (`TEXT_STAGE_SOURCE`) or the
/// widget bitmap's: a text run carries one set of coordinates.
pub fn predict_text(mem: &dyn GuestMemory, datum: u32) -> Result<TextMaterial, Unpredicted> {
    let hud = shader(mem, datum)?;
    if !hud_inputs::provider_installed(mem) {
        return Err(Unpredicted::Inputs);
    }
    let inputs = |name: u32| hud_inputs::function_input(mem, name);
    let words = hud.words(render_time(mem), &inputs).map_err(Unpredicted::Refused)?;
    let sampled = sampled_stages(&words);
    let glyphs = |stage: StageTexture| matches!(stage, StageTexture::Glyphs | StageTexture::Widget);
    if let Some(stage) = (0..4).find(|&n| sampled[n] && (n != 0 || !glyphs(hud.stages[n]))) {
        return Err(Unpredicted::Refused(HudRefusal::Texture { stage }));
    }
    if !sampled[0] {
        return Err(Unpredicted::Refused(HudRefusal::Texture { stage: 0 }));
    }
    Ok(TextMaterial { words, state: hud.state, alpha_kill: hud.alpha_kill, sampled, sampler: hud.samplers[0] })
}

/// Whether `captured` (the capture's one-glyph overlay, over the guest's
/// glyph cache) draws with `m`: its words, its state and its stages.
pub fn compare_text(m: &TextMaterial, captured: &Overlay) -> Result<(), Mismatch> {
    let item = captured.items.first().ok_or(Mismatch::Corners)?;
    if item.shader_words.as_deref() != Some(&m.words[..]) {
        return Err(Mismatch::Words);
    }
    if item.material.blend != m.state.blend
        || item.blend_constant != m.state.blend_constant
        || item.alpha_test != m.state.alpha_test
        || item.color_write != m.state.color_write
        || item.alpha_kill != m.alpha_kill
    {
        return Err(Mismatch::State);
    }
    if (0..4).any(|n| item.shader_stages[n].is_some() != m.sampled[n]) || item.material.sampler != m.sampler {
        return Err(Mismatch::Stages);
    }
    Ok(())
}

/// A string's runs: its glyph quads in order, each run the quads next
/// to each other in one colour (red, green, blue, alpha), placed at
/// `origin` (the window's corner in the frame).
fn runs(quads: &[GlyphQuad], origin: [f32; 2]) -> Vec<([f32; 4], Vec<OverlayGlyph>)> {
    let mut out: Vec<(u32, Vec<OverlayGlyph>)> = Vec::new();
    for quad in quads {
        // The quads are the window's; the overlay is the frame's.
        let [x0, y0, x1, y1] = quad.rect;
        let [x, y] = origin;
        let glyph = OverlayGlyph { index: quad.index, rect: [x0 + x, y0 + y, x1 + x, y1 + y] };
        match out.last_mut() {
            Some((colour, glyphs)) if *colour == quad.colour => glyphs.push(glyph),
            _ => out.push((quad.colour, vec![glyph])),
        }
    }
    out.into_iter()
        .map(|(colour, glyphs)| {
            let [b, g, r, a] = colour.to_le_bytes();
            ([r, g, b, a].map(|v| f32::from(v) / 255.0), glyphs)
        })
        .collect()
}

/// The text as overlay runs over the glyph pages of its font, drawn by
/// its shader's words with each run's colour as the diffuse.
pub fn build_text(m: &TextMaterial, text: &WalkText, size: [u32; 2], clip: ClipRect) -> Option<Overlay> {
    let pages = OverlayTexture::Glyphs { font: text.font };
    let material = OverlayMaterial { program: Program::Shader { tag: text.shader }, texture: pages, sampler: m.sampler, blend: m.state.blend };
    let mut overlay = Overlay::new(size);
    for (colour, glyphs) in runs(&text.quads, text.origin) {
        let index = overlay.push_text_run(material, clip, colour, &glyphs).ok()?;
        let item = &mut overlay.items[index as usize];
        item.alpha_test = m.state.alpha_test;
        item.alpha_kill = m.alpha_kill;
        item.shader_stages = [Some(pages), None, None, None];
        item.blend_constant = m.state.blend_constant;
        item.shader_words = Some(Box::new(m.words));
        item.color_write = m.state.color_write;
    }
    Some(overlay)
}


/// A text widget laid out in Rust (`render_bridge::native_hud::walk`),
/// all of it or none: its globals written, its shader's material read,
/// and its glyph quads placed as one pass over the host's glyph pages
/// under the fit of the widget's anchor group. Its shader must be proven
/// on its glyph quads ([`check_text`]).
pub fn draw_text(
    mem: &mut dyn GuestMemory,
    text: &WalkText,
) -> bool {
    for &(at, word) in &text.writes {
        if mem.write(at, &word.to_le_bytes()).is_err() {
            refuse("a text widget's global did not take its word", &text.shader);
        }
    }
    let m = predict_text(mem, text.shader).unwrap_or_else(|why| refuse(&format!("text shader {:#x} does not predict", text.shader), &why));
    let Some(overlay) = build_text(&m, text, FRAME, ClipRect::full(FRAME)) else {
        refuse(&format!("text of shader {:#x} does not build", text.shader), &"overlay")
    };
    let group = crate::render_aspect::hud_group_fit(text.anchor, text.point);
    let enclosing = crate::hud_overlay::ui_fit();
    if let Some(fit) = group {
        crate::hud_overlay::set_ui_fit(fit);
    }
    let drawn = crate::hud_overlay::place_text(mem, overlay, frame_scene::ViewSlot::Hud);
    if group.is_some() {
        crate::hud_overlay::set_ui_fit(enclosing);
    }
    if !drawn {
        refuse(&format!("text of shader {:#x} is not placed", text.shader), &"hud");
    }
    true
}

/// A string of the string draw (`render_bridge::native_hud::strings`)
/// as overlay runs over the glyph pages of its font: the batch's glyph
/// material (the glyphs' coverage times each run's colour) with its
/// blend, each run's colour the quads' packed colour.
pub fn build_string(run: &StringRun, size: [u32; 2], clip: ClipRect) -> Option<Overlay> {
    let material = OverlayMaterial { blend: run.blend, ..OverlayMaterial::text(run.font) };
    let mut overlay = Overlay::new(size);
    for (colour, glyphs) in runs(&run.quads, run.origin) {
        overlay.push_text_run(material, clip, colour, &glyphs).ok()?;
    }
    Some(overlay)
}

/// A string laid out in Rust: its runs placed as one pass over the
/// host's glyph pages ([`build_string`]) on the UI's guest frame (every
/// guest UI coordinate is 640 x 480, `render_aspect`), clipped to the
/// whole frame; the run's origin places split-screen windows.
pub fn draw_string(
    mem: &mut dyn GuestMemory,
    run: &StringRun,
) -> bool {
    const FRAME: [u32; 2] = [640, 480];
    let Some(overlay) = build_string(run, FRAME, ClipRect::full(FRAME)) else {
        unimplemented!("strings: font {}'s runs do not fit one overlay", run.font)
    };
    if let Err(why) = crate::hud_overlay::place_string(mem, overlay, frame_scene::ViewSlot::Hud) {
        unimplemented!("strings: font {}: {why}", run.font)
    }
    true
}

/// A bitmap widget laid out in Rust (`render_bridge::native_hud::walk`),
/// all of it or none: each quad's globals written, its material read as
/// [`draw`] reads it, and the quads placed as one pass under the fit of
/// the widget's anchor group. Its shader must be proven; a quad that
/// does not predict or a pass that is not placed leaves the widget to
/// the guest, which writes the same globals again.
pub fn draw_widget(
    mem: &mut dyn GuestMemory,
    widget: &WalkWidget,
) -> bool {
    let (size, clip) = (FRAME, ClipRect::full(FRAME));
    let mut all = Overlay::new(size);
    for quad in &widget.quads {
        for &(at, word) in &quad.writes {
            if mem.write(at, &word.to_le_bytes()).is_err() {
                refuse("a widget's global did not take its word", &widget.shader);
            }
        }
        let Some(hud_input) = render_bridge::native_hud::hud_overlay::inputs(mem) else {
            refuse("a widget's shader inputs do not read", &widget.shader)
        };
        let at = HudTagQuad { bitmap: widget.bitmap, image: quad.image, shader: widget.shader, hud_input, widget: true };
        let p = predict(mem, &at, &quad.vertices)
            .unwrap_or_else(|why| refuse(&format!("widget shader {:#x} does not predict", widget.shader), &why));
        let Some(one) = build(&p, &at, size, clip) else { refuse("a widget quad does not build", &widget.shader) };
        if all.append(&one).is_err() {
            refuse("a widget's quads do not join one pass", &widget.shader);
        }
    }
    if all.is_empty() {
        return true;
    }
    // The group's fit, as the guest's anchor call sets it for its quads
    // (`render_aspect`), and the enclosing one back after them.
    let group = crate::render_aspect::hud_group_fit(widget.anchor, widget.point);
    let enclosing = crate::hud_overlay::ui_fit();
    if let Some(fit) = group {
        crate::hud_overlay::set_ui_fit(fit);
    }
    let drawn = crate::hud_overlay::place_hud(mem, all);
    if group.is_some() {
        crate::hud_overlay::set_ui_fit(enclosing);
    }
    if !drawn {
        refuse("a widget's pass is not placed", &widget.shader);
    }
    true
}

#[cfg(test)]
mod refusal_tests {
    use super::*;

    #[test]
    #[should_panic(expected = "hud: quad of shader 0xe2000009 does not predict")]
    fn a_quad_that_does_not_predict_is_named_not_left_to_the_guest() {
        refuse("quad of shader 0xe2000009 does not predict", &Unpredicted::Inputs);
    }

    #[test]
    fn the_frame_is_the_guests_640_by_480_whole() {
        assert_eq!(FRAME, [640, 480]);
        assert_eq!(ClipRect::full(FRAME), ClipRect { x: 0, y: 0, width: 640, height: 480 });
    }
}

#[cfg(test)]
mod text_tests {
    use super::*;
    use frame_scene::OverlayKind;
    use render_bridge::native_hud::walk::GlyphQuad;

    fn text() -> WalkText {
        let quad = |index: u32, x: f32, colour: u32| GlyphQuad { index, rect: [x, 20.0, x + 8.0, 30.0], colour };
        WalkText {
            anchor: 3,
            point: [592.0, 444.0],
            shader: 0xe200_0009,
            font: 8,
            quads: vec![quad(1, 11.0, 0x8000_0000), quad(1, 10.0, 0xffff_8040), quad(2, 20.0, 0xffff_8040), quad(3, 30.0, 0xffff_8040)],
            origin: [100.0, 0.0],
            writes: Vec::new(),
        }
    }

    fn material() -> TextMaterial {
        let mut words = [0u32; 60];
        words[7] = 0x1234_5678;
        let state = HudState {
            blend: frame_scene::OverlayBlend::On { source: 0x302, dest: 0x303, equation: 0x8006 },
            blend_constant: [0.0; 4],
            alpha_test: None,
            color_write: 0x0101_0101,
        };
        TextMaterial { words, state, alpha_kill: 1, sampled: [true, false, false, false], sampler: Sampler::default() }
    }

    #[test]
    fn a_strings_quads_are_runs_in_the_guests_order_one_per_change_of_colour() {
        let t = text();
        let runs = runs(&t.quads, t.origin);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].0, [0.0, 0.0, 0.0, 128.0 / 255.0]);
        assert_eq!(runs[0].1, vec![OverlayGlyph { index: 1, rect: [111.0, 20.0, 119.0, 30.0] }]);
        assert_eq!(runs[1].0, [1.0, 128.0 / 255.0, 64.0 / 255.0, 1.0]);
        assert_eq!(runs[1].1.iter().map(|g| g.index).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn a_text_run_draws_by_its_shaders_words_over_its_fonts_glyph_pages() {
        let (t, m) = (text(), material());
        let clip = ClipRect { x: 0, y: 0, width: 640, height: 480 };
        let overlay = build_text(&m, &t, [640, 480], clip).unwrap();
        assert_eq!(overlay.items.len(), 2);
        for item in &overlay.items {
            assert_eq!(item.kind, OverlayKind::TextRun);
            assert_eq!(item.material.program, Program::Shader { tag: t.shader });
            assert_eq!(item.material.texture, OverlayTexture::Glyphs { font: 8 });
            assert_eq!(item.shader_stages, [Some(OverlayTexture::Glyphs { font: 8 }), None, None, None]);
            assert_eq!(item.shader_words.as_deref(), Some(&m.words[..]));
            assert_eq!((item.alpha_kill, item.glyph_cache), (1, 0));
        }
        assert_eq!(overlay.glyphs_of(&overlay.items[1]).len(), 3);
    }

    /// A plain string's runs are its glyph quads where the guest's draws
    /// put them: each glyph's place in the frame, its glyph in its font's
    /// pages (the texels the guest's quad samples) and its colour, in the
    /// guest's order, shadow then glyph.
    #[test]
    fn a_plain_strings_runs_are_its_glyph_quads_place_glyph_and_colour() {
        let t = text();
        let run = StringRun { font: 8, quads: t.quads.clone(), origin: t.origin, blend: frame_scene::OverlayBlend::ALPHA };
        let clip = ClipRect { x: 0, y: 0, width: 640, height: 480 };
        let overlay = build_string(&run, [640, 480], clip).unwrap();
        let mut drawn = Vec::new();
        for item in &overlay.items {
            assert_eq!(item.kind, OverlayKind::TextRun);
            assert_eq!(item.material, OverlayMaterial::text(8));
            let [r, g, b, a] = item.color.map(|v| (v * 255.0).round() as u32);
            for glyph in overlay.glyphs_of(item) {
                drawn.push((glyph.index, glyph.rect, a << 24 | r << 16 | g << 8 | b));
            }
        }
        let expected: Vec<_> = t.quads.iter().map(|q| {
            let [x0, y0, x1, y1] = q.rect;
            (q.index, [x0 + 100.0, y0, x1 + 100.0, y1], q.colour)
        }).collect();
        assert_eq!(drawn, expected);
        let blended = StringRun { blend: frame_scene::OverlayBlend::Off, ..run };
        assert_eq!(build_string(&blended, [640, 480], clip).unwrap().items[0].material.blend, frame_scene::OverlayBlend::Off);
    }

    /// A refused prediction is counted by its cause; the map loading is
    /// no refusal.
    #[test]
    fn a_refused_prediction_is_counted_by_its_cause() {
        let count = |name: &str| refusal_counters().into_iter().find(|(k, _)| k == name).map_or(0, |(_, n)| n);
        let before = count("native_ui_overlay_hud_rust_refused_texture");
        refused(&Unpredicted::Refused(HudRefusal::Texture { stage: 0 }));
        assert_eq!(count("native_ui_overlay_hud_rust_refused_texture"), before + 1);
        assert_eq!(Unpredicted::Inputs.cause(), Some("inputs"));
        assert_eq!(Unpredicted::Loading.cause(), None);
    }

    #[test]
    fn a_glyph_capture_with_other_words_or_stages_is_not_the_materials() {
        let (t, m) = (text(), material());
        let clip = ClipRect { x: 0, y: 0, width: 640, height: 480 };
        let captured = build_text(&m, &t, [640, 480], clip).unwrap();
        assert_eq!(compare_text(&m, &captured), Ok(()));
        let mut other = m.clone();
        other.words[7] = 0;
        assert_eq!(compare_text(&other, &captured), Err(Mismatch::Words));
        let mut other = m.clone();
        other.alpha_kill = 0;
        assert_eq!(compare_text(&other, &captured), Err(Mismatch::State));
        let mut other = m.clone();
        other.sampled[1] = true;
        assert_eq!(compare_text(&other, &captured), Err(Mismatch::Stages));
    }
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
