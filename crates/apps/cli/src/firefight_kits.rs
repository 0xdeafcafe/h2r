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

/// Builds the built-in kits that are missing or stale, in the foreground,
/// for a run that names a kit package as its map (`h2 sim run`). A kit that
/// cannot be built is an error naming it, not a map that is missing later.
pub fn ensure_now(disc: &Path, data: &Path) -> Result<()> {
    let maps = content::store::maps_dir(data);
    if builtin::pending(&maps).is_empty() {
        return Ok(());
    }
    eprintln!("firefight: preparing the kits");
    let reports = builtin::ensure(&Disc(disc.to_owned()), &maps);
    named_refusals(&reports)
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
}
