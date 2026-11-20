//! The Halo 2 side of the multiplayer placements: netgame flag kinds, read
//! off every multiplayer scenario on the owner's disc (`$H2_DISC`).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;

use map::{CacheType, Map};

/// `scnr+0x118`: netgame flags, `0x20` each: position, facing, `u16` type at
/// `0x10`, `u16` team at `0x12` (crates/tools/scenario-edit/src/layout.rs).
const NETGAME_FLAGS: usize = 0x118;
const FLAG_LEN: usize = 0x20;

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn netgame_flag_kinds_on_the_disc() {
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc).unwrap())).unwrap();
    // type -> (count, maps, teams seen)
    let mut kinds: BTreeMap<u16, (usize, usize, BTreeMap<u16, usize>)> = BTreeMap::new();
    for e in iso.entries().unwrap() {
        if !e.path.ends_with(".map") {
            continue;
        }
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let Ok(mut m) = Map::from_region(
            BufReader::new(File::open(&disc).unwrap()),
            base,
            e.size.into(),
        ) else {
            continue;
        };
        if m.header().cache_type != CacheType::Multiplayer {
            continue;
        }
        let s = m.scenario().unwrap();
        let (id, address) = (s.id, s.location.unwrap().address);
        let b = m.read_tag_bytes(id).unwrap();
        let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let (count, at) = (
            u(NETGAME_FLAGS) as usize,
            (u(NETGAME_FLAGS + 4).wrapping_sub(address)) as usize,
        );
        let mut seen = std::collections::BTreeSet::new();
        let mut per_map: BTreeMap<u16, usize> = BTreeMap::new();
        let mut line = String::new();
        for i in 0..count {
            let f = &b[at + i * FLAG_LEN..at + (i + 1) * FLAG_LEN];
            let ty = u16::from_le_bytes([f[0x10], f[0x11]]);
            let team = u16::from_le_bytes([f[0x12], f[0x13]]);
            let k = kinds.entry(ty).or_default();
            k.0 += 1;
            *k.2.entry(team).or_default() += 1;
            if seen.insert(ty) {
                k.1 += 1;
            }
            line += &format!(" {ty}/{team}");
            *per_map.entry(ty).or_default() += 1;
            if ty == 4 {
                assert_eq!(team, 8, "{}: oddball spawns are neutral", e.path);
            }
        }
        // Teleporter sources and destinations come in equal numbers.
        assert_eq!(per_map.get(&7), per_map.get(&8), "{}", e.path);
        eprintln!("{}:{line}", e.path);
    }
    for (ty, (n, maps, teams)) in &kinds {
        eprintln!("type {ty}: {n} flags on {maps} maps, teams {teams:?}");
    }
    assert!(!kinds.is_empty());
}

/// `scnr +0x58`, the scenery palette, and the scenery placements at
/// `+0x50` (`0x5c` each, palette index `i16` at `+0`).
const SCENERY: usize = 0x50;
const SCENERY_LEN: usize = 0x5c;
const SCENERY_PALETTE: usize = 0x58;
const PALETTE_LEN: usize = 0x28;

/// Every object palette is read with one stride: `0x000a743b` takes the
/// type's descriptor from the type table (`0x00468630`), its palette block
/// offset (`+0xc`), and the element's datum at `index * 5 * 8 + 4`, so a
/// `0x28`-byte element with its tag reference at `+0` — scenery (type 6,
/// palette `+0x58`) as much as the rest.
#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn every_object_palette_element_is_0x28_bytes_in_the_xbe() {
    use iced_x86::{Decoder, DecoderOptions, Mnemonic, Register};
    let bytes = std::fs::read(std::env::var("H2_XBE").expect("set H2_XBE")).unwrap();
    let x = xbe::Xbe::parse(&bytes).unwrap();
    let code = x.bytes_at(0x000a_743b, 0x40).unwrap();
    let mut d = Decoder::with_ip(32, code, 0x000a_743b, DecoderOptions::NONE);
    let c: Vec<_> = d.iter().take_while(|i| i.ip() < 0x000a_7477).collect();
    let at = |ip: u64| c.iter().find(|i| i.ip() == ip).unwrap();
    // mov ecx, [ebp*4 + 0x468630]: the type's descriptor.
    let i = at(0xa743b);
    assert_eq!(
        (i.mnemonic(), i.memory_displacement32()),
        (Mnemonic::Mov, 0x0046_8630)
    );
    assert_eq!(
        (i.memory_index(), i.memory_index_scale()),
        (Register::EBP, 4)
    );
    // movsx eax, word [ecx + 0xc]: its palette block's offset in scnr.
    let i = at(0xa7442);
    assert_eq!(
        (i.mnemonic(), i.memory_displacement32()),
        (Mnemonic::Movsx, 0xc)
    );
    // lea edx, [ecx + ecx*4]; mov edx, [ecx + edx*8 + 4]: element
    // index * 0x28, its datum at +4.
    let i = at(0xa746d);
    assert_eq!((i.mnemonic(), i.memory_index_scale()), (Mnemonic::Lea, 4));
    let i = at(0xa7473);
    assert_eq!((i.mnemonic(), i.memory_index_scale()), (Mnemonic::Mov, 8));
    assert_eq!(i.memory_displacement32(), 4);
}

