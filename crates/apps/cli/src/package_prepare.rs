//! `h2 package prepare`, and the same step before `h2 play`, `h2 run` and
//! `h2 serve` boot (docs/editor/studio-redesign.md, "How the game consumes
//! packages", Prepare; "Conversions"). The game's set (baked, then
//! `<data>/packages/`, under `set.toml`) is read as prepare sees it, and
//! its `convert` items run with the converters `h2` has: the Map Pack
//! install ([`MapPack`]) and the Halo 1 import ([`Halo1`]). What each
//! converted is stamped under `<data>/packages/stamps/`
//! (`workspace::convert`). A package whose converter finds no disc and has
//! converted nothing before is off at boot, with the reason.
//!
//! `map` and `map-patch` items build at boot (`boot::packages::prepare_maps`).
//! The other `h2 package` commands run in `h2-tools`; this one runs the
//! converters, which link the game's disc readers, so it stays in `h2`
//! (`tools::forwards`).
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use workspace::{Converted, Converter, GameSet};

use crate::h1_import::package::Halo1;
use crate::mappack_cmd::package::MapPack;

const USAGE: &str = "usage: h2 package prepare [--disc <halo2.iso>] [--data DIR] [--force] [--no-halo1] [--no-mappack]

Runs the convert items of the game's packages: the Halo 1 maps from the
Halo 1 disc beside the game disc, the Map Pack from the Map Pack disc beside
it. Each map converts once; a map whose disc or converter changed, or whose
output is gone, converts again, and a run that stopped carries on where it
was. --force converts everything again. h2 play, run and serve do this
before they boot.";

/// Which converters run, and whether the stamps are forgotten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opts {
    pub force: bool,
    pub halo1: bool,
    pub map_pack: bool,
}

/// Run the set's `convert` items for the game disc `disc` into `data`.
pub fn prepare(disc: &Path, data: &Path, o: Opts, say: &mut dyn FnMut(String)) -> Converted {
    let (baked, problems) = boot::packages::baked();
    for p in problems {
        say(p);
    }
    let set = GameSet::read_unprepared(baked, data, &|_| true);
    let halo1 = Halo1 {
        game_disc: disc.to_owned(),
        data: data.to_owned(),
        force: o.force,
    };
    let map_pack = MapPack {
        game_disc: disc.to_owned(),
        data: data.to_owned(),
    };
    let mut converters: Vec<&dyn Converter> = Vec::new();
    if o.map_pack {
        converters.push(&map_pack);
    }
    if o.halo1 {
        converters.push(&halo1);
    }
    workspace::convert(data, &set.loaded(), &converters, o.force, say)
}

/// The lines a run ends with. `offs`: name the packages left off for want
/// of a disc (the boot leaves those to `packages.list`).
pub fn summary(out: &Converted, offs: bool) -> Vec<String> {
    let mut lines = Vec::new();
    if offs {
        for o in &out.off {
            lines.push(format!("{} is off: {}", o.package, o.why));
        }
    }
    for f in &out.failed {
        lines.push(match &f.unit {
            Some(u) => format!("{} item {} {u}: {}", f.package, f.item, f.why),
            None => format!("{} item {}: {}", f.package, f.item, f.why),
        });
    }
    for s in &out.skipped {
        lines.push(format!(
            "{} item {}: the {} converter is switched off for this run",
            s.package, s.item, s.converter
        ));
    }
    if !out.ran.is_empty() || out.held > 0 || out.not_retried > 0 {
        let mut line = format!("{} converted, {} up to date", out.ran.len(), out.held);
        if out.not_retried > 0 {
            line += &format!(
                ", {} failed before and not tried again (--force tries)",
                out.not_retried
            );
        }
        lines.push(line);
    }
    lines
}

