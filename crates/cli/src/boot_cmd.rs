//! `h2 build` and `h2 run`: the boot tracer (crates/boot, docs/BOOT.md).

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

const USAGE: &str = "usage:
  h2 build <disc.iso|default.xbe> [--out DIR] [--jobs N]
  h2 run   <disc.iso> [--out DIR] [--data DIR] [--slices N] [--transfers N] [--no-stopgaps] [--no-d3d] [--no-bink] [--no-dsound] [--no-xapi]

--out defaults to $TMPDIR/h2boot (generated C, objects, libh2game.dylib).
--data defaults to $TMPDIR/h2boot-data (host root of the hard-disk partitions).
`run` builds first when the dylib is missing.";

pub fn run(cmd: &str, args: Vec<String>) -> Result<()> {
    let tmp = std::env::temp_dir();
    let mut out = tmp.join("h2boot");
    let mut data = tmp.join("h2boot-data");
    let mut jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut slices = 100_000u64;
    let mut transfers = 50_000_000u64;
    let mut disc = None;
    let mut stopgaps = true;
    let mut d3d = true;
    let mut bink = true;
    let mut dsound = true;
    let mut xapi = true;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--out" => out = PathBuf::from(val()?),
            "--data" => data = PathBuf::from(val()?),
            "--jobs" => jobs = val()?.parse()?,
            "--no-stopgaps" => stopgaps = false,
            "--no-d3d" => d3d = false,
            "--no-bink" => bink = false,
            "--no-dsound" => dsound = false,
            "--no-xapi" => xapi = false,
            "--slices" => slices = val()?.parse()?,
            "--transfers" => transfers = val()?.parse()?,
            "-h" | "--help" => bail!("{USAGE}"),
            _ if disc.is_none() => disc = Some(PathBuf::from(a)),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    let disc = disc.or_else(|| std::env::var_os("H2_DISC").map(PathBuf::from)).context(USAGE)?;
    let xbe = boot::read_xbe(&disc)?;
    let lib = out.join(boot::DYLIB);
    if cmd == "build" || !lib.exists() {
        boot::build(&xbe, &out, jobs)?;
    }
    if cmd == "build" {
        return Ok(());
    }
    let r = boot::run(
        &xbe,
        &lib,
        &boot::RunOpts { disc, data_root: data, max_slices: slices, transfers_per_slice: transfers, stopgaps, trace: Some(out.join("kernel-trace.tsv")), d3d, bink, dsound, xapi },
    )?;
    for l in &r.kernel_log {
        println!("DbgPrint: {}", l.trim_end());
    }
    println!("outcome: {}", r.outcome.chars().take(400).collect::<String>());
    println!("stop: {}", r.stop);
    println!("last eip: {:#010x}", r.eip);
    Ok(())
}
