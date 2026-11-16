//! Disc-backed map commands. Keep their argument handling out of the XBE path.
use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use map::{FourCC, Map};

const USAGE: &str = "usage: h2 maps [disc.iso]\n       h2 maps install [--data DIR] [--disc disc.iso]\n       h2 tags <map-name-or-file> [group] [--disc disc.iso]\n       h2 tag <map-name-or-file> <tag-name> [group] [--lenient] [--disc disc.iso]";

pub(super) fn run(command: &str, args: Vec<String>) -> Result<()> {
    if command == "maps" && args.first().is_some_and(|a| a == "install") {
        return install(&args[1..]);
    }
    let mut args = args.into_iter();
    let mut positional = Vec::new();
    let mut disc = None;
    let mut mode = tags::DecodeMode::Strict;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--lenient" if command == "tag" => mode = tags::DecodeMode::Lenient,
            "--disc" => {
                if disc.is_some() {
                    bail!("--disc specified twice");
                }
                disc = Some(PathBuf::from(
                    args.next().context("--disc requires an ISO path")?,
                ));
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            a if a.starts_with('-') => bail!("unknown option {a}\n{USAGE}"),
            _ => positional.push(arg),
        }
    }
    match (command, positional.as_slice()) {
        ("maps", []) => list_maps(&disc.map(Ok).unwrap_or_else(super::default_disc)?),
        ("maps", [path]) if disc.is_none() => list_maps(Path::new(path)),
        ("tags", [name]) => list_tags(name, None, disc),
        ("tags", [name, group]) => list_tags(name, Some(group.parse()?), disc),
        ("tag", [name, tag]) => dump_tag(name, tag, None, disc, mode),
        ("tag", [name, tag, group]) => dump_tag(name, tag, Some(group.parse()?), disc, mode),
        _ => bail!("{USAGE}"),
    }
}

pub(super) fn disc_map(
    path: &Path,
    partition: u64,
    entry: &xiso::Entry,
) -> Result<Map<BufReader<File>>> {
    let base = partition + u64::from(entry.sector) * xiso::SECTOR;
    Map::from_region(BufReader::new(File::open(path)?), base, entry.size.into())
        .with_context(|| format!("parsing {}", entry.path))
}

fn list_maps(path: &Path) -> Result<()> {
    let mut iso = super::open_iso(path)?;
    println!(
        "{:<28} {:>3} {:<18} {:>10} {:>6} {:>7}  build",
        "map", "ver", "type", "bytes", "tags", "strings"
    );
    let mut count = 0;
    let mut total = 0;
    for entry in iso
        .entries()?
        .iter()
        .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".map"))
    {
        let map = disc_map(path, iso.partition_offset(), entry)?;
        let h = map.header();
        println!(
            "{:<28} {:>3} {:<18} {:>10} {:>6} {:>7}  {}",
            entry.path.rsplit('/').next().unwrap_or(&entry.path),
            h.version,
            format!("{:?}", h.cache_type),
            h.file_size,
            map.tags().len(),
            map.strings().len(),
            h.build
        );
        count += 1;
        total += map.tags().len();
    }
    println!("{count} maps, {total} tag entries (shared identities counted per map)");
    Ok(())
}

pub(super) fn load_map(name: &str, disc: Option<PathBuf>) -> Result<Map<BufReader<File>>> {
    if Path::new(name).is_file() {
        Map::from_reader(BufReader::new(File::open(name)?))
            .with_context(|| format!("parsing {name}"))
    } else {
        let disc = disc.map(Ok).unwrap_or_else(super::default_disc)?;
        let mut iso = super::open_iso(&disc)?;
        let mut path = name.trim_start_matches('/').to_owned();
        if !path.to_ascii_lowercase().ends_with(".map") {
            path.push_str(".map");
        }
        if !path.contains('/') {
            path = format!("maps/{path}");
        }
        let entry = iso.find(&path)?;
        disc_map(&disc, iso.partition_offset(), &entry)
    }
}

