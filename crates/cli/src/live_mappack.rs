//! Map pack maps for the stats site's art (docs/live/live-art.md, "Map pack"):
//! found on the Multiplayer Map Pack disc itself or where `h2
//! import-mappack` installed them, each with what its map list record
//! says (map id, English name and description). The extractor then draws
//! them like the retail disc's multiplayer maps: preview, geometry, views,
//! sky and a catalog entry. Nothing of them is in the repository; the tests
//! build invented records and files.
//!
//! - **The disc**: every `/demos/dlc/<package>/` directory's `.map` files,
//!   named by the package's `patch_v3.lvl` records (docs/game/map-pack.md 1).
//! - **Installed**: `<data>/disk/Partition1/TDATA/4D530064/$c/<offer>/`,
//!   one map and its version 2 `patch.lvl` per package (docs/game/map-pack.md 5).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use content::patch_lvl;
use serde::{Deserialize, Serialize};

/// What a map's list record says.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackMap {
    /// The map list id (v2 `+0x10c`).
    pub id: u32,
    pub name: String,
    pub description: String,
}

/// A map pack map: where its bytes are and its header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundMap {
    /// The map file's stem, lower case (`containment`).
    pub stem: String,
    pub host: PathBuf,
    /// The map's first byte in `host`.
    pub base: u64,
    pub size: u32,
    /// Its first 2 KiB (the cache header).
    pub head: Vec<u8>,
    pub pack: PackMap,
}

impl FoundMap {
    /// The whole map, read from its host (`h2 live maps import` reads a
    /// map pack base this way).
    pub fn read(&self) -> Result<Vec<u8>> {
        let mut f =
            File::open(&self.host).with_context(|| format!("opening {}", self.host.display()))?;
        let mut out = vec![0u8; self.size as usize];
        f.seek(SeekFrom::Start(self.base))?;
        f.read_exact(&mut out)
            .with_context(|| format!("reading {}.map from {}", self.stem, self.host.display()))?;
        Ok(out)
    }
}

/// v2 record: the English name (32 UTF-16 units) and description (128),
/// as `content::map_list` writes them.
const NAMES: usize = 0x114;
const NAME_UNITS: usize = 32;
const DESCRIPTIONS: usize = 0x354;
const DESCRIPTION_UNITS: usize = 128;
const HEAD: usize = 2048;
/// Halo 2's content directory under a runtime data root.
const CONTENT_DIR: &str = "disk/Partition1/TDATA/4D530064/$c";

fn utf16_at(b: &[u8], at: usize, units: usize) -> String {
    let Some(bytes) = b.get(at..at + units * 2) else {
        return String::new();
    };
    let words: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|w| u16::from_le_bytes(*w))
        .take_while(|&w| w != 0)
        .collect();
    String::from_utf16_lossy(&words).trim().to_string()
}

/// A version 2 record's map id, English name and description.
pub fn pack_map(v2: &[u8]) -> PackMap {
    PackMap {
        id: if v2.len() >= patch_lvl::V2_LEN {
            patch_lvl::v2_map_id(v2)
        } else {
            0
        },
        name: utf16_at(v2, NAMES, NAME_UNITS),
        description: utf16_at(v2, DESCRIPTIONS, DESCRIPTION_UNITS),
    }
}

/// The record in a `patch_v3.lvl` for map `stem`, as a version 2 record.
pub fn v2_for(patch_v3: &[u8], stem: &str) -> Option<Vec<u8>> {
    patch_lvl::v3_records(patch_v3)
        .ok()?
        .into_iter()
        .find(|r| patch_lvl::map_name(r).is_ok_and(|n| n.eq_ignore_ascii_case(stem)))
        .and_then(|r| patch_lvl::v2_from_v3(r).ok())
}

fn stem_of(path: &str) -> Option<String> {
    let file = path.rsplit(['/', '\\']).next()?;
    let lower = file.to_ascii_lowercase();
    lower.strip_suffix(".map").map(str::to_string)
}

fn head_at(f: &mut File, base: u64, size: u32) -> Result<Vec<u8>> {
    let mut head = vec![0u8; HEAD.min(size as usize)];
    f.seek(SeekFrom::Start(base))?;
    f.read_exact(&mut head)?;
    Ok(head)
}

