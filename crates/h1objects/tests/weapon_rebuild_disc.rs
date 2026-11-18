//! Weapons, equipment, projectiles and damage effects built from their
//! definitions (`h1objects::arsenal`) against the discs. Reads `$H1_ISO`
//! and `$H2_DISC`; nothing is written. Prints counts, names and paths only.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test weapon_rebuild_disc -- --ignored --nocapture
//! ```
//!
//! - `halo_1_and_halo_2_pairs_agree_on_the_named_fields`: the evidence for
//!   the names in `weap`, `eqip`, `proj` and `jpt!`
//!   (docs/formats/tag-definitions.md): on Blood Gulch's arsenal against
//!   Coagulation's counterparts, the fields hold Halo 1's values.
//! - `every_weapon_equipment_and_projectile_rebuilds`: every `weap`,
//!   `eqip` and `proj` on the ten campaign levels (on 05a_deltaapproach)
//!   and the multiplayer maps (on their bases) converts; every `weap`,
//!   `eqip`, `proj` and `jpt!` the converter writes reads back through its
//!   definition and writes the same again, has no template bytes (no raw,
//!   no pinned block), and holds Halo 1's values in the fields it carries;
//!   every reference is a converted tag or a Halo 2 one of the right group,
//!   and the Halo 2 ones are listed with why.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1objects::Converted;
use h1objects::arsenal::{self, H1_DAMAGE_LOWER, H1_DAMAGE_UPPER};
use h1objects::base::Base;
use h1objects::convert::{Converter, Tree, template_candidates};
use h1objects::h1;
use map::{FourCC, Map, TagId};
use tags::defs::{self, Defs, Record, StructDef, Target, Val};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn h2_map(want: &str) -> Option<Map<Cursor<Vec<u8>>>> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H2_DISC", "halo2.iso")).unwrap(),
    ))
    .unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{want}.map")))?;
    Some(Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap())
}

fn h1_level(name: &str) -> Option<H1Map> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name)?.clone();
    Some(H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap())
}

fn def(g: &[u8; 4]) -> &'static StructDef {
    Defs::builtin().group(FourCC(*g)).unwrap()
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn i16_at(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn halo_1_and_halo_2_pairs_agree_on_the_named_fields() {
    let mut m = h2_map("coagulation").unwrap();
    let base = Base::read(&mut m).unwrap();
    let h1 = h1_level("bloodgulch").unwrap();
    let mut agree: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut tally = |k: &'static str, ok: bool| {
        let e = agree.entry(k).or_default();
        e.0 += usize::from(ok);
        e.1 += 1;
    };
    for t in h1.tags() {
        let g = t.group.0.to_be_bytes();
        if &g != b"weap" && &g != b"eqip" {
            continue;
        }
        let Some(tpl) = template_candidates(&g, &t.name)
            .into_iter()
            .find_map(|n| base.find(&g, n))
        else {
            continue;
        };
        let r2 = base.head(tpl, def(&g).size.into()).unwrap();
        let pair = format!("{} / {}", t.name, base.name(tpl).unwrap().1);
        if &g == b"eqip" {
            let e = h1::Equipment::read(&h1, t).unwrap();
            if e.powerup_type == 0 || e.powerup_type == 5 {
                continue; // ammunition, and the health pack (Halo 2 has none)
            }
            tally("eqip powerup_type", i16_at(&r2, 0x12c) == e.powerup_type);
            tally("eqip grenade_type", i16_at(&r2, 0x12e) == e.grenade_type);
            tally("eqip powerup_time", f32_at(&r2, 0x130) == e.powerup_time);
            println!("{pair}: powerup {} grenade {}", e.powerup_type, e.grenade_type);
            continue;
        }
        let w = h1::Weapon::read(&h1, t).unwrap();
        if let (Some(m1), Some(m2)) = (w.magazines.first(), base.element(&r2, 0x2c0, 0x5c, 0).unwrap()) {
            tally("magazine rounds_loaded_maximum", i16_at(&m2, 0xa) == m1.rounds_loaded_maximum);
            tally("magazine rounds_reloaded", i16_at(&m2, 0x14) == m1.rounds_reloaded);
            tally("magazine rounds_total_initial", i16_at(&m2, 0x6) == m1.rounds_total_initial);
            tally("magazine rounds_total_maximum", i16_at(&m2, 0x8) == m1.rounds_total_maximum);
            tally("magazine reload_time", f32_at(&m2, 0x10) == m1.reload_time);
        }
        for (i, tr) in w.triggers.iter().enumerate() {
            let Some(b2) = base.element(&r2, 0x2d0, 0xec, i).unwrap() else {
                continue;
            };
            tally("barrel firing_noise", i16_at(&b2, 0x36) == tr.firing_noise);
            let (Some(p1), Some(p2)) = (tr.projectile, base.reference(&b2, 0x8c)) else {
                continue;
            };
            let pt = h1.tag(p1).unwrap();
            let p = h1::Projectile::read(&h1, pt).unwrap();
            let pr2 = base.head(p2, def(b"proj").size.into()).unwrap();
            tally("proj collision_radius", f32_at(&pr2, 0xc8) == p.collision_radius);
            tally("proj maximum_range", f32_at(&pr2, 0xe0) == p.maximum_range);
            // Damage: the same order (lower, upper from, upper to) in both.
            if let (Some(j1), Some(j2)) = (p.impact_damage, base.reference(&pr2, 0x148)) {
                let b1 = h1.tag_data(h1.tag(j1).unwrap()).unwrap();
                let b2 = base.head(j2, 0xc8).unwrap();
                let order = |lo: f32, a: f32, b: f32| lo > 0.0 && lo <= a && a <= b;
                tally(
                    "jpt! damage bounds in order in both",
                    order(f32_at(b1, H1_DAMAGE_LOWER), f32_at(b1, H1_DAMAGE_UPPER), f32_at(b1, H1_DAMAGE_UPPER + 4))
                        && order(f32_at(&b2, 0x1c), f32_at(&b2, 0x20), f32_at(&b2, 0x24)),
                );
            }
        }
    }
    for (k, (ok, n)) in &agree {
        println!("{k}: {ok} of {n} pairs");
    }
    let at_least = |k: &str, want: usize| {
        let (ok, _) = agree[k];
        assert!(ok >= want, "{k}: {ok} pairs agree, want {want}");
    };
    at_least("magazine rounds_loaded_maximum", 5);
    at_least("magazine rounds_reloaded", 4);
    at_least("magazine reload_time", 2);
    at_least("barrel firing_noise", 8);
    at_least("proj maximum_range", 3);
    at_least("proj collision_radius", 2);
    at_least("jpt! damage bounds in order in both", 5);
    for k in ["eqip powerup_type", "eqip grenade_type", "eqip powerup_time"] {
        let (ok, n) = agree[k];
        assert_eq!(ok, n, "{k}");
        assert!(n >= 4, "{k}: over shield, camouflage and both grenades");
    }
}

