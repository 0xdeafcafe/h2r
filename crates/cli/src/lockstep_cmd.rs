//! `h2 lockstep A B`: compare two determinism traces (`H2_LOCKSTEP`,
//! crates/lockstep, docs/game/high-framerate.md).

use anyhow::{Context, Result, bail};

const USAGE: &str = "usage: h2 lockstep <trace-a> <trace-b>

Compares two traces written by H2_LOCKSTEP=FILE runs (guest-RAM hashes at
every game tick and present). Prints the first record that differs, with the
logical clock, the game tick and the physical RAM megabytes that changed.
Exits 0 when the traces are identical, 1 when they differ.";

/// The process exit code.
pub fn run(args: Vec<String>) -> Result<i32> {
    let [a, b] = args.as_slice() else {
        bail!("{USAGE}")
    };
    let read = |p: &str| -> Result<lockstep::Trace> {
        let text = std::fs::read_to_string(p).with_context(|| format!("reading {p}"))?;
        lockstep::Trace::parse(&text).map_err(|e| anyhow::anyhow!("{p}: {e}"))
    };
    let (ta, tb) = (read(a)?, read(b)?);
    let verdict = lockstep::compare(&ta, &tb);
    println!("{a}: {} ticks, {} records", ta.ticks(), ta.records.len());
    println!("{b}: {} ticks, {} records", tb.ticks(), tb.records.len());
    println!("{verdict}");
    Ok(if verdict.identical() { 0 } else { 1 })
}
