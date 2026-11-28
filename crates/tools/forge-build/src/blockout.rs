//! Block-out pieces materialised (docs/game/forge-blockout.md,
//! "Materialising" and "The cut order").
//!
//! [`definitions`] collects the definitions of a document's live pieces,
//! once each, in name order. [`install`] adds their tags to the base map
//! (`content_kit::blockout::install`), one crate palette row a definition after the
//! map's own rows, and with the kit also the dev set and the starter set
//! (`crate::kit::starter`) after them. The scenario editor then opens the
//! map it gives, and each piece's placement takes its definition's row
//! (`Materialiser::pieces`).
//!
//! What does not fit the map's tag memory is cut in one order, alike on
//! every machine: the starter set from the last, then the kit's own dev
//! set; then the first definition the document needs that does not fit is
//! refused by name with its [`Cost`] and the headroom.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Cursor;

use content_kit::blockout::{Cost, Facts, cost_since, dev};
use forge_doc::{BlockKind, Pieces};
use forge_shapes::{Material, PieceDef, Role};
use map::build::{Block, TagPayload, TagSet};
use map::scenario_layout::{self as layout, PaletteDef};
use map::writer::max_metadata_size;
use map::{CacheType, FourCC, Map, TagId};

use crate::{Error, kit};

/// The crate palette (`scnr +0x330`, rows of `0x28`).
fn crate_palette() -> &'static PaletteDef {
    layout::block(BlockKind::Crates)
        .palette
        .as_ref()
        .expect("crates have a palette")
}

/// The bytes of a crate palette of `old` rows and `added` more, written
/// whole as a new block; nothing when nothing is added.
pub fn palette_bytes(old: u32, added: usize) -> u64 {
    if added == 0 {
        0
    } else {
        (u64::from(old) + added as u64) * u64::from(crate_palette().stride)
    }
}

/// The definitions of the active pieces, once each, in name order.
pub fn definitions(pieces: &Pieces) -> Vec<PieceDef> {
    let mut by_name = BTreeMap::new();
    for piece in &pieces.active {
        by_name
            .entry(piece.def.name())
            .or_insert_with(|| piece.def.clone());
    }
    by_name.into_values().collect()
}

/// The tag memory `base` has left: `max_metadata_size(Multiplayer)` less
/// its header's `+0x1c`.
pub fn headroom(base: &[u8]) -> Result<u64, Error> {
    let map = Map::from_reader(Cursor::new(base))?;
    Ok(u64::from(
        max_metadata_size(CacheType::Multiplayer)?.saturating_sub(map.header().meta_size),
    ))
}

/// One installed definition and its crate palette row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// `PieceDef::name`.
    pub name: String,
    /// The row in the crate palette.
    pub row: u32,
    /// What its install added.
    pub cost: Cost,
    /// Of the kit's starter set, not the document's.
    pub starter: bool,
}

/// What [`install`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The document's definitions in name order, then the starter set's.
    pub rows: Vec<Row>,
    /// The crate palette rows the base map has.
    pub base_rows: u32,
    /// The kit's dev set, installed before any definition; zero when the
    /// kit did not install it on its own (no kit, or it was cut).
    pub dev_set: Cost,
    /// The starter set's definitions cut to fit, last first.
    pub cut: Vec<String>,
    /// The kit's own dev set was cut to fit.
    pub dev_set_cut: bool,
    /// The tag memory of everything added, the palette block included.
    pub tag_memory: u64,
    /// The headroom it was fitted to.
    pub headroom: u64,
    /// Why the kit carries no block-out set: the base map has nothing the
    /// pieces draw with (`Facts::read`'s sentence). Only a kit build with
    /// no piece is built so; a document with a piece is refused.
    pub unavailable: Option<String>,
}

impl Report {
    /// Each definition's palette row, by name.
    pub fn palette_rows(&self) -> BTreeMap<String, u32> {
        self.rows.iter().map(|r| (r.name.clone(), r.row)).collect()
    }
}

/// The map [`install`] gives: `None` when nothing was added.
pub struct Materialised {
    pub bytes: Option<Vec<u8>>,
    pub report: Report,
}

/// A definition the document needs that does not fit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub name: String,
    /// What its install adds, after the definitions before it.
    pub cost: Cost,
    /// The tag memory left for it after the definitions before it and the
    /// palette.
    pub left: u64,
    pub headroom: u64,
    /// Its cost holds the dev set, which its `Dev` faces need.
    pub dev: bool,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} does not fit the map's tag memory: it takes {} B of tag memory ({} tags, {} B raw)",
            self.name, self.cost.tag_memory, self.cost.tags, self.cost.raw
        )?;
        if self.dev {
            f.write_str(", the dev set its Dev material needs included")?;
        }
        write!(
            f,
            ", and {} B of the map's {} B headroom are left for it",
            self.left, self.headroom
        )
    }
}

