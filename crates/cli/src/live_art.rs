//! `h2 live assets extract`: the stats site's art from the owner's own disc
//! (docs/live.md, "Game art"; docs/live-api.md, "Optional game art").
//!
//! No game asset is named here (CONTRIBUTING.md; docs/bitmaps.md: "No pixels
//! or names of game assets are stored in the repo"). Each category is found
//! by structure at run time: which shared map holds it, how many images the
//! bitmap tag has, and their shape. Multiplayer map previews are the one
//! picture-sized UI image a multiplayer map carries that the shared map
//! does not. Everything found is decoded to PNG (RGBA) under semantic keys and
//! listed in `manifest.json`; output is deterministic for a given disc.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use map::{CacheType, FourCC};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tags::{DecodedTag, TagData};

/// Bumped when the output for a given disc would change.
pub const EXTRACTOR_VERSION: u32 = 1;

const BITM: FourCC = FourCC(*b"bitm");

/// Where a category's tag lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    MainMenu,
    Shared,
}

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

/// A category: the key prefix, where it lives, how many images its tag has,
/// and what each image looks like. Among matching tags the one with the
/// largest images wins (the others are smaller copies).
pub struct Category {
    pub prefix: &'static str,
    pub source: Source,
    pub images: usize,
    pub shape: fn(&Shape) -> bool,
    /// The key for image `i`.
    pub key: fn(usize) -> String,
}

pub const CATEGORIES: &[Category] = &[
    Category {
        prefix: "emblem/fg",
        source: Source::MainMenu,
        images: 64,
        shape: Shape::square_pow2,
        key: |i| format!("emblem/fg/{i}"),
    },
    Category {
        prefix: "emblem/bg",
        source: Source::MainMenu,
        images: 32,
        shape: Shape::square_pow2,
        key: |i| format!("emblem/bg/{i}"),
    },
    Category {
        prefix: "rank",
        source: Source::MainMenu,
        images: 50,
        shape: |s| s.linear,
        // Ranks count from 1.
        key: |i| format!("rank/{}", i + 1),
    },
    Category {
        prefix: "live",
        source: Source::MainMenu,
        images: 32,
        shape: |s| s.linear && !s.square_pow2(),
        key: |i| format!("live/{i}"),
    },
    Category {
        prefix: "gametype",
        source: Source::MainMenu,
        images: 11,
        shape: |s| s.linear && s.width.abs_diff(s.height) <= 2,
        key: |i| format!("gametype/{i}"),
    },
    Category {
        prefix: "weapon",
        source: Source::Shared,
        images: 19,
        shape: |s| s.linear && s.width > s.height,
        key: |i| format!("weapon/{i}"),
    },
];

/// Game-type icons identified by eye on a retail disc: the index in the
/// game-type icon set of each game type's icon. The other indexes are left
/// as `gametype/<i>` only.
pub const GAMETYPE_ALIASES: &[(&str, usize)] = &[
    ("slayer", 0),
    ("king_of_the_hill", 1),
    ("oddball", 3),
    ("juggernaut", 4),
    ("ctf", 6),
    ("assault", 7),
    ("territories", 8),
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
    pub version: u32,
    /// The cache key of the disc it came from ([`cache_key`]).
    pub source: String,
    /// Semantic key -> picture.
    pub images: BTreeMap<String, Entry>,
    /// Friendlier keys for some of `images` (e.g. `gametype/slayer`).
    pub aliases: BTreeMap<String, String>,
    /// How to colour emblem layers.
    pub emblem: serde_json::Value,
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
        "confidence": "the masks are measured; which profile colour fills which mask is not confirmed"
    })
}

/// Adds every category's images to `images`, and aliases.
pub fn build_manifest(
    source: String,
    images: BTreeMap<String, Entry>,
    skipped: BTreeMap<String, String>,
) -> Manifest {
    let aliases = GAMETYPE_ALIASES
        .iter()
        .map(|(name, i)| (format!("gametype/{name}"), format!("gametype/{i}")))
        .filter(|(_, target)| images.contains_key(target))
        .collect();
    Manifest {
        version: EXTRACTOR_VERSION,
        source,
        images,
        aliases,
        emblem: emblem_rules(),
        skipped,
    }
}

