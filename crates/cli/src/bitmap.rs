//! `h2 bitmap`: decode bitmap tags to PNG, or bulk-check a whole map.
//! Thin wrapper over `crates/assets`; pixel data may live in sibling maps.
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use assets::{Region, ResourceFiles};
use map::{FourCC, Map, ResourceFile, TagQuery};
use tags::{DecodedTag, TagData};

const USAGE: &str = "usage: h2 bitmap <map> <tag name> [--image N] [--out DIR] [--mips] [--disc disc.iso]
       h2 bitmap --all <map> [--list] [--out DIR] [--disc disc.iso]   (--list: one line per image)

<map> is a map name on the disc (mainmenu, c10_...) or a local .map file;
sibling shared maps are found on the same disc / directory. Without --out
nothing is written. --mips also writes every mip level (default: base only);
--raw also dumps each LOD's undecoded bytes as .bin (for format debugging).";

const BITM: FourCC = FourCC(*b"bitm");

fn sibling(name: &str) -> String {
    format!("maps/{name}.map")
}

/// Opens the map plus the region table for its raw-resource pointers.
fn open(name: &str, disc: Option<PathBuf>) -> Result<(Map<BufReader<File>>, ResourceFiles)> {
    let mut files = ResourceFiles::new();
    if Path::new(name).is_file() {
        let path = PathBuf::from(name);
        let length = fs::metadata(&path)?.len();
        files.set(ResourceFile::Local, Region { path: path.clone(), base: 0, length })?;
        let dir = path.parent().unwrap_or(Path::new("."));
        for (sel, n) in [
            (ResourceFile::MainMenu, "mainmenu.map"),
            (ResourceFile::Shared, "shared.map"),
            (ResourceFile::SinglePlayerShared, "single_player_shared.map"),
        ] {
            let p = dir.join(n);
            if let Ok(m) = fs::metadata(&p) {
                files.set(sel, Region { path: p, base: 0, length: m.len() })?;
            }
        }
        let map = Map::from_reader(BufReader::new(File::open(&path)?))
            .with_context(|| format!("parsing {name}"))?;
        return Ok((map, files));
    }
    let disc = disc.map(Ok).unwrap_or_else(super::default_disc)?;
    let mut iso = super::open_iso(&disc)?;
    let base_of = |e: &xiso::Entry, part: u64| part + u64::from(e.sector) * xiso::SECTOR;
    let part = iso.partition_offset();
    let want = name.trim_start_matches('/').trim_end_matches(".map").trim_start_matches("maps/");
    let entry = iso.find(&sibling(want))?;
    files.set(
        ResourceFile::Local,
        Region { path: disc.clone(), base: base_of(&entry, part), length: entry.size.into() },
    )?;
    for (sel, n) in [
        (ResourceFile::MainMenu, "mainmenu"),
        (ResourceFile::Shared, "shared"),
        (ResourceFile::SinglePlayerShared, "single_player_shared"),
    ] {
        if let Ok(e) = iso.find(&sibling(n)) {
            files.set(sel, Region { path: disc.clone(), base: base_of(&e, part), length: e.size.into() })?;
        }
    }
    let map = Map::from_region(
        BufReader::new(File::open(&disc)?),
        base_of(&entry, part),
        entry.size.into(),
    )
    .with_context(|| format!("parsing {want}"))?;
    Ok((map, files))
}

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    let (mut positional, mut all, mut mips, mut raw) = (Vec::new(), false, false, false);
    let mut list = false;
    let (mut image, mut out, mut disc) = (None, None, None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--all" => all = true,
            "--mips" => mips = true,
            "--raw" => raw = true,
            "--list" => list = true,
            "--image" => image = Some(args.next().context("--image needs N")?.parse::<usize>()?),
            "--out" => out = Some(PathBuf::from(args.next().context("--out needs a dir")?)),
            "--disc" => disc = Some(PathBuf::from(args.next().context("--disc needs an ISO")?)),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            s if s.starts_with("--") => bail!("unknown option {s}\n{USAGE}"),
            _ => positional.push(a),
        }
    }
    match (all, positional.as_slice()) {
        (true, [map]) => check_all(map, disc, out.as_deref(), list),
        (false, [map, tag]) => one(map, tag, disc, image, out.as_deref(), mips, raw),
        _ => bail!("{USAGE}"),
    }
}

/// Output file stem: sanitised tag name plus the tag's datum, so names that
/// sanitise identically (`a\b`, `a_b`) still get distinct files.
fn file_stem(tag: &str, id: map::TagId) -> String {
    let clean: String = tag
        .chars()
        .take(120)
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    format!("{clean}_{:08x}", id.0)
}

/// Creates output files inside one directory, never following symlinks and
/// never reusing a name within a run.
struct Out {
    dir: PathBuf,
    seen: HashSet<String>,
}

impl Out {
    fn new(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        Ok(Self { dir: dir.to_owned(), seen: HashSet::new() })
    }

