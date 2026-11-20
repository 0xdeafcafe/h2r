use std::{collections::{BTreeMap, BTreeSet}, io::Cursor};

use super::{count_at, donors, h1_iso, h2_map, map_memory, scripting};
use h1campaign::{Options, h2assets::Assets};
use h1convert::convert::{Sandbox, halo1, known, structure_mopp};
use h1map::{CacheType, H1Map, disc::Disc};
use h1objects::{base::Base, sandbox::Objects};
use map::{FourCC, Map, TagId, TagQuery};

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
    ].into_iter().map(|(kind, field, palette)| {
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
    }).collect()
}

fn check_campaign_objects(bytes: Vec<u8>, expected: usize) {
    let mut map = map::Map::from_reader(Cursor::new(bytes)).unwrap();
    let root = map.scenario().unwrap().location.unwrap().address;
    let mem = map_memory(&mut map);
    let mut ids = BTreeSet::new();
    for ty in tags::object::OBJECT_TYPES {
        let Some((field, palette, stride)) = ty.placement else { continue };
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
                assert!(map.tag(map::TagId(tag)).is_ok(), "unresolved H1 object {id:08x}");
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
    let (mut campaign, mut multiplayer, mut sound_total) = (0, 0, 0);
    let (mut rows, mut live, mut vehicles) = (0, 0, 0);
    let (mut fixtures, mut authored_lights, mut promotions) = (0, 0, 0);
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
        assert_eq!(scenery_rows - scenery, usize::from(matches!(name.as_str(), "a10" | "a30" | "b40" | "c40")), "{name}");
        let source_sounds = authored_script_sounds(&h1, &scripts);
        sound_total += source_sounds.len();
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
                            check_campaign_objects(done.bytes.clone(), counts.values().map(|v| v.1).sum());
                            let mut out = Map::from_reader(Cursor::new(done.bytes)).unwrap();
                            assert_script_sounds(&mut out, &scripts, &source_sounds);
                            let (count, authored) = assert_fixtures(&h1, &mut out);
                            promotions += assert_installed_promotions(&h1, &mut out);
                            if assets == Assets::Halo1 {
                                fixtures += count;
                                authored_lights += authored;
                                assert_rebuilt_emblem(&mut out);
                            }
                        }
                        Err(e) => failed.push(format!("{name} {assets:?}: {e}")),
                    }
                }
            }
            CacheType::Multiplayer => {
                multiplayer += 1;
                vehicles += counts["vehicles"].1;
                assert!(source_sounds.is_empty(), "{name} has script sounds");
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
                        assert!(p.dropped.iter().all(|line| !line.starts_with("vehicle ")), "{name}");
                        let mopp = structure_mopp(&p.collision)?;
                        p.build_fresh(&mut base, &mopp)
                    });
                    match result {
                        Ok(done) => {
                            let mut out = Map::from_reader(Cursor::new(done.bytes)).unwrap();
                            let (count, authored) = assert_fixtures(&h1, &mut out);
                            promotions += assert_installed_promotions(&h1, &mut out);
                            if sandbox == Sandbox::Halo1 {
                                fixtures += count;
                                authored_lights += authored;
                                assert_rebuilt_emblem(&mut out);
                            }
                        }
                        Err(e) => failed.push(format!("{name} {sandbox:?}: {e}")),
                    }
                }
            }
            _ => {}
        }
    }
    assert_eq!((campaign, multiplayer), (10, 13), "retail source maps");
    assert_eq!((rows, live, vehicles), (11_624, 11_620, 16));
    assert_eq!(sound_total, 570, "distinct script sounds counted per map");
    assert_eq!((fixtures, authored_lights), (886, 42));
    assert!(
        promotions > 0,
        "no rebuilt sound promotion reached the final maps"
    );
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

