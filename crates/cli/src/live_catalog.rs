//! The playlist builder's catalog: what the owner's own game ships that a
//! hopper can point at, written as `catalog.json` beside the art manifest by
//! `h2 live assets extract` (docs/live-admin.md, "The catalog"). Nothing of
//! it is in the repo; tests build synthetic tag memory.
//!
//! Three lists, each read from the main menu map, each left out (and named
//! in `skipped`) when it cannot be read:
//!
//! - **maps**: the globals tag's multiplayer level list (`matg +0x178`,
//!   element 0, block at `+0x10`, `0xc64`-byte entries). The game looks a
//!   hopper record's id up here (`0x0019c5f0`, compares the entry's first
//!   dword), so these ids are exactly the record ids that resolve.
//! - **playlists**: the strings of the `unic` tag the hopper file's
//!   descriptor names (`matchmaking_hopper_descriptions`), paired
//!   `<key>_title` / `<key>_description`; the compiled-in defaults
//!   (`0x00193c70`) write these texts into a hopper's title and description.
//! - **variants**: the built-in game variants of the variant-settings tag
//!   (`goof +0x28`, docs/community-gametypes.md): name and game type.

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
    /// record's variant holds; docs/live-gamestate.md).
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
            let key = path
                .rsplit(['\\', '/'])
                .next()
                .filter(|k| !k.is_empty())
                .map(str::to_string);
            Some(MapEntry {
                id,
                name,
                description: utf16(e, LEVEL_DESC, 128),
                key,
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

/// The hopper file descriptor's tag (docs/live-motd.md §1.2), by its last
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
                },
                MapEntry {
                    id: 0x2a,
                    name: "Beta".into(),
                    description: String::new(),
                    key: None,
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
    }
}
