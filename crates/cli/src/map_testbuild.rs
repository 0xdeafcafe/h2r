//! `h2 map testbuild`: write builder load-test maps for `H2_MAP_DIR`.
//! See docs/formats/map-builder.md, "Load tests".
use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use map::Map;

use crate::safe_out::{self, OutDir};

const USAGE: &str = "usage: h2 map testbuild [disc.iso] --out DIR [--map NAME] [--keep-tail] [--keep-header] [--force]
  writes DIR/moved/NAME.map (every bitmap and its pixels moved) and
  DIR/<fresh variant>/NAME.map (index tail cleared, header from scratch);
  with --keep-tail or --keep-header only the fresh variant is written;
  load one with H2_MAP_DIR=DIR/<variant> h2 play <disc> --level NAME";

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Args {
    pub disc: Option<PathBuf>,
    pub out: PathBuf,
    pub map: String,
    pub keep_tail: bool,
    pub keep_header: bool,
    pub force: bool,
}

impl Args {
    /// Subfolder of the fresh variant: `fresh`, plus one suffix per kept
    /// retail part, so bisecting loads never overwrite each other.
    pub fn fresh_dir(&self) -> String {
        let mut name = String::from("fresh");
        if self.keep_tail {
            name.push_str("-keep-tail");
        }
        if self.keep_header {
            name.push_str("-keep-header");
        }
        name
    }
}

/// Parse the arguments after `h2 map`. `Ok(None)` means help was printed.
pub(crate) fn parse(args: &[String]) -> Result<Option<Args>> {
    let mut it = args.iter();
    match it.next().map(String::as_str) {
        Some("testbuild") => {}
        Some("-h" | "--help") => return Ok(None),
        _ => bail!("{USAGE}"),
    }
    let mut disc = None;
    let mut out = None;
    let mut map = None;
    let (mut keep_tail, mut keep_header, mut force) = (false, false, false);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--out" => {
                if out.is_some() {
                    bail!("--out specified twice");
                }
                out = Some(PathBuf::from(
                    it.next().context("--out requires a directory")?,
                ));
            }
            "--map" => {
                if map.is_some() {
                    bail!("--map specified twice");
                }
                map = Some(it.next().context("--map requires a map name")?.clone());
            }
            "--keep-tail" => keep_tail = true,
            "--keep-header" => keep_header = true,
            "--force" => force = true,
            "-h" | "--help" => return Ok(None),
            a if a.starts_with('-') => bail!("unknown option {a}\n{USAGE}"),
            a => {
                if disc.is_some() {
                    bail!("unexpected argument {a}\n{USAGE}");
                }
                disc = Some(PathBuf::from(a));
            }
        }
    }
    let map = map.unwrap_or_else(|| "lockout".into());
    let map = map.strip_suffix(".map").unwrap_or(&map).to_owned();
    if map.is_empty() || !map.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        bail!("--map takes a disc map name such as lockout or 01a_tutorial");
    }
    Ok(Some(Args {
        disc,
        out: out.context(format!("--out is required\n{USAGE}"))?,
        map,
        keep_tail,
        keep_header,
        force,
    }))
}

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let Some(args) = parse(&args)? else {
        println!("{USAGE}");
        return Ok(());
    };
    let disc = args
        .disc
        .clone()
        .map(Ok)
        .unwrap_or_else(super::default_disc)?;
    safe_out::protect(&disc);
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(&disc).with_context(|| format!("opening {}", disc.display()))?,
    ))?;
    let wanted = format!("{}.map", args.map);
    let entries = iso.entries()?;
    let entry = entries
        .iter()
        .find(|e| {
            !e.is_dir
                && e.path
                    .rsplit(['/', '\\'])
                    .next()
                    .is_some_and(|n| n.eq_ignore_ascii_case(&wanted))
        })
        .with_context(|| format!("{wanted} is not on {}", disc.display()))?;
    let mut map = crate::maps::disc_map(&disc, iso.partition_offset(), entry)?;
    map.validate_checksum()
        .context("the disc map's checksum does not match")?;
    let out = OutDir::new(&args.out, args.force)?.protecting(&disc);

    // The keep flags only bisect the fresh variant; moved doesn't change.
    if !(args.keep_tail || args.keep_header) {
        let (moved, summary) = tags::rebuild::moved_variant(&mut map)?;
        let (bytes, layout) = moved
            .build()
            .context("building the moved variant (try a smaller map, e.g. lockout)")?;
        check(&bytes, &map)?;
        let path = out.write(&["moved".into(), wanted.clone()], &bytes)?;
        println!(
            "moved: {} ({} bytes): {}/{} bitmaps at new addresses, {} LODs ({} bytes) of pixels in new raw data, metadata {:#x}",
            path.display(),
            layout.file_size,
            summary.relocated,
            summary.bitmaps,
            summary.moved.moved_lods,
            summary.moved.moved_bytes,
            layout.memory_size,
        );
    }

    let (fresh, cleared) =
        tags::rebuild::fresh_variant(&mut map, args.keep_tail, args.keep_header)?;
    let (bytes, layout) = fresh.build().context("building the fresh variant")?;
    check(&bytes, &map)?;
    let path = out.write(&[args.fresh_dir(), wanted.clone()], &bytes)?;
    println!(
        "{}: {} ({} bytes): index tail {}, header {}, retail salts",
        args.fresh_dir(),
        path.display(),
        layout.file_size,
        if args.keep_tail {
            "kept".to_string()
        } else {
            format!("cleared ({cleared} nonzero bytes)")
        },
        if args.keep_header {
            "kept"
        } else {
            "from scratch (low-detail fields kept)"
        },
    );
    println!(
        "load with: H2_MAP_DIR={}/<variant> h2 play {} --level {}",
        args.out.display(),
        disc.display(),
        args.map
    );
    Ok(())
}

