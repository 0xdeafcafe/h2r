//! Disc proof for Blood Gulch's cyborg, weapon projectiles and damage.

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1convert::convert::halo1::prepare_halo1;
use h1convert::convert::structure_mopp;
use h1convert::surface_query::{Outcome, vector};
use h1map::{Group, H1Map};
use h1models::mode::{ModelOptions, world_transforms};
use h1objects::arsenal;
use h1objects::base::Base;
use h1objects::h1;
use h1objects::sandbox::Objects;
use map::{FourCC, Map, TagId};
use tags::StringId;
use tags::collision::Collision;

const CYBORG: &str = r"characters\cyborg_mp\cyborg_mp";
const CHIEF: &str = r"objects\characters\masterchief\masterchief_mp";
type Pose = ([[f32; 3]; 3], [f32; 3]);

fn bloodgulch() -> H1Map {
    let file = File::open(std::env::var("H1_ISO").expect("set H1_ISO")).unwrap();
    let mut iso = xiso::Xiso::open(BufReader::new(file)).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let entry = disc.map("bloodgulch").unwrap().clone();
    H1Map::from_reader(Cursor::new(iso.read(&entry).unwrap())).unwrap()
}

fn coagulation() -> Map<Cursor<Vec<u8>>> {
    let file = File::open(std::env::var("H2_DISC").expect("set H2_DISC")).unwrap();
    let mut iso = xiso::Xiso::open(BufReader::new(file)).unwrap();
    let entry = iso.find("/maps/coagulation.map").unwrap();
    Map::from_reader(Cursor::new(iso.read(&entry).unwrap())).unwrap()
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn half(b: &[u8], at: usize) -> usize {
    usize::from(u16::from_le_bytes(b[at..at + 2].try_into().unwrap()))
}

fn float(b: &[u8], at: usize) -> f32 {
    f32::from_bits(word(b, at))
}

fn floats<const N: usize>(b: &[u8], at: usize) -> [f32; N] {
    std::array::from_fn(|i| float(b, at + 4 * i))
}

fn block(base: &Base, root: &[u8], at: usize, size: usize) -> Vec<Vec<u8>> {
    (0..word(root, at) as usize)
        .map(|i| base.element(root, at, size, i).unwrap().unwrap())
        .collect()
}

fn model(base: &Base, biped: &str) -> (Vec<u8>, Vec<u8>) {
    let biped = base
        .head(base.find(b"bipd", biped).unwrap(), 0x314)
        .unwrap();
    let hlmt = base
        .head(base.reference(&biped, 0x34).unwrap(), 0xfc)
        .unwrap();
    (biped, hlmt)
}

fn local((r, t): &Pose, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|a| (0..3).map(|i| r[i][a] * (p[i] - t[i])).sum())
}

fn nearest(hits: impl Iterator<Item = (f32, usize, u16)>) -> Option<(f32, usize, u16)> {
    hits.min_by(|a, b| a.0.total_cmp(&b.0))
}

