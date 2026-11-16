//! The playlist builder's catalog: what the owner's own game ships that a
//! hopper can point at, written as `catalog.json` beside the art manifest by
//! `h2 live assets extract` (docs/live/live-admin.md, "The catalog"). Nothing of
//! it is in the repo; tests build synthetic tag memory.
//!
//! Three lists, each read from the main menu map, each left out (and named
//! in `skipped`) when it cannot be read:
//!
//! - **maps**: the globals tag's multiplayer level list (`matg +0x178`,
//!   element 0, block at `+0x10`, `0xc64`-byte entries). The game looks a
//!   hopper record's id up here (`0x0019c5f0`, compares the entry's first
//!   dword), so these ids are exactly the record ids that resolve. Then the
//!   campaign levels, `kind: "campaign"`, keyed by map file stem: the
//!   level list the game starts levels from (`matg +0x170`, element 0,
//!   `0x108`-byte entries: campaign id, map id, scenario path;
//!   docs/runtime/level-boot.md) joined on (campaign id, map id) with the level
//!   select's entries (`matg +0x178`, element 0, block at `+0x8`,
//!   `0xb50`-byte entries: campaign id, map id, picture, names,
//!   descriptions).
//! - **playlists**: the strings of the `unic` tag the hopper file's
//!   descriptor names (`matchmaking_hopper_descriptions`), paired
//!   `<key>_title` / `<key>_description`; the compiled-in defaults
//!   (`0x00193c70`) write these texts into a hopper's title and description.
//! - **variants**: the built-in game variants of the variant-settings tag
//!   (`goof +0x28`, docs/game/community-gametypes.md): name and game type.

use std::collections::BTreeMap;
use std::io::{Read, Seek};

use anyhow::{Context, Result, anyhow};
use map::{CacheType, FourCC, Map};
use serde::{Deserialize, Serialize};
use tags::{Language, Localization, StringId};

/// The catalog's format, bumped only for changes a client cannot read.
pub const CATALOG_VERSION: u32 = 1;
/// Its file name inside the art directory.
pub const CATALOG: &str = "catalog.json";

/// `matg +0x178`: the UI level data block; its element 0 holds the
/// multiplayer levels at `+0x10`.
const MATG_LEVELS: usize = 0x178;
const LEVEL_DATA_SIZE: u32 = 0x18;
const LEVEL_DATA_MP: usize = 0x10;
/// The level select's campaign levels in the same element: `+0` campaign
/// id, `+4` map id, `+8` the picture (tag reference, ID at `+0xc`), `+0x10`
/// nine 32-unit UTF-16 names, `+0x250` nine 128-unit descriptions.
const LEVEL_DATA_CAMPAIGN: usize = 0x8;
pub const CAMPAIGN_LEVEL_SIZE: usize = 0xb50;
const CAMPAIGN_PICTURE: usize = 0xc;
const CAMPAIGN_NAME: usize = 0x10;
const CAMPAIGN_DESC: usize = 0x250;
/// `matg +0x170`: the levels the game starts; element 0 holds a block of
/// `0x108`-byte entries (`+0` campaign id, `+4` map id, `+8` scenario path).
const MATG_START_LEVELS: usize = 0x170;
pub const START_LEVEL_SIZE: usize = 0x108;
/// One multiplayer level: `+0` map id, `+0xc` nine 32-unit UTF-16 names,
/// `+0x24c` nine 128-unit UTF-16 descriptions, `+0xb4c` the scenario path.
pub const LEVEL_SIZE: usize = 0xc64;
const LEVEL_NAME: usize = 0xc;
const LEVEL_DESC: usize = 0x24c;
const LEVEL_PATH: usize = 0xb4c;
/// More would not be a level list.
const MAX_LEVELS: u32 = 256;

/// `goof +0x20` its `unic` (tag reference: group, datum), `+0x28` the
/// built-in variants block of `0x14`-byte entries (`+0` name string id,
/// `+4` game type).
const GOOF_UNIC: usize = 0x20;
const GOOF_BUILTINS: usize = 0x28;
const BUILTIN_SIZE: usize = 0x14;
const MAX_BUILTINS: u32 = 101;

/// The padlock glyph retail built-in names end with.
const PADLOCK: char = '\u{e008}';

