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
use h1map::disc::{self, Copied, Disc};
use h1map::{Group, H1Map, HEADER_LEN};
use xiso::{Entry, Xiso};

use crate::safe_out::OutDir;

const H1_USAGE: &str = "usage: h2 h1 info <iso|map> [map]
       h2 h1 tags <iso|map> [map] [--group G]

info: with a disc and no map, the disc's title and each map's header; with a
map (a .map file, or a disc and a map name such as a30), its header, tag index
and tag counts by group. tags: every tag, or one group's: ID, group, data
range and name. See docs/formats/h1-map-format.md.";

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

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|a| a.to_string()).collect()
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
