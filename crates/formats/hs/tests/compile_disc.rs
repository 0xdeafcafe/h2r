//! Opt-in: compile each retail script and global from its decompiled
//! source, alone, and count those whose nodes equal the map's
//! ([`hs::first_difference`]). This is the evidence that the compiler's
//! type rules match what Bungie's compiler emitted
//! (docs/formats/haloscript.md, "Compiling source"). It reports a share
//! and the first few differences; it does not require every one.
//!
//! ```sh
//! H2_DISC=disc/halo2.iso cargo test -p hs --release --test compile_disc -- --ignored --nocapture
//! ```
use std::{fs::File, io::BufReader};

#[test]
#[ignore = "needs H2_DISC pointing at your own disc image"]
fn retail_scripts_round_trip_through_the_compiler() {
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
    let (mut total, mut same, mut refused, mut differ) = (0usize, 0usize, 0usize, 0usize);
    let mut shown = 0;
    for (path, s) in &scenarios {
        for item in s.decompile_each(&catalog).unwrap() {
            total += 1;
            let old = match item.item {
                hs::Item::Global(i) => s.globals()[i].root(),
                hs::Item::Script(i) => s.scripts()[i].root(),
            };
            let report = |what: String, shown: &mut usize| {
                if *shown < 40 {
                    println!("{path} {}: {what}", item.name);
                    *shown += 1;
                }
            };
            match hs::compile(s, &catalog, &item.source, &hs::Options::default()) {
                Err(messages) => {
                    refused += 1;
                    report(format!("refused: {}", messages[0]), &mut shown);
                }
                Ok(out) => {
                    let new = match item.item {
                        hs::Item::Global(i) => out.scenario.globals()[i].root(),
                        hs::Item::Script(i) => out.scenario.scripts()[i].root(),
                    };
                    match hs::first_difference(s, old, &out.scenario, new).unwrap() {
                        None => same += 1,
                        Some(d) => {
                            differ += 1;
                            report(d, &mut shown);
                        }
                    }
                }
            }
        }
    }
    println!(
        "{} scenarios, {total} definitions: {same} round-trip ({:.1}%), {differ} differ, {refused} refused",
        scenarios.len(),
        100.0 * same as f64 / total.max(1) as f64
    );
}
