//! An object section's vertex streams as the game's vertex declaration
//! feeds them to its vertex program (docs/graphics/native-objects.md,
//! "Object programs"): which `v` register reads which bytes, in which
//! format. Pure: the stream formats are read from guest memory at run time
//! ([`StreamFormats::read`]), nothing of them is copied here.
//!
//! Research points (Code, `h2 disasm`):
//!
//! - The object draw (`0x0004d0b0`) sets the vertex program and streams
//!   through `0x0001caa0`: the classification of the section's `vrtx`
//!   program by `0x0001cb20` ([`classification`]), the program
//!   (`0x0001c590`), and the streams (`0x0001c8c0`): the section's vertex
//!   buffer descriptors (`+0x38` count, `+0x3c` elements of `0x20` bytes,
//!   byte `+0` the stream type) in order, each stream's format the record
//!   of its type in the table at `0x0043f408` (`0x15` bytes a type).
//! - A format record (read by `0x0001c3e3`..`0x0001c46b` when the
//!   declaration is built): byte 0 the type, then up to ten pairs from
//!   byte 1: a semantic and a format code. A semantic of `0xfe` skips as
//!   many bytes as its second byte says; any other negative one ends the
//!   record. Each attribute's offset in the vertex is the sum of the sizes
//!   before it.
//! - The program's classification lists, per input register `v<i>`, the
//!   semantic it reads (classification `+4` count, `+8` u16 list): the
//!   declaration puts a stream's attribute of semantic `s` in the register
//!   whose list entry is `s` (a later entry for the same semantic wins, and
//!   a later stream for the same register).
//! - The format codes (`0x00035790` the D3D type, `0x00035850` the size)
//!   are the Xbox vertex data types in order: 0..3 float 1..4, 4..7 packed
//!   bytes 1..4 (`PBYTE`, over 255), 8..11 shorts 1..4, 12..15 normalised
//!   shorts 1..4 (over 32767), 16 normalised packed 11:11:10, 17 a
//!   `D3DCOLOR`.

use std::collections::{BTreeMap, HashMap};

use crate::camera::GuestRead;

/// The stream format table: one record per stream type.
pub const STREAM_FORMATS: u32 = 0x0043_f408;
/// Bytes per record.
pub const STREAM_FORMAT_SIZE: usize = 0x15;
/// The records read (types `0..=0x39`; `0x30` is the one `0x0001c9e0`
/// adds for the extra stream it is handed, `0x39` layer 5's PRT stream
/// `0x0001c96f` binds, `scene_extract::layout::prt`).
pub const STREAM_TYPES: usize = 0x3a;
/// Pairs at most per record.
const PAIRS: usize = 10;
/// The semantic that skips bytes.
const SKIP: u8 = 0xfe;

/// One attribute's format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Attribute {
    /// `n` floats.
    Float(u8),
    /// `n` bytes over 255.
    PByte(u8),
    /// `n` signed shorts, as they are.
    Short(u8),
    /// `n` signed shorts over 32767.
    NormShort(u8),
    /// Three signed fields of 11, 11 and 10 bits, over 1023, 1023, 511.
    NormPacked3,
    /// A `D3DCOLOR`: bytes B, G, R, A over 255, read as (R, G, B, A).
    Colour,
}

