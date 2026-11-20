//! Opcode histogram of retail versus our program for one level. Args: map bsp
#[path = "../tests/common/mod.rs"]
mod common;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut map = common::open_map(&a[0])?;
    let idx: usize = a[1].parse()?;
    let id = common::scenario_bsps(&mut map)?[idx];
    let l = common::load_level(&mut map, &a[0], idx, id)?;
    let retail = mopp::Code::parse(&l.mopp)?;
    let ours = mopp::Code::parse(&mopp::build_structure(&l.collision))?;
    let r = mopp::check::validate(&retail.bytecode).unwrap();
    let o = mopp::check::validate(&ours.bytecode).unwrap();
    println!(
        "surfaces={} retail bytes={} keys={} ours bytes={} keys={}",
        l.collision.surfaces.len(),
        retail.bytecode.len(),
        r.keys.len(),
        ours.bytecode.len(),
        o.keys.len()
    );
    let len = |op: usize| match op {
        0x00 | 0x30..=0x4f => 1,
        0x01..=0x04 | 0x07 | 0x10..=0x1c | 0x52 => 4,
        0x05 | 0x09 | 0x50 | 0x60 => 2,
        0x06 | 0x0a | 0x20..=0x22 | 0x26..=0x28 | 0x51 | 0x64 => 3,
        0x0b | 0x53 | 0x68 => 5,
        _ => 7,
    };
    for op in 0..256 {
        if r.histogram[op] + o.histogram[op] > 0 {
            println!(
                "{op:02x}: retail {:>7} ({:>7} B)  ours {:>7} ({:>7} B)",
                r.histogram[op],
                r.histogram[op] * len(op),
                o.histogram[op],
                o.histogram[op] * len(op)
            );
        }
    }
    Ok(())
}