fn check_player(h1: &H1Map, out: &mut Map<Cursor<Vec<u8>>>, built: &Base, retail: &Base) {
    let source = h1.find(Group::new(b"bipd"), CYBORG).unwrap();
    let unit = h1::Unit::read(h1, source).unwrap();
    let source_coll = h1.tag(unit.object.collision.unwrap()).unwrap();
    let coll = h1::Collision::read(h1, source_coll).unwrap();
    let mats = h1::elements(h1, h1.tag_data(source_coll).unwrap(), 0x234, 0x48).unwrap();
    let mode = h1models::model_from_map(
        h1,
        h1.tag(unit.object.model.unwrap()).unwrap(),
        &ModelOptions::default(),
    )
    .unwrap()
    .0;
    let (biped, hlmt) = model(built, CYBORG);
    let (chief, chief_hlmt) = model(retail, CHIEF);
    let size = unit.size.unwrap();
    assert_eq!(
        floats::<3>(&biped, 0x268),
        [size.standing_height, size.crouching_height, size.radius]
    );
    assert_eq!(floats::<3>(&chief, 0x268), [0.725, 0.5, 0.175]);
    let pills = block(built, &biped, 0x28c, 0x50);
    assert_eq!(pills.len(), 2);
    for (pill, height) in pills
        .iter()
        .zip([size.standing_height, size.crouching_height])
    {
        assert_eq!(float(pill, 0x2c), size.radius);
        assert!((float(pill, 0x38) + size.radius - height).abs() < 1e-6);
        assert_eq!(float(pill, 0x48), size.radius);
    }
    let spheres = block(built, &biped, 0x284, 0x80);
    assert_eq!(spheres.len(), 3);
    for (sphere, factor) in spheres.iter().zip([1.0, 1.35, 1.7]) {
        assert_eq!(float(sphere, 0x2c), size.radius * factor);
        assert_eq!(float(sphere, 0x78), size.radius * factor);
    }

    let converted_nodes = block(built, &hlmt, 0x78, 0x5c);
    assert_eq!(converted_nodes.len(), mode.nodes.len());
    let source_poses = world_transforms(
        &mode
            .nodes
            .iter()
            .map(|n| (n.parent, n.rotation, n.translation))
            .collect::<Vec<_>>(),
    );
    let converted_poses = world_transforms(
        &converted_nodes
            .iter()
            .map(|n| (half(n, 4) as i16, floats(n, 0x18), floats(n, 0xc)))
            .collect::<Vec<_>>(),
    );
    assert_eq!(source_poses, converted_poses);

    let phmo = built
        .head(built.reference(&hlmt, 0x20).unwrap(), 0x40)
        .unwrap();
    let rigid_bodies = block(built, &phmo, 0x38, 0x90);
    assert!(!rigid_bodies.is_empty());
    for body in rigid_bodies {
        assert!(!coll.nodes[half(&body, 0)].bsps.is_empty());
    }

    let render = built
        .head(built.reference(&hlmt, 0).unwrap(), 0x60)
        .unwrap();
    let rendered_markers = block(built, &render, 0x58, 0xc);
    let targets = block(built, &hlmt, 0x68, 0x1c);
    let native_targets = block(retail, &chief_hlmt, 0x68, 0x1c);
    assert_eq!(targets.len(), 2);
    assert_eq!(targets.len(), native_targets.len());
    for ((target, native), name) in targets.iter().zip(native_targets).zip(["body", "head"]) {
        assert_eq!(&target[4..], &native[4..]);
        assert_eq!(
            StringId(word(target, 0)).resolve(out.strings()).unwrap(),
            Some(name)
        );
        let authored = mode.markers.iter().find(|m| m.name == name).unwrap();
        let marker = rendered_markers
            .iter()
            .find(|m| word(m, 0) == word(target, 0))
            .unwrap();
        let entries = block(built, marker, 4, 0x24);
        assert_eq!(entries.len(), authored.instances.len());
        for (entry, src) in entries.iter().zip(&authored.instances) {
            assert_eq!(entry[2] as i8, src.node);
            assert_eq!(floats::<3>(entry, 4), src.translation);
        }
    }

    let damage = built.element(&hlmt, 0x60, 0xf8, 0).unwrap().unwrap();
    assert_eq!(float(&damage, 0x28), coll.body_vitality);
    assert_eq!(float(&damage, 0x8c), coll.shield_vitality);
    let sections = block(built, &damage, 0xbc, 0x38);
    let materials = block(built, &hlmt, 0x58, 0x14);
    assert_eq!(materials.len(), mats.len());
    for (material, source) in materials.iter().zip(&mats) {
        assert_eq!(
            sections[half(material, 6)][4] & 0x80 != 0,
            source[0x20] & 1 != 0
        );
    }

    let converted = Collision::read(out, built.reference(&hlmt, 8).unwrap()).unwrap();
    let regions = block(built, &hlmt, 0x70, 0x10);
    assert_eq!(regions.len(), 1);
    let region = &converted.regions[usize::from(regions[0][4])];
    let choices = block(built, &regions[0], 8, 8);
    let permutation = &region.permutations[usize::from(choices[0][5])];
    assert_eq!(
        permutation.bsps.len(),
        coll.nodes.iter().filter(|n| !n.bsps.is_empty()).count()
    );
    assert_eq!(permutation.bsp_physics_count, 0);
    for bsp in &permutation.bsps {
        let node = bsp.unknown_00 as usize;
        assert_eq!(coll.nodes[node].name, mode.nodes[node].name);
    }
    let mesh = mode.regions[0].permutations[0].lods[4] as usize;
    let render_top = mode.geometries[mesh]
        .parts
        .iter()
        .flat_map(|part| &part.vertices)
        .map(|vertex| vertex.position[2])
        .fold(f32::NEG_INFINITY, f32::max);
    let collision_top = permutation
        .bsps
        .iter()
        .flat_map(|bsp| {
            let (r, t) = &converted_poses[bsp.unknown_00 as usize];
            bsp.vertices
                .iter()
                .map(move |v| t[2] + (0..3).map(|a| r[2][a] * v.point[a]).sum::<f32>())
        })
        .fold(f32::NEG_INFINITY, f32::max);
    assert!((render_top - collision_top).abs() < 0.01);
    for (height, side, headshot) in [(0.67, 0.0, true), (0.52, 0.0, false), (0.34, 0.04, false)] {
        let from = [1.0, side, height];
        let to = [-1.0, side, height];
        let source = nearest(coll.nodes.iter().enumerate().filter_map(|(i, n)| {
            let bsp = n.bsps.first()?;
            let pose = &source_poses[i];
            match vector(bsp, local(pose, from), local(pose, to)).outcome {
                Outcome::Hit(hit) => Some((hit.t, i, bsp.surfaces[hit.surface].material)),
                Outcome::Clear => None,
            }
        }))
        .expect("source biped hit");
        let result = nearest(permutation.bsps.iter().filter_map(|bsp| {
            let i = bsp.unknown_00 as usize;
            let pose = &converted_poses[i];
            match vector(bsp, local(pose, from), local(pose, to)).outcome {
                Outcome::Hit(hit) => Some((hit.t, i, bsp.surfaces[hit.surface].material)),
                Outcome::Clear => None,
            }
        }))
        .expect("converted biped hit");
        assert_eq!(source, result, "ray at {side} / {height}");
        assert_eq!(mats[usize::from(source.2)][0x20] & 1 != 0, headshot);
    }
}

