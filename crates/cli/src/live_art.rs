//! `h2 live assets extract`: the stats site's art from the owner's own disc
//! (docs/live/live.md, "Game art"; docs/live/live-api.md, "Optional game art").
//!
//! No game asset is named here (CONTRIBUTING.md; docs/formats/bitmaps.md: "No pixels
//! or names of game assets are stored in the repo"). Each category is found
//! by structure at run time: which kind of map holds it (the map header's
//! cache type), how many images the bitmap tag has, and their shape.
//! Multiplayer map previews are the one picture-sized UI image a
//! multiplayer map carries that the shared map does not. Medals and the
//! colour palette are icon glyphs in the fonts' private-use range
//! (docs/formats/fonts.md). Everything found is decoded to PNG (RGBA) under semantic
//! keys and listed in `manifest.json`; output is deterministic for a disc.
//!
//! Only the maps that can hold art are opened, each on its own thread, and
//! only the bitmap tags that can match are decoded; PNGs are encoded in
//! parallel with fast compression.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use assets::{Region, ResourceFiles};
use map::{CacheType, FourCC, Map, ResourceFile};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tags::{DecodedTag, TagData};

/// Bumped when the output for a given disc would change (part of the cache
/// key).
pub const EXTRACTOR_VERSION: u32 = 24;
/// The manifest's format, bumped only for changes a client cannot read
/// (clients check it; added fields do not count).
pub const MANIFEST_VERSION: u32 = 1;

const BITM: FourCC = FourCC(*b"bitm");

/// One image's shape, as the picker sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    pub width: u32,
    pub height: u32,
    /// Stored linear (UI art), not swizzled or block-compressed.
    pub linear: bool,
}

impl Shape {
    fn square_pow2(&self) -> bool {
        self.width == self.height && self.width.is_power_of_two()
    }
}

/// A category: the key prefix, which kind of map holds it, how many images
/// its tag has, and what each image looks like. Among matching tags the one
/// with the largest images wins (the others are smaller copies).
pub struct Category {
    pub prefix: &'static str,
    pub source: CacheType,
    pub images: usize,
    pub shape: fn(&Shape) -> bool,
    /// The key for image `i`.
    pub key: fn(usize) -> String,
}

pub const CATEGORIES: &[Category] = &[
    Category {
        prefix: "emblem/fg",
        source: CacheType::MainMenu,
        images: 64,
        shape: Shape::square_pow2,
        key: |i| format!("emblem/fg/{i}"),
    },
    Category {
        prefix: "emblem/bg",
        source: CacheType::MainMenu,
        images: 32,
        shape: Shape::square_pow2,
        key: |i| format!("emblem/bg/{i}"),
    },
    Category {
        prefix: "rank",
        source: CacheType::MainMenu,
        images: 50,
        shape: |s| s.linear,
        // Ranks count from 1.
        key: |i| format!("rank/{}", i + 1),
    },
    Category {
        prefix: "live",
        source: CacheType::MainMenu,
        images: 32,
        shape: |s| s.linear && !s.square_pow2(),
        key: |i| format!("live/{i}"),
    },
    Category {
        prefix: "gametype",
        source: CacheType::MainMenu,
        images: 11,
        shape: |s| s.linear && s.width.abs_diff(s.height) <= 2,
        key: |i| format!("gametype/{i}"),
    },
    Category {
        prefix: "weapon",
        source: CacheType::Shared,
        images: 19,
        shape: |s| s.linear && s.width > s.height,
        key: |i| format!("weapon/{i}"),
    },
];

/// Friendlier names for some images, identified by eye on a retail disc:
/// `(prefix, [(name, index)])` gives `prefix/name` -> `prefix/index`. Indexes
/// not listed stay numeric only.
pub const ALIASES: &[(&str, &[(&str, usize)])] = &[
    (
        "gametype",
        &[
            ("slayer", 0),
            ("king_of_the_hill", 1),
            ("oddball", 3),
            ("juggernaut", 4),
            ("ctf", 6),
            ("assault", 7),
            ("territories", 8),
        ],
    ),
    (
        "weapon",
        &[
            ("energy_sword", 2),
            ("plasma_pistol", 3),
            ("plasma_rifle", 4),
            ("sniper_rifle", 5),
            ("rocket_launcher", 6),
            ("smg", 8),
            ("beam_rifle", 9),
            ("sentinel_beam", 12),
        ],
    ),
    (
        "medal",
        &[
            ("killing_spree", 0),
            ("running_riot", 1),
            ("rampage", 2),
            ("berserker", 3),
            ("overkill", 4),
            ("double_kill", 5),
            ("triple_kill", 6),
            ("killtacular", 7),
            ("kill_frenzy", 8),
            ("killtrocity", 9),
            ("killimanjaro", 10),
            ("sniper_kill", 11),
            ("splatter", 12),
            ("bone_cracker", 13),
            ("assassin", 14),
            ("stick_it", 15),
            ("flag_carrier_kill", 16),
            ("bomb_carrier_kill", 18),
            ("flag_returned", 25),
            ("bomb_returned", 26),
        ],
    ),
];

/// A bitmap tag as the picker sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub shapes: Vec<Shape>,
}

impl Candidate {
    fn area(&self) -> u64 {
        self.shapes
            .iter()
            .map(|s| u64::from(s.width) * u64::from(s.height))
            .max()
            .unwrap_or(0)
    }
}

/// The candidate that is `category`: the right image count, every image
/// the right shape, the largest of those. `None` if nothing fits.
pub fn pick(category: &Category, candidates: &[Candidate]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.shapes.len() == category.images && c.shapes.iter().all(category.shape))
        .max_by_key(|(i, c)| (c.area(), std::cmp::Reverse(*i)))
        .map(|(i, _)| i)
}

/// A map's preview among the bitmaps only that map has (`unique`): the one
/// single-image, linear, at-least-128-pixel picture. `None` unless exactly
/// one fits.
pub fn pick_preview(unique: &[Candidate]) -> Option<usize> {
    let mut fits = unique.iter().enumerate().filter(|(_, c)| {
        matches!(c.shapes.as_slice(), [s] if s.linear && s.width >= 128 && s.height >= 128)
    });
    let first = fits.next()?.0;
    fits.next().is_none().then_some(first)
}

/// Cuts a sprite out of an RGBA image. `bounds` are the tag's normalised
/// `[left, right, top, bottom]` (a hypothesis in the tag schema); the rect
/// is clamped to the image. `None` for an empty rect.
pub fn slice(
    rgba: &[u8],
    width: u32,
    height: u32,
    bounds: [f32; 4],
) -> Option<(u32, u32, Vec<u8>)> {
    let px = |v: f32, max: u32| ((v.clamp(0.0, 1.0) * max as f32).round() as u32).min(max);
    let (l, r) = (px(bounds[0], width), px(bounds[1], width));
    let (t, b) = (px(bounds[2], height), px(bounds[3], height));
    if r <= l || b <= t {
        return None;
    }
    let (w, h) = (r - l, b - t);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in t..b {
        let row = ((y * width + l) * 4) as usize;
        out.extend_from_slice(&rgba[row..row + (w * 4) as usize]);
    }
    Some((w, h, out))
}

/// The fonts' private-use icon range (docs/formats/fonts.md).
const ICONS: RangeInclusive<u16> = 0xe000..=0xe3ff;
/// Medal glyphs: 24 pixels tall, at least as wide.
pub const MEDALS: usize = 32;
/// Colour swatches: glyphs that are mostly one flat colour.
pub const PALETTE: usize = 18;

/// An icon glyph: its size and ARGB4444 pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icon {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u16>,
}

impl Icon {
    fn is_medal(&self) -> bool {
        self.height == 24 && self.width >= 24
    }

    /// The most common pixel and how many pixels have it.
    fn dominant(&self) -> (u16, usize) {
        let mut counts = HashMap::new();
        for &p in &self.pixels {
            *counts.entry(p).or_insert(0usize) += 1;
        }
        counts
            .into_iter()
            .max_by_key(|&(p, n)| (n, std::cmp::Reverse(p)))
            .unwrap_or_default()
    }

    /// A swatch: at least 80% of it is one colour (medals and buttons
    /// stay under 75%).
    fn is_swatch(&self) -> bool {
        !self.pixels.is_empty() && self.dominant().1 * 5 >= self.pixels.len() * 4
    }

    fn rgba(&self) -> Vec<u8> {
        self.pixels.iter().flat_map(|&p| argb4444(p)).collect()
    }
}

/// An ARGB4444 word as RGBA8.
fn argb4444(p: u16) -> [u8; 4] {
    let c = |shift: u16| (((p >> shift) & 15) * 17) as u8;
    [c(8), c(4), c(0), c(12)]
}

/// Glyph records drawn by exactly one code unit, in code-unit order. A
/// record shared by many units is the font's placeholder for unused ones.
pub fn own_records(units: &[(u16, u32)]) -> Vec<u32> {
    let mut uses: HashMap<u32, usize> = HashMap::new();
    for &(_, r) in units {
        *uses.entry(r).or_default() += 1;
    }
    units
        .iter()
        .map(|&(_, r)| r)
        .filter(|r| *r != 0 && uses[r] == 1)
        .collect()
}

