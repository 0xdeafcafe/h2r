//! A one-shot comparison of the uniforms the D3D path draws a model with
//! against another renderer's for the same draw (the native world's for a
//! placement: `H2_NATIVE_WORLD_DUMP` in the boot glue). That renderer arms
//! it with its uniforms ([`arm`]); the next draws whose model rows
//! `c[50..52]` hold the same bits are compared and printed, each register
//! the D3D draw's vertex program reads that differs, the inputs it reads
//! that one streams and the other does not, or whose defaults differ, and
//! the pixel uniforms, and each sampled stage's sampler and the D3D
//! draw's texture there. Inert until armed (one relaxed load per draw).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use gfx::translate::SamplerKey;
use shaders::psh::{PixelOptions, PixelShaderDef, PixelUniforms};

use crate::backend::TextureInfo;
use shaders::vsh::{self, ProgramKind, SrcReg, VertexOptions, VertexUniforms};

/// The model rows the structure draw uploads for a placement
/// (`0x0004d830`, hardware numbering): the draws compared are the ones
/// with the expected rows.
pub const MODEL_ROWS: usize = 50;

/// Lines at most for draws whose rows match but whose program is another
/// (the placement's other layers).
const OTHER_PROGRAMS: u32 = 16;

/// What the other renderer draws with.
#[derive(Clone, Debug)]
pub struct Expected {
    /// Names the draw in each line.
    pub label: String,
    /// The vertex program's instruction words (four per instruction).
    pub words: Vec<u32>,
    pub vertex: VertexUniforms,
    pub pixel: PixelUniforms,
    /// How its program's inputs arrive and are interpolated (the
    /// translation's options; `provided_inputs`: the `v` registers it
    /// streams, bit `n` for `v<n>`).
    pub options: VertexOptions,
    /// The pixel shader definition and the options it is translated with.
    pub def: PixelShaderDef,
    pub pixel_options: PixelOptions,
    /// Its pipeline's primitive (topology, winding, cull, fill) and depth
    /// write, compare and bias.
    pub primitive: wgpu::PrimitiveState,
    pub depth: (bool, wgpu::CompareFunction, wgpu::DepthBiasState),
    /// What it binds at each stage, described (`None`: not known).
    pub textures: [Option<String>; 4],
    /// Each stage it samples, with its sampler (`None`: not sampled).
    pub samplers: [Option<SamplerKey>; 4],
    /// Draws with the same program to compare before it disarms.
    pub draws: u32,
}

struct Armed {
    expected: Expected,
    others: u32,
}

static ARMED: AtomicBool = AtomicBool::new(false);
static EXPECTED: Mutex<Option<Armed>> = Mutex::new(None);

/// Compares the next draws with `expected`'s rows (replacing any armed).
pub fn arm(expected: Expected) {
    *EXPECTED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Armed { expected, others: 0 });
    ARMED.store(true, Ordering::Release);
}

/// Whether a comparison waits for its draws.
pub fn armed() -> bool {
    ARMED.load(Ordering::Acquire)
}

/// A D3D draw with vertex program `words`, uniforms `vertex` and `pixel`,
/// the layout's streamed registers, and its samplers and textures:
/// printed when it matches.
pub(crate) fn observe(
    words: &[u32],
    vertex: &VertexUniforms,
    pixel: &PixelUniforms,
    options: &VertexOptions,
    pixel_shader: (&PixelShaderDef, &PixelOptions),
    pipeline: (&wgpu::PrimitiveState, Option<&wgpu::DepthStencilState>),
    samplers: &[SamplerKey; 4],
    textures: &[Option<TextureInfo>; 4],
) {
    if !ARMED.load(Ordering::Relaxed) {
        return;
    }
    let mut guard = EXPECTED.lock().unwrap_or_else(|e| e.into_inner());
    let Some(armed) = guard.as_mut() else { return };
    let e = &armed.expected;
    if !rows_match(vertex, &e.vertex) {
        return;
    }
    if !same_program(words, &e.words) {
        if armed.others < OTHER_PROGRAMS {
            armed.others += 1;
            let depth = pipeline.1.map(|d| (d.depth_write_enabled, d.depth_compare, d.bias));
            eprintln!(
                "uniform-compare {}: a D3D draw with the rows and another program ({} words, {:#018x}): cull {:?} front {:?} depth (write, compare, bias) {:?}, c[0..4] {:?}",
                e.label,
                words.len(),
                crate::shader::Microcode::hash_words(words),
                pipeline.0.cull_mode,
                pipeline.0.front_face,
                depth,
                &vertex.constants[..4]
            );
        }
        return;
    }
    for line in compare(e, words, vertex, pixel, options, pixel_shader, pipeline, samplers, textures) {
        eprintln!("{line}");
    }
    armed.expected.draws = armed.expected.draws.saturating_sub(1);
    if armed.expected.draws == 0 {
        *guard = None;
        ARMED.store(false, Ordering::Release);
    }
}

