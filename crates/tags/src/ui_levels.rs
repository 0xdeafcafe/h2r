//! The main menu's level lists, in the globals tag (`matg`): the
//! multiplayer maps the Select Map dialog lists and the campaign levels the
//! Select Level screen lists, each with its English name, description and
//! picture (a bitmap tag of the same map). One reader for everything that
//! shows them: the boot's map select (`boot::map_select`), the Live catalog
//! (`cli::live_catalog`) and the editor's Maps list.
//!
//! Layout (docs/game/map-pack.md 8.1, docs/live/live-admin.md "The
//! catalog"):
//!
//! - `matg +0x170`: the levels the game starts, one element of 8 bytes
//!   whose block at `+0` holds `0x108`-byte entries: `+0` campaign id, `+4`
//!   map id, `+8` the scenario path (docs/runtime/level-boot.md).
//! - `matg +0x178`: the UI level data, one element of `0x18` bytes:
//!   - its block at `+0x8`: the level select's campaign levels,
//!     `0xb50`-byte entries: `+0` campaign id, `+4` map id, `+8` the
//!     picture (tag reference, datum at `+0xc`), `+0x10` nine 32-unit
//!     UTF-16 names, `+0x250` nine 128-unit descriptions;
//!   - its block at `+0x10`: the multiplayer list, `0xc64`-byte slots: `+0`
//!     map id (`-1` for a free slot), `+4` the picture (tag reference,
//!     datum at `+0x8`), `+0xc` nine 32-unit UTF-16 names, `+0x24c` nine
//!     128-unit descriptions, `+0xb4c` the scenario path, `+0xc4c` the
//!     sort order.
//!
//! English is each name and description's first language. Offsets only:
//! no list, name or picture of the game's is kept here.

use std::io::{Read, Seek};

use map::Map;

use crate::{Error, Result};

/// `matg +0x170`: the levels the game starts.
pub const MATG_START_LEVELS: usize = 0x170;
const START_DATA_SIZE: usize = 8;
pub const START_LEVEL_SIZE: usize = 0x108;
const START_LEVEL_PATH: usize = 0x8;
/// `matg +0x178`: the UI level data.
pub const MATG_LEVELS: usize = 0x178;
pub const LEVEL_DATA_SIZE: usize = 0x18;
/// The UI level data's campaign levels and multiplayer list.
pub const LEVEL_DATA_CAMPAIGN: usize = 0x8;
pub const LEVEL_DATA_MP: usize = 0x10;

/// A level select entry's fields.
pub const CAMPAIGN_LEVEL_SIZE: usize = 0xb50;
pub const CAMPAIGN_PICTURE: usize = 0xc;
pub const CAMPAIGN_NAME: usize = 0x10;
pub const CAMPAIGN_DESC: usize = 0x250;

/// A multiplayer slot's fields.
pub const MP_LEVEL_SIZE: usize = 0xc64;
pub const MP_PICTURE: usize = 0x8;
pub const MP_NAME: usize = 0xc;
pub const MP_DESC: usize = 0x24c;
pub const MP_PATH: usize = 0xb4c;
pub const MP_SORT: usize = 0xc4c;

/// Text field sizes: a name's and a description's UTF-16 units (one
/// language), a scenario path's bytes.
pub const NAME_UNITS: usize = 32;
pub const DESC_UNITS: usize = 128;
pub const PATH_LEN: usize = 0x100;

/// More would not be a level list.
pub const MAX_LEVELS: u32 = 256;
const MAX_LEVEL_DATA: u32 = 16;

/// One slot of the multiplayer list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MpLevel {
    /// Its map id, what the lobby's map is set to.
    pub id: u32,
    /// English name and description.
    pub name: String,
    pub description: String,
    /// The scenario path; its last part is the map file's stem.
    pub path: String,
    /// The list's sort order.
    pub sort: i32,
    /// The picture's bitmap tag datum, when it names one.
    pub picture: Option<u32>,
}

