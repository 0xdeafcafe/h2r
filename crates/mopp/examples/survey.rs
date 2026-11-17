//! Retail MOPP survey: header fields, opcode histogram, emitted key range.
#[path = "../tests/common/mod.rs"]
mod common;
fn main() -> anyhow::Result<()> {
    let mut total = [0u64; 256];
    for l in common::all_levels()? {
        let code = mopp::Code::parse(&l.mopp)?;
        let r = mopp::check::validate(&code.bytecode).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        for (i, h) in r.histogram.iter().enumerate() {
            total[i] += u64::from(*h);
        }
        let types: std::collections::BTreeSet<u32> = r.keys.iter().map(|k| k >> 29).collect();
        let max_low = r.keys.iter().map(|k| k & 0x1fff_ffff).max();
        println!(
            "{} {} size={} keys={} surfaces={} types={types:?} maxlow={max_low:?} unreached={} depth={} count={} scale={} info={:#x}",
            l.name,
            l.bsp_index,
            code.bytecode.len(),
            r.keys.len(),
            l.collision.surfaces.len(),
            r.unreached,
            r.max_depth,
            l.mopp_count,
            code.scale,
            code.info
        );
    }
    for (i, t) in total.iter().enumerate() {
        if *t > 0 {
            println!("op {i:02x}: {t}");
        }
    }
    Ok(())
}