    /// `name` must be a plain file name. An existing regular file is replaced;
    /// an existing symlink (or anything else) is an error, and a name already
    /// written in this run is a collision error.
    fn create(&mut self, name: &str) -> Result<(File, PathBuf)> {
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
            bail!("unsafe output file name {name:?}");
        }
        if !self.seen.insert(name.to_owned()) {
            bail!("output name collision: {name}");
        }
        let path = self.dir.join(name);
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                bail!("refusing to write through symlink {}", path.display())
            }
            Ok(m) if m.is_file() => fs::remove_file(&path)?,
            Ok(_) => bail!("{} exists and is not a regular file", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        // create_new is O_EXCL: it fails rather than follow a symlink that
        // appears after the check above.
        let f = fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        Ok((f, path))
    }
}

/// Writes the levels of one decoded image; returns the files written.
fn write_image(
    out: &mut Out,
    stem: &str,
    index: usize,
    img: &assets::DecodedImage,
    mips: bool,
) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    let cube = img.desc.faces() > 1;
    for l in &img.levels {
        if l.level != 0 && !mips {
            continue;
        }
        let slice = l.width as usize * l.height as usize * 4;
        for z in 0..l.depth {
            let mut name = format!("{stem}_img{index}");
            if cube {
                name += &format!("_face{}", l.face);
            }
            if l.depth > 1 {
                name += &format!("_z{z}");
            }
            if mips {
                name += &format!("_mip{}", l.level);
            }
            let (f, path) = out.create(&(name + ".png"))?;
            let z = z as usize;
            assets::png::write_rgba(BufWriter::new(f), l.width, l.height, &l.rgba[z * slice..(z + 1) * slice])?;
            written.push(path);
        }
    }
    Ok(written)
}

fn one(
    map_name: &str,
    tag: &str,
    disc: Option<PathBuf>,
    image: Option<usize>,
    out: Option<&Path>,
    mips: bool,
    raw: bool,
) -> Result<()> {
    let (mut map, mut files) = open(map_name, disc)?;
    let id = map.tag(TagQuery::NameInGroup(tag, BITM))?.id;
    let decoded = DecodedTag::read(&mut map, id)?;
    let TagData::Bitmap(bitmap) = decoded.data else { bail!("{tag} is not a bitmap") };
    println!("{tag}: {} image(s)", bitmap.images.len());
    if let Some(n) = image
        && n >= bitmap.images.len()
    {
        bail!("--image {n} out of range: the tag has {} image(s)", bitmap.images.len());
    }
    let stem = file_stem(tag, id);
    let mut out = out.map(Out::new).transpose()?;
    let mut failed = 0usize;
    for (i, im) in bitmap.images.iter().enumerate() {
        if image.is_some_and(|n| n != i) {
            continue;
        }
        let storage = if im.is_linear() { "linear" } else if im.is_swizzled() { "swizzled" } else { "plain" };
        print!(
            "  [{i}] {:?} {:?} {}x{}x{} mips={} {storage} flags={:#x}: ",
            im.format, im.kind, im.dimensions[0], im.dimensions[1], im.dimensions[2], im.mip_count,
            im.wire.flags.0
        );
        if let (Some(out), true) = (out.as_mut(), raw) {
            for (l, r) in im.lods.iter().enumerate() {
                if let Some(r) = r {
                    // Raw dumps are capped like every other read.
                    let bytes = files.read(r.pointer.file, r.pointer.offset, r.size)?;
                    let (mut f, _) = out.create(&format!("{stem}_img{i}_lod{l}.bin"))?;
                    std::io::Write::write_all(&mut f, &bytes)?;
                }
            }
        }
        match assets::decode_image(im, &mut files) {
            Ok(d) => {
                println!("ok (lod {}, {} level(s), {:?})", d.lod, d.levels.len(), d.desc.format);
                if let Some(out) = out.as_mut() {
                    for p in write_image(out, &stem, i, &d, mips)? {
                        println!("      wrote {}", p.display());
                    }
                }
            }
            Err(e) => {
                failed += 1;
                println!("FAILED: {e}");
            }
        }
    }
    if failed > 0 {
        bail!("{failed} image(s) failed to decode");
    }
    Ok(())
}