fn icons(font: &font::Font) -> Vec<Icon> {
    let units: Vec<(u16, u32)> = ICONS.map(|u| (u, font.glyph_index(u))).collect();
    own_records(&units)
        .into_iter()
        .filter_map(|r| font.glyph_by_index(r).ok())
        .filter(|g| !g.pixels.is_empty())
        .map(|g| Icon {
            width: g.record.width.into(),
            height: g.record.height.into(),
            pixels: g.pixels,
        })
        .collect()
}

/// The medals among a font's icons, in code-unit order, if there are
/// exactly [`MEDALS`].
pub fn medals(icons: &[Icon]) -> Option<Vec<Piece>> {
    let found: Vec<Piece> = icons
        .iter()
        .filter(|i| i.is_medal())
        .enumerate()
        .map(|(n, i)| Piece {
            key: format!("medal/{n}"),
            width: i.width,
            height: i.height,
            rgba: i.rgba(),
        })
        .collect();
    (found.len() == MEDALS).then_some(found)
}

/// The colour palette (`#rrggbb`, 4-bit precision) from a font's swatches,
/// in code-unit order, if there are exactly [`PALETTE`].
pub fn palette(icons: &[Icon]) -> Option<Vec<String>> {
    let found: Vec<String> = icons
        .iter()
        .filter(|i| i.is_swatch())
        .map(|i| {
            let [r, g, b, _] = argb4444(i.dominant().0);
            format!("#{r:02x}{g:02x}{b:02x}")
        })
        .collect();
    (found.len() == PALETTE).then_some(found)
}

/// One picture in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Relative to the manifest, `/`-separated.
    pub file: String,
    pub width: u32,
    pub height: u32,
}

/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// [`MANIFEST_VERSION`].
    pub version: u32,
    /// [`EXTRACTOR_VERSION`] of the extraction that wrote it (the admin
    /// status shows it); 0 in manifests from before it was recorded.
    #[serde(default)]
    pub extractor: u32,
    /// The cache key of the disc it came from ([`cache_key`]).
    pub source: String,
    /// Semantic key -> picture.
    pub images: BTreeMap<String, Entry>,
    /// Friendlier keys for some of `images` (e.g. `gametype/slayer`).
    pub aliases: BTreeMap<String, String>,
    /// How to colour emblem layers.
    pub emblem: serde_json::Value,
    /// The player colour palette (`#rrggbb`), by colour index; empty if not
    /// found.
    #[serde(default)]
    pub palette: Vec<String>,
    /// What was looked for and not found or not decoded, by key or prefix.
    pub skipped: BTreeMap<String, String>,
    /// Per map stem (multiplayer and campaign), its level geometry and views
    /// (docs/live/live-art.md); absent when none were extracted.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub maps: BTreeMap<String, crate::live_geometry::MapEntry>,
    /// Player and weapon models (docs/live/live-art.md); absent when none were
    /// extracted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<crate::live_models::ModelsEntry>,
}

/// How emblem layers are coloured; see docs/live/live-api.md. Measured on the
/// disc: each layer is two masks, one in red (= green; drawn yellow) and
/// one in blue, and alpha copies the blue mask, so alpha is not coverage.
pub fn emblem_rules() -> serde_json::Value {
    serde_json::json!({
        "layers": {"background": "emblem/bg/<n>", "foreground": "emblem/fg/<n>"},
        "masks": {
            "a": "the red channel (green equals it; the pixels look yellow)",
            "b": "the blue channel (alpha equals it)"
        },
        "colour": "per pixel, with channels in 0..1: rgb = colour_a * red + colour_b * blue; ignore the image's alpha",
        "coverage": {"background": "opaque", "foreground": "max(red, blue); draw it over the background"},
        "colours": {
            "background": {"a": "background colour", "b": "secondary background colour"},
            "foreground": {"a": "primary colour", "b": "secondary colour"}
        },
        "palette": "profile colours are indexes into the manifest's palette",
        "confidence": "the masks are measured; which profile colour fills which mask is not confirmed"
    })
}

/// The manifest for `images`, with the aliases whose target exists.
pub fn build_manifest(
    source: String,
    images: BTreeMap<String, Entry>,
    palette: Vec<String>,
    skipped: BTreeMap<String, String>,
) -> Manifest {
    let aliases = ALIASES
        .iter()
        .flat_map(|(prefix, names)| {
            names
                .iter()
                .map(move |(name, i)| (format!("{prefix}/{name}"), format!("{prefix}/{i}")))
        })
        .filter(|(_, target)| images.contains_key(target))
        .collect();
    Manifest {
        version: MANIFEST_VERSION,
        extractor: EXTRACTOR_VERSION,
        source,
        images,
        aliases,
        emblem: emblem_rules(),
        palette,
        skipped,
        maps: BTreeMap::new(),
        models: None,
    }
}

/// The cache key of a disc: the extractor version and, for every map file,
/// its path, size, sector and first 2 KiB (the map header, which carries the
/// build and checksums). Cheap, and different for a different disc or a
/// re-mastered map.
pub fn cache_key(maps: &[(String, u32, u32, Vec<u8>)]) -> String {
    let mut h = Sha256::new();
    h.update(EXTRACTOR_VERSION.to_le_bytes());
    let mut sorted: Vec<_> = maps.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, size, sector, head) in sorted {
        h.update((path.len() as u32).to_le_bytes());
        h.update(path.as_bytes());
        h.update(size.to_le_bytes());
        h.update(sector.to_le_bytes());
        h.update((head.len() as u32).to_le_bytes());
        h.update(head);
    }
    live_proto::sign::hex(&h.finalize())
}

/// A map's cache type from its header bytes (`0x140`, docs/formats/map-format.md).
pub fn cache_type(head: &[u8]) -> Option<CacheType> {
    let word = head.get(0x140..0x144)?;
    Some(u32::from_le_bytes(word.try_into().ok()?).into())
}

/// A map file on the disc (or from a map pack) and its cache type.
pub(crate) struct MapFile {
    entry: xiso::Entry,
    cache_type: Option<CacheType>,
    /// Where it is when not on the retail disc: the host file and the
    /// map's first byte in it (`crate::live_mappack`).
    host: Option<(PathBuf, u64)>,
    /// What the map list says about a map pack map.
    pub(crate) pack: Option<crate::live_mappack::PackMap>,
    /// A converted Halo 1 map's name and picture (`crate::live_halo1`).
    pub(crate) halo1: Option<crate::live_halo1::Halo1Map>,
    /// An installed mod's package (`crate::live_mods`).
    pub(crate) package: Option<crate::live_mods::ModInfo>,
}

impl MapFile {
    pub(crate) fn cache_type(&self) -> Option<CacheType> {
        self.cache_type
    }

    /// A package map the game staged as content from `<data>/maps`
    /// (`boot::new_maps::MARKER` beside it), not one of the map pack's.
    fn is_staged(&self) -> bool {
        self.pack.is_some()
            && self
                .host
                .as_ref()
                .and_then(|(host, _)| host.parent())
                .is_some_and(|dir| dir.join(boot::new_maps::MARKER).is_file())
    }

    pub(crate) fn name(&self) -> &str {
        let file = self
            .entry
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&self.entry.path);
        file.strip_suffix(".map").unwrap_or(file)
    }
}

/// The disc as the extractor sees it: one directory walk, the map headers.
pub struct Disc {
    path: PathBuf,
    partition: u64,
    entries: Vec<xiso::Entry>,
    maps: Vec<MapFile>,
    /// Converted Halo 1 maps ([`Disc::add_halo1`]): kept apart from `maps`,
    /// so only the level geometry and previews read them.
    halo1: Vec<MapFile>,
    /// Installed mods ([`Disc::add_mods`]): apart from `maps` too.
    mods: Vec<MapFile>,
    /// What [`cache_key`] hashes, so added maps can join it.
    parts: Vec<(String, u32, u32, Vec<u8>)>,
    /// [`cache_key`] of the maps.
    pub key: String,
}

impl Disc {
    /// Walks the disc's directory once and reads each map's header.
    pub fn scan(path: &Path) -> Result<Self> {
        let mut iso = crate::open_iso(path)?;
        let partition = iso.partition_offset();
        let entries: Vec<xiso::Entry> = iso.entries()?.into_iter().filter(|e| !e.is_dir).collect();
        let mut f = File::open(path)?;
        let mut parts = Vec::new();
        let mut maps = Vec::new();
        for e in entries
            .iter()
            .filter(|e| e.path.to_ascii_lowercase().ends_with(".map"))
        {
            let mut head = vec![0u8; 2048.min(e.size as usize)];
            f.seek(SeekFrom::Start(
                partition + u64::from(e.sector) * xiso::SECTOR,
            ))?;
            f.read_exact(&mut head)?;
            maps.push(MapFile {
                entry: e.clone(),
                cache_type: cache_type(&head),
                host: None,
                pack: None,
                halo1: None,
                package: None,
            });
            parts.push((e.path.clone(), e.size, e.sector, head));
        }
        Ok(Disc {
            path: path.to_path_buf(),
            partition,
            entries,
            maps,
            halo1: Vec::new(),
            mods: Vec::new(),
            key: cache_key(&parts),
            parts,
        })
    }

