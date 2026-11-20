//! Disc tests: they read the user's own discs (`$H1_ISO` or
//! `disc/halo1.iso`, `$H2_DISC` or `disc/halo2.iso`) at run time and store
//! nothing from them.
//!
//! ```text
//! cargo test -p h1ai --release --test disc -- --ignored --nocapture
//! ```

mod common;

use std::collections::BTreeMap;

use common::{H2_CAMPAIGN, build_and_read, h1_map, h2_map};
use h1campaign::ai::convert::{Options, character_for, convert, convert_for_map};
use h1campaign::ai::h1::{ActorType, H1Ai};
use h1campaign::ai::h2::{Character, ScenarioAi};
use h1map::disc::CAMPAIGN_LEVELS;
use map::FourCC;
use tags::geometry::TagView;

fn world_bounds(m: &h1map::H1Map) -> Vec<[f32; 6]> {
    m.structure_bsps()
        .unwrap()
        .iter()
        .map(|b| m.structure_bsp(b).unwrap().world_bounds())
        .collect()
}

#[test]
#[ignore = "reads the Halo 1 disc"]
fn h1_campaign_ai_reads_on_every_level() {
    println!(
        "{:<4} {:>3} {:>3} {:>4} {:>3} {:>5} {:>4} {:>4} {:>5} {:>4} {:>5} {:>4} {:>4}",
        "map",
        "pal",
        "act",
        "enc",
        "sqd",
        "start",
        "move",
        "plat",
        "fp",
        "cl",
        "atoms",
        "pts",
        "conv"
    );
    let mut types: BTreeMap<String, usize> = BTreeMap::new();
    for level in CAMPAIGN_LEVELS {
        let m = h1_map(level);
        let ai = H1Ai::read(&m).unwrap_or_else(|e| panic!("{level}: {e}"));
        // Dangling atom points and squad platoons are retail data faults
        // (a10's moving_1_3 goes to point 5 of 5; c40 has a squad in
        // platoon 3 of 3): a few per level are printed and counted, and any
        // other bad index fails. So are maneuver destinations past the
        // encounter's squads (a10's first_contact has 2 squads and maneuvers
        // to 2 and 3; b30's a2_top_cov goes to 19 of 12), which the
        // converter treats as no destination.
        let (dangling, bad): (Vec<String>, Vec<String>) = ai
            .check_indices()
            .into_iter()
            .partition(|m| m.contains(" point ") || m.contains(" platoon "));
        let (maneuvers, bad): (Vec<String>, Vec<String>) =
            bad.into_iter().partition(|m| m.contains(": maneuver "));
        assert!(bad.is_empty(), "{level}: {bad:#?}");
        assert!(dangling.len() <= 8, "{level}: {dangling:#?}");
        assert!(maneuvers.len() <= 4, "{level}: {maneuvers:#?}");
        for d in dangling.iter().chain(&maneuvers) {
            println!("{level}: dangling {d}");
        }
        let bounds = world_bounds(&m);
        // How far a point is outside the nearest BSP's world bounds (0
        // inside). Flying actors' firing positions (a30's banshees) are out
        // over the edges, so points within 100 m are counted, not failed.
        let out = |p: [f32; 3]| {
            bounds
                .iter()
                .map(|b| {
                    (0..3)
                        .map(|k| (b[2 * k] - p[k]).max(p[k] - b[2 * k + 1]).max(0.0))
                        .fold(0.0, f32::max)
                })
                .fold(f32::MAX, f32::min)
        };
        let mut outside = [0usize; 3];
        let mut worst = 0.0f32;
        let mut check = |what: usize, p: [f32; 3], label: &dyn Fn() -> String| {
            assert!(p.iter().all(|v| v.is_finite()), "{level} {}", label());
            let d = out(p);
            if d > 0.5 {
                outside[what] += 1;
                worst = worst.max(d);
            }
            assert!(
                d < 100.0,
                "{level} {}: {p:?} is {d} m outside every BSP",
                label()
            );
        };
        let mut n = [0usize; 10];
        for e in &ai.encounters {
            n[0] += e.squads.len();
            n[3] += e.platoons.len();
            n[4] += e.firing_positions.len();
            for f in &e.firing_positions {
                check(0, f.position, &|| format!("{} firing position", e.name));
            }
            for s in &e.squads {
                n[1] += s.starting_locations.len();
                n[2] += s.move_positions.len();
                for l in &s.starting_locations {
                    check(1, l.position, &|| format!("{}/{} start", e.name, s.name));
                    assert!(l.facing.abs() <= 7.0, "{level}: facing {}", l.facing);
                }
                assert!(
                    s.normal_count >= 0 && s.insane_count >= 0,
                    "{level}: counts"
                );
                assert!(s.delay.is_finite() && s.delay >= 0.0);
            }
        }
        for c in &ai.command_lists {
            n[5] += c.atoms.len();
            n[6] += c.points.len();
            for p in &c.points {
                check(2, p.position, &|| format!("{} point", c.name));
            }
        }
        if outside.iter().any(|&o| o > 0) {
            println!(
                "{level}: outside the world bounds: {} firing positions, {} starts, {} points (at most {worst:.1} m)",
                outside[0], outside[1], outside[2]
            );
        }
        for v in ai.actor_palette.iter().flatten() {
            let t = m.tag(v.unit.id).unwrap();
            let g = t.group.to_string();
            assert!(
                g == "bipd" || g == "vehi",
                "{level}: {} unit is a {g}",
                v.name
            );
            if !v.weapon.id.is_none() {
                assert_eq!(
                    m.tag(v.weapon.id).unwrap().group.to_string(),
                    "weap",
                    "{}",
                    v.name
                );
            }
        }
        for i in 0..ai.actor_palette.len() {
            let a = ai.actor_of(i).expect("every palette entry has an actor");
            assert!(a.kind.0 < 16, "{level}: {} type {}", a.name, a.kind.0);
            *types.entry(a.kind.name().to_owned()).or_default() += 1;
        }
        println!(
            "{:<4} {:>3} {:>3} {:>4} {:>3} {:>5} {:>4} {:>4} {:>5} {:>4} {:>5} {:>4} {:>4}",
            level,
            ai.actor_palette.len(),
            ai.actors.len(),
            ai.encounters.len(),
            n[0],
            n[1],
            n[2],
            n[3],
            n[4],
            ai.command_lists.len(),
            n[5],
            n[6],
            ai.conversations.len()
        );
    }
    println!("actor types over all palettes: {types:?}");
}

