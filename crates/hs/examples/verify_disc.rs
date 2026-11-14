//! Opt-in private-disc codec symmetry and source node-count checks.
use std::{fs::File, io::BufReader};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("pass your own disc path")?;
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path)?))?;
    let e = iso.find("default.xbe")?;
    let bytes = iso.read(&e)?;
    let mut catalog = hs::Catalog::from_xbe(&xbe::Xbe::parse(&bytes)?)?;
    let mut maps = 0;
    for e in iso
        .entries()?
        .iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
    {
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let mut map =
            map::Map::from_region(BufReader::new(File::open(&path)?), base, e.size.into())?;
        let check = hs::check_map(&mut map, &mut catalog)
            .map_err(|error| format!("{}: {error}", e.path))?;
        println!(
            "{} slots={} live={} emitted={}",
            e.path, check.validation.slots, check.validation.live_nodes, check.emitted_nodes
        );
        maps += 1;
    }
    println!("{maps} maps; codec symmetry and source node-count checks pass");
    Ok(())
}
