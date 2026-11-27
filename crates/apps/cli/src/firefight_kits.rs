//! The built-in Firefight kits build themselves when `h2 run`, `h2 serve`
//! and `h2 play` start (docs/game/firefight.md, "Built-in kits"). A worker
//! thread runs `firefight_kit::builtin::ensure` over the player's disc into
//! `<data>/maps`; while it runs the lobby says it is preparing
//! (`boot::lobby_start::kit_build_started`), and when it is done the game's
//! map list is read again and a Firefight lobby lands on Coagulation
//! (`kit_build_finished`). Nothing is built when every package is current,
//! and a disc that cannot be opened is a line on stderr, not a stop.
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use firefight_kit::builtin::{self, DiscMaps, Outcome};
use vfs::Backend;

/// The disc's `maps\<name>.map`, read whole.
struct Disc(PathBuf);

impl DiscMaps for Disc {
    fn read_map(&self, name: &str) -> Result<Vec<u8>> {
        let file = vfs::open_disc(&self.0)
            .with_context(|| format!("opening the disc {}", self.0.display()))?
            .host_file(&format!("maps\\{name}.map"))
            .with_context(|| format!("the disc has no maps\\{name}.map"))?;
        let mut reader = std::fs::File::open(&file.path)
            .with_context(|| format!("reading {}", file.path.display()))?;
        reader.seek(SeekFrom::Start(file.offset))?;
        let mut bytes = vec![0; usize::try_from(file.len)?];
        reader.read_exact(&mut bytes)?;
        Ok(bytes)
    }
}

/// How long the worker waits for the boot to prepare the map list and the
/// VFS before it gives up refreshing them (the boot reads the folder afresh
/// anyway).
const PREPARED_WAIT: Duration = Duration::from_secs(120);

/// Starts the worker when a built-in kit is missing or stale; returns at
/// once. Call before the game boots.
pub fn start(disc: &Path, data: &Path) {
    let maps = content::store::maps_dir(data);
    let pending = builtin::pending(&maps);
    if pending.is_empty() {
        return;
    }
    eprintln!("firefight: preparing the kits for {}", pending.join(", "));
    boot::lobby_start::kit_build_started();
    let disc = disc.to_owned();
    let spawned = std::thread::Builder::new()
        .name("firefight-kits".into())
        .spawn(move || work(&disc, &maps));
    if let Err(e) = spawned {
        eprintln!("firefight: the kit worker did not start ({e})");
        boot::lobby_start::kit_build_finished(vec![format!("the kit worker did not start: {e}")]);
    }
}

/// Stages the kit package `map` (`firefight-<base>`) for a run that names
/// it as its map (`h2 sim run`). A built-in kit (`builtin::MAPS`) that is
/// missing or stale is built here, in the foreground. Any other kit that is
/// not in `<data>/maps` is made by `h2 firefight kit <base>`, run as a child
/// of this same binary into `data`; if that fails, the run stops with its
/// exit status and the package named.
pub fn ensure_map(disc: &Path, data: &Path, map: &str) -> Result<()> {
    let exe = std::env::current_exe().context("finding h2's own path")?;
    ensure_map_with(&exe, disc, data, map)
}

fn ensure_map_with(exe: &Path, disc: &Path, data: &Path, map: &str) -> Result<()> {
    let Some(base) = map.strip_prefix("firefight-") else {
        return Ok(());
    };
    let maps = content::store::maps_dir(data);
    if builtin::MAPS.contains(&base) {
        if builtin::pending(&maps).is_empty() {
            return Ok(());
        }
        eprintln!("firefight: preparing the kits");
        let reports = builtin::ensure(&Disc(disc.to_owned()), &maps);
        // Only the run's own kit can stop the run; another built-in's
        // refusal is named and the run goes on.
        let (own, others): (Vec<_>, Vec<_>) = reports.into_iter().partition(|r| r.map == base);
        for line in others.iter().filter_map(|r| r.line()) {
            eprintln!("firefight: kit not built: {line}");
        }
        return named_refusals(&own);
    }
    let made = || maps.join(map).join("manifest.toml").is_file();
    if made() {
        return Ok(());
    }
    eprintln!("firefight: building the kit {map}");
    let status = std::process::Command::new(exe)
        .args(["firefight", "kit", base, "--disc"])
        .arg(disc)
        .arg("--data")
        .arg(data)
        .status()
        .with_context(|| format!("starting `h2 firefight kit {base}`"))?;
    anyhow::ensure!(status.success(), "the kit {map} did not build: `h2 firefight kit {base}` {status}");
    anyhow::ensure!(made(), "the kit {map} did not build: `h2 firefight kit {base}` left no package in {}", maps.display());
    Ok(())
}