/// A set with `defs` installed in order, after the dev set when `dev`.
struct Trial {
    set: TagSet,
    dev: Cost,
    /// Each definition's cost and whether its install added the dev set.
    costs: Vec<(Cost, bool)>,
    ids: Vec<TagId>,
}

fn has_dev_set(set: &TagSet) -> bool {
    set.find(FourCC(*b"shad"), &dev::font_path()).is_some()
}

fn trial(pristine: &TagSet, facts: &Facts, dev: bool, defs: &[PieceDef]) -> Result<Trial, Error> {
    let mut set = pristine.clone();
    let dev_cost = if dev {
        let (slots, raw) = (set.slots.len(), set.raw.len());
        dev::install(&mut set, facts).map_err(|e| Error::Piece(format!("the dev set: {e:#}")))?;
        cost_since(&set, slots, raw)
    } else {
        Cost::default()
    };
    let mut costs = Vec::with_capacity(defs.len());
    let mut ids = Vec::with_capacity(defs.len());
    for def in defs {
        let had = has_dev_set(&set);
        let installed = content_kit::blockout::install(&mut set, def, facts)
            .map_err(|e| Error::Piece(format!("{e:#}")))?;
        costs.push((installed.cost, !had && has_dev_set(&set)));
        ids.push(installed.crate_tag);
    }
    Ok(Trial {
        set,
        dev: dev_cost,
        costs,
        ids,
    })
}

/// Refuses a `Map` material whose shader the base map lacks, by name.
fn check_shaders(set: &TagSet, def: &PieceDef) -> Result<(), Error> {
    for role in Role::ALL {
        if let Material::Map(key) = def.materials.get(role) {
            let group: [u8; 4] = key.group.as_bytes().try_into().map_err(|_| {
                Error::Piece(format!(
                    "{} names a tag of group {:?}",
                    def.name(),
                    key.group
                ))
            })?;
            if set.find(FourCC(group), &key.path).is_none() {
                return Err(Error::Piece(format!(
                    "{} names the shader {}, which the base map lacks",
                    def.name(),
                    key.path
                )));
            }
        }
    }
    Ok(())
}

/// The crate palette's count and rows in the base map.
fn base_palette(map: &mut Map<Cursor<&[u8]>>) -> Result<(u32, Vec<u8>), Error> {
    let palette = crate_palette();
    let scenario = map
        .scenario()
        .ok_or_else(|| Error::Piece("missing scenario".into()))?
        .id;
    let root = map.read_tag_bytes(scenario)?;
    let at = palette.offset as usize;
    let pair = root.get(at..at + 8).ok_or_else(|| {
        Error::Piece("the scenario root is too short for the crate palette".into())
    })?;
    let (count, address) = (kit::word(&pair[..4]), kit::word(&pair[4..]));
    let rows = kit::read_entries(map, address, count * palette.stride)?;
    Ok((count, rows))
}

/// Writes the crate palette, the base rows and then one a crate in `ids`,
/// as a new block, and points the scenario at it.
fn write_palette(set: &mut TagSet, base_rows: Vec<u8>, ids: &[TagId]) -> Result<(), Error> {
    let palette = crate_palette();
    let stride = palette.stride as usize;
    let start = base_rows.len();
    let count = u32::try_from(start / stride + ids.len())
        .map_err(|_| Error::Piece("too many crate palette rows".into()))?;
    let mut block = Block::new(base_rows);
    block.bytes.resize(start + ids.len() * stride, 0);
    for (i, id) in ids.iter().enumerate() {
        block.tag_ref((start + i * stride) as u32, *id);
    }
    let placed = set.append_pinned_blocks(&[block])?;
    let address = placed
        .first()
        .map(|&(address, _)| address)
        .ok_or_else(|| Error::Piece("the crate palette was not placed".into()))?;
    let scenario = set.scenario;
    let Some(TagPayload::Pinned { bytes, .. }) = set.tag_mut(scenario).map(|t| &mut t.payload)
    else {
        return Err(Error::Piece("the scenario is not pinned".into()));
    };
    let at = palette.offset as usize;
    let pair = bytes.get_mut(at..at + 8).ok_or_else(|| {
        Error::Piece("the scenario root is too short for the crate palette".into())
    })?;
    pair[..4].copy_from_slice(&count.to_le_bytes());
    pair[4..].copy_from_slice(&address.to_le_bytes());
    Ok(())
}

