//! Lighting inspection and bounded, no-clobber lightmap page export.
use crate::safe_out::OutDir;
use anyhow::{Context, Result, bail};
use assets::ResourceFiles;
use map::{FourCC, Map, RawPointer, TagId};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};
use tags::{
    DecodedTag, PixelFormat, TagData,
    geometry::{RawSource, VertexKind},
    lighting::{
        ATMOSPHERE_GROUPS, Atmosphere, LightmapLighting, VertexLighting, bsp_lightmaps,
        palette_rgba, vertex_lighting,
    },
    structure::load_bucket,
};

const LTMP: FourCC = FourCC(*b"ltmp");
const USAGE: &str = "usage: h2 lighting <map> [--bsp N] [--disc ISO]
       h2 lighting <map> --bsp N --page N [--group N] [--palette N] --out DIR [--force] [--disc ISO]
       h2 lighting --all [<map>] [--disc ISO]
Summary only by default. --all validates reviewed metadata, references, bitmap
pages and bucket resources; opaque fields are reported as unreviewed.
--page exports one palette-coloured P8 page as PNG; ambiguous palette choices
require --palette. A page is not the final shaded material.";

type Cache = Map<BufReader<File>>;
#[derive(Default, Debug)]
struct Options {
    map: Option<String>,
    disc: Option<PathBuf>,
    bsp: Option<usize>,
    group: Option<usize>,
    page: Option<usize>,
    palette: Option<usize>,
    out: Option<PathBuf>,
    all: bool,
    force: bool,
    help: bool,
}
impl Options {
    fn parse(args: Vec<String>) -> Result<Self> {
        let mut o = Self::default();
        let mut a = args.into_iter();
        while let Some(arg) = a.next() {
            match arg.as_str() {
                "--disc" if o.disc.is_none() => {
                    o.disc = Some(a.next().context("--disc needs ISO")?.into())
                }
                "--bsp" if o.bsp.is_none() => {
                    o.bsp = Some(a.next().context("--bsp needs N")?.parse()?)
                }
                "--group" if o.group.is_none() => {
                    o.group = Some(a.next().context("--group needs N")?.parse()?)
                }
                "--page" if o.page.is_none() => {
                    o.page = Some(a.next().context("--page needs N")?.parse()?)
                }
                "--palette" if o.palette.is_none() => {
                    o.palette = Some(a.next().context("--palette needs N")?.parse()?)
                }
                "--out" if o.out.is_none() => {
                    o.out = Some(a.next().context("--out needs DIR")?.into())
                }
                "--all" if !o.all => o.all = true,
                "--force" if !o.force => o.force = true,
                "--help" | "-h" => o.help = true,
                s if s.starts_with('-') => bail!("unknown or repeated option {s}\n{USAGE}"),
                _ if o.map.is_none() => o.map = Some(arg),
                _ => bail!("{USAGE}"),
            }
        }
        if !o.help {
            if !o.all && o.map.is_none() {
                bail!("{USAGE}");
            }
            if o.all && (o.bsp.is_some() || o.page.is_some() || o.out.is_some()) {
                bail!("--all cannot select a BSP or export\n{USAGE}");
            }
            if o.page.is_some() != o.out.is_some() {
                bail!("--page and --out are required together");
            }
            if o.page.is_some() && o.bsp.is_none() {
                bail!("page export requires --bsp N");
            }
            if o.page.is_none() && (o.group.is_some() || o.palette.is_some() || o.force) {
                bail!("--group, --palette and --force require a page export");
            }
        }
        Ok(o)
    }
}

/// Adapter reuses the bitmap reader's file routing, bounds and read-size cap.
struct Raw<'a>(&'a mut ResourceFiles);
impl RawSource for Raw<'_> {
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.0.read(p.file, p.offset, size).map_err(|e| match e {
            assets::Error::Map(e) => tags::Error::Map(e),
            assets::Error::Io(e) => tags::Error::Map(e.into()),
            assets::Error::MissingResource(f) => tags::Error::MissingResource(f),
            _ => tags::Error::Invalid("bucket resource read rejected"),
        })
    }
}

