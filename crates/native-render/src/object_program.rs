//! An object's layer-3 pass with its own programs, as the world's
//! [`crate::program`] path draws a world pass: the pass's vertex program
//! and pixel shader translated once per material, and per drawn object a
//! vertex constant block holding what the game's upload would hold
//! (docs/graphics/native-renderer.md, "Object programs"):
//!
//! - `c[0..18]`, the view's, from the device each frame
//!   ([`crate::program::frame_vertex`]);
//! - `c[18 + i]`, the implementation's constants: the instance's, with the
//!   engine-driven ones that read the object's lighting put in per object
//!   from [`crate::object_lighting`] ([`object_vertex_uniforms`]);
//! - `c[50..]`, the object's node rows as the draw uploads them for the
//!   render record ([`node_rows`]);
//! - `c[170..173]`, the model's decompression ([`decompression_rows`]).
//!
//! Research points (Code, `h2 disasm`):
//!
//! - `0x0004d830`: a render record with a matrix at `+0x1c` uploads it
//!   transposed (the node matrix's forward, left, up and position columns
//!   as rows; its scale at `+0` not read) as three registers at hardware
//!   `c[50]` (`SetVertexShaderConstantNotInlineFast` `0x003f7330`, `ECX`
//!   0x32, 12 dwords).
//! - Otherwise `0x0004d8e0`: the record's `+8` is a node block handle
//!   (-1: nothing is uploaded, the registers keep the previous draw's);
//!   the block is at `0x00487b1c + (handle & 0x0fffffff)`: `+0` the
//!   matrix count (u16), `+4 + 4·part` per part (`+0xc & 0xf`) its first
//!   matrix (i16) and matrix count (u16), `+0x44 + 0x30·n` matrix `n`,
//!   twelve floats already in register order.
//!   - A model record (`+0xc` bits 29..31 = 1) whose section (bits 9..17)
//!     names a node (`0x0004cb80`: the model tag's sections, stride 0x5c,
//!     the i16 at `+0x2c`) uploads one matrix: the part's first when the
//!     part has one, else that node's.
//!   - Anything else (`0x000152a0`): the part's matrices, `count · 12`
//!     dwords from its first, at `c[50 + 3·k]`: the skinned palette the
//!     program indexes with `a0.x`.
//! - `0x0001cdd0` (from the section setup `0x0004d7c0`): twelve dwords at
//!   hardware `c[170]` (`ECX` 0xaa): the position scale and offset (half
//!   the bounds' extent, w 1; their centre, w 0) and the texture
//!   coordinate's (`(su, sv, ou, ov)`), each the identity when the
//!   section's flag for it is clear.

use shaders::vsh::{self, VertexUniforms};
use tags::material::{Origin, ResolvedImplementation, VectorTarget};

use crate::camera::GuestRead;
use crate::look::{PassLook, Refusal, implementation_look};
use crate::object_lighting::{
    Defaults, LIGHTING_SCALARS, LIGHTING_VECTORS, Lighting, ShaderValues, scalar_input, vector_input,
};
use crate::program::{
    INPUT_MASK, MODEL_ROWS, MaterialProgram, PassParts, TEXCOORD_DECOMPRESSION, VIEW_CONSTANTS, translate_pass,
};

/// The node blocks a render record's `+8` handle points into.
pub const NODE_BLOCKS: u32 = 0x0048_7b1c;
/// A render record's node block handle.
pub const RECORD_NODES: u32 = 0x8;
/// A render record's key: bits 0..3 the part, 9..17 the section, 29..31
/// the type.
pub const RECORD_KEY: u32 = 0xc;
/// A render record's single matrix (a node matrix, `0x34` bytes).
pub const RECORD_MATRIX: u32 = 0x1c;
/// A render record's model datum.
pub const RECORD_MODEL: u32 = 0x4;
/// A node block's per-part (first, count), four bytes each.
pub const BLOCK_PARTS: u32 = 0x4;
/// A node block's matrices.
pub const BLOCK_MATRICES: u32 = 0x44;
/// Bytes per matrix in a node block.
pub const MATRIX_STRIDE: u32 = 0x30;
/// The record type of a model's geometry.
pub const MODEL_RECORD: u32 = 0x2000_0000;
/// The tag instance table (16 bytes an entry, `+8` the tag's data).
pub const TAG_INSTANCES: u32 = 0x004e_3b44;
/// A `mode` tag's sections (pointer at `+0x28`), and a section's node.
pub const MODEL_SECTIONS: u32 = 0x28;
pub const SECTION_STRIDE: u32 = 0x5c;
pub const SECTION_NODE: u32 = 0x2c;
/// The position decompression the draw uploads (`c[170]` scale,
/// `c[171]` offset); `c[172]` is the texture coordinate's
/// ([`TEXCOORD_DECOMPRESSION`]).
pub const POSITION_DECOMPRESSION: usize = 170;
/// The most matrices a palette can hold below the decompression rows.
pub const MAX_PALETTE: usize = (POSITION_DECOMPRESSION - MODEL_ROWS) / 3;

