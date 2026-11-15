//! Retail data is opened only when explicitly requested; no game fixtures.
use map::{FourCC, Map};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};

struct Compare<R> {
    source: R,
    compared: u64,
}
impl<R: Read> Write for Compare<R> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut original = vec![0; bytes.len()];
        self.source.read_exact(&mut original)?;
        if bytes != original {
            return Err(std::io::Error::other("round-trip byte mismatch"));
        }
        self.compared += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
#[ignore = "requires retail ISO in H2_DISC and a writable TMPDIR"]
fn every_retail_map_roundtrips_and_edits_validate() -> Result<(), Box<dyn std::error::Error>> {
    let disc = std::env::var_os("H2_DISC").ok_or("set H2_DISC to your retail ISO")?;
    let temp = std::path::PathBuf::from(std::env::var_os("TMPDIR").ok_or("set TMPDIR")?);
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc)?))?;
    let entries = iso.entries()?;
    let mut count = 0;
    let mut edited = 0;
    for entry in entries
        .iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
    {
        let base = iso.partition_offset() + u64::from(entry.sector) * xiso::SECTOR;
        let mut map =
            Map::from_region(BufReader::new(File::open(&disc)?), base, entry.size.into())?;
        map.validate_checksum()?;
        let path = temp.join(format!("map-write-test-{}-{count}.map", std::process::id()));
        let mut out = OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(&path)?;
        let size = map.write_to(&mut out)?;
        assert_eq!(size, u64::from(entry.size));
        out.rewind()?;
        let mut written = Map::from_reader(BufReader::new(out.try_clone()?))?;
        written.validate_checksum()?;
        let mut source = BufReader::new(File::open(&disc)?);
        source.seek(SeekFrom::Start(base))?;
        let mut compare = Compare {
            source,
            compared: 0,
        };
        written.write_to(&mut compare)?;
        assert_eq!(compare.compared, size);
        drop(written);
        let weapon = map
            .tags()
            .iter()
            .find(|t| t.group == FourCC(*b"weap"))
            .map(|t| t.id);
        if let Some(id) = weapon {
            let mut editor = map.edit();
            let previous = tags::edit::set_object_bounding_radius(&mut editor, id, 1.25)?;
            editor.rename_tag(id, "mod_tool_demo\\weapon")?;
            out.set_len(0)?;
            out.rewind()?;
            editor.write_to(&mut out)?;
            out.rewind()?;
            let mut changed = Map::from_reader(BufReader::new(out.try_clone()?))?;
            changed.validate_checksum()?;
            assert_eq!(
                changed.tag(id)?.name.as_deref(),
                Some("mod_tool_demo\\weapon")
            );
            let raw = changed.read_tag_bytes(id)?;
            assert_eq!(f32::from_le_bytes(raw[4..8].try_into()?), 1.25);
            assert!(previous.is_finite());
            edited += 1;
        }
        println!(
            "{} bytes={} declared={} exact=PASS checksum=PASS edit={}",
            entry.path,
            size,
            map.header().file_size,
            if weapon.is_some() {
                "PASS"
            } else {
                "no weapon"
            }
        );
        drop(out);
        std::fs::remove_file(path)?;
        count += 1;
    }
    assert!(count > 0, "disc contains no maps");
    assert!(edited > 0, "no weapon edit exercised");
    println!("{count} maps passed through byte-identically, {edited} scalar+rename edits validated");
    Ok(())
}

fn synthetic_object(group: FourCC) -> Vec<u8> {
    let mut b = vec![0; 0x1000];
    b[..4].copy_from_slice(b"daeh");
    b[0x7fc..0x800].copy_from_slice(b"toof");
    b[0x120..0x12e].copy_from_slice(b"02.09.27.09809");
    for (at, word) in [
        (4, 8),
        (8, 0x1000),
        (16, 0x800),
        (20, 0x200),
        (24, 0x600),
        (0x800, 0x8000_0020),
        (0x804, 1),
        (0x808, 0x8000_002c),
        (0x80c, u32::MAX),
        (0x810, u32::MAX),
        (0x818, 1),
        (0x81c, FourCC(*b"tags").as_u32()),
        (0x820, group.as_u32()),
        (0x824, u32::MAX),
        (0x828, u32::MAX),
        (0x82c, group.as_u32()),
        (0x830, 0xa000_0000),
        (0x834, 0x9000_0000),
        (0x838, 0xbc),
        (0xa04, 0.5f32.to_bits()),
    ] {
        b[at..at + 4].copy_from_slice(&word.to_le_bytes());
    }
    b
}

#[test]
fn reviewed_radius_edit_validates_values_classes_and_preserves_other_bytes() {
    use std::io::Cursor;
    let id = map::TagId(0xa000_0000);
    let original = synthetic_object(FourCC(*b"weap"));
    let mut map = Map::from_reader(Cursor::new(original.clone())).unwrap();
    let mut edit = map.edit();
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0] {
        assert!(tags::edit::set_object_bounding_radius(&mut edit, id, bad).is_err());
    }
    assert_eq!(
        tags::edit::set_object_bounding_radius(&mut edit, id, 1.25).unwrap(),
        0.5
    );
    let mut out = Cursor::new(Vec::new());
    edit.write_to(&mut out).unwrap();
    let bytes = out.get_ref();
    for at in 0..bytes.len() {
        if !(0x2d0..0x2d4).contains(&at) && !(0xa04..0xa08).contains(&at) {
            assert_eq!(bytes[at], original[at], "offset {at:#x}");
        }
    }
    let mut map = Map::from_reader(out).unwrap();
    map.validate_checksum().unwrap();
    assert_eq!(
        &map.read_tag_bytes(id).unwrap()[4..8],
        &1.25f32.to_le_bytes()
    );
    let mut other = Map::from_reader(Cursor::new(synthetic_object(FourCC(*b"bitm")))).unwrap();
    assert!(tags::edit::set_object_bounding_radius(&mut other.edit(), id, 1.25).is_err());
}
