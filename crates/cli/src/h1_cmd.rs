//! `h2 h1 info|tags` and `h2 import halo1`: the Halo 1 (Combat Evolved)
//! disc and its cache files, step 1 of the campaigns work
//! (docs/game/campaigns.md, docs/formats/h1-map-format.md).
//!
//! The import copies the campaign maps byte for byte, as the disc has them
//! (compressed), into `<data>/campaigns/halo1/source/`, with a manifest of
//! their SHA-256. Converting them to Halo 2 maps is a later step.
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use h1convert::convert::{Sandbox, known, map_name, prepare, structure_mopp};
use h1map::disc::{self, Copied, Disc};
use h1map::{Group, H1Map, HEADER_LEN};
use xiso::{Entry, Xiso};

use crate::safe_out::OutDir;

const H1_USAGE: &str = "usage: h2 h1 info <iso|map> [map]
       h2 h1 tags <iso|map> [map] [--group G]
       h2 h1 bitmap <iso|map> [map] <tag> --out DIR [--mips] [--force]
       h2 h1 convert <iso|map> [map] --out DIR [--sandbox h2|h1] [--base MAP]
                     [--disc halo2.iso] [--dry-run] [--force]

info: with a disc and no map, the disc's title and each map's header; with a
map (a .map file, or a disc and a map name such as a30), its header, tag index
and tag counts by group. tags: every tag, or one group's: ID, group, data
range and name. See docs/formats/h1-map-format.md.

bitmap: one bitm tag (its path, as `h2 h1 tags --group bitm` prints it):
each image's format, type and size, decoded to PNGs in DIR (one per image,
cube face and volume slice; every mip level with --mips). See
docs/formats/h1-bitmaps.md.

convert: a Halo 1 multiplayer map (a .map file, or a disc and a map name such
as beavercreek) onto a Halo 2 base map from the Halo 2 disc (--disc, else
$H2_DISC, else disc/halo2.iso), written as the package DIR/h1_<map>/
(manifest.toml and h1_<map>.map). --base picks the base (default per map,
mostly beavercreek). --dry-run does everything but write and prints what it
would write. See docs/game/h1-multiplayer.md.";

const IMPORT_USAGE: &str = "usage: h2 import halo1 [halo1.iso] [--data DIR] [--force] [--dry-run]

Checks the Halo: Combat Evolved disc and copies its campaign maps (a10 to d40,
ui.map, and bitmaps.map and sounds.map if the disc has them) as they are on
the disc into <data>/campaigns/halo1/source/, with manifest.txt listing each
file's SHA-256. The disc defaults to disc/halo1.iso; --data to
$TMPDIR/h2boot-data, as for `h2 play`. See docs/game/campaigns.md.";

/// Where an import writes, under the data root.
const SOURCE_DIR: [&str; 3] = ["campaigns", "halo1", "source"];
const MANIFEST: &str = "manifest.txt";
/// A compressed Halo 1 map is tens of MB; refuse to buffer anything absurd.
const MAP_MAX: u32 = 512 << 20;

pub fn run(args: Vec<String>) -> Result<()> {
    let Some((sub, rest)) = args.split_first() else {
        bail!("{H1_USAGE}")
    };
    match sub.as_str() {
        "info" => info(rest),
        "tags" => tags(rest),
        "bitmap" => bitmap(rest),
        "convert" => convert(rest),
        "-h" | "--help" => {
            println!("{H1_USAGE}");
            Ok(())
        }
        other => bail!("unknown h1 command {other}\n\n{H1_USAGE}"),
    }
}

/// `h2 import <what>`: only `halo1` so far.
pub fn run_import(args: Vec<String>) -> Result<()> {
    match args.split_first() {
        Some((what, rest)) if what == "halo1" => import(rest),
        Some((what, _)) if what != "-h" && what != "--help" => {
            bail!(
                "unknown import {what}: `h2 import halo1`, or `h2 import-mappack` for the map pack"
            )
        }
        _ => {
            println!("{IMPORT_USAGE}");
            Ok(())
        }
    }
}