/// One vertex constant register.
pub type Row = [f32; 4];

fn word(mem: &dyn GuestRead, at: u32) -> Result<u32, String> {
    mem.read_u32(at).ok_or_else(|| format!("{at:#010x} does not read"))
}

fn float(mem: &dyn GuestRead, at: u32) -> Result<f32, String> {
    word(mem, at).map(f32::from_bits)
}

/// `0x0004cb80`: the node of section `section` of the model `datum`, or
/// -1 (the datum is -1, or its tag has no data).
pub fn section_node(mem: &dyn GuestRead, datum: u32, section: u32) -> Result<i16, String> {
    if datum == u32::MAX {
        return Ok(-1);
    }
    let table = word(mem, TAG_INSTANCES)?;
    let tag = word(mem, table.wrapping_add(16 * (datum & 0xffff)).wrapping_add(8))?;
    if tag == 0 {
        return Ok(-1);
    }
    let sections = word(mem, tag.wrapping_add(MODEL_SECTIONS))?;
    let at = sections.wrapping_add(SECTION_STRIDE.wrapping_mul(section)).wrapping_add(SECTION_NODE);
    Ok(word(mem, at & !3)?.wrapping_shr(8 * (at & 3)) as u16 as i16)
}

/// The node rows the draw uploads at `c[50..]` for the render record at
/// `record` (`0x0004d830`, `0x0004d8e0`): three per matrix. An error when
/// the draw uploads nothing (the registers would hold the previous
/// draw's), the palette does not fit below `c[170]`, or memory does not
/// read.
pub fn node_rows(mem: &dyn GuestRead, record: u32) -> Result<Vec<Row>, String> {
    let matrix = word(mem, record.wrapping_add(RECORD_MATRIX))?;
    if matrix != 0 {
        let f = |o: u32| float(mem, matrix.wrapping_add(o));
        let mut rows = Vec::with_capacity(3);
        for axis in 0..3 {
            let a = 4 + 4 * axis;
            rows.push([f(a)?, f(a + 0xc)?, f(a + 0x18)?, f(a + 0x24)?]);
        }
        return Ok(rows);
    }
    let handle = word(mem, record.wrapping_add(RECORD_NODES))?;
    if handle == u32::MAX {
        return Err("the record has no node matrices (the draw uploads none)".into());
    }
    if (handle as i32) < 0 {
        return Err(format!("node block handle {handle:#x} is negative"));
    }
    let block = NODE_BLOCKS.wrapping_add(handle & 0x0fff_ffff);
    let key = word(mem, record.wrapping_add(RECORD_KEY))?;
    let part = key & 0xf;
    let entry = word(mem, block.wrapping_add(BLOCK_PARTS + 4 * part))?;
    let (first, count) = (entry as u16 as i16, (entry >> 16) as u16);
    let matrices = |first: i32, count: usize| -> Result<Vec<Row>, String> {
        let base = block.wrapping_add(BLOCK_MATRICES).wrapping_add((first as u32).wrapping_mul(MATRIX_STRIDE));
        (0..3 * count)
            .map(|r| -> Result<Row, String> {
                let at = base.wrapping_add(16 * r as u32);
                let f = |o: u32| float(mem, at.wrapping_add(o));
                Ok([f(0)?, f(4)?, f(8)?, f(12)?])
            })
            .collect()
    };
    if key & 0xe000_0000 == MODEL_RECORD {
        let datum = word(mem, record.wrapping_add(RECORD_MODEL))?;
        let node = section_node(mem, datum, (key >> 9) & 0x1ff)?;
        if node != -1 {
            let index = if count == 1 { i32::from(first) } else { i32::from(node) };
            return matrices(index, 1);
        }
    }
    if count == 0 {
        return Err(format!("part {part} has no matrices (the draw uploads none)"));
    }
    if usize::from(count) > MAX_PALETTE {
        return Err(format!("a palette of {count} matrices runs past c[170]"));
    }
    matrices(i32::from(first), usize::from(count))
}

