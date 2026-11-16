//! Generate pathfinding for retail BSPs and compare with the retail data.
//!   compare [map ...]   (default: every map with pathfinding)
#[path = "common/disc.rs"]
mod disc;
use anyhow::Result;
use pathfind::compare::compare;
use pathfind::generate::{Options, generate};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let names = if args.is_empty() {
        disc::map_names()?
    } else {
        args
    };
    let opts = Options::default();
    println!(
        "map bsp | retail sectors/links/verts | gen sectors/links/verts | coverage excess | misplaced r/g | connect agree (both a-only b-only neither) | reach agree (both r-only g-only neither) | gen problems | ck"
    );
    for name in names {
        let mut map = disc::open_map(&name)?;
        let n = disc::scenario_bsps(&mut map).map(|b| b.len()).unwrap_or(0);
        for i in 0..n {
            let (bsp, retail, bytes, _) = disc::load_bsp(&mut map, i)?;
            let Some(retail) = retail else { continue };
            let u = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
            let instances = u(0x140) as usize;
            let g = generate(&bsp, instances, retail.checksum, &opts)?;
            let problems = g.validate(Some(bsp.surfaces.len()));
            let rp = retail.validate(Some(bsp.surfaces.len()));
            let m = compare(&bsp, &retail, &g);
            println!(
                "{name} {i} | {}/{}/{} | {}/{}/{} | {:.3} {:.3} | {}/{} of {} | {:.3} ({} {} {} {}) | {:.3} ({} {} {} {}) | {} (retail {}, orphans {}) | sbsp+8 {:08x} pf {:08x}",
                retail.sectors.len(),
                retail.links.len(),
                retail.vertices.len(),
                g.sectors.len(),
                g.links.len(),
                g.vertices.len(),
                m.coverage(),
                m.excess(),
                m.misplaced_a,
                m.misplaced_b,
                m.samples,
                m.connect_agreement(),
                m.connect_both,
                m.connect_a_only,
                m.connect_b_only,
                m.connect_neither,
                m.reach_agreement(),
                m.reach_both,
                m.reach_a_only,
                m.reach_b_only,
                m.reach_neither,
                problems.len(),
                rp.len(),
                retail.orphan_links(),
                u(8),
                retail.checksum
            );
            if std::env::var_os("KINDS").is_some() {
                let mut k: std::collections::BTreeMap<String, usize> = Default::default();
                for p in &rp {
                    *k.entry(p.split(": ").last().unwrap().to_string())
                        .or_default() += 1;
                }
                println!("  retail problem kinds {k:?}");
            }
            pathfind::build::element(&g).map_err(anyhow::Error::msg)?;
        }
    }
    Ok(())
}