/// The positional arguments and `--group` of `h1 info|tags`.
#[derive(Debug, PartialEq)]
pub struct ViewArgs {
    pub path: PathBuf,
    pub map: Option<String>,
    pub group: Option<Group>,
}

pub fn parse_view(args: &[String], allow_group: bool) -> Result<ViewArgs, String> {
    let (mut path, mut map, mut group) = (None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--group" if allow_group => {
                let g = it.next().ok_or("--group needs a group, such as scnr")?;
                group = Some(Group::parse(g).ok_or_else(|| format!("{g} is not a tag group"))?);
            }
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ if path.is_none() => path = Some(PathBuf::from(a)),
            _ if map.is_none() => map = Some(a.clone()),
            _ => return Err(format!("unexpected argument {a}")),
        }
    }
    Ok(ViewArgs {
        path: path.ok_or("missing the disc or map")?,
        map,
        group,
    })
}

/// What `h1 info|tags` was pointed at.
enum Source {
    Disc(Disc, Xiso<BufReader<File>>),
    Map(String, H1Map),
}

fn open_source(path: &Path, map: Option<&str>) -> Result<Source> {
    crate::safe_out::protect(path);
    let f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let Ok(mut iso) = Xiso::open(BufReader::new(f)) else {
        if map.is_some() {
            bail!(
                "{} is not a disc image, so it takes no map name",
                path.display()
            );
        }
        let f = File::open(path)?;
        let m =
            H1Map::from_reader(BufReader::new(f)).with_context(|| path.display().to_string())?;
        return Ok(Source::Map(path.display().to_string(), m));
    };
    let d = Disc::open(&mut iso)?;
    match map {
        None => Ok(Source::Disc(d, iso)),
        Some(name) => {
            let e = d
                .map(name)
                .with_context(|| format!("no map {name} on the disc"))?
                .clone();
            Ok(Source::Map(
                disc::file_name(&e).to_string(),
                read_map(&mut iso, &e)?,
            ))
        }
    }
}

fn read_map<R: Read + Seek>(iso: &mut Xiso<R>, e: &Entry) -> Result<H1Map> {
    if e.size > MAP_MAX {
        bail!("{} is {} bytes, too big for a map", e.path, e.size);
    }
    let bytes = iso.read(e)?;
    H1Map::from_reader(Cursor::new(bytes)).with_context(|| e.path.clone())
}

/// The uncompressed header of a map on the disc, without inflating it.
fn read_header<R: Read + Seek>(iso: &mut Xiso<R>, e: &Entry) -> Result<h1map::Header> {
    if (e.size as usize) < HEADER_LEN {
        bail!("{} is {} bytes, shorter than a map header", e.path, e.size);
    }
    let head = iso.read(&Entry {
        size: HEADER_LEN as u32,
        ..e.clone()
    })?;
    h1map::parse_header(&head).with_context(|| e.path.clone())
}

fn info(args: &[String]) -> Result<()> {
    let a = parse_view(args, false).map_err(|e| anyhow::anyhow!("{e}\n\n{H1_USAGE}"))?;
    match open_source(&a.path, a.map.as_deref())? {
        Source::Disc(d, mut iso) => {
            println!(
                "title {:08X} {:?} version {:#x}",
                d.title_id, d.title_name, d.version
            );
            println!("{} maps under {}", d.maps.len(), disc::MAPS_DIR);
            for e in &d.maps {
                let h = read_header(&mut iso, e)?;
                println!(
                    "  {:<20} {:>10} bytes, {:#010x} inflated  {:<11} {} {}",
                    disc::file_name(e),
                    e.size,
                    h.file_length,
                    h.cache_type.to_string(),
                    h.build,
                    h.name
                );
            }
        }
        Source::Map(name, m) => print_map(&name, &m)?,
    }
    Ok(())
}