/// A section's compression as `0x0001cdd0` reads it: the position bounds
/// `(x min, x max, y min, y max, z min, z max)` when its position is
/// compressed, the texture coordinate's `(u min, u max, v min, v max)`
/// when its is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Compression {
    pub position: Option<[f32; 6]>,
    pub texcoord: Option<[f32; 4]>,
}

/// `c[170..173]` as `0x0001cdd0` uploads them.
pub fn decompression_rows(c: &Compression) -> [Row; 3] {
    let mut rows = [[1.0, 1.0, 1.0, 1.0], [0.0; 4], [1.0, 1.0, 0.0, 0.0]];
    if let Some(b) = c.position {
        rows[0] = [(b[1] - b[0]) * 0.5, (b[3] - b[2]) * 0.5, (b[5] - b[4]) * 0.5, 1.0];
        rows[1] = [(b[0] + b[1]) * 0.5, (b[2] + b[3]) * 0.5, (b[4] + b[5]) * 0.5, 0.0];
    }
    if let Some(t) = c.texcoord {
        rows[2] = [(t[1] - t[0]) * 0.5, (t[3] - t[2]) * 0.5, (t[0] + t[1]) * 0.5, (t[2] + t[3]) * 0.5];
    }
    rows
}

/// The scalar inputs every draw of a shader holds the same
/// (`0x0001bbf0` copies them from the `shad`), and the integer ones.
const SHADER_SCALARS: u64 = 1 << 21;
const SHADER_INTS: u32 = 1 << 4 | 1 << 5;

/// Whether every engine input `imp` reads is one the object path gives:
/// the object's lighting ([`crate::object_lighting`]) or the shader's
/// own. And that the lighting lands only in vertex registers: a combiner
/// constant computed from it would need a per-object pixel block. The
/// first input that is not, as the reason the object stays on D3D.
pub fn inputs_supported(imp: &ResolvedImplementation) -> Result<(), String> {
    let r = imp.input_reads;
    let v = r.vectors & !LIGHTING_VECTORS;
    if v != 0 {
        return Err(format!("reads vector input {}, which is not the object's lighting", v.trailing_zeros()));
    }
    let s = r.scalars & !LIGHTING_SCALARS & !SHADER_SCALARS;
    if s != 0 {
        return Err(format!("reads scalar input {}, which is not the object's lighting", s.trailing_zeros()));
    }
    let i = r.ints & !SHADER_INTS;
    if i != 0 {
        return Err(format!("reads integer input {}", i.trailing_zeros()));
    }
    let lit = |b: &&tags::material::VectorBinding, mask: u64| b.source < 64 && mask & (1u64 << b.source) != 0;
    let pixel = |b: &&tags::material::VectorBinding| matches!(b.target, VectorTarget::Pixel { .. });
    if let Some(b) = imp.vector_bindings.iter().filter(pixel).find(|b| lit(b, LIGHTING_VECTORS)) {
        return Err(format!("a combiner constant reads lighting vector {}", b.source));
    }
    if let Some(b) = imp.scalar_bindings.iter().filter(pixel).find(|b| lit(b, LIGHTING_SCALARS)) {
        return Err(format!("a combiner constant reads lighting scalar {}", b.source));
    }
    Ok(())
}

/// Why objects with `material` stay on D3D for their lighting, or `None`
/// when no implementation of its layer-3 passes (first level of detail)
/// reads it. Every implementation counts (the build picks one per object
/// from its lighting): one reading an input the object path does not give
/// is named with its reason; otherwise the inputs are all given but the
/// native objects pass does not draw through [`compile_object`] yet.
/// Never drawn unlit.
pub fn lighting_refusal(material: &tags::material::Material) -> Option<String> {
    let input = crate::look::object_lighting_input(material)?;
    let layers = material.layers.iter().filter(|l| l.lod == 0 && l.layer == crate::look::LIGHTMAP_LAYER);
    for imp in layers.flat_map(|l| &l.passes).flat_map(|p| &p.implementations) {
        if let Err(e) = inputs_supported(imp) {
            return Some(format!("implementation {}: {e}", imp.index));
        }
    }
    Some(format!("reads lighting input {input:?}, which is given, but the object program draw is not wired yet"))
}