fn list_tags(name: &str, group: Option<FourCC>, disc: Option<PathBuf>) -> Result<()> {
    let map = load_map(name, disc)?;
    let mut counts = BTreeMap::<FourCC, usize>::new();
    let mut shown = 0;
    println!("datum       group  file offset      bytes  name");
    for tag in map.tags() {
        *counts.entry(tag.group).or_default() += 1;
        if group.is_some_and(|g| tag.group != g) {
            continue;
        }
        let (offset, size) = match tag.location {
            Some(loc) => (format!("{:#010x}", loc.file_offset()?), loc.size),
            None => ("absent".to_owned(), 0),
        };
        println!(
            "{}  {}   {:>10}  {:>9}  {}",
            tag.id,
            tag.group,
            offset,
            size,
            tag.name.as_deref().unwrap_or("<unnamed>")
        );
        shown += 1;
    }
    println!("\ngroup counts (whole map)");
    for (class, count) in counts {
        println!("  {class}  {count}");
    }
    println!(
        "{shown} shown, {} total tags, {} index slots",
        map.tags().len(),
        map.index_slots()
    );
    Ok(())
}

fn dump_tag(
    name: &str,
    tag_name: &str,
    group: Option<FourCC>,
    disc: Option<PathBuf>,
    mode: tags::DecodeMode,
) -> Result<()> {
    let mut map = load_map(name, disc)?;
    let query = match group {
        Some(group) => map::TagQuery::NameInGroup(tag_name, group),
        None => map::TagQuery::Name(tag_name),
    };
    let id = map.tag(query)?.id;
    let decoded = tags::DecodedTag::read_with_mode(&mut map, id, mode)?;
    println!("{} {} {}", decoded.id, decoded.group, tag_name);
    println!("{:#?}", decoded.data);
    println!("Structural checks: {:?}", decoded.validation);
    for warning in &decoded.warnings {
        println!(
            "warning: {}.{} {:?}: preserved, layout and bounds unverified",
            warning.record, warning.field, warning.block
        );
    }
    println!(
        "Bitmap resource locations are decoded; external files are not opened by this command."
    );
    Ok(())
}

/// `h2 maps install`: puts every map package whose map name the disc lacks
/// into 1.0's map list as a content package, and removes the ones whose
/// package is gone (docs/maps.md §4). The boot does the same at start-up.
fn install(args: &[String]) -> Result<()> {
    let (mut data, mut disc) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--data" => {
                data = Some(PathBuf::from(
                    it.next().context("--data needs a directory")?,
                ))
            }
            "--disc" => {
                disc = Some(PathBuf::from(
                    it.next().context("--disc needs an ISO path")?,
                ))
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => bail!("unexpected argument {a}\n{USAGE}"),
        }
    }
    let data = data.unwrap_or_else(content::store::default_data_root);
    let disc = match disc {
        Some(d) => d,
        None => super::default_disc()?,
    };
    let r = boot::new_maps::install(&data, &mut boot::new_maps::IsoDisc::new(&disc))?;
    for n in &r.notices {
        println!("{n}");
    }
    for (name, offer, id) in &r.installed {
        println!(
            "{name}: map id {id:#x}, {}",
            vfs::Config::new(&data, boot::new_maps::HALO2_TITLE)
                .content_dir(*offer)
                .display()
        );
    }
    for p in &r.removed {
        println!("removed {}", p.display());
    }
    for s in &r.skipped {
        println!("skipped {s}");
    }
    println!(
        "{} new map(s) in the map list, {} written, {} removed",
        r.installed.len(),
        r.materialised.len(),
        r.removed.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn lenient_is_an_explicit_single_tag_option() {
        assert!(super::run("tag", vec!["--lenient".into(), "--help".into()]).is_ok());
        for command in ["maps", "tags"] {
            assert!(super::run(command, vec!["--lenient".into(), "--help".into()]).is_err());
        }
    }
}
