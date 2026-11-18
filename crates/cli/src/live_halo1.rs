//! Halo 1 maps for the stats site's art (docs/live/live-art.md, "Halo 1
//! maps"): the levels `h2 import halo1` converted into Halo 2 maps under a
//! runtime data root (docs/game/campaigns.md "Importing"). They are real
//! Halo 2 cache files, so the extractor draws them like the disc's own:
//! geometry, views, sky and a preview, each map under the name the game
//! reports it by (the cache header's short name: `h1_<map>` for a
//! multiplayer map, the level's own, `a10`, for a campaign level).
//!
//! - **Campaign**: `<data>/campaigns/halo1/<level>.map`, named by that
//!   folder's `campaign.toml`, pictured by its `cards/<level>.png`.
//! - **Multiplayer**: every `<data>/maps/<dir>/manifest.toml` package with a
//!   variant whose origin is Halo 1; the Halo 1 sandbox's map when it has
//!   one (the level is the same in both), named and pictured by the
//!   package.
//!
//! `<data>/campaigns/halo1/import.stamp` (which disc, which converter)
//! joins the art's cache key, so a new conversion extracts again. Nothing of
//! the maps is in the repository; the tests build invented packages.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::live_geometry::MapKind;

/// The `game` a Halo 1 map's manifest entry carries.
pub const GAME: &str = "halo1";
/// The import's stamp file in the campaign folder (`h1_import`).
const STAMP: &str = "import.stamp";
const HEAD: usize = 2048;

/// What the manifest says about a Halo 1 map besides its geometry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Halo1Map {
    /// Its display name (the package's or the campaign list's).
    pub name: String,
    /// A picture of it (PNG) to serve as `map/<stem>/preview`.
    pub preview: Option<PathBuf>,
}

/// A converted map: where it is, its header and what it is called.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundMap {
    /// The name the game reports it by, lower case (`h1_bloodgulch`, `a10`).
    pub stem: String,
    pub path: PathBuf,
    pub size: u32,
    /// Its first 2 KiB (the cache header).
    pub head: Vec<u8>,
    pub kind: MapKind,
    pub info: Halo1Map,
}

/// Every converted Halo 1 map under a data root, and the import's stamp.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub maps: Vec<FoundMap>,
    /// The import stamp's text, when there is one.
    pub stamp: Option<Vec<u8>>,
}

/// The cache header's short name (`+0x198`, 32 bytes), lower case.
pub fn header_name(head: &[u8]) -> Option<String> {
    let raw = head.get(0x198..0x1b8)?;
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    let name = std::str::from_utf8(&raw[..end]).ok()?.trim();
    (!name.is_empty() && name.bytes().all(|b| b.is_ascii_graphic()))
        .then(|| name.to_ascii_lowercase())
}

/// A PNG's width and height from its header.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..8)? != b"\x89PNG\r\n\x1a\n" || bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
    let h = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

fn read_map(path: &Path, kind: MapKind, fallback: &str, info: Halo1Map) -> Result<FoundMap> {
    let mut f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let size = u32::try_from(f.metadata()?.len())?;
    let mut head = vec![0u8; HEAD.min(size as usize)];
    f.read_exact(&mut head)?;
    Ok(FoundMap {
        stem: header_name(&head).unwrap_or_else(|| fallback.to_ascii_lowercase()),
        path: path.to_path_buf(),
        size,
        head,
        kind,
        info,
    })
}

fn existing(p: PathBuf) -> Option<PathBuf> {
    p.is_file().then_some(p)
}

/// The converted campaign levels in `dir` (`<data>/campaigns/halo1`), in
/// the campaign list's order.
fn campaign(dir: &Path) -> Vec<FoundMap> {
    let Some(list) = std::fs::read_to_string(dir.join(content::campaign_list::MANIFEST_FILE))
        .ok()
        .and_then(|t| content::campaign_list::CampaignManifest::parse(&t).ok())
    else {
        return Vec::new();
    };
    list.levels
        .iter()
        .filter_map(|l| {
            let path = existing(dir.join(format!("{}.map", l.map)))?;
            let preview = l
                .picture
                .as_deref()
                .and_then(|p| existing(dir.join(p)))
                .or_else(|| {
                    existing(
                        dir.join(boot::campaign_cards::CARDS_DIR)
                            .join(format!("{}.png", l.map)),
                    )
                });
            let info = Halo1Map {
                name: l.name.clone(),
                preview,
            };
            read_map(&path, MapKind::Campaign, &l.map, info).ok()
        })
        .collect()
}

/// The converted multiplayer packages under `maps` (`<data>/maps`).
fn multiplayer(maps: &Path) -> Vec<FoundMap> {
    let Ok(dirs) = std::fs::read_dir(maps) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = dirs.flatten().map(|d| d.path()).collect();
    dirs.sort();
    let mut out = Vec::new();
    for dir in dirs {
        let Some(m) = std::fs::read_to_string(dir.join(content::manifest::MANIFEST_FILE))
            .ok()
            .and_then(|t| content::manifest::Manifest::from_toml(&t).ok())
        else {
            continue;
        };
        let halo1: Vec<_> = m
            .variants
            .iter()
            .filter(|v| v.origin.game == GAME)
            .collect();
        let Some(variant) = halo1
            .iter()
            .find(|v| v.origin.sandbox == "h1")
            .or_else(|| halo1.first())
        else {
            continue;
        };
        let Ok(file) = content::manifest::relative(&variant.file) else {
            continue;
        };
        let info = Halo1Map {
            name: m.package.name.clone(),
            preview: m
                .package
                .image
                .as_deref()
                .and_then(|i| content::manifest::relative(i).ok())
                .and_then(|i| existing(dir.join(i))),
        };
        if let Ok(found) = read_map(&dir.join(file), MapKind::Multiplayer, &m.base.map, info) {
            out.push(found);
        }
    }
    out
}