/// A multiplayer map a hopper record can name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapEntry {
    /// The record id that resolves to this map.
    pub id: u32,
    /// English name and description.
    pub name: String,
    pub description: String,
    /// The map file's stem (the art manifest's `map/<key>/preview`), when
    /// the level names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// `"campaign"` for a campaign level (its `id` is the level's map id,
    /// not a hopper record id); absent for a multiplayer map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// One playlist's text from the hopper descriptions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistText {
    /// The strings' shared prefix (`<key>_title`).
    pub key: String,
    pub title: String,
    pub description: String,
}

/// A built-in game variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantEntry {
    /// English name, without the built-in padlock.
    pub name: String,
    /// `slayer`, `oddball`, `juggernaut`, `king_of_the_hill`, `ctf`,
    /// `assault`, `territories`.
    pub game_type: String,
    /// The game-engine index a variant carries at `+0x44` (what a hopper
    /// record's variant holds; docs/live/live-gamestate.md).
    pub engine: u32,
}

/// `catalog.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub version: u32,
    /// The art manifest's `source`: the disc it came from.
    pub source: String,
    pub maps: Vec<MapEntry>,
    pub playlists: Vec<PlaylistText>,
    pub variants: Vec<VariantEntry>,
    /// What could not be read, by list.
    pub skipped: BTreeMap<String, String>,
}

/// The `goof` game-type enum (`0x149f88`) as (name, engine index).
pub fn game_type(goof: u32) -> Option<(&'static str, u32)> {
    Some(match goof {
        0 => ("slayer", 2),
        1 => ("oddball", 3),
        2 => ("juggernaut", 7),
        3 => ("king_of_the_hill", 4),
        4 => ("ctf", 1),
        5 => ("assault", 9),
        6 => ("territories", 8),
        _ => return None,
    })
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

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

/// A `(count, address)` tag block at `at`, read whole with `read`.
fn block(
    b: &[u8],
    at: usize,
    size: usize,
    max: u32,
    read: &mut dyn FnMut(u32, u32) -> Option<Vec<u8>>,
) -> Result<Vec<u8>> {
    let (n, addr) = (
        le32(b, at).context("short tag")?,
        le32(b, at + 4).context("short tag")?,
    );
    if n == 0 {
        return Ok(Vec::new());
    }
    if n > max {
        return Err(anyhow!("block of {n} entries is not plausible"));
    }
    read(addr, n * size as u32).with_context(|| format!("block at {addr:#x} is outside the map"))
}

/// The multiplayer levels from a globals tag's bytes; `read` resolves a
/// tag-memory address. Unused slots (id `0xffffffff` or no name) are left
/// out.
pub fn levels(
    matg: &[u8],
    read: &mut dyn FnMut(u32, u32) -> Option<Vec<u8>>,
) -> Result<Vec<MapEntry>> {
    let data = block(matg, MATG_LEVELS, LEVEL_DATA_SIZE as usize, 16, read)?;
    if data.is_empty() {
        return Err(anyhow!("the globals have no level data"));
    }
    let all = block(&data, LEVEL_DATA_MP, LEVEL_SIZE, MAX_LEVELS, read)?;
    Ok(all
        .as_chunks::<LEVEL_SIZE>()
        .0
        .iter()
        .filter_map(|e| {
            let id = le32(e, 0)?;
            let name = utf16(e, LEVEL_NAME, 32);
            if id == u32::MAX || name.is_empty() {
                return None;
            }
            let path = &e[LEVEL_PATH..LEVEL_PATH + 0x100];
            let path = String::from_utf8_lossy(
                &path[..path.iter().position(|c| *c == 0).unwrap_or(path.len())],
            )
            .into_owned();
            let key = stem(&path);
            Some(MapEntry {
                id,
                name,
                description: utf16(e, LEVEL_DESC, 128),
                key,
                kind: None,
            })
        })
        .collect())
}

/// A campaign level for the catalog, and its level-select picture's tag ID
/// (in the map the globals came from) when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignLevel {
    pub entry: MapEntry,
    pub picture: Option<u32>,
}

fn stem(path: &str) -> Option<String> {
    path.rsplit(['\\', '/'])
        .next()
        .filter(|k| !k.is_empty())
        .map(str::to_string)
}

