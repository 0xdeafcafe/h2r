use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
};

use super::{count_at, donors, h1_iso, h2_map, map_memory, scripting};
use h1campaign::{Options, h2assets::Assets};
use h1convert::convert::{Sandbox, halo1, known, structure_mopp};
use h1map::{CacheType, H1Map, disc::Disc};
use h1objects::{base::Base, sandbox::Objects};

/// Count source rows independently of the converter. Halo 1's two no-object
/// branches are at 0x90cc1 and 0x90cfa.
fn placement_counts(map: &H1Map) -> BTreeMap<&'static str, (usize, usize)> {
    use h1campaign::scenario::h1::*;
    let root = map.tag_data(map.scenario()).unwrap();
    let space = map.tag_space();
    let block = |(field, size)| space.block_at(root, field, size).unwrap();
    [
        ("scenery", SCENERY, SCENERY_PALETTE),
        ("bipeds", BIPEDS, BIPED_PALETTE),
        ("vehicles", VEHICLES, VEHICLE_PALETTE),
        ("equipment", EQUIPMENT, EQUIPMENT_PALETTE),
        ("weapons", WEAPONS, WEAPON_PALETTE),
        ("machines", MACHINES, MACHINE_PALETTE),
        ("controls", CONTROLS, CONTROL_PALETTE),
        ("light fixtures", LIGHT_FIXTURES, LIGHT_FIXTURE_PALETTE),
    ]
    .into_iter()
    .map(|(kind, field, palette)| {
        let palette = block(palette);
        let rows = block(field);
        let mut live = 0;
        for row in &rows {
            let slot = i16::from_le_bytes(row[..2].try_into().unwrap());
            if slot != -1 {
                let entry = palette[usize::try_from(slot).unwrap()];
                let reference = h1map::TagRef::read(entry, 0).unwrap();
                if !reference.id.is_none() {
                    map.tag(reference.id).unwrap();
                    live += 1;
                }
            }
        }
        (kind, (rows.len(), live))
    })
    .collect()
}

fn check_campaign_objects(name: &str, bytes: Vec<u8>, expected: usize) {
    let mut map = map::Map::from_reader(Cursor::new(bytes)).unwrap();
    let root = map.scenario().unwrap().location.unwrap().address;
    let mem = map_memory(&mut map);
    if name == "a50" {
        let (field, stride) = h1ai::h2::layout::SQUADS;
        let count = count_at(&mem, root + field as u32);
        assert_eq!(count, 386, "a50 needs all 386 native slots");
        let address = count_at(&mem, root + field as u32 + 4);
        let rows = mem.slice(address, count * stride as u32).unwrap();
        for name in [
            "grav_mus_hall_covenant/grav_mus_manuever",
            "control_covenant/squad_all",
        ] {
            let native = &name.as_bytes()[..name.len().min(31)];
            assert!(
                rows.chunks_exact(stride).any(|row| {
                    row.split(|&b| b == 0).next() == Some(native)
                        && i16::from_le_bytes(row[0x42..0x44].try_into().unwrap()) >= 0
                }),
                "a50 missing maneuver destination {name}"
            );
        }
    }
    let mut ids = BTreeSet::new();
    for ty in tags::object::OBJECT_TYPES {
        let Some((field, palette, stride)) = ty.placement else {
            continue;
        };
        let count = count_at(&mem, root + u32::from(field));
        let address = count_at(&mem, root + u32::from(field) + 4);
        for i in 0..count {
            let row = address + i * u32::from(stride);
            let id = count_at(&mem, row + 0x28);
            if id >> 16 == h1campaign::scenario::UNIQUE_ID_HIGH {
                assert!(ids.insert(id), "duplicate H1 object {id:08x}");
                let slot = count_at(&mem, row) & 0xffff;
                let field = root + u32::from(palette);
                assert!(slot < count_at(&mem, field));
                let tag = count_at(&mem, count_at(&mem, field + 4) + slot * 0x28 + 4);
                assert!(
                    map.tag(map::TagId(tag)).is_ok(),
                    "unresolved H1 object {id:08x}"
                );
            }
        }
    }
    assert_eq!(ids.len(), expected);
}

