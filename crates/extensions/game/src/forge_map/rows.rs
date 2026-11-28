//! The loaded map's placement rows, read from the running scenario
//! (docs/game/forge-map-variants.md, "Things" and "Naming an entry").
//!
//! [`read`] walks every block `map::scenario_layout` names: the block's
//! `(count, address)` pair in the loaded scenario root, then each row's
//! bytes. A row is kept three ways: its raw bytes, so an undo or a
//! `Reset` can put it back exactly; its typed fields as `forge_doc::Value`s
//! in the layout's order; and the [`Print`] of those fields, the digest an
//! entry op carries to name the entry it was made on.
//!
//! A tag reference enters as its `TagKey` and a palette index as the key of
//! the palette entry it selects, so a print holds no salted ID. The guest
//! has no tag names, so the caller gives a resolver from a salted datum to
//! its key; the resolver is the map file's names, as the Objects palette is
//! read. A block that does not read (an address that faults, a count past
//! the engine's limit, a value of the wrong kind, a tag the resolver does
//! not know) is left out by name in [`MapRows::left_out`], and every other
//! block is kept. Nothing here is written to the document or the network.
use std::collections::BTreeMap;

use guest_layout::GuestMemory;
use forge::BlockKind;
use forge_doc::{F32, Print, TagKey, Value};
use map::scenario_layout::{self as layout, BlockDef, FieldDef, FieldType, Role};

/// The datum of a null tag reference.
const NULL_DATUM: u32 = u32::MAX;
/// The most rows of a block read when the layout names no limit: a label
/// or an entry op names an entry by an index of at most `i16::MAX`.
const ROW_LIMIT: u32 = i16::MAX as u32 + 1;

/// A tag's key from the salted datum a scenario row holds.
pub(crate) type Names<'a> = &'a dyn Fn(u32) -> Option<TagKey>;

/// One entry of a block as the loaded scenario holds it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    /// The entry's index in the block, as the disc has it.
    pub index: u32,
    /// The entry's bytes, the layout's stride of them.
    pub raw: Vec<u8>,
    /// One value for each of the layout's fields, in its order.
    pub fields: Vec<Value>,
    /// The digest of `fields` with the block's name.
    pub print: Print,
}

/// One block's rows and where the loaded scenario keeps them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BlockRows {
    /// The guest address of the first row.
    pub address: u32,
    pub stride: u32,
    pub rows: Vec<Row>,
}

/// Every block that read.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MapRows {
    pub blocks: BTreeMap<BlockKind, BlockRows>,
    /// Each block that did not read, as `block: reason`.
    pub left_out: Vec<String>,
}

/// What a thing is to a player: the session's own kinds
/// (docs/game/forge-map-variants.md, "Things").
pub(crate) use forge::ThingKind;

/// One thing of the loaded map: an entry of a block, named by its index
/// and its print, with the layout's fields by name.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MapThing {
    pub block: BlockKind,
    pub index: u32,
    pub kind: ThingKind,
    pub fields: Vec<(&'static str, Value)>,
    pub print: Print,
}

impl MapRows {
    /// Every thing of every block that read, in the layout's block order
    /// and then the block's index order.
    pub(crate) fn things(&self) -> Vec<MapThing> {
        let mut out = Vec::new();
        for def in layout::BLOCKS {
            let Some(block) = self.blocks.get(&def.kind) else {
                continue;
            };
            for row in &block.rows {
                let Some(kind) = ThingKind::of(def.kind, &row.fields) else {
                    break;
                };
                out.push(MapThing {
                    block: def.kind,
                    index: row.index,
                    kind,
                    fields: def
                        .fields
                        .iter()
                        .map(|f| f.name)
                        .zip(row.fields.iter().cloned())
                        .collect(),
                    print: row.print,
                });
            }
        }
        out
    }

    /// The row at `index` of `block`, when that block read.
    pub(crate) fn row(&self, block: BlockKind, index: u32) -> Option<&Row> {
        self.blocks.get(&block)?.rows.get(index as usize)
    }

    /// The rows as the session keeps them (`forge::Session::with_base`).
    pub(crate) fn into_base(self) -> forge::BaseRows {
        forge::BaseRows {
            rows: self
                .blocks
                .into_iter()
                .flat_map(|(block, rows)| {
                    rows.rows.into_iter().map(move |row| forge::BaseRow {
                        block,
                        index: row.index,
                        raw: row.raw,
                        fields: row.fields,
                        print: row.print,
                    })
                })
                .collect(),
            left_out: self.left_out,
        }
    }
}

/// Read every block of the scenario at `scenario` (the loaded scenario's
/// root address). `names` gives a tag's key from the datum a row holds.
pub(crate) fn read(m: &dyn GuestMemory, scenario: u32, names: Names<'_>) -> MapRows {
    let mut out = MapRows::default();
    for def in layout::BLOCKS {
        match read_block(m, scenario, def, names) {
            Ok(rows) => {
                out.blocks.insert(def.kind, rows);
            }
            Err(reason) => out.left_out.push(format!("{}: {reason}", def.kind.name())),
        }
    }
    out
}