/// The campaign levels from a globals tag's bytes, in the order the game
/// lists the levels it can start, each named by the level select's entry
/// with the same campaign and map id. A level the level select does not
/// name is left out.
pub fn campaign_levels(
    matg: &[u8],
    read: &mut dyn FnMut(u32, u32) -> Option<Vec<u8>>,
) -> Result<Vec<CampaignLevel>> {
    let start = block(matg, MATG_START_LEVELS, 8, 16, read)?;
    if start.is_empty() {
        return Err(anyhow!("the globals have no level list"));
    }
    let paths = block(&start, 0, START_LEVEL_SIZE, MAX_LEVELS, read)?;
    let data = block(matg, MATG_LEVELS, LEVEL_DATA_SIZE as usize, 16, read)?;
    if data.is_empty() {
        return Err(anyhow!("the globals have no level data"));
    }
    let ui = block(
        &data,
        LEVEL_DATA_CAMPAIGN,
        CAMPAIGN_LEVEL_SIZE,
        MAX_LEVELS,
        read,
    )?;
    let ui: Vec<&[u8]> = ui
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
            let (campaign, id) = (le32(e, 0)?, le32(e, 4)?);
            let path = &e[8..];
            let path =
                String::from_utf8_lossy(&path[..path.iter().position(|c| *c == 0)?]).into_owned();
            let key = stem(&path)?;
            let u = ui
                .iter()
                .find(|u| le32(u, 0) == Some(campaign) && le32(u, 4) == Some(id))?;
            let name = utf16(u, CAMPAIGN_NAME, 32);
            if name.is_empty() {
                return None;
            }
            let picture = le32(u, CAMPAIGN_PICTURE).filter(|p| *p != u32::MAX);
            Some(CampaignLevel {
                entry: MapEntry {
                    id,
                    name,
                    description: utf16(u, CAMPAIGN_DESC, 128),
                    key: Some(key),
                    kind: Some("campaign".into()),
                },
                picture,
            })
        })
        .collect())
}

/// Pairs `<key>_title` / `<key>_description` strings, in the order the
/// titles come. A title without a description keeps an empty one.
pub fn playlist_texts(strings: &[(String, String)]) -> Vec<PlaylistText> {
    let text = |name: &str| {
        strings
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.clone())
    };
    strings
        .iter()
        .filter_map(|(name, title)| {
            let key = name.strip_suffix("_title")?;
            Some(PlaylistText {
                key: key.to_string(),
                title: title.clone(),
                description: text(&format!("{key}_description")).unwrap_or_default(),
            })
        })
        .collect()
}

/// The built-in variants of a variant-settings tag; `name` resolves a name
/// string id. Entries whose game type or name is unknown are left out.
pub fn builtins(
    goof: &[u8],
    read: &mut dyn FnMut(u32, u32) -> Option<Vec<u8>>,
    name: &dyn Fn(u32) -> Option<String>,
) -> Result<Vec<VariantEntry>> {
    let all = block(goof, GOOF_BUILTINS, BUILTIN_SIZE, MAX_BUILTINS, read)?;
    Ok(all
        .as_chunks::<BUILTIN_SIZE>()
        .0
        .iter()
        .filter_map(|e| {
            let (game_type, engine) = game_type(le32(e, 4)?)?;
            let name = name(le32(e, 0)?)?;
            let name = name.trim_end_matches(PADLOCK).trim_end().to_string();
            (!name.is_empty()).then(|| VariantEntry {
                name,
                game_type: game_type.into(),
                engine,
            })
        })
        .collect())
}

fn tag_bytes<R: Read + Seek>(map: &mut Map<R>, tag: &map::Tag) -> Result<Vec<u8>> {
    let loc = tag.location.context("the tag has no data")?;
    Ok(map.read_file_range(loc.file_offset()?, loc.size.into())?)
}