/// Actor variants and the character each maps to, per level.
#[test]
#[ignore = "reads the Halo 1 disc"]
fn h1_actor_variants_and_their_characters() {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for level in CAMPAIGN_LEVELS {
        let m = h1_map(level);
        let ai = H1Ai::read(&m).unwrap();
        for (i, v) in ai.actor_palette.iter().enumerate() {
            let v = v.as_ref().unwrap();
            let kind = ai.actor_of(i).unwrap().kind;
            let (c, g) = character_for(kind, &v.name);
            seen.entry(format!("{:<14} {}", kind.name(), v.name))
                .or_insert(format!("{c} (grenade {g})"));
        }
    }
    for (k, v) in &seen {
        println!("{k} -> {v}");
    }
}

#[test]
#[ignore = "reads the Halo 2 disc"]
fn h2_retail_census() {
    let mut fp_flags: BTreeMap<u16, usize> = BTreeMap::new();
    let mut zone_flags: BTreeMap<u32, usize> = BTreeMap::new();
    let mut area_kinds: BTreeMap<u16, usize> = BTreeMap::new();
    let mut styles: BTreeMap<i16, usize> = BTreeMap::new();
    let mut chars: BTreeMap<String, (Option<u16>, Vec<&str>)> = BTreeMap::new();
    println!(
        "{:<20} {:>4} {:>4} {:>5} {:>4} {:>5} {:>5} {:>5} {:>4} {:>4}",
        "map", "grp", "sqd", "start", "zone", "fp", "area", "order", "pset", "char"
    );
    for name in H2_CAMPAIGN {
        let mut m = h2_map(name);
        let t = m.scenario().unwrap().clone();
        let payload = m.read_tag_bytes(t.id).unwrap();
        let view = TagView::new(&payload, t.location.unwrap().address);
        let ai = ScenarioAi::read(&view).unwrap_or_else(|e| panic!("{name}: {e}"));
        let bad = ai.check_indices();
        assert!(bad.is_empty(), "{name}: {bad:#?}");
        for z in &ai.zones {
            *zone_flags.entry(z.flags).or_default() += 1;
            for f in &z.firing_positions {
                *fp_flags.entry(f.flags).or_default() += 1;
            }
        }
        for o in &ai.orders {
            *styles.entry(o.style).or_default() += 1;
            for r in o.primary_areas.iter().chain(&o.secondary_areas) {
                *area_kinds.entry(r.kind).or_default() += 1;
            }
        }
        for s in &ai.squads {
            for l in &s.starting_locations {
                assert!(l.position.iter().all(|v| v.is_finite()));
            }
        }
        // Characters: every palette entry is a char, and each decodes.
        let ids: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"char"))
            .map(|t| {
                (
                    t.id,
                    t.name.clone().unwrap_or_default(),
                    t.location.unwrap().address,
                )
            })
            .collect();
        for p in &ai.character_palette {
            // 07a_highcharity has a null entry.
            assert!(
                p.0 == u32::MAX || ids.iter().any(|(id, ..)| id == p),
                "{name}: palette {p:?}"
            );
        }
        let mut decoded = std::collections::HashMap::new();
        for (id, n, addr) in &ids {
            let b = m.read_tag_bytes(*id).unwrap();
            match Character::read(&TagView::new(&b, *addr)) {
                Ok(c) => {
                    decoded.insert(*id, (n.clone(), c));
                }
                Err(e) => println!("{name}: char {n} ({} bytes): {e}", b.len()),
            }
        }
        for (n, c) in decoded.values() {
            // The type, inherited through parents when the tag has none.
            let mut k = c.kind;
            let mut p = c.parent;
            while k.is_none() {
                let Some((_, pc)) = decoded.get(&p) else {
                    break;
                };
                k = pc.kind;
                p = pc.parent;
            }
            chars
                .entry(n.clone())
                .or_insert((k, Vec::new()))
                .1
                .push(name);
        }
        println!(
            "{:<20} {:>4} {:>4} {:>5} {:>4} {:>5} {:>5} {:>5} {:>4} {:>4}",
            name,
            ai.squad_groups.len(),
            ai.squads.len(),
            ai.squads
                .iter()
                .map(|s| s.starting_locations.len())
                .sum::<usize>(),
            ai.zones.len(),
            ai.zones
                .iter()
                .map(|z| z.firing_positions.len())
                .sum::<usize>(),
            ai.zones.iter().map(|z| z.areas.len()).sum::<usize>(),
            ai.orders.len(),
            ai.point_sets.len(),
            ai.character_palette.len()
        );
    }
    println!("firing position flags {fp_flags:x?}");
    println!("zone flags {zone_flags:x?}");
    println!("area set kinds {area_kinds:?}");
    println!("order styles {styles:?}");
    for (n, (k, maps)) in &chars {
        println!(
            "char {n:<62} type {:<15} {}",
            k.map_or_else(|| "?".to_owned(), |k| ActorType(k).name()),
            maps.join(",")
        );
    }
    // Every character the stopgap table names is on the disc, and has the
    // type the table claims.
    for kind in 0..16u16 {
        for words in [
            "",
            "major",
            "stealth",
            "stealth major",
            "zealot",
            "specops",
            "human",
            "johnson",
            "sergeant",
        ] {
            let (path, _) = character_for(ActorType(kind), words);
            let (k, _) = chars
                .get(path)
                .unwrap_or_else(|| panic!("{path} is on no retail map"));
            // Same type, where the retail tag's own type is readable.
            let same = match kind {
                4..=6 | 14 => true, // ai\generic
                // Halo 2's crewman and Johnson are marines and crew both.
                7 | 8 => k.is_none_or(|k| k == 7 || k == 8),
                _ => k.is_none_or(|k| k == kind),
            };
            assert!(same, "{path}: type {k:?}, table maps type {kind}");
        }
    }
}