impl Attribute {
    /// The format of code `code` (`0x00035790`).
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0..=3 => Self::Float(code + 1),
            4..=7 => Self::PByte(code - 3),
            8..=11 => Self::Short(code - 7),
            12..=15 => Self::NormShort(code - 11),
            16 => Self::NormPacked3,
            17 => Self::Colour,
            _ => return None,
        })
    }

    /// The D3D vertex data type (`0x00035790`): the component count in
    /// bits 4..8, the NV2A type in bits 0..4 (`0x45` four `S32K` shorts).
    pub fn xbox_type(self) -> u32 {
        match self {
            Self::Float(n) => u32::from(n) * 16 + 2,
            Self::PByte(n) => u32::from(n) * 16 + 4,
            Self::Short(n) => u32::from(n) * 16 + 5,
            Self::NormShort(n) => u32::from(n) * 16 + 1,
            Self::NormPacked3 => 0x16,
            Self::Colour => 0x40,
        }
    }

    /// Bytes per attribute (`0x00035850`).
    pub fn size(self) -> usize {
        match self {
            Self::Float(n) => 4 * usize::from(n),
            Self::PByte(n) => usize::from(n),
            Self::Short(n) | Self::NormShort(n) => 2 * usize::from(n),
            Self::NormPacked3 | Self::Colour => 4,
        }
    }

    /// The float4 the vertex unit reads from `b` (at least [`Self::size`]
    /// bytes): missing components are `(0, 0, 0, 1)`; a normalised value
    /// below -1 is -1.
    pub fn decode(self, b: &[u8]) -> [f32; 4] {
        let mut out = [0.0, 0.0, 0.0, 1.0];
        let s = |i: usize| i16::from_le_bytes([b[2 * i], b[2 * i + 1]]);
        let sn = |v: i32, max: f32| (v as f32 / max).max(-1.0);
        match self {
            Self::Float(n) => {
                for (i, o) in out.iter_mut().enumerate().take(usize::from(n)) {
                    *o = f32::from_le_bytes(b[4 * i..4 * i + 4].try_into().expect("4 bytes"));
                }
            }
            Self::PByte(n) => {
                for (o, x) in out.iter_mut().zip(&b[..usize::from(n)]) {
                    *o = f32::from(*x) / 255.0;
                }
            }
            Self::Short(n) => {
                for (i, o) in out.iter_mut().enumerate().take(usize::from(n)) {
                    *o = f32::from(s(i));
                }
            }
            Self::NormShort(n) => {
                for (i, o) in out.iter_mut().enumerate().take(usize::from(n)) {
                    *o = sn(i32::from(s(i)), 32767.0);
                }
            }
            Self::NormPacked3 => {
                let w = u32::from_le_bytes(b[..4].try_into().expect("4 bytes"));
                let x = ((w << 21) as i32) >> 21;
                let y = ((w << 10) as i32) >> 21;
                let z = (w as i32) >> 22;
                out = [sn(x, 1023.0), sn(y, 1023.0), sn(z, 511.0), 1.0];
            }
            Self::Colour => out = [b[2], b[1], b[0], b[3]].map(|x| f32::from(x) / 255.0),
        }
        out
    }
}

/// One entry of a stream's format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StreamElement {
    Attribute { semantic: u8, format: Attribute },
    /// Bytes skipped.
    Skip(u8),
}

/// The stream format table, as read from guest memory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamFormats {
    records: Vec<[u8; STREAM_FORMAT_SIZE]>,
}

impl StreamFormats {
    /// Reads the table at [`STREAM_FORMATS`]; `None` when it does not read.
    pub fn read(mem: &dyn GuestRead) -> Option<Self> {
        let bytes = STREAM_TYPES * STREAM_FORMAT_SIZE;
        let mut flat = Vec::with_capacity(bytes + 4);
        let mut at = STREAM_FORMATS;
        while flat.len() < bytes {
            flat.extend_from_slice(&mem.read_u32(at)?.to_le_bytes());
            at = at.checked_add(4)?;
        }
        Some(Self::from_bytes(&flat[..bytes]))
    }

    /// The table from its bytes (records of [`STREAM_FORMAT_SIZE`]).
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            records: bytes
                .chunks_exact(STREAM_FORMAT_SIZE)
                .map(|c| c.try_into().expect("a whole record"))
                .collect(),
        }
    }

    /// The format of stream type `kind`; an error for a type past the
    /// table, a record of another type, or an unknown format code.
    pub fn elements(&self, kind: u8) -> Result<Vec<StreamElement>, String> {
        let r = self.records.get(usize::from(kind)).ok_or_else(|| format!("stream type {kind:#x} is past the format table"))?;
        if r[0] != kind {
            return Err(format!("stream type {kind:#x}'s format record names type {:#x}", r[0]));
        }
        let mut out = Vec::new();
        for pair in r[1..1 + 2 * PAIRS].chunks_exact(2) {
            let (semantic, second) = (pair[0], pair[1]);
            if semantic == SKIP {
                out.push(StreamElement::Skip(second));
            } else if (semantic as i8) < 0 {
                break;
            } else {
                let format = Attribute::from_code(second)
                    .ok_or_else(|| format!("stream type {kind:#x}: format code {second:#x}"))?;
                out.push(StreamElement::Attribute { semantic, format });
            }
        }
        Ok(out)
    }
}

