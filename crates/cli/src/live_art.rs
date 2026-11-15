//! `h2 live assets extract`: the stats site's art from the owner's own disc
//! (docs/live.md, "Game art"; docs/live-api.md, "Optional game art").
//!
//! No game asset is named here (CONTRIBUTING.md; docs/bitmaps.md: "No pixels
//! or names of game assets are stored in the repo"). Each category is found
//! by structure at run time: which kind of map holds it (the map header's
//! cache type), how many images the bitmap tag has, and their shape.
//! Multiplayer map previews are the one picture-sized UI image a
//! multiplayer map carries that the shared map does not. Medals and the
//! colour palette are icon glyphs in the fonts' private-use range
//! (docs/fonts.md). Everything found is decoded to PNG (RGBA) under semantic
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
pub const EXTRACTOR_VERSION: u32 = 2;
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

/// The fonts' private-use icon range (docs/fonts.md).
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
}

/// How emblem layers are coloured; see docs/live-api.md. Measured on the
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
        source,
        images,
        aliases,
        emblem: emblem_rules(),
        palette,
        skipped,
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

/// A map's cache type from its header bytes (`0x140`, docs/map-format.md).
pub fn cache_type(head: &[u8]) -> Option<CacheType> {
    let word = head.get(0x140..0x144)?;
    Some(u32::from_le_bytes(word.try_into().ok()?).into())
}

/// A map file on the disc and its cache type.
struct MapFile {
    entry: xiso::Entry,
    cache_type: Option<CacheType>,
}

impl MapFile {
    fn name(&self) -> &str {
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
            });
            parts.push((e.path.clone(), e.size, e.sector, head));
        }
        Ok(Disc {
            path: path.to_path_buf(),
            partition,
            entries,
            maps,
            key: cache_key(&parts),
        })
    }

    fn base(&self, e: &xiso::Entry) -> u64 {
        self.partition + u64::from(e.sector) * xiso::SECTOR
    }

    fn maps_of(&self, ty: CacheType) -> impl Iterator<Item = &MapFile> {
        self.maps.iter().filter(move |m| m.cache_type == Some(ty))
    }

    /// Opens a map with its shared maps (found by cache type) for pixel data.
    fn open(&self, m: &MapFile) -> Result<(Map<BufReader<File>>, ResourceFiles)> {
        let region = |e: &xiso::Entry| Region {
            path: self.path.clone(),
            base: self.base(e),
            length: e.size.into(),
        };
        let mut files = ResourceFiles::new();
        files.set(ResourceFile::Local, region(&m.entry))?;
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
            BufReader::new(File::open(&self.path)?),
            self.base(&m.entry),
            m.entry.size.into(),
        )
        .with_context(|| format!("parsing {}", m.entry.path))?;
        Ok((map, files))
    }

    /// A file's bytes, by path (case-insensitive).
    fn read(&self, path: &str) -> Result<Vec<u8>> {
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

/// What [`extract`] did.
pub struct Report {
    pub manifest: Manifest,
    /// Images written per key prefix (`emblem/fg`, `map`, ...).
    pub counts: BTreeMap<String, usize>,
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
    let images = write_pieces(&pieces, &staging)?;
    let manifest = build_manifest(disc.key.clone(), images, palette, skipped);
    fs::write(
        staging.join("manifest.json"),
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;
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
    Ok(Report { manifest, counts })
}

/// `h2 live assets extract [--disc PATH] [--out DIR]`.
pub fn run(disc: PathBuf, out: PathBuf) -> Result<()> {
    let report = extract(&Disc::scan(&disc)?, &out)?;
    println!("game art in {}", out.display());
    for (prefix, n) in &report.counts {
        println!("  {prefix:<10} {n}");
    }
    println!("  {:<10} {}", "palette", report.manifest.palette.len());
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
}