fn check_all(map_name: &str, disc: Option<PathBuf>, out: Option<&Path>, verbose: bool) -> Result<()> {
    let (mut map, mut files) = open(map_name, disc)?;
    let mut out = out.map(Out::new).transpose()?;
    let list: Vec<_> = map
        .tags()
        .iter()
        .filter(|t| t.group == BITM)
        .map(|t| (t.id, t.name.clone().unwrap_or_default(), t.location.is_some()))
        .collect();
    // key: "format kind storage" -> (ok, failed)
    let mut by_kind: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut reasons: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut suspects: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut sources: BTreeMap<String, usize> = BTreeMap::new();
    let (mut tags_ok, mut tags_failed, mut tags_absent, mut images) = (0, 0, 0, 0);
    for (id, name, local) in &list {
        if !local {
            tags_absent += 1;
            continue;
        }
        let decoded = match DecodedTag::read(&mut map, *id) {
            Ok(d) => d,
            Err(e) => {
                tags_failed += 1;
                reasons.entry(format!("tag decode: {e}")).or_default().push(name.clone());
                continue;
            }
        };
        let TagData::Bitmap(bitmap) = decoded.data else { continue };
        tags_ok += 1;
        for (i, im) in bitmap.images.iter().enumerate() {
            images += 1;
            if let Some(r) = im.lods.iter().flatten().next() {
                *sources.entry(format!("{:?}", r.pointer.file)).or_default() += 1;
            }
            let storage = if im.is_linear() { "linear" } else if im.is_swizzled() { "swizzled" } else { "plain" };
            let key = format!("{:?} {:?} {storage}", im.format, im.kind);
            if verbose {
                println!("{key} {}x{}x{} mips={} {name}[{i}]", im.dimensions[0], im.dimensions[1], im.dimensions[2], im.mip_count);
            }
            let slot = by_kind.entry(key.clone()).or_default();
            match assets::decode_image(im, &mut files) {
                Ok(d) => {
                    slot.0 += 1;
                    if !d.size_matches() {
                        suspects
                            .entry(format!("{key}: lod{} size {} vs layout {}", d.lod, d.data_len, d.layout_len))
                            .or_default()
                            .push(format!("{name}[{i}]"));
                    }
                    if let Some(out) = out.as_mut() {
                        write_image(out, &file_stem(name, *id), i, &d, false)?;
                    }
                }
                Err(e) => {
                    slot.1 += 1;
                    reasons.entry(format!("{key}: {e}")).or_default().push(format!("{name}[{i}]"));
                }
            }
        }
    }
    println!("{map_name}: {} bitmap tags ({tags_absent} without local payload, {tags_failed} tag-decode failures), {tags_ok} decoded tags, {images} images", list.len());
    println!("pixel data source file (first LOD): {sources:?}");
    println!("\n{:<34} {:>8} {:>8}", "format kind storage", "decoded", "failed");
    let (mut ok, mut bad) = (0, 0);
    for (k, (o, f)) in &by_kind {
        println!("{k:<34} {o:>8} {f:>8}");
        ok += o;
        bad += f;
    }
    println!("{:<34} {ok:>8} {bad:>8}", "TOTAL");
    let n_suspect: usize = suspects.values().map(Vec::len).sum();
    println!("\nsize check: {} of {} decoded images have LOD size == layout rounded up to 128", ok - n_suspect, ok);
    for (r, names) in &suspects {
        println!("  {} x {r}", names.len());
        for n in names.iter().take(3) {
            println!("      e.g. {n}");
        }
    }
    if !reasons.is_empty() {
        println!("\nfailure reasons");
        for (r, names) in &reasons {
            println!("  {} x {r}", names.len());
            for n in names.iter().take(3) {
                println!("      e.g. {n}");
            }
        }
    }
    if tags_failed + bad + n_suspect > 0 {
        bail!("{tags_failed} tag decode failure(s), {bad} image decode failure(s), {n_suspect} size mismatch(es)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-bitmap-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(unix)]
    #[test]
    fn output_never_follows_a_symlink_out_of_the_directory() {
        let root = scratch("symlink");
        let out_dir = root.join("out");
        fs::create_dir_all(&out_dir).unwrap();
        let victim = root.join("victim.txt");
        fs::write(&victim, b"precious").unwrap();
        std::os::unix::fs::symlink(&victim, out_dir.join("x.png")).unwrap();
        let mut out = Out::new(&out_dir).unwrap();
        assert!(out.create("x.png").is_err());
        assert_eq!(fs::read(&victim).unwrap(), b"precious");
        // dangling link too
        std::os::unix::fs::symlink(root.join("nowhere"), out_dir.join("y.png")).unwrap();
        assert!(out.create("y.png").is_err());
        assert!(!root.join("nowhere").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn existing_regular_files_are_replaced_and_names_stay_inside() {
        let root = scratch("regular");
        fs::write(root.join("a.png"), b"old").unwrap();
        let mut out = Out::new(&root).unwrap();
        let (mut f, p) = out.create("a.png").unwrap();
        std::io::Write::write_all(&mut f, b"new").unwrap();
        drop(f);
        assert_eq!(fs::read(p).unwrap(), b"new");
        for bad in ["../evil.png", "a/b.png", "a\\b.png", "..", ""] {
            assert!(out.create(bad).is_err(), "{bad:?}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn colliding_sanitised_names_get_distinct_stems_and_repeats_are_detected() {
        let a = file_stem("a\\b", map::TagId(0xe1000001));
        let b = file_stem("a_b", map::TagId(0xe1000002));
        assert_ne!(a, b);
        let root = scratch("collide");
        let mut out = Out::new(&root).unwrap();
        out.create(&format!("{a}.png")).unwrap();
        out.create(&format!("{b}.png")).unwrap();
        assert!(out.create(&format!("{a}.png")).is_err());
        // long names are bounded
        assert!(file_stem(&"x".repeat(1000), map::TagId(1)).len() < 140);
        fs::remove_dir_all(root).unwrap();
    }
}
