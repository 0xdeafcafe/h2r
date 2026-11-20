//! `H2_XBE=/path/default.xbe cargo run -p analyze --example dispatch_coverage`
//! Output is TSV with addresses/status only, never executable bytes.
use analyze::dispatch::{CAMPAIGN_ROOTS, catalog, coverage, entries};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data = std::fs::read(std::env::var_os("H2_XBE").ok_or("set H2_XBE")?)?;
    let xbe = xbe::Xbe::parse(&data)?;
    let seeded = !std::env::args().any(|arg| arg == "--unseeded");
    let a = analyze::analyze_with_pointer_seeds(&xbe, seeded)?;
    let tables = catalog(&xbe, &a)?;
    if !tables.iter().any(|t| t.name == "haloscript-evaluate") {
        return Err("retail campaign table layout did not validate".into());
    }
    let exported = entries(&xbe, &a);
    println!(
        "census\tfunctions\t{}\tlabels\t{}\tentries\t{}\tconflicts\t{}\trounds\t{}\tconverged\t{}",
        a.functions.len(),
        a.labels.len(),
        exported.len(),
        a.conflicts.len(),
        a.rounds,
        a.converged
    );
    for &entry in &exported {
        if let Some(function) = a.functions.get(&entry) {
            println!(
                "entry\t{entry:08x}\tfunction\t{:?}\treturns={}",
                function.source, function.returns
            );
        } else {
            println!("entry\t{entry:08x}\tinterior-entry");
        }
    }
    let mut selected = std::collections::BTreeSet::new();
    let mut switches = std::collections::BTreeSet::new();
    for root in CAMPAIGN_ROOTS {
        let c = coverage(&xbe, &a, &tables, &[root]);
        println!(
            "root\t{root:08x}\tfunctions\t{}\tsites\t{}\ttables\t{}\tunassociated-sites\t{}\tcfg-errors\t{}",
            c.functions.len(),
            c.sites.len(),
            c.tables.len(),
            c.sites
                .values()
                .filter(|s| matches!(s.kind, "call" | "jump") && s.candidate_tables.is_empty())
                .count(),
            c.cfg_errors.len()
        );
        for (pc, site) in c.sites {
            if a.jump_tables.contains_key(&pc) {
                switches.insert(pc);
            }
            let candidates = site
                .candidate_tables
                .iter()
                .map(|&i| format!("{:08x}", tables[i].base))
                .collect::<Vec<_>>()
                .join(",");
            println!(
                "site\t{root:08x}\t{pc:08x}\t{}\t{:08x}\t{candidates}",
                site.kind, site.owner
            );
        }
        for missing in c.missing_functions {
            println!("unreached\t{root:08x}\t{missing:08x}");
        }
        for (owner, error) in c.cfg_errors {
            println!("cfg-error\t{root:08x}\t{owner:08x}\t{error}");
        }
        selected.extend(c.tables);
    }
    for (i, t) in tables.iter().enumerate() {
        let table_kind = if selected.contains(&i) {
            "table"
        } else {
            "catalog-table"
        };
        let target_kind = if selected.contains(&i) {
            "target"
        } else {
            "catalog-target"
        };
        let targets: std::collections::BTreeSet<_> = t
            .slots
            .iter()
            .map(|&(_, target)| target)
            .filter(|&target| target != 0)
            .collect();
        let misses: Vec<_> = targets
            .iter()
            .filter(|target| !exported.contains(target))
            .map(|t| format!("{t:08x}"))
            .collect();
        let not_functions = targets
            .iter()
            .filter(|target| !a.functions.contains_key(target))
            .count();
        println!(
            "{table_kind}\t{:08x}\t{}\tknown-layout={}\tslots={}\ttargets={}\tnot-functions={}\tmissing={}\t{}",
            t.base,
            t.name,
            t.known_layout,
            t.slots.len(),
            targets.len(),
            not_functions,
            misses.len(),
            misses.join(",")
        );
        for &(slot, target) in &t.slots {
            let status = if target == 0 {
                "null"
            } else if a.functions.contains_key(&target) {
                "function"
            } else if exported.contains(&target) {
                "interior-entry"
            } else {
                "MISSING"
            };
            println!(
                "{target_kind}\t{:08x}\t{slot:08x}\t{target:08x}\t{status}",
                t.base
            );
        }
    }
    for pc in switches {
        let t = &a.jump_tables[&pc];
        println!(
            "switch-table\t{:08x}\t{pc:08x}\t{:?}\tcount={}\tholes={:?}",
            t.table, t.kind, t.count, t.holes
        );
        for (index, target) in t.cases() {
            let status = if a.functions.contains_key(&target) {
                "function"
            } else if exported.contains(&target) {
                "interior-entry"
            } else if a.claim(target).is_some_and(|(state, _)| state == 'I') {
                "local-instruction"
            } else {
                "MISSING"
            };
            println!(
                "switch-target\t{:08x}\t{pc:08x}\t{index}\t{target:08x}\t{status}",
                t.table
            );
        }
    }
    Ok(())
}