/// `pass`'s implementation `imp` as an object draws it, or why not: what
/// [`implementation_look`] takes, except that engine vertex constants are
/// allowed when [`inputs_supported`] (they are put in per object by
/// [`object_vertex_uniforms`]; the look holds their resolved values).
pub fn object_look(pass: map::TagId, imp: &ResolvedImplementation) -> Result<PassLook, String> {
    inputs_supported(imp)?;
    let mut instance = imp.clone();
    for r in instance.vertex_constants.iter_mut().chain(instance.vertex_defaults.iter_mut()) {
        r.origin = Origin::Instance;
    }
    let mut look = implementation_look(pass, &instance).map_err(|r: Refusal| format!("{r:?}"))?;
    look.per_draw_inputs = false;
    Ok(look)
}

/// Whether an object pass sets constant `c` (hardware numbering) for
/// instance constants `look`: the view's, the implementation's, the node
/// rows and the decompression.
fn object_sets(look: &PassLook, c: usize) -> bool {
    c < VIEW_CONSTANTS
        || (MODEL_ROWS..MODEL_ROWS + 3).contains(&c)
        || (POSITION_DECOMPRESSION..=TEXCOORD_DECOMPRESSION).contains(&c)
        || look.vertex_constants.iter().any(|r| usize::from(r.index) == c && r.written != 0)
}

/// Translates an object pass with its vertex program `microcode`, or says
/// why it stays on D3D. Every constant it reads directly must be one
/// [`object_sets`]; a read relative to `a0.x` must start in the node rows
/// (the skinned palette, which the draw fills for the part's matrices).
pub fn compile_object(
    look: &PassLook,
    microcode: &[u8],
    alpha_test: Option<(wgpu::CompareFunction, u8)>,
) -> Result<MaterialProgram, String> {
    let program = vsh::Program::parse(microcode).map_err(|e| format!("vertex program: {e:?}"))?;
    for i in &program.instructions {
        for s in i.sources() {
            if let vsh::SrcReg::Const { index, relative } = s.reg {
                let c = usize::from(index);
                if relative && !(MODEL_ROWS..POSITION_DECOMPRESSION).contains(&c) {
                    return Err(format!("vertex program reads c[a0.x + {c}], outside the node rows"));
                }
                if !relative && !object_sets(look, c) {
                    return Err(format!("vertex program reads c[{c}], which the object pass does not set"));
                }
            }
        }
    }
    let t = translate_pass(&PassParts {
        microcode,
        pixel_shader: &look.pixel_shader,
        dims: look.textures.map(|s| (s != crate::look::Stage::Empty).then_some(shaders::psh::TextureDim::D2)),
        biased: look.textures.map(|s| matches!(s, crate::look::Stage::Bitmap { .. })),
        stage_states: &look.stage_states,
        vertex_constants: &look.vertex_constants,
        vertex_defaults: &look.vertex_defaults,
        alpha_test,
    })?;
    let mut vertex = t.vertex;
    vertex.constants[POSITION_DECOMPRESSION..=TEXCOORD_DECOMPRESSION].copy_from_slice(&decompression_rows(&Compression::default()));
    Ok(MaterialProgram {
        wgsl: t.wgsl,
        stages: look.textures,
        samplers: t.samplers,
        dims: t.dims,
        signed: t.signed,
        vertex,
        pixel: t.pixel,
        inputs: t.inputs & INPUT_MASK,
        state: None,
    })
}

/// One drawn object's inputs.
#[derive(Clone, Copy, Debug)]
pub struct ObjectInputs<'a> {
    pub lighting: Option<&'a Lighting>,
    pub defaults: &'a Defaults,
    pub shader: ShaderValues,
}