fn sum(costs: &[(Cost, bool)]) -> u64 {
    costs.iter().map(|(c, _)| c.tag_memory).sum()
}

/// Installs `needed` (the document's definitions, [`definitions`]) on
/// `base`, and with `kit` the dev set and the starter set, within
/// `headroom` bytes of tag memory ([`headroom`]), cutting in the order of
/// docs/game/forge-blockout.md, "The cut order". Refused, by name, when a
/// `Map` material names a shader the base lacks, when a definition does
/// not install, or when one the document needs does not fit
/// ([`Refused`]).
pub fn install(
    base: &[u8],
    needed: &[PieceDef],
    kit: bool,
    headroom: u64,
) -> Result<Materialised, Error> {
    let mut report = Report {
        headroom,
        ..Report::default()
    };
    if needed.is_empty() && !kit {
        return Ok(Materialised {
            bytes: None,
            report,
        });
    }
    let mut map = Map::from_reader(Cursor::new(base))?;
    let facts = match Facts::read(&mut map) {
        Ok(facts) => facts,
        Err(e) if needed.is_empty() => {
            report.unavailable = Some(format!("{e:#}"));
            return Ok(Materialised {
                bytes: None,
                report,
            });
        }
        Err(e) => {
            return Err(Error::Piece(format!(
                "the base map cannot carry block-out pieces: {e:#}"
            )));
        }
    };
    let (base_count, base_rows) = base_palette(&mut map)?;
    report.base_rows = base_count;
    let mut pristine = TagSet::from_map(&mut map)?;
    content_kit::blockout::make_index_room(&mut pristine);
    for def in needed {
        check_shaders(&pristine, def)?;
    }
    let names: BTreeSet<String> = needed.iter().map(PieceDef::name).collect();
    let starter: Vec<PieceDef> = if kit {
        kit::starter()
            .into_iter()
            .filter(|d| !names.contains(&d.name()))
            .collect()
    } else {
        Vec::new()
    };
    let n = needed.len();
    let fits = |tags: u64, rows: usize| tags + palette_bytes(base_count, rows) <= headroom;

    // Everything; then the starter set from the last.
    let mut all = needed.to_vec();
    all.extend(starter.iter().cloned());
    let full = trial(&pristine, &facts, kit, &all)?;
    let mut keep = starter.len();
    let mut total = full.dev.tag_memory + sum(&full.costs);
    while keep > 0 && !fits(total, n + keep) {
        keep -= 1;
        total -= full.costs[n + keep].0.tag_memory;
        report.cut.push(starter[keep].name());
    }
    let chosen = if fits(total, n + keep) {
        report.dev_set = full.dev;
        if keep == starter.len() {
            full
        } else {
            trial(&pristine, &facts, kit, &all[..n + keep])?
        }
    } else {
        // The kit's own dev set, last of all.
        let lean = if kit {
            trial(&pristine, &facts, false, needed)?
        } else {
            full
        };
        if !fits(sum(&lean.costs), n) {
            let mut used = 0;
            for (i, &(cost, dev)) in lean.costs.iter().enumerate() {
                let before = used + palette_bytes(base_count, i + 1);
                if before + cost.tag_memory > headroom {
                    return Err(Error::PieceCost(Box::new(Refused {
                        name: needed[i].name(),
                        cost,
                        left: headroom.saturating_sub(before),
                        headroom,
                        dev,
                    })));
                }
                used += cost.tag_memory;
            }
            return Err(Error::Piece(
                "the definitions do not fit the map's tag memory".into(),
            ));
        }
        report.dev_set_cut = kit && !lean.costs.iter().any(|&(_, dev)| dev);
        lean
    };

    let Trial {
        mut set,
        dev: _,
        costs,
        ids,
    } = chosen;
    for (i, (def, &(cost, _))) in all.iter().zip(&costs).enumerate() {
        report.rows.push(Row {
            name: def.name(),
            row: base_count + i as u32,
            cost,
            starter: i >= n,
        });
    }
    if ids.is_empty() {
        if report.dev_set == Cost::default() {
            // Everything was cut: the map is the base.
            return Ok(Materialised {
                bytes: None,
                report,
            });
        }
    } else {
        write_palette(&mut set, base_rows, &ids)?;
    }
    report.tag_memory =
        report.dev_set.tag_memory + sum(&costs) + palette_bytes(base_count, ids.len());
    let (bytes, _) = set.build()?;
    Ok(Materialised {
        bytes: Some(bytes),
        report,
    })
}