    /// Adds map pack maps (`crate::live_mappack`): each from its own host
    /// file, opened with this disc's shared maps. A map whose stem the disc
    /// (or an earlier added map) already has is left out. The cache key
    /// then covers them too. Returns how many were added.
    pub(crate) fn add_maps(&mut self, found: Vec<crate::live_mappack::FoundMap>) -> usize {
        let mut n = 0;
        for f in found {
            let file = MapFile {
                entry: xiso::Entry {
                    path: format!("/{}.map", f.stem),
                    sector: 0,
                    size: f.size,
                    is_dir: false,
                },
                cache_type: cache_type(&f.head),
                host: Some((f.host.clone(), f.base)),
                pack: Some(f.pack.clone()),
                halo1: None,
                package: None,
            };
            if self
                .maps
                .iter()
                .chain(&self.halo1)
                .any(|m| m.name().eq_ignore_ascii_case(file.name()))
            {
                continue;
            }
            self.parts.push((
                format!("{}#{}", f.host.display(), f.stem),
                f.size,
                0,
                f.head,
            ));
            self.maps.push(file);
            n += 1;
        }
        self.key = cache_key(&self.parts);
        n
    }

    /// Adds converted Halo 1 maps (`crate::live_halo1`), each opened with
    /// this disc's shared maps like a map pack map. A stem the disc (or an
    /// earlier source) already has is left out, so a Halo 1 map never takes
    /// a Halo 2 map's name. The cache key then covers their headers, sizes
    /// and the import's stamp. Returns how many were added.
    pub(crate) fn add_halo1(&mut self, found: crate::live_halo1::Found) -> usize {
        let mut n = 0;
        for f in found.maps {
            // The game's copy of a converted multiplayer map, staged as
            // content beside the map pack's (`boot::new_maps`), is found by
            // `live_mappack::installed` too: the Halo 1 map replaces it.
            let staged: Vec<String> = self
                .maps
                .iter()
                .filter(|m| m.name().eq_ignore_ascii_case(&f.stem) && m.is_staged())
                .filter_map(|m| m.host.as_ref())
                .map(|(host, _)| format!("{}#{}", host.display(), f.stem))
                .collect();
            if !staged.is_empty() {
                self.maps
                    .retain(|m| !(m.name().eq_ignore_ascii_case(&f.stem) && m.is_staged()));
                self.parts.retain(|p| !staged.contains(&p.0));
            }
            let taken = |m: &MapFile| m.name().eq_ignore_ascii_case(&f.stem);
            if self.maps.iter().any(taken) || self.halo1.iter().any(taken) {
                continue;
            }
            self.parts.push((
                format!("{}#{}", f.path.display(), f.stem),
                f.size,
                0,
                f.head.clone(),
            ));
            self.halo1.push(MapFile {
                entry: xiso::Entry {
                    path: format!("/{}.map", f.stem),
                    sector: 0,
                    size: f.size,
                    is_dir: false,
                },
                cache_type: cache_type(&f.head),
                host: Some((f.path, 0)),
                pack: None,
                halo1: Some(f.info),
                package: None,
            });
            n += 1;
        }
        if n > 0 {
            if let Some(stamp) = found.stamp {
                self.parts.push(("halo1#stamp".into(), 0, 0, stamp));
            }
        }
        self.key = cache_key(&self.parts);
        n
    }

    /// The converted Halo 1 maps added ([`Disc::add_halo1`]).
    pub(crate) fn halo1_maps(&self) -> impl Iterator<Item = &MapFile> {
        self.halo1.iter()
    }

    /// Joins the installed mods' `MapId`s to the cache key
    /// ([`crate::live_mods::package_ids`]): each names its patched map's
    /// bytes (the base's SHA-256 and every layer's), so a new, changed or
    /// removed mod extracts again, and a mod that fails to resolve does
    /// not make every start extract again.
    pub(crate) fn key_mods(&mut self, ids: &[content::MapId]) {
        for id in ids {
            let part = format!("mod#{id}");
            if !self.parts.iter().any(|p| p.0 == part) {
                self.parts.push((part, 0, 0, Vec::new()));
            }
        }
        self.key = cache_key(&self.parts);
    }

    /// Adds installed mods (`crate::live_mods`), each opened with this
    /// disc's shared maps. Keyed by their stems (`mod-<MapId>`), never by
    /// the base map's name, so a mod never stands in for its base; their
    /// manifest entries carry the SHA-256 of the patched map. The cache
    /// key comes from [`Disc::key_mods`]. Returns how many were added.
    pub(crate) fn add_mods(&mut self, found: Vec<crate::live_mods::FoundMod>) -> usize {
        let mut n = 0;
        for f in found {
            if self.mods.iter().any(|m| m.name() == f.stem) {
                continue;
            }
            self.mods.push(MapFile {
                entry: xiso::Entry {
                    path: format!("/{}.map", f.stem),
                    sector: 0,
                    size: f.size,
                    is_dir: false,
                },
                cache_type: cache_type(&f.head),
                host: Some((f.path, 0)),
                pack: None,
                halo1: None,
                package: Some(f.info),
            });
            n += 1;
        }
        n
    }

    /// Resolves the mods installed under the data root `data` over this
    /// disc's maps and adds them ([`Disc::add_mods`]); offset-patched ones
    /// are written into `scratch`, which the caller removes once the art is
    /// extracted. Returns the notices of mods left out.
    pub(crate) fn add_installed_mods(&mut self, data: &Path, scratch: &Path) -> Vec<String> {
        let found = crate::live_mods::installed(data, scratch, &mut |name| {
            self.base_source(name)
                .map(|r| Box::new(r) as Box<dyn content::patch::Source>)
        });
        self.add_mods(found.mods);
        found.notices
    }

    /// The installed mods added ([`Disc::add_mods`]).
    pub(crate) fn mod_maps(&self) -> impl Iterator<Item = &MapFile> {
        self.mods.iter()
    }

    /// A map of the disc or the added map pack by name, as a patch source:
    /// the base a mod is laid over.
    pub(crate) fn base_source(&self, name: &str) -> Option<crate::live_mods::Region> {
        let m = self
            .maps
            .iter()
            .find(|m| m.name().eq_ignore_ascii_case(name))?;
        let (host, base) = match &m.host {
            Some((path, base)) => (path.clone(), *base),
            None => (self.path.clone(), self.base(&m.entry)),
        };
        crate::live_mods::Region::open(&host, base, m.entry.size.into()).ok()
    }

    /// The map pack maps added ([`Disc::add_maps`]).
    pub(crate) fn pack_maps(&self) -> impl Iterator<Item = &MapFile> {
        self.maps.iter().filter(|m| m.pack.is_some())
    }

    fn base(&self, e: &xiso::Entry) -> u64 {
        self.partition + u64::from(e.sector) * xiso::SECTOR
    }

    pub(crate) fn maps_of(&self, ty: CacheType) -> impl Iterator<Item = &MapFile> {
        self.maps.iter().filter(move |m| m.cache_type == Some(ty))
    }

    /// Opens a map with its shared maps (found by cache type) for pixel data.
    pub(crate) fn open(&self, m: &MapFile) -> Result<(Map<BufReader<File>>, ResourceFiles)> {
        let region = |e: &xiso::Entry| Region {
            path: self.path.clone(),
            base: self.base(e),
            length: e.size.into(),
        };
        let mut files = ResourceFiles::new();
        let (host, base) = match &m.host {
            Some((path, base)) => (path.clone(), *base),
            None => (self.path.clone(), self.base(&m.entry)),
        };
        let local = Region {
            path: host.clone(),
            base,
            length: m.entry.size.into(),
        };
        files.set(ResourceFile::Local, local)?;
        for (sel, ty) in [
            (ResourceFile::MainMenu, CacheType::MainMenu),
            (ResourceFile::Shared, CacheType::Shared),
            (
                ResourceFile::SinglePlayerShared,
                CacheType::SinglePlayerShared,
            ),
        ] {
            if let Some(s) = self.maps_of(ty).next() {
                files.set(sel, region(&s.entry))?;
            }
        }
        let map = Map::from_region(
            BufReader::new(File::open(&host)?),
            base,
            m.entry.size.into(),
        )
        .with_context(|| format!("parsing {}", m.entry.path))?;
        Ok((map, files))
    }

    /// A file's bytes, by path (case-insensitive).
    pub(crate) fn read(&self, path: &str) -> Result<Vec<u8>> {
        let e = self
            .entries
            .iter()
            .find(|e| e.path.eq_ignore_ascii_case(path))
            .with_context(|| format!("{path} is not on the disc"))?;
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(self.base(e)))?;
        let mut out = vec![0u8; e.size as usize];
        f.read_exact(&mut out)?;
        Ok(out)
    }
}

