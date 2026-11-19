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
//!   (`effe`, or `snd!`: the plasma rifle's empty effect) and
//!   `h1::FIRING_EFFECT_DAMAGE` (`jpt!`).
//! - `halo2_firing_effect_groups`: on Coagulation every retail barrel firing
//!   effect names `effe` or `snd!` in `arsenal::FIRING_EFFECT_REFS` and
//!   `jpt!` in `arsenal::FIRING_DAMAGE_REFS`; magazine `+0x34` names `effe`.
//! - `every_weapon_fires_its_halo1_sounds`: Blood Gulch (`$H1_MAPS`, comma
//!   separated, picks other multiplayer maps) built fresh in the Halo 1
//!   sandbox, as `h2 serve` plays it, within the engine's tag memory (the
//!   numbers printed). Every Halo 1 weapon in the built map:
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
use h1objects::arsenal::{
    FIRING_DAMAGE_REFS, FIRING_EFFECT_REFS, MAGAZINE_CHAMBERING_EFFECT, MAGAZINE_RELOADING_EFFECT,
    WEAPON_EFFECTS,
};
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

fn int(r: &Record, path: &str) -> i64 {
    match r.at(path) {
        Some(Val::Int(x)) => *x,
        v => panic!("{path}: {v:?}"),
    }
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
    let (mut elements, mut effects, mut sounds, mut damage) = (0, 0, 0, 0);
    for (name, h1) in h1_maps() {
        let effe = Group::new(b"effe");
        let jpt = Group::new(b"jpt!");
        let snd = Group::new(b"snd!");
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
                        // An effect slot names an effect or a bare sound
                        // (the plasma rifle's empty effect is a `snd!`).
                        let ok = (FIRING_EFFECT_EFFECTS.contains(&f.offset)
                            && (f.group == effe || f.group == snd))
                            || (FIRING_EFFECT_DAMAGE.contains(&f.offset) && f.group == jpt);
                        assert!(ok, "{name} {}: {} at {:#x}", t.name, f.group, f.offset);
                        if f.tag.is_some() {
                            if f.group == effe {
                                effects += 1;
                            } else if f.group == snd {
                                sounds += 1;
                            } else {
                                damage += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "{elements} firing effects: {effects} effects, {sounds} sounds and {damage} damage named"
    );
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
    let mut anim_sounds = 0;
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
        // The tag memory the map takes, against the engine's (the build
        // refuses a map over it, saying which groups take the most).
        let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let budget = map::writer::max_metadata_size(map::CacheType::Multiplayer).unwrap();
        let memory = word(&fresh.bytes, 0x1c);
        println!(
            "{name}: metadata {:#x}, tag memory {memory:#x} of {budget:#x} ({:#x} free)",
            word(&fresh.bytes, 0x18),
            budget.saturating_sub(memory),
        );
        assert!(memory <= budget, "{name}: over the engine's tag memory");
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
            assert_eq!(n, want_name, "{what}: not Halo 1's");
            if h1.tag(want).unwrap().group == Group::new(b"snd!") {
                // A sound slot: Halo 1's sound itself.
                assert_eq!(g, FourCC(*b"snd!"), "{what}");
                assert!(find(b"snd!", &n).is_some(), "{what}: {n} not in the map");
                println!("  {what}: {n}");
                return;
            }
            assert_eq!(g, FourCC(*b"effe"), "{what}");
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
                check(
                    &mut record,
                    format!("{} magazine {i} chambering", t.name),
                    m,
                    MAGAZINE_CHAMBERING_EFFECT,
                    h.chambering_effect,
                );
            }
            // The weapon's own effects and sounds: Halo 1's, or empty
            // (never the counterpart's).
            let bytes = h1objects::arsenal::weapon_bytes(&h1, t).unwrap();
            for (slot, at) in WEAPON_EFFECTS {
                let want = h1::reference(&h1, &bytes, *at);
                check(&mut record, format!("{} {slot}", t.name), &r, slot, want);
            }
            // First-person animation sounds (reload, ready): every Halo 1
            // animation's sound is the converted graph's sound event on the
            // same animation and frame, naming Halo 1's sound.
            if let Some(antr) = w.first_person_animations {
                let antr = h1.tag(antr).unwrap();
                let sounds = h1models::sounds::read(&h1, antr).unwrap();
                let fp = block(&r, "first_person");
                let (_, graph) = path(&fp[0], "animations")
                    .unwrap_or_else(|| panic!("{}: no first-person graph", t.name));
                let jmad = record(b"jmad", find(b"jmad", &graph).unwrap());
                let refs: Vec<Option<String>> = block(&jmad, "sound_references")
                    .iter()
                    .map(|x| path(x, "sound").map(|k| k.1))
                    .collect();
                let anims = block(&jmad, "animations");
                for (a, k, frame) in &sounds.events {
                    let Some(sref) = sounds.refs[*k] else {
                        continue;
                    };
                    let want = h1.tag(sref).unwrap().name.clone();
                    let events = block(&anims[*a], "sound_events");
                    let got: Vec<(Option<String>, i64)> = events
                        .iter()
                        .map(|e| {
                            let i = int(e, "sound");
                            (refs.get(i as usize).cloned().flatten(), int(e, "frame"))
                        })
                        .collect();
                    println!("  {} animation {a}: {want} at frame {frame}", graph);
                    assert_eq!(
                        got,
                        [(Some(want.clone()), i64::from(*frame))],
                        "{} animation {a}: Halo 1 plays {want} at frame {frame}",
                        graph
                    );
                    assert!(find(b"snd!", &want).is_some(), "{want} not in the map");
                }
                anim_sounds += sounds.events.len();
            }
        }
    }
    println!("{weapons} weapons checked, {anim_sounds} first-person animation sounds");
    assert!(weapons > 0);
}