/// The cache key of a disc: the extractor version and, for every map file
/// read, its path, size, sector and first 2 KiB (the map header, which
/// carries the build and checksums). Cheap, and different for a different
/// disc or a re-mastered map.
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

/// The map files on the disc and their cache key.
pub fn disc_key(disc: &Path) -> Result<(String, Vec<xiso::Entry>)> {
    let mut iso = crate::open_iso(disc)?;
    let part = iso.partition_offset();
    let maps: Vec<xiso::Entry> = iso
        .entries()?
        .into_iter()
        .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".map"))
        .collect();
    let mut f = fs::File::open(disc)?;
    let mut parts = Vec::new();
    for e in &maps {
        let mut head = vec![0u8; 2048.min(e.size as usize)];
        f.seek(SeekFrom::Start(part + u64::from(e.sector) * xiso::SECTOR))?;
        f.read_exact(&mut head)?;
        parts.push((e.path.clone(), e.size, e.sector, head));
    }
    Ok((cache_key(&parts), maps))
}

/// The manifest in `dir`, if it is from the disc with `key`.
pub fn cached(dir: &Path, key: &str) -> Option<Manifest> {
    let m: Manifest = serde_json::from_slice(&fs::read(dir.join("manifest.json")).ok()?).ok()?;
    (m.source == key).then_some(m)
}

struct Tag {
    name: String,
    id: map::TagId,
    candidate: Candidate,
}

