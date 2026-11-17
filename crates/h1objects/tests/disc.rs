//! Real-data proof: every object tag on every Halo 1 multiplayer map reads,
//! and Blood Gulch's sandbox converts onto Coagulation and reads back
//! through `crates/tags`. Reads `$H1_ISO` (the Halo 1 disc) and `$H2_DISC`
//! (the Halo 2 disc) at run time; prints counts, names and tuning values
//! only. Nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test -p h1objects --test disc -- --ignored --nocapture
//! ```

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::{Group, H1Map};
use h1objects::base::Base;
use h1objects::convert::{Converter, local};
use h1objects::h1::{Collision, H1Object, OBJECT_TYPES, Physics};
use h1objects::refs;
use map::build::TagSet;
use map::{FourCC, Map, TagId};

fn h1_disc() -> (xiso::Xiso<BufReader<File>>, h1map::disc::Disc) {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    (iso, disc)
}

fn h1_map(
    iso: &mut xiso::Xiso<BufReader<File>>,
    disc: &h1map::disc::Disc,
    name: &str,
) -> Option<H1Map> {
    let e = disc.map(name)?.clone();
    Some(H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap())
}

fn h2_map(name: &str) -> Map<Cursor<Vec<u8>>> {
    let path = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{name}.map")))
        .unwrap();
    Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn every_object_tag_on_every_multiplayer_map_reads() {
    let (mut iso, disc) = h1_disc();
    let mut failures = Vec::new();
    let mut total = std::collections::BTreeMap::<String, usize>::new();
    let mut maps = 0;
    for (name, _, _) in h1convert::convert::MAPS {
        let Some(map) = h1_map(&mut iso, &disc, name) else {
            continue;
        };
        maps += 1;
        for tag in map.tags() {
            let g = tag.group.0.to_be_bytes();
            let r: Result<(), String> = match &g {
                b"weap" | b"proj" | b"eqip" | b"bipd" | b"vehi" => H1Object::read(&map, tag)
                    .map_err(|e| e.to_string())
                    .and_then(|o| {
                        let o = o.object();
                        let want = OBJECT_TYPES.iter().position(|t| **t == g).unwrap() as u16;
                        if o.object_type != want {
                            return Err(format!("object type {} for {}", o.object_type, tag.group));
                        }
                        // Every typed model-chain reference is of its group.
                        for (id, grp) in [
                            (o.model, b"mode"),
                            (o.animation, b"antr"),
                            (o.collision, b"coll"),
                            (o.physics, b"phys"),
                        ] {
                            if let Some(id) = id
                                && map.tag(id).unwrap().group != Group::new(grp)
                            {
                                return Err(format!(
                                    "{} is not a {}",
                                    map.tag(id).unwrap().name,
                                    String::from_utf8_lossy(grp)
                                ));
                            }
                        }
                        if !o.bounding_radius.is_finite() || o.bounding_radius < 0.0 {
                            return Err("bounding radius".into());
                        }
                        Ok(())
                    }),
                b"coll" => Collision::read(&map, tag)
                    .map_err(|e| e.to_string())
                    .and_then(|c| {
                        for n in &c.nodes {
                            let perms = if n.region < 0 {
                                0
                            } else {
                                c.regions
                                    .get(n.region as usize)
                                    .ok_or("node region out of range")?
                                    .permutations
                                    .len()
                            };
                            if n.bsps.len() > perms.max(1) {
                                return Err(format!(
                                    "node {} has {} BSPs for {perms} permutations",
                                    n.name,
                                    n.bsps.len()
                                ));
                            }
                            for b in &n.bsps {
                                let v = b.validate();
                                if !v.is_empty() {
                                    return Err(format!("node {} BSP: {}", n.name, v[0]));
                                }
                            }
                        }
                        if c.spheres.iter().any(|s| {
                            (s.node >= 0 && s.node as usize >= c.nodes.len())
                                || s.radius.is_nan()
                                || s.radius < 0.0
                        }) {
                            return Err("pathfinding sphere".into());
                        }
                        Ok(())
                    }),
                b"phys" => Physics::read(&map, tag)
                    .map_err(|e| e.to_string())
                    .and_then(|p| {
                        if p.mass.is_nan()
                            || p.mass <= 0.0
                            || p.mass_points.iter().any(|m| {
                                m.radius.is_nan()
                                    || m.radius < 0.0
                                    || !m.position.iter().all(|v| v.is_finite())
                            })
                        {
                            return Err("mass or mass point".into());
                        }
                        Ok(())
                    }),
                b"effe" | b"jpt!" | b"snd!" | b"cont" | b"ligh" => {
                    let _ = refs::scan(&map, map.tag_data(tag).unwrap_or(&[]));
                    Ok(())
                }
                _ => continue,
            };
            *total.entry(tag.group.to_string()).or_default() += 1;
            if let Err(e) = r {
                failures.push(format!("{name}: {} {}: {e}", tag.group, tag.name));
            }
        }
    }
    eprintln!("{maps} maps: {total:?}");
    for f in &failures {
        eprintln!("FAIL {f}");
    }
    assert!(maps > 0);
    assert!(failures.is_empty(), "{} failures", failures.len());
}

/// The Halo 1 tags of the proof set, by group and words in the path.
const PROOF: &[(&[u8; 4], &[&str])] = &[
    (b"weap", &["assault rifle"]),
    (b"weap", &["pistol\\pistol"]),
    (b"weap", &["plasma rifle"]),
    (b"weap", &["needler"]),
    (b"eqip", &["frag grenade"]),
    (b"proj", &["frag grenade"]),
    (b"eqip", &["plasma grenade"]),
    (b"proj", &["plasma grenade"]),
    (b"eqip", &["health"]),
    (b"bipd", &["cyborg"]),
    (b"vehi", &["warthog"]),
    (b"vehi", &["ghost"]),
];

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn bloodgulch_sandbox_converts_and_reads_back() {
    let (mut iso, disc) = h1_disc();
    let h1 = h1_map(&mut iso, &disc, "bloodgulch").expect("bloodgulch on the disc");
    let mut base_map = h2_map("coagulation");
    let base = Base::read(&mut base_map).unwrap();
    let mut conv = Converter::new(&h1, &base);
    let mut wanted = Vec::new();
    for (g, words) in PROOF {
        let tag = h1
            .tags()
            .iter()
            .find(|t| t.group == Group::new(g) && words.iter().all(|w| t.name.contains(w)))
            .unwrap_or_else(|| panic!("no {words:?}"));
        let id = conv
            .object(tag)
            .unwrap_or_else(|e| panic!("{}: {e}", tag.name));
        wanted.push((tag.name.clone(), FourCC(**g), id));
    }
    let tree = conv.tree;
    let reports = tree.reports.clone();
    let count = tree.tags.len();
    let mut set = TagSet::from_map(&mut base_map).unwrap();
    // A pinned index cannot grow, so the tree takes over slots the
    // converted map does not need: Coagulation's own level bitmaps, which no
    // object references.
    let donors: Vec<TagId> = base_map
        .tags()
        .iter()
        .filter(|t| {
            t.group == FourCC(*b"bitm")
                && t.name
                    .as_deref()
                    .is_some_and(|n| n.starts_with("scenarios\\"))
        })
        .map(|t| t.id)
        .collect();
    eprintln!("{} donor slots for {count} tags", donors.len());
    let ids = tree.install_over(&mut set, &donors).unwrap();
    let (bytes, _) = set.build().unwrap();
    eprintln!("{count} tags installed; map {} bytes", bytes.len());
    let mut out = Map::from_reader(Cursor::new(bytes)).unwrap();
    let ctx = tags::object::Context::read(&mut out).unwrap();

    // Every reference word pair in every new tag's payload names a tag of
    // that group in the map, and no placeholder is left.
    let tags_by_id: std::collections::HashMap<u32, FourCC> =
        out.tags().iter().map(|t| (t.id.0, t.group)).collect();
    let groups: std::collections::HashSet<u32> = out
        .tags()
        .iter()
        .map(|t| u32::from_be_bytes(t.group.0))
        .collect();
    let mut live = 0;
    for id in &ids {
        let b = out.read_tag_bytes(*id).unwrap();
        for at in (0..b.len().saturating_sub(7)).step_by(4) {
            let w = u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
            let d = u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap());
            assert!(d & 0xffff_0000 != local(0).0, "placeholder left at {at:#x}");
            if groups.contains(&w) && d != u32::MAX && d != 0 {
                let g = tags_by_id.get(&d).unwrap_or_else(|| {
                    panic!("{id:?} +{at:#x}: reference to a missing tag {d:08x}")
                });
                let code = |g: Option<FourCC>| g.map(|g| u32::from_be_bytes(g.0));
                let fits = u32::from_be_bytes(g.0) == w
                    || out
                        .groups()
                        .iter()
                        .find(|x| x.class == *g)
                        .is_some_and(|x| {
                            code(x.parent) == Some(w) || code(x.grandparent) == Some(w)
                        });
                assert!(
                    fits,
                    "{id:?} +{at:#x}: a {} reference to a {g}",
                    FourCC(w.to_be_bytes())
                );
                live += 1;
            }
        }
    }
    eprintln!("{live} live references in the new tags' payloads, all resolving");

    eprintln!("| Halo 1 object | Halo 2 template | mode | coll | jmad | phmo |");
    for (name, group, local_id) in &wanted {
        let id = ids[(local_id.0 & 0xffff) as usize];
        let o = ctx
            .resolve_object(id)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(o.tag.group, *group);
        let m = o
            .model
            .as_ref()
            .unwrap_or_else(|| panic!("{name}: no model"));
        let link = |l: Option<(FourCC, Option<String>)>, g: &[u8; 4]| match l {
            Some((group, n)) => {
                assert_eq!(group, FourCC(*g), "{name}");
                n.unwrap_or_default()
            }
            None => "-".to_string(),
        };
        let mode = link(
            m.render_model.as_ref().map(|l| (l.group, l.name.clone())),
            b"mode",
        );
        let coll = link(
            m.collision_model
                .as_ref()
                .map(|l| (l.group, l.name.clone())),
            b"coll",
        );
        let jmad = link(
            m.animation.as_ref().map(|l| (l.group, l.name.clone())),
            b"jmad",
        );
        let phmo = link(
            m.physics_model.as_ref().map(|l| (l.group, l.name.clone())),
            b"phmo",
        );
        assert_ne!(mode, "-", "{name}: no render model");
        if *group == FourCC(*b"bipd") || *group == FourCC(*b"vehi") {
            assert_ne!(jmad, "-", "{name}: no animation");
            assert_ne!(coll, "-", "{name}: no collision");
        }
        if *group == FourCC(*b"vehi") {
            assert_ne!(phmo, "-", "{name}: no physics model");
        }
        if coll != "-" {
            let c =
                tags::collision::Collision::read(&mut out, m.collision_model.as_ref().unwrap().id)
                    .unwrap();
            let v = c.validate();
            assert!(v.is_empty(), "{name} coll: {v:?}");
        }
        let r = reports.iter().find(|r| &r.source == name).unwrap();
        eprintln!(
            "| {name} | {} | {mode} | {coll} | {jmad} | {phmo} |",
            r.template
        );
    }
    for r in &reports {
        eprintln!(
            "\n## {} {} on {} (physics: {:?})",
            r.group, r.source, r.template, r.physics
        );
        for t in &r.tuning {
            eprintln!(
                "| {} | {} | {} | {} |",
                t.field, t.halo1, t.halo2, t.template
            );
        }
        for (at, g, n) in &r.stopgaps {
            eprintln!("  stopgap {at:#x} {g} {n}");
        }
        for n in &r.no_home {
            eprintln!("  no home: {n}");
        }
    }
    let _ = TagId::NULL;
}