/// Where one input register reads from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Element {
    /// `v<register>`.
    pub register: u8,
    /// The stream's index among the section's descriptors.
    pub stream: usize,
    /// Bytes into the stream's vertex.
    pub offset: usize,
    pub format: Attribute,
}

/// The declaration a program with input semantics `semantics` (register
/// `i` reads `semantics[i]`) gets over streams of formats `streams` (in
/// descriptor order), by register.
pub fn declaration(semantics: &[u16], streams: &[Vec<StreamElement>]) -> Vec<Element> {
    let mut register: HashMap<u16, u8> = HashMap::new();
    for (i, &s) in semantics.iter().enumerate().take(16) {
        register.insert(s, i as u8);
    }
    let mut out: BTreeMap<u8, Element> = BTreeMap::new();
    for (stream, elements) in streams.iter().enumerate() {
        let mut offset = 0;
        for e in elements {
            match *e {
                StreamElement::Skip(n) => offset += usize::from(n),
                StreamElement::Attribute { semantic, format } => {
                    if let Some(&r) = register.get(&u16::from(semantic)) {
                        out.insert(r, Element { register: r, stream, offset, format });
                    }
                    offset += format.size();
                }
            }
        }
    }
    out.into_values().collect()
}

/// Bit `n` for each `v<n>` the declaration supplies.
pub fn registers(declaration: &[Element]) -> u16 {
    declaration.iter().fold(0, |m, e| m | 1 << e.register)
}

/// One stream's bytes and vertex stride.
#[derive(Clone, Copy, Debug)]
pub struct StreamData<'a> {
    pub bytes: &'a [u8],
    pub stride: usize,
}

/// Bytes a register's slot holds in [`gather`]: the widest attribute
/// (`FLOAT4`), and room for the four-wide read of a three-wide one.
pub const SLOT: usize = 16;