fn print_map(name: &str, m: &H1Map) -> Result<()> {
    let h = m.header();
    let i = m.index();
    println!("{name}: {} ({}), build {}", h.name, h.cache_type, h.build);
    println!(
        "  version {}  inflated {:#x}  tag data {:#x}+{:#x}  checksum {:#010x}  [0x0c] {:#x}",
        h.version, h.file_length, h.index_offset, h.metadata_size, h.checksum, h.field_0c
    );
    println!(
        "  tag index: {} tags at {:#010x}, base {:#010x}, scenario {} {}, checksum {:#010x}, header {:#x} bytes",
        i.tag_count,
        i.tag_array_address,
        m.metadata_base(),
        i.scenario,
        m.scenario().name,
        i.checksum,
        i.len
    );
    println!(
        "  model data: {} parts, vertices {:#010x}, indices {:#010x}",
        i.model_part_count(),
        i.vertex_data(),
        i.index_data()
    );
    match m.structure_bsps() {
        Ok(bsps) => {
            for b in bsps {
                println!(
                    "  bsp {:#010x}+{:#x} at {:#010x}  {}",
                    b.file_offset, b.size, b.address, b.name
                );
            }
        }
        Err(e) => println!("  structure BSPs: {e}"),
    }
    println!("  tags by group:");
    for (g, n) in m.group_counts() {
        println!("    {g}  {n}");
    }
    Ok(())
}

fn tags(args: &[String]) -> Result<()> {
    let a = parse_view(args, true).map_err(|e| anyhow::anyhow!("{e}\n\n{H1_USAGE}"))?;
    let m = match open_source(&a.path, a.map.as_deref())? {
        Source::Map(_, m) => m,
        Source::Disc(d, _) => {
            let names: Vec<&str> = d.maps.iter().map(disc::map_stem).collect();
            bail!("name a map on the disc: {}", names.join(", "))
        }
    };
    for t in m
        .tags()
        .iter()
        .filter(|t| a.group.is_none_or(|g| t.group == g))
    {
        let data = match (&t.data, t.external) {
            (_, true) => format!("external {:#x}", t.data_address),
            (Some(r), _) => format!("{:#010x}+{:#x}", r.start, r.len()),
            (None, _) => format!("@{:#010x}", t.data_address),
        };
        println!("{}  {}  {:<24} {}", t.id, t.group, data, t.name);
    }
    Ok(())
}

/// The arguments of `h1 bitmap`.
#[derive(Debug, PartialEq)]
pub struct BitmapArgs {
    pub path: PathBuf,
    pub map: Option<String>,
    pub tag: String,
    pub out: PathBuf,
    pub mips: bool,
    pub force: bool,
}

pub fn parse_bitmap(args: &[String]) -> Result<BitmapArgs, String> {
    let (mut pos, mut out, mut mips, mut force) = (Vec::new(), None, false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(it.next().ok_or("--out needs a directory")?)),
            "--mips" => mips = true,
            "--force" => force = true,
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => pos.push(a.clone()),
        }
    }
    let (path, map, tag) = match &pos[..] {
        [p, t] => (p, None, t),
        [p, m, t] => (p, Some(m.clone()), t),
        [] | [_] => return Err("missing the disc or map, and the bitmap's tag path".into()),
        _ => return Err(format!("unexpected argument {}", pos[3])),
    };
    Ok(BitmapArgs {
        path: PathBuf::from(path),
        map,
        tag: tag.clone(),
        out: out.ok_or("--out is needed")?,
        mips,
        force,
    })
}