/// Halo 1's animation sounds (`h1models::sounds`) on every map: each
/// sound reference element (`H1_SOUND_REF_LEN`) holds a `snd!` reference
/// at `+0` and nothing else, and every animation's sound index is -1 or a
/// reference with its frame inside the animation.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn halo1_animation_sounds() {
    use h1models::sounds::{H1_ANIMATION_SOUND, H1_SOUND_REF_LEN, H1_SOUND_REFS};
    let (mut graphs, mut refs, mut events, mut out_of_range) = (0, 0, 0, 0);
    let snd = Group::new(b"snd!");
    for (name, h1) in h1_maps() {
        for t in h1.tags() {
            if t.group != Group::new(b"antr") {
                continue;
            }
            let h = h1.tag_data(t).unwrap();
            let blk = h1map::Block::read(h, H1_SOUND_REFS).unwrap();
            let sref = h1.block(blk, H1_SOUND_REF_LEN).unwrap().to_vec();
            let n = sref.len() / H1_SOUND_REF_LEN;
            for e in sref.chunks_exact(H1_SOUND_REF_LEN) {
                let found = h1objects::refs::scan(&h1, e);
                assert!(found.len() <= 1, "{name} {}: {found:?}", t.name);
                if let Some(f) = found.first() {
                    assert_eq!((f.offset, f.group), (0, snd), "{name} {}", t.name);
                }
            }
            let ab = h1map::Block::read(h, 0x74).unwrap();
            let anims = h1.block(ab, 0xb4).unwrap().to_vec();
            for a in anims.chunks_exact(0xb4) {
                let s = i16::from_le_bytes([a[H1_ANIMATION_SOUND], a[H1_ANIMATION_SOUND + 1]]);
                let f = i16::from_le_bytes([a[H1_ANIMATION_SOUND + 2], a[H1_ANIMATION_SOUND + 3]]);
                let frames = i16::from_le_bytes([a[0x22], a[0x23]]);
                if s < 0 {
                    continue;
                }
                assert!((s as usize) < n, "{name} {}: sound {s} of {n}", t.name);
                if (0..frames.max(1)).contains(&f) {
                    events += 1;
                } else {
                    out_of_range += 1;
                }
            }
            let got = h1models::sounds::read(&h1, t).unwrap();
            assert_eq!(got.refs.len(), n);
            graphs += 1;
            refs += n;
        }
    }
    println!(
        "{graphs} graphs, {refs} sound references, {events} animation sounds ({out_of_range} with a frame outside the animation)"
    );
    assert!(events > 0);
}

/// Retail Halo 2 graphs on Coagulation: how many animations carry sound
/// events, and what the word after the frame holds (the evidence for
/// writing 0 there, `h1models::sounds`); every sound reference names a
/// `snd!`.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn retail_animation_sound_events() {
    let mut m = h2_map("coagulation");
    let base = Base::read(&mut m).unwrap();
    let (mut graphs, mut events, mut zero) = (0, 0, 0);
    let mut words = BTreeSet::new();
    for t in m.tags().to_vec() {
        if t.group != FourCC(*b"jmad") || t.location.is_none() {
            continue;
        }
        let Ok(r) = base.record(def(b"jmad"), t.id) else {
            continue;
        };
        let name = t.name.clone().unwrap_or_default();
        for x in block(&r, "sound_references") {
            if let Some(Val::Ref {
                target: Target::Id(id),
                ..
            }) = x.at("sound")
                && let Some((g, _)) = base.name(*id)
            {
                assert_eq!(g, FourCC(*b"snd!"), "{name}");
            }
        }
        let mut here = 0;
        for a in block(&r, "animations") {
            for e in block(a, "sound_events") {
                here += 1;
                let w = format!("{:?}", e.at("string_4"));
                if w.contains("Index(0)") || w.contains("Text(\"\")") {
                    zero += 1;
                }
                words.insert(w);
            }
        }
        if here > 0 {
            graphs += 1;
            events += here;
            if name.contains("\\fp\\") {
                println!("{name}: {here} sound events");
            }
        }
    }
    println!(
        "{graphs} graphs with {events} sound events; {zero} with an empty word; words {words:?}"
    );
    assert!(events > 0);
}

