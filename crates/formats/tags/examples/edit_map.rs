//! Edit a reviewed weapon field and rename its tag in a standalone map or ISO entry.
//! cargo run -p tags --example edit_map -- /path/to/input.map
//! Output is created exclusively in TMPDIR; input is always read-only.
use map::{FourCC, Map};
use std::{
    fs::{File, OpenOptions},
    io::{BufReader, BufWriter},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = std::env::args_os()
        .nth(1)
        .ok_or("pass a standalone input map")?;
    let temp = std::env::var_os("TMPDIR").ok_or("set TMPDIR")?;
    let output =
        std::path::PathBuf::from(temp).join(format!("edited-map-{}.map", std::process::id()));
    let mut map = if let Some(name) = std::env::args().nth(2) {
        let mut iso = xiso::Xiso::open(BufReader::new(File::open(&input)?))?;
        let entry = iso.find(&name)?;
        if entry.is_dir {
            return Err("expected a map file, not a directory".into());
        }
        let base = iso.partition_offset() + u64::from(entry.sector) * xiso::SECTOR;
        Map::from_region(BufReader::new(File::open(&input)?), base, entry.size.into())?
    } else {
        Map::from_reader(BufReader::new(File::open(&input)?))?
    };
    map.validate_checksum()?;
    let id = map
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"weap"))
        .ok_or("map has no weapon tag")?
        .id;
    let mut editor = map.edit();
    let old = tags::edit::set_object_bounding_radius(&mut editor, id, 1.25)?;
    editor.rename_tag(id, "mod_tool_demo\\weapon")?;
    {
        let mut out = BufWriter::new(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)?,
        );
        editor.write_to(&mut out)?;
        std::io::Write::flush(&mut out)?;
    }
    let mut reread = Map::open(&output)?;
    reread.validate_checksum()?;
    let bytes = reread.read_tag_bytes(id)?;
    assert_eq!(f32::from_le_bytes(bytes[4..8].try_into()?), 1.25);
    assert_eq!(
        reread.tag(id)?.name.as_deref(),
        Some("mod_tool_demo\\weapon")
    );
    println!(
        "weapon {id}: bounding radius {old} -> 1.25; name offsets rebuilt; checksum verified\n{}",
        output.display()
    );
    Ok(())
}