/// Firing positions over the generated pathfinding: inside a sector's
/// polygon, with the sector's floor at most `below` under the point and at
/// most `above` over it.
fn on_sector(pf: &tags::pathfinding::PathfindingData, p: [f32; 3], below: f32, above: f32) -> bool {
    use pathfind::geom::{contains_xy, newell};
    (0..pf.sectors.len()).any(|k| {
        let Some(ring) = pf.sector_polygon(k) else {
            return false;
        };
        let poly: Vec<[f32; 3]> = ring.into_iter().map(|i| pf.vertices[i]).collect();
        if poly.len() < 3 || !contains_xy(&poly, p[0], p[1]) {
            return false;
        }
        let n = newell(&poly);
        if n[2].abs() < 1e-6 {
            return false;
        }
        let d = n[0] * poly[0][0] + n[1] * poly[0][1] + n[2] * poly[0][2];
        let z = (d - n[0] * p[0] - n[1] * p[1]) / n[2];
        z <= p[2] + above && p[2] - z <= below
    })
}

#[test]
#[ignore = "reads the Halo 1 disc"]
fn a30_and_a10_convert_read_back_and_sit_on_generated_pathfinding() {
    for level in ["a30", "a10"] {
        let m = h1_map(level);
        let src = H1Ai::read(&m).unwrap();
        // Pathfinding generated from each BSP's converted collision.
        let pfs: Vec<_> = m
            .structure_bsps()
            .unwrap()
            .iter()
            .map(|b| {
                let s = m.structure_bsp(b).unwrap();
                let c = &s.collision_bsps().unwrap()[0];
                let h2 = h1convert::collision::convert(c).unwrap();
                pathfind::generate::generate(&h2, 0, 0, &Default::default()).unwrap()
            })
            .collect();
        let mut plan = convert_for_map(&src, &Options::default(), &pfs, None).unwrap();
        let floors: Vec<_> = pfs.iter().map(h1campaign::ai::floors::Floors::new).collect();
        for (zone, enc) in plan.ai.zones.iter_mut().zip(&src.encounters) {
            if enc.flags & 0x20 == 0 {
                floors[zone.manual_bsp as usize]
                    .bind(zone, |_| None)
                    .unwrap();
            }
        }
        let (back, palette) = build_and_read(&plan);
        let mut want = plan.ai.clone();
        want.character_palette = palette;
        assert_eq!(back, want, "{level}: read back differs from the plan");
        assert!(
            back.check_indices().is_empty(),
            "{level}: {:#?}",
            back.check_indices()
        );

        let on_any = |p: [f32; 3], below: f32| pfs.iter().any(|pf| on_sector(pf, p, below, 0.25));
        let (mut fps, mut air) = (0, 0);
        for (z, enc) in back.zones.iter().zip(&src.encounters) {
            let mut authored = enc.firing_positions.clone();
            authored.sort_by_key(|p| p.group);
            assert_eq!(z.firing_positions.len(), authored.len());
            for (f, original) in z.firing_positions.iter().zip(authored) {
                fps += 1;
                if enc.flags & 0x20 != 0 {
                    air += 1;
                    assert_eq!(f.position, original.position);
                    assert_eq!((f.flags, f.sector), (0, u32::MAX));
                } else {
                    use pathfind::geom::{dot, newell, sub};
                    let pf = &pfs[z.manual_bsp as usize];
                    let ring: Vec<_> = pf
                        .sector_polygon(f.sector as usize)
                        .unwrap()
                        .into_iter()
                        .map(|i| pf.vertices[i])
                        .collect();
                    let n = newell(&ring);
                    assert!(dot(n, sub(f.position, ring[0])).abs() <= 0.001 * dot(n, n).sqrt());
                    // For a convex sector, every edge has the point on its interior side or on the edge.
                    let side = n[2].signum();
                    for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
                        let edge = sub(*b, *a);
                        let rel = sub(f.position, *a);
                        assert!(
                            (edge[0] * rel[1] - edge[1] * rel[0]) * side
                                >= -0.001 * dot(edge, edge).sqrt()
                        );
                    }
                }
            }
        }
        if level == "a30" {
            assert_eq!(air, 584);
        }
        let (mut starts, mut starts_off) = (0, Vec::new());
        for s in &back.squads {
            for l in &s.starting_locations {
                starts += 1;
                if !on_any(l.position, 2.0) {
                    starts_off.push(format!("{} {:?}", s.name, l.position));
                }
            }
        }
        println!(
            "{level}: {} groups, {} squads, {starts} starts ({} off sectors), {} zones, {fps} firing positions ({air} flying), {} orders, {} point sets, {} characters",
            back.squad_groups.len(),
            back.squads.len(),
            starts_off.len(),
            back.zones.len(),
            back.orders.len(),
            back.point_sets.len(),
            back.character_palette.len()
        );
        let mut by: BTreeMap<&str, usize> = BTreeMap::new();
        for o in &starts_off {
            *by.entry(o.split(' ').next().unwrap()).or_default() += 1;
        }
        println!("  off sectors by zone or squad: {by:?}");
        for c in &plan.characters {
            println!(
                "  {:<14} {:<60} -> {}",
                c.h1_type.name(),
                c.h1_variant,
                c.h2_character
            );
        }
        for n in &plan.notes {
            println!("  note: {n}");
        }
        // Off a generated sector: standing on scenery (not in the BSP), on
        // a slope steeper than 45 degrees, or in water. Bounded, not zero.
        assert!(
            starts_off.len() * 10 <= starts,
            "{level}: {} of {starts} starting locations off the generated sectors",
            starts_off.len()
        );
    }
}