/// The vertex block an object's draw of `imp` reads: `base` (the
/// compiled pass's, [`compile_object`]) with the engine constants that
/// read the lighting evaluated for this object, its node rows at `c[50]`
/// and its decompression at `c[170]`. The view's `c[0..18]` are put in
/// per frame. An error names a lighting input the port cannot give for
/// this object (then it stays on D3D) or rows that do not fit.
pub fn object_vertex_uniforms(
    base: &VertexUniforms,
    imp: &ResolvedImplementation,
    inputs: &ObjectInputs<'_>,
    nodes: &[Row],
    decompression: [Row; 3],
) -> Result<VertexUniforms, String> {
    let mut u = base.clone();
    let mut put = |bank: u8, reg: u8, components: u8, value: [u32; 3], scalar: bool| {
        let row = match bank {
            1 => u.constants.get_mut(18 + usize::from(reg)),
            _ => u.defaults.get_mut(usize::from(reg)),
        };
        let Some(row) = row else { return };
        for (c, slot) in row.iter_mut().enumerate() {
            if components & (1 << c) != 0 {
                *slot = f32::from_bits(if scalar { value[0] } else { value[c.min(2)] });
            }
        }
    };
    for b in &imp.vector_bindings {
        let VectorTarget::Vertex { bank, reg, components } = b.target else { continue };
        if b.source >= 64 || LIGHTING_VECTORS & (1u64 << b.source) == 0 {
            continue;
        }
        let v = vector_input(u32::from(b.source), inputs.lighting, inputs.defaults, inputs.shader)
            .ok_or_else(|| format!("lighting vector {} is not given for this object", b.source))?;
        put(bank, reg, components & 0b0111, v, false);
    }
    for b in &imp.scalar_bindings {
        let VectorTarget::Vertex { bank, reg, components } = b.target else { continue };
        if b.source >= 64 || LIGHTING_SCALARS & (1u64 << b.source) == 0 {
            continue;
        }
        let s = scalar_input(u32::from(b.source), inputs.lighting)
            .ok_or_else(|| format!("lighting scalar {} is not given for this object", b.source))?;
        put(bank, reg, components, [s, s, s], true);
    }
    if nodes.len() > 3 * MAX_PALETTE {
        return Err(format!("{} node rows run past c[170]", nodes.len()));
    }
    u.constants[MODEL_ROWS..MODEL_ROWS + nodes.len()].copy_from_slice(nodes);
    u.constants[POSITION_DECOMPRESSION..=TEXCOORD_DECOMPRESSION].copy_from_slice(&decompression);
    Ok(u)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::tests::Mem;
    use crate::look::Stage;
    use shaders::vsh::encode::{Inst, Operand as O, program};
    use shaders::vsh::{MacOp, OutReg};
    use tags::material::{InputReads, Register, VectorBinding};

    const RECORD: u32 = 0x0060_0000;
    const MATRIX: u32 = 0x0060_1000;

    /// Matrix `n`'s twelve floats: `100·n + i`.
    fn block(m: &mut Mem, handle: u32, parts: &[(i16, u16)], matrices: u32) -> u32 {
        let b = NODE_BLOCKS + handle;
        m.w(b, matrices);
        for (p, &(first, count)) in parts.iter().enumerate() {
            m.w(b + BLOCK_PARTS + 4 * p as u32, u32::from(first as u16) | u32::from(count) << 16);
        }
        for n in 0..matrices {
            for i in 0..12 {
                m.f(b + BLOCK_MATRICES + MATRIX_STRIDE * n + 4 * i, (100 * n + i) as f32);
            }
        }
        b
    }

    fn rows_of(n: u32) -> Vec<Row> {
        (0..3).map(|r| std::array::from_fn(|c| (100 * n + 4 * r + c as u32) as f32)).collect()
    }

    fn record(m: &mut Mem, matrix: u32, nodes: u32, key: u32, model: u32) {
        m.w(RECORD + RECORD_MATRIX, matrix);
        m.w(RECORD + RECORD_NODES, nodes);
        m.w(RECORD + RECORD_KEY, key);
        m.w(RECORD + RECORD_MODEL, model);
    }

    #[test]
    fn a_single_matrix_is_uploaded_transposed_without_its_scale() {
        let mut m = Mem::default();
        // Scale 9, forward (1,2,3), left (4,5,6), up (7,8,9), position (10,11,12).
        m.f(MATRIX, 9.0);
        for i in 0..12 {
            m.f(MATRIX + 4 + 4 * i, (i + 1) as f32);
        }
        record(&mut m, MATRIX, u32::MAX, 0, u32::MAX);
        assert_eq!(
            node_rows(&m, RECORD).unwrap(),
            vec![[1.0, 4.0, 7.0, 10.0], [2.0, 5.0, 8.0, 11.0], [3.0, 6.0, 9.0, 12.0]]
        );
    }

    #[test]
    fn a_skinned_part_uploads_its_palette_from_its_first_matrix() {
        let mut m = Mem::default();
        block(&mut m, 0x40, &[(0, 1), (2, 3)], 6);
        // Part 1, not a model record: matrices 2, 3, 4 at c[50], c[53], c[56].
        record(&mut m, 0, 0x40, 1, u32::MAX);
        let rows = node_rows(&m, RECORD).unwrap();
        assert_eq!(rows, [rows_of(2), rows_of(3), rows_of(4)].concat());
        // No block: the draw uploads nothing.
        record(&mut m, 0, u32::MAX, 1, u32::MAX);
        assert!(node_rows(&m, RECORD).unwrap_err().contains("no node matrices"));
        // A palette past c[170].
        block(&mut m, 0x40, &[(0, 1), (0, MAX_PALETTE as u16 + 1)], 1);
        record(&mut m, 0, 0x40, 1, u32::MAX);
        assert!(node_rows(&m, RECORD).unwrap_err().contains("runs past"));
    }

    #[test]
    fn a_rigid_model_section_uploads_its_nodes_matrix() {
        let mut m = Mem::default();
        let (table, tag, sections) = (0x0061_0000, 0x0062_0000, 0x0063_0000);
        m.w(TAG_INSTANCES, table);
        m.w(table + 16 * 5 + 8, tag);
        m.w(tag + MODEL_SECTIONS, sections);
        // Section 2's node is 4 (the i16 at +0x2c).
        m.w(sections + SECTION_STRIDE * 2 + SECTION_NODE, 0xbeef_0004);
        block(&mut m, 0x80, &[(0, 1), (1, 3)], 6);
        let key = MODEL_RECORD | 2 << 9 | 1;
        record(&mut m, 0, 0x80, key, 0xe000_0005);
        assert_eq!(section_node(&m, 0xe000_0005, 2), Ok(4));
        assert_eq!(node_rows(&m, RECORD).unwrap(), rows_of(4), "the section's node");
        // A part with one matrix: that one.
        record(&mut m, 0, 0x80, MODEL_RECORD | 2 << 9, 0xe000_0005);
        assert_eq!(node_rows(&m, RECORD).unwrap(), rows_of(0));
        // A section without a node (-1): the part's palette.
        m.w(sections + SECTION_STRIDE * 2 + SECTION_NODE, 0xffff);
        record(&mut m, 0, 0x80, key, 0xe000_0005);
        assert_eq!(node_rows(&m, RECORD).unwrap(), [rows_of(1), rows_of(2), rows_of(3)].concat());
    }

    #[test]
    fn decompression_is_half_extent_and_centre_or_the_identity() {
        assert_eq!(decompression_rows(&Compression::default()), [[1.0; 4], [0.0; 4], [1.0, 1.0, 0.0, 0.0]]);
        let c = Compression {
            position: Some([-2.0, 4.0, 0.0, 1.0, 10.0, 20.0]),
            texcoord: Some([0.0, 2.0, -1.0, 1.0]),
        };
        assert_eq!(
            decompression_rows(&c),
            [[3.0, 0.5, 5.0, 1.0], [1.0, 0.5, 15.0, 0.0], [1.0, 1.0, 1.0, 0.0]]
        );
    }

    fn implementation(reads: InputReads, vectors: Vec<VectorBinding>, scalars: Vec<VectorBinding>) -> ResolvedImplementation {
        let reg = |index, origin| Register { index, value: [7.0; 4], written: 0xf, origin };
        ResolvedImplementation {
            index: 0,
            vertex_shader: Some(map::TagId(0xe000_0002)),
            textures: [None; 4],
            stage_states: Vec::new(),
            render_states: Vec::new(),
            restore_states: Vec::new(),
            pixel_shader: shaders::PixelShaderDef::default().to_bytes(),
            variant: false,
            c0_written: 0,
            c1_written: 0,
            pixel_constants_from_engine: false,
            per_draw_inputs: true,
            input_reads: reads,
            vector_bindings: vectors,
            scalar_bindings: scalars,
            vertex_constants: vec![reg(18, Origin::Engine), reg(19, Origin::Engine), reg(20, Origin::Instance)],
            vertex_defaults: Vec::new(),
            animated: Vec::new(),
            engine_render_states: Vec::new(),
            engine_stage_states: Vec::new(),
        }
    }

    fn vertex(reg: u8, components: u8) -> VectorTarget {
        VectorTarget::Vertex { bank: 1, reg, components }
    }

    /// A lit object implementation: vector 15 into `c[18].xyz`, vector 40
    /// into `c[19].xyz`, scalar 23 into `c[19].w`.
    fn lit() -> ResolvedImplementation {
        implementation(
            InputReads { vectors: 1 << 15 | 1 << 40, scalars: 1 << 23 | 1 << 21, ints: 1 << 4 },
            vec![
                VectorBinding { source: 15, target: vertex(0, 0b0111) },
                VectorBinding { source: 40, target: vertex(1, 0b0111) },
            ],
            vec![VectorBinding { source: 23, target: vertex(1, 0b1000) }],
        )
    }

    #[test]
    fn inputs_other_than_the_lighting_and_the_shaders_are_refused() {
        assert_eq!(inputs_supported(&lit()), Ok(()));
        let mut time = lit();
        time.input_reads.scalars |= 1 << 2;
        assert_eq!(inputs_supported(&time).unwrap_err(), "reads scalar input 2, which is not the object's lighting");
        let mut camera = lit();
        camera.input_reads.vectors |= 1 << 5;
        assert!(inputs_supported(&camera).unwrap_err().starts_with("reads vector input 5"));
        let mut pixel = lit();
        pixel.vector_bindings.push(VectorBinding {
            source: 3,
            target: VectorTarget::Pixel { c1: false, slot: 0, mask: 0x00ff_ffff, half: false },
        });
        assert_eq!(inputs_supported(&pixel).unwrap_err(), "a combiner constant reads lighting vector 3");
    }

    #[test]
    fn an_objects_block_holds_what_the_games_upload_would() {
        // The lighting: +0x30 (vector 15) = (0.5, 0.25, 0.125); +0x10 = 0;
        // its object's root value 0.75.
        let mut words = [0u32; 30];
        for (i, v) in [0.5f32, 0.25, 0.125].iter().enumerate() {
            words[12 + i] = v.to_bits();
        }
        let l = Lighting { words, root: Some(0.75f32.to_bits()) };
        let d = Defaults::default();
        let inputs = ObjectInputs { lighting: Some(&l), defaults: &d, shader: ShaderValues::default() };
        let imp = lit();
        let look = object_look(map::TagId(0xe000_0001), &imp).unwrap();
        let p = compile_object(&look, &rigid_program(), None).unwrap();
        let nodes = rows_of(3);
        let decompression = decompression_rows(&Compression { position: Some([0.0, 2.0, 0.0, 2.0, 0.0, 2.0]), texcoord: None });
        let u = object_vertex_uniforms(&p.vertex, &imp, &inputs, &nodes, decompression).unwrap();
        // What the game would hold: c[18] = vector 15 (w the resolved value),
        // c[19] = vector 40 broadcast and scalar 23 in w, c[20] the instance's.
        assert_eq!(u.constants[18], [0.5, 0.25, 0.125, 7.0]);
        let luma = (0.125f32 * 0.114 + 0.25 * 0.587 + 0.5 * 0.299).clamp(0.0, 1.0);
        assert_eq!(u.constants[19], [0.75, 0.75, 0.75, luma]);
        assert_eq!(u.constants[20], [7.0; 4]);
        assert_eq!(u.constants[50..53], nodes[..]);
        assert_eq!(u.constants[170..173], decompression[..]);
        // No root value (a parent cycle): not drawn.
        let stale = Lighting { root: None, ..l };
        let e = object_vertex_uniforms(&p.vertex, &imp, &ObjectInputs { lighting: Some(&stale), ..inputs }, &nodes, decompression);
        assert_eq!(e.unwrap_err(), "lighting vector 40 is not given for this object");
    }

    /// Shaped like a `lightmap_dynamic` object program: decompress the
    /// position (`c[170]`, `c[171]`), place it with the node rows
    /// (`c[50..53]`), project (`c[0..4]`), decompress the texture
    /// coordinate (`c[172]`), light with the implementation's `c[18]`,
    /// `c[19]`.
    fn rigid_program() -> Vec<u8> {
        let r = |n| O::r(n);
        program(&[
            Inst::mac(MacOp::Mul, Some(O::v(0)), Some(O::c(170)), None).to_temp(0, 0xf),
            Inst::mac(MacOp::Add, Some(r(0)), None, Some(O::c(171))).to_temp(0, 0xf),
            Inst::mac(MacOp::Dp4, Some(r(0)), Some(O::c(50)), None).to_temp(1, 8),
            Inst::mac(MacOp::Dp4, Some(r(0)), Some(O::c(51)), None).to_temp(1, 4),
            Inst::mac(MacOp::Dp4, Some(r(0)), Some(O::c(52)), None).to_temp(1, 2),
            Inst::mac(MacOp::Mov, Some(O::c(170)), None, None).to_temp(1, 1),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(0)), None).to_out(false, OutReg::Pos, 8),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(1)), None).to_out(false, OutReg::Pos, 4),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(2)), None).to_out(false, OutReg::Pos, 2),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(3)), None).to_out(false, OutReg::Pos, 1),
            Inst::mac(MacOp::Mad, Some(O::v(1)), Some(O::c(172).swz("xyxy")), Some(O::c(172).swz("zwzw")))
                .to_out(false, OutReg::T(0), 0xf),
            Inst::mac(MacOp::Dp3, Some(O::v(2)), Some(O::c(18)), None).to_temp(2, 0xf),
            Inst::mac(MacOp::Mul, Some(r(2)), Some(O::c(19)), None).to_out(false, OutReg::D0, 0xf),
        ])
    }

    /// Shaped like an `sh_dynamic` skinned program: the node index from
    /// the vertex into `a0.x`, the palette read relative to it
    /// (`c[a0.x + 50..53]`), the lighting from `c[18..21]`.
    fn skinned_program() -> Vec<u8> {
        let r = |n| O::r(n);
        program(&[
            Inst::mac(MacOp::Mul, Some(O::v(0)), Some(O::c(170)), None).to_temp(0, 0xf),
            Inst::mac(MacOp::Add, Some(r(0)), None, Some(O::c(171))).to_temp(0, 0xf),
            Inst::mac(MacOp::Arl, Some(O::v(3)), None, None),
            Inst::mac(MacOp::Dp4, Some(r(0)), Some(O::c_rel(50)), None).to_temp(1, 8),
            Inst::mac(MacOp::Dp4, Some(r(0)), Some(O::c_rel(51)), None).to_temp(1, 4),
            Inst::mac(MacOp::Dp4, Some(r(0)), Some(O::c_rel(52)), None).to_temp(1, 2),
            Inst::mac(MacOp::Mov, Some(O::c(170)), None, None).to_temp(1, 1),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(0)), None).to_out(false, OutReg::Pos, 8),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(1)), None).to_out(false, OutReg::Pos, 4),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(2)), None).to_out(false, OutReg::Pos, 2),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(3)), None).to_out(false, OutReg::Pos, 1),
            Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf),
            Inst::mac(MacOp::Dp3, Some(O::v(2)), Some(O::c(18)), None).to_temp(2, 0xf),
            Inst::mac(MacOp::Mul, Some(r(2)), Some(O::c(19)), None).to_temp(3, 0xf),
            Inst::mac(MacOp::Add, Some(r(3)), None, Some(O::c(20))).to_out(false, OutReg::D0, 0xf),
        ])
    }

    #[test]
    fn lightmap_and_sh_dynamic_shaped_programs_compile_with_their_constants() {
        let imp = lit();
        let look = object_look(map::TagId(0xe000_0001), &imp).unwrap();
        assert_eq!(look.textures, [Stage::Empty; 4]);
        for (name, microcode) in [("lightmap_dynamic", rigid_program()), ("sh_dynamic", skinned_program())] {
            let p = compile_object(&look, &microcode, None).unwrap_or_else(|e| panic!("{name}: {e}"));
            shaders::validate(&p.wgsl).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(p.vertex.constants[170..173], decompression_rows(&Compression::default())[..], "{name}");
            assert_eq!(p.vertex.constants[20], [7.0; 4], "{name}: the instance constant");
        }
        let p = compile_object(&look, &skinned_program(), None).unwrap();
        assert!(p.wgsl.contains("a0"), "the palette is read relative to a0.x");
    }

    #[test]
    fn an_object_program_reading_what_the_pass_does_not_set_is_refused() {
        let look = object_look(map::TagId(0xe000_0001), &lit()).unwrap();
        let reads = |src: O| {
            program(&[Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(src), None).to_out(false, OutReg::Pos, 0xf)])
        };
        for c in [0, 17, 18, 19, 20, 50, 52, 170, 171, 172] {
            assert!(compile_object(&look, &reads(O::c(c)), None).is_ok(), "c[{c}] is set");
        }
        for c in [21, 31, 53, 173] {
            let e = compile_object(&look, &reads(O::c(c)), None).unwrap_err();
            assert_eq!(e, format!("vertex program reads c[{c}], which the object pass does not set"));
        }
        let e = compile_object(&look, &reads(O::c_rel(18)), None).unwrap_err();
        assert_eq!(e, "vertex program reads c[a0.x + 18], outside the node rows");
    }
}