fn check_weapons(h1: &H1Map, built: &Base, globals: &[u8]) {
    let matrix = built.element(globals, 0xd0, 8, 0).unwrap().unwrap();
    let rows = block(built, &matrix, 0, 0xc);
    assert!(rows.windows(2).all(|w| word(&w[0], 0) < word(&w[1], 0)));
    let ty = [21, 22];
    let mkeys = ty.map(|i| {
        let name = arsenal::MATERIALS[i].1;
        let index = built.materials.iter().position(|m| m == name).unwrap();
        let mat = built.element(globals, 0x150, 0xb4, index).unwrap().unwrap();
        [word(&mat, 0x10), word(&mat, 0x14)]
    });
    let mut projectile_count = 0;
    for t in h1.tags().iter().filter(|t| t.group == Group::new(b"weap")) {
        let Some(id) = built.find(b"weap", &t.name) else {
            continue;
        };
        let weapon = h1::Weapon::read(h1, t).unwrap();
        let h1_bytes = h1.tag_data(t).unwrap();
        let root = built.head(id, 0x2d8).unwrap();
        assert_eq!(
            floats::<5>(&root, 0x208),
            floats::<5>(h1_bytes, 0x3e4),
            "{}",
            t.name
        );
        for (i, trigger) in weapon.triggers.iter().enumerate() {
            let Some(projectile) = trigger.projectile else {
                continue;
            };
            projectile_count += 1;
            let projectile = h1.tag(projectile).unwrap();
            let source = h1.tag_data(projectile).unwrap();
            let barrel = built.element(&root, 0x2d0, 0xec, i).unwrap().unwrap();
            let p = built
                .head(built.reference(&barrel, 0x8c).unwrap(), 0x1a4)
                .unwrap();
            assert_eq!(word(&p, 0xbc), word(source, 0x17c), "{}", projectile.name);
            assert_eq!(float(&p, 0xc8), float(source, 0x1a0));
            assert_eq!(float(&p, 0xe0), float(source, 0x1c8));
            for (a, b) in [(0x17c, 0x1e4), (0x180, 0x1e8), (0x184, 0x1ec)] {
                assert_eq!(float(&p, a), float(source, b) * 30.0);
            }
            let [start, end] = floats::<2>(source, 0x1d0);
            if float(source, 0x1e4) != float(source, 0x1e8) && end > start {
                assert_eq!(floats::<2>(&p, 0x18c), [start, end]);
                assert_eq!(float(&p, 0x194), (end - start).recip());
            }
            let source_projectile = h1::Projectile::read(h1, projectile).unwrap();
            if let Some(d) = source_projectile.impact_damage {
                let d = h1.tag_data(h1.tag(d).unwrap()).unwrap();
                let damage = built
                    .head(built.reference(&p, 0x148).unwrap(), 0xc8)
                    .unwrap();
                assert_eq!(floats::<2>(&damage, 0), floats::<2>(d, 0));
                assert_eq!(floats::<3>(&damage, 0x1c), floats::<3>(d, 0x1d0));
                assert_eq!(word(&damage, 0x14), word(d, 0x1c8));
                if floats::<3>(d, 0x1d0).into_iter().any(|d| d > 0.0) {
                    // Same two row keys and two column keys as 0x000d5bc0.
                    for (material_keys, source_material) in mkeys.iter().zip(ty) {
                        let scale = [word(&damage, 0x50), word(&damage, 0x54)]
                            .into_iter()
                            .filter_map(|key| rows.iter().find(|row| word(row, 0) == key))
                            .flat_map(|row| block(built, row, 4, 8))
                            .filter(|value| material_keys.contains(&word(value, 0)))
                            .map(|value| float(&value, 4))
                            .product::<f32>();
                        assert_eq!(
                            scale,
                            float(d, 0x200 + 4 * source_material),
                            "{}",
                            projectile.name
                        );
                    }
                }
            }
        }
    }
    assert!(
        projectile_count >= 9,
        "Blood Gulch's hand weapons and vehicle guns"
    );
}