/// The catalog from the main menu map of `disc`.
pub fn extract(disc: &crate::live_art::Disc) -> Catalog {
    let mut catalog = Catalog {
        version: CATALOG_VERSION,
        source: disc.key.clone(),
        maps: Vec::new(),
        playlists: Vec::new(),
        variants: Vec::new(),
        skipped: BTreeMap::new(),
    };
    let opened = disc
        .maps_of(CacheType::MainMenu)
        .next()
        .context("no main menu map on the disc")
        .and_then(|m| disc.open(m));
    let mut map = match opened {
        Ok((map, _)) => map,
        Err(e) => {
            for k in ["maps", "playlists", "variants"] {
                catalog.skipped.insert(k.into(), format!("{e:#}"));
            }
            return catalog;
        }
    };
    let localization = Localization::read(&mut map);

    match read_maps(&mut map) {
        Ok(m) if !m.is_empty() => catalog.maps = m,
        Ok(_) => drop(
            catalog
                .skipped
                .insert("maps".into(), "no multiplayer levels".into()),
        ),
        Err(e) => drop(catalog.skipped.insert("maps".into(), format!("{e:#}"))),
    }
    // Campaign levels after the multiplayer maps, so a lookup by hopper
    // record id finds a multiplayer map first.
    match read_campaign(&mut map) {
        Ok(c) if !c.is_empty() => catalog.maps.extend(c.into_iter().map(|c| c.entry)),
        Ok(_) => drop(
            catalog
                .skipped
                .insert("maps/campaign".into(), "no campaign levels".into()),
        ),
        Err(e) => drop(
            catalog
                .skipped
                .insert("maps/campaign".into(), format!("{e:#}")),
        ),
    }
    let result = localization
        .as_ref()
        .map_err(|e| anyhow!("{e}"))
        .and_then(|l| read_playlists(&mut map, l));
    match result {
        Ok(p) if !p.is_empty() => catalog.playlists = p,
        Ok(_) => drop(
            catalog
                .skipped
                .insert("playlists".into(), "no playlist strings".into()),
        ),
        Err(e) => drop(catalog.skipped.insert("playlists".into(), format!("{e:#}"))),
    }
    let result = localization
        .as_ref()
        .map_err(|e| anyhow!("{e}"))
        .and_then(|l| read_variants(&mut map, l));
    match result {
        Ok(v) if !v.is_empty() => catalog.variants = v,
        Ok(_) => drop(
            catalog
                .skipped
                .insert("variants".into(), "no built-in variants".into()),
        ),
        Err(e) => drop(catalog.skipped.insert("variants".into(), format!("{e:#}"))),
    }
    catalog
}

fn reader<R: Read + Seek>(map: &mut Map<R>) -> impl FnMut(u32, u32) -> Option<Vec<u8>> + '_ {
    move |addr, size| {
        let at = map.file_offset(addr, size).ok()?;
        map.read_file_range(at, size.into()).ok()
    }
}

fn read_maps<R: Read + Seek>(map: &mut Map<R>) -> Result<Vec<MapEntry>> {
    let globals = map.globals().context("no globals tag")?.clone();
    let matg = tag_bytes(map, &globals)?;
    levels(&matg, &mut reader(map))
}

/// The campaign levels from a map's globals.
pub(crate) fn read_campaign<R: Read + Seek>(map: &mut Map<R>) -> Result<Vec<CampaignLevel>> {
    let globals = map.globals().context("no globals tag")?.clone();
    let matg = tag_bytes(map, &globals)?;
    campaign_levels(&matg, &mut reader(map))
}

fn read_playlists<R: Read + Seek>(map: &mut Map<R>, l: &Localization) -> Result<Vec<PlaylistText>> {
    let tag = map
        .tags()
        .iter()
        .find(|t| {
            t.group == FourCC(*b"unic")
                && t.name.as_deref().is_some_and(|n| {
                    n.rsplit('\\').next() == Some(crate::live_catalog::HOPPER_STRINGS)
                })
        })
        .context("no hopper description strings")?
        .id;
    let unic = l.unic(map, tag)?;
    let strings: Vec<(String, String)> = l
        .entries_for(&unic, Language::English)?
        .iter()
        .filter_map(|e| {
            let name = e.id.resolve(map.strings()).ok()??.to_string();
            let text = l.text_in(&unic, e.id, Language::English).ok()??;
            Some((name, text))
        })
        .collect();
    Ok(playlist_texts(&strings))
}

/// The hopper file descriptor's tag (docs/live/live-motd.md §1.2), by its last
/// path component.
pub const HOPPER_STRINGS: &str = "matchmaking_hopper_descriptions";

