//! Write a retail multiplayer map with its structure MOPP rebuilt by us, for
//! a game run through `H2_MAP_DIR`. Usage: replace <map> <out dir>
#[path = "../tests/common/mod.rs"]
mod common;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [name, out] = a.as_slice() else {
        anyhow::bail!("usage: replace <map> <out dir>")
    };
    let mut map = common::open_map(name)?;
    let r = common::replace::replace(&mut map, 0)?;
    std::fs::create_dir_all(out)?;
    let path = std::path::Path::new(out).join(format!("{name}.map"));
    std::fs::write(&path, &r.map)?;
    println!(
        "{}: MOPP block {} bytes (retail {}), map {} bytes",
        path.display(),
        r.block.len(),
        r.retail_count,
        r.map.len()
    );
    Ok(())
}