fn check_detonation(built: &Base, shot: &str, slot: usize, effect_slot: usize, damage_path: &str) {
    let projectile = built
        .head(built.find(b"proj", shot).unwrap(), 0x1a4)
        .unwrap();
    let primary = built.reference(&projectile, slot).unwrap();
    assert_eq!(built.name(primary).unwrap().1, damage_path);
    let primary_damage = built.head(primary, 0xc8).unwrap();
    assert!(float(&primary_damage, 4) > 0.0);
    assert!(float(&primary_damage, 0x24) > 0.0);

    let effect = built
        .head(built.reference(&projectile, effect_slot).unwrap(), 0x30)
        .unwrap();
    let parts: Vec<TagId> = block(built, &effect, 0x14, 0x38)
        .into_iter()
        .flat_map(|event| block(built, &event, 0x18, 0x38))
        .filter(|part| word(part, 8) == FourCC(*b"jpt!").as_u32())
        .filter_map(|part| built.reference(&part, 0xc))
        .collect();
    assert!(!parts.contains(&primary));
    assert!(parts.iter().any(|id| {
        let impulse = built.head(*id, 0xc8).unwrap();
        float(&impulse, 4) > 0.0 && float(&impulse, 0x24) == 0.0
    }));
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn bloodgulch_aim_targets_collision_and_damage_survive_map_build() {
    let h1 = bloodgulch();
    let mut retail_map = coagulation();
    let retail = Base::read(&mut retail_map).unwrap();
    let mut objects = Objects::new(&h1, &retail);
    let prepared = prepare_halo1(
        &h1,
        content::Sha256::of(b"bloodgulch"),
        &mut retail_map,
        &mut objects,
    )
    .unwrap();
    let mopp = structure_mopp(&prepared.collision).unwrap();
    let fresh = prepared.build_fresh(&mut retail_map, &mopp).unwrap();
    let mut out = Map::from_reader(Cursor::new(fresh.bytes)).unwrap();
    let built = Base::read(&mut out).unwrap();
    check_player(&h1, &mut out, &built, &retail);
    let globals = built.head(out.globals().unwrap().id, 0x284).unwrap();
    check_weapons(&h1, &built, &globals);
    check_detonation(
        &built,
        r"weapons\plasma grenade\plasma grenade",
        0x100,
        0xf8,
        r"weapons\plasma grenade\explosion",
    );
    check_detonation(
        &built,
        r"weapons\needler\needle",
        0x118,
        0x110,
        r"weapons\needler\explosion",
    );
}
