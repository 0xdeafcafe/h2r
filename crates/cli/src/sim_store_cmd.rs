//! `h2 sim store`: builds the per-map stores a sim or light host can
//! serve instead of the disc (crates/vfs/src/sim_store.rs,
//! docs/runtime/sim.md "Sim stores").
//!
//! One recording covers the build: run the sim once with
//! `H2_SIM_STORE_RECORD=<file>` (each run merges into the file), then
//! `h2 sim store build <map|all> --disc ISO --out DIR --reads <file>`.

use std::{
    collections::BTreeMap,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use vfs::sim_store::{self, BuildError};

const USAGE: &str = "usage:
  h2 sim store build <map|all> --disc ISO --out DIR --reads FILE

Packs the recorded byte ranges of each named map — every range the guest
read of it through the VFS during a run with H2_SIM_STORE_RECORD=<FILE>
(runs merge, so different seeds add coverage) — into <DIR>/<map>.h2store,
verified byte for byte against the disc. A sim or light host with
H2_SIM_STORE=<DIR> then serves the map from the store, never the disc;
a read of unrecorded bytes is an error naming the range. `all` builds
every map the recording holds (docs/runtime/sim.md, \"Sim stores\").";

pub fn run(args: Vec<String>) -> Result<()> {
    let mut it = args.into_iter();
    match it.next().as_deref() {
        Some("build") => build(it.collect()),
        _ => bail!("{USAGE}"),
    }
}

fn build(args: Vec<String>) -> Result<()> {
    let mut which = None;
    let mut disc = None;
    let mut out = None;
    let mut reads_file = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--disc" => disc = Some(PathBuf::from(val()?)),
            "--out" => out = Some(PathBuf::from(val()?)),
            "--reads" => reads_file = Some(PathBuf::from(val()?)),
            "-h" | "--help" => bail!("{USAGE}"),
            _ if which.is_none() && !a.starts_with('-') => which = Some(a),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    let which = which.context(USAGE)?;
    let disc = disc.context(format!("--disc names the ISO\n\n{USAGE}"))?;
    let out = out.context(format!("--out names the store directory\n\n{USAGE}"))?;
    let reads_file = reads_file.context(format!("--reads names the recording\n\n{USAGE}"))?;
    let reads = read_recording(&reads_file)?;
    let maps = match which.as_str() {
        "all" => {
            let all = sim_store::recorded_maps(&reads);
            if all.is_empty() {
                bail!("{}: the recording holds no map reads", reads_file.display());
            }
            all
        }
        map => vec![map.to_ascii_lowercase()],
    };
    std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    let mut built = 0;
    for map in &maps {
        let ranges = sim_store::ranges_for(&reads, map);
        let path = out.join(format!("{map}.{}", sim_store::EXTENSION));
        match sim_store::build(&disc, map, &ranges, &path) {
            Ok(b) => {
                println!(
                    "sim store: {map}: {} range(s), {} MiB of {} MiB ({:.0}%) -> {}",
                    b.ranges,
                    b.store_len / (1 << 20),
                    b.logical_len / (1 << 20),
                    100.0 * b.store_len as f64 / b.logical_len as f64,
                    path.display()
                );
                built += 1;
            }
            Err(e @ (BuildError::NoReads(_) | BuildError::MapNotOnDisc(_))) => {
                // One missing map in `all` is reported, not fatal: the rest
                // still build. A named map must be there or the run fails.
                if maps.len() == 1 {
                    return Err(anyhow::anyhow!("{e}"));
                }
                eprintln!("sim store: {e}");
            }
            Err(e) => return Err(anyhow::anyhow!("{e}")),
        }
    }
    anyhow::ensure!(built > 0, "no store built");
    Ok(())
}

fn read_recording(path: &Path) -> Result<BTreeMap<String, Vec<(u64, u64)>>> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    sim_store::parse_reads(BufReader::new(file))
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
}