fn authored_script_sounds(h1: &H1Map, scripts: &h1campaign::Scripting) -> BTreeSet<String> {
    let tables = h1scripts::Tables::read(h1).unwrap();
    let sound_type = scripts
        .catalog
        .types()
        .iter()
        .position(|ty| ty == "sound")
        .unwrap() as u16;
    let atoms = tables.atom_tokens(sound_type);
    let refs: BTreeSet<_> = tables
        .references
        .iter()
        .filter(|r| r.group == h1sounds::h1::SND && !r.id.is_none())
        .map(|r| h1.tag(r.id).unwrap().name.to_ascii_lowercase())
        .collect();
    for name in atoms {
        let name = name.to_ascii_lowercase();
        assert!(
            name == "none" || refs.contains(&name),
            "{}: {name}",
            h1.header().name
        );
    }
    refs
}

fn assert_script_sounds(
    out: &mut Map<Cursor<Vec<u8>>>,
    scripts: &h1campaign::Scripting,
    expected: &BTreeSet<String>,
) {
    let scenario = hs::Scenario::read(out).unwrap();
    let snd = FourCC(*b"snd!");
    let refs: BTreeSet<_> = scenario
        .references()
        .iter()
        .filter(|r| r.group() == snd)
        .map(|r| {
            out.tag(r.id())
                .unwrap()
                .name
                .as_ref()
                .unwrap()
                .to_ascii_lowercase()
        })
        .collect();
    assert!(
        expected.is_subset(&refs),
        "{}: missing script sounds {:?}",
        out.header().name,
        expected.difference(&refs).collect::<Vec<_>>()
    );

    let sound_type = scripts.target.type_index("sound").unwrap();
    for node in scenario.nodes().iter().filter(|n| {
        n.live()
            && n.atom()
            && !n.global()
            && n.opcode() == sound_type
            && n.value_type() != hs::vm::HsType::FUNCTION_NAME.0
    }) {
        let token = scenario.string(node.string_offset()).unwrap();
        let id = TagId(node.data());
        if token.eq_ignore_ascii_case("none") {
            assert_eq!(id, TagId::NULL);
        } else {
            let sound = out
                .tag(id)
                .unwrap_or_else(|e| panic!("{}: script sound {token}: {e}", out.header().name));
            assert_eq!(sound.group, snd);
            assert!(
                sound.name.as_ref().unwrap().eq_ignore_ascii_case(token),
                "{token}"
            );
        }
    }
}