#[test]
#[ignore = "reads H1_ISO and H2_DISC and converts both asset sets for every retail level"]
fn both_sandboxes_rebuild_every_retail_map() {
    let mut iso = h1_iso();
    let disc = Disc::open(&mut iso).unwrap();
    let donors = donors();
    let scripts = scripting();
    let mut failed = Vec::new();
    let (mut campaign, mut multiplayer) = (0, 0);
    let (mut rows, mut live, mut vehicles) = (0, 0, 0);
    for entry in &disc.maps {
        let file = iso.read(entry).unwrap();
        let h1 = H1Map::from_bytes(&file).unwrap();
        let name = &h1.header().name;
        if h1.header().cache_type == CacheType::UserInterface {
            continue;
        }
        let counts = placement_counts(&h1);
        for &(total, placed) in counts.values() {
            rows += total;
            live += placed;
        }
        let (scenery_rows, scenery) = counts["scenery"];
        assert_eq!(
            scenery_rows - scenery,
            usize::from(matches!(name.as_str(), "a10" | "a30" | "b40" | "c40")),
            "{name}"
        );
        match h1.header().cache_type {
            CacheType::Campaign => {
                campaign += 1;
                let shared = h1campaign::Shared::default();
                for assets in [Assets::Halo1, Assets::Halo2] {
                    let mut base = h2_map(h1campaign::base_for(name));
                    let options = Options {
                        assets,
                        scripts: Some(&scripts),
                        donors: Some(&donors),
                        sounds: usize::MAX,
                        shared: Some(&shared),
                        ..Default::default()
                    };
                    match h1campaign::convert(&h1, &mut base, &options) {
                        Ok(done) => {
                            let report = &done.report.scenario.placements;
                            assert_eq!(report.len(), counts.len(), "{name}");
                            for &(kind, written, total, _, _) in report {
                                assert_eq!((total, written), counts[kind], "{name} {kind}");
                            }
                            check_campaign_objects(
                                name,
                                done.bytes,
                                counts.values().map(|v| v.1).sum(),
                            );
                        }
                        Err(e) => failed.push(format!("{name} {assets:?}: {e}")),
                    }
                }
            }
            CacheType::Multiplayer => {
                multiplayer += 1;
                vehicles += counts["vehicles"].1;
                let (_, native) =
                    known(name).unwrap_or_else(|| panic!("no retail base for {name}"));
                let hash = content::Sha256::of(&file);
                for sandbox in [Sandbox::Halo1, Sandbox::Halo2] {
                    let mut base = h2_map(native);
                    let programs = Base::read(&mut base).unwrap();
                    let mut objects = Objects::new(&h1, &programs);
                    let prepared = match sandbox {
                        Sandbox::Halo1 => halo1::prepare_halo1(&h1, hash, &mut base, &mut objects),
                        Sandbox::Halo2 => halo1::prepare_halo2(&h1, hash, &mut base, &mut objects),
                    };
                    let result = prepared.and_then(|p| {
                        let objects = p.scenery.as_ref().unwrap();
                        assert_eq!(objects.placements.len(), scenery, "{name}");
                        assert_eq!(objects.empty_placements, scenery_rows - scenery, "{name}");
                        assert_eq!(p.applied.vehicles, counts["vehicles"].1, "{name}");
                        assert!(
                            p.dropped.iter().all(|line| !line.starts_with("vehicle ")),
                            "{name}"
                        );
                        let mopp = structure_mopp(&p.collision)?;
                        p.build_fresh(&mut base, &mopp)
                    });
                    if let Err(e) = result {
                        failed.push(format!("{name} {sandbox:?}: {e}"));
                    }
                }
            }
            _ => {}
        }
    }
    assert_eq!((campaign, multiplayer), (10, 13), "retail source maps");
    assert_eq!((rows, live, vehicles), (11_624, 11_620, 16));
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
