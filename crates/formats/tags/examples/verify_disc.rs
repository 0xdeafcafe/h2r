//! Private-disc structural validation; emits aggregate counts only.
//! cargo run -p tags --example verify_disc -- /path/to/halo2.iso
use map::{FourCC, Map, ResourceFile};
use std::{collections::BTreeMap, fs::File, io::BufReader};
use tags::wire::BinaryRecord;
use tags::{DecodedTag, ResourceSizes, TagData};

fn roundtrip<T: BinaryRecord>(v: &T, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut encoded = vec![0; T::SIZE];
    v.encode_into(&mut encoded)?;
    if encoded != bytes[..T::SIZE] {
        return Err("root record did not round-trip".into());
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("pass your own disc image path")?;
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path)?))?;
    let entries = iso.entries()?;
    let mut counts = BTreeMap::<FourCC, [usize; 7]>::new();
    let mut maps = 0;
    for entry in entries
        .iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
    {
        let base = iso.partition_offset() + u64::from(entry.sector) * xiso::SECTOR;
        let mut map =
            Map::from_region(BufReader::new(File::open(&path)?), base, entry.size.into())?;
        let mut sizes = ResourceSizes::default();
        sizes.set(ResourceFile::Local, entry.size.into());
        for (file, name) in [
            (ResourceFile::MainMenu, "/maps/mainmenu.map"),
            (ResourceFile::Shared, "/maps/shared.map"),
            (
                ResourceFile::SinglePlayerShared,
                "/maps/single_player_shared.map",
            ),
        ] {
            if let Some(e) = entries.iter().find(|e| e.path.eq_ignore_ascii_case(name)) {
                sizes.set(file, e.size.into());
            }
        }
        let ids: Vec<_> = map
            .tags()
            .iter()
            .filter(|t| DecodedTag::supports(t.group))
            .map(|t| t.id)
            .collect();
        for id in ids {
            let decoded =
                DecodedTag::read(&mut map, id).map_err(|e| format!("{} {id}: {e}", entry.path))?;
            let raw = decoded.validate_resources(&sizes)?;
            let original = map.read_tag_bytes(id)?;
            let c = counts.entry(decoded.group).or_default();
            c[0] += 1;
            c[1] += decoded.validation.blocks;
            c[2] += decoded.validation.nonempty_blocks;
            c[3] += raw;
            c[4] += decoded.validation.records;
            match &decoded.data {
                TagData::Bitmap(v) => {
                    roundtrip(&v.header, &original)?;
                    c[5] += v.images.len();
                    c[6] += v
                        .images
                        .iter()
                        .filter(|i| matches!(i.format, tags::PixelFormat::Unknown(_)))
                        .count();
                }
                TagData::Scenario(v) => roundtrip(&v.header, &original)?,
                TagData::Globals(v) => roundtrip(&v.header, &original)?,
                TagData::Bsp(v) => roundtrip(&v.header, &original)?,
                TagData::RenderModel(v) => roundtrip(&v.header, &original)?,
            }
        }
        maps += 1;
    }
    println!("{maps} maps; root codecs round-trip exactly");
    println!("group tags block_fields nonempty_blocks raw_ranges records images unknown_formats");
    for (g, c) in counts {
        println!(
            "{g} {} {} {} {} {} {} {}",
            c[0], c[1], c[2], c[3], c[4], c[5], c[6]
        );
    }
    Ok(())
}