fn assert_rebuilt_emblem(out: &mut Map<Cursor<Vec<u8>>>) {
    let emblem = out
        .tag(TagQuery::NameInGroup(
            h1objects::convert::EMBLEM_SHADER,
            FourCC(*b"shad"),
        ))
        .unwrap_or_else(|e| panic!("{}: rebuilt player emblem: {e}", out.header().name))
        .id;
    let (program, maps) = scenario_edit::scene::shader_bitmaps(out, emblem).unwrap();
    assert_eq!(program, h1convert::shad::EMBLEM_STEM);
    assert_eq!(maps.len(), 1);
    assert_eq!(
        out.tag(maps[0]).unwrap().name.as_deref(),
        Some(h1convert::shad::EMBLEM_BITMAP)
    );
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn assert_fixtures(h1: &H1Map, out: &mut Map<Cursor<Vec<u8>>>) -> (usize, usize) {
    use h1convert::fixture::{Lighting, PALETTE, PLACEMENTS};

    let scenario = h1.tag_data(h1.scenario()).unwrap();
    let space = h1.tag_space();
    let placements = space
        .block_at(scenario, PLACEMENTS.0, PLACEMENTS.1)
        .unwrap();
    if placements.is_empty() {
        return (0, 0);
    }
    let source_palette = space.block_at(scenario, PALETTE.0, PALETTE.1).unwrap();
    let m = map_memory(out);
    let root = out.read_tag_bytes(out.scenario().unwrap().id).unwrap();
    let count = word(&root, 0xa8) as usize;
    let preceding_machines = if h1.header().cache_type == CacheType::Campaign {
        word(scenario, h1campaign::scenario::h1::MACHINES.0) as usize
    } else {
        0
    };
    assert_eq!(
        count,
        placements.len() + preceding_machines,
        "{}",
        h1.header().name
    );
    let mut authored = 0;
    for (i, source) in placements.iter().enumerate() {
        let dest = m
            .slice(
                word(&root, 0xac) + (i + preceding_machines) as u32 * 0x48,
                0x48,
            )
            .unwrap();
        assert_eq!(&dest[8..0x20], &source[8..0x20], "fixture {i} transform");
        assert_eq!(
            &dest[0x34..0x3c],
            &source[0x28..0x30],
            "fixture {i} device fields"
        );
        let index = |bytes: &[u8]| u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
        let entry = m
            .slice(word(&root, 0xb4) + index(dest) as u32 * 0x28, 8)
            .unwrap();
        let id = TagId(word(entry, 4));
        let original = h1map::TagRef::read(source_palette[index(source)], 0).unwrap();
        let fixture = h1.tag(original.id).unwrap();
        let converted = out.tag(id).unwrap();
        assert_eq!(converted.group, FourCC(*b"mach"));
        assert!(converted.name.as_ref().unwrap().starts_with(&fixture.name));
        let Some(lighting) = Lighting::read(source).unwrap() else {
            continue;
        };
        authored += 1;
        let machine = out.read_tag_bytes(id).unwrap();
        let mut lights = 0;
        for j in 0..word(&machine, 0x94) {
            let a = m.slice(word(&machine, 0x98) + j * 0x18, 8).unwrap();
            if word(a, 0) != FourCC(*b"ligh").as_u32() {
                continue;
            }
            lights += 1;
            let light = out.read_tag_bytes(TagId(word(a, 4))).unwrap();
            for field in [0x40, 0x4c, 0x58, 0x64] {
                for (k, component) in lighting.rgb.iter().enumerate() {
                    assert_eq!(word(&light, field + 4 * k), component.to_bits());
                }
            }
            for field in [0x70, 0x74] {
                assert_eq!(word(&light, field), lighting.intensity.to_bits());
            }
        }
        assert!(
            lights > 0,
            "{} fixture {i} has no attached light",
            h1.header().name
        );
    }
    (placements.len(), authored)
}

fn assert_installed_promotions(h1: &H1Map, out: &mut Map<Cursor<Vec<u8>>>) -> usize {
    use tags::sound::h1_promotion::*;

    let sounds: Vec<_> = out
        .tags()
        .iter()
        .filter_map(|tag| {
            if tag.group != FourCC(*b"snd!") {
                return None;
            }
            let source = h1.find(h1sounds::h1::SND, tag.name.as_deref()?)?;
            Some((source, tag.id))
        })
        .collect();
    let mut promotions = 0;
    for (source, id) in sounds {
        let source_bytes = h1.tag_data(source).unwrap();
        let target = word(source_bytes, 0x7c);
        let count = u16::from_le_bytes([source_bytes[0x80], source_bytes[0x81]]);
        if target == u32::MAX && count == 0 {
            continue;
        }
        let bytes = out.read_tag_bytes(id).unwrap();
        assert!(present(&bytes), "{}: lost promotion", source.name);
        assert_eq!(word(&bytes, COUNT as usize), u32::from(count));
        assert_eq!(word(&bytes, MILLIS as usize), word(source_bytes, 0x84));
        let rebuilt = TagId(word(&bytes, TARGET as usize));
        if target == u32::MAX {
            assert_eq!(rebuilt, TagId::NULL);
        } else {
            let live = out.tag(rebuilt).unwrap();
            assert_eq!(live.group, FourCC(*b"snd!"));
            assert_eq!(
                live.name.as_ref(),
                Some(&h1.tag(h1map::TagId(target)).unwrap().name)
            );
        }
        promotions += 1;
    }
    promotions
}