/// The tree's tag a local datum names.
fn local(tree: &Tree, id: TagId) -> Option<&Converted> {
    if id.0 & 0xffff_0000 != h1objects::convert::local(0).0 {
        return None;
    }
    tree.tags.get(id.0 as usize & 0xffff).map(|t| &t.1)
}

/// Every reference in `r`: (path, group word, target).
fn targets(d: &StructDef, r: &Record, path: &str, out: &mut Vec<(String, u32, TagId)>, pinned: &mut Vec<String>) {
    for (f, (_, v)) in d.fields.iter().zip(&r.fields) {
        let p = format!("{path}.{}", f.name);
        match (&f.kind, v) {
            (_, Val::Unresolved { .. }) => pinned.push(p),
            (_, Val::Ref { group, target: Target::Id(id) }) => out.push((p, *group, *id)),
            (_, Val::Datum(Target::Id(id))) => out.push((p, 0, *id)),
            (defs::Kind::Block(s), Val::Block(e)) => {
                for (i, x) in e.iter().enumerate() {
                    targets(s, x, &format!("{p}[{i}]"), out, pinned);
                }
            }
            (defs::Kind::Struct(s), Val::Struct(x)) => targets(s, x, &p, out, pinned),
            _ => {}
        }
    }
}

fn float(r: &Record, path: &str) -> f32 {
    match r.at(path) {
        Some(Val::Float(x)) => *x,
        v => panic!("{path}: {v:?}"),
    }
}

fn int(r: &Record, path: &str) -> i64 {
    match r.at(path) {
        Some(Val::Int(x)) => *x,
        v => panic!("{path}: {v:?}"),
    }
}

