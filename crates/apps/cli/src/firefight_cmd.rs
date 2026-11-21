//! `h2 firefight kit`: build a Firefight package from the user's disc.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use boot::new_maps::{Disc, IsoDisc};
use firefight_kit::Mode;
use scenario_edit::{MapSource, recipe};

const USAGE: &str =
    "usage: h2 firefight kit MAP [--disc ISO_OR_EXTRACTED_DISC] [--data DIR] [--probe] [--replace]
  builds <data>/maps/firefight-MAP from a multiplayer map on the disc.
  --probe builds the firefight-probe test map with one squad of two Elites.
  --replace updates an existing package of the same name.";

struct Args {
    base: String,
    disc: Option<PathBuf>,
    data: Option<PathBuf>,
    probe: bool,
    replace: bool,
}

fn parse(words: Vec<String>) -> Result<Option<Args>> {
    let mut it = words.into_iter();
    match it.next().as_deref() {
        Some("kit") => {}
        Some("-h" | "--help") => return Ok(None),
        _ => bail!("{USAGE}"),
    }
    let mut base = None;
    let mut disc = None;
    let mut data = None;
    let mut probe = false;
    let mut replace = false;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--probe" => probe = true,
            "--replace" => replace = true,
            "--disc" => disc = Some(PathBuf::from(it.next().context("--disc requires a path")?)),
            "--data" => {
                data = Some(PathBuf::from(
                    it.next().context("--data requires a directory")?,
                ))
            }
            name if base.is_none() && !name.starts_with('-') => {
                let name = name.to_ascii_lowercase();
                let name = name.strip_suffix(".map").unwrap_or(&name);
                if !content::manifest::is_map_name(name) {
                    bail!("expected a disc map name such as coagulation, got {arg:?}");
                }
                base = Some(name.to_owned());
            }
            _ => bail!("unexpected argument {arg:?}\n{USAGE}"),
        }
    }
    Ok(Some(Args {
        base: base.context(USAGE)?,
        disc,
        data,
        probe,
        replace,
    }))
}

pub fn run(words: Vec<String>) -> Result<()> {
    let Some(args) = parse(words)? else {
        println!("{USAGE}");
        return Ok(());
    };
    let (mode, dir) = if args.probe {
        let recipe = recipe::built_in_named("firefight-probe")?.on_base(&args.base)?;
        (Mode::Probe, recipe.package_dir())
    } else {
        (Mode::Kit, format!("firefight-{}", args.base))
    };
    let disc = args.disc.map(Ok).unwrap_or_else(crate::default_disc)?;
    let file = IsoDisc::new(&disc)
        .map_file(&args.base)?
        .with_context(|| format!("{} has no maps/{}.map", disc.display(), args.base))?;
    let bytes = MapSource::region(file.path, Some((file.offset, file.len))).read_base()?;
    let data = args.data.unwrap_or_else(content::store::default_data_root);
    let maps = content::store::maps_dir(&data);
    let saved = firefight_kit::build_package(&bytes, &args.base, mode, &maps, &dir, args.replace)?;
    println!("{}", maps.join(dir).display());
    println!("MapId {}", saved.id);
    println!("{} drop zones, {} squads", saved.zones, saved.squads);
    println!("{}", saved.costs);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &str) -> Result<Option<Args>> {
        parse(words.split_whitespace().map(str::to_owned).collect())
    }

    #[test]
    fn the_probe_builds_on_a_named_disc_map() {
        let a = args("kit Lockout.map --probe --disc disc.iso --data own-data --replace")
            .unwrap()
            .unwrap();
        assert_eq!(a.base, "lockout");
        assert!(a.probe && a.replace);
        assert_eq!(a.disc.unwrap(), PathBuf::from("disc.iso"));
        assert_eq!(a.data.unwrap(), PathBuf::from("own-data"));
        assert!(!args("kit lockout").unwrap().unwrap().probe);
        for bad in ["kit", "kit ../some-file", "kit one two", "kit one --disc"] {
            assert!(args(bad).is_err(), "{bad}");
        }
    }
}
