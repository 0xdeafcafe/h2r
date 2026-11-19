//! What a converted Halo 1 weapon plays when it fires and reloads, end to
//! end, against the discs. Reads `$H1_ISO` and `$H2_DISC`; nothing is
//! written. Prints tag paths only.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test weapon_sounds_disc -- --ignored --nocapture
//! ```
//!
//! - `halo1_firing_effect_layout`: every trigger firing effect on every
//!   Halo 1 map holds references only at `h1::FIRING_EFFECT_EFFECTS`
//!   (`effe`) and `h1::FIRING_EFFECT_DAMAGE` (`jpt!`).
//! - `halo2_firing_effect_groups`: on Coagulation every retail barrel firing
//!   effect names `effe` or `snd!` in `arsenal::FIRING_EFFECT_REFS` and
//!   `jpt!` in `arsenal::FIRING_DAMAGE_REFS`; magazine `+0x34` names `effe`.
//! - `every_weapon_fires_its_halo1_sounds`: Blood Gulch (`$H1_MAPS`, comma
//!   separated, picks other multiplayer maps) built fresh in the Halo 1
//!   sandbox, as `h2 serve` plays it. Every Halo 1 weapon in the built map:
//!   each barrel's firing effects are Halo 1's (by path), and every sound
//!   part of each is the Halo 1 sound its Halo 1 effect plays (by path) and
//!   is in the map; the same for the magazines' reloading effects. Before
//!   converter version 84 the effects were the Halo 2 counterpart's (the
//!   assault rifle fired the SMG's), and weapons with no counterpart (the
//!   vehicle guns) had none.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1convert::convert::halo1::prepare_halo1;
use h1convert::convert::structure_mopp;
use h1map::{Group, H1Map};
use h1objects::arsenal::{FIRING_DAMAGE_REFS, FIRING_EFFECT_REFS, MAGAZINE_RELOADING_EFFECT};
use h1objects::base::Base;
use h1objects::h1::{self, FIRING_EFFECT_DAMAGE, FIRING_EFFECT_EFFECTS, FIRING_EFFECT_SIZE};
use h1objects::sandbox::Objects;
use map::{FourCC, Map, TagId};
use tags::defs::{self, Defs, Record, StructDef, Target, Val};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn h2_map(want: &str) -> Map<Cursor<Vec<u8>>> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H2_DISC", "halo2.iso")).unwrap(),
    ))
    .unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{want}.map")))
        .unwrap();
    Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

/// Every Halo 1 map on the disc, by name.
fn h1_maps() -> Vec<(String, H1Map)> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let names: Vec<String> = h1map::disc::CAMPAIGN_LEVELS
        .iter()
        .map(|s| s.to_string())
        .chain(h1convert::convert::MAPS.iter().map(|m| m.0.to_string()))
        .collect();
    names
        .into_iter()
        .filter_map(|n| {
            let e = d.map(&n)?.clone();
            Some((
                n,
                H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap(),
            ))
        })
        .collect()
}

