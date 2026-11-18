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
    // The Halo 1 objects, then Halo 1's textures (bitmaps, shader copies;
    // a structure bitmap an object already added is reused), skies and
    // ambience (looping sounds, the sounds they play, environments; a
    // sound an object already added is reused).
    let ambience = &p.ambience;
    let textures = ambience.backgrounds.len()
        + ambience
            .backgrounds
            .iter()
            .map(|(_, l)| l.sounds.len())
            .sum::<usize>()
        + ambience.environments.len()
        + p.textures.bitmaps.len()
        + p.textures.copies.len()
        + p.skies
            .skies
            .iter()
            .map(|s| 1 + usize::from(s.model.is_some()))
            .sum::<usize>();
    let slots = out.index_slots() as usize;
    assert!(
        (first + added..=first + added + textures).contains(&slots),
        "{slots} slots"
    );
    // Halo 1's textures on the converted models: each material names a
    // copy of the base's model shader whose bitmaps include one this
    // conversion added (a Halo 1 bitmap).
    let modes: Vec<(TagId, String)> = out
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"mode") && (t.id.0 & 0xffff) as usize >= first)
        .map(|t| (t.id, t.name.clone().unwrap_or_default()))
        .collect();
    let stand_in = templates.model_shader.as_ref().map(|t| t.shader);
    // The sky model's materials name the base sky's shader (h1convert::sky).
    let sky_shader = p.skies.shader.expect("the base sky's shader");
    let (mut drawn, mut null, mut stood_in, mut sky) = (0, 0, 0, 0);
    for (id, name) in &modes {
        let b = out.read_tag_bytes(*id).unwrap();
        let at = out.tag(*id).unwrap().location.unwrap().address;
        let w = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let (count, start) = (w(0x60) as usize, w(0x64));
        for k in 0..count {
            let e = (start - at) as usize + 0x20 * k;
            let shader = TagId(w(e + 0xc));
            if shader.0 == u32::MAX {
                null += 1;
                continue;
            }
            if Some(shader) == stand_in {
                stood_in += 1;
                continue;
            }
            if shader == sky_shader {
                sky += 1;
                continue;
            }
            let (template, bitmaps) = scenario_edit::scene::shader_bitmaps(&mut out, shader)
                .unwrap_or_else(|e| panic!("{name} material {k}: {e}"));
            assert_eq!(template, h1convert::textures::MODEL_TEMPLATE, "{name}");
            assert!(
                bitmaps.iter().any(|b| (b.0 & 0xffff) as usize >= first),
                "{name} material {k} draws no Halo 1 bitmap"
            );
            drawn += 1;
        }
    }
    eprintln!(
        "models: {} converted, {drawn} materials with Halo 1 textures, {stood_in} the base's model shader, {sky} the base sky's shader, {null} null",
        modes.len()
    );
    // No material is null: the shader draw does not skip a NONE shader.
    assert_eq!(null, 0, "{drawn} drawn, {null} null");
    assert!(
        drawn > 0 && drawn >= 4 * stood_in,
        "{drawn} drawn, {stood_in} stood in"
    );
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