fn bitmap(args: &[String]) -> Result<()> {
    use h1bitmaps::BitmapExt;
    use h1map::bitmap::{BITM, Bitmap};
    let a = parse_bitmap(args).map_err(|e| anyhow::anyhow!("{e}\n\n{H1_USAGE}"))?;
    let m = match open_source(&a.path, a.map.as_deref())? {
        Source::Map(_, m) => m,
        Source::Disc(..) => bail!("name a map on the disc, then the bitmap"),
    };
    let tag = m
        .find(BITM, &a.tag)
        .with_context(|| format!("no bitm {} (see h2 h1 tags --group bitm)", a.tag))?;
    let bm = Bitmap::read(&m, tag)?;
    let out = OutDir::new_strict(&a.out, a.force)?.protecting(&a.path);
    let stem = h1models::short_name(&tag.name).replace(' ', "_");
    println!(
        "{}: {} image(s), {} sequence(s), usage {}",
        tag.name,
        bm.images.len(),
        bm.sequences.len(),
        bm.usage
    );
    for (i, img) in bm.images.iter().enumerate() {
        println!(
            "  #{i}: {:?} {:?} {}x{}x{}, {} mip(s) below, flags {:#x}, pixels {:#x}+{:#x}",
            img.format,
            img.kind,
            img.width,
            img.height,
            img.depth,
            img.mip_count,
            img.flags,
            img.pixel_offset,
            img.pixel_size
        );
        let levels = match bm.decode_levels(&m, i) {
            Ok(l) => l,
            Err(e) => {
                println!("     not decoded: {e}");
                continue;
            }
        };
        let faces = levels.iter().map(|l| l.face).max().unwrap_or(0) + 1;
        for l in levels.iter().filter(|l| a.mips || l.level == 0) {
            let slice = (l.width * l.height * 4) as usize;
            for z in 0..l.depth as usize {
                let mut name = format!("{stem}_img{i}");
                if faces > 1 {
                    name += &format!("_face{}", l.face);
                }
                if l.depth > 1 {
                    name += &format!("_z{z}");
                }
                if a.mips {
                    name += &format!("_mip{}", l.level);
                }
                let mut png = Vec::new();
                assets::png::write_rgba(
                    &mut png,
                    l.width,
                    l.height,
                    &l.rgba[z * slice..(z + 1) * slice],
                )?;
                let p = out.write(&[name + ".png"], &png)?;
                println!("     {}", p.display());
            }
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
pub struct ImportArgs {
    pub iso: PathBuf,
    pub data: Option<PathBuf>,
    pub force: bool,
    pub dry_run: bool,
}

pub fn parse_import(args: &[String], default_iso: PathBuf) -> Result<ImportArgs, String> {
    let (mut iso, mut data, mut force, mut dry_run) = (None, None, false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--data" => data = Some(PathBuf::from(it.next().ok_or("--data needs a directory")?)),
            "--force" => force = true,
            "--dry-run" => dry_run = true,
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ if iso.is_none() => iso = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument {a}")),
        }
    }
    Ok(ImportArgs {
        iso: iso.unwrap_or(default_iso),
        data,
        force,
        dry_run,
    })
}

/// What an import wrote, or would write.
#[derive(Debug, Default)]
pub struct Imported {
    pub copied: Vec<Copied>,
    pub written: Vec<PathBuf>,
}

/// Checks every campaign map's header, then copies them and writes the
/// manifest. Nothing is written if a header is bad.
pub fn install<R: Read + Seek>(
    iso: &mut Xiso<R>,
    out: &OutDir,
    data_root: &Path,
    dry_run: bool,
) -> Result<Imported> {
    let d = Disc::open(iso)?;
    let maps: Vec<Entry> = d.campaign_maps()?.into_iter().cloned().collect();
    for e in &maps {
        read_header(iso, e)?;
    }
    let comps = |name: &str| -> Vec<String> {
        SOURCE_DIR
            .iter()
            .map(|s| s.to_string())
            .chain([name.to_string()])
            .collect()
    };
    let at = |c: &[String]| c.iter().fold(data_root.to_path_buf(), |p, c| p.join(c));
    let mut done = Imported::default();
    for e in &maps {
        let name = disc::file_name(e).to_ascii_lowercase();
        let c = comps(&name);
        if dry_run {
            done.copied.push(Copied {
                name,
                size: e.size as u64,
                sha256: [0; 32],
            });
            done.written.push(at(&c));
            continue;
        }
        let mut hashed = None;
        let path = out
            .write_with(&c, |w| {
                let mut h = disc::HashingWriter::new(w);
                iso.copy_to(e, &mut h)
                    .map_err(|err| std::io::Error::other(err.to_string()))?;
                hashed = Some(h.finish());
                Ok(())
            })
            .with_context(|| format!("writing {}", e.path))?;
        let (size, sha256) = hashed.expect("written");
        done.copied.push(Copied { name, size, sha256 });
        done.written.push(path);
    }
    let c = comps(MANIFEST);
    done.written.push(if dry_run {
        at(&c)
    } else {
        out.write(&c, disc::manifest(&d, &done.copied).as_bytes())?
    });
    Ok(done)
}

fn import(args: &[String]) -> Result<()> {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{IMPORT_USAGE}");
        return Ok(());
    }
    let default_iso = crate::repo_root().join("disc").join("halo1.iso");
    let a =
        parse_import(args, default_iso).map_err(|e| anyhow::anyhow!("{e}\n\n{IMPORT_USAGE}"))?;
    let data = a
        .data
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("h2boot-data"));
    let mut iso = crate::open_iso(&a.iso)?;
    let out = OutDir::new(&data, a.force)?.protecting(&a.iso);
    let done = install(&mut iso, &out, &data, a.dry_run)?;
    let verb = if a.dry_run { "would write" } else { "wrote" };
    for p in &done.written {
        println!("{verb} {}", p.display());
    }
    println!(
        "{} map(s) from {} into {}",
        done.copied.len(),
        a.iso.display(),
        data.display()
    );
    Ok(())
}

