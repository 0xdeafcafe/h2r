//! Opt-in: every retail scenario only asks the interpreter for casts and
//! builtins it has ([`hs::castable`], `Scenario::unrunnable`), so the model
//! of `0x0020a290` and its table matches what Bungie's compiler emitted.
//!
//! ```sh
//! H2_DISC=disc/halo2.iso cargo test -p hs --release --test casts_disc -- --ignored --nocapture
//! ```
use std::{fs::File, io::BufReader};

#[test]
#[ignore = "needs H2_DISC pointing at your own disc image"]
fn every_retail_scenario_is_runnable() {
    let path = std::env::var_os("H2_DISC").expect("set H2_DISC to your disc image");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let e = iso.find("default.xbe").unwrap();
    let xbe_bytes = iso.read(&e).unwrap();
    let xbe = xbe::Xbe::parse(&xbe_bytes).unwrap();
    let mut catalog = hs::Catalog::from_xbe(&xbe).unwrap();
    let mut scenarios = Vec::new();
    for e in iso
        .entries()
        .unwrap()
        .iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
    {
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let mut map = map::Map::from_region(
            BufReader::new(File::open(&path).unwrap()),
            base,
            e.size.into(),
        )
        .unwrap();
        let s = hs::Scenario::read(&mut map).unwrap();
        catalog.observe(&s).unwrap();
        scenarios.push((e.path.clone(), s));
    }
    let mut nodes = 0;
    for (path, s) in &scenarios {
        let bad = s.unrunnable(&catalog).unwrap();
        nodes += s.reachable(s.roots()).unwrap().len();
        assert!(bad.is_empty(), "{path}: {bad:#?}");
    }
    println!(
        "{} scenarios, {nodes} reached nodes: every cast and builtin runs",
        scenarios.len()
    );
}