/// The manifest in `dir`, if it is from the disc with `key`.
pub fn cached(dir: &Path, key: &str) -> Option<Manifest> {
    let m: Manifest = serde_json::from_slice(&fs::read(dir.join("manifest.json")).ok()?).ok()?;
    (m.source == key).then_some(m)
}

/// How the art in a directory compares with what this build extracts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtAge {
    /// No readable manifest.
    Missing,
    /// Written by an older extractor (the version it recorded; 0 from
    /// before versions were recorded): it lacks what this build adds, e.g.
    /// skinned, animated player models.
    Stale(u32),
    Current,
}

/// The age of the art in `dir`, from its manifest's `extractor`.
pub fn art_age(dir: &Path) -> ArtAge {
    let Some(m) = fs::read(dir.join("manifest.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Manifest>(&b).ok())
    else {
        return ArtAge::Missing;
    };
    if m.extractor < EXTRACTOR_VERSION {
        ArtAge::Stale(m.extractor)
    } else {
        ArtAge::Current
    }
}

/// What to tell someone serving the art in `dir` when it is stale.
pub fn stale_warning(dir: &Path) -> Option<String> {
    match art_age(dir) {
        ArtAge::Stale(found) => Some(format!(
            "the game art in {} is from extractor v{found} and this h2 extracts \
             v{EXTRACTOR_VERSION}: the 3D viewer's player models won't animate or get \
             the newer materials until it is extracted again: \
             h2 live assets extract --disc <your halo2.iso> --out {}",
            dir.display(),
            dir.display()
        )),
        _ => None,
    }
}

/// A decoded picture waiting to be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub key: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// What one part of the extraction found.
#[derive(Default)]
struct Found {
    pieces: Vec<Piece>,
    palette: Vec<String>,
    skipped: BTreeMap<String, String>,
}

impl Found {
    fn skip(&mut self, key: impl Into<String>, why: impl Into<String>) {
        self.skipped.insert(key.into(), why.into());
    }
}

struct Tag {
    id: map::TagId,
    candidate: Candidate,
}

/// The bitmap tags of one map whose name passes `keep`, with their shapes.
/// Names are used only to compare maps with each other at run time.
fn bitmaps(map: &mut Map<BufReader<File>>, keep: impl Fn(&str) -> bool) -> Vec<Tag> {
    let ids: Vec<_> = map
        .tags()
        .iter()
        .filter(|t| t.group == BITM && t.location.is_some())
        .filter(|t| keep(t.name.as_deref().unwrap_or_default()))
        .map(|t| t.id)
        .collect();
    ids.into_iter()
        .filter_map(|id| {
            let TagData::Bitmap(b) = DecodedTag::read(map, id).ok()?.data else {
                return None;
            };
            let shapes = b
                .images
                .iter()
                .map(|im| Shape {
                    width: im.dimensions[0].into(),
                    height: im.dimensions[1].into(),
                    linear: im.is_linear(),
                })
                .collect();
            Some(Tag {
                id,
                candidate: Candidate { shapes },
            })
        })
        .collect()
}

/// The categories held by the map of cache type `source`.
fn category_art(disc: &Disc, source: CacheType) -> Found {
    let mut found = Found::default();
    let cats = CATEGORIES.iter().filter(|c| c.source == source);
    let opened = disc
        .maps_of(source)
        .next()
        .context("no such map on the disc")
        .and_then(|m| disc.open(m));
    let (mut map, mut files) = match opened {
        Ok(o) => o,
        Err(e) => {
            for c in cats {
                found.skip(c.prefix, format!("{e:#}"));
            }
            return found;
        }
    };
    let tags = bitmaps(&mut map, |_| true);
    let cands: Vec<Candidate> = tags.iter().map(|t| t.candidate.clone()).collect();
    for cat in cats {
        match pick(cat, &cands) {
            Some(i) => decode_tag(&mut map, &mut files, tags[i].id, cat.key, &mut found),
            None => found.skip(cat.prefix, "no bitmap of this shape on the disc"),
        }
    }
    found
}

/// A multiplayer map's preview: the picture only that map has. (The main
/// menu may carry one map's preview too, so only the shared map's tags,
/// `common`, are excluded.)
fn preview_art(disc: &Disc, m: &MapFile, common: &BTreeSet<String>) -> Found {
    let mut found = Found::default();
    let key = format!("map/{}/preview", m.name());
    let (mut map, mut files) = match disc.open(m) {
        Ok(o) => o,
        Err(e) => {
            found.skip(key, format!("{e:#}"));
            return found;
        }
    };
    let tags = bitmaps(&mut map, |name| !common.contains(name));
    let cands: Vec<Candidate> = tags.iter().map(|t| t.candidate.clone()).collect();
    match pick_preview(&cands) {
        Some(i) => {
            let k = key.clone();
            decode_tag(
                &mut map,
                &mut files,
                tags[i].id,
                move |_| k.clone(),
                &mut found,
            );
        }
        None => found.skip(key, "no single preview picture in this map"),
    }
    found
}

/// Campaign level-select pictures as `map/<stem>/preview`: the bitmap the
/// main menu's level select shows for each level (`live_catalog`).
fn campaign_previews(disc: &Disc) -> Found {
    let mut found = Found::default();
    let opened = disc
        .maps_of(CacheType::MainMenu)
        .next()
        .context("no main menu map on the disc")
        .and_then(|m| disc.open(m));
    let (mut map, mut files) = match opened {
        Ok(o) => o,
        Err(e) => {
            found.skip("map/campaign/preview", format!("{e:#}"));
            return found;
        }
    };
    let levels = match crate::live_catalog::read_campaign(&mut map) {
        Ok(l) => l,
        Err(e) => {
            found.skip("map/campaign/preview", format!("{e:#}"));
            return found;
        }
    };
    for l in levels {
        let Some(stem) = l.entry.key else { continue };
        let key = format!("map/{stem}/preview");
        match l.picture {
            Some(id) => decode_tag(
                &mut map,
                &mut files,
                map::TagId(id),
                move |_| key.clone(),
                &mut found,
            ),
            None => found.skip(key, "the level select has no picture for it"),
        }
    }
    found
}

/// Medals and the palette from the fonts, in the engine's font-table
/// order; each from the first font that has the full set.
fn font_art(disc: &Disc) -> Found {
    let mut found = Found::default();
    let (mut medal_set, mut colours) = (None, None);
    let table = disc.read("/maps/fonts/font_table.txt").unwrap_or_default();
    let mut seen = BTreeSet::new();
    for name in String::from_utf8_lossy(&table).lines().map(str::trim) {
        if medal_set.is_some() && colours.is_some() {
            break;
        }
        if name.is_empty() || !seen.insert(name.to_string()) {
            continue;
        }
        let Ok(data) = disc.read(&format!("/maps/fonts/{name}")) else {
            continue;
        };
        let Ok(font) = font::Font::parse(&data) else {
            continue;
        };
        let icons = icons(&font);
        if medal_set.is_none() {
            medal_set = medals(&icons);
        }
        if colours.is_none() {
            colours = palette(&icons);
        }
    }
    match medal_set {
        Some(p) => found.pieces = p,
        None => found.skip("medal", "no font with the medal icons"),
    }
    match colours {
        Some(c) => found.palette = c,
        None => found.skip("palette", "no font with the colour swatches"),
    }
    found
}

/// The executable's own pictures, as the original dashboard showed them:
/// the title image (`$$XTIMAGE`, the game's tile in the Games and Memory
/// screens) and the save image (`$$XSIMAGE`, the picture of a save that
/// has none of its own). Each section is an `XPR0` texture (`assets::xpr`);
/// the section names are the XDK's, not the game's.
pub const TITLE_IMAGES: [(&str, &str); 2] = [
    ("$$XTIMAGE", "title/image"),
    ("$$XSIMAGE", "title/save-image"),
];

fn title_art(disc: &Disc) -> Found {
    let mut found = Found::default();
    let bytes = match disc.read("/default.xbe") {
        Ok(b) => b,
        Err(e) => {
            for (_, key) in TITLE_IMAGES {
                found.skip(key, format!("{e:#}"));
            }
            return found;
        }
    };
    title_pieces(&bytes, &mut found);
    found
}

/// [`TITLE_IMAGES`] from an executable's bytes.
fn title_pieces(xbe_bytes: &[u8], found: &mut Found) {
    let xbe = match xbe::Xbe::parse(xbe_bytes) {
        Ok(x) => x,
        Err(e) => {
            for (_, key) in TITLE_IMAGES {
                found.skip(key, format!("default.xbe: {e}"));
            }
            return;
        }
    };
    for (section, key) in TITLE_IMAGES {
        let Some(s) = xbe.section(section) else {
            found.skip(key, format!("no {section} section"));
            continue;
        };
        match xbe
            .section_bytes(s)
            .map_err(anyhow::Error::from)
            .and_then(|b| assets::xpr::decode(b).map_err(anyhow::Error::from))
        {
            Ok(l) => found.pieces.push(Piece {
                key: key.to_string(),
                width: l.width,
                height: l.height,
                rgba: l.rgba,
            }),
            Err(e) => found.skip(key, format!("{section}: {e:#}")),
        }
    }
}

/// Decodes one tag's images (or its sprites, when it has any) under
/// `key(i)`, recording failures as skipped.
fn decode_tag(
    map: &mut Map<BufReader<File>>,
    files: &mut ResourceFiles,
    id: map::TagId,
    key: impl Fn(usize) -> String,
    found: &mut Found,
) {
    let bitmap = match DecodedTag::read(map, id).map(|t| t.data) {
        Ok(TagData::Bitmap(b)) => b,
        Ok(_) => return found.skip(key(0), "not a bitmap"),
        Err(e) => return found.skip(key(0), format!("tag read failed: {e}")),
    };
    let mut decoded = Vec::new();
    for (i, im) in bitmap.images.iter().enumerate() {
        decoded.push(match assets::decode_image(im, files) {
            Ok(d) => {
                let base = d.levels.into_iter().find(|l| l.level == 0 && l.face == 0);
                if base.is_none() {
                    found.skip(key(i), "no base level");
                }
                base.map(|l| (l.width, l.height, l.rgba))
            }
            Err(e) => {
                found.skip(key(i), format!("decode failed: {e}"));
                None
            }
        });
    }
    let sprites: Vec<_> = bitmap
        .sequences
        .iter()
        .flat_map(|s| s.sprites.iter())
        .collect();
    let piece = |i: usize, (width, height, rgba): (u32, u32, Vec<u8>)| Piece {
        key: key(i),
        width,
        height,
        rgba,
    };
    if sprites.is_empty() {
        let pieces = decoded.into_iter().enumerate();
        found
            .pieces
            .extend(pieces.filter_map(|(i, d)| Some(piece(i, d?))));
    } else {
        found
            .pieces
            .extend(sprites.iter().enumerate().filter_map(|(i, s)| {
                let (w, h, px) = decoded
                    .get(usize::try_from(s.image_index).ok()?)?
                    .as_ref()?;
                Some(piece(i, slice(px, *w, *h, s.bounds)?))
            }));
    }
}

/// Encodes and writes every piece under `dir`, spread over the CPUs.
pub fn write_pieces(pieces: &[Piece], dir: &Path) -> Result<BTreeMap<String, Entry>> {
    let parents: BTreeSet<PathBuf> = pieces
        .iter()
        .filter_map(|p| dir.join(&p.key).parent().map(Path::to_path_buf))
        .collect();
    for p in parents {
        fs::create_dir_all(&p).with_context(|| format!("creating {}", p.display()))?;
    }
    let write = |p: &Piece| -> Result<(String, Entry)> {
        let file = format!("{}.png", p.key);
        let mut png = Vec::new();
        assets::png::write_rgba_fast(&mut png, p.width, p.height, &p.rgba)
            .with_context(|| format!("encoding {}", p.key))?;
        fs::write(dir.join(&file), png).with_context(|| format!("writing {file}"))?;
        Ok((
            p.key.clone(),
            Entry {
                file,
                width: p.width,
                height: p.height,
            },
        ))
    };
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = pieces.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        let workers: Vec<_> = pieces
            .chunks(chunk)
            .map(|c| s.spawn(move || c.iter().map(write).collect::<Result<Vec<_>>>()))
            .collect();
        let mut out = BTreeMap::new();
        for w in workers {
            out.extend(w.join().map_err(|_| anyhow!("a PNG encoder panicked"))??);
        }
        Ok(out)
    })
}