/// Every `shad` of Blood Gulch's installed maps, both sandboxes built fresh
/// as `h2 h1 import` builds them, has what the shader draw `0x16b90` reads
/// before anything else: one compiled postprocess element (`shad +0x20`, its
/// address read at `0x16bc7` and dereferenced at `0x16bde`), its template's
/// (`stem +0x58`, `0x16bf4`) and each pass's (`spas +0x1c`). A shader
/// without one is a null read on the first frame that draws it.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn bloodgulch_shaders_all_have_their_compiled_postprocess_in_both_sandboxes() {
    let h1 = bloodgulch();
    let sha = content::Sha256::of(b"bloodgulch");
    let mut bad = Vec::new();
    for sandbox in [Sandbox::Halo2, Sandbox::Halo1] {
        let mut base = h2_map("coagulation");
        let p = if sandbox == Sandbox::Halo1 {
            let templates = Base::read(&mut base).unwrap();
            let mut objects = Objects::new(&h1, &templates);
            prepare_halo1(&h1, sha, &mut base, &mut objects).unwrap()
        } else {
            h1convert::convert::prepare(&h1, sha, &mut base, sandbox).unwrap()
        };
        let mopp = structure_mopp(&p.collision).unwrap();
        let fresh = p.build_fresh(&mut base, &mopp).unwrap();
        let manifest = p.manifest(&fresh.bytes);
        let mut out = Map::from_reader(Cursor::new(fresh.bytes)).unwrap();
        if sandbox == Sandbox::Halo2 {
            // The precision swap: the SMG and the battle rifle collections,
            // both in the fresh map (the BR kept by the prune), and the
            // manifest records it.
            let swaps = &p.applied.precision;
            assert_eq!(swaps.len(), 1, "{swaps:?}");
            for id in [swaps[0].0, swaps[0].1] {
                let t = out.tag(id).unwrap();
                assert_eq!(t.group, FourCC(*b"itmc"));
                eprintln!("precision: {:#x} {:?}", id.0, t.name);
            }
            assert_eq!(
                manifest.variants[0].precision,
                [content::Swap {
                    from: swaps[0].0.0,
                    to: swaps[0].1.0
                }]
            );
        }
        let shaders: Vec<(TagId, String)> = out
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"shad") && t.location.is_some())
            .map(|t| (t.id, t.name.clone().unwrap_or_default()))
            .collect();
        // Every render model's materials name a shader: the shader draw
        // begin `0x1bbf0` does not skip NONE.
        let modes: Vec<(TagId, String)> = out
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"mode") && t.location.is_some())
            .map(|t| (t.id, t.name.clone().unwrap_or_default()))
            .collect();
        for (id, name) in &modes {
            let b = out.read_tag_bytes(*id).unwrap();
            let at = out.tag(*id).unwrap().location.unwrap().address;
            let w = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
            let (count, start) = (w(0x60) as usize, w(0x64));
            for k in 0..count {
                let shader = w((start - at) as usize + 0x20 * k + 0xc);
                if shader == u32::MAX {
                    bad.push(format!("{sandbox:?} {name}: material {k} names no shader"));
                }
            }
        }
        let mut lib = tags::material::ShaderLibrary::read(&mut out).unwrap();
        let mut drawn = 0;
        for (id, name) in &shaders {
            match lib.material(*id, None) {
                Ok(_) => drawn += 1,
                Err(e) => bad.push(format!("{sandbox:?} {name} ({:#x}): {e}", id.0)),
            }
        }
        eprintln!(
            "{sandbox:?}: {drawn} of {} shaders resolve to passes",
            shaders.len()
        );
    }
    assert!(bad.is_empty(), "{} shaders: {bad:#?}", bad.len());
}

/// The metadata of `m` as one region, for reading blocks wherever they lie.
fn memory(m: &mut Map<Cursor<Vec<u8>>>) -> tags::schema::Memory {
    let space = m.metadata_space().unwrap();
    let meta = m
        .read_file_range(space.file_offset, u64::from(space.size))
        .unwrap();
    tags::schema::Memory::from_regions(
        vec![(space.virtual_base, meta)],
        vec![(space.virtual_base, space.virtual_base + space.size)],
    )
}

