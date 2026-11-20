//! Every write the game tick can make, by where it lands
//! (`emit::sim::census`), and how much of the tick the sim tier compiles.
//! Needs the user's own XBE; prints markdown for
//! `docs/engine/sim-tier-writes.md` and writes it under Cargo's test
//! scratch directory.
//!
//! `H2_XBE=<path>/default.xbe cargo test -p emit --release --test sim_tier_writes -- --ignored --nocapture`

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use emit::sim::census::{self, Region};
use emit::sim::{Closure, Refusal, Tier};

/// `game_tick`: the root of everything a simulation tick runs.
const GAME_TICK: u32 = 0x0013_7fe0;
/// Upper bound on the closure walk; the tick reaches a few thousand.
const LIMIT: usize = 40_000;

#[test]
#[ignore = "needs the XBE; prints the census"]
fn tick_writes_census() {
    let Some(path) = common::xbe_path() else {
        common::skip("tick_writes_census", "needs the XBE (H2_XBE)");
        return;
    };
    let data = std::fs::read(path).unwrap();
    let x = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&x).unwrap();
    let lifter = lift::Lifter::new(&analysis, &x).unwrap();
    let regions: Vec<Region> = x
        .sections
        .iter()
        .map(|s| Region { name: s.name.clone(), start: s.va, end: s.va.wrapping_add(s.vsize) })
        .collect();

    let tick = Closure::build(&[GAME_TICK], LIMIT, |pc| lifter.lift(pc));
    let mut out = census::markdown("Game tick closure", &census::census(&tick), &regions);

    // How much of the tick the tier takes, and why the rest stays behind.
    let tier = Tier::build(&tick);
    let mut why: BTreeMap<String, usize> = BTreeMap::new();
    for r in tier.refused.values() {
        let key = match r {
            Refusal::Unsupported { what, .. } => what.clone(),
            Refusal::X87Depth { .. } => "x87 depth not static".into(),
        };
        *why.entry(key).or_default() += 1;
    }
    out.push_str(&format!(
        "\n## Sim tier coverage of the tick\n\n{} of {} functions compile; {} refused.\n\n| reason | functions |\n|---|---|\n",
        tier.signatures.len(),
        tick.functions.len(),
        tier.refused.len()
    ));
    for (k, n) in &why {
        out.push_str(&format!("| {k} | {n} |\n"));
    }

    // The whole image, for scale: everything analysis found.
    let all: Vec<u32> = analysis.functions.keys().copied().collect();
    let image = Closure::build(&all, usize::MAX, |pc| lifter.lift(pc));
    let image_census = census::census(&image);
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for s in &image_census.sites {
        let k = match s.kind {
            census::Kind::Absolute(_) => "absolute",
            census::Kind::Indexed(_) => "indexed global",
            census::Kind::Field(..) => "field of a root",
            census::Kind::Reachable(_) => "reachable from a root",
            census::Kind::Stack => "stack",
            census::Kind::Fs(_) => "fs",
            census::Kind::Unknown => "unknown",
        };
        *kinds.entry(k).or_default() += 1;
    }
    out.push_str(&format!(
        "\n## Whole image, for scale\n\n{} functions, {} stores.\n\n| class | stores |\n|---|---|\n",
        image_census.functions,
        image_census.sites.len()
    ));
    for (k, n) in &kinds {
        out.push_str(&format!("| {k} | {n} |\n"));
    }

    let file = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sim-tier-writes.md");
    std::fs::write(&file, &out).unwrap();
    println!("{out}");
    eprintln!("written to {}", file.display());
    assert!(tick.functions.contains_key(&GAME_TICK), "the tick itself lifts");
}
