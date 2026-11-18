//! Halo 1's compiled script tables in the scenario tag (`scnr`).
//!
//! | scnr field | Layout |
//! |---|---|
//! | `+0x474` script syntax data | data reference: a `script node` data array |
//! | `+0x488` script string data | data reference: NUL-terminated tokens |
//! | `+0x49c` scripts | block, `0x5c` bytes each |
//! | `+0x4a8` globals | block, `0x5c` bytes each |
//! | `+0x4b4` references | block, `0x28` bytes each |
//!
//! A data reference is 20 bytes: size `+0`, address `+0xc`. The syntax data
//! is a data array header (`0x38` bytes) followed by every slot up to its
//! maximum count; each slot is a 20-byte node with the same wire layout as
//! Halo 2's ([`hs::Node`]). Evidence is in `docs/halo1/scripts.md`.

use h1map::{Block, H1Map, Space, TagRef};
use hs::{Datum, Node};

use crate::{Result, invalid, u16_at, u32_at};

/// The script syntax data reference.
pub const SCNR_SYNTAX_DATA: usize = 0x474;
/// The script string data reference.
pub const SCNR_STRING_DATA: usize = 0x488;
/// The scripts block.
pub const SCNR_SCRIPTS: usize = 0x49c;
/// The globals block.
pub const SCNR_GLOBALS: usize = 0x4a8;
/// The references block.
pub const SCNR_REFERENCES: usize = 0x4b4;
/// Bytes of scenario the tables need.
pub const SCNR_MIN_LEN: usize = SCNR_REFERENCES + h1map::BLOCK_LEN;
/// Bytes per script.
pub const SCRIPT_LEN: usize = 0x5c;
/// Bytes per global.
pub const GLOBAL_LEN: usize = 0x5c;
/// Bytes per reference.
pub const REFERENCE_LEN: usize = 0x28;
/// The data array header before the first node.
pub const SYNTAX_HEADER_LEN: usize = 0x38;
/// `d@t@`, the data array signature at header `+0x28`.
pub const DATA_SIGNATURE: u32 = u32::from_be_bytes(*b"d@t@");
/// Most nodes accepted (host limit; retail arrays hold 19,001).
pub const MAX_NODES: usize = 0x10000;

/// One script: a 32-byte name, type `+0x20`, return type `+0x22`, root
/// node `+0x24`; the rest is padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    /// Name.
    pub name: String,
    /// Script type (startup, dormant, continuous, static, stub).
    pub kind: u16,
    /// Return type.
    pub return_type: u16,
    /// Root expression.
    pub root: Datum,
}

/// One global: a 32-byte name, type `+0x20`, initial value root `+0x28`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Global {
    /// Name.
    pub name: String,
    /// Value type.
    pub value_type: u16,
    /// Initial value expression.
    pub root: Datum,
}

/// The data array header of the syntax data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxHeader {
    /// Slots stored (`+0x20`).
    pub capacity: u16,
    /// Slots in use up to the highest live one (`+0x2e`).
    pub count: u16,
}

/// Every script table of one scenario.
#[derive(Debug, Clone)]
pub struct Tables {
    /// The syntax data header.
    pub header: SyntaxHeader,
    /// Every node slot; salt zero is free.
    pub nodes: Vec<Node>,
    /// The string data.
    pub strings: Vec<u8>,
    /// Scripts.
    pub scripts: Vec<Script>,
    /// Globals.
    pub globals: Vec<Global>,
    /// Tag references (`+0x18` of each element).
    pub references: Vec<TagRef>,
}

impl Tables {
    /// The scenario's tables.
    pub fn read(map: &H1Map) -> Result<Self> {
        let scnr = map.tag_data(map.scenario())?;
        Self::decode(scnr, &map.tag_space())
    }