#[test]
#[ignore = "reads H1_ISO and H2_DISC"]
fn command_script_placement_uses_retail_indices_and_authored_points() {
    let mut retail = h2_map("03a_oldmombasa");
    let scenario = retail.scenario().unwrap().clone();
    let payload = retail.read_tag_bytes(scenario.id).unwrap();
    let view = TagView::new(&payload, scenario.location.unwrap().address);
    let definitions = view.block(&payload, 0x1b8, 0x28).unwrap();
    let retail_ai = ScenarioAi::read(&view).unwrap();
    let mut bound = 0;
    for loc in retail_ai.squads.iter().flat_map(|q| &q.starting_locations) {
        if let Ok(index) = usize::try_from(loc.placement_script_index) {
            let def = definitions[index];
            let end = def[..32].iter().position(|&v| v == 0).unwrap_or(32);
            assert_eq!(loc.placement_script.as_bytes(), &def[..end]);
            assert_eq!(u16::from_le_bytes([def[32], def[33]]), 5);
            bound += 1;
        }
    }
    assert_eq!(bound, 57);

    let mut lists = 0;
    let mut placed = 0;
    for level in CAMPAIGN_LEVELS {
        let m = h1_map(level);
        let src = H1Ai::read(&m).unwrap();
        let root = m.tag_data(m.scenario()).unwrap();
        let first = u32::from_le_bytes(root[0x49c..0x4a0].try_into().unwrap()) as u16;
        let plan = convert(
            &src,
            &Options {
                first_command_script: Some(first),
                ..Default::default()
            },
        )
        .unwrap();
        let (out, _) = build_and_read(&plan);
        assert_eq!(src.command_lists.len(), out.point_sets.len(), "{level}");
        for (list, set) in src.command_lists.iter().zip(&out.point_sets) {
            assert_eq!(list.bsp, set.bsp, "{level}");
            assert_eq!(list.points.len(), set.points.len(), "{level}");
            for (i, (a, b)) in list.points.iter().zip(&set.points).enumerate() {
                assert_eq!(a.position, b.position);
                assert_eq!(a.surface, b.surface);
                assert_eq!(b.name, format!("p{i}"));
            }
            lists += 1;
        }
        let squads: Vec<_> = src.encounters.iter().flat_map(|e| &e.squads).collect();
        assert_eq!(squads.len(), out.squads.len(), "{level}");
        for (src_squad, dst_squad) in squads.into_iter().zip(&out.squads) {
            assert_eq!(src_squad.starting_locations.len(), dst_squad.starting_locations.len());
            for (a, b) in src_squad.starting_locations.iter().zip(&dst_squad.starting_locations) {
                if src.command_lists.get(a.command_list as usize).is_some() {
                    assert_eq!(b.placement_script_index, first as i16 + a.command_list);
                    assert_eq!(b.placement_script, h1campaign::ai::h1::command_script_name(a.command_list as usize));
                    placed += 1;
                } else {
                    assert_eq!(b.placement_script_index, -1);
                    assert!(b.placement_script.is_empty());
                }
            }
        }
    }
    assert_eq!(lists, 552);
    assert!(placed > 0);
}