/// The bitmap tags of one map with their shapes. Names are kept only to
/// compare maps with each other at run time.
fn bitmaps(map: &mut map::Map<std::io::BufReader<fs::File>>) -> Vec<Tag> {
    let list: Vec<_> = map
        .tags()
        .iter()
        .filter(|t| t.group == BITM && t.location.is_some())
        .map(|t| (t.id, t.name.clone().unwrap_or_default()))
        .collect();
    let mut out = Vec::new();
    for (id, name) in list {
        let Ok(decoded) = DecodedTag::read(map, id) else {
            continue;
        };
        let TagData::Bitmap(b) = decoded.data else {
            continue;
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
        out.push(Tag {
            name,
            id,
            candidate: Candidate { shapes },
        });
    }
    out
}

/// What [`extract`] did.
pub struct Report {
    pub manifest: Manifest,
    /// Images written per key prefix (`emblem/fg`, `map`, ...).
    pub counts: BTreeMap<String, usize>,
}

/// Decodes every category from `disc` into `out` (replacing what was
/// there) and writes `manifest.json` last.
pub fn extract(disc: &Path, out: &Path) -> Result<Report> {
    let (key, maps) = disc_key(disc)?;
    let staging = out.with_extension("partial");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).with_context(|| format!("creating {}", staging.display()))?;
    let mut images = BTreeMap::new();
    let mut skipped = BTreeMap::new();

    let (mut mainmenu, mut mm_files) = crate::bitmap::open("mainmenu", Some(disc.to_path_buf()))?;
    let (mut shared, mut sh_files) = crate::bitmap::open("shared", Some(disc.to_path_buf()))?;
    let mm_tags = bitmaps(&mut mainmenu);
    let sh_tags = bitmaps(&mut shared);
    for cat in CATEGORIES {
        let (tags, map, files) = match cat.source {
            Source::MainMenu => (&mm_tags, &mut mainmenu, &mut mm_files),
            Source::Shared => (&sh_tags, &mut shared, &mut sh_files),
        };
        let cands: Vec<Candidate> = tags.iter().map(|t| t.candidate.clone()).collect();
        let Some(i) = pick(cat, &cands) else {
            skipped.insert(
                cat.prefix.to_string(),
                "no bitmap of this shape on the disc".into(),
            );
            continue;
        };
        write_tag(
            map,
            files,
            tags[i].id,
            &staging,
            cat.key,
            &mut images,
            &mut skipped,
        )?;
    }

    // Multiplayer previews: the picture only that map has.
    // (The main menu may carry one map's preview too, so only the shared
    // map's tags are excluded.)
    let common: BTreeSet<&str> = sh_tags.iter().map(|t| t.name.as_str()).collect();
    for e in &maps {
        let file = e.path.rsplit('/').next().unwrap_or(&e.path);
        let name = file.trim_end_matches(".map");
        let Ok((mut map, mut files)) = crate::bitmap::open(name, Some(disc.to_path_buf())) else {
            continue;
        };
        if map.header().cache_type != CacheType::Multiplayer {
            continue;
        }
        let tags: Vec<Tag> = bitmaps(&mut map)
            .into_iter()
            .filter(|t| !common.contains(t.name.as_str()))
            .collect();
        let cands: Vec<Candidate> = tags.iter().map(|t| t.candidate.clone()).collect();
        let key = format!("map/{name}/preview");
        match pick_preview(&cands) {
            Some(i) => {
                let one = |_: usize| key.clone();
                write_tag(
                    &mut map,
                    &mut files,
                    tags[i].id,
                    &staging,
                    one,
                    &mut images,
                    &mut skipped,
                )?;
            }
            None => {
                skipped.insert(key, "no single preview picture in this map".into());
            }
        }
    }

    let manifest = build_manifest(key, images, skipped);
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

/// Decodes one tag's images (or its sprites, when it has any) under
/// `key(i)`, recording failures in `skipped`.
fn write_tag(
    map: &mut map::Map<std::io::BufReader<fs::File>>,
    files: &mut assets::ResourceFiles,
    id: map::TagId,
    out: &Path,
    key: impl Fn(usize) -> String,
    images: &mut BTreeMap<String, Entry>,
    skipped: &mut BTreeMap<String, String>,
) -> Result<()> {
    let TagData::Bitmap(bitmap) = DecodedTag::read(map, id)?.data else {
        bail!("not a bitmap")
    };
    let sprites: Vec<_> = bitmap
        .sequences
        .iter()
        .flat_map(|s| s.sprites.iter())
        .collect();
    let mut decoded = Vec::new();
    for (i, im) in bitmap.images.iter().enumerate() {
        match assets::decode_image(im, files) {
            Ok(d) => match d.levels.into_iter().find(|l| l.level == 0 && l.face == 0) {
                Some(l) => decoded.push(Some((l.width, l.height, l.rgba))),
                None => {
                    skipped.insert(key(i), "no base level".into());
                    decoded.push(None);
                }
            },
            Err(e) => {
                skipped.insert(key(i), format!("decode failed: {e}"));
                decoded.push(None);
            }
        }
    }
    let pieces: Vec<(String, u32, u32, Vec<u8>)> = if sprites.is_empty() {
        decoded
            .into_iter()
            .enumerate()
            .filter_map(|(i, d)| d.map(|(w, h, px)| (key(i), w, h, px)))
            .collect()
    } else {
        sprites
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let (w, h, px) = decoded
                    .get(usize::try_from(s.image_index).ok()?)?
                    .as_ref()?;
                let (sw, sh, cut) = slice(px, *w, *h, s.bounds)?;
                Some((key(i), sw, sh, cut))
            })
            .collect()
    };
    for (k, w, h, px) in pieces {
        let file = format!("{k}.png");
        let path = out.join(&file);
        fs::create_dir_all(path.parent().expect("a parent"))?;
        let mut png = Vec::new();
        assets::png::write_rgba(&mut png, w, h, &px)?;
        fs::write(&path, png)?;
        images.insert(
            k,
            Entry {
                file,
                width: w,
                height: h,
            },
        );
    }
    Ok(())
}

/// `h2 live assets extract [--disc PATH] [--out DIR]`.
pub fn run(disc: PathBuf, out: PathBuf) -> Result<()> {
    let report = extract(&disc, &out)?;
    println!("game art in {}", out.display());
    for (prefix, n) in &report.counts {
        println!("  {prefix:<10} {n}");
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
        for i in [0usize, 3] {
            images.insert(
                format!("gametype/{i}"),
                Entry {
                    file: format!("gametype/{i}.png"),
                    width: 40,
                    height: 40,
                },
            );
        }
        let m = build_manifest("k".into(), images, BTreeMap::new());
        assert_eq!(
            m.aliases.get("gametype/slayer").map(String::as_str),
            Some("gametype/0")
        );
        assert_eq!(
            m.aliases.get("gametype/oddball").map(String::as_str),
            Some("gametype/3")
        );
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
}