/// What the weapons' Halo 1 effects and sounds add to a map's metadata,
/// measured: on each map in `$H1_MAPS` (default Blood Gulch), every Halo 1
/// weapon's firing, misfire and empty effects, magazine effects, own
/// effect and sound slots (`WEAPON_EFFECTS`) and first-person animation
/// sounds, converted alone (a fresh `Converter`, so nothing else the map
/// converts is counted), as tags and tree bytes per group, the largest
/// tags, and the sounds' `snd!` and gestalt rows (permutations `0x10`,
/// pitch ranges `0xc`, chunks `0xc`, parameters `0xa`). Before converter
/// version 84 none of this was in the map (the counterparts' were, from
/// the base). Prints only.
#[test]
#[ignore = "reads the Halo 1 and Halo 2 discs ($H1_ISO, $H2_DISC)"]
fn weapon_effects_metadata() {
    use h1objects::convert::{Converter, ObjectReport};
    let maps = std::env::var("H1_MAPS").unwrap_or_else(|_| "bloodgulch".into());
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
        let mut c = Converter::new(&h1, &templates);
        let mut rep = ObjectReport::default();
        let (mut effects, mut sounds) = (BTreeSet::new(), BTreeSet::new());
        for t in h1.tags() {
            if t.group != Group::new(b"weap") {
                continue;
            }
            let w = h1::Weapon::read(&h1, t).unwrap();
            let bytes = h1objects::arsenal::weapon_bytes(&h1, t).unwrap();
            let mut ids: Vec<Option<h1map::TagId>> = Vec::new();
            for tr in &w.triggers {
                for f in &tr.firing_effects {
                    ids.extend(f.effects);
                }
            }
            for m in &w.magazines {
                ids.extend([m.reloading_effect, m.chambering_effect]);
            }
            for (_, at) in WEAPON_EFFECTS {
                ids.push(h1::reference(&h1, &bytes, *at));
            }
            if let Some(a) = w.first_person_animations {
                let s = h1models::sounds::read(&h1, h1.tag(a).unwrap()).unwrap();
                ids.extend(s.refs);
            }
            for id in ids.into_iter().flatten() {
                let tag = h1.tag(id).unwrap();
                if tag.group == Group::new(b"snd!") {
                    c.sound(tag, &mut rep);
                    sounds.insert(tag.name.clone());
                } else if tag.group == Group::new(b"effe") {
                    if let Err(e) = c.effect(tag) {
                        println!("{name} {}: {e}", tag.name);
                    }
                    effects.insert(tag.name.clone());
                }
            }
        }
        let mut groups: std::collections::BTreeMap<String, (usize, u64)> = Default::default();
        let mut sizes = Vec::new();
        for (n, t) in &c.tree.tags {
            let len = t.root.tree_len();
            let g = groups.entry(t.group.to_string()).or_default();
            g.0 += 1;
            g.1 += len;
            sizes.push((len, t.group.to_string(), n.clone()));
        }
        let (mut rows, mut perms, mut chunks) = (0u64, 0usize, 0usize);
        for (_, s) in &c.tree.sounds {
            rows += 0x14 + 0x38 + 0x14;
            for r in &s.ranges {
                rows += 0xc + 0xa;
                perms += r.permutations.len();
                chunks += r.permutations.iter().map(|p| p.chunks.len()).sum::<usize>();
            }
        }
        rows += 0x10 * perms as u64 + 0xc * chunks as u64;
        let tags_total: u64 = groups.values().map(|g| g.1).sum();
        println!(
            "{name}: {} Halo 1 weapon effects and {} sounds named; adds {} tags, {tags_total:#x} bytes, and {} sounds ({perms} permutations, {chunks} chunks), about {rows:#x} bytes of snd! and gestalt rows; {:#x} in all (padding not counted)",
            effects.len(),
            sounds.len(),
            c.tree.tags.len(),
            c.tree.sounds.len(),
            tags_total + rows,
        );
        let mut by: Vec<_> = groups.into_iter().collect();
        by.sort_by(|a, b| b.1.1.cmp(&a.1.1));
        for (g, (n, b)) in by {
            println!("  {g}: {n} tags, {b:#x} bytes");
        }
        sizes.sort_by(|a, b| b.0.cmp(&a.0));
        for (len, g, n) in sizes.iter().take(15) {
            println!("  {len:#7x} {g} {n}");
        }
    }
}