/// The built map must read back with the same tags and a valid checksum.
fn check<R: std::io::Read + std::io::Seek>(bytes: &[u8], source: &Map<R>) -> Result<()> {
    let mut built = Map::from_reader(Cursor::new(bytes))?;
    built.validate_checksum()?;
    if built.tags().len() != source.tags().len()
        || built
            .tags()
            .iter()
            .zip(source.tags())
            .any(|(a, b)| (a.id, a.group, &a.name) != (b.id, b.group, &b.name))
    {
        bail!("built map's tags differ from the source");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Result<Option<Args>> {
        parse(&s.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
    }

    #[test]
    fn defaults_and_flags() {
        let a = args("testbuild --out /tmp/x").unwrap().unwrap();
        assert_eq!(
            a,
            Args {
                disc: None,
                out: "/tmp/x".into(),
                map: "lockout".into(),
                keep_tail: false,
                keep_header: false,
                force: false,
            }
        );
        assert_eq!(a.fresh_dir(), "fresh");
        let a = args("testbuild disc/halo2.iso --map 01a_tutorial.map --keep-header --out o --keep-tail --force")
            .unwrap()
            .unwrap();
        assert_eq!(a.disc, Some("disc/halo2.iso".into()));
        assert_eq!(a.map, "01a_tutorial");
        assert!(a.keep_tail && a.keep_header && a.force);
        assert_eq!(a.fresh_dir(), "fresh-keep-tail-keep-header");
        assert_eq!(
            args("testbuild --out o --keep-header")
                .unwrap()
                .unwrap()
                .fresh_dir(),
            "fresh-keep-header"
        );
    }

    #[test]
    fn bad_arguments_are_rejected() {
        assert!(args("testbuild").is_err()); // no --out
        assert!(args("other --out o").is_err());
        assert!(args("").is_err());
        assert!(args("testbuild --out").is_err());
        assert!(args("testbuild --out a --out b").is_err());
        assert!(args("testbuild --out a --map").is_err());
        assert!(args("testbuild --out a --map ../x").is_err());
        assert!(args("testbuild --out a --bogus").is_err());
        assert!(args("testbuild a.iso b.iso --out o").is_err());
        assert!(args("--help").unwrap().is_none());
        assert!(args("testbuild --help").unwrap().is_none());
    }
}