/// The arguments of `h1 convert`.
#[derive(Debug, PartialEq)]
pub struct ConvertArgs {
    pub path: PathBuf,
    pub map: Option<String>,
    pub out: Option<PathBuf>,
    pub sandbox: Sandbox,
    pub base: Option<String>,
    pub disc: Option<PathBuf>,
    pub dry_run: bool,
    pub force: bool,
}

pub fn parse_convert(args: &[String]) -> Result<ConvertArgs, String> {
    let mut a = ConvertArgs {
        path: PathBuf::new(),
        map: None,
        out: None,
        sandbox: Sandbox::Halo2,
        base: None,
        disc: None,
        dry_run: false,
        force: false,
    };
    let mut path = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |what: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs {what}"))
        };
        match arg.as_str() {
            "--out" => a.out = Some(value("a directory")?.into()),
            "--disc" => a.disc = Some(value("the Halo 2 disc")?.into()),
            "--base" => a.base = Some(value("a Halo 2 map name")?),
            "--sandbox" => {
                a.sandbox = match value("h2 or h1")?.as_str() {
                    "h2" => Sandbox::Halo2,
                    "h1" => Sandbox::Halo1,
                    other => return Err(format!("--sandbox is h2 or h1, not {other}")),
                }
            }
            "--dry-run" => a.dry_run = true,
            "--force" => a.force = true,
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ if path.is_none() => path = Some(PathBuf::from(arg)),
            _ if a.map.is_none() => a.map = Some(arg.clone()),
            _ => return Err(format!("unexpected argument {arg}")),
        }
    }
    a.path = path.ok_or("missing the Halo 1 disc or map")?;
    if a.out.is_none() && !a.dry_run {
        return Err("--out is needed unless --dry-run".into());
    }
    Ok(a)
}

/// The Halo 1 map's file bytes as stored (for its hash) and the map.
fn read_source(path: &Path, map: Option<&str>) -> Result<(Vec<u8>, H1Map)> {
    crate::safe_out::protect(path);
    let f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let bytes = match Xiso::open(BufReader::new(f)) {
        Ok(mut iso) => {
            let name = map.context("a disc needs a map name, such as beavercreek")?;
            let d = Disc::open(&mut iso)?;
            let e = d
                .map(name)
                .with_context(|| format!("no map {name} on the disc"))?
                .clone();
            if e.size > MAP_MAX {
                bail!("{} is {} bytes, too big for a map", e.path, e.size);
            }
            iso.read(&e)?
        }
        Err(_) => {
            if map.is_some() {
                bail!(
                    "{} is not a disc image, so it takes no map name",
                    path.display()
                );
            }
            std::fs::read(path)?
        }
    };
    let m = H1Map::from_reader(Cursor::new(bytes.as_slice()))
        .with_context(|| path.display().to_string())?;
    Ok((bytes, m))
}