fn reference(r: &Record, path: &str) -> Option<TagId> {
    match r.at(path)? {
        Val::Ref { target: Target::Id(id), .. } => Some(*id),
        _ => None,
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_weapon_equipment_and_projectile_rebuilds() {
    let only = std::env::var("H1_LEVELS").ok();
    let mut levels: Vec<(String, String)> = h1map::disc::CAMPAIGN_LEVELS
        .iter()
        .map(|l| (l.to_string(), "05a_deltaapproach".to_string()))
        .collect();
    levels.extend(
        h1convert::convert::MAPS
            .iter()
            .map(|(m, _, b)| (m.to_string(), b.to_string())),
    );
    let mut bases: HashMap<String, Option<Base>> = HashMap::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut halo2: BTreeMap<String, usize> = BTreeMap::new();
    let mut maps = 0;
    for (level, base_name) in &levels {
        if only.as_deref().is_some_and(|o| !o.split(',').any(|x| x == level)) {
            continue;
        }
        let Some(h1) = h1_level(level) else {
            println!("{level}: not on the disc");
            continue;
        };
        let base = bases
            .entry(base_name.clone())
            .or_insert_with(|| h2_map(base_name).map(|mut m| Base::read(&mut m).unwrap()));
        let Some(base) = base.as_ref() else {
            println!("{level}: base {base_name} not on the disc");
            continue;
        };
        maps += 1;
        let mut c = Converter::new(&h1, base);
        c.options.rebuild_items = true;
        let mut sources: HashMap<TagId, &h1map::Tag> = HashMap::new();
        for t in h1.tags() {
            let g = t.group.0.to_be_bytes();
            if !matches!(&g, b"weap" | b"eqip" | b"proj") {
                continue;
            }
            let id = c
                .object(t)
                .unwrap_or_else(|e| panic!("{level} {}: {e}", t.name));
            sources.insert(id, t);
        }
        // Every arsenal tag in the tree, with what it was made from.
        for (k, (name, conv)) in c.tree.tags.iter().enumerate() {
            let g = conv.group.0;
            if !matches!(&g, b"weap" | b"eqip" | b"proj" | b"jpt!") {
                continue;
            }
            let id = h1objects::convert::local(k);
            let d = def(&g);
            assert!(conv.raw.is_empty(), "{level} {name}: raw");
            let r = defs::from_built(d, &conv.root)
                .unwrap_or_else(|e| panic!("{level} {name}: reads: {e}"));
            let again = defs::to_built(d, &r, &mut |_| None).unwrap();
            assert_eq!(defs::from_built(d, &again).unwrap(), r, "{level} {name}: writes the same");
            let mut refs = Vec::new();
            let mut pinned = Vec::new();
            targets(d, &r, &String::from_utf8_lossy(&g), &mut refs, &mut pinned);
            assert!(pinned.is_empty(), "{level} {name}: pinned {pinned:?}");
            for (p, word, t) in &refs {
                match local(&c.tree, *t) {
                    Some(x) => {
                        if *word != 0 && *word != u32::MAX {
                            assert_eq!(*word, x.group.as_u32(), "{level} {name} {p}");
                        }
                    }
                    None => {
                        let (bg, bn) = base
                            .name(*t)
                            .unwrap_or_else(|| panic!("{level} {name} {p}: {t:?} is in neither the tree nor the base"));
                        if *word != 0 {
                            assert_eq!(*word, bg.as_u32(), "{level} {name} {p}");
                        }
                        // Halo 2 references: the slots Halo 1 has nothing
                        // converted for (its wphi HUDs, effe effects and
                        // sounds have no converter; Halo 2's melee damage
                        // and pickup sounds), from the counterpart.
                        let slot = p
                            .split('.')
                            .skip(1)
                            .map(|s| s.split('[').next().unwrap())
                            .collect::<Vec<_>>()
                            .join(".");
                        *halo2.entry(format!("{} {slot} -> {bg}", String::from_utf8_lossy(&g))).or_default() += 1;
                        let _ = bn;
                    }
                }
            }
            *counts.entry(String::from_utf8_lossy(&g).into_owned()).or_default() += 1;

            // Known fields hold Halo 1's values.
            let src = sources.get(&id);
            match &g {
                b"weap" => {
                    let t = src.expect("weapons are converted directly");
                    let w = h1::Weapon::read(&h1, t).unwrap();
                    assert_eq!(r.at("item.obje.object_type"), Some(&Val::Enum(arsenal::WEAPON_TYPE)));
                    assert_eq!(float(&r, "item.obje.bounding_radius"), w.object.bounding_radius);
                    let Some(Val::Block(mags)) = r.at("magazines") else { panic!() };
                    assert_eq!(mags.len(), w.magazines.len(), "{name}");
                    for (m, h) in mags.iter().zip(&w.magazines) {
                        assert_eq!(int(m, "rounds_loaded_maximum"), i64::from(h.rounds_loaded_maximum));
                        assert_eq!(int(m, "rounds_total_maximum"), i64::from(h.rounds_total_maximum));
                        assert_eq!(int(m, "rounds_reloaded"), i64::from(h.rounds_reloaded));
                        assert_eq!(float(m, "reload_time"), h.reload_time);
                    }
                    let Some(Val::Block(barrels)) = r.at("barrels") else { panic!() };
                    assert_eq!(barrels.len(), w.triggers.len(), "{name}");
                    for (b, t) in barrels.iter().zip(&w.triggers) {
                        assert_eq!(int(b, "firing_noise"), i64::from(t.firing_noise));
                        let p = reference(b, "projectile");
                        assert_eq!(p.is_some(), t.projectile.is_some(), "{name}: projectile");
                        if let Some(p) = p {
                            assert_eq!(local(&c.tree, p).map(|x| x.group), Some(FourCC(*b"proj")), "{name}: Halo 1's projectile");
                        }
                    }
                    let model = reference(&r, "item.obje.model");
                    assert_eq!(model.and_then(|m| local(&c.tree, m)).map(|x| x.group), w.object.model.map(|_| FourCC(*b"hlmt")));
                    let Some(Val::Block(fp)) = r.at("first_person") else { panic!() };
                    if w.first_person_model.is_some() {
                        assert_eq!(fp.len(), 2, "{name}");
                        let m = reference(&fp[0], "model").expect("first-person model");
                        assert_eq!(local(&c.tree, m).map(|x| x.group), Some(FourCC(*b"mode")), "{name}: Halo 1's first-person model");
                        if let Some(a) = reference(&fp[0], "animations") {
                            assert_eq!(local(&c.tree, a).map(|x| x.group), Some(FourCC(*b"jmad")));
                            *counts.entry("weap first-person animations converted".into()).or_default() += 1;
                        }
                    }
                }
                b"eqip" => {
                    let t = src.expect("equipment is converted directly");
                    let e = h1::Equipment::read(&h1, t).unwrap();
                    assert_eq!(int(&r, "powerup_type"), i64::from(e.powerup_type), "{name}");
                    assert_eq!(int(&r, "grenade_type"), i64::from(e.grenade_type), "{name}");
                    if e.powerup_type != 5 {
                        assert_eq!(float(&r, "powerup_time"), e.powerup_time, "{name}");
                    }
                    if matches!(e.powerup_type, 2 | 3) {
                        *counts.entry(format!("eqip powerup {}", e.powerup_type)).or_default() += 1;
                    }
                }
                b"proj" => {
                    let t = h1.find(h1map::Group::new(b"proj"), name).expect("Halo 1 projectile");
                    let p = h1::Projectile::read(&h1, t).unwrap();
                    assert_eq!(float(&r, "collision_radius"), p.collision_radius, "{name}");
                    assert_eq!(float(&r, "maximum_range"), p.maximum_range, "{name}");
                    let words = defs::to_built(d, &r, &mut |_| None).unwrap().bytes;
                    assert_eq!(f32_at(&words, 0x17c), p.initial_velocity * 30.0, "{name}: per second");
                    let imp = reference(&r, "impact_damage");
                    assert_eq!(imp.is_some(), p.impact_damage.is_some(), "{name}: impact damage");
                    for j in [imp, reference(&r, "detonation_damage")].into_iter().flatten() {
                        assert_eq!(local(&c.tree, j).map(|x| x.group), Some(FourCC(*b"jpt!")), "{name}: Halo 1's damage");
                    }
                }
                b"jpt!" => {
                    let t = h1.find(h1map::Group::new(b"jpt!"), name).expect("Halo 1 damage");
                    let b = h1.tag_data(t).unwrap();
                    assert_eq!(float(&r, "damage_lower_bound"), f32_at(b, H1_DAMAGE_LOWER), "{name}");
                    assert_eq!(
                        r.at("damage_upper_bound"),
                        Some(&Val::floats(&[f32_at(b, H1_DAMAGE_UPPER), f32_at(b, H1_DAMAGE_UPPER + 4)])),
                        "{name}"
                    );
                }
                _ => unreachable!(),
            }
        }
        println!("{level} on {base_name}: ok");
    }
    println!("{maps} maps");
    for (k, n) in &counts {
        println!("{k}: {n}");
    }
    println!("references from Halo 2 (slot -> group: tags):");
    for (k, n) in &halo2 {
        println!("  {k}: {n}");
    }
    if only.is_none() {
        assert!(maps >= 20, "the ten levels and the multiplayer maps");
        assert!(counts.get("eqip powerup 2").is_some() && counts.get("eqip powerup 3").is_some(), "over shield and camouflage rebuilt");
    }
}