/// The declaration's registers for `count` vertices as the guest stores
/// them: per vertex, per element in `declaration`'s order, the element's
/// bytes at the start of a zeroed [`SLOT`]. The vertex unit converts them
/// as the D3D path's fetch does (`object_program_gpu::vertex_fetch`), not
/// the CPU: a depth-equal pass must reproduce, bit for bit, the depth an
/// earlier D3D pass wrote with the same program. An error when an
/// element's stream is missing or too short.
pub fn gather(declaration: &[Element], streams: &[Option<StreamData<'_>>], count: usize) -> Result<Vec<[u8; SLOT]>, String> {
    let mut out = Vec::with_capacity(count * declaration.len());
    for v in 0..count {
        for e in declaration {
            let s = streams
                .get(e.stream)
                .copied()
                .flatten()
                .ok_or_else(|| format!("v{} reads stream {}, which has no data", e.register, e.stream))?;
            let at = v * s.stride + e.offset;
            let size = e.format.size();
            let bytes = s.bytes.get(at..at + size).ok_or_else(|| format!("v{} reads past stream {} at vertex {v}", e.register, e.stream))?;
            let mut slot = [0u8; SLOT];
            slot[..size].copy_from_slice(bytes);
            out.push(slot);
        }
    }
    Ok(out)
}

/// The declaration's registers for `count` vertices as float4s, vertex
/// after vertex, each vertex its elements in `declaration`'s order. An
/// error when an element's stream is missing or too short.
pub fn expand(declaration: &[Element], streams: &[Option<StreamData<'_>>], count: usize) -> Result<Vec<[f32; 4]>, String> {
    let mut out = Vec::with_capacity(count * declaration.len());
    for v in 0..count {
        for e in declaration {
            let s = streams
                .get(e.stream)
                .copied()
                .flatten()
                .ok_or_else(|| format!("v{} reads stream {}, which has no data", e.register, e.stream))?;
            let at = v * s.stride + e.offset;
            let bytes = s
                .bytes
                .get(at..at + e.format.size())
                .ok_or_else(|| format!("v{} reads past stream {} at vertex {v}", e.register, e.stream))?;
            out.push(e.format.decode(bytes));
        }
    }
    Ok(out)
}

/// The `vrtx` classification of a section (moved with the object walk
/// that reads it).
pub use scene_extract::layout::object_walk::classification;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::tests::Mem;

    /// A synthetic table: type 1 a 3-float position (semantic 0) and a
    /// 2-normshort coordinate (semantic 3); type 2 two padding bytes, a
    /// packed normal (semantic 2) and a colour (semantic 5); type 3 an
    /// unknown code.
    fn table() -> Vec<u8> {
        let mut t = vec![0u8; 4 * STREAM_FORMAT_SIZE];
        let mut put = |kind: usize, pairs: &[u8]| {
            let r = &mut t[kind * STREAM_FORMAT_SIZE..(kind + 1) * STREAM_FORMAT_SIZE];
            r[0] = kind as u8;
            r[1..1 + pairs.len()].copy_from_slice(pairs);
        };
        put(0, &[0xff]);
        put(1, &[0, 2, 3, 13, 0xff]);
        put(2, &[SKIP, 2, 2, 16, 5, 17, 0xff]);
        put(3, &[1, 40, 0xff]);
        t
    }

    #[test]
    fn a_format_record_lists_its_attributes_and_skips() {
        let f = StreamFormats::from_bytes(&table());
        assert_eq!(f.elements(0), Ok(Vec::new()));
        use StreamElement::{Attribute as A, Skip};
        assert_eq!(
            f.elements(1),
            Ok(vec![
                A { semantic: 0, format: Attribute::Float(3) },
                A { semantic: 3, format: Attribute::NormShort(2) }
            ])
        );
        assert_eq!(
            f.elements(2),
            Ok(vec![Skip(2), A { semantic: 2, format: Attribute::NormPacked3 }, A { semantic: 5, format: Attribute::Colour }])
        );
        assert!(f.elements(3).unwrap_err().contains("format code 0x28"));
        assert!(f.elements(9).unwrap_err().contains("past"));
        let mut wrong = table();
        wrong[STREAM_FORMAT_SIZE] = 7;
        assert!(StreamFormats::from_bytes(&wrong).elements(1).unwrap_err().contains("names type 0x7"));
    }

    #[test]
    fn the_table_reads_from_guest_memory() {
        let mut m = Mem::default();
        let mut bytes = table();
        bytes.resize(STREAM_TYPES * STREAM_FORMAT_SIZE + 4, 0);
        for (i, w) in bytes.chunks_exact(4).enumerate() {
            m.w(STREAM_FORMATS + 4 * i as u32, u32::from_le_bytes(w.try_into().unwrap()));
        }
        let f = StreamFormats::read(&m).unwrap();
        assert_eq!(f, StreamFormats::from_bytes(&bytes[..STREAM_TYPES * STREAM_FORMAT_SIZE]));
        assert_eq!(StreamFormats::read(&Mem::default()), None);
    }

    #[test]
    fn the_declaration_puts_each_semantic_in_the_register_listing_it() {
        let f = StreamFormats::from_bytes(&table());
        let streams = [f.elements(1).unwrap(), f.elements(2).unwrap()];
        // v0 position, v1 the normal, v2 nothing streamed (semantic 9), v3
        // the coordinate; the colour's semantic 5 is not listed.
        let d = declaration(&[0, 2, 9, 3], &streams);
        assert_eq!(
            d,
            vec![
                Element { register: 0, stream: 0, offset: 0, format: Attribute::Float(3) },
                Element { register: 1, stream: 1, offset: 2, format: Attribute::NormPacked3 },
                Element { register: 3, stream: 0, offset: 12, format: Attribute::NormShort(2) },
            ]
        );
        assert_eq!(registers(&d), 0b1011);
        // A semantic listed twice: the later register.
        let d = declaration(&[0, 0], &streams[..1]);
        assert_eq!(d.iter().map(|e| e.register).collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn formats_decode_as_the_vertex_unit_reads_them() {
        assert_eq!(Attribute::Float(2).decode(&[0, 0, 0x80, 0x3f, 0, 0, 0, 0x40]), [1.0, 2.0, 0.0, 1.0]);
        assert_eq!(Attribute::PByte(2).decode(&[255, 0]), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(Attribute::Short(1).decode(&(-3i16).to_le_bytes()), [-3.0, 0.0, 0.0, 1.0]);
        let ns = [i16::MAX.to_le_bytes(), i16::MIN.to_le_bytes(), 0i16.to_le_bytes()].concat();
        assert_eq!(Attribute::NormShort(3).decode(&ns), [1.0, -1.0, 0.0, 1.0]);
        // x 1023, y -1023 (0x401), z 511.
        let w: u32 = 1023 | 0x401 << 11 | 511 << 22;
        assert_eq!(Attribute::NormPacked3.decode(&w.to_le_bytes()), [1.0, -1.0, 1.0, 1.0]);
        assert_eq!(Attribute::Colour.decode(&[0, 51, 255, 255]), [1.0, 0.2, 0.0, 1.0]);
        let sizes: Vec<usize> = (0..18).map(|c| Attribute::from_code(c).unwrap().size()).collect();
        assert_eq!(sizes, [4, 8, 12, 16, 1, 2, 3, 4, 2, 4, 6, 8, 2, 4, 6, 8, 4, 4]);
        assert_eq!(Attribute::from_code(18), None);
    }

    #[test]
    fn expansion_reads_each_register_from_its_stream() {
        let d = [
            Element { register: 0, stream: 0, offset: 0, format: Attribute::Short(2) },
            Element { register: 2, stream: 1, offset: 1, format: Attribute::PByte(1) },
        ];
        let a = [1i16, 2, 3, 4].map(i16::to_le_bytes).concat();
        let b = [9u8, 255, 9, 0];
        let streams = [Some(StreamData { bytes: &a, stride: 4 }), Some(StreamData { bytes: &b, stride: 2 })];
        let v = expand(&d, &streams, 2).unwrap();
        assert_eq!(v, vec![[1.0, 2.0, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0], [3.0, 4.0, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0]]);
        assert!(expand(&d, &streams, 3).unwrap_err().contains("past stream 0"));
        assert!(expand(&d, &[streams[0], None], 1).unwrap_err().contains("no data"));
        // Gathered: the same bytes, each in its slot, decoding as expanded.
        let g = gather(&d, &streams, 2).unwrap();
        assert_eq!(g[0][..4], [1, 0, 2, 0]);
        assert_eq!(g[0][4..], [0; 12]);
        assert_eq!(g[1][..2], [255, 0], "the PByte and the slot's zeros");
        let decoded: Vec<[f32; 4]> = g.iter().zip(d.iter().cycle()).map(|(s, e)| e.format.decode(s)).collect();
        assert_eq!(decoded, v);
        assert!(gather(&d, &streams, 3).unwrap_err().contains("past stream 0"));
    }

    #[test]
    fn the_classification_follows_the_sections_nodes() {
        assert_eq!(classification(0, 0, 8), Some(0));
        assert_eq!(classification(1, 4, 8), Some(1));
        assert_eq!(classification(2, 4, 8), Some(2));
        assert_eq!(classification(3, 3, 8), Some(5));
        assert_eq!(classification(3, 6, 8), Some(0), "past the tag's classifications");
        assert_eq!(classification(4, 0, 8), None);
    }
}