/// A Halo 2 map's bytes from the disc.
fn read_h2_map(disc: &Path, name: &str) -> Result<Vec<u8>> {
    crate::safe_out::protect(disc);
    let f = File::open(disc).with_context(|| format!("opening {}", disc.display()))?;
    let mut iso = Xiso::open(BufReader::new(f))?;
    let e = iso
        .entries()?
        .into_iter()
        .find(|e| {
            e.path
                .rsplit(['/', '\\'])
                .next()
                .is_some_and(|f| f.eq_ignore_ascii_case(&format!("{name}.map")))
        })
        .with_context(|| format!("no {name}.map on {}", disc.display()))?;
    Ok(iso.read(&e)?)
}

fn convert(args: &[String]) -> Result<()> {
    let a = parse_convert(args).map_err(|e| anyhow::anyhow!("{e}\n\n{H1_USAGE}"))?;
    let (file, h1) = read_source(&a.path, a.map.as_deref())?;
    let h1_name = h1.header().name.clone();
    let base_name = a
        .base
        .clone()
        .or_else(|| known(&h1_name).map(|k| k.1.to_owned()))
        .unwrap_or_else(|| "beavercreek".into());
    let disc = match &a.disc {
        Some(d) => d.clone(),
        None => std::env::var_os("H2_DISC")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../disc/halo2.iso")),
    };
    let mut base = map::Map::from_reader(Cursor::new(read_h2_map(&disc, &base_name)?))
        .with_context(|| format!("reading {base_name}.map"))?;
    let sha = content::Sha256::of(&file);
    let p = prepare(&h1, sha, &mut base, a.sandbox)?;
    let name = map_name(&h1_name);
    let vertices: usize = p.sections.iter().map(|s| s.vertices.len()).sum();
    let triangles: usize = p.sections.iter().map(|s| s.indices.len() / 3).sum();
    println!(
        "{h1_name} ({}) onto {base_name}, {} sandbox, as {name}",
        p.display_name,
        a.sandbox.name()
    );
    println!(
        "  structure: {} cluster(s), {vertices} vertices, {triangles} triangles, {} materials, \
         {} collision materials, {} collision surfaces{}",
        p.sections.len(),
        p.materials.len(),
        p.collision_materials,
        p.collision.surfaces.len(),
        if p.flipped { ", winding flipped" } else { "" }
    );
    let [spawns, flags, items] = p.applied.counts;
    let src = p.source;
    println!(
        "  placements: {spawns} spawns, {flags} flags, {items} items ({} of them vehicles); \
         Halo 1 has {} spawns, {} flags, {} items, {} vehicles",
        p.applied.vehicles, src.spawns, src.flags, src.items, src.vehicles
    );
    println!(
        "  base: {} placements removed, {} trigger volumes moved out of the level",
        p.applied.removed, p.trigger_volumes
    );
    match &p.lightmap {
        Some(l) => println!(
            "  lightmap: {} pages from Halo 1's {}, in the unused slot of {}",
            l.pages, l.source, l.replaces
        ),
        None => println!("  lightmap: none, the level is unlit"),
    }
    for (h1s, (_, b)) in p.materials.iter().zip(&p.material_shaders) {
        println!("  shader {h1s} -> {b} (the base's, a stand-in)");
    }
    for c in &p.applied.collections {
        println!(
            "  built vehicle collection {}{} in the unused slot of {}",
            c.name,
            c.variant
                .map_or_else(String::new, |v| format!(" (variant {v})")),
            c.replaces
        );
    }
    for u in p.applied.unresolved.iter().chain(&p.dropped) {
        println!("  not placed: {u}");
    }
    let mopp = structure_mopp(&p.collision);
    let dir = a.out.as_ref().map(|o| o.join(&name));
    let where_ = dir
        .as_ref()
        .map_or_else(|| "<--out>".into(), |d| d.display().to_string());
    if a.dry_run {
        println!(
            "would write {where_}/manifest.toml and {where_}/{}",
            h1convert::convert::variant_file(&h1_name, a.sandbox)
        );
        if let Err(e) = mopp {
            println!("but would refuse: {e}");
        }
        return Ok(());
    }
    let mopp = mopp?;
    let done = p.build(&mut base, &mopp)?;
    let out = OutDir::new(a.out.clone().expect("checked"), a.force)?.protecting(&a.path);
    // One package per map; this run's variant beside any other already
    // there (docs/game/h1-multiplayer.md, "Package").
    let manifest_path = dir
        .as_ref()
        .expect("checked")
        .join(content::manifest::MANIFEST_FILE);
    let mut manifest = done.manifest.clone();
    if let Some(old) = std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|t| content::Manifest::from_toml(&t).ok())
    {
        let ours = a.sandbox.name();
        manifest.variants.extend(
            old.variants
                .into_iter()
                .filter(|v| v.origin.sandbox != ours),
        );
        manifest
            .variants
            .sort_by(|x, y| x.origin.sandbox.cmp(&y.origin.sandbox));
    }
    let file = done.manifest.variants[0].file.clone();
    let parts: Vec<String> = std::iter::once(name.clone())
        .chain(file.split('/').map(str::to_owned))
        .collect();
    let map_file = out.write(&parts, &done.bytes)?;
    let manifest = out.write(
        &[name.clone(), content::manifest::MANIFEST_FILE.to_owned()],
        manifest.to_toml().as_bytes(),
    )?;
    println!("wrote {} ({} bytes)", map_file.display(), done.bytes.len());
    println!("wrote {}", manifest.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn parses_convert_arguments() {
        let a = parse_convert(&s(&[
            "h1.iso",
            "beavercreek",
            "--out",
            "o",
            "--sandbox",
            "h2",
        ]))
        .unwrap();
        assert_eq!(a.path, PathBuf::from("h1.iso"));
        assert_eq!(a.map.as_deref(), Some("beavercreek"));
        assert_eq!(a.out, Some(PathBuf::from("o")));
        assert_eq!(a.sandbox, Sandbox::Halo2);
        assert!(!a.dry_run && !a.force && a.base.is_none() && a.disc.is_none());
        let a = parse_convert(&s(&[
            "x.map",
            "--dry-run",
            "--base",
            "lockout",
            "--sandbox",
            "h1",
        ]))
        .unwrap();
        assert!(a.dry_run && a.out.is_none());
        assert_eq!(
            (a.base.as_deref(), a.sandbox),
            (Some("lockout"), Sandbox::Halo1)
        );
        assert!(parse_convert(&s(&["x.map"])).unwrap_err().contains("--out"));
        assert!(parse_convert(&s(&["x.map", "--sandbox", "h3", "--dry-run"])).is_err());
        assert!(parse_convert(&s(&["x.map", "--out"])).is_err());
        assert!(parse_convert(&s(&["--dry-run"])).is_err());
        assert!(parse_convert(&s(&["a", "b", "c", "--dry-run"])).is_err());
    }

    #[test]
    fn parses_bitmap_arguments() {
        let a = parse_bitmap(&s(&[
            "h1.iso",
            "a30",
            "levels\\a30\\a30_a",
            "--out",
            "o",
            "--mips",
        ]))
        .unwrap();
        assert_eq!(
            (a.map.as_deref(), a.tag.as_str(), a.mips, a.force),
            (Some("a30"), "levels\\a30\\a30_a", true, false)
        );
        let a = parse_bitmap(&s(&["x.map", "t", "--out", "o", "--force"])).unwrap();
        assert_eq!((a.map, a.tag.as_str(), a.force), (None, "t", true));
        assert!(
            parse_bitmap(&s(&["x.map", "t"])).is_err(),
            "--out is needed"
        );
        assert!(
            parse_bitmap(&s(&["x.map", "--out", "o"])).is_err(),
            "a tag is needed"
        );
        assert!(parse_bitmap(&s(&["a", "b", "c", "d", "--out", "o"])).is_err());
        assert!(parse_bitmap(&s(&["a", "b", "--out", "o", "--bogus"])).is_err());
    }

    #[test]
    fn parses_view_arguments() {
        let a = parse_view(&s(&["x.iso", "a30", "--group", "hmt"]), true).unwrap();
        assert_eq!(a.path, PathBuf::from("x.iso"));
        assert_eq!(a.map.as_deref(), Some("a30"));
        assert_eq!(a.group, Group::parse("hmt "));
        assert!(parse_view(&s(&["x.iso", "--group", "scnr"]), false).is_err());
        assert!(parse_view(&s(&["x.iso", "--group", "toolong"]), true).is_err());
        assert!(parse_view(&s(&[]), false).is_err());
        assert!(parse_view(&s(&["a", "b", "c"]), false).is_err());
    }

    #[test]
    fn parses_import_arguments() {
        let d = PathBuf::from("disc/halo1.iso");
        let a = parse_import(&s(&["--data", "/x", "--force"]), d.clone()).unwrap();
        assert_eq!(
            a,
            ImportArgs {
                iso: d.clone(),
                data: Some("/x".into()),
                force: true,
                dry_run: false
            }
        );
        let a = parse_import(&s(&["my.iso", "--dry-run"]), d.clone()).unwrap();
        assert_eq!((a.iso, a.dry_run), (PathBuf::from("my.iso"), true));
        assert!(parse_import(&s(&["--data"]), d.clone()).is_err());
        assert!(parse_import(&s(&["a.iso", "b.iso"]), d).is_err());
    }

    #[test]
    fn imports_the_campaign_maps_with_a_manifest() {
        let img = h1map::testing::campaign_disc();
        let root = std::env::temp_dir().join(format!("h1-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let out = OutDir::new(&root, false).unwrap();
        let mut iso = Xiso::open(Cursor::new(img.clone())).unwrap();
        let dry = install(&mut iso, &out, &root, true).unwrap();
        assert_eq!(dry.written.len(), 12, "10 levels, ui.map, the manifest");
        assert!(!root.join("campaigns").exists(), "a dry run writes nothing");

        let done = install(&mut iso, &out, &root, false).unwrap();
        let dir = root.join("campaigns/halo1/source");
        let map = h1map::testing::tiny_map();
        for c in &done.copied {
            assert_eq!(std::fs::read(dir.join(&c.name)).unwrap(), map, "{}", c.name);
        }
        assert!(
            !dir.join("bloodgulch.map").exists(),
            "multiplayer maps stay on the disc"
        );
        let manifest = std::fs::read_to_string(dir.join(MANIFEST)).unwrap();
        assert!(manifest.starts_with("# h2 import halo1: title 4D530004"));
        assert_eq!(manifest.lines().count(), 2 + 11);
        let mut want = Vec::new();
        let (_, sha) = disc::copy_hashed(&mut map.as_slice(), &mut want).unwrap();
        assert!(manifest.contains(&format!("{}  {}  a30.map", disc::hex(&sha), map.len())));
        // Again without --force: refused, nothing clobbered.
        assert!(install(&mut iso, &out, &root, false).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_disc_with_a_bad_map_header_writes_nothing() {
        let mut img = h1map::testing::campaign_disc();
        let mut iso = Xiso::open(Cursor::new(img.clone())).unwrap();
        let e = Disc::open(&mut iso).unwrap().map("d40").unwrap().clone();
        let at = iso.partition_offset() as usize + e.sector as usize * xiso::SECTOR as usize;
        img[at..at + 4].copy_from_slice(b"junk");
        let root = std::env::temp_dir().join(format!("h1-import-bad-{}", std::process::id()));
        let out = OutDir::new(&root, false).unwrap();
        let mut iso = Xiso::open(Cursor::new(img)).unwrap();
        assert!(install(&mut iso, &out, &root, false).is_err());
        assert!(!root.join("campaigns").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