/// The top-level fields of two `{:?}` renderings of one struct
/// (`Name { field: value, ... }`) whose values differ, as (field, `a`'s,
/// `b`'s); the whole of each when they do not split alike.
pub fn field_differences(a: &str, b: &str) -> Vec<(String, String, String)> {
    if a == b {
        return Vec::new();
    }
    let (fa, fb) = (fields(a), fields(b));
    if fa.is_empty() || fa.len() != fb.len() || fa.iter().zip(&fb).any(|(x, y)| x.0 != y.0) {
        return vec![("(whole)".into(), a.into(), b.into())];
    }
    fa.into_iter()
        .zip(fb)
        .filter(|(x, y)| x.1 != y.1)
        .map(|((name, x), (_, y))| (name, x, y))
        .collect()
}

/// `Name { f: v, g: w }` as `[(f, v), (g, w)]`, split at the commas
/// outside brackets.
fn fields(s: &str) -> Vec<(String, String)> {
    let Some(open) = s.find('{') else { return Vec::new() };
    let Some(close) = s.rfind('}') else { return Vec::new() };
    let body = &s[open + 1..close];
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    let mut push = |part: &str| {
        if let Some((k, v)) = part.split_once(':') {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    };
    for (i, c) in body.char_indices() {
        match c {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth -= 1,
            ',' if depth == 0 => {
                push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    push(&body[start..]);
    out
}

/// Whether `a` and `b` hold the same bits in the model rows.
pub fn rows_match(a: &VertexUniforms, b: &VertexUniforms) -> bool {
    let bits = |u: &VertexUniforms| -> [[u32; 4]; 3] { std::array::from_fn(|k| u.constants[MODEL_ROWS + k].map(f32::to_bits)) };
    bits(a) == bits(b)
}

/// Whether the D3D draw's words are `expected`'s program (the upload may
/// run past the final instruction).
pub fn same_program(words: &[u32], expected: &[u32]) -> bool {
    !expected.is_empty() && words.get(..expected.len()) == Some(expected)
}

/// The lines comparing a D3D draw with `e`: a header, then only what
/// differs (bits; `d3d` first, then the other renderer's).
pub fn compare(
    e: &Expected,
    words: &[u32],
    vertex: &VertexUniforms,
    pixel: &PixelUniforms,
    options: &VertexOptions,
    (def, pixel_options): (&PixelShaderDef, &PixelOptions),
    (primitive, depth): (&wgpu::PrimitiveState, Option<&wgpu::DepthStencilState>),
    samplers: &[SamplerKey; 4],
    textures: &[Option<TextureInfo>; 4],
) -> Vec<String> {
    let streamed = options.provided_inputs;
    let program = vsh::Program::from_words(ProgramKind::Normal, words).ok();
    // Every constant counts as read when the program does not decode or
    // addresses them relative to a0.x.
    let mut read = [program.is_none(); vsh::CONSTANT_COUNT];
    let mut inputs = if program.is_none() { u16::MAX } else { 0 };
    if let Some(p) = &program {
        inputs = p.inputs_read();
        for s in p.instructions.iter().flat_map(|i| i.sources()) {
            match s.reg {
                SrcReg::Const { relative: true, .. } => read = [true; vsh::CONSTANT_COUNT],
                SrcReg::Const { index, .. } => {
                    if let Some(r) = read.get_mut(usize::from(index)) {
                        *r = true;
                    }
                }
                _ => {}
            }
        }
    }
    let label = &e.label;
    let mut out = vec![format!(
        "uniform-compare {label}: a D3D draw with the rows and the program; streams {streamed:#06x} (other {:#06x}), reads v {inputs:#06x}",
        e.options.provided_inputs
    )];
    let same = |a: &[f32; 4], b: &[f32; 4]| a.map(f32::to_bits) == b.map(f32::to_bits);
    let mut unread = 0;
    for (c, (d, o)) in vertex.constants.iter().zip(&e.vertex.constants).enumerate() {
        if same(d, o) {
            continue;
        }
        if read[c] {
            out.push(format!("  c[{c}] d3d {d:?} other {o:?}"));
        } else {
            unread += 1;
        }
    }
    out.push(format!("  {unread} constants the program does not read differ"));
    // How the inputs it reads arrive (the D3D path may bind guest bytes
    // and decode them in the shader) and the interpolation.
    let fetch = |o: &VertexOptions| -> Vec<(usize, vsh::InputFetch)> {
        (0..16usize).filter(|&n| inputs & 1 << n != 0).map(|n| (n, o.input_fetch[n])).collect()
    };
    if fetch(options) != fetch(&e.options) || options.flat_shading != e.options.flat_shading {
        out.push(format!(
            "  input fetch d3d {:?} flat {} other {:?} flat {}",
            fetch(options),
            options.flat_shading,
            fetch(&e.options),
            e.options.flat_shading
        ));
    }
    for n in 0..16 {
        if inputs & 1 << n == 0 {
            continue;
        }
        let (d, o) = (streamed & 1 << n != 0, e.options.provided_inputs & 1 << n != 0);
        let (dv, ov) = (&vertex.defaults[n], &e.vertex.defaults[n]);
        if d != o || (!d && !same(dv, ov)) {
            let says = |s: bool, v: &[f32; 4]| if s { "streamed".to_string() } else { format!("default {v:?}") };
            out.push(format!("  v{n} d3d {} other {}", says(d, dv), says(o, ov)));
        }
    }
    let bits = |v: &[f32]| v.iter().map(|f| f.to_bits()).collect::<Vec<_>>();
    let show = |v: &[f32]| format!("{v:?}");
    let pair = |name: &str, d: &[f32], o: &[f32]| (name.to_string(), bits(d) != bits(o), show(d), show(o));
    let (vu, ou) = (vertex, &e.vertex);
    let (pu, op) = (pixel, &e.pixel);
    let flat = |v: &[[f32; 4]]| v.iter().flatten().copied().collect::<Vec<f32>>();
    let scalars = [
        pair("surface", &vu.surface, &ou.surface),
        pair("z_max", &[vu.z_max], &[ou.z_max]),
        pair("w_clip", &vu.w_clip.unwrap_or([f32::NAN; 2]), &ou.w_clip.unwrap_or([f32::NAN; 2])),
        pair("w_clamp", &[f32::from(u8::from(vu.w_clamp))], &[f32::from(u8::from(ou.w_clamp))]),
        pair("shift", &[vu.shift], &[ou.shift]),
        pair("c0", &flat(&pu.c0), &flat(&op.c0)),
        pair("c1", &flat(&pu.c1), &flat(&op.c1)),
        pair("fog_color", &pu.fog_color, &op.fog_color),
        pair("fog", &pu.fog, &op.fog),
        pair("bump_mat", &flat(&pu.bump_mat), &flat(&op.bump_mat)),
        pair("bump_lum", &flat(&pu.bump_lum), &flat(&op.bump_lum)),
        pair("tex_scale", &flat(&pu.tex_scale), &flat(&op.tex_scale)),
        pair("eye", &pu.eye, &op.eye),
        pair("alpha_ref", &pu.alpha_ref, &op.alpha_ref),
        pair("border", &flat(&pu.border), &flat(&op.border)),
        pair("lod_bias", &pu.lod_bias, &op.lod_bias),
        pair("depth", &pu.depth, &op.depth),
        pair("depth_range", &pu.depth_range, &op.depth_range),
    ];
    for (name, differs, d, o) in scalars {
        if differs {
            out.push(format!("  {name} d3d {d} other {o}"));
        }
    }
    // The combiners and what changes their code: the fields that differ.
    for (field, d, o) in field_differences(&format!("{def:?}"), &format!("{:?}", e.def)) {
        out.push(format!("  pixel shader {field} d3d {d} other {o}"));
    }
    for (field, d, o) in field_differences(&format!("{pixel_options:?}"), &format!("{:?}", e.pixel_options)) {
        out.push(format!("  pixel options {field} d3d {d} other {o}"));
    }
    // The pipeline: the D3D draw's topology is the guest's (a strip where
    // the other renderer draws the strip's triangles as a list).
    for (field, d, o) in field_differences(&format!("{primitive:?}"), &format!("{:?}", e.primitive)) {
        out.push(format!("  primitive {field} d3d {d} other {o}"));
    }
    let d3d_depth = depth.map(|d| (d.depth_write_enabled.unwrap_or(false), d.depth_compare.unwrap_or(wgpu::CompareFunction::Always), d.bias));
    if d3d_depth != Some(e.depth) {
        out.push(format!("  depth (write, compare, bias) d3d {d3d_depth:?} other {:?}", e.depth));
    }
    // The polygon stipple the D3D draw masks its fragments with (only
    // with `PixelOptions::stipple`; all ones masks nothing).
    if pixel_options.stipple && pixel.stipple != [u32::MAX; 32] {
        let rows: Vec<String> = pixel.stipple.iter().map(|r| format!("{r:08x}")).collect();
        out.push(format!("  stipple d3d {}", rows.join(" ")));
    }
    // The stages the other renderer samples: the D3D draw's texture
    // there, and the samplers when they differ.
    for (n, other) in e.samplers.iter().enumerate() {
        let Some(other) = other else { continue };
        let t = textures[n].map_or_else(
            || "none".to_string(),
            |t| format!("{:?} {}x{} levels {}", t.format, t.width, t.height, t.levels),
        );
        let o = e.textures[n].as_deref().unwrap_or("not known");
        let d = &samplers[n];
        if d == other {
            out.push(format!("  stage {n} d3d texture {t}, other {o}, the same sampler"));
        } else {
            out.push(format!("  stage {n} d3d texture {t}, other {o}, sampler d3d {d:?} other {other:?}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaders::vsh::encode::{Inst, Operand as O, program};
    use shaders::vsh::{MacOp, OutReg};

    /// `dph r0.x, v0, c[50]`, `dph oPos, r0, c[0]`, `mov oT0, v5`.
    fn words() -> Vec<u32> {
        let code = program(&[
            Inst::mac(MacOp::Dph, Some(O::v(0)), Some(O::c(50)), None).to_temp(0, 8),
            Inst::mac(MacOp::Dph, Some(O::r(0)), Some(O::c(0)), None).to_out(false, OutReg::Pos, 0xf),
            Inst::mac(MacOp::Mov, Some(O::v(5)), None, None).to_out(false, OutReg::T(0), 0xf),
        ]);
        code[4..].chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
    }

    fn expected() -> Expected {
        let mut vertex = VertexUniforms::default();
        vertex.constants[MODEL_ROWS] = [1.0, 0.0, 0.0, 5.0];
        let mut samplers = [None; 4];
        samplers[1] = Some(sampler(wgpu::FilterMode::Linear));
        Expected {
            label: "placement 7".into(),
            words: words(),
            vertex,
            pixel: PixelUniforms::default(),
            options: VertexOptions { provided_inputs: 0x7f, ..Default::default() },
            def: PixelShaderDef::default(),
            pixel_options: PixelOptions::default(),
            primitive: wgpu::PrimitiveState::default(),
            depth: (true, wgpu::CompareFunction::LessEqual, wgpu::DepthBiasState::default()),
            textures: [None, Some("Rgba8Unorm 4x4".into()), None, None],
            samplers,
            draws: 1,
        }
    }

    fn sampler(min: wgpu::FilterMode) -> SamplerKey {
        let mut t = gfx::state::TextureStageState::default_for_stage(0);
        t.min_filter = if min == wgpu::FilterMode::Linear { gfx::state::TextureFilter::Linear } else { gfx::state::TextureFilter::Point };
        SamplerKey::from_stage(&t, gfx::translate::Capabilities::ALL)
    }

    #[test]
    fn draws_are_matched_by_their_rows_bits_and_program() {
        let e = expected();
        let mut v = e.vertex.clone();
        assert!(rows_match(&v, &e.vertex));
        v.constants[MODEL_ROWS + 2][3] = -0.0;
        assert!(!rows_match(&v, &e.vertex), "negative zero is other bits");
        let mut longer = words();
        longer.extend([0; 4]);
        assert!(same_program(&longer, &e.words), "an upload past the final instruction");
        assert!(!same_program(&longer[4..], &e.words));
        assert!(!same_program(&longer, &[]));
    }

    #[test]
    fn only_what_differs_and_is_read_is_printed() {
        let e = expected();
        let mut v = e.vertex.clone();
        let mut p = e.pixel;
        let mut samplers = [sampler(wgpu::FilterMode::Linear); 4];
        let textures = [None; 4];
        let mut options = e.options;
        let mut primitive = e.primitive;
        let mut depth = wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth24PlusStencil8,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };
        let mut def = e.def.clone();
        assert_eq!(
            compare(&e, &e.words, &v, &p, &options, (&def, &e.pixel_options), (&primitive, Some(&depth)), &samplers, &textures)[1..],
            ["  0 constants the program does not read differ", "  stage 1 d3d texture none, other Rgba8Unorm 4x4, the same sampler"]
        );
        v.constants[0] = [2.0; 4];
        v.constants[60] = [3.0; 4];
        v.defaults[5] = [0.5, 0.5, 0.5, 1.0];
        p.c0[1] = [0.25; 4];
        v.w_clip = Some([1.0, 2.0]);
        samplers[1] = sampler(wgpu::FilterMode::Nearest);
        options.provided_inputs = 0x1f;
        def.compare_mode = 1;
        primitive.topology = wgpu::PrimitiveTopology::TriangleStrip;
        depth.bias.constant = 2;
        let lines = compare(&e, &e.words, &v, &p, &options, (&def, &e.pixel_options), (&primitive, Some(&depth)), &samplers, &textures);
        assert_eq!(lines[1], "  c[0] d3d [2.0, 2.0, 2.0, 2.0] other [0.0, 0.0, 0.0, 0.0]");
        assert_eq!(lines[2], "  1 constants the program does not read differ", "c[60]");
        assert_eq!(lines[3], "  v5 d3d default [0.5, 0.5, 0.5, 1.0] other streamed");
        assert!(lines[4].starts_with("  w_clip d3d [1.0, 2.0] other [NaN, NaN]"), "{}", lines[4]);
        assert!(lines[5].starts_with("  c0 d3d"), "{}", lines[5]);
        assert_eq!(lines[6], "  pixel shader compare_mode d3d 1 other 0");
        assert_eq!(lines[7], "  primitive topology d3d TriangleStrip other TriangleList");
        assert!(lines[8].starts_with("  depth (write, compare, bias) d3d Some((true, LessEqual, DepthBiasState { constant: 2"), "{}", lines[8]);
        assert!(lines[9].starts_with("  stage 1 d3d texture none, other Rgba8Unorm 4x4, sampler d3d SamplerKey"), "{}", lines[9]);
        assert!(lines[9].contains("min: Nearest") && lines[9].contains("min: Linear"), "{}", lines[9]);
        assert_eq!(lines.len(), 10);
    }

    #[test]
    fn only_the_fields_that_differ_are_listed() {
        #[derive(Debug)]
        #[allow(dead_code)]
        struct S {
            a: [u32; 2],
            b: Option<(u8, u8)>,
            c: bool,
        }
        let x = format!("{:?}", S { a: [1, 2], b: Some((3, 4)), c: true });
        let y = format!("{:?}", S { a: [1, 2], b: Some((3, 5)), c: false });
        assert_eq!(
            field_differences(&x, &y),
            vec![
                ("b".to_string(), "Some((3, 4))".to_string(), "Some((3, 5))".to_string()),
                ("c".to_string(), "true".to_string(), "false".to_string())
            ]
        );
        assert!(field_differences(&x, &x).is_empty());
        assert_eq!(field_differences("A", "B"), vec![("(whole)".to_string(), "A".to_string(), "B".to_string())]);
    }
}