fn fault(e: guest_layout::MemFault) -> String {
    format!("guest memory at {:#x} does not read", e.addr)
}

fn read_block(
    m: &dyn GuestMemory,
    scenario: u32,
    def: &BlockDef,
    names: Names<'_>,
) -> Result<BlockRows, String> {
    let pair = scenario
        .checked_add(def.offset)
        .ok_or("the scenario address is out of range")?;
    let count = m.u32(pair).map_err(fault)?;
    let address = m.u32(pair + 4).map_err(fault)?;
    let limit = def.max_count.map_or(ROW_LIMIT, |n| n as u32);
    if count > limit {
        return Err(format!(
            "it holds {count} rows, more than the {limit} the engine takes"
        ));
    }
    let palette = palette(m, scenario, def, names)?;
    let mut rows = Vec::with_capacity(count as usize);
    if count > 0 {
        let len = count
            .checked_mul(def.stride)
            .filter(|len| address.checked_add(*len).is_some() && address != 0)
            .ok_or("its address is not a block")?;
        let bytes = m.bytes(address, len as usize).map_err(fault)?;
        for (index, raw) in bytes.chunks_exact(def.stride as usize).enumerate() {
            let fields = def
                .fields
                .iter()
                .map(|field| value(raw, field, &palette, names))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|why| format!("row {index}: {why}"))?;
            let print =
                forge_doc::print(def.kind, &fields).map_err(|e| format!("row {index}: {e}"))?;
            rows.push(Row {
                index: index as u32,
                raw: raw.to_vec(),
                fields,
                print,
            });
        }
    }
    Ok(BlockRows {
        address,
        stride: def.stride,
        rows,
    })
}

/// The block's palette as keys, one for each entry; a null entry is none.
fn palette(
    m: &dyn GuestMemory,
    scenario: u32,
    def: &BlockDef,
    names: Names<'_>,
) -> Result<Vec<Option<TagKey>>, String> {
    let Some(def) = &def.palette else {
        return Ok(Vec::new());
    };
    let pair = scenario
        .checked_add(def.offset)
        .ok_or("the scenario address is out of range")?;
    let count = m.u32(pair).map_err(fault)?;
    let address = m.u32(pair + 4).map_err(fault)?;
    if count > ROW_LIMIT {
        return Err(format!("its palette holds {count} entries"));
    }
    (0..count)
        .map(|i| -> Result<Option<TagKey>, String> {
            let at = address
                .checked_add(i * def.stride)
                .ok_or("its palette address is not a block")?;
            let datum = m.u32(at + 4).map_err(fault)?;
            if datum == NULL_DATUM {
                return Ok(None);
            }
            names(datum).map(Some).ok_or_else(|| {
                format!("palette entry {i} names tag {datum:#010x}, which the map does not hold")
            })
        })
        .collect()
}

/// One field of a row as a typed value.
fn value(
    raw: &[u8],
    field: &FieldDef,
    palette: &[Option<TagKey>],
    names: Names<'_>,
) -> Result<Value, String> {
    let at = field.offset as usize;
    let bytes = raw
        .get(at..at + field.ty.size() as usize)
        .ok_or_else(|| format!("{} lies outside the row", field.name))?;
    let u16_at = |b: &[u8]| u16::from_le_bytes([b[0], b[1]]);
    let u32_at = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let float = |b: &[u8]| {
        F32::new(f32::from_bits(u32_at(b)))
            .map_err(|_| format!("{} is not a finite number", field.name))
    };
    if field.role == Role::PaletteIndex {
        let index = i16::from_le_bytes([bytes[0], bytes[1]]);
        return match usize::try_from(index) {
            Err(_) => Ok(Value::Tag(None)),
            Ok(i) => palette
                .get(i)
                .cloned()
                .map(Value::Tag)
                .ok_or_else(|| format!("{} {index} is outside the palette", field.name)),
        };
    }
    Ok(match field.ty {
        FieldType::U8 => Value::Int(i64::from(bytes[0])),
        FieldType::I16 => Value::Int(i64::from(u16_at(bytes) as i16)),
        FieldType::U16 => Value::Int(i64::from(u16_at(bytes))),
        FieldType::U32 => Value::Int(i64::from(u32_at(bytes))),
        FieldType::F32 => Value::Float(float(bytes)?),
        FieldType::Vec3 => Value::Vec3([
            float(&bytes[0..4])?,
            float(&bytes[4..8])?,
            float(&bytes[8..12])?,
        ]),
        FieldType::TagRef => {
            let datum = u32_at(&bytes[4..8]);
            if datum == NULL_DATUM {
                Value::Tag(None)
            } else {
                Value::Tag(Some(names(datum).ok_or_else(|| {
                    format!(
                        "{} names tag {datum:#010x}, which the map does not hold",
                        field.name
                    )
                })?))
            }
        }
    })
}