/// Copies a converted Halo 1 map's PNG picture into `dir` as `<key>.png`.
fn halo1_preview(path: &Path, dir: &Path, key: &str) -> Result<Entry> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let (width, height) =
        crate::live_halo1::png_size(&bytes).with_context(|| format!("{} is no PNG", path.display()))?;
    let file = format!("{key}.png");
    let to = dir.join(&file);
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&to, bytes).with_context(|| format!("writing {file}"))?;
    Ok(Entry {
        file,
        width,
        height,
    })
}

/// What [`extract`] did.
pub struct Report {
    pub manifest: Manifest,
    /// Images written per key prefix (`emblem/fg`, `map`, ...).
    pub counts: BTreeMap<String, usize>,
    /// The playlist builder's catalog, written beside the manifest.
    pub catalog: crate::live_catalog::Catalog,
    /// Per-map geometry counts and timings.
    pub geometry: Vec<crate::live_geometry::MapStats>,
    /// Per model file: triangles and size.
    pub models: Vec<crate::live_models::ModelStats>,
    /// Where each biped came from and how it was posed.
    pub bipeds: Vec<crate::live_models::BipedInfo>,
}

/// Decodes every category from `disc` into `out` (replacing what was
/// there) and writes `manifest.json` last.
pub fn extract(disc: &Disc, out: &Path) -> Result<Report> {
    // The shared map's bitmap names, to tell a multiplayer map's own art.
    let common: BTreeSet<String> = match disc.maps_of(CacheType::Shared).next() {
        Some(m) => disc
            .open(m)?
            .0
            .tags()
            .iter()
            .filter(|t| t.group == BITM)
            .filter_map(|t| t.name.clone())
            .collect(),
        None => BTreeSet::new(),
    };
    let parts: Vec<Found> = std::thread::scope(|s| {
        let common = &common;
        let mut workers = vec![
            s.spawn(|| category_art(disc, CacheType::MainMenu)),
            s.spawn(|| category_art(disc, CacheType::Shared)),
            s.spawn(|| font_art(disc)),
            s.spawn(|| campaign_previews(disc)),
            s.spawn(|| title_art(disc)),
        ];
        workers.extend(
            disc.maps_of(CacheType::Multiplayer)
                .map(|m| s.spawn(move || preview_art(disc, m, common))),
        );
        workers
            .into_iter()
            .map(|w| {
                w.join()
                    .map_err(|_| anyhow!("an extractor thread panicked"))
            })
            .collect::<Result<_>>()
    })?;
    let (mut pieces, mut palette, mut skipped) = (Vec::new(), Vec::new(), BTreeMap::new());
    for f in parts {
        pieces.extend(f.pieces);
        palette.extend(f.palette);
        skipped.extend(f.skipped);
    }

    let staging = out.with_extension("partial");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).with_context(|| format!("creating {}", staging.display()))?;
    let mut images = write_pieces(&pieces, &staging)?;
    // Converted Halo 1 maps' pictures, as the import wrote them.
    for m in disc.halo1_maps() {
        let key = format!("map/{}/preview", m.name());
        let Some(path) = m.halo1.as_ref().and_then(|h| h.preview.as_ref()) else {
            skipped.insert(key, "the import wrote no picture for it".into());
            continue;
        };
        match halo1_preview(path, &staging, &key) {
            Ok(entry) => {
                images.insert(key, entry);
            }
            Err(e) => {
                skipped.insert(key, format!("{e:#}"));
            }
        }
    }
    // Level geometry and views of every multiplayer map (docs/live/live-art.md).
    let geometry = crate::live_geometry::extract(disc, &staging);
    skipped.extend(geometry.skipped);
    let mut manifest = build_manifest(disc.key.clone(), images, palette, skipped);
    manifest.maps = geometry.maps;
    // Player and weapon models (docs/live/live-art.md).
    let (models, bipeds) = crate::live_models::extract(disc, &staging);
    manifest.skipped.extend(models.skipped);
    manifest.models = models.entry;
    // The playlist builder's catalog rides along (docs/live/live-admin.md), with
    // the map pack's maps as their map list records name them.
    let mut catalog = crate::live_catalog::extract(disc);
    for m in disc.pack_maps() {
        let pack = m.pack.clone().unwrap_or_default();
        if catalog
            .maps
            .iter()
            .any(|e| e.key.as_deref() == Some(m.name()))
        {
            continue;
        }
        catalog.maps.push(crate::live_catalog::MapEntry {
            id: pack.id,
            name: if pack.name.is_empty() {
                m.name().to_string()
            } else {
                pack.name
            },
            description: pack.description,
            key: Some(m.name().to_string()),
            kind: None,
        });
    }
    fs::write(
        staging.join(crate::live_catalog::CATALOG),
        format!("{}\n", serde_json::to_string_pretty(&catalog)?),
    )?;
    fs::write(
        staging.join("manifest.json"),
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;
    // Packed for the web, as both homes send it (docs/live/live-art.md,
    // "Packing"): meshopt geometry, Brotli and gzip copies.
    let packed = live_server::art_pack::pack(&staging).context("packing the art")?;
    eprintln!(
        "packed: {} glbs {} -> {} KiB (meshopt), {} files {} -> {} KiB (Brotli)",
        packed.glbs,
        packed.glb_before / 1024,
        packed.glb_after / 1024,
        packed.precompressed,
        packed.plain / 1024,
        packed.brotli / 1024
    );
    if out.exists() {
        fs::remove_dir_all(out).with_context(|| format!("replacing {}", out.display()))?;
    }
    fs::rename(&staging, out).with_context(|| format!("moving the art into {}", out.display()))?;
    let mut counts = BTreeMap::new();
    for k in manifest.images.keys() {
        let prefix = match k.split('/').collect::<Vec<_>>().as_slice() {
            ["emblem", layer, _] => format!("emblem/{layer}"),
            [first, ..] => first.to_string(),
            [] => continue,
        };
        *counts.entry(prefix).or_default() += 1;
    }
    Ok(Report {
        manifest,
        counts,
        catalog,
        geometry: geometry.stats,
        models: models.stats,
        bipeds,
    })
}