#[derive(Default)]
struct Stats {
    maps: usize,
    lightmaps: usize,
    groups: usize,
    palettes: usize,
    probes: usize,
    pages: usize,
    buckets: usize,
    color_vertices: usize,
    vector_vertices: usize,
    unknown_streams: usize,
    sky_aliases: usize,
    root_aliases: usize,
    unsupported_blocks: usize,
    tags: BTreeMap<FourCC, usize>,
    absent: BTreeMap<FourCC, usize>,
    unique: BTreeSet<(FourCC, String)>,
    failures: Vec<String>,
}
impl Stats {
    fn report(&self) {
        println!(
            "{} maps; {} local lightmaps, {} groups, {} palettes, {} probes",
            self.maps, self.lightmaps, self.groups, self.palettes, self.probes
        );
        println!(
            "{} bitmap pages; {} buckets; {} colour vertices, {} vector vertices",
            self.pages, self.buckets, self.color_vertices, self.vector_vertices
        );
        println!(
            "Unreviewed: {} group blocks, {} vertex streams",
            self.unsupported_blocks, self.unknown_streams
        );
        println!(
            "Aliases: {} sky light blocks, {} zero-sized roots",
            self.sky_aliases, self.root_aliases
        );
        for (&g, &n) in &self.tags {
            let unique = self.unique.iter().filter(|(group, _)| *group == g).count();
            println!(
                "{g}: {n} instances, {unique} distinct paths, {} absent local payloads",
                self.absent.get(&g).copied().unwrap_or(0)
            );
        }
        println!("{} failures", self.failures.len());
        for f in &self.failures {
            eprintln!("  {f}");
        }
    }
}

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let o = Options::parse(args)?;
    if o.help {
        println!("{USAGE}");
        return Ok(());
    }
    let mut stats = Stats::default();
    if o.all {
        let names = if let Some(name) = &o.map {
            vec![name.clone()]
        } else {
            let disc = o.disc.clone().map(Ok).unwrap_or_else(super::default_disc)?;
            let mut iso = super::open_iso(&disc)?;
            iso.entries()?
                .into_iter()
                .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".map"))
                .map(|e| e.path)
                .collect()
        };
        for name in names {
            if let Err(e) = inspect(&name, &o, &mut stats) {
                stats.failures.push(format!("{name}: {e:#}"));
            }
        }
        stats.report();
        if !stats.failures.is_empty() {
            bail!(
                "lighting validation failed ({} failures)",
                stats.failures.len()
            );
        }
    } else {
        inspect(o.map.as_deref().context(USAGE)?, &o, &mut stats)?;
    }
    Ok(())
}