fn h1_map(name: &str) -> Option<H1Map> {
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

fn block<'r>(r: &'r Record, path: &str) -> &'r [Record] {
    match r.at(path) {
        Some(Val::Block(b)) => b,
        x => panic!("{path}: {x:?}"),
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn halo1_firing_effect_layout() {
    let (mut elements, mut effects, mut damage) = (0, 0, 0);
    for (name, h1) in h1_maps() {
        let effe = Group::new(b"effe");
        let jpt = Group::new(b"jpt!");
        for t in h1.tags() {
            if t.group != Group::new(b"weap") {
                continue;
            }
            let b = h1map::Block::read(h1.tag_data(t).unwrap(), 0x4fc).unwrap();
            let trig = h1.block(b, h1::TRIGGER_SIZE).unwrap().to_vec();
            for tr in trig.chunks_exact(h1::TRIGGER_SIZE) {
                let fb = h1map::Block::read(tr, 0x108).unwrap();
                let fx = h1.block(fb, FIRING_EFFECT_SIZE).unwrap();
                for e in fx.chunks_exact(FIRING_EFFECT_SIZE) {
                    elements += 1;
                    let found = h1objects::refs::scan(&h1, e);
                    for f in &found {
                        let ok = (FIRING_EFFECT_EFFECTS.contains(&f.offset) && f.group == effe)
                            || (FIRING_EFFECT_DAMAGE.contains(&f.offset) && f.group == jpt);
                        assert!(ok, "{name} {}: {} at {:#x}", t.name, f.group, f.offset);
                        if f.tag.is_some() {
                            if f.group == effe {
                                effects += 1;
                            } else {
                                damage += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    println!("{elements} firing effects: {effects} effects and {damage} damage named");
    assert!(elements > 0 && effects > 0);
}

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn halo2_firing_effect_groups() {
    let mut m = h2_map("coagulation");
    let base = Base::read(&mut m).unwrap();
    let (effe, snd, jpt) = (FourCC(*b"effe"), FourCC(*b"snd!"), FourCC(*b"jpt!"));
    let (mut elements, mut named, mut reloading) = (0, 0, 0);
    for t in m.tags().to_vec() {
        if t.group != FourCC(*b"weap") || t.location.is_none() {
            continue;
        }
        let Ok(r) = base.record(def(b"weap"), t.id) else {
            continue;
        };
        let name = t.name.clone().unwrap_or_default();
        let group = |x: &Record, p: &str| match x.at(p) {
            Some(Val::Ref {
                target: Target::Id(id),
                ..
            }) => base.name(*id).map(|n| n.0),
            _ => None,
        };
        for b in block(&r, "barrels") {
            for e in block(b, "block_e4") {
                elements += 1;
                for p in FIRING_EFFECT_REFS {
                    if let Some(g) = group(e, p) {
                        assert!(g == effe || g == snd, "{name} {p}: {g}");
                        named += 1;
                    }
                }
                for p in FIRING_DAMAGE_REFS {
                    if let Some(g) = group(e, p) {
                        assert_eq!(g, jpt, "{name} {p}");
                    }
                }
            }
        }
        for mag in block(&r, "magazines") {
            if let Some(g) = group(mag, MAGAZINE_RELOADING_EFFECT) {
                assert_eq!(g, effe, "{name} reloading effect");
                reloading += 1;
            }
        }
    }
    println!(
        "{elements} retail firing effects, {named} effects or sounds named; {reloading} reloading effects"
    );
    assert!(elements > 0 && named > 0);
}

/// The Halo 1 sounds an effect's parts play, by path.
fn halo1_sounds(h1: &H1Map, effect: h1map::TagId) -> BTreeSet<String> {
    let e = h1::Effect::read(h1, h1.tag(effect).unwrap()).unwrap();
    e.events
        .iter()
        .flat_map(|ev| &ev.parts)
        .filter(|p| p.group == Group::new(b"snd!"))
        .filter_map(|p| Some(h1.tag(p.reference?).unwrap().name.clone()))
        .collect()
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_weapon_fires_its_halo1_sounds() {
    let maps = std::env::var("H1_MAPS").unwrap_or_else(|_| "bloodgulch".into());
    let mut weapons = 0;
    for name in maps.split(',') {
        let (_, _, base_name) = h1convert::convert::MAPS
            .iter()
            .find(|m| m.0 == name)
            .unwrap_or_else(|| panic!("{name}: not a multiplayer map"));
        let Some(h1) = h1_map(name) else {
            println!("{name}: not on the disc");
            continue;
        };
        let mut base = h2_map(base_name);
        let templates = Base::read(&mut base).unwrap();
        let mut objects = Objects::new(&h1, &templates);
        let p = prepare_halo1(
            &h1,
            content::Sha256::of(name.as_bytes()),
            &mut base,
            &mut objects,
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let mopp = structure_mopp(&p.collision).unwrap();
        let fresh = p.build_fresh(&mut base, &mopp).unwrap();
        let built = Map::from_reader(Cursor::new(fresh.bytes.clone())).unwrap();
        let index: Vec<(TagId, FourCC, String)> = built
            .tags()
            .iter()
            .map(|t| (t.id, t.group, t.name.clone().unwrap_or_default()))
            .collect();
        let find = |g: &[u8; 4], n: &str| {
            index
                .iter()
                .find(|t| t.1 == FourCC(*g) && t.2 == n)
                .map(|t| t.0)
        };
        let mut shared = tags::repo::extract::SharedFiles::default();
        let mut src = tags::repo::MapSource::new(
            tags::schema::Schema::builtin(),
            Map::from_reader(Cursor::new(fresh.bytes)).unwrap(),
            &mut shared,
        )
        .unwrap();
        let mut record = |g: &[u8; 4], id: TagId| {
            let tag = src
                .extract(id)
                .unwrap()
                .unwrap_or_else(|e| panic!("{g:?} {id:?}: {e:?}"));
            defs::decode(def(g), &tag).unwrap()
        };
        let path = |r: &Record, p: &str| match r.at(p) {
            Some(Val::Ref {
                target: Target::Key(k),
                ..
            }) => Some((k.group, k.path.clone())),
            Some(Val::Ref { .. }) => None,
            x => panic!("{p}: {x:?}"),
        };
        // An effect slot: Halo 1's effect `want` (or none), and the sounds
        // the built effect plays are Halo 1's, each in the map.
        let check = |record: &mut dyn FnMut(&[u8; 4], TagId) -> Record,
                     what: String,
                     r: &Record,
                     slot: &str,
                     want: Option<h1map::TagId>| {
            let got = path(r, slot);
            let Some(want) = want else {
                assert_eq!(got, None, "{what}: Halo 1 names no effect");
                return;
            };
            let want_name = h1.tag(want).unwrap().name.clone();
            let (g, n) =
                got.unwrap_or_else(|| panic!("{what}: no effect, Halo 1's is {want_name}"));
            assert_eq!(g, FourCC(*b"effe"), "{what}");
            assert_eq!(n, want_name, "{what}: not Halo 1's effect");
            let id = find(b"effe", &n).unwrap_or_else(|| panic!("{what}: {n} not in the map"));
            let fx = record(b"effe", id);
            let mut sounds = BTreeSet::new();
            for ev in block(&fx, "events") {
                for part in block(ev, "parts") {
                    if let Some((g, s)) = path(part, "reference")
                        && g == FourCC(*b"snd!")
                    {
                        assert!(find(b"snd!", &s).is_some(), "{what}: {s} not in the map");
                        sounds.insert(s);
                    }
                }
            }
            let expect = halo1_sounds(&h1, want);
            println!("  {what}: {n} -> {sounds:?}");
            // A Halo 1 sound that did not convert leaves its part out: the
            // weapon would be silent there.
            assert_eq!(
                sounds, expect,
                "{what}: {n} plays other sounds than Halo 1's"
            );
        };
        for t in h1.tags() {
            if t.group != Group::new(b"weap") {
                continue;
            }
            let Some(id) = find(b"weap", &t.name) else {
                println!(
                    "{name} {}: not in the map (no spawn or vehicle names it)",
                    t.name
                );
                continue;
            };
            weapons += 1;
            println!("{name} {}", t.name);
            let w = h1::Weapon::read(&h1, t).unwrap();
            let r = record(b"weap", id);
            let barrels = block(&r, "barrels");
            assert_eq!(barrels.len(), w.triggers.len(), "{}", t.name);
            for (i, (b, tr)) in barrels.iter().zip(&w.triggers).enumerate() {
                let fx = block(b, "block_e4");
                assert_eq!(fx.len(), tr.firing_effects.len(), "{} barrel {i}", t.name);
                for (k, (e, f)) in fx.iter().zip(&tr.firing_effects).enumerate() {
                    for (slot, want) in FIRING_EFFECT_REFS.iter().zip(f.effects) {
                        let what = format!("{} barrel {i} firing effect {k} {slot}", t.name);
                        check(&mut record, what, e, slot, want);
                    }
                }
            }
            let mags = block(&r, "magazines");
            for (i, (m, h)) in mags.iter().zip(&w.magazines).enumerate() {
                check(
                    &mut record,
                    format!("{} magazine {i} reloading", t.name),
                    m,
                    MAGAZINE_RELOADING_EFFECT,
                    h.reloading_effect,
                );
            }
        }
    }
    println!("{weapons} weapons checked");
    assert!(weapons > 0);
}
