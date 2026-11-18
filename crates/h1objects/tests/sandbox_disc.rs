//! Real-data proof of the Halo 1 sandbox: Blood Gulch converted onto
//! Coagulation with its Halo 1 objects added as new tags, and every netgame
//! equipment spawn of the built map resolving to an object that decodes.
//! Reads `$H1_ISO` and `$H2_DISC` at run time; prints names and counts
//! only. Nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test -p h1objects --test sandbox_disc -- --ignored --nocapture
//! ```

use std::collections::BTreeSet;
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
    // The player's Halo 1 HUD centres the motion sensor where its unhi's
    // placement (+0x35c) does, recorded for boot::hud_swap.
    let cyborg_hud = h1
        .tags()
        .iter()
        .find(|t| t.group == h1map::Group::new(b"unhi") && t.name.contains("cyborg"))
        .expect("the cyborg's unhi");
    let centre = h1objects::h1hud::sensor_centre(h1.tag_data(cyborg_hud).unwrap()).unwrap();
    eprintln!("motion sensor centre {centre:?} (Halo 2's own (41, -42))");
    assert_eq!(done.manifest.variants[0].hud_sensor, Some(centre));
    let text = done.manifest.to_toml();
    assert!(text.contains(&format!("hud_sensor = [{}, {}]", centre[0], centre[1])), "{text}");

    let built: std::sync::Arc<[u8]> = done.bytes.clone().into();
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
            .map(|s| {
                // The sky, its model, and at most a shader copy per
                // material and each Halo 1 bitmap they draw.
                1 + usize::from(s.model.is_some())
                    + s.shaders.iter().flatten().count()
                    + s.bitmaps.len()
            })
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
    // The sky model's materials name copies of the base's sky shaders
    // drawing Halo 1's bitmaps, or the base sky's own (h1convert::sky).
    let sky_fallback = p.skies.templates.fallback;
    let sky_templates = [
        h1convert::sky_shaders::ALPHA_TEMPLATE,
        h1convert::sky_shaders::CLOUDS_TEMPLATE,
        h1convert::sky_shaders::ADDITIVE_TEMPLATE,
    ];
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
            if Some(shader) == sky_fallback {
                sky += 1;
                continue;
            }
            let (template, bitmaps) = scenario_edit::scene::shader_bitmaps(&mut out, shader)
                .unwrap_or_else(|e| panic!("{name} material {k}: {e}"));
            if sky_templates.contains(&template.as_str()) {
                assert!(
                    bitmaps.iter().any(|b| (b.0 & 0xffff) as usize >= first),
                    "{name} sky material {k} draws no Halo 1 bitmap"
                );
                sky += 1;
                continue;
            }
            assert_eq!(template, h1convert::textures::MODEL_TEMPLATE, "{name}");
            assert!(
                bitmaps.iter().any(|b| (b.0 & 0xffff) as usize >= first),
                "{name} material {k} draws no Halo 1 bitmap"
            );
            drawn += 1;
        }
    }
    eprintln!(
        "models: {} converted, {drawn} materials with Halo 1 textures, {stood_in} the base's model shader, {sky} sky shaders, {null} null",
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
    let mem = memory(&mut out);
    let mut ours = std::collections::BTreeMap::<String, usize>::new();
    for i in 0..block.len() {
        let e = block.entry(i).unwrap();
        let w = |at: usize| u32::from_le_bytes(e[at..at + 4].try_into().unwrap());
        let (group, id) = (
            FourCC(w(NETGAME_ITEM).to_be_bytes()),
            TagId(w(NETGAME_ITEM + 4)),
        );
        let (g, cname) = names
            .get(&id)
            .unwrap_or_else(|| panic!("spawn {i}: {group} {id:?} is no tag"));
        assert_eq!(*g, group, "spawn {i}: {cname}");
        // Every spawn names a collection this conversion added: the
        // spawner `0x0015aad0` places only what an `itmc` or `vehc` rolls.
        assert!(
            group == FourCC(*b"itmc") || group == FourCC(*b"vehc"),
            "spawn {i}: {group} {cname} is no collection"
        );
        let slot = (id.0 & 0xffff) as usize;
        assert!(slot >= first, "spawn {i} keeps the base's {group} {cname}");
        assert_eq!(id, added_datum(slot), "spawn {i}: {cname}");
        // Its entries (`+0`, `0x10` each, the object at `+4`) name objects
        // this conversion added, each with a render model.
        let at = out.tag(id).unwrap().location.unwrap().address;
        let n = rw(&mem, at) as usize;
        let entries = rw(&mem, at + 4);
        assert!(n > 0, "spawn {i}: {cname} is empty");
        for k in 0..n as u32 {
            let e = entries + k * 0x10;
            let (og, oid) = (FourCC(rw(&mem, e + 4).to_be_bytes()), TagId(rw(&mem, e + 8)));
            let (g, name) = names
                .get(&oid)
                .unwrap_or_else(|| panic!("spawn {i}: {cname} entry {k} is no tag"));
            assert_eq!(*g, og, "{cname} entry {k}: {name}");
            let want: &[&[u8; 4]] = if group == FourCC(*b"vehc") {
                &[b"vehi"]
            } else {
                &[b"weap", b"eqip"]
            };
            assert!(
                want.iter().any(|w| FourCC(**w) == *g),
                "{cname} entry {k}: {g} {name}"
            );
            assert!(
                (oid.0 & 0xffff) as usize >= first,
                "{cname} entry {k}: the base's {name}"
            );
            let o = ctx
                .resolve_object(oid)
                .unwrap_or_else(|e| panic!("spawn {i}: {name}: {e}"));
            assert!(
                o.model.as_ref().is_some_and(|m| m.render_model.is_some()),
                "spawn {i}: {name} has no render model"
            );
            if k == 0 {
                *ours.entry(format!("{g} {name}")).or_default() += 1;
            }
        }
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
    let mut player = TagId::NULL;
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
        player = id;
    }
    // The player's camera sits at its model's head marker and its weapons
    // attach at the hand markers (h1objects::rebuild::UNIT_MARKERS): each
    // names a marker group of the converted render model.
    {
        let strings = out.strings().to_vec();
        let s = |v: u32| strings.get((v & 0xff_ffff) as usize).cloned().flatten();
        let bipd = out.tag(player).unwrap().location.unwrap().address;
        let hlmt = out.tag(TagId(rw(&mem, bipd + 0x38))).unwrap().location.unwrap().address;
        let mode = out.tag(TagId(rw(&mem, hlmt + 4))).unwrap().location.unwrap().address;
        let markers: Vec<String> = (0..rw(&mem, mode + 0x58))
            .filter_map(|k| s(rw(&mem, rw(&mem, mode + 0x5c) + k * 0xc)))
            .collect();
        for (at, want) in [(0xd4, "head"), (0x158, "right hand"), (0x15c, "left hand")] {
            let got = s(rw(&mem, bipd + at));
            assert_eq!(got.as_deref(), Some(want), "player unit +{at:#x}");
            assert!(markers.iter().any(|m| m == want), "{want} in {markers:?}");
        }
    }
    // The player's HUD is Halo 1's (docs/game/hud.md): every HUD
    // interface names the converted unhi's nhdt, whose widgets draw Halo
    // 1's own HUD bitmaps (converted, so named as on the Halo 1 disc)
    // through Halo 2's HUD shaders, the meters on the shield and body
    // inputs, every widget behind the shield meter's state flags.
    {
        use h1objects::h1hud;
        use h1objects::health_hud::{self as hh, widget};
        let space = out.metadata_space().unwrap();
        let meta = out
            .read_file_range(space.file_offset, u64::from(space.size))
            .unwrap();
        let bytes = |a: u32, n: usize| {
            let o = (a - space.virtual_base) as usize;
            meta[o..o + n].to_vec()
        };
        let w = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let unit = bytes(out.tag(player).unwrap().location.unwrap().address, 0x1a8);
        let count = w(&unit, hh::UNIT_HUD_INTERFACES) as usize;
        assert!(count > 0, "the player has no HUD");
        let huds = bytes(
            w(&unit, hh::UNIT_HUD_INTERFACES + 4),
            count * hh::HUD_INTERFACE_SIZE,
        );
        let h1_bitmaps: std::collections::HashSet<String> = h1
            .tags()
            .iter()
            .filter(|t| t.group == h1map::Group::new(b"bitm"))
            .map(|t| t.name.clone())
            .collect();
        for i in 0..count {
            let id = TagId(w(&huds, i * hh::HUD_INTERFACE_SIZE + 4));
            let (grp, name) = &names[&id];
            assert_eq!(*grp, FourCC(*b"nhdt"));
            assert!(name.ends_with(h1hud::NHDT_SUFFIX), "interface {i}: {name}");
            let root = bytes(
                out.tag(id).unwrap().location.unwrap().address,
                hh::NHDT_ROOT,
            );
            // Bitmap widgets, and Halo 1's grenade counts as Halo 2 number
            // texts on inputs 20 and 21 reading hud_input1 (Halo 2's own
            // string ID, as retail frag_count's), through text_flash.
            assert!(root[0x18..hh::NHDT_ROOT].iter().all(|b| *b == 0), "{name}");
            let texts = w(&root, h1hud::NHDT_TEXT_WIDGETS) as usize;
            assert_eq!(texts, 2, "{name}: grenade counts");
            let tb = bytes(w(&root, h1hud::NHDT_TEXT_WIDGETS + 4), texts * h1hud::text::SIZE);
            for (k, t) in tb.as_chunks::<{ h1hud::text::SIZE }>().0.iter().enumerate() {
                assert_eq!(t[h1hud::text::INPUTS], 20 + k as u8, "{name}");
                assert_eq!(t[h1hud::text::STATE..h1hud::text::STATE + 20], h1hud::GRENADE_STATES[k]);
                assert_eq!(w(t, h1hud::text::STRING), 0x0a00_05cf, "{name}: hud_input1");
                assert_eq!(u16::from_le_bytes([t[h1hud::text::FLAGS], t[h1hud::text::FLAGS + 1]]), h1hud::TEXT_NUMBER);
                let (sg, shader) = names[&TagId(w(t, h1hud::text::SHADER + 4))].clone();
                assert_eq!((sg, shader.as_str()), (FourCC(*b"shad"), h1hud::TEXT_SHADER));
                eprintln!(
                    "{name}: grenade count {k} anchor {} justification {} offset {:?}",
                    t[h1hud::text::ANCHOR],
                    t[h1hud::text::JUSTIFICATION],
                    hh::offset(t, h1hud::text::OFFSETS[0])
                );
            }
            let n = w(&root, hh::NHDT_BITMAP_WIDGETS) as usize;
            assert!(n >= 4, "{name}: {n} widgets");
            let widgets = bytes(
                w(&root, hh::NHDT_BITMAP_WIDGETS + 4),
                n * hh::BITMAP_WIDGET_SIZE,
            );
            let mut inputs = Vec::new();
            let mut grenade_widgets = 0;
            let mut alphas = Vec::new();
            let mut sensor = Vec::new();
            for x in widgets.as_chunks::<{ hh::BITMAP_WIDGET_SIZE }>().0 {
                let name_of = |at: usize| names[&TagId(w(x, at + 4))].clone();
                let (bg, bitmap) = name_of(widget::BITMAP);
                let (sg, shader) = name_of(widget::SHADER);
                assert_eq!(bg, FourCC(*b"bitm"));
                assert_eq!(sg, FourCC(*b"shad"));
                // Halo 1's bitmap, or its copy with the element colour's
                // alpha (h1hud::scale_alpha).
                let (source, alpha) = match bitmap.rsplit_once("_alpha_") {
                    Some((s, a)) => (s.to_owned(), Some(a.to_owned())),
                    None => (bitmap.clone(), None),
                };
                assert!(h1_bitmaps.contains(&source), "{name}: {bitmap} is Halo 1's");
                if let Some(a) = alpha {
                    if a == "28" {
                        sensor.push((TagId(w(x, widget::BITMAP + 4)), source.clone(), x[widget::SEQUENCES]));
                    }
                    alphas.push(a);
                }
                let state = &x[widget::STATE..widget::STATE_END];
                if h1hud::GRENADE_STATES.iter().any(|g| g == state) {
                    grenade_widgets += 1;
                } else {
                    assert_eq!(state, h1hud::STATE);
                }
                assert_eq!(x[widget::SPECIAL..widget::SPECIAL + 2], [0, 0]);
                let input = x[widget::INPUTS];
                let want = if input == 0 { h1hud::STATIC_SHADER } else { h1hud::METER_SHADER };
                assert_eq!(shader, want, "{name}: {bitmap}");
                inputs.push(input);
                eprintln!(
                    "{name}: {bitmap} seq {} input {input} anchor {} offset {:?}",
                    x[widget::SEQUENCES],
                    x[widget::ANCHOR],
                    hh::offset(x, widget::OFFSETS[0])
                );
            }
            assert!(inputs.contains(&hh::INPUT_SHIELD), "{name}: a shield meter");
            assert!(inputs.contains(&hh::INPUT_BODY), "{name}: a health meter");
            // Halo 1's grenade HUD per grenade type (frag, plasma), each
            // background and total background.
            assert_eq!(grenade_widgets, 4, "{name}: grenade widgets");
            // Every sprite carries its element colour's alpha: the grenade
            // outlines (0xdc, byte alpha, scaled in place) and the motion
            // sensor background (0x28, DXT, decoded and rewritten as
            // linear A8R8G8B8).
            assert!(!sensor.is_empty(), "{name}: the motion sensor background carries 0x28");
            for (id, source, seq) in &sensor {
                let (copy, source_mean) = sensor_alpha(&h1, &mut out, built.clone(), *id, source, *seq);
                eprintln!(
                    "{name}: {source} seq {seq}: mean alpha in the circle {copy:.2}, source {source_mean:.2}, want {:.2}",
                    source_mean * f64::from(0x28) / 255.0
                );
                assert!(source_mean > 64.0, "{source}: the source circle is mostly opaque");
                let want = source_mean * f64::from(0x28) / 255.0;
                assert!((copy - want).abs() < 1.0, "{source}: {copy:.2} against {want:.2}");
            }
            eprintln!("{name}: colour alphas {alphas:?}");
        }
        // Every converted Halo 1 weapon with a wphi names its Halo 1 HUD at
        // +0x2b0: Halo 1's bitmaps, a centred crosshair, meters on the
        // Halo 2 inputs h1hud names.
        let h1_weapons: std::collections::HashSet<String> = h1
            .tags()
            .iter()
            .filter(|t| t.group == h1map::Group::new(b"weap"))
            .filter(|t| {
                let b = h1.tag_data(t).unwrap();
                h1objects::h1::reference(&h1, b, 0x480).is_some()
            })
            .map(|t| t.name.clone())
            .collect();
        let mut weapons = 0;
        for t in out.tags().iter().filter(|t| {
            t.group == FourCC(*b"weap")
                && t.location.is_some()
                && t.name.as_ref().is_some_and(|n| h1_weapons.contains(n))
        }) {
            let wname = t.name.clone().unwrap();
            let weap = bytes(t.location.unwrap().address, 0x2b8);
            let id = TagId(w(&weap, 0x2b4));
            let (grp, name) = &names[&id];
            assert_eq!(*grp, FourCC(*b"nhdt"), "{wname}");
            assert!(name.ends_with(h1hud::WEAPON_NHDT_SUFFIX), "{wname}: {name}");
            let root = bytes(out.tag(id).unwrap().location.unwrap().address, hh::NHDT_ROOT);
            let n = w(&root, hh::NHDT_BITMAP_WIDGETS) as usize;
            let widgets = bytes(w(&root, hh::NHDT_BITMAP_WIDGETS + 4), n * hh::BITMAP_WIDGET_SIZE);
            let mut centred = 0;
            for x in widgets.as_chunks::<{ hh::BITMAP_WIDGET_SIZE }>().0 {
                let (bg, bitmap) = names[&TagId(w(x, widget::BITMAP + 4))].clone();
                let (sg, _) = names[&TagId(w(x, widget::SHADER + 4))].clone();
                assert_eq!(bg, FourCC(*b"bitm"));
                assert_eq!(sg, FourCC(*b"shad"));
                assert!(h1_bitmaps.contains(&bitmap), "{name}: {bitmap} is Halo 1's");
                let input = x[widget::INPUTS];
                assert!([0, 0x31, 0x36, 0x38].contains(&input), "{name}: input {input}");
                if x[widget::ANCHOR] == h1hud::Anchor::Centre as u8 {
                    centred += 1;
                }
            }
            eprintln!("{wname}: {name}, {n} widgets, {centred} centred");
            weapons += 1;
        }
        assert!(weapons >= 5, "{weapons} Halo 1 weapons with Halo 1 HUDs");
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
            {
            let templates = Base::read(&mut base).unwrap();
            let mut objects = Objects::new(&h1, &templates);
            h1convert::convert::halo1::prepare_halo2(&h1, sha, &mut base, &mut objects).unwrap()
        }
        };
        let mopp = structure_mopp(&p.collision).unwrap();
        let fresh = p.build_fresh(&mut base, &mopp).unwrap();
        // The fresh map is built from scratch on a new index, so the swaps
        // are named by its own datums (`Fresh::precision`), as the importer
        // writes them (`manifest_with`).
        let manifest = p.manifest_fresh(&fresh);
        let mut out = Map::from_reader(Cursor::new(fresh.bytes)).unwrap();
        if sandbox == Sandbox::Halo2 {
            // The precision swap: the SMG and the battle rifle collections,
            // both in the fresh map (the BR kept by the prune), the same
            // tags the plan swapped on the base, and the manifest records
            // it.
            let planned = &p.applied.precision;
            let swaps = &fresh.precision;
            assert_eq!(planned.len(), 1, "{planned:?}");
            assert_eq!(swaps.len(), 1, "{swaps:?}");
            for (id, was) in [(swaps[0].0, planned[0].0), (swaps[0].1, planned[0].1)] {
                let t = out.tag(id).unwrap().clone();
                assert_eq!(t.group, FourCC(*b"itmc"));
                let base_name = base.tag(was).unwrap().name.clone();
                assert_eq!(t.name, base_name, "the swap names the planned collection");
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
        if sandbox == Sandbox::Halo1 {
            // The HUD pair (docs/game/hud.md): the player biped, its Halo 1
            // HUD, and its Halo 2 HUD with the health strip, kept by the
            // prune though only the pair names it, in the manifest by the
            // fresh map's datums.
            let name = |id: TagId| {
                let t = out.tag(id).unwrap();
                (t.group, t.name.clone().unwrap_or_default())
            };
            // One pair for the player biped, one per Halo 1 weapon whose
            // Halo 2 counterpart has a HUD: its Halo 1 HUD and the
            // counterpart's.
            let bipeds: Vec<_> =
                fresh.hud.iter().filter(|p| name(p[0]).0 == FourCC(*b"bipd")).collect();
            assert_eq!(bipeds.len(), 1, "{:?}", fresh.hud);
            for [owner, halo1, halo2] in fresh.hud.iter().filter(|p| name(p[0]).0 != FourCC(*b"bipd")) {
                assert_eq!(name(*owner).0, FourCC(*b"weap"));
                assert!(name(*halo1).1.ends_with(h1objects::h1hud::WEAPON_NHDT_SUFFIX));
                assert_eq!(name(*halo2).0, FourCC(*b"nhdt"));
                assert!(name(*halo2).1.starts_with("ui\\hud\\"), "{}", name(*halo2).1);
                eprintln!("hud: {} -> {} / {}", name(*owner).1, name(*halo1).1, name(*halo2).1);
            }
            assert!(fresh.hud.len() > 1, "weapon pairs too");
            let [owner, halo1, halo2] = *bipeds[0];
            let (g1, n1) = name(halo1);
            let (g2, n2) = name(halo2);
            assert_eq!(g1, FourCC(*b"nhdt"));
            assert_eq!(g2, FourCC(*b"nhdt"));
            assert!(n1.ends_with(h1objects::h1hud::NHDT_SUFFIX), "{n1}");
            assert!(n2.ends_with(h1objects::health_hud::NHDT_SUFFIX), "{n2}");
            assert_eq!(
                manifest.variants[0].hud,
                fresh
                    .hud
                    .iter()
                    .map(|[o, a, b]| content::HudSwap {
                        owner: o.0,
                        halo1: a.0,
                        halo2: b.0
                    })
                    .collect::<Vec<_>>()
            );
            eprintln!("hud: {} -> {n1} / {n2}", name(owner).1);
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
        // Read through the whole metadata: a fresh map may place a tag's
        // blocks away from its root (shared or rehomed blocks).
        let mem = memory(&mut out);
        let w = |a: u32| {
            let (start, b) = mem.region(a).expect("address in the metadata");
            let o = (a - start) as usize;
            u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
        };
        for (id, name) in &modes {
            let at = out.tag(*id).unwrap().location.unwrap().address;
            let (count, start) = (w(at + 0x60) as usize, w(at + 0x64));
            for k in 0..count {
                let shader = w(start + 0x20 * k as u32 + 0xc);
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

/// The word at virtual address `a`.
/// The mean alpha, over the circle inscribed in the sprite sequence `seq`
/// draws, of the built map's bitmap `id` (decoded from the map's bytes,
/// as the game reads it) and of the Halo 1 bitmap `source` it copies.
fn sensor_alpha(
    h1: &H1Map,
    out: &mut Map<Cursor<Vec<u8>>>,
    built: std::sync::Arc<[u8]>,
    id: TagId,
    source: &str,
    seq: u8,
) -> (f64, f64) {
    let t = h1.tags().iter().find(|t| t.name == source).unwrap().clone();
    let bm = h1map::bitmap::Bitmap::read(h1, &t).unwrap();
    let s = &bm.sequences[usize::from(seq)];
    let (image, bounds) = match s.sprites.first() {
        Some(p) => (p.image as usize, p.bounds),
        None => (s.first_image as usize, [0.0, 1.0, 0.0, 1.0]),
    };
    let src = h1bitmaps::decode::stack(
        &h1bitmaps::decode_image(&bm.images[image], bm.pixels(h1, image).unwrap()).unwrap(),
        0,
    )
    .unwrap();
    let tags::TagData::Bitmap(copy) = tags::DecodedTag::read(out, id).unwrap().data else {
        panic!("{source}: the copy is not a bitmap");
    };
    let mut files = assets::ResourceFiles::new();
    files.set_bytes(map::ResourceFile::Local, built);
    let level = assets::decode_image(&copy.images[image], &mut files)
        .unwrap()
        .levels
        .into_iter()
        .find(|l| l.face == 0 && l.level == 0)
        .unwrap();
    let (w, h) = (src.width as usize, src.height as usize);
    assert_eq!((level.width as usize, level.height as usize), (w, h), "{source}: the copy's size");
    let [l, r, top, b] = bounds.map(f64::from);
    let (cx, cy) = ((l + r) / 2.0 * w as f64, (top + b) / 2.0 * h as f64);
    let radius = ((r - l) * w as f64).min((b - top) * h as f64) / 2.0;
    let (mut copy_sum, mut src_sum, mut n) = (0.0, 0.0, 0.0);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
            if dx * dx + dy * dy <= radius * radius {
                copy_sum += f64::from(level.rgba[(y * w + x) * 4 + 3]);
                src_sum += f64::from(src.rgba[(y * w + x) * 4 + 3]);
                n += 1.0;
            }
        }
    }
    assert!(n > 0.0, "{source}: an empty circle");
    (copy_sum / n, src_sum / n)
}

fn rw(mem: &tags::schema::Memory, a: u32) -> u32 {
    mem.slice(a, 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .unwrap_or_else(|| panic!("{a:#x} is outside the metadata"))
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
                {
                let templates = Base::read(&mut base).unwrap();
                let mut objects = Objects::new(&h1, &templates);
                h1convert::convert::halo1::prepare_halo2(&h1, sha, &mut base, &mut objects)
            }
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

/// Every Halo 1 multiplayer map in both sandboxes through the importer's
/// path (`prepare_halo1` or `prepare`, then `build_fresh`, as `h2 import
/// halo1`): each builds (the Halo 1 objects land at the datums the
/// placements name, their sounds installed after them), and in the Halo 1
/// sandbox every attachment of every vehicle names a tag of its group in
/// the built map.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_map_builds_in_both_sandboxes_with_the_objects_sounds() {
    let mut maps = 0;
    let mut attachments = 0;
    every_fresh_map(|name, sandbox, _, fresh| {
        maps += 1;
        if sandbox != Sandbox::Halo1 {
            return;
        }
        let m = Map::from_reader(Cursor::new(fresh.bytes.clone())).unwrap();
        let d = tags::defs::Defs::builtin().group(FourCC(*b"vehi")).unwrap();
        let tags: Vec<(TagId, FourCC)> = m.tags().iter().map(|t| (t.id, t.group)).collect();
        let mut shared = tags::repo::extract::SharedFiles::default();
        let mut src = tags::repo::MapSource::new(
            tags::schema::Schema::builtin(),
            Map::from_reader(Cursor::new(fresh.bytes.clone())).unwrap(),
            &mut shared,
        )
        .unwrap();
        for (id, g) in &tags {
            if *g != FourCC(*b"vehi") {
                continue;
            }
            // The base's own vehicles point into the shared maps; the
            // converted ones are whole in this map.
            let Ok(Ok(tag)) = src.extract(*id) else { continue };
            let r = tags::defs::decode(d, &tag).unwrap();
            let Some(tags::defs::Val::Block(att)) = r.at("unit.obje.attachments") else {
                panic!("{name}: attachments")
            };
            for a in att {
                let Some(tags::defs::Val::Ref { group, target }) = a.at("type") else { panic!() };
                // A loose tag names its target by group and path.
                let tags::defs::Target::Key(k) = target else {
                    panic!("{name}: attachment {target:?}")
                };
                assert_eq!(k.group.as_u32(), *group, "{name}");
                assert!(
                    m.tags().iter().any(|t| t.group == k.group && t.name.as_deref() == Some(k.path.as_str())),
                    "{name}: {k:?} not in the map"
                );
                attachments += 1;
            }
        }
        println!("{name}: built in the Halo 1 sandbox");
    });
    println!("{maps} builds, {attachments} vehicle attachments resolved");
    assert!(maps > 0);
}

/// Prints Halo 2's weapon HUD widgets on coagulation (inputs, state flags,
/// anchor, special type): the evidence for the inputs and states the
/// converted Halo 1 weapon HUDs use.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn print_halo2_weapon_hud_widgets() {
    let mut base_map = h2_map("coagulation");
    let base = Base::read(&mut base_map).unwrap();
    let strings = base_map.strings().to_vec();
    for (id, g, name, _) in &base.tags {
        if *g != FourCC(*b"nhdt") {
            continue;
        }
        let root = base.head(*id, 0x28).unwrap();
        let mut i = 0;
        while let Some(w) = base.element(&root, 8, 0x64, i).unwrap() {
            let bitmap = base.reference(&w, 0x20).and_then(|b| base.name(b)).map_or("-", |n| n.1);
            let shader = base.reference(&w, 0x28).and_then(|b| base.name(b)).map_or("-", |n| n.1);
            eprintln!(
                "{name} #{i}: inputs {:?} state {:02x?} anchor {} flags {:x} seq {:?} offset {:?} special {} {bitmap} {shader}",
                &w[4..8],
                &w[8..0x1c],
                u16::from_le_bytes([w[0x1c], w[0x1d]]),
                u16::from_le_bytes([w[0x1e], w[0x1f]]),
                &w[0x30..0x34],
                (i16::from_le_bytes([w[0x34], w[0x35]]), i16::from_le_bytes([w[0x36], w[0x37]])),
                u16::from_le_bytes([w[0x60], w[0x61]]),
            );
            i += 1;
        }
        let mut i = 0;
        while let Some(w) = base.element(&root, 0x10, 0x54, i).unwrap() {
            let sid = u32::from_le_bytes(w[0..4].try_into().unwrap());
            let label = strings.get((sid & 0xff_ffff) as usize).cloned().flatten().unwrap_or_default();
            let shader = base.reference(&w, 0x20).and_then(|b| base.name(b)).map_or("-", |n| n.1);
            let sid28 = u32::from_le_bytes(w[0x28..0x2c].try_into().unwrap());
            let text = strings.get((sid28 & 0xff_ffff) as usize).cloned().flatten().unwrap_or_default();
            eprintln!(
                "{name} text #{i} {label}: inputs {:?} state {:02x?} anchor {} shader {shader} string {sid28:08x} {text:?}\n  {:02x?}\n  {:02x?}",
                &w[4..8],
                &w[8..0x1c],
                u16::from_le_bytes([w[0x1c], w[0x1d]]),
                &w[0x1c..0x38],
                &w[0x38..0x54],
            );
            i += 1;
        }
    }
}

/// Halo 1's scenery in both sandboxes' fresh maps: the scenario's scenery
/// palette (`+0x58`) is the converted `scen`s, each with a render model,
/// and every Halo 1 placement whose palette entry converted is placed, at
/// Halo 1's position, on its entry.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_map_places_halo1_s_scenery_in_both_sandboxes() {
    let mut bad = Vec::new();
    let mut total = 0;
    every_fresh_map(|name, sandbox, p, fresh| {
        let sc = p.scenery.as_ref().expect("scenery planned");
        let mut out = Map::from_reader(Cursor::new(fresh.bytes.clone())).unwrap();
        let ctx = tags::object::Context::read(&mut out).unwrap();
        let s = Scenario::read(&mut out).unwrap();
        let b = s.block(BlockKind::Scenery);
        println!(
            "{name} {sandbox:?}: {} scenery placements on {} palette entries ({} left out)",
            b.len(),
            b.palette.len(),
            sc.dropped
        );
        total += b.len();
        for u in p.applied.unresolved.iter().filter(|u| u.starts_with("scenery")) {
            println!("{name} {sandbox:?}: not placed: {u}");
        }
        if b.len() != sc.placements.len() || b.palette.len() != sc.palette.len() {
            bad.push(format!(
                "{name} {sandbox:?}: {} of {} placements, {} of {} palette entries",
                b.len(),
                sc.placements.len(),
                b.palette.len(),
                sc.palette.len()
            ));
            return;
        }
        for (k, r) in b.palette.iter().enumerate() {
            let ok = r.group == FourCC(*b"scen")
                && ctx
                    .resolve_object(r.id)
                    .is_ok_and(|o| o.model.as_ref().is_some_and(|m| m.render_model.is_some()));
            if !ok {
                bad.push(format!("{name} {sandbox:?}: palette {k} {r:?}"));
            }
        }
        for i in 0..b.len() {
            let e = b.entry(i).unwrap();
            let want = &sc.placements[i];
            if e[..0x20] != want[..0x20] {
                bad.push(format!("{name} {sandbox:?}: placement {i} moved"));
            }
        }
    });
    assert!(total > 0, "no scenery placed");
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Every `hlmt` of both sandboxes' fresh maps holds the materials the
/// engine reads ([`h1objects::verify::hlmt_materials`]), beyond what
/// retail's own base map already breaks: a ray's hit on an object reads
/// its `hlmt` material of the hit's index with no bound check
/// (`0x000d8ae0`), and the converted objects' empty blocks faulted at
/// `0x000d8b33` in the Halo 1 sandbox.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_hlmt_holds_its_materials_in_both_sandboxes() {
    let mut retail = std::collections::HashMap::new();
    let mut bad = Vec::new();
    let mut maps = 0;
    every_fresh_map(|name, sandbox, _, fresh| {
        let base_name = h1convert::convert::MAPS
            .iter()
            .find(|m| m.0 == name)
            .unwrap()
            .2;
        let known: &Vec<String> = retail.entry(base_name).or_insert_with(|| {
            h1objects::verify::hlmt_materials(&mut h2_map(base_name)).unwrap()
        });
        let mut out = Map::from_reader(Cursor::new(fresh.bytes.clone())).unwrap();
        let lines = h1objects::verify::hlmt_materials(&mut out).unwrap();
        let hlmts = out.tags().iter().filter(|t| t.group == FourCC(*b"hlmt")).count();
        let new: Vec<_> = lines.into_iter().filter(|l| !known.contains(l)).collect();
        println!("{name} {sandbox:?}: {hlmts} hlmts, {} problems", new.len());
        bad.extend(new.into_iter().map(|l| format!("{name} {sandbox:?}: {l}")));
        maps += 1;
    });
    for (base, lines) in &retail {
        println!("retail {base}: {lines:#?}");
    }
    assert!(maps > 0);
    assert!(bad.is_empty(), "{bad:#?}");
}

/// The modes, classes and types of the graph `jmad` (at `a`): `jmad +0x34`
/// modes (`0x14`), each `+4` classes (`0x14`), each `+4` types (`0x34`),
/// the name first in each.
fn jmad_tree(
    mem: &tags::schema::Memory,
    s: &dyn Fn(u32) -> String,
    a: u32,
) -> BTreeSet<(String, String, String)> {
    let mut out = BTreeSet::new();
    let (mn, mf) = (rw(mem, a + 0x34), rw(mem, a + 0x38));
    for i in 0..mn {
        let me = mf + i * 0x14;
        let (cn, cf) = (rw(mem, me + 4), rw(mem, me + 8));
        for c in 0..cn {
            let ce = cf + c * 0x14;
            let (tn, tf) = (rw(mem, ce + 4), rw(mem, ce + 8));
            for k in 0..tn {
                out.insert((s(rw(mem, me)), s(rw(mem, ce)), s(rw(mem, tf + k * 0x34))));
            }
        }
    }
    out
}

/// In the Halo 1 sandbox every Halo 1 hand weapon names the weapon class
/// and type (`weap +0x288`, `+0x28c`) the converted player biped's graph
/// holds it by in mode `combat`: a unit takes a weapon (picked up or
/// given at spawn) only when its graph has an `idle` for them
/// (`0x000cd6a0` -> `0x0010fcd0`), and the converted weapons named none,
/// so the player could pick nothing up.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_halo1_weapon_is_held_by_the_halo1_player_graph() {
    let mut bad = Vec::new();
    let mut checked = 0;
    every_fresh_map(|name, sandbox, _, fresh| {
        if sandbox != Sandbox::Halo1 {
            return;
        }
        let mut out = Map::from_reader(Cursor::new(fresh.bytes.clone())).unwrap();
        let mem = memory(&mut out);
        let strings = out.strings().to_vec();
        let s = |v: u32| -> String {
            if v == 0 {
                return String::new();
            }
            strings
                .get((v & 0xff_ffff) as usize)
                .cloned()
                .flatten()
                .unwrap_or_default()
        };
        let tags = out.tags().to_vec();
        let at = |id: u32| {
            tags.iter()
                .find(|t| t.id.0 == id && id != u32::MAX)
                .and_then(|t| t.location.map(|l| l.address))
        };
        // The converted player bipeds: Halo 1 paths, not `objects\...`.
        let graphs: Vec<_> = tags
            .iter()
            .filter(|t| t.group == FourCC(*b"bipd"))
            .filter(|t| !t.name.as_deref().unwrap_or("").starts_with("objects\\"))
            .filter_map(|t| {
                let h = at(rw(&mem, t.location?.address + 0x38))?;
                let j = at(rw(&mem, h + 0x14))?;
                Some((t.name.clone().unwrap_or_default(), jmad_tree(&mem, &s, j)))
            })
            .collect();
        assert!(!graphs.is_empty(), "{name}: no converted biped");
        // The weapon types some Halo 1 graph of the map holds (the third
        // word of `<unit> <class> <type> <action>`): a weapon whose label
        // none holds (Derelict's flamethrower, `ft`) has no class to name.
        let h1 = h1_map(name).unwrap();
        let mut held = BTreeSet::new();
        for t in h1.tags().iter().filter(|t| t.group.to_string() == "antr") {
            for n in h1models::graph_names(&h1, t).unwrap() {
                let n = h1models::names::map_name(&n);
                if n.class != "any" && n.kind != "any" {
                    held.insert(n.kind);
                }
            }
        }
        let labels: std::collections::HashMap<String, String> = h1
            .tags()
            .iter()
            .filter(|t| t.group.to_string() == "weap")
            .map(|t| {
                let w = h1objects::h1::Weapon::read(&h1, t).unwrap();
                (t.name.clone(), w.label)
            })
            .collect();
        for t in tags.iter().filter(|t| t.group == FourCC(*b"weap")) {
            let n = t.name.as_deref().unwrap_or("");
            if !n.starts_with("weapons\\") {
                continue;
            }
            let Some(l) = t.location else { continue };
            let (class, kind) = (s(rw(&mem, l.address + 0x288)), s(rw(&mem, l.address + 0x28c)));
            if !held.contains(&labels[n]) {
                println!("{name}: {n}: label {:?} held by no Halo 1 graph", labels[n]);
                if !(class.is_empty() && kind.is_empty()) {
                    bad.push(format!("{name}: {n} names ({class:?}, {kind:?}) for a label no graph holds"));
                }
                continue;
            }
            checked += 1;
            println!("{name}: {n}: {class:?} {kind:?}");
            for (b, tree) in &graphs {
                if !tree.contains(&("combat".into(), class.clone(), kind.clone())) {
                    bad.push(format!("{name}: {n} ({class:?}, {kind:?}) not in {b}'s combat"));
                }
            }
        }
    });
    assert!(checked > 0);
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Halo 2's motion sensor blips centre on its own table (`.data`
/// `0x47ff40`, `boot::hud_swap::SENSOR_CENTRES`): (41, -42) fullscreen,
/// (31, -31) half and quarter screen, the centre of the retail 83 x 83
/// `motion_sensor` background (masterchief widget 5, offset (-1, -84)).
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn halo2s_motion_sensor_table() {
    let path = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| {
            e.path
                .rsplit(['/', '\\'])
                .next()
                .is_some_and(|n| n.eq_ignore_ascii_case("default.xbe"))
        })
        .unwrap();
    let bytes = iso.read(&e).unwrap();
    let x = xbe::Xbe::parse(&bytes).unwrap();
    assert_eq!(x.section_containing(0x0047_ff40).unwrap().name, ".data");
    let t = x.bytes_at(0x0047_ff40, 16).unwrap();
    let words: Vec<i16> = t.chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    assert_eq!(words, [41, -42, 31, -31, 31, -31, 0, 0]);
}
