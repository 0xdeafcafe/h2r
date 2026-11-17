//! Real-data proof of the Halo 1 sandbox: Blood Gulch converted onto
//! Coagulation with its Halo 1 objects added as new tags, and every netgame
//! equipment spawn of the built map resolving to an object that decodes.
//! Reads `$H1_ISO` and `$H2_DISC` at run time; prints names and counts
//! only. Nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test -p h1objects --test sandbox_disc -- --ignored --nocapture
//! ```

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1convert::apply::NETGAME_ITEM;
use h1convert::convert::halo1::{added_datum, prepare_halo1};
use h1convert::convert::{Sandbox, structure_mopp};
use h1convert::globals::{PLAYER_REPRESENTATION, PLAYER_REPRESENTATION_SIZE, REPRESENTATION_UNIT};
use h1map::H1Map;
use h1objects::base::Base;
use h1objects::sandbox::Objects;
use map::{FourCC, Map, TagId};
use scenario_edit::{BlockKind, Scenario};

fn bloodgulch() -> H1Map {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = disc
        .map("bloodgulch")
        .expect("bloodgulch on the disc")
        .clone();
    H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
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
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn bloodgulch_halo1_sandbox_places_halo1_objects() {
    let h1 = bloodgulch();
    let mut base_map = h2_map("coagulation");
    let templates = Base::read(&mut base_map).unwrap();
    let first = base_map.index_slots() as usize;
    let mut objects = Objects::new(&h1, &templates);
    let p = prepare_halo1(
        &h1,
        content::Sha256::of(b"bloodgulch"),
        &mut base_map,
        &mut objects,
    )
    .unwrap();
    assert_eq!(p.sandbox, Sandbox::Halo1);
    let added = p.objects.as_ref().expect("objects").count;
    let placed = p.applied.counts[2];
    let vehicles = p.applied.vehicles;
    for n in &p.objects.as_ref().unwrap().notes {
        eprintln!("{n}");
    }
    for u in &p.applied.unresolved {
        eprintln!("not placed: {u}");
    }
    let src = p.source;
    eprintln!(
        "{placed} items ({vehicles} vehicles) from Halo 1's {} items and {} vehicles; {added} tags added",
        src.items, src.vehicles
    );
    assert!(p.applied.collections.is_empty(), "no collections built");
    let mopp = structure_mopp(&p.collision).unwrap();
    let done = p.build(&mut base_map, &mopp).unwrap();
    assert_eq!(done.manifest.variants[0].origin.sandbox, "h1");

    let mut out = Map::from_reader(Cursor::new(done.bytes)).unwrap();
    assert_eq!(out.index_slots() as usize, first + added);
    let ctx = tags::object::Context::read(&mut out).unwrap();
    let names: std::collections::HashMap<TagId, (FourCC, String)> = out
        .tags()
        .iter()
        .map(|t| (t.id, (t.group, t.name.clone().unwrap_or_default())))
        .collect();
    let s = Scenario::read(&mut out).unwrap();
    let block = s.block(BlockKind::NetgameEquipment);
    let mut ours = std::collections::BTreeMap::<String, usize>::new();
    for i in 0..block.len() {
        let e = block.entry(i).unwrap();
        let w = |at: usize| u32::from_le_bytes(e[at..at + 4].try_into().unwrap());
        let (group, id) = (
            FourCC(w(NETGAME_ITEM).to_be_bytes()),
            TagId(w(NETGAME_ITEM + 4)),
        );
        let (g, name) = names
            .get(&id)
            .unwrap_or_else(|| panic!("spawn {i}: {group} {id:?} is no tag"));
        assert_eq!(*g, group, "spawn {i}: {name}");
        // Every spawn names a tag this conversion added: no Halo 2 item.
        let slot = (id.0 & 0xffff) as usize;
        assert!(slot >= first, "spawn {i} keeps the base's {group} {name}");
        assert_eq!(id, added_datum(slot), "spawn {i}: {name}");
        let o = ctx
            .resolve_object(id)
            .unwrap_or_else(|e| panic!("spawn {i}: {name}: {e}"));
        assert!(
            o.model.as_ref().is_some_and(|m| m.render_model.is_some()),
            "spawn {i}: {name} has no render model"
        );
        *ours.entry(format!("{group} {name}")).or_default() += 1;
    }
    assert_eq!(block.len(), placed);
    for (n, c) in &ours {
        eprintln!("{c:>3} x {n}");
    }
    for want in ["assault rifle", "health", "warthog", "ghost"] {
        assert!(
            ours.keys().any(|n| n.contains(want)),
            "no {want} placed: {ours:?}"
        );
    }
    // Players spawn as the converted cyborg: the globals' multiplayer
    // player representations (elements 2 and 3) name it.
    let g = out.globals().unwrap().clone();
    let gb = out.read_tag_bytes(g.id).unwrap();
    let gw = |at: usize| u32::from_le_bytes(gb[at..at + 4].try_into().unwrap());
    let reps = (gw(PLAYER_REPRESENTATION + 4) - g.location.unwrap().address) as usize;
    for e in [2, 3] {
        let at = reps + e * PLAYER_REPRESENTATION_SIZE + REPRESENTATION_UNIT;
        let id = TagId(gw(at + 4));
        let (grp, name) = &names[&id];
        assert_eq!(*grp, FourCC(*b"bipd"));
        assert!(name.contains("cyborg"), "player {e}: {name}");
        assert!(
            (id.0 & 0xffff) as usize >= first,
            "player {e}: the base's {name}"
        );
        let o = ctx.resolve_object(id).unwrap();
        assert!(o.model.as_ref().is_some_and(|m| m.render_model.is_some()));
        eprintln!("player representation {e}: {name}");
    }
    assert_eq!(
        ours.keys()
            .filter(|n| n.starts_with("vehi"))
            .map(|n| ours[n])
            .sum::<usize>(),
        vehicles
    );
}