impl MpLevel {
    /// The map file's stem: the scenario path's last part.
    pub fn stem(&self) -> Option<&str> {
        stem(&self.path)
    }
}

/// A campaign level the game can start, named as the level select names
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CampaignLevel {
    pub campaign: u32,
    pub map_id: u32,
    /// English name and description.
    pub name: String,
    pub description: String,
    /// The scenario path; its last part is the map file's stem.
    pub path: String,
    /// The picture's bitmap tag datum, when it names one.
    pub picture: Option<u32>,
}

impl CampaignLevel {
    /// The map file's stem: the scenario path's last part.
    pub fn stem(&self) -> Option<&str> {
        stem(&self.path)
    }
}

/// A scenario path's last part (`\` or `/` separated), when not empty.
pub fn stem(path: &str) -> Option<&str> {
    path.rsplit(['\\', '/']).next().filter(|k| !k.is_empty())
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// `units` UTF-16 units at `at`, up to the first NUL.
fn utf16(b: &[u8], at: usize, units: usize) -> String {
    let raw = b.get(at..at + units * 2).unwrap_or_default();
    let units: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|u| *u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// `len` bytes of text at `at`, up to the first NUL (all of them without
/// one).
fn text(b: &[u8], at: usize, len: usize) -> String {
    let raw = b.get(at..at + len).unwrap_or_default();
    let raw = &raw[..raw.iter().position(|c| *c == 0).unwrap_or(raw.len())];
    String::from_utf8_lossy(raw).into_owned()
}

fn picture(b: &[u8], at: usize) -> Option<u32> {
    le32(b, at).filter(|p| *p != u32::MAX)
}

/// A multiplayer slot's fields; `None` for a free slot (map id `-1`) or a
/// short one.
pub fn mp_level(slot: &[u8]) -> Option<MpLevel> {
    let slot = slot.get(..MP_LEVEL_SIZE)?;
    let id = le32(slot, 0)?;
    if id == u32::MAX {
        return None;
    }
    Some(MpLevel {
        id,
        name: utf16(slot, MP_NAME, NAME_UNITS),
        description: utf16(slot, MP_DESC, DESC_UNITS),
        path: text(slot, MP_PATH, PATH_LEN),
        sort: le32(slot, MP_SORT)? as i32,
        picture: picture(slot, MP_PICTURE),
    })
}

/// Reads tag memory: `(address, size)` to the bytes there, `None` outside
/// the map.
pub type Fetch<'a> = dyn FnMut(u32, u32) -> Option<Vec<u8>> + 'a;

/// A `(count, address)` tag block at `at` of `b`, its `size`-byte elements
/// read whole. Empty for a count of 0.
pub fn block(b: &[u8], at: usize, size: usize, max: u32, read: &mut Fetch<'_>) -> Result<Vec<u8>> {
    let (Some(n), Some(addr)) = (le32(b, at), le32(b, at + 4)) else {
        return Err(Error::Invalid("tag shorter than its block field"));
    };
    if n == 0 {
        return Ok(Vec::new());
    }
    if n > max {
        return Err(Error::Invalid("tag block count is not plausible"));
    }
    let len = n * size as u32;
    read(addr, len).ok_or(Error::Bounds {
        address: addr,
        size: len.into(),
    })
}

/// The UI level data's element 0.
fn level_data(matg: &[u8], read: &mut Fetch<'_>) -> Result<Vec<u8>> {
    let data = block(matg, MATG_LEVELS, LEVEL_DATA_SIZE, MAX_LEVEL_DATA, read)?;
    if data.is_empty() {
        return Err(Error::Invalid("the globals have no UI level data"));
    }
    Ok(data)
}

/// The multiplayer list's used slots, in slot order, from a globals tag's
/// bytes (`read` resolves its addresses). A slot with no name is kept: the
/// caller decides.
pub fn mp_levels(matg: &[u8], read: &mut Fetch<'_>) -> Result<Vec<MpLevel>> {
    let data = level_data(matg, read)?;
    let all = block(&data, LEVEL_DATA_MP, MP_LEVEL_SIZE, MAX_LEVELS, read)?;
    Ok(all
        .as_chunks::<MP_LEVEL_SIZE>()
        .0
        .iter()
        .filter_map(|s| mp_level(s))
        .collect())
}

/// The campaign levels from a globals tag's bytes, in the order the game
/// lists the levels it can start, each named by the level select's entry
/// with the same campaign and map id. A level the level select does not
/// name (or names with an empty name), or with no scenario path, is left
/// out. No start list or no level data is an error, not an empty list.
pub fn campaign_levels(matg: &[u8], read: &mut Fetch<'_>) -> Result<Vec<CampaignLevel>> {
    let start = block(
        matg,
        MATG_START_LEVELS,
        START_DATA_SIZE,
        MAX_LEVEL_DATA,
        read,
    )?;
    if start.is_empty() {
        return Err(Error::Invalid("the globals have no level list"));
    }
    let paths = block(&start, 0, START_LEVEL_SIZE, MAX_LEVELS, read)?;
    let data = level_data(matg, read)?;
    let select = block(
        &data,
        LEVEL_DATA_CAMPAIGN,
        CAMPAIGN_LEVEL_SIZE,
        MAX_LEVELS,
        read,
    )?;
    let select: Vec<&[u8]> = select
        .as_chunks::<CAMPAIGN_LEVEL_SIZE>()
        .0
        .iter()
        .map(|e| e.as_slice())
        .collect();
    Ok(paths
        .as_chunks::<START_LEVEL_SIZE>()
        .0
        .iter()
        .filter_map(|e| {
            let (campaign, map_id) = (le32(e, 0)?, le32(e, 4)?);
            let raw = &e[START_LEVEL_PATH..];
            // A path with no NUL in its field is not one.
            raw.iter().position(|c| *c == 0)?;
            let path = text(e, START_LEVEL_PATH, START_LEVEL_SIZE - START_LEVEL_PATH);
            stem(&path)?;
            let u = select
                .iter()
                .find(|u| le32(u, 0) == Some(campaign) && le32(u, 4) == Some(map_id))?;
            let name = utf16(u, CAMPAIGN_NAME, NAME_UNITS);
            if name.is_empty() {
                return None;
            }
            Some(CampaignLevel {
                campaign,
                map_id,
                name,
                description: utf16(u, CAMPAIGN_DESC, DESC_UNITS),
                path,
                picture: picture(u, CAMPAIGN_PICTURE),
            })
        })
        .collect())
}

/// `map`'s globals tag bytes.
pub fn globals<R: Read + Seek>(map: &mut Map<R>) -> Result<Vec<u8>> {
    let id = map
        .globals()
        .ok_or(Error::Invalid("the map has no globals tag"))?
        .id;
    Ok(map.read_tag_bytes(id)?)
}

/// A [`Fetch`] over `map`'s tag memory.
pub fn fetch<R: Read + Seek>(map: &mut Map<R>) -> impl FnMut(u32, u32) -> Option<Vec<u8>> + '_ {
    move |addr, size| {
        let at = map.file_offset(addr, size).ok()?;
        map.read_file_range(at, size.into()).ok()
    }
}

/// [`mp_levels`] of `map` (the main menu map).
pub fn read_mp<R: Read + Seek>(map: &mut Map<R>) -> Result<Vec<MpLevel>> {
    let matg = globals(map)?;
    mp_levels(&matg, &mut fetch(map))
}

/// [`campaign_levels`] of `map` (the main menu map).
pub fn read_campaign<R: Read + Seek>(map: &mut Map<R>) -> Result<Vec<CampaignLevel>> {
    let matg = globals(map)?;
    campaign_levels(&matg, &mut fetch(map))
}

#[cfg(test)]
#[path = "ui_levels_tests.rs"]
mod tests;
