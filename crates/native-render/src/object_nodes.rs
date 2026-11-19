//! The node rows and decompression an object's layer-3 draw uploads as
//! vertex constants, read from the render record at hook time
//! (docs/graphics/native-renderer.md, "Object programs"). Pure; the
//! feature's hook copies them into the frame ([`crate::objects`]).
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
//! - The implementation (`0x0004cbf0`, the build of one record's draw;
//!   its third argument the draw, `edi`, through the jump table at
//!   `0x0004d054` indexed by the bytes at `0x0004d068`): at draw 3 (layer
//!   3) it is the record's `+0` bits 29..31 (`0x0004cc70`, the table at
//!   `0x0004d07c`: 0..4 as they are, 5..7 give 3), whatever `0x00015d70`
//!   chose before. The lighting's `+0x76` and the template's layer-15 bit
//!   pick it only for draw 18 (`0x0004cc9f`..`0x0004cd46`), not layer 3.
//! - `0x0001cdd0` (from the section setup `0x0004d7c0`): twelve dwords at
//!   hardware `c[170]` (`ECX` 0xaa): the position scale and offset (half
//!   the bounds' extent, w 1; their centre, w 0) and the texture
//!   coordinate's (`(su, sv, ou, ov)`), each the identity when the
//!   section's flag for it is clear.

use crate::camera::GuestRead;

/// The node rows (`c[50]`, hardware numbering).
pub const MODEL_ROWS: usize = 50;

/// The node blocks a render record's `+8` handle points into.
pub const NODE_BLOCKS: u32 = 0x0048_7b1c;
/// A render record's node block handle.
pub const RECORD_NODES: u32 = 0x8;
/// The record's word whose bits 29..31 pick the layer-3 implementation.
pub const RECORD_IMPLEMENTATION: u32 = 0x0;
/// Implementations a layer-3 draw may pick ([`record_implementation`]).
pub const IMPLEMENTATIONS: usize = 5;

/// The implementation of its layer-3 pass a record draws, from its word
/// `+0` (`0x0004cc70`): bits 29..31, 5..7 taken as 3.
pub fn record_implementation(word: u32) -> usize {
    match word >> 29 {
        n @ 0..=4 => n as usize,
        _ => 3,
    }
}
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
/// (`crate::program::TEXCOORD_DECOMPRESSION`).
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


#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::camera::tests::Mem;

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

    pub(crate) fn rows_of(n: u32) -> Vec<Row> {
        (0..3).map(|r| std::array::from_fn(|c| (100 * n + 4 * r + c as u32) as f32)).collect()
    }

    fn record(m: &mut Mem, matrix: u32, nodes: u32, key: u32, model: u32) {
        m.w(RECORD + RECORD_MATRIX, matrix);
        m.w(RECORD + RECORD_NODES, nodes);
        m.w(RECORD + RECORD_KEY, key);
        m.w(RECORD + RECORD_MODEL, model);
    }

    #[test]
    fn a_records_top_bits_pick_its_layer3_implementation() {
        let got: Vec<usize> = (0..8u32).map(|t| record_implementation(t << 29 | 0x1234)).collect();
        assert_eq!(got, vec![0, 1, 2, 3, 4, 3, 3, 3]);
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

}
