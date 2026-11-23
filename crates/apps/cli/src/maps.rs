//! `h2 maps install`, which runs the game's own installer
//! (`boot::new_maps`), and the disc map readers `ui` shares with the
//! tools. The other map and tag commands are in `h2-tools`
//! (crates/apps/h2-tools/src/maps.rs, docs/runtime/cli-tools.md).
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

pub(crate) use cli_common::{disc_map, load_map};

const USAGE: &str = "usage: h2 maps install [--data DIR] [--disc disc.iso]";

/// `h2 maps install` writes content offers for installed map packages.
/// The boot reconstructs the same offers and mounts their map paths.
pub(super) fn install(args: &[String]) -> Result<()> {
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
        "{} package map(s) in the map list, {} removed",
        r.installed.len(),
        r.removed.len()
    );
    if r.overflow != 0 {
        println!("{} mods not shown, list full", r.overflow);
    }
    Ok(())
}