/// The maps of the Map Pack disc at `iso`, named by their packages'
/// `patch_v3.lvl`. A map its package's records do not name keeps an empty
/// name.
pub fn from_iso(iso: &Path) -> Result<Vec<FoundMap>> {
    let mut x = crate::open_iso(iso)?;
    let partition = x.partition_offset();
    let entries: Vec<xiso::Entry> = x.entries()?.into_iter().filter(|e| !e.is_dir).collect();
    let mut f = File::open(iso).with_context(|| format!("opening {}", iso.display()))?;
    let at = |e: &xiso::Entry| partition + u64::from(e.sector) * xiso::SECTOR;
    let mut out = Vec::new();
    for e in &entries {
        let lower = e.path.to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix("/demos/dlc/") else {
            continue;
        };
        let Some((package, _)) = rest.split_once('/') else {
            continue;
        };
        let Some(stem) = stem_of(&e.path) else {
            continue;
        };
        let lvl = entries.iter().find(|p| {
            p.path
                .eq_ignore_ascii_case(&format!("/demos/dlc/{package}/patch_v3.lvl"))
        });
        let record = match lvl {
            Some(p) => {
                let mut bytes = vec![0u8; p.size as usize];
                f.seek(SeekFrom::Start(at(p)))?;
                f.read_exact(&mut bytes)?;
                v2_for(&bytes, &stem)
            }
            None => None,
        };
        out.push(FoundMap {
            head: head_at(&mut f, at(e), e.size)?,
            stem,
            host: iso.to_path_buf(),
            base: at(e),
            size: e.size,
            pack: record.as_deref().map(pack_map).unwrap_or_default(),
        });
    }
    out.sort_by(|a, b| a.stem.cmp(&b.stem));
    Ok(out)
}

/// The map pack maps `h2 import-mappack` installed under the runtime data
/// root `data`, each named by its package's `patch.lvl`. None when there
/// are none.
pub fn installed(data: &Path) -> Vec<FoundMap> {
    let Ok(dirs) = std::fs::read_dir(data.join(CONTENT_DIR)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for dir in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        let files: Vec<PathBuf> = files.flatten().map(|f| f.path()).collect();
        let record = files
            .iter()
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.eq_ignore_ascii_case("patch.lvl"))
            })
            .and_then(|p| std::fs::read(p).ok());
        for path in &files {
            let Some(stem) = path.to_str().and_then(stem_of) else {
                continue;
            };
            let found = (|| -> Result<FoundMap> {
                let mut f = File::open(path)?;
                let size = u32::try_from(f.metadata()?.len())?;
                Ok(FoundMap {
                    head: head_at(&mut f, 0, size)?,
                    stem: stem.clone(),
                    host: path.clone(),
                    base: 0,
                    size,
                    pack: record.as_deref().map(pack_map).unwrap_or_default(),
                })
            })();
            if let Ok(m) = found {
                out.push(m);
            }
        }
    }
    out.sort_by(|a, b| a.stem.cmp(&b.stem));
    out.dedup_by(|a, b| a.stem == b.stem);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v2_named(id: u32, name: &str, description: &str) -> Vec<u8> {
        let mut v2 = vec![0u8; patch_lvl::V2_LEN];
        v2[0x10c..0x110].copy_from_slice(&id.to_le_bytes());
        for (i, u) in name.encode_utf16().enumerate() {
            v2[NAMES + 2 * i..NAMES + 2 * i + 2].copy_from_slice(&u.to_le_bytes());
        }
        for (i, u) in description.encode_utf16().enumerate() {
            v2[DESCRIPTIONS + 2 * i..DESCRIPTIONS + 2 * i + 2].copy_from_slice(&u.to_le_bytes());
        }
        v2
    }

    #[test]
    fn a_record_gives_the_map_id_name_and_description() {
        let p = pack_map(&v2_named(0x0301, "Invented Map", "A made-up place."));
        assert_eq!(
            p,
            PackMap {
                id: 0x0301,
                name: "Invented Map".into(),
                description: "A made-up place.".into()
            }
        );
        assert_eq!(
            pack_map(&[0u8; 16]),
            PackMap::default(),
            "a short record names nothing"
        );
    }

    #[test]
    fn a_patch_v3_record_is_found_by_its_map() {
        let a = patch_lvl::testing::v3_record(r"scenarios\multi\alpha\alpha", 0x0101, 7);
        let b = patch_lvl::testing::v3_record(r"scenarios\multi\beta\beta", 0x0102, 9);
        let file = patch_lvl::testing::v3_file(&[a, b]);
        let v2 = v2_for(&file, "BETA").unwrap();
        assert_eq!(patch_lvl::v2_map_id(&v2), 0x0102);
        assert!(v2_for(&file, "gamma").is_none());
        assert!(v2_for(b"junk", "beta").is_none());
    }

    #[test]
    fn installed_packages_are_found_under_the_data_root() {
        let root = std::env::temp_dir().join(format!("h2-mappack-art-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pkg = root.join(CONTENT_DIR).join("4d53006400000301");
        std::fs::create_dir_all(&pkg).unwrap();
        let mut map = vec![0u8; 4096];
        map[0x140..0x144].copy_from_slice(&1u32.to_le_bytes());
        std::fs::write(pkg.join("Invented.map"), &map).unwrap();
        std::fs::write(pkg.join("patch.lvl"), v2_named(0x0301, "Invented", "")).unwrap();
        std::fs::write(pkg.join("ContentMeta.xbx"), b"meta").unwrap();
        let found = installed(&root);
        assert_eq!(found.len(), 1);
        let m = &found[0];
        assert_eq!(m.stem, "invented");
        assert_eq!((m.base, m.size, m.head.len()), (0, 4096, HEAD));
        assert_eq!(m.pack.name, "Invented");
        assert_eq!(m.pack.id, 0x0301);
        assert!(installed(&root.join("nothing")).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