fn read_variants<R: Read + Seek>(map: &mut Map<R>, l: &Localization) -> Result<Vec<VariantEntry>> {
    let tag = map
        .tags()
        .iter()
        .find(|t| t.group == tags::ui::VARIANT_SETTINGS)
        .context("no variant settings tag")?
        .clone();
    let goof = tag_bytes(map, &tag)?;
    let unic_id = le32(&goof, GOOF_UNIC + 4).context("short variant settings tag")?;
    let unic = l.unic(map, map::TagId(unic_id))?;
    let name = |id: u32| {
        l.text_in(&unic, StringId(id), Language::English)
            .ok()
            .flatten()
    };
    builtins(&goof, &mut reader(map), &name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invented tag memory: `(address, bytes)` regions.
    struct Mem(Vec<(u32, Vec<u8>)>);
    impl Mem {
        fn read(&self, addr: u32, size: u32) -> Option<Vec<u8>> {
            self.0.iter().find_map(|(base, b)| {
                let off = addr.checked_sub(*base)? as usize;
                b.get(off..off + size as usize).map(<[u8]>::to_vec)
            })
        }
    }

    fn put(b: &mut [u8], at: usize, v: u32) {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn put_wide(b: &mut [u8], at: usize, s: &str) {
        for (i, u) in s.encode_utf16().enumerate() {
            b[at + i * 2..at + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
    }

    fn level(id: u32, name: &str, desc: &str, path: &str) -> Vec<u8> {
        let mut e = vec![0u8; LEVEL_SIZE];
        put(&mut e, 0, id);
        put_wide(&mut e, LEVEL_NAME, name);
        // Another language's name must not leak into the English one.
        put_wide(&mut e, LEVEL_NAME + 0x40, "Autre");
        put_wide(&mut e, LEVEL_DESC, desc);
        e[LEVEL_PATH..LEVEL_PATH + path.len()].copy_from_slice(path.as_bytes());
        e
    }

    fn globals_with(levels: &[Vec<u8>]) -> (Vec<u8>, Mem) {
        let mut matg = vec![0u8; 0x200];
        put(&mut matg, MATG_LEVELS, 1);
        put(&mut matg, MATG_LEVELS + 4, 0x8000_1000);
        let mut data = vec![0u8; 0x18];
        put(&mut data, LEVEL_DATA_MP, levels.len() as u32);
        put(&mut data, LEVEL_DATA_MP + 4, 0x8000_2000);
        (
            matg,
            Mem(vec![(0x8000_1000, data), (0x8000_2000, levels.concat())]),
        )
    }

    #[test]
    fn levels_read_ids_names_and_keys() {
        let (matg, mem) = globals_with(&[
            level(
                0x10,
                "Alpha Yard",
                "A yard.",
                "scenarios\\multi\\alpha\\alpha_yard",
            ),
            level(u32::MAX, "", "", ""),
            level(0x2a, "Beta", "", ""),
        ]);
        let got = levels(&matg, &mut |a, s| mem.read(a, s)).unwrap();
        assert_eq!(
            got,
            vec![
                MapEntry {
                    id: 0x10,
                    name: "Alpha Yard".into(),
                    description: "A yard.".into(),
                    key: Some("alpha_yard".into()),
                    kind: None,
                },
                MapEntry {
                    id: 0x2a,
                    name: "Beta".into(),
                    description: String::new(),
                    key: None,
                    kind: None,
                },
            ]
        );
    }

    #[test]
    fn levels_fail_cleanly() {
        let (mut matg, mem) = globals_with(&[level(1, "A", "", "")]);
        // Address outside the map.
        put(&mut matg, MATG_LEVELS + 4, 0x9000_0000);
        assert!(levels(&matg, &mut |a, s| mem.read(a, s)).is_err());
        // Implausible count.
        let (mut matg, mem) = globals_with(&[level(1, "A", "", "")]);
        put(&mut matg, MATG_LEVELS, 1_000_000);
        assert!(levels(&matg, &mut |a, s| mem.read(a, s)).is_err());
        // No level data at all.
        assert!(levels(&[0u8; 0x200], &mut |a, s| mem.read(a, s)).is_err());
        // Short tag.
        assert!(levels(&[0u8; 4], &mut |a, s| mem.read(a, s)).is_err());
    }

    fn start_level(campaign: u32, id: u32, path: &str) -> Vec<u8> {
        let mut e = vec![0u8; START_LEVEL_SIZE];
        put(&mut e, 0, campaign);
        put(&mut e, 4, id);
        e[8..8 + path.len()].copy_from_slice(path.as_bytes());
        e
    }

    fn select_level(campaign: u32, id: u32, picture: u32, name: &str, desc: &str) -> Vec<u8> {
        let mut e = vec![0u8; CAMPAIGN_LEVEL_SIZE];
        put(&mut e, 0, campaign);
        put(&mut e, 4, id);
        put(&mut e, 8, u32::from_le_bytes(*b"mtib"));
        put(&mut e, CAMPAIGN_PICTURE, picture);
        put_wide(&mut e, CAMPAIGN_NAME, name);
        put_wide(&mut e, CAMPAIGN_NAME + 0x40, "Autre");
        put_wide(&mut e, CAMPAIGN_DESC, desc);
        e
    }

    /// Globals with both level lists: `(start levels, level select)`.
    fn campaign_globals(start: &[Vec<u8>], select: &[Vec<u8>]) -> (Vec<u8>, Mem) {
        let mut matg = vec![0u8; 0x200];
        put(&mut matg, MATG_START_LEVELS, 1);
        put(&mut matg, MATG_START_LEVELS + 4, 0x8000_4000);
        put(&mut matg, MATG_LEVELS, 1);
        put(&mut matg, MATG_LEVELS + 4, 0x8000_1000);
        let mut start_el = vec![0u8; 8];
        put(&mut start_el, 0, start.len() as u32);
        put(&mut start_el, 4, 0x8000_5000);
        let mut data = vec![0u8; 0x18];
        put(&mut data, LEVEL_DATA_CAMPAIGN, select.len() as u32);
        put(&mut data, LEVEL_DATA_CAMPAIGN + 4, 0x8001_0000);
        (
            matg,
            Mem(vec![
                (0x8000_1000, data),
                (0x8000_4000, start_el),
                (0x8000_5000, start.concat()),
                (0x8001_0000, select.concat()),
            ]),
        )
    }

    #[test]
    fn campaign_levels_join_the_start_list_and_the_level_select() {
        let (matg, mem) = campaign_globals(
            &[
                start_level(1, 7, "scenarios\\solo\\07_moon\\07_moon"),
                start_level(1, 3, "scenarios\\solo\\03_dock\\03_dock"),
                // Not on the level select: left out.
                start_level(1, 9, "scenarios\\solo\\09_test\\09_test"),
                // Same map id in another campaign: not the same level.
                start_level(2, 3, "scenarios\\solo\\other\\other"),
            ],
            &[
                select_level(1, 3, 0xe00a_0001, "The Dock", "Boats."),
                select_level(1, 7, u32::MAX, "Moonrise", ""),
            ],
        );
        let got = campaign_levels(&matg, &mut |a, s| mem.read(a, s)).unwrap();
        let entry = |id: u32, name: &str, desc: &str, key: &str| MapEntry {
            id,
            name: name.into(),
            description: desc.into(),
            key: Some(key.into()),
            kind: Some("campaign".into()),
        };
        assert_eq!(
            got,
            vec![
                CampaignLevel {
                    entry: entry(7, "Moonrise", "", "07_moon"),
                    picture: None,
                },
                CampaignLevel {
                    entry: entry(3, "The Dock", "Boats.", "03_dock"),
                    picture: Some(0xe00a_0001),
                },
            ]
        );
        // The multiplayer list in the same globals is untouched by them.
        assert!(
            levels(&matg, &mut |a, s| mem.read(a, s))
                .unwrap()
                .is_empty()
        );

        // No start list, or no level data: an error, not an empty list.
        let mut none = matg.clone();
        put(&mut none, MATG_START_LEVELS, 0);
        assert!(campaign_levels(&none, &mut |a, s| mem.read(a, s)).is_err());
        let mut none = matg;
        put(&mut none, MATG_LEVELS, 0);
        assert!(campaign_levels(&none, &mut |a, s| mem.read(a, s)).is_err());
    }

    #[test]
    fn playlist_strings_pair_by_key() {
        let s = |a: &str, b: &str| (a.to_string(), b.to_string());
        let got = playlist_texts(&[
            s("solo_title", "Solo"),
            s("solo_description", "Alone."),
            s("pairs_description", "Two by two."),
            s("pairs_title", "Pairs"),
            s("orphan_title", "Orphan"),
            s("unrelated", "x"),
        ]);
        let keys: Vec<_> = got
            .iter()
            .map(|p| (p.key.as_str(), p.title.as_str(), p.description.as_str()))
            .collect();
        assert_eq!(
            keys,
            [
                ("solo", "Solo", "Alone."),
                ("pairs", "Pairs", "Two by two."),
                ("orphan", "Orphan", "")
            ]
        );
    }

    #[test]
    fn builtins_map_game_types_and_strip_the_padlock() {
        let mut goof = vec![0u8; 0x40];
        put(&mut goof, GOOF_BUILTINS, 4);
        put(&mut goof, GOOF_BUILTINS + 4, 0x8000_3000);
        let mut entries = vec![0u8; 4 * BUILTIN_SIZE];
        for (i, (name, ty)) in [(1u32, 0u32), (2, 4), (3, 9), (99, 1)].iter().enumerate() {
            put(&mut entries, i * BUILTIN_SIZE, *name);
            put(&mut entries, i * BUILTIN_SIZE + 4, *ty);
        }
        let mem = Mem(vec![(0x8000_3000, entries)]);
        let names = |id: u32| match id {
            1 => Some("Team Fight \u{e008}".to_string()),
            2 => Some("Flags".to_string()),
            3 => Some("Bad Type".to_string()),
            _ => None,
        };
        let got = builtins(&goof, &mut |a, s| mem.read(a, s), &names).unwrap();
        assert_eq!(
            got,
            vec![
                VariantEntry {
                    name: "Team Fight".into(),
                    game_type: "slayer".into(),
                    engine: 2
                },
                VariantEntry {
                    name: "Flags".into(),
                    game_type: "ctf".into(),
                    engine: 1
                },
            ]
        );
    }

    #[test]
    fn catalog_json_shape() {
        let c = Catalog {
            version: CATALOG_VERSION,
            source: "k".into(),
            maps: vec![MapEntry {
                id: 1,
                name: "A".into(),
                description: String::new(),
                key: None,
                kind: None,
            }],
            playlists: vec![],
            variants: vec![],
            skipped: BTreeMap::from([("variants".to_string(), "why".to_string())]),
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(
            v["maps"][0],
            serde_json::json!({"id": 1, "name": "A", "description": ""})
        );
        assert_eq!(serde_json::from_value::<Catalog>(v).unwrap(), c);
        let campaign = MapEntry {
            id: 301,
            name: "B".into(),
            description: String::new(),
            key: Some("03_b".into()),
            kind: Some("campaign".into()),
        };
        assert_eq!(
            serde_json::to_value(&campaign).unwrap(),
            serde_json::json!({"id": 301, "name": "B", "description": "", "key": "03_b", "kind": "campaign"})
        );
    }

    /// Reads the owner's disc; prints counts only.
    #[test]
    #[ignore = "needs the disc (H2_DISC)"]
    fn from_the_disc() {
        let path = std::env::var("H2_DISC").expect("H2_DISC");
        let disc = crate::live_art::Disc::scan(std::path::Path::new(&path)).unwrap();
        let c = extract(&disc);
        println!(
            "maps {} playlists {} variants {} skipped {:?}",
            c.maps.len(),
            c.playlists.len(),
            c.variants.len(),
            c.skipped
        );
        assert!(!c.maps.is_empty() && !c.playlists.is_empty() && !c.variants.is_empty());
        // Stems, kinds, ids and names, outside the repository, for review.
        let out = std::env::var("H2_CATALOG_CHECK").unwrap_or_else(|_| {
            "/Users/afr/.config/rush/keep/h2recomp-b5afe71a/bed6c8dc/campaign-geometry/campaign-names.txt"
                .into()
        });
        let mut text = String::new();
        for m in &c.maps {
            text += &format!(
                "{:<20} {:<11} id {:>3} {}\n",
                m.key.as_deref().unwrap_or("-"),
                m.kind.as_deref().unwrap_or("multiplayer"),
                m.id,
                m.name
            );
        }
        print!("{text}");
        std::fs::write(out, text).unwrap();
        let campaign: Vec<_> = disc
            .maps_of(CacheType::Campaign)
            .map(|m| m.name().to_string())
            .collect();
        for stem in &campaign {
            assert!(
                c.maps
                    .iter()
                    .any(|m| m.kind.is_some() && m.key.as_deref() == Some(stem)),
                "{stem} has no catalog entry"
            );
        }
    }
}