#[test]
#[ignore = "reads H1_ISO and serializes every playable map's AI"]
fn retail_spawn_bindings_keep_every_actor_weapon_and_command() {
    use std::{fs::File, io::BufReader};
    let path = common::repo_disc("halo1.iso", "H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let (mut maps, mut starts, mut inherited, mut empty, mut commands) = (0, 0, 0, 0, 0);
    for entry in &disc.maps {
        let m = h1map::H1Map::from_bytes(&iso.read(entry).unwrap()).unwrap();
        if m.header().cache_type == h1map::CacheType::UserInterface {
            continue;
        }
        let name = &m.header().name;
        maps += 1;
        let src = H1Ai::read(&m).unwrap();
        let root = m.tag_data(m.scenario()).unwrap();
        let first =
            u16::try_from(u32::from_le_bytes(root[0x49c..0x4a0].try_into().unwrap())).unwrap();
        let options = Options {
            first_command_script: Some(first),
            ..Default::default()
        };
        // Give distinct source weapons distinct slots in the test map's palette.
        let mut palette = Vec::new();
        let weapons: Vec<_> = src
            .actor_palette
            .iter()
            .map(|v| {
                let weapon = v.as_ref()?.weapon.id;
                if weapon.is_none() {
                    return None;
                }
                let index = palette
                    .iter()
                    .position(|&id| id == weapon)
                    .unwrap_or_else(|| {
                        palette.push(weapon);
                        palette.len() - 1
                    });
                Some(i16::try_from(index).unwrap())
            })
            .collect();
        let plan = convert_for_map(&src, &options, &[], Some(&weapons))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let (back, _) = build_and_read(&plan);
        let squads: Vec<_> = src.encounters.iter().flat_map(|e| &e.squads).collect();
        assert_eq!(squads.len(), back.squads.len(), "{name}");
        assert_eq!(squads.len(), plan.squad_voices.len(), "{name}");
        for ((source, output), voices) in
            squads.into_iter().zip(back.squads).zip(&plan.squad_voices)
        {
            assert_eq!(
                voices.locations.len(),
                source.starting_locations.len(),
                "{name}"
            );
            assert_eq!(
                source.starting_locations.len(),
                output.starting_locations.len(),
                "{name}"
            );
            let effective = |start, squad| if start == -1 { squad } else { start };
            for ((a, b), voice) in source
                .starting_locations
                .iter()
                .zip(output.starting_locations)
                .zip(&voices.locations)
            {
                starts += 1;
                inherited += usize::from(a.actor == -1);
                let actor = effective(a.actor, source.actor);
                let variant = src
                    .actor_palette
                    .get(actor as usize)
                    .and_then(Option::as_ref);
                empty += usize::from(variant.is_none());
                let source_bank = variant.and_then(|v| {
                    src.voice_definitions
                        .iter()
                        .find(|bank| bank.tag == v.unit.id)
                });
                assert_eq!(
                    voice.map(|i| &plan.voice_definitions[usize::from(i)]),
                    source_bank,
                    "{name}"
                );
                let character = effective(b.character, output.character);
                let mapped = (character != -1).then(|| &plan.characters[character as usize]);
                assert_eq!(
                    mapped.map(|c| &c.h2_character),
                    variant.map(|v| &v.name),
                    "{name}"
                );
                let weapon = effective(b.initial_weapon, output.initial_weapon);
                assert_eq!(
                    (weapon != -1).then(|| palette[weapon as usize]),
                    variant.map(|v| v.weapon.id).filter(|id| !id.is_none()),
                    "{name}"
                );
                if a.command_list == -1 {
                    assert_eq!(b.placement_script_index, -1, "{name}");
                    assert!(b.placement_script.is_empty(), "{name}");
                } else {
                    commands += 1;
                    assert_eq!(
                        b.placement_script_index,
                        first as i16 + a.command_list,
                        "{name}"
                    );
                    assert_eq!(
                        b.placement_script,
                        h1campaign::ai::h1::command_script_name(a.command_list as usize)
                    );
                }
            }
        }
    }
    assert_eq!(
        (maps, starts, inherited, empty, commands),
        (23, 10_129, 8_206, 112, 1_614)
    );
}
