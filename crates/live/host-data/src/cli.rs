//! `h2 host bundle`: the owner-side command that writes the two bundle
//! files from their own disc (contract 7). Wired in crates/apps/cli/src/main.rs.

use anyhow::{Result, bail};

use crate::{bundle, BundleSpec};

const USAGE: &str = "usage: h2 host bundle --disc <iso|dir> --out <dir>
                      [--stripped-maps <dir>] [--sim-stores <dir>] [--force] [--zstd-level 1..=21]

Writes <out>/host-data.tar.zst — the files a multiplayer host reads off
the DVD (default.xbe, mainmenu/shared/single_player_shared and every
multiplayer map, and the menu fonts) — and
<out>/host-data.json beside it, naming the archive's sha256 and every
file in it.

--disc is the owner's own disc: an XISO image or a directory holding the
disc's files. --stripped-maps replaces a map with the same-named one from that
directory when its header and size match the original. --sim-stores takes
shared.h2store and/or single_player_shared.h2store from the directory. Each
must cover the cache header and match the disc at every recorded byte.
Mainmenu and playable maps keep their regular files. The bundle is the
owner's own game data: upload it to a private bucket only, never commit
it or bake it into an image.";

/// Run `h2 host bundle`. `args` are the words after `bundle`.
pub fn bundle_cmd(args: Vec<String>) -> Result<()> {
    let spec = match parse_args(&args) {
        Ok(Some(spec)) => spec,
        Ok(None) => {
            println!("{USAGE}");
            return Ok(());
        }
        Err(why) => bail!("{why}\n{USAGE}"),
    };
    let out = bundle::build(&spec)?;
    println!(
        "host-data: {mp} multiplayer maps + {shared} shared ({stripped} from the stripped dir, \
         {stores} resource stores, {campaign} campaign and {unknown} other maps left out)",
        mp = out.multiplayer,
        shared = out.shared,
        stripped = out.stripped_used,
        stores = out.stores_used,
        campaign = out.campaign_skipped,
        unknown = out.unknown_skipped,
    );
    println!(
        "host-data: {raw} bytes -> {packed} bytes (zstd {level})",
        raw = out.raw_bytes,
        packed = out.archive_bytes,
        level = spec.zstd_level,
    );
    println!("sha256 {}", out.manifest.sha256);
    println!("wrote {}", spec.out.join(crate::ARCHIVE_NAME).display());
    println!("wrote {}", spec.out.join(crate::MANIFEST_NAME).display());
    Ok(())
}

/// `None` is a help request; `Err` is a usage error.
fn parse_args(args: &[String]) -> Result<Option<BundleSpec>, String> {
    use std::path::PathBuf;
    let (mut disc, mut out, mut stripped, mut force, mut level) = (None, None, None, false, 3);
    let mut sim_stores = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--disc" => {
                disc = Some(PathBuf::from(it.next().ok_or("--disc needs a path")?));
            }
            "--out" => {
                out = Some(PathBuf::from(it.next().ok_or("--out needs a directory")?));
            }
            "--stripped-maps" => {
                stripped = Some(PathBuf::from(it.next().ok_or("--stripped-maps needs a directory")?));
            }
            "--sim-stores" => {
                sim_stores = Some(PathBuf::from(it.next().ok_or("--sim-stores needs a directory")?));
            }
            "--force" => force = true,
            "--zstd-level" => {
                let raw = it.next().ok_or("--zstd-level needs a number")?;
                level = raw.parse::<i32>().map_err(|_| format!("--zstd-level {raw} is not a number"))?;
            }
            "-h" | "--help" => return Ok(None),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let Some(disc) = disc else { return Err("--disc is required".into()) };
    let Some(out) = out else { return Err("--out is required".into()) };
    if !(1..=21).contains(&level) {
        return Err(format!("--zstd-level {level} is outside 1..=21"));
    }
    Ok(Some(BundleSpec { disc, out, stripped, sim_stores, force, zstd_level: level }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_flags_fill_the_spec() {
        let spec = parse_args(&args(&[
            "--disc", "d.iso", "--out", "o", "--stripped-maps", "s", "--sim-stores", "t", "--force", "--zstd-level", "7",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(spec.disc, std::path::Path::new("d.iso"));
        assert_eq!(spec.out, std::path::Path::new("o"));
        assert_eq!(spec.stripped.as_deref(), Some(std::path::Path::new("s")));
        assert_eq!(spec.sim_stores.as_deref(), Some(std::path::Path::new("t")));
        assert!(spec.force);
        assert_eq!(spec.zstd_level, 7);
    }

    #[test]
    fn help_short_circuits_and_missing_flags_are_usage_errors() {
        assert!(parse_args(&args(&["--help"])).unwrap().is_none());
        for bad in [
            vec![],
            args(&["--out", "o"]),
            args(&["--disc", "d.iso"]),
            args(&["--disc"]),
            args(&["--disc", "d", "--out", "o", "--zstd-level", "0"]),
            args(&["--disc", "d", "--out", "o", "--wat"]),
        ] {
            assert!(parse_args(&bad).is_err(), "{bad:?}");
        }
    }
}