/// Every `vehi`'s `+0x2f8` elements (`0x2a0` each) as (name, element,
/// count at `+0x38`). The load-time loop `0x205550` (from `0x11c8f3`)
/// writes that many `0x90`-byte entries from element `+0x60`: room for 4.
fn vehicle_physics_counts(m: &mut Map<Cursor<Vec<u8>>>) -> Vec<(String, u32, u32)> {
    let mem = memory(m);
    let word = |a: u32| {
        mem.slice(a, 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let mut out = Vec::new();
    for t in m.tags().to_vec() {
        let Some(loc) = t.location else { continue };
        if t.group != FourCC(*b"vehi") {
            continue;
        }
        let name = t.name.clone().unwrap_or_default();
        let count = word(loc.address + 0x2f8).unwrap_or(u32::MAX);
        let first = word(loc.address + 0x2fc).unwrap_or(0);
        for k in 0..count.min(16) {
            let n = word(first + k * 0x2a0 + 0x38).unwrap_or(u32::MAX);
            out.push((name.clone(), k, n));
        }
        if count > 16 {
            out.push((name, count, u32::MAX));
        }
    }
    out
}

/// Halo 1 multiplayer map `name` from the disc, if the disc has it (the
/// Xbox release lacks the PC-only maps).
fn h1_map(name: &str) -> Option<H1Map> {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = disc.map(name)?.clone();
    Some(H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap())
}

/// The fresh build of every Halo 1 multiplayer map (`$H1_MAPS`, comma
/// separated, narrows it) in both sandboxes.
fn every_fresh_map(
    mut each: impl FnMut(&str, Sandbox, &h1convert::convert::Prepared, &h1convert::fresh::Fresh),
) {
    let only = std::env::var("H1_MAPS").ok();
    for (name, _, base_name) in h1convert::convert::MAPS {
        if only
            .as_deref()
            .is_some_and(|o| !o.split(',').any(|m| m == name))
        {
            continue;
        }
        let Some(h1) = h1_map(name) else {
            println!("{name}: not on the disc");
            continue;
        };
        let sha = content::Sha256::of(name.as_bytes());
        for sandbox in [Sandbox::Halo2, Sandbox::Halo1] {
            let mut base = h2_map(base_name);
            let p = if sandbox == Sandbox::Halo1 {
                let templates = Base::read(&mut base).unwrap();
                let mut objects = Objects::new(&h1, &templates);
                prepare_halo1(&h1, sha, &mut base, &mut objects)
            } else {
                h1convert::convert::prepare(&h1, sha, &mut base, sandbox)
            }
            .unwrap_or_else(|e| panic!("{name} {sandbox:?}: {e}"));
            let mopp = structure_mopp(&p.collision).unwrap();
            let fresh = p
                .build_fresh(&mut base, &mopp)
                .unwrap_or_else(|e| panic!("{name} {sandbox:?}: {e}"));
            // Built from scratch, not the pruned fallback.
            assert!(
                fresh.report.scratch.is_some(),
                "{name} {sandbox:?}: not from scratch: {:?}",
                fresh.report.scratch_failed
            );
            each(name, sandbox, &p, &fresh);
        }
    }
}

/// The fresh map keeps no template address: a converted vehicle built on
/// a retail one carried the template's `+0x2f8` block by its pinned
/// address, which in a fresh map holds other bytes, so the load loop
/// `0x205550` read a junk count and wrote past its element (pc `0x205574`,
/// Blood Gulch and Sidewinder in the Halo 1 sandbox).
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_vehicle_s_physics_entries_fit_their_element() {
    let mut bad = Vec::new();
    let mut vehicles = 0;
    every_fresh_map(|name, sandbox, _, fresh| {
        let mut out = Map::from_reader(Cursor::new(fresh.bytes.clone())).unwrap();
        let counts = vehicle_physics_counts(&mut out);
        println!(
            "{name} {sandbox:?}: {} vehicle elements, rehomed {:#?}",
            counts.len(),
            fresh.report.rehomed
        );
        vehicles += counts.len();
        for (v, k, n) in counts {
            if n > 4 {
                bad.push(format!("{name} {sandbox:?} {v} [{k}]: {n:#x}"));
            }
        }
        for s in &fresh.report.stale {
            bad.push(format!("{name} {sandbox:?}: stale {s}"));
        }
    });
    assert!(vehicles > 0);
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Each starting location's bytes after position and facing.
fn starting_locations(m: &mut Map<Cursor<Vec<u8>>>) -> Vec<([f32; 3], Vec<u8>)> {
    let s = Scenario::read(m).unwrap();
    let block = s.block(BlockKind::StartingLocations);
    (0..block.len())
        .map(|i| {
            let e = block.entry(i).unwrap();
            let f = |at: usize| f32::from_le_bytes(e[at..at + 4].try_into().unwrap());
            ([f(0), f(4), f(8)], e[0x10..0x34].to_vec())
        })
        .collect()
}

/// Every converted starting location is in open space with the converted
/// collision's floor under it (Battle Creek's player died at spawn and
/// never respawned: not from where it stood).
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_starting_location_stands_on_the_converted_floor() {
    for base in ["beavercreek", "coagulation"] {
        for (p, rest) in starting_locations(&mut h2_map(base)) {
            println!("retail {base} {p:?} {rest:02x?}");
        }
    }
    let mut bad = Vec::new();
    every_fresh_map(|name, sandbox, p, fresh| {
        let mut out = Map::from_reader(Cursor::new(fresh.bytes.clone())).unwrap();
        let spawns = starting_locations(&mut out);
        for (k, (at, rest)) in spawns.iter().enumerate() {
            let up = [at[0], at[1], at[2] + 0.1];
            let open = h1convert::query::leaf_at(&p.collision, up).is_some();
            // How far below the location the floor is, within 2 units.
            let floor = h1convert::query::first_hit(&p.collision, up, [at[0], at[1], at[2] - 2.0])
                .map(|t| t * 2.1 - 0.1);
            if k < 3 {
                println!("{name} {sandbox:?} [{k}] {at:?} open {open} floor {floor:?} {rest:02x?}");
            }
            if !open || floor.is_none() {
                bad.push(format!(
                    "{name} {sandbox:?} [{k}] {at:?}: open {open}, floor {floor:?}"
                ));
            }
        }
    });
    assert!(bad.is_empty(), "{bad:#?}");
}
