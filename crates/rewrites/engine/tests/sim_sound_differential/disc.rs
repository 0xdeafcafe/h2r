//! Ladder step 3 for impulse admission: every locally stored snd! in every
//! map, with its map's actual promotion rules, timer counts and cooldowns.
//! Only the two simulated address spaces see the owner's tag payload.
use super::*;
use map::{FourCC, Map};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

const SELECT: u32 = 0x126ec0;
const TAG_TABLE: u32 = 0x600000;
const HEADER: u32 = 0x700000;
const GLOBALS: u32 = 0x700100;
const GESTALT: u32 = 0x700400;
const RESULT: u32 = 0x700800;
const PROMOTIONS: u32 = 0x701000;
const RULE_DATA: u32 = 0x710000;

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

// Load a tag block with its map's address translation. The synthetic
// addresses in each arm must point to the same rules and writable timers.
fn block(map: &mut Map<BufReader<File>>, owner: &[u8], offset: usize, stride: u32) -> Vec<u8> {
    let size = word(owner, offset).checked_mul(stride).unwrap();
    if size == 0 {
        return Vec::new();
    }
    let offset = map.file_offset(word(owner, offset + 4), size).unwrap();
    map.read_file_range(offset, size.into()).unwrap()
}

fn install_promotions(g: &mut Guest, rows: &[u8], blocks: &[(Vec<u8>, Vec<u8>)]) {
    g.set_u32(0x4e3b44, TAG_TABLE).unwrap();
    g.set_u32(0x4e6380, GLOBALS).unwrap();
    g.set_u32(0x51ebd4, GESTALT).unwrap();
    g.set_u32(GESTALT + 0x48, blocks.len() as u32).unwrap();
    g.set_u32(GESTALT + 0x4c, PROMOTIONS).unwrap();
    assert!(PROMOTIONS + rows.len() as u32 <= RULE_DATA);
    g.write_bytes(PROMOTIONS, rows, 1).unwrap();
    let mut cursor = RULE_DATA;
    for (i, (rules, timers)) in blocks.iter().enumerate() {
        let row = PROMOTIONS + i as u32 * 28;
        for (child, off) in [(rules, 4), (timers, 12)] {
            let end = cursor
                .checked_add(u32::try_from(child.len()).unwrap())
                .unwrap();
            assert!(end < STACK_BOTTOM);
            g.set_u32(row + off, cursor).unwrap();
            g.write_bytes(cursor, child, 1).unwrap();
            cursor = end;
        }
    }
    g.set_u32(STACK + 4, RESULT).unwrap();
}

#[test]
#[ignore = "sweeps local snd! tags and promotion records from the owner's H2_DISC and H2_XBE"]
fn every_disc_sound_admission_matches_the_guest() {
    let image = Image::require();
    let mut code = image.original();
    code.prime(SELECT);
    let (body, _) = engine::sim_sound::TABLE
        .at(SELECT)
        .expect("sound promotion body");
    let table = Table::new(std::iter::once(body)).unwrap();
    let path = std::env::var_os("H2_DISC")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../disc/halo2.iso")
        });
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let part = iso.partition_offset();
    let mut entries = iso.entries().unwrap();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut compared = 0u64;
    let mut with_rule = 0u64;
    let mut external = 0u64;
    let fixture = Case {
        setup: |_, _, _| {},
        reply: |_, _, target, _| panic!("unexpected promotion callout {target:#x}"),
        seed: 0,
    };
    for entry in entries {
        let name = entry.path.to_ascii_lowercase();
        if entry.is_dir || !name.starts_with("/maps/") || !name.ends_with(".map") {
            continue;
        }
        let mut map = Map::from_region(
            BufReader::new(File::open(&path).unwrap()),
            part + u64::from(entry.sector) * xiso::SECTOR,
            u64::from(entry.size),
        )
        .unwrap();
        let ids: Vec<_> = map
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"snd!"))
            .filter_map(|t| {
                if t.location.is_some() {
                    Some(t.id)
                } else {
                    external += 1;
                    None
                }
            })
            .collect();
        if ids.is_empty() {
            continue;
        }
        let gestalt = map
            .tags()
            .iter()
            .find(|t| t.group == FourCC(*b"ugh!") && t.location.is_some())
            .map(|t| t.id);
        let rows = gestalt
            .map(|id| {
                let root = map.read_tag_bytes(id).unwrap();
                block(&mut map, &root, 0x48, 28)
            })
            .unwrap_or_default();
        let blocks: Vec<_> = rows
            .chunks_exact(28)
            .map(|row| (block(&mut map, row, 0, 16), block(&mut map, row, 8, 4)))
            .collect();

        let (mut reference_ram, base_cpu) = start(image, SELECT, &fixture);
        let (mut native_ram, _) = start(image, SELECT, &fixture);
        install_promotions(&mut reference_ram, &rows, &blocks);
        install_promotions(&mut native_ram, &rows, &blocks);
        for (n, id) in ids.into_iter().enumerate() {
            let header = map.read_tag_bytes(id).unwrap();
            let header = &header[..tags::sound::SoundHeader::SIZE];
            let index = header[12] as i8;
            assert!(
                index == -1 || (index as usize) < blocks.len(),
                "{name} {id}: promotion {index}"
            );
            with_rule += u64::from(index != -1);
            for g in [&mut reference_ram, &mut native_ram] {
                g.set_u32(TAG_TABLE + id.index() as u32 * 16 + 8, HEADER)
                    .unwrap();
                g.write_bytes(HEADER, header, 1).unwrap();
                g.set_u32(GLOBALS + 0x84, n as u32).unwrap();
                g.set_u32(RESULT, 0x5a5a_5a5a).unwrap();
            }
            let mut reference = base_cpu.clone();
            reference.gpr[1] = id.0;
            let mut native = from_interp(&reference);
            let a = segment(&mut code, &mut reference, &mut reference_ram);
            let b = sim_objects::drive(&mut native, &mut native_ram, &table, SELECT).unwrap();
            let context = format!("{name} {id}: promotion {index}");
            assert_eq!(a, TransferAbi::ret(EXIT), "{context}");
            assert_eq!(b, a, "{context}");
            native.eip = b.target_pc;
            let diff = lifted::cpu::diff(&to_interp(&native), &reference);
            assert!(diff.is_empty(), "{context}: {diff:?}");
            compare_ram(&reference_ram, &native_ram, reference.gpr[4], &context);
            compared += 1;
        }
    }
    assert!(
        compared > 0,
        "no locally stored sound tags in the selected image"
    );
    assert!(with_rule > 0, "no stored sound referenced a promotion rule");
    eprintln!(
        "{compared} sound tags compared, {with_rule} promoted, {external} external references"
    );
}
