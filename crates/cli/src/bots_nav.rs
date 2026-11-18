//! `h2 bots nav`: build the bots' nav mesh and walls for disc maps, cached
//! under the data root (docs/game/bots.md, stage 2). Derived from the
//! user's disc, so the files stay on this machine. The game builds the
//! same files by itself (`bots::autonav`); this is the same worker, run in
//! the foreground.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use bots::autonav::{Region, Source, Worker};

const USAGE: &str = "usage: h2 bots nav <map-name-or-file|all> [--force] [--disc disc.iso] [--data DIR]\n       h2 bots test|verdict ... (`h2 bots test --help`)\n\nBuilds each structure BSP's nav mesh and line-of-sight walls from its\ncollision and writes <data>/bots/<map>.<bsp>.bots, which the bots load\nwhen the map does. `all` builds the disc's multiplayer maps whose file is\nmissing or stale (another generator version, or another map file); add\n--force to build them all. A named map is always built. The game does\nthis by itself when it runs (H2_BOTS_NAV_AUTO=0 turns that off).\n--data defaults to the data root `h2 play` uses.";

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("nav") => {}
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
            return Ok(());
        }
        Some(other) => bail!("unknown bots command {other}\n{USAGE}"),
    }
    let (mut positional, mut disc, mut data, mut force) = (Vec::new(), None, None, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--disc" => {
                disc = Some(PathBuf::from(
                    args.next().context("--disc needs an ISO path")?,
                ))
            }
            "--data" => {
                data = Some(PathBuf::from(
                    args.next().context("--data needs a directory")?,
                ))
            }
            "--force" => force = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            s if s.starts_with('-') => bail!("unknown option {s}\n{USAGE}"),
            _ => positional.push(a),
        }
    }
    let [map] = positional.as_slice() else {
        bail!("{USAGE}")
    };
    let out = data
        .unwrap_or_else(content::store::default_data_root)
        .join("bots");
    std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    let all = map == "all";
    let sources = if all {
        disc_maps(&disc.map(Ok).unwrap_or_else(super::default_disc)?)?
    } else {
        vec![one(map, disc)?]
    };
    let mut sources = Some(sources);
    let (worker, _) = Worker::new(out, Box::new(move || sources.take().unwrap_or_default()));
    let sum = worker
        .force(force || !all)
        .any_kind(!all)
        .log(|l| println!("{l}"))
        .run();
    if !sum.failed.is_empty() {
        bail!("{} maps failed", sum.failed.len());
    }
    Ok(())
}

/// The disc's `maps\*.map`, each a region of the image.
pub(super) fn disc_maps(disc: &Path) -> Result<Vec<Box<dyn Source>>> {
    let mut iso = super::open_iso(disc)?;
    let partition = iso.partition_offset();
    Ok(iso
        .entries()?
        .iter()
        .filter(|e| {
            let p = e.path.to_ascii_lowercase();
            !e.is_dir && p.starts_with("/maps/") && p.ends_with(".map") && p.matches('/').count() == 2
        })
        .map(|e| disc_region(disc, partition, e))
        .collect())
}

fn disc_region(disc: &Path, partition: u64, e: &xiso::Entry) -> Box<dyn Source> {
    let name = e.path.rsplit('/').next().unwrap_or(&e.path);
    let key = name[..name.len() - ".map".len()].to_owned();
    let disc = disc.to_path_buf();
    Box::new(Region::new(
        key,
        partition + u64::from(e.sector) * xiso::SECTOR,
        e.size.into(),
        Box::new(move || {
            Ok(Box::new(std::io::BufReader::new(std::fs::File::open(&disc)?)))
        }),
    ))
}

/// A map file, or a disc map by name.
fn one(name: &str, disc: Option<PathBuf>) -> Result<Box<dyn Source>> {
    if Path::new(name).is_file() {
        return Ok(Box::new(Region::file(PathBuf::from(name))));
    }
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
    Ok(disc_region(&disc, iso.partition_offset(), &entry))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against the user's disc (`H2_DISC` or `disc/`), read in place: every
    /// multiplayer map builds into a scratch directory, then a second pass
    /// finds them all up to date.
    #[test]
    #[ignore = "needs the user's disc"]
    fn every_multiplayer_map_builds_then_is_up_to_date() {
        let disc = super::super::default_disc().unwrap();
        let out = std::env::temp_dir().join(format!("cli-bots-nav-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        let pass = || {
            let mut s = Some(disc_maps(&disc).unwrap());
            let (w, _) = Worker::new(out.clone(), Box::new(move || s.take().unwrap_or_default()));
            w.log(|l| println!("{l}")).run()
        };
        let first = pass();
        assert!(first.failed.is_empty(), "{:?}", first.failed);
        assert!(first.built.len() >= 12, "{:?}", first.built);
        assert!(first.built.iter().any(|m| m == "lockout"));
        let again = pass();
        assert!(again.built.is_empty(), "{:?}", again.built);
        assert_eq!(again.fresh, first.built.len());
        std::fs::remove_dir_all(&out).unwrap();
    }
}