/// `h2 live assets extract [--disc PATH] [--out DIR] [--mappack ISO]
/// [--installed DIR] [--halo1 ISO]`: the retail disc's art, the map pack's
/// maps from its disc and from where `h2 import-mappack` installed them
/// under the runtime data root `installed`, the Halo 1 maps `h2 import
/// halo1` converted there (`halo1` imports what is missing first), and the
/// mods installed there as map packages (`crate::live_mods`).
pub fn run(
    disc_path: PathBuf,
    out: PathBuf,
    mappack: Option<PathBuf>,
    installed: Option<PathBuf>,
    halo1: Option<PathBuf>,
) -> Result<()> {
    let mut disc = Disc::scan(&disc_path)?;
    let installed_root = installed.clone();
    if let Some(iso) = mappack {
        let found = crate::live_mappack::from_iso(&iso)
            .with_context(|| format!("reading the map pack disc {}", iso.display()))?;
        disc.add_maps(found);
    }
    if let Some(root) = installed {
        disc.add_maps(crate::live_mappack::installed(&root));
        if let Some(iso) = halo1 {
            // The same import `h2 play` runs: only what its stamp says is
            // missing or stale is converted.
            let mut say = |s: String| eprintln!("halo1: {s}");
            let sum = crate::h1_import::import(
                &crate::h1_import::Request {
                    h1_iso: &iso,
                    h2_disc: &disc_path,
                    data: &root,
                    force: false,
                    dry_run: false,
                },
                &mut say,
            )?;
            eprintln!(
                "halo1: {} converted, {} failed, {} already there",
                sum.converted, sum.failed, sum.skipped
            );
        }
        disc.add_halo1(crate::live_halo1::installed(&root));
    }
    // Installed mods, resolved over this disc's (and the map pack's) maps;
    // offset-patched ones are written beside the output while it runs.
    let scratch = out.with_extension("mods");
    let _ = fs::remove_dir_all(&scratch);
    if let Some(root) = &installed_root {
        disc.key_mods(&crate::live_mods::package_ids(root));
        for n in disc.add_installed_mods(root, &scratch) {
            eprintln!("mods: {n}");
        }
    }
    let report = extract(&disc, &out);
    let _ = fs::remove_dir_all(&scratch);
    let report = report?;
    println!("game art in {}", out.display());
    let pack: Vec<&str> = disc.pack_maps().map(|m| m.name()).collect();
    if !pack.is_empty() {
        println!("  {:<10} {}", "map pack", pack.join(", "));
    }
    let halo1: Vec<&str> = disc.halo1_maps().map(|m| m.name()).collect();
    if !halo1.is_empty() {
        println!("  {:<10} {}", "halo 1", halo1.join(", "));
    }
    for m in disc.mod_maps() {
        if let Some(p) = &m.package {
            println!("  {:<10} {} ({}, on {})", "mod", m.name(), p.name, p.base);
        }
    }
    for (prefix, n) in &report.counts {
        println!("  {prefix:<10} {n}");
    }
    println!("  {:<10} {}", "palette", report.manifest.palette.len());
    let tris: usize = report.geometry.iter().map(|g| g.triangles).sum();
    println!(
        "  {:<10} {} maps, {tris} triangles",
        "geometry",
        report.manifest.maps.len()
    );
    for g in &report.geometry {
        println!(
            "    {:<14} {:>7} triangles, {:>5} KiB",
            g.stem,
            g.triangles,
            g.glb_bytes / 1024
        );
    }
    println!("  {:<10} {} files", "models", report.models.len());
    for b in &report.bipeds {
        println!(
            "    {:<14} {} ({}), posed by {}",
            b.species, b.tag, b.map, b.animation
        );
    }
    for m in &report.models {
        println!(
            "    {:<28} {:>7} triangles, {:>5} KiB",
            m.key,
            m.triangles,
            m.glb_bytes / 1024
        );
    }
    let c = &report.catalog;
    println!(
        "catalog: {} maps, {} playlists, {} variants",
        c.maps.len(),
        c.playlists.len(),
        c.variants.len()
    );
    for (k, why) in &c.skipped {
        println!("  catalog {k}: {why}");
    }
    if !report.manifest.skipped.is_empty() {
        println!("skipped:");
        for (k, why) in &report.manifest.skipped {
            println!("  {k}: {why}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(n: usize, w: u32, h: u32, linear: bool) -> Candidate {
        Candidate {
            shapes: vec![
                Shape {
                    width: w,
                    height: h,
                    linear
                };
                n
            ],
        }
    }

    fn category(prefix: &str) -> &'static Category {
        CATEGORIES.iter().find(|c| c.prefix == prefix).unwrap()
    }

    #[test]
    fn categories_pick_by_count_and_shape() {
        // Three sizes of one set, plus look-alikes.
        let cands = vec![
            cand(32, 32, 32, false),
            cand(32, 128, 128, false),
            cand(32, 28, 28, true),
            cand(32, 64, 64, false),
            cand(32, 14, 14, true),
            cand(50, 28, 26, true),
            cand(50, 14, 13, true),
            cand(11, 40, 40, true),
        ];
        assert_eq!(
            pick(category("emblem/bg"), &cands),
            Some(1),
            "the largest square set"
        );
        assert_eq!(pick(category("live"), &cands), Some(2));
        assert_eq!(pick(category("rank"), &cands), Some(5));
        assert_eq!(pick(category("gametype"), &cands), Some(7));
        assert_eq!(pick(category("emblem/fg"), &cands), None);
        assert_eq!(
            pick(category("weapon"), &[cand(19, 40, 40, true)]),
            None,
            "weapons are wide"
        );
        assert_eq!(
            pick(category("weapon"), &[cand(19, 103, 19, true)]),
            Some(0)
        );
        // A mixed set is not a match.
        let mut mixed = cand(11, 40, 40, true);
        mixed.shapes[3] = Shape {
            width: 256,
            height: 128,
            linear: false,
        };
        assert_eq!(pick(category("gametype"), &[mixed]), None);
    }

    #[test]
    fn art_from_an_older_extractor_is_stale() {
        let dir = std::env::temp_dir().join(format!("h2-art-age-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(art_age(&dir), ArtAge::Missing);
        assert_eq!(stale_warning(&dir), None);
        let write = |extractor: Option<u32>| {
            let m = build_manifest("k".into(), BTreeMap::new(), Vec::new(), BTreeMap::new());
            let mut v = serde_json::to_value(&m).unwrap();
            match extractor {
                Some(e) => v["extractor"] = e.into(),
                None => drop(v.as_object_mut().unwrap().remove("extractor")),
            }
            fs::write(dir.join("manifest.json"), serde_json::to_vec(&v).unwrap()).unwrap();
        };
        write(Some(EXTRACTOR_VERSION));
        assert_eq!(art_age(&dir), ArtAge::Current);
        write(Some(EXTRACTOR_VERSION - 1));
        assert_eq!(art_age(&dir), ArtAge::Stale(EXTRACTOR_VERSION - 1));
        let warning = stale_warning(&dir).unwrap();
        assert!(warning.contains("h2 live assets extract"), "{warning}");
        assert!(warning.contains(&dir.display().to_string()), "{warning}");
        // From before the version was recorded.
        write(None);
        assert_eq!(art_age(&dir), ArtAge::Stale(0));
        fs::write(dir.join("manifest.json"), b"not json").unwrap();
        assert_eq!(art_age(&dir), ArtAge::Missing);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys() {
        assert_eq!((category("rank").key)(0), "rank/1");
        assert_eq!((category("rank").key)(49), "rank/50");
        assert_eq!((category("emblem/fg").key)(3), "emblem/fg/3");
    }

    #[test]
    fn previews_need_exactly_one_picture() {
        let pic = cand(1, 220, 207, true);
        assert_eq!(
            pick_preview(&[cand(5, 64, 64, false), pic.clone(), cand(1, 4, 4, false)]),
            Some(1)
        );
        assert_eq!(pick_preview(&[pic.clone(), pic.clone()]), None, "ambiguous");
        assert_eq!(
            pick_preview(&[cand(1, 128, 128, false)]),
            None,
            "swizzled textures are not UI art"
        );
        assert_eq!(pick_preview(&[]), None);
    }

    #[test]
    fn sprites_slice_out_of_their_sheet() {
        // 4x2 image, pixel value = its index.
        let rgba: Vec<u8> = (0..8u8).flat_map(|i| [i, i, i, 255]).collect();
        let (w, h, px) = slice(&rgba, 4, 2, [0.5, 1.0, 0.0, 1.0]).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(px.chunks(4).map(|p| p[0]).collect::<Vec<_>>(), [2, 3, 6, 7]);
        let (w, h, px) = slice(&rgba, 4, 2, [0.0, 0.25, 0.5, 1.0]).unwrap();
        assert_eq!((w, h, px[0]), (1, 1, 4));
        assert!(slice(&rgba, 4, 2, [0.5, 0.5, 0.0, 1.0]).is_none());
        // Out-of-range bounds clamp.
        assert_eq!(slice(&rgba, 4, 2, [-1.0, 2.0, 0.0, 1.0]).unwrap().0, 4);
    }

    #[test]
    fn the_cache_key_follows_the_disc() {
        let maps = vec![
            ("/maps/a.map".to_string(), 10, 5, vec![1u8, 2]),
            ("/maps/b.map".to_string(), 20, 9, vec![3]),
        ];
        let key = cache_key(&maps);
        assert_eq!(key.len(), 64);
        let mut reordered = maps.clone();
        reordered.reverse();
        assert_eq!(cache_key(&reordered), key, "order does not matter");
        for change in 0..3 {
            let mut m = maps.clone();
            match change {
                0 => m[0].1 += 1,
                1 => m[1].3[0] ^= 1,
                _ => m[0].0.push('x'),
            }
            assert_ne!(cache_key(&m), key);
        }
    }

    /// An invented cache header: the cache type at `0x140`.
    fn head(cache_type: u32) -> Vec<u8> {
        let mut h = vec![0u8; 2048];
        h[0x140..0x144].copy_from_slice(&cache_type.to_le_bytes());
        h
    }

    fn halo1_found(stem: &str, cache_type: u32, stamp: &[u8]) -> crate::live_halo1::Found {
        crate::live_halo1::Found {
            maps: vec![crate::live_halo1::FoundMap {
                stem: stem.into(),
                path: PathBuf::from(format!("/nowhere/{stem}.map")),
                size: 4096,
                head: head(cache_type),
                kind: crate::live_geometry::MapKind::Multiplayer,
                info: crate::live_halo1::Halo1Map {
                    name: "Invented".into(),
                    preview: None,
                },
            }],
            stamp: Some(stamp.to_vec()),
        }
    }

    fn empty_disc() -> Disc {
        Disc {
            path: PathBuf::from("/nowhere.iso"),
            partition: 0,
            entries: Vec::new(),
            maps: Vec::new(),
            halo1: Vec::new(),
            mods: Vec::new(),
            parts: Vec::new(),
            key: cache_key(&[]),
        }
    }

    #[test]
    fn halo1_maps_join_the_disc_apart_and_key_it_with_the_stamp() {
        let mut disc = empty_disc();
        disc.maps.push(MapFile {
            entry: xiso::Entry {
                path: "/maps/teacup.map".into(),
                sector: 0,
                size: 1,
                is_dir: false,
            },
            cache_type: Some(CacheType::Multiplayer),
            host: None,
            pack: None,
            halo1: None,
            package: None,
        });
        let before = disc.key.clone();
        assert_eq!(disc.add_halo1(halo1_found("teacup", 1, b"s1")), 0, "a Halo 2 stem is kept");
        assert_eq!(disc.add_halo1(halo1_found("h1_alpha", 1, b"s1")), 1);
        assert_eq!(disc.add_halo1(halo1_found("h1_alpha", 1, b"s1")), 0, "once");
        let h1: Vec<_> = disc.halo1_maps().map(|m| m.name().to_string()).collect();
        assert_eq!(h1, ["h1_alpha"]);
        assert_eq!(disc.maps_of(CacheType::Multiplayer).count(), 1, "not among the disc's maps");
        assert_eq!(disc.halo1_maps().next().unwrap().cache_type(), Some(CacheType::Multiplayer));
        assert_ne!(disc.key, before);

        let mut other = empty_disc();
        other.add_halo1(halo1_found("h1_alpha", 1, b"s2"));
        let mut same = empty_disc();
        same.add_halo1(halo1_found("h1_alpha", 1, b"s1"));
        assert_ne!(other.key, same.key, "another import stamp extracts again");
    }

    fn found_mod(id: u8, base: &str) -> crate::live_mods::FoundMod {
        let map_id = content::MapId([id; 32]);
        crate::live_mods::FoundMod {
            stem: crate::live_mods::mod_stem(&map_id),
            path: PathBuf::from(format!("/nowhere/{id}.map")),
            size: 4096,
            head: head(1),
            info: crate::live_mods::ModInfo {
                name: format!("Mod {id}"),
                map_id,
                base: base.into(),
                sha256: content::Sha256([id; 32]),
                image: None,
            },
        }
    }

    #[test]
    fn mods_join_the_disc_by_map_id_and_never_take_their_base_name() {
        let mut disc = empty_disc();
        disc.maps.push(MapFile {
            entry: xiso::Entry {
                path: "/maps/teacup.map".into(),
                sector: 0,
                size: 1,
                is_dir: false,
            },
            cache_type: Some(CacheType::Multiplayer),
            host: None,
            pack: None,
            halo1: None,
            package: None,
        });
        let before = disc.key.clone();
        disc.key_mods(&[content::MapId([1; 32]), content::MapId([2; 32])]);
        let keyed = disc.key.clone();
        assert_ne!(keyed, before, "installed mods join the key");
        disc.key_mods(&[content::MapId([1; 32])]);
        assert_eq!(disc.key, keyed, "once each");
        assert_eq!(disc.add_mods(vec![found_mod(1, "teacup"), found_mod(2, "teacup")]), 2);
        assert_eq!(disc.add_mods(vec![found_mod(1, "teacup")]), 0, "once");
        assert_eq!(disc.key, keyed, "resolving does not change the key");
        let stems: Vec<_> = disc.mod_maps().map(|m| m.name().to_string()).collect();
        assert_eq!(stems, ["mod-0101010101010101", "mod-0202020202020202"]);
        assert_eq!(disc.maps_of(CacheType::Multiplayer).count(), 1, "the base stays the disc's");
        assert_eq!(disc.maps_of(CacheType::Multiplayer).next().unwrap().name(), "teacup");
        let m = disc.mod_maps().next().unwrap();
        assert_eq!(m.cache_type(), Some(CacheType::Multiplayer));
        assert_eq!(m.package.as_ref().unwrap().base, "teacup");

        let mut changed = empty_disc();
        changed.key_mods(&[content::MapId([1; 32]), content::MapId([3; 32])]);
        let mut same = empty_disc();
        same.key_mods(&[content::MapId([1; 32]), content::MapId([2; 32])]);
        assert_ne!(changed.key, same.key, "a changed mod extracts again");
    }

    #[test]
    fn a_staged_copy_of_a_halo1_map_gives_way_to_it() {
        let dir = std::env::temp_dir().join(format!("h2-art-staged-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(boot::new_maps::MARKER), "map = \"h1_alpha\"\n").unwrap();
        let mut disc = empty_disc();
        disc.add_maps(vec![crate::live_mappack::FoundMap {
            stem: "h1_alpha".into(),
            host: dir.join("h1_alpha.map"),
            base: 0,
            size: 4096,
            head: head(1),
            pack: Default::default(),
        }]);
        assert_eq!(disc.pack_maps().count(), 1);
        assert_eq!(disc.add_halo1(halo1_found("h1_alpha", 1, b"s")), 1);
        assert_eq!(disc.pack_maps().count(), 0);
        assert_eq!(disc.halo1_maps().count(), 1);
        assert!(!disc.parts.iter().any(|p| p.0.starts_with(&dir.display().to_string())));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn manifests_alias_what_exists_and_round_trip() {
        let mut images = BTreeMap::new();
        for k in ["gametype/0", "gametype/3", "medal/5", "weapon/2"] {
            images.insert(
                k.to_string(),
                Entry {
                    file: format!("{k}.png"),
                    width: 40,
                    height: 40,
                },
            );
        }
        let m = build_manifest("k".into(), images, vec!["#ffffff".into()], BTreeMap::new());
        assert_eq!(
            m.aliases.get("gametype/slayer").map(String::as_str),
            Some("gametype/0")
        );
        assert_eq!(
            m.aliases.get("gametype/oddball").map(String::as_str),
            Some("gametype/3")
        );
        assert_eq!(m.aliases["medal/double_kill"], "medal/5");
        assert_eq!(m.aliases["weapon/energy_sword"], "weapon/2");
        assert_eq!(m.palette, ["#ffffff"]);
        assert!(
            !m.aliases.contains_key("gametype/ctf"),
            "no alias to a missing image"
        );
        let dir = std::env::temp_dir().join(format!("h2-art-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
        assert_eq!(cached(&dir, "k"), Some(m));
        assert_eq!(cached(&dir, "other"), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    fn icon(width: u32, height: u32, fill: u16, flat_share: f32) -> Icon {
        let n = (width * height) as usize;
        let flat = (n as f32 * flat_share) as usize;
        let pixels = (0..n)
            .map(|i| if i < flat { fill } else { i as u16 })
            .collect();
        Icon {
            width,
            height,
            pixels,
        }
    }

    #[test]
    fn the_placeholder_record_is_not_an_icon() {
        let units = [
            (0xe000, 0),
            (0xe001, 7),
            (0xe002, 9),
            (0xe003, 7),
            (0xe004, 3),
        ];
        assert_eq!(own_records(&units), [9, 3], "7 is shared, 0 is unmapped");
    }

    #[test]
    fn medals_are_the_24_pixel_icons() {
        let mut icons: Vec<Icon> = (0..MEDALS)
            .map(|i| icon(if i % 8 == 0 { 32 } else { 24 }, 24, 0xf123, 0.3))
            .collect();
        icons.insert(3, icon(24, 23, 0xf000, 0.3));
        icons.insert(9, icon(16, 24, 0xf000, 0.3));
        let m = medals(&icons).unwrap();
        assert_eq!(m.len(), MEDALS);
        assert_eq!((m[0].key.as_str(), m[0].width), ("medal/0", 32));
        assert_eq!((m[1].key.as_str(), m[1].width), ("medal/1", 24));
        assert_eq!(m[5].rgba.len(), 24 * 24 * 4);
        assert_eq!(
            &m[5].rgba[..4],
            &[0x11, 0x22, 0x33, 0xff],
            "ARGB4444 to RGBA8"
        );
        icons.pop();
        assert!(medals(&icons).is_none(), "a partial set is not the medals");
    }

    #[test]
    fn the_palette_is_the_flat_swatches() {
        let mut icons: Vec<Icon> = (0..PALETTE as u16)
            .map(|i| icon(43, 22, 0xf000 | i, 0.82))
            .collect();
        icons.push(icon(24, 24, 0xf0f0, 0.72));
        icons.insert(0, icon(16, 18, 0xffff, 0.5));
        let p = palette(&icons).unwrap();
        assert_eq!(p.len(), PALETTE);
        assert_eq!((p[0].as_str(), p[17].as_str()), ("#000000", "#001111"));
        icons.truncate(5);
        assert!(palette(&icons).is_none());
    }

    #[test]
    fn the_cache_type_is_read_from_the_header() {
        let mut head = vec![0u8; 0x800];
        head[0x140] = 1;
        assert_eq!(cache_type(&head), Some(CacheType::Multiplayer));
        head[0x140] = 2;
        assert_eq!(cache_type(&head), Some(CacheType::MainMenu));
        assert_eq!(cache_type(&head[..0x100]), None, "short header");
    }

    #[test]
    fn pieces_are_written_deterministically() {
        let pieces: Vec<Piece> = (0..9u8)
            .map(|i| Piece {
                key: format!("medal/{i}"),
                width: 2,
                height: 1,
                rgba: vec![i; 8],
            })
            .chain([Piece {
                key: "map/x/preview".into(),
                width: 1,
                height: 1,
                rgba: vec![9; 4],
            }])
            .collect();
        let dir = std::env::temp_dir().join(format!("h2-art-pieces-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let entries = write_pieces(&pieces, &dir).unwrap();
        assert_eq!(entries.len(), 10);
        assert_eq!(
            entries["map/x/preview"],
            Entry {
                file: "map/x/preview.png".into(),
                width: 1,
                height: 1
            }
        );
        let first = fs::read(dir.join("medal/4.png")).unwrap();
        assert_eq!(&first[1..4], b"PNG");
        write_pieces(&pieces, &dir).unwrap();
        assert_eq!(fs::read(dir.join("medal/4.png")).unwrap(), first);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// An invented XPR0 of one `side`x`side` DXT1 texture, every block
    /// colour 0 = `c565`.
    fn xpr(log_side: u32, c565: u16) -> Vec<u8> {
        let side = 1u32 << log_side;
        let data = (side.div_ceil(4) * side.div_ceil(4) * 8) as usize;
        let mut out = vec![0u8; 0x40 + data];
        out[..4].copy_from_slice(b"XPR0");
        let format = (log_side << 24) | (log_side << 20) | (1 << 16) | (0x0c << 8) | (2 << 4) | 1;
        for (i, v) in [
            (0x40 + data) as u32,
            0x40,
            0x0004_0001,
            0,
            0,
            format,
            0,
            u32::MAX,
        ]
        .iter()
        .enumerate()
        {
            out[4 + i * 4..8 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        for c in out[0x40..].chunks_mut(8) {
            c[..2].copy_from_slice(&c565.to_le_bytes());
        }
        out
    }

    /// An invented executable: a code section and the two image sections.
    fn xbe_with_images(title: &[u8], save: &[u8]) -> Vec<u8> {
        let base = 0x10000u32;
        let mut d = vec![0u8; 0x1000];
        d[..4].copy_from_slice(b"XBEH");
        let put = |d: &mut Vec<u8>, o: usize, v: u32| d[o..o + 4].copy_from_slice(&v.to_le_bytes());
        for (o, v) in [
            (0x104, base),
            (0x108, 0x1000),
            (0x118, base + 0x800),
            (0x11c, 3),
            (0x120, base + 0x200),
            (0x128, 0x11000 ^ 0xa8fc57ab),
        ] {
            put(&mut d, o, v);
        }
        let sections = [
            (".text", 0x06u32, vec![0xccu8; 0x10]),
            ("$$XTIMAGE", 0, title.to_vec()),
            ("$$XSIMAGE", 0, save.to_vec()),
        ];
        let mut va = 0x11000u32;
        for (i, (name, flags, bytes)) in sections.iter().enumerate() {
            let raw = d.len() as u32;
            let h = 0x200 + i * 56;
            let name_at = 0x400 + i * 16;
            d[name_at..name_at + name.len()].copy_from_slice(name.as_bytes());
            for (o, v) in [
                (0, *flags),
                (4, va),
                (8, bytes.len() as u32),
                (12, raw),
                (16, bytes.len() as u32),
                (20, base + name_at as u32),
            ] {
                put(&mut d, h + o, v);
            }
            d.extend_from_slice(bytes);
            va += (bytes.len() as u32).next_multiple_of(0x1000);
        }
        d
    }

    #[test]
    fn title_and_save_images_come_from_the_executable() {
        let mut found = Found::default();
        title_pieces(
            &xbe_with_images(&xpr(7, 0xF800), &xpr(6, 0x001F)),
            &mut found,
        );
        assert!(found.skipped.is_empty(), "{:?}", found.skipped);
        let by: BTreeMap<_, _> = found.pieces.iter().map(|p| (p.key.as_str(), p)).collect();
        let t = by["title/image"];
        assert_eq!((t.width, t.height), (128, 128));
        assert_eq!(&t.rgba[..4], &[255, 0, 0, 255]);
        let s = by["title/save-image"];
        assert_eq!((s.width, s.height), (64, 64));
        assert_eq!(&s.rgba[..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn a_missing_or_broken_image_section_is_skipped_not_fatal() {
        let mut found = Found::default();
        title_pieces(&xbe_with_images(&xpr(7, 0xF800), b"not an xpr"), &mut found);
        assert_eq!(found.pieces.len(), 1);
        assert!(found.skipped["title/save-image"].contains("$$XSIMAGE"));
        let mut found = Found::default();
        title_pieces(b"not an xbe", &mut found);
        assert!(found.pieces.is_empty());
        assert_eq!(found.skipped.len(), 2);
    }

    /// The disc's title and save images, written outside the repository
    /// (`$H2_TITLE_CHECK`) for review.
    #[test]
    #[ignore = "needs the disc (H2_DISC) and H2_TITLE_CHECK"]
    fn title_images_from_the_disc() {
        let disc = Disc::scan(Path::new(&std::env::var("H2_DISC").expect("H2_DISC"))).unwrap();
        let out = std::env::var("H2_TITLE_CHECK").expect("H2_TITLE_CHECK");
        let found = title_art(&disc);
        assert!(found.skipped.is_empty(), "{:?}", found.skipped);
        write_pieces(&found.pieces, Path::new(&out)).unwrap();
        for p in &found.pieces {
            println!("{} {}x{}", p.key, p.width, p.height);
        }
    }

    /// The campaign level-select pictures from the owner's disc, written
    /// outside the repository (`$H2_PREVIEW_CHECK`, else the default below)
    /// for review.
    #[test]
    #[ignore = "needs the disc (H2_DISC)"]
    fn campaign_previews_from_the_disc() {
        let path = std::env::var("H2_DISC").expect("H2_DISC");
        let out = std::env::var("H2_PREVIEW_CHECK").unwrap_or_else(|_| {
            "/Users/afr/.config/rush/keep/h2recomp-b5afe71a/bed6c8dc/campaign-geometry/previews"
                .into()
        });
        let disc = Disc::scan(Path::new(&path)).unwrap();
        let found = campaign_previews(&disc);
        let _ = fs::remove_dir_all(&out);
        write_pieces(&found.pieces, Path::new(&out)).unwrap();
        for p in &found.pieces {
            println!("{} {}x{}", p.key, p.width, p.height);
        }
        println!("skipped {:?}", found.skipped);
        let campaign = disc.maps_of(CacheType::Campaign).count();
        assert_eq!(found.pieces.len() + found.skipped.len(), campaign);
    }
}