    /// The tables of the scenario `scnr`, whose blocks point into `space`.
    pub fn decode(scnr: &[u8], space: &Space) -> Result<Self> {
        if scnr.len() < SCNR_MIN_LEN {
            return Err(invalid("scenario too short for script tables"));
        }
        let data = |at: usize| -> Result<&[u8]> {
            let size = u32_at(scnr, at).unwrap_or(0) as usize;
            if size == 0 {
                return Ok(&[]);
            }
            Ok(space.at(u32_at(scnr, at + 0xc).unwrap_or(0), size)?)
        };
        let syntax = data(SCNR_SYNTAX_DATA)?;
        let strings = data(SCNR_STRING_DATA)?.to_vec();
        let (header, nodes) = if syntax.is_empty() {
            (
                SyntaxHeader {
                    capacity: 0,
                    count: 0,
                },
                Vec::new(),
            )
        } else {
            decode_syntax(syntax)?
        };
        let block = |at: usize, len: usize| -> Result<Vec<&[u8]>> {
            let b = Block::read(scnr, at).ok_or_else(|| invalid("block field"))?;
            Ok(space.block(b, len)?.chunks_exact(len).collect())
        };
        let scripts = block(SCNR_SCRIPTS, SCRIPT_LEN)?
            .into_iter()
            .map(|b| {
                Ok(Script {
                    name: name(b)?,
                    kind: u16_at(b, 0x20).unwrap_or(0),
                    return_type: u16_at(b, 0x22).unwrap_or(0),
                    root: Datum(u32_at(b, 0x24).unwrap_or(0)),
                })
            })
            .collect::<Result<_>>()?;
        let globals = block(SCNR_GLOBALS, GLOBAL_LEN)?
            .into_iter()
            .map(|b| {
                Ok(Global {
                    name: name(b)?,
                    value_type: u16_at(b, 0x20).unwrap_or(0),
                    root: Datum(u32_at(b, 0x28).unwrap_or(0)),
                })
            })
            .collect::<Result<_>>()?;
        let references = block(SCNR_REFERENCES, REFERENCE_LEN)?
            .into_iter()
            .map(|b| TagRef::read(b, 0x18).ok_or_else(|| invalid("reference")))
            .collect::<Result<_>>()?;
        Ok(Self {
            header,
            nodes,
            strings,
            scripts,
            globals,
            references,
        })
    }

    /// The live node `datum` names; its salt must match.
    pub fn node(&self, datum: Datum) -> Result<&Node> {
        let n = self
            .nodes
            .get(datum.slot())
            .filter(|n| n.live() && n.salt() == datum.salt())
            .ok_or_else(|| invalid(format!("dead or stale node {:#010x}", datum.0)))?;
        Ok(n)
    }

    /// The token at `offset` in the string data, as UTF-8.
    pub fn string(&self, offset: u32) -> Result<&str> {
        std::str::from_utf8(self.string_bytes(offset)?).map_err(|_| invalid("non-UTF-8 token"))
    }

    /// The bytes of the token at `offset`, without its NUL. String literals
    /// may hold bytes that are not UTF-8.
    pub fn string_bytes(&self, offset: u32) -> Result<&[u8]> {
        let tail = self
            .strings
            .get(offset as usize..)
            .ok_or_else(|| invalid(format!("string offset {offset:#x} out of range")))?;
        let end = tail
            .iter()
            .position(|&c| c == 0)
            .ok_or_else(|| invalid("unterminated string"))?;
        Ok(&tail[..end])
    }

    /// The tokens of every live literal atom whose source type is `ty`.
    pub fn atom_tokens(&self, ty: u16) -> std::collections::BTreeSet<String> {
        self.nodes
            .iter()
            .filter(|n| n.live() && n.atom() && !n.global() && n.opcode() == ty)
            .filter_map(|n| self.string(n.string_offset()).ok().map(str::to_owned))
            .collect()
    }

    /// Every definition root: globals, then scripts.
    pub fn roots(&self) -> impl Iterator<Item = Datum> + '_ {
        self.globals
            .iter()
            .map(|g| g.root)
            .chain(self.scripts.iter().map(|s| s.root))
    }
}

fn decode_syntax(b: &[u8]) -> Result<(SyntaxHeader, Vec<Node>)> {
    if b.len() < SYNTAX_HEADER_LEN || u32_at(b, 0x28) != Some(DATA_SIGNATURE) {
        return Err(invalid("syntax data is not a data array"));
    }
    let capacity = u16_at(b, 0x20).unwrap_or(0);
    let count = u16_at(b, 0x2e).unwrap_or(0);
    if u16_at(b, 0x22) != Some(Node::SIZE as u16) {
        return Err(invalid("syntax node size is not 20"));
    }
    let body = &b[SYNTAX_HEADER_LEN..];
    if count > capacity || body.len() < capacity as usize * Node::SIZE {
        return Err(invalid("syntax data shorter than its capacity"));
    }
    if capacity as usize > MAX_NODES {
        return Err(invalid("too many syntax nodes"));
    }
    let nodes = body
        .as_chunks::<{ Node::SIZE }>()
        .0
        .iter()
        .take(capacity as usize)
        .map(|b| Node::decode(b))
        .collect::<hs::Result<_>>()?;
    Ok((SyntaxHeader { capacity, count }, nodes))
}

fn name(b: &[u8]) -> Result<String> {
    let raw = &b[..32];
    let end = raw.iter().position(|&c| c == 0).unwrap_or(32);
    String::from_utf8(raw[..end].to_vec()).map_err(|_| invalid("non-UTF-8 name"))
}