/// Before the game boots. Never fails: a problem is a line, and the game
/// starts with what is there.
pub fn before_boot(disc: &Path, data: &Path, no_halo1: bool, no_mappack: bool) {
    let o = Opts {
        force: false,
        halo1: crate::h1_import::auto_enabled(no_halo1),
        map_pack: crate::mappack_cmd::auto_enabled(no_mappack),
    };
    let r = catch_unwind(AssertUnwindSafe(|| {
        prepare(disc, data, o, &mut |s| eprintln!("packages: {s}"))
    }));
    match r {
        Ok(out) => {
            if out.ran.is_empty() && out.failed.is_empty() {
                return;
            }
            for line in summary(&out, false) {
                eprintln!("packages: {line}");
            }
        }
        Err(_) => eprintln!("packages: prepare stopped; the game starts with what is there"),
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    disc: Option<PathBuf>,
    data: Option<PathBuf>,
    force: bool,
    no_halo1: bool,
    no_mappack: bool,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut out = Args::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |what: &str| {
            it.next()
                .filter(|v| !v.starts_with("--"))
                .map(PathBuf::from)
                .ok_or_else(|| format!("{what} needs a path"))
        };
        match a.as_str() {
            "--disc" => out.disc = Some(value("--disc")?),
            "--data" => out.data = Some(value("--data")?),
            "--force" => out.force = true,
            "--no-halo1" => out.no_halo1 = true,
            "--no-mappack" => out.no_mappack = true,
            other => return Err(format!("unexpected argument {other}")),
        }
    }
    Ok(out)
}

/// `h2 package prepare ...` (the words after `prepare`).
pub fn run(args: Vec<String>) -> Result<()> {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    let a = parse(&args).map_err(|e| anyhow::anyhow!("{e}\n\n{USAGE}"))?;
    let disc = a
        .disc
        .or_else(|| std::env::var_os("H2_DISC").map(PathBuf::from))
        .unwrap_or_else(|| crate::repo_root().join("disc").join("halo2.iso"));
    let data = a.data.unwrap_or_else(content::store::default_data_root);
    let o = Opts {
        force: a.force,
        halo1: crate::h1_import::auto_enabled(a.no_halo1),
        map_pack: crate::mappack_cmd::auto_enabled(a.no_mappack),
    };
    let out = prepare(&disc, &data, o, &mut |s| println!("{s}"));
    let lines = summary(&out, true);
    if lines.is_empty() {
        println!("no package has a convert item to run");
    }
    for line in &lines {
        println!("{line}");
    }
    if !out.failed.is_empty() {
        bail!("{} conversions failed", out.failed.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use workspace::{ConvertFailed, InputOff, Ran};

    fn words(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_flags_read() {
        let a = parse(&words(&["--disc", "x.iso", "--force", "--no-halo1"])).unwrap();
        assert_eq!(a.disc, Some(PathBuf::from("x.iso")));
        assert!(a.force && a.no_halo1 && !a.no_mappack);
        assert!(parse(&words(&["--data"])).unwrap_err().contains("--data"));
        assert!(parse(&words(&["--what"])).unwrap_err().contains("--what"));
    }

    #[test]
    fn the_summary_names_what_is_off_only_when_asked() {
        let out = Converted {
            ran: vec![Ran {
                package: "halo1".into(),
                item: 0,
                unit: "a10".into(),
            }],
            held: 2,
            off: vec![InputOff {
                package: "map-pack".into(),
                item: 0,
                why: "no Map Pack disc".into(),
            }],
            failed: vec![ConvertFailed {
                package: "halo1".into(),
                item: 0,
                unit: Some("b30".into()),
                why: "invented".into(),
            }],
            ..Converted::default()
        };
        let with = summary(&out, true);
        assert_eq!(with[0], "map-pack is off: no Map Pack disc");
        assert_eq!(with[1], "halo1 item 0 b30: invented");
        assert_eq!(with[2], "1 converted, 2 up to date");
        assert_eq!(summary(&out, false).len(), 2);
    }
}