fn inspect(name: &str, o: &Options, stats: &mut Stats) -> Result<()> {
    let (mut map, mut files) = super::bitmap::open(name, o.disc.clone())?;
    stats.maps += 1;
    let pairs = bsp_lightmaps(&mut map)?;
    if let Some(bsp) = o.bsp
        && bsp >= pairs.len()
    {
        bail!("--bsp {bsp} outside {} scenario BSPs", pairs.len());
    }
    if o.page.is_some() {
        let b = o.bsp.context("page export needs a BSP")?;
        let id = pairs[b].1.context("selected BSP has no lightmap")?;
        if map.tag(id)?.location.is_none() {
            bail!("selected lightmap has no local payload");
        }
    }
    if !o.all {
        println!("{name}: {} scenario BSPs", pairs.len());
    }
    let tags: Vec<_> = map
        .tags()
        .iter()
        .filter(|t| t.group == LTMP || ATMOSPHERE_GROUPS.contains(&t.group))
        .cloned()
        .collect();
    for tag in tags {
        *stats.tags.entry(tag.group).or_default() += 1;
        // Distinct paths are an inventory metric, not byte-identical content.
        stats.unique.insert((
            tag.group,
            tag.name
                .clone()
                .unwrap_or_else(|| format!("{name}:{}", tag.id)),
        ));
        if tag.location.is_none() {
            *stats.absent.entry(tag.group).or_default() += 1;
            continue;
        }
        let result = if tag.group == LTMP {
            if o.bsp.is_some_and(|b| pairs[b].1 != Some(tag.id)) {
                continue;
            }
            inspect_lightmap(&mut map, &mut files, tag.id, &pairs, o, stats)
        } else {
            Atmosphere::read(&mut map, tag.id)
                .map_err(anyhow::Error::from)
                .map(|v| {
                    if tag.location.is_some_and(|l| l.size == 0) {
                        stats.root_aliases += 1;
                    }
                    if let Atmosphere::Sky(s) = &v {
                        stats.sky_aliases += usize::from(s.lights_alias);
                    }
                    if !o.all {
                        let detail = match &v {
                            Atmosphere::Sky(s) => format!(
                                "{} lights, fallback RGB {:?}",
                                s.lights.len(),
                                s.fallback_rgb
                            ),
                            Atmosphere::LightVolume(v) => format!("{} layers", v.layers.len()),
                            Atmosphere::SoundEnvironment(s) => {
                                format!("decay {}s, room {}dB", s.decay_seconds, s.room_db)
                            }
                            Atmosphere::GlobalLighting(_) | Atmosphere::Weather(_) => {
                                "opaque; unreviewed".into()
                            }
                            _ => "references reviewed; optical fields opaque".into(),
                        };
                        println!("  {} {}: {detail}", tag.group, tag.id);
                    }
                })
        };
        if let Err(e) = result {
            if !o.all {
                return Err(e).with_context(|| format!("{} {}", tag.group, tag.id));
            }
            stats
                .failures
                .push(format!("{name}: {} {}: {e:#}", tag.group, tag.id));
        }
    }
    if !o.all
        && let Some(b) = o.bsp
        && pairs[b].1.is_none()
    {
        println!("  BSP {b}: no lightmap");
    }
    if o.page.is_some() && o.bsp.is_some_and(|b| pairs[b].1.is_none()) {
        bail!("selected BSP has no lightmap");
    }
    Ok(())
}

fn inspect_lightmap(
    map: &mut Cache,
    files: &mut ResourceFiles,
    id: TagId,
    pairs: &[(TagId, Option<TagId>)],
    o: &Options,
    stats: &mut Stats,
) -> Result<()> {
    let lightmap = LightmapLighting::read(map, id)?;
    stats.lightmaps += 1;
    if o.group.is_some_and(|i| i >= lightmap.linkage.groups.len()) {
        bail!("--group outside lightmap groups");
    }
    if o.page.is_some() && lightmap.linkage.groups.is_empty() {
        bail!("selected lightmap has no groups");
    }
    for (gi, g) in lightmap.linkage.groups.iter().enumerate() {
        stats.groups += 1;
        stats.palettes += g.palettes.len();
        stats.probes += lightmap.probes[gi].len();
        // +38 is now reviewed by this decoder; +10 and +58 remain opaque.
        stats.unsupported_blocks +=
            usize::from(g.unsupported_counts[0] != 0) + usize::from(g.unsupported_counts[2] != 0);
        let bitmap = match g.bitmap {
            None => None,
            Some(id) => {
                let d = DecodedTag::read(map, id)?;
                let TagData::Bitmap(b) = d.data else {
                    bail!("lightmap bitmap class mismatch");
                };
                Some(b)
            }
        };
        let page_count = bitmap.as_ref().map_or(0, |b| b.images.len());
        for r in g.cluster_render_info.iter().chain(&g.instance_render_info) {
            if r.bitmap_index < -1 || r.bitmap_index >= 0 && r.bitmap_index as usize >= page_count {
                bail!("lightmap bitmap index out of range");
            }
            if r.palette_index < -1
                || r.palette_index >= 0 && r.palette_index as usize >= g.palettes.len()
            {
                bail!("lightmap palette index out of range");
            }
        }
        if !o.all {
            let bsp = pairs
                .iter()
                .position(|(_, l)| *l == Some(id))
                .map_or("unpaired".into(), |n| n.to_string());
            println!(
                "  ltmp {id}, BSP {bsp}, group {gi}: {page_count} pages, {} palettes, {} probes, {} buckets",
                g.palettes.len(),
                lightmap.probes[gi].len(),
                g.buckets.len()
            );
        }
        if o.all {
            if let Some(b) = &bitmap {
                for image in &b.images {
                    let d = assets::decode_image(image, files)?;
                    if !d.size_matches() {
                        bail!("lightmap LOD size does not match decoded layout");
                    }
                    stats.pages += 1;
                }
            }
            for bucket in &g.buckets {
                if let Some(section) = load_bucket(bucket, &mut Raw(files))? {
                    stats.buckets += 1;
                    for stream in &section.vertex_buffers {
                        match vertex_lighting(&section, stream)? {
                            Some(VertexLighting::Color(v)) => stats.color_vertices += v.len(),
                            Some(VertexLighting::Vector(v)) => stats.vector_vertices += v.len(),
                            // Kind and stride were checked by the shared decoder.
                            None if stream.kind == VertexKind::LightmapTexcoord => {}
                            None => stats.unknown_streams += 1,
                        }
                    }
                }
            }
        }
        if let Some(page) = o.page
            && gi == o.group.unwrap_or(0)
        {
            let b = bitmap.as_ref().context("lightmap has no bitmap")?;
            let image = b.images.get(page).context("--page outside bitmap images")?;
            let palette = select_palette(g, page, o.palette)?;
            let mut decoded = decode_page(image, files, palette)?;
            let level = &mut decoded.levels[0];
            let mut png = Vec::new();
            assets::png::write_rgba(&mut png, level.width, level.height, &level.rgba)?;
            let filename = format!("lightmap_{:08x}_g{gi}_page{page}.png", id.0);
            let out = o.out.as_ref().context("--out missing")?;
            let path = write_page(out, o.force, filename, &png)?;
            println!("  wrote {} (source LOD {})", path.display(), decoded.lod);
        }
    }
    Ok(())
}
/// Write one exported page. The root is walked component by component and
/// may not be (or pass through) a user-controlled symlink; see
/// [`OutDir::new_strict`].
fn write_page(out: &Path, force: bool, filename: String, png: &[u8]) -> Result<PathBuf> {
    let dir = OutDir::new_strict(out, force)
        .with_context(|| format!("output directory {}", out.display()))?;
    Ok(dir.write(&[filename], png)?)
}