/// D: on every disc map, the scenery palette's elements at `0x28` each
/// name a `scen` (or nothing), and every scenery placement's palette index
/// is in it.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn the_scenery_palette_is_0x28_bytes_an_element_on_the_disc() {
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc).unwrap())).unwrap();
    let (mut maps, mut entries, mut placements) = (0, 0, 0);
    for e in iso.entries().unwrap() {
        if !e.path.ends_with(".map") || e.path.contains("shared") || e.path.contains("mainmenu") {
            continue;
        }
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let Ok(mut m) = Map::from_region(
            BufReader::new(File::open(&disc).unwrap()),
            base,
            e.size.into(),
        ) else {
            continue;
        };
        let s = m.scenario().unwrap();
        let (id, address) = (s.id, s.location.unwrap().address);
        let b = m.read_tag_bytes(id).unwrap();
        let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let block = |field: usize, len: usize| -> Vec<Vec<u8>> {
            let n = u(field) as usize;
            if n == 0 {
                return Vec::new();
            }
            let at = u(field + 4);
            let off = m.file_offset(at, (n * len) as u32).unwrap();
            let mut r = Map::from_region(
                BufReader::new(File::open(&disc).unwrap()),
                base,
                e.size.into(),
            )
            .unwrap();
            let bytes = r.read_file_range(off, (n * len) as u64).unwrap();
            bytes.chunks(len).map(<[u8]>::to_vec).collect()
        };
        let _ = address;
        let palette = block(SCENERY_PALETTE, PALETTE_LEN);
        for (k, p) in palette.iter().enumerate() {
            let group = u32::from_le_bytes(p[0..4].try_into().unwrap());
            let datum = u32::from_le_bytes(p[4..8].try_into().unwrap());
            if datum == u32::MAX {
                continue;
            }
            let t = m.tag(map::TagId(datum)).unwrap();
            assert_eq!(t.group, map::FourCC(*b"scen"), "{} [{k}]", e.path);
            assert_eq!(group, map::FourCC(*b"scen").as_u32(), "{} [{k}]", e.path);
            entries += 1;
        }
        for (k, p) in block(SCENERY, SCENERY_LEN).iter().enumerate() {
            let i = i16::from_le_bytes([p[0], p[1]]);
            assert!(i < palette.len() as i16, "{} placement {k}: {i}", e.path);
            placements += 1;
        }
        maps += 1;
    }
    eprintln!("{maps} maps: {entries} scenery palette entries, {placements} placements");
    assert!(entries > 0 && placements > 0);
}

/// Weather: every reference to a weather system (`weat`) on the disc,
/// found by scanning every tag for the group word and a `weat` datum, is
/// the tag reference at `+0x20` of a `0x88`-byte element of a weather
/// palette: the structure's (`sbsp +0x84`) or the scenario's
/// (`scnr +0x260`). Nothing else names one.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn weather_systems_are_named_by_the_weather_palettes() {
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc).unwrap())).unwrap();
    let weat = map::FourCC(*b"weat");
    let mut seen: BTreeMap<(String, usize), usize> = BTreeMap::new();
    for e in iso.entries().unwrap() {
        if !e.path.ends_with(".map") {
            continue;
        }
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let Ok(mut m) = Map::from_region(
            BufReader::new(File::open(&disc).unwrap()),
            base,
            e.size.into(),
        ) else {
            continue;
        };
        let ids: std::collections::HashSet<u32> = m
            .tags()
            .iter()
            .filter(|t| t.group == weat)
            .map(|t| t.id.0)
            .collect();
        if ids.is_empty() {
            continue;
        }
        let tags: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.location.is_some())
            .map(|t| (t.id, t.group, t.location.unwrap().address))
            .collect();
        for (id, group, address) in tags {
            let Ok(b) = m.read_tag_bytes(id) else {
                continue;
            };
            for at in (0..b.len().saturating_sub(7)).step_by(4) {
                let w = u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
                let d = u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap());
                if w == weat.as_u32() && ids.contains(&d) {
                    let palette = match &group.0 {
                        b"sbsp" => 0x84,
                        b"scnr" => 0x260,
                        _ => panic!("{}: a {group} names a weather system", e.path),
                    };
                    let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
                    let (n, first) = (
                        u(palette) as usize,
                        u(palette + 4).wrapping_sub(address) as usize,
                    );
                    let k = (at - 0x20).wrapping_sub(first) / 0x88;
                    assert!(
                        at >= first + 0x20 && k < n && (at - first) % 0x88 == 0x20,
                        "{}: {group} +{at:#x} is outside its weather palette",
                        e.path
                    );
                    *seen.entry((group.to_string(), palette)).or_default() += 1;
                }
            }
        }
    }
    eprintln!("weather palette references: {seen:?}");
    assert!(!seen.is_empty());
}
