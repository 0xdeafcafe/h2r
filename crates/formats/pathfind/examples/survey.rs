//! Per BSP: the pathfinding element (sbsp +0xc4) words and block counts.
#[path = "common/disc.rs"]
mod disc;
use anyhow::Result;

fn main() -> Result<()> {
    for name in disc::map_names()? {
        let Ok(mut map) = disc::open_map(&name) else {
            continue;
        };
        let Ok(bsps) = disc::scenario_bsps(&mut map) else {
            continue;
        };
        for (i, id) in bsps.into_iter().enumerate() {
            let (b, base) = disc::bsp_payload(&mut map, id)?;
            let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
            let (n, a) = (u(0xc4), u(0xc8));
            let (cn, ca) = (u(0x14), u(0x18));
            print!("{name:24} {i} pf={n} coll={cn}");
            if cn == 1 {
                let c = (ca - base) as usize;
                print!(" surf={}", u(c + 0x28));
            }
            if n >= 1 {
                let e = (a - base) as usize;
                let w: Vec<String> = (0..0x74 / 4)
                    .map(|k| format!("{:x}", u(e + 4 * k)))
                    .collect();
                print!(" | {}", w.join(" "));
            }
            println!();
        }
    }
    Ok(())
}