fn decode_page(
    image: &tags::Image,
    files: &mut ResourceFiles,
    palette: &[u32; 256],
) -> Result<assets::DecodedImage> {
    if image.format != PixelFormat::P8 || image.kind != tags::TextureKind::D2 {
        bail!("palette page export requires a 2D P8 bitmap");
    }
    // The existing decoder unswizzles P8 losslessly as index luminance.
    // Replace each index with its actual palette entry, including alpha.
    let mut decoded = assets::decode_image(image, files)?;
    if decoded.lod != 0 {
        bail!("full-resolution lightmap page (LOD 0) is absent");
    }
    decoded.levels.retain(|l| l.face == 0 && l.level == 0);
    if decoded.levels.len() != 1 {
        bail!("missing base level");
    }
    for pixel in decoded.levels[0].rgba.as_chunks_mut::<4>().0 {
        *pixel = palette_rgba(palette, pixel[0]);
    }
    Ok(decoded)
}

fn select_palette(
    g: &tags::structure::LightmapGroup,
    page: usize,
    selected: Option<usize>,
) -> Result<&[u32; 256]> {
    let n = if let Some(n) = selected {
        n
    } else {
        let choices: BTreeSet<_> = g
            .cluster_render_info
            .iter()
            .chain(&g.instance_render_info)
            .filter(|r| {
                r.bitmap_index >= 0 && r.bitmap_index as usize == page && r.palette_index >= 0
            })
            .map(|r| r.palette_index as usize)
            .collect();
        if choices.len() != 1 {
            bail!(
                "page has {} referenced palettes; choose --palette N",
                choices.len()
            );
        }
        *choices.first().expect("one palette")
    };
    g.palettes.get(n).context("--palette outside palette block")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Options> {
        Options::parse(args.iter().map(|s| (*s).into()).collect())
    }
    #[test]
    fn selection_and_validation_are_explicit() {
        assert!(parse(&["--all"]).unwrap().all);
        assert!(parse(&["invented", "--all"]).is_ok());
        assert!(parse(&["invented", "--bsp", "2"]).is_ok());
        assert!(parse(&["invented", "--bsp", "2", "--page", "1", "--out", "out"]).is_ok());
        for args in [
            vec![],
            vec!["--all", "--bsp", "0"],
            vec!["x", "--out", "out"],
            vec!["x", "--page", "0", "--out", "out"],
            vec!["x", "--palette", "0"],
            vec!["x", "--bsp", "0", "--bsp", "1"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }
}

#[cfg(test)]
mod export_tests {
    use super::*;
    use tags::{records::BitmapImage, wire::BinaryRecord};
    #[test]
    fn p8_page_unswizzles_then_uses_palette_channels_and_no_clobber_output() {
        let root = std::env::temp_dir().join(format!("lighting-png-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("indices.bin");
        let mut bytes = vec![0; 128];
        bytes[..4].copy_from_slice(&[0, 1, 2, 3]);
        std::fs::write(&source, bytes).unwrap();
        let mut files = ResourceFiles::new();
        files
            .set(
                map::ResourceFile::Local,
                assets::Region {
                    path: source.clone(),
                    base: 0,
                    length: 128,
                },
            )
            .unwrap();
        let mut wire = BitmapImage::decode(&[0; BitmapImage::SIZE]).unwrap();
        wire.flags = tags::wire::Flags16(8);
        let mut image = tags::Image {
            wire,
            format: PixelFormat::P8,
            kind: tags::TextureKind::D2,
            dimensions: [2, 2, 1],
            mip_count: 0,
            low_detail: None,
            lods: [
                Some(tags::ResourceRange {
                    pointer: RawPointer {
                        file: map::ResourceFile::Local,
                        offset: 0,
                    },
                    size: 128,
                }),
                None,
                None,
            ],
        };
        let mut palette = [0; 256];
        palette[..4].copy_from_slice(&[0xff123456, 0x78123456, 0xffabcdef, 0xff010203]);
        let d = decode_page(&image, &mut files, &palette).unwrap();
        assert_eq!(
            d.levels[0].rgba,
            [
                0x12, 0x34, 0x56, 255, 0x12, 0x34, 0x56, 0x78, 0xab, 0xcd, 0xef, 255, 1, 2, 3, 255
            ]
        );
        let mut png = Vec::new();
        assets::png::write_rgba(&mut png, 2, 2, &d.levels[0].rgba).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let dest = write_page(&root.join("out"), false, "page.png".into(), &png).unwrap();
        assert!(write_page(&root.join("out"), false, "page.png".into(), b"bad").is_err());
        assert_eq!(std::fs::read(dest).unwrap(), png);
        image.format = PixelFormat::P8Bump;
        assert!(decode_page(&image, &mut files, &palette).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn ambiguous_palette_selection_requires_an_explicit_index() {
        let mut b = vec![0; 0x104 + 0x68];
        let put = |b: &mut [u8], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 0x80, 1);
        put(&mut b, 0x84, 0x104);
        put(&mut b, 0x104 + 0x1c, u32::MAX);
        let mut l =
            tags::structure::Lightmap::decode(TagId(1), &tags::geometry::TagView::new(&b, 0))
                .unwrap();
        let g = &mut l.groups[0];
        g.palettes = vec![[0x11223344; 256], [0x55667788; 256]];
        let r = |palette_index| tags::structure::LightmapRenderInfo {
            bitmap_index: 3,
            palette_index,
            unknown_3: 0,
        };
        g.cluster_render_info = vec![r(0), r(1)];
        assert!(select_palette(g, 3, None).is_err());
        assert_eq!(select_palette(g, 3, Some(1)).unwrap()[0], 0x55667788);
        assert!(select_palette(g, 3, Some(2)).is_err());
        g.cluster_render_info = vec![r(0), r(0)];
        assert_eq!(select_palette(g, 3, None).unwrap()[0], 0x11223344);
        assert!(select_palette(g, 4, None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn page_export_refuses_a_symlinked_output_root_or_ancestor() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("lighting-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let real = root.join("real");
        std::fs::create_dir_all(&real).unwrap();
        symlink(&real, root.join("linked")).unwrap();
        let png = b"\x89PNG\r\n\x1a\n".to_vec();
        // The root itself, and a root below a linked ancestor.
        for out in [root.join("linked"), root.join("linked").join("deeper")] {
            assert!(write_page(&out, false, "page.png".into(), &png).is_err());
            assert!(write_page(&out, true, "page.png".into(), &png).is_err());
        }
        assert_eq!(std::fs::read_dir(&real).unwrap().count(), 0);
        // The real directory is accepted, and a new nested root is created.
        assert!(write_page(&real, false, "page.png".into(), &png).is_ok());
        assert!(write_page(&real.join("a").join("b"), false, "page.png".into(), &png).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// End-to-end `--all` exit status over invented maps.
#[cfg(test)]
mod all_tests {
    use super::*;
    const BASE: u32 = 0x8100_0000;
    fn put(b: &mut [u8], o: usize, v: u32) {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn fourcc(g: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*g)
    }
    /// Globals, a scenario with no BSPs, two skies (one aliasing the other's
    /// light) and two identities of one fog. Every byte is invented.
    fn invented_map() -> Vec<u8> {
        let mut b = vec![0u8; 0x3000];
        b[..4].copy_from_slice(b"daeh");
        b[0x7fc..0x800].copy_from_slice(b"toof");
        b[0x120..0x12e].copy_from_slice(b"02.09.27.09809");
        for (o, v) in [
            (4, 8),
            (8, 0x3000),
            (0x10, 0x800),
            (0x14, 0x800),
            (0x18, 0x2000),
            (0x1c, 0x2000),
        ] {
            put(&mut b, o, v);
        }
        for (i, g) in [b"matg", b"scnr", b"sky ", b"fog "].iter().enumerate() {
            put(&mut b, 0x820 + i * 12, fourcc(g));
            put(&mut b, 0x824 + i * 12, u32::MAX);
            put(&mut b, 0x828 + i * 12, u32::MAX);
        }
        for (o, v) in [
            (0, 0x8000_0020),
            (4, 4),
            (8, 0x8000_0050),
            (12, 0xa000_0001),
            (16, 0xa000_0000),
            (24, 6),
            (28, fourcc(b"tags")),
        ] {
            put(&mut b, 0x800 + o, v);
        }
        for (i, (g, offset, size)) in [
            (b"matg", 0, 0x10),
            (b"scnr", 0x100, 0x218),
            (b"sky ", 0x400, 0xac),
            (b"sky ", 0x600, 0xe0),
            (b"fog ", 0x800, 0x60),
            (b"fog ", 0x800, 0),
        ]
        .iter()
        .enumerate()
        {
            let row = 0x850 + i * 16;
            put(&mut b, row, fourcc(g));
            put(&mut b, row + 4, 0xa000_0000 + i as u32);
            put(&mut b, row + 8, BASE + offset);
            put(&mut b, row + 12, *size);
        }
        for root in [0x1400, 0x1600] {
            put(&mut b, root + 4, u32::MAX);
            put(&mut b, root + 0x78, 1);
            put(&mut b, root + 0x7c, BASE + 0x6ac);
        }
        put(&mut b, 0x16ac + 0x18, u32::MAX);
        for o in [0x3c, 0x44, 0x54, 0x5c] {
            put(&mut b, 0x1800 + o, u32::MAX);
        }
        b
    }
    fn all(path: &Path) -> Result<()> {
        run(vec!["--all".into(), path.to_string_lossy().into_owned()])
    }
    #[test]
    fn all_returns_an_error_when_any_tag_fails() {
        let dir = std::env::temp_dir().join(format!("lighting-all-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let valid = dir.join("valid.map");
        std::fs::write(&valid, invented_map()).unwrap();
        all(&valid).unwrap();
        // One sky's light block now points at bytes no sky payload owns.
        let mut bad = invented_map();
        put(&mut bad, 0x1400 + 0x7c, BASE + 0x1200);
        let malformed = dir.join("malformed.map");
        std::fs::write(&malformed, bad).unwrap();
        let e = all(&malformed).unwrap_err();
        assert!(
            e.to_string().contains("lighting validation failed"),
            "{e:#}"
        );
        // A map that cannot be opened is a failure too, not a skip.
        let garbage = dir.join("garbage.map");
        std::fs::write(&garbage, [0u8; 64]).unwrap();
        assert!(all(&garbage).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