/// The refusals among `reports`, each named, as one error.
fn named_refusals(reports: &[builtin::Report]) -> Result<()> {
    let lines: Vec<String> = reports.iter().filter_map(|r| r.line()).collect();
    anyhow::ensure!(lines.is_empty(), "the Firefight kits did not stage: {}", lines.join("; "));
    Ok(())
}

fn work(disc: &Path, maps: &Path) {
    let reports = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        builtin::ensure(&Disc(disc.to_owned()), maps)
    }));
    let failures = match reports {
        Ok(reports) => {
            for r in &reports {
                match &r.outcome {
                    Outcome::Current => {}
                    Outcome::Built { id, fitted } => {
                        eprintln!("firefight: built the {} kit ({id}, {} fit steps)", r.map, fitted.len());
                    }
                    Outcome::Refused(why) => eprintln!("firefight: the {} kit: {why}", r.map),
                }
            }
            reports.iter().filter_map(|r| r.line()).collect()
        }
        Err(_) => vec!["the kit build stopped".to_owned()],
    };
    // The game lists what is on disk when it boots; one already running
    // reads the folder again.
    let waited = Instant::now();
    while !boot::map_select::publish_ready() && waited.elapsed() < PREPARED_WAIT {
        std::thread::sleep(Duration::from_millis(200));
    }
    boot::map_select::publish_packages(disc);
    boot::lobby_start::kit_build_finished(failures);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_kit_stops_the_run_with_its_name() {
        let ok = builtin::Report { map: "coagulation".into(), outcome: Outcome::Current };
        let bad = builtin::Report { map: "zanzibar".into(), outcome: Outcome::Refused("6 KB over".into()) };
        assert!(named_refusals(std::slice::from_ref(&ok)).is_ok());
        let e = named_refusals(&[ok, bad]).unwrap_err().to_string();
        assert_eq!(e, "the Firefight kits did not stage: Zanzibar: 6 KB over");
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_kit_map_is_built_by_a_child_and_a_failure_is_named() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("h2-kit-child-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let script = |name: &str, body: &str| {
            let p = root.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        // Args: firefight kit BASE --disc DISC --data DATA
        let good = script("good", "mkdir -p \"$7/maps/firefight-$3\" && : > \"$7/maps/firefight-$3/manifest.toml\"");
        let bad = script("bad", "exit 3");
        let lazy = script("lazy", "exit 0");
        let disc = root.join("disc");
        let data = root.join("data");
        // Maps that are not built-in kits (`builtin::MAPS`), which are
        // staged from the disc instead of by a child.
        assert!(!builtin::MAPS.contains(&"elongation") && !builtin::MAPS.contains(&"turf"));
        assert!(ensure_map_with(&good, &disc, &data, "firefight-elongation").is_ok());
        assert!(data.join("maps/firefight-elongation/manifest.toml").is_file());
        // Present already: the child is not run.
        assert!(ensure_map_with(&bad, &disc, &data, "firefight-elongation").is_ok());
        let other = root.join("other");
        let e = ensure_map_with(&bad, &disc, &other, "firefight-turf").unwrap_err().to_string();
        assert!(e.contains("firefight-turf") && e.contains("did not build") && e.contains("3"), "{e}");
        let e = ensure_map_with(&lazy, &disc, &other, "firefight-turf").unwrap_err().to_string();
        assert!(e.contains("left no package"), "{e}");
        assert!(ensure_map_with(&bad, &disc, &other, "turf").is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }
}