/// Every converted Halo 1 map under the data root `data`: the campaign's
/// levels, then the multiplayer maps, each stem once. Empty when nothing
/// was imported.
pub fn installed(data: &Path) -> Found {
    let dir = crate::h1_import::campaign_dir(data);
    let mut maps = campaign(&dir);
    maps.extend(multiplayer(&content::store::maps_dir(data)));
    let mut seen = std::collections::BTreeSet::new();
    maps.retain(|m| seen.insert(m.stem.clone()));
    Found {
        maps,
        stamp: std::fs::read(dir.join(STAMP)).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An invented cache file: the cache type and the header's short name.
    fn map_bytes(cache_type: u32, name: &str) -> Vec<u8> {
        let mut b = vec![0u8; 4096];
        b[0x140..0x144].copy_from_slice(&cache_type.to_le_bytes());
        b[0x198..0x198 + name.len()].copy_from_slice(name.as_bytes());
        b
    }

    /// A 3 x 2 PNG header (enough for [`png_size`]).
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend([8, 6, 0, 0, 0]);
        b
    }

    #[test]
    fn the_header_names_the_map() {
        assert_eq!(header_name(&map_bytes(1, "H1_Alpha")).as_deref(), Some("h1_alpha"));
        assert_eq!(header_name(&map_bytes(1, "")), None);
        assert_eq!(header_name(&[0u8; 16]), None);
        assert_eq!(png_size(&png(3, 2)), Some((3, 2)));
        assert_eq!(png_size(b"not a png at all, not at all"), None);
    }

    #[test]
    fn converted_maps_are_found_under_the_data_root() {
        let root = std::env::temp_dir().join(format!("h2-halo1-art-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // A campaign of two levels, one of them not converted.
        let camp = crate::h1_import::campaign_dir(&root);
        std::fs::create_dir_all(camp.join("cards")).unwrap();
        std::fs::write(
            camp.join("campaign.toml"),
            "format = 1\n[campaign]\nkey = \"halo1\"\nname = \"Invented\"\n\
             [[level]]\nmap = \"x10\"\nname = \"First Level\"\n\
             [[level]]\nmap = \"x20\"\nname = \"Second Level\"\n",
        )
        .unwrap();
        std::fs::write(camp.join("x10.map"), map_bytes(0, "x10")).unwrap();
        std::fs::write(camp.join("cards/x10.png"), png(4, 4)).unwrap();
        std::fs::write(camp.join(STAMP), "converter=1\n").unwrap();
        // A converted multiplayer package in both sandboxes, and a package
        // that is not from Halo 1.
        let pkg = content::store::maps_dir(&root).join("h1_alpha");
        for s in ["h1", "h2"] {
            std::fs::create_dir_all(pkg.join(s)).unwrap();
            std::fs::write(pkg.join(s).join("h1_alpha.map"), map_bytes(1, "h1_alpha")).unwrap();
        }
        std::fs::write(pkg.join("preview.png"), png(3, 2)).unwrap();
        let variant = |s: &str| {
            format!(
                "[[variant]]\nfile = \"{s}/h1_alpha.map\"\nsha256 = \"{z}\"\n\
                 [variant.origin]\ngame = \"halo1\"\nmap = \"alpha\"\nsandbox = \"{s}\"\n\
                 source_sha256 = \"{z}\"\n",
                z = "0".repeat(64)
            )
        };
        std::fs::write(
            pkg.join("manifest.toml"),
            format!(
                "[package]\nformat = 1\nname = \"Alpha Canyon\"\nimage = \"preview.png\"\n\
                 [base]\nmap = \"h1_alpha\"\nderived = true\n{}{}",
                variant("h2"),
                variant("h1")
            ),
        )
        .unwrap();
        let other = content::store::maps_dir(&root).join("modded");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join("manifest.toml"),
            "[package]\nformat = 1\nname = \"Modded\"\n[base]\nmap = \"lockout\"\n",
        )
        .unwrap();

        let found = installed(&root);
        assert_eq!(found.stamp.as_deref(), Some(&b"converter=1\n"[..]));
        let stems: Vec<_> = found.maps.iter().map(|m| (m.stem.as_str(), m.kind)).collect();
        assert_eq!(
            stems,
            [("x10", MapKind::Campaign), ("h1_alpha", MapKind::Multiplayer)]
        );
        let (c, mp) = (&found.maps[0], &found.maps[1]);
        assert_eq!(c.info.name, "First Level");
        assert_eq!(c.info.preview, Some(camp.join("cards/x10.png")));
        assert_eq!((c.size, c.head.len()), (4096, HEAD));
        assert_eq!(mp.info.name, "Alpha Canyon");
        assert_eq!(mp.path, pkg.join("h1/h1_alpha.map"), "the Halo 1 sandbox's map");
        assert_eq!(mp.info.preview, Some(pkg.join("preview.png")));
        assert_eq!(installed(&root.join("nothing")), Found::default());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
