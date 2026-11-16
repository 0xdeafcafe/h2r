//! The names behind hopper record numbers, read from the user's own disc at
//! run time: the multiplayer maps a record can name (the globals tag's level
//! list in the main menu map), and the game-engine names a record's variant
//! carries. The same reading as `h2 live assets extract`'s catalog
//! (`crates/cli/src/live_catalog.rs`, docs/live/live-admin.md "The catalog"),
//! maps only; nothing of it is in the repo.

use std::io::{Read, Seek};

use anyhow::{Context, Result, anyhow};
use map::Map;

use crate::source::MapSource;

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
const MAX_LEVELS: u32 = 256;

/// A multiplayer map a hopper record can name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogMap {
    /// The record id that resolves to this map.
    pub id: u32,
    pub name: String,
    pub description: String,
    /// The map file's stem, when the level names a scenario.
    pub key: Option<String>,
}

/// What the hopper tool knows about the user's game.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    pub maps: Vec<CatalogMap>,
}

/// Game types by engine index, as a record's variant carries them:
/// (index, key, display name).
pub const ENGINES: [(u32, &str, &str); 7] = [
    (1, "ctf", "Capture the Flag"),
    (2, "slayer", "Slayer"),
    (3, "oddball", "Oddball"),
    (4, "king_of_the_hill", "King of the Hill"),
    (7, "juggernaut", "Juggernaut"),
    (8, "territories", "Territories"),
    (9, "assault", "Assault"),
];

/// The engine index of a game-type key (`slayer`, `ctf`, ...).
pub fn engine_of(game_type: &str) -> Option<u32> {
    ENGINES
        .iter()
        .find(|(_, k, _)| *k == game_type)
        .map(|(n, ..)| *n)
}

/// The display name of an engine index.
pub fn engine_name(engine: u32) -> Option<&'static str> {
    ENGINES
        .iter()
        .find(|(n, ..)| *n == engine)
        .map(|(_, _, name)| *name)
}

/// Lower case, letters and digits only: how preset names match the disc's.
pub fn normalise(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

impl Catalog {
    /// A map by display name (case and punctuation ignored).
    pub fn find_map(&self, name: &str) -> Option<&CatalogMap> {
        let want = normalise(name);
        self.maps.iter().find(|m| normalise(&m.name) == want)
    }

    pub fn map_by_id(&self, id: u32) -> Option<&CatalogMap> {
        self.maps.iter().find(|m| m.id == id)
    }

    /// The catalog from a source's main menu map.
    pub fn from_source(source: &MapSource) -> Result<Catalog> {
        let mut map = source
            .open_map("mainmenu")
            .context("the source has no mainmenu map")?;
        Ok(Catalog {
            maps: read_maps(&mut map)?,
        })
    }
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
        .map(|c| u16::from_le_bytes(*c))
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
) -> Result<Vec<CatalogMap>> {
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
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            Some(CatalogMap {
                id,
                name,
                description: utf16(e, LEVEL_DESC, 128),
                key,
            })
        })
        .collect())
}

fn read_maps<R: Read + Seek>(map: &mut Map<R>) -> Result<Vec<CatalogMap>> {
    let globals = map.globals().context("no globals tag")?.clone();
    let loc = globals.location.context("the globals tag has no data")?;
    let matg = map.read_file_range(loc.file_offset()?, loc.size.into())?;
    let mut read = |addr: u32, size: u32| {
        let at = map.file_offset(addr, size).ok()?;
        map.read_file_range(at, size.into()).ok()
    };
    levels(&matg, &mut read)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(b: &mut [u8], at: usize, v: u32) {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn put_wide(b: &mut [u8], at: usize, s: &str) {
        for (i, u) in s.encode_utf16().enumerate() {
            b[at + i * 2..at + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
    }

    fn level(id: u32, name: &str, path: &str) -> Vec<u8> {
        let mut e = vec![0u8; LEVEL_SIZE];
        put(&mut e, 0, id);
        put_wide(&mut e, LEVEL_NAME, name);
        put_wide(&mut e, LEVEL_DESC, "a description");
        e[LEVEL_PATH..LEVEL_PATH + path.len()].copy_from_slice(path.as_bytes());
        e
    }

    #[test]
    fn levels_read_from_invented_tag_memory() {
        let mut matg = vec![0u8; 0x200];
        put(&mut matg, MATG_LEVELS, 1);
        put(&mut matg, MATG_LEVELS + 4, 0x1000);
        let mut data = vec![0u8; LEVEL_DATA_SIZE as usize];
        put(&mut data, LEVEL_DATA_MP, 3);
        put(&mut data, LEVEL_DATA_MP + 4, 0x2000);
        let mut levels_bytes = level(4, "Alpha Base", "scenarios\\multi\\alpha\\alpha");
        levels_bytes.extend(level(u32::MAX, "Unused", ""));
        levels_bytes.extend(level(9, "Beta Yard", ""));
        let mem = [(0x1000u32, data), (0x2000u32, levels_bytes)];
        let mut read = |addr: u32, size: u32| {
            mem.iter().find_map(|(base, b)| {
                let off = addr.checked_sub(*base)? as usize;
                b.get(off..off + size as usize).map(<[u8]>::to_vec)
            })
        };
        let maps = levels(&matg, &mut read).unwrap();
        assert_eq!(maps.len(), 2);
        assert_eq!(maps[0].id, 4);
        assert_eq!(maps[0].name, "Alpha Base");
        assert_eq!(maps[0].description, "a description");
        assert_eq!(maps[0].key.as_deref(), Some("alpha"));
        assert_eq!(maps[1].key, None);

        let c = Catalog { maps };
        assert_eq!(c.find_map("alpha-base").unwrap().id, 4);
        assert_eq!(c.map_by_id(9).unwrap().name, "Beta Yard");
        assert!(c.find_map("Gamma").is_none());

        let mut empty = vec![0u8; 0x200];
        put(&mut empty, MATG_LEVELS, 0);
        assert!(levels(&empty, &mut read).is_err());
        put(&mut empty, MATG_LEVELS, 1000);
        assert!(levels(&empty, &mut read).is_err(), "implausible count");
    }

    #[test]
    fn engines_by_key_and_index() {
        assert_eq!(engine_of("king_of_the_hill"), Some(4));
        assert_eq!(engine_of("race"), None);
        assert_eq!(engine_name(9), Some("Assault"));
        assert_eq!(engine_name(5), None);
    }

    /// Reads the user's own disc when `H2_DISC` points at it.
    #[test]
    #[ignore = "needs the user's disc (H2_DISC)"]
    fn maps_from_the_disc() {
        let path = crate::source::default_disc(None).expect("H2_DISC");
        let source = MapSource::open(&path).unwrap();
        let c = Catalog::from_source(&source).unwrap();
        assert!(!c.maps.is_empty());
    }
}
