//! Exercise the installed player's graph through the same sorted tables
//! and wildcard order as the action, overlay and damage searches.
use super::*;
use h1models::h1::{AnimType, Graph};
use tags::schema::Memory;

struct Lookup<'a> {
    mem: &'a Memory,
    strings: &'a [Option<String>],
    graph: u32,
}

impl Lookup<'_> {
    fn named(&self, list: u32, stride: u32, name: &str) -> Option<u32> {
        bsearch(self.mem, list, stride, sid_of(self.strings, name)?)
    }

    fn leaf(&self, path: [&str; 3], list: u32, stride: u32, key: &str) -> Option<u32> {
        let [mode, class, kind] = path;
        // 0x2963f0 enumerates the eight exact/any combinations, with
        // type changing first, class second and mode last.
        for m in [mode, "any"] {
            let Some(m) = self.named(self.graph + 0x34, 20, m) else {
                continue;
            };
            for c in [class, "any"] {
                let Some(c) = self.named(m + 4, 20, c) else {
                    continue;
                };
                for t in [kind, "any"] {
                    if let Some(e) = self
                        .named(c + 4, 52, t)
                        .and_then(|t| self.named(t + list, stride, key))
                    {
                        return Some(e);
                    }
                }
            }
        }
        None
    }

    fn clip(&self, path: [&str; 3], list: u32, key: &str) -> Option<i16> {
        let leaf = self.leaf(path, list, 8, key)?;
        let reference = rw(self.mem, leaf + 4);
        assert_eq!(reference as i16, -1, "{path:?} {key} should be local");
        Some((reference >> 16) as i16)
    }

    fn check(&self, path: [&str; 3], list: u32, key: &str, slots: &[i16], k: usize) {
        let want = slots.get(k).copied().filter(|&i| i != -1);
        assert_eq!(
            self.clip(path, list, key),
            want,
            "{path:?} {key} (slot {k})"
        );
    }
}

fn check_player(source: &Graph, out: &Lookup<'_>, class: &str, kind: &str) {
    for (label, mode) in [("stand", "combat"), ("crouch", "crouch")] {
        let u = source.units.iter().find(|u| u.label == label).unwrap();
        let c = u.classes.iter().find(|c| c.label == class).unwrap();
        let path = [mode, class, kind];
        for (slot, key) in [(0, "airborne_dead"), (1, "landing_dead")] {
            out.check(path, 4, key, &u.slots, slot);
        }
        for (slot, key) in [(5, "push"), (9, "look")] {
            out.check(path, 12, key, &u.slots, slot);
        }
        for (slot, key) in [
            (0, "idle"),
            (2, "turn_left"),
            (3, "turn_right"),
            (8, "move_front"),
            (9, "move_back"),
            (10, "move_left"),
            (11, "move_right"),
            (8, "walk_front"),
            (9, "walk_back"),
            (10, "walk_left"),
            (11, "walk_right"),
            (16, "airborne"),
            (17, "land_soft"),
            (18, "land_hard"),
            (20, "throw_grenade"),
            (35, "stunned_front"),
            (36, "stunned_back"),
            (37, "stunned_left"),
            (38, "stunned_right"),
        ] {
            out.check(path, 4, key, &c.slots, slot);
        }
        out.check(path, 12, "ready", &c.slots, 23);
        for (slot, key) in [
            (25, "aim_still_up"),
            (25, "aim_still_down"),
            (26, "aim_move_up"),
            (26, "aim_move_down"),
        ] {
            out.check(path, 12, key, &c.slots, slot);
            if let Some(i) = out.clip(path, 12, key) {
                let record = rw(out.mem, out.graph + 0x30) + i as u32 * 108;
                let screen = out.mem.slice(record + 0x12, 1).unwrap()[0] as u32;
                assert!(screen < rw(out.mem, out.graph + 0x24));
                let addr = rw(out.mem, out.graph + 0x28) + screen * 28 + 4;
                assert_eq!(
                    out.mem.slice(addr, 24).unwrap(),
                    &c.screen.bytes(),
                    "{path:?} {key}"
                );
            }
        }
        if kind != "any" {
            let slots = &c.types.iter().find(|(t, _)| t == kind).unwrap().1;
            for (slot, key) in [
                "reload_1",
                "reload_2",
                "chamber_1",
                "chamber_2",
                "fire_1",
                "fire_2",
                "charged_1",
                "charged_2",
                "melee_strike_1",
                "overheat",
            ]
            .iter()
            .enumerate()
            {
                out.check(path, 12, key, slots, slot);
            }
            out.check(path, 12, "melee", slots, 8);
        }
        for (k, &slot) in source.damage_slots.iter().enumerate() {
            let key = ["s_ping", "h_ping", "s_kill", "h_kill"][k / 44];
            let index = out
                .leaf(path, 20, 12, key)
                .map(|entry| {
                    let dirs = rw(out.mem, entry + 8);
                    let regions = rw(out.mem, dirs + (k as u32 / 11 % 4) * 8 + 4);
                    let reference = rw(out.mem, regions + (k as u32 % 11) * 4);
                    if slot >= 0 {
                        assert_eq!(reference as i16, -1);
                    }
                    (reference >> 16) as i16
                })
                .unwrap_or(-1);
            assert_eq!(index, slot, "{path:?} {key} slot {k}");
        }
    }
}

fn check_loop_frames(source: &Graph, out: &Lookup<'_>) {
    let records = rw(out.mem, out.graph + 0x30);
    for (i, a) in source.animations.iter().enumerate() {
        let start = (rw(out.mem, records + i as u32 * 108 + 0x2c) >> 16) as u16;
        assert_eq!(
            start,
            ((a.loop_frame as i16).max(0) as u16).min(a.frame_count - 1),
            "{}",
            a.name
        );
    }
}

fn check_first_person(source: &Graph, out: &Lookup<'_>) {
    check_loop_frames(source, out);
    let path = ["any"; 3];
    for (slot, &i) in source.first_person_slots.iter().enumerate() {
        let key = match slot {
            13 => "melee_strike_1".to_string(),
            _ => h1models::h1::FIRST_PERSON_SLOTS[slot].replace('-', "_"),
        };
        if i == -1 {
            assert!(
                out.clip(path, 4, &key).is_none(),
                "empty FP slot {slot}: {key}"
            );
            assert!(
                out.clip(path, 12, &key).is_none(),
                "empty FP slot {slot}: {key}"
            );
            continue;
        }
        let list = if source.animations[i as usize].kind == AnimType::Overlay {
            12
        } else {
            4
        };
        out.check(path, list, &key, &source.first_person_slots, slot);
    }
    if source.first_person_slots.get(23).is_some_and(|&i| i >= 0) {
        for (slot, key) in [
            (23, "reload_enter"),
            (7, "reload_continue_empty"),
            (8, "reload_continue_full"),
            (25, "reload_exit"),
        ] {
            out.check(path, 4, key, &source.first_person_slots, slot);
        }
    }
    for (slot, key) in [(2, "fire_1"), (13, "melee"), (22, "ready_overheated")] {
        out.check(path, 4, key, &source.first_person_slots, slot);
    }
}

#[test]
#[ignore = "reads Blood Gulch from H1_ISO and Coagulation from H2_DISC"]
fn bloodgulch_player_animation_slots() {
    let h1 = bloodgulch();
    let mut base = h2_map("coagulation");
    let templates = Base::read(&mut base).unwrap();
    let mut objects = Objects::new(&h1, &templates);
    let p = prepare_halo1(
        &h1,
        content::Sha256::of(b"bloodgulch"),
        &mut base,
        &mut objects,
    )
    .unwrap();
    let built = p
        .build_fresh(&mut base, &structure_mopp(&p.collision).unwrap())
        .unwrap();
    let mut out = Map::from_reader(Cursor::new(built.bytes)).unwrap();
    let mem = memory(&mut out);
    let strings = out.strings();
    let addr = |id: u32| out.tag(TagId(id)).unwrap().location.unwrap().address;
    let globals = out.globals().unwrap().location.unwrap().address;
    let reps = rw(&mem, globals + PLAYER_REPRESENTATION as u32 + 4);
    let player = |e: u32| {
        rw(
            &mem,
            reps + e * PLAYER_REPRESENTATION_SIZE as u32 + REPRESENTATION_UNIT as u32 + 4,
        )
    };
    assert_eq!(player(2u32), player(3u32));
    let biped = out.tag(TagId(player(2))).unwrap();
    let h1_tag = h1
        .find(h1map::Group::new(b"bipd"), biped.name.as_deref().unwrap())
        .unwrap();
    let unit = h1objects::h1::Unit::read(&h1, h1_tag).unwrap();
    let source = |id| Graph::read(&h1, h1.tag(id).unwrap().data_address).unwrap();
    let g = source(unit.object.animation.unwrap());
    let model = addr(rw(&mem, addr(player(2)) + 0x38));
    let graph = Lookup {
        mem: &mem,
        strings,
        graph: addr(rw(&mem, model + 0x14)),
    };
    check_loop_frames(&g, &graph);
    check_player(&g, &graph, "unarmed", "any");
    for (class, kind) in [("support", "ft"), ("rifle", "f"), ("pistol", "b")] {
        check_player(&g, &graph, class, kind);
    }
    for (src, mode, class) in [
        ("W-driver", "warthog_d", "unarmed"),
        ("W-gunner", "warthog_g", "fixed"),
    ] {
        let seat = g.units.iter().find(|u| u.label == src).unwrap();
        let path = [mode, class, "any"];
        for (k, action) in [(7, "enter"), (8, "exit")] {
            graph.check(path, 4, action, &seat.slots, k);
        }
        for (k, overlay) in [
            (2, "acc_front_back"),
            (3, "acc_left_right"),
            (4, "acc_up_down"),
            (9, "look"),
        ] {
            graph.check(path, 12, overlay, &seat.slots, k);
        }
        let class = seat.classes.iter().find(|c| c.label == class).unwrap();
        graph.check(path, 12, "aim_still_up", &class.slots, 25);
    }
    let records = rw(&mem, graph.graph + 0x30);
    for (i, a) in g.animations.iter().enumerate() {
        let record = records + i as u32 * 108;
        let links = rw(&mem, record + 0x30);
        assert_eq!(
            links as i16,
            if a.first_permutation == i as i16 {
                -1
            } else {
                a.first_permutation
            }
        );
        assert_eq!((links >> 16) as i16, a.next_permutation);
        assert_eq!(rw(&mem, record + 0x1c), a.cumulative_weight.to_bits());
        let mut want: Vec<_> = a
            .key_frames
            .iter()
            .enumerate()
            .filter(|(_, frame)| **frame > 0)
            .map(|(event, &frame)| ((frame as u32) << 16) | event as u32)
            .collect();
        if a.foot_frames != [0, 0] {
            for (foot, frame) in a.foot_frames.into_iter().enumerate() {
                want.push((u32::from(frame) << 16) | (2 + foot as u32));
            }
        }
        let events = rw(&mem, record + 0x50);
        let got: Vec<_> = (0..rw(&mem, record + 0x4c))
            .map(|i| rw(&mem, events + 4 * i))
            .collect();
        assert_eq!(got, want, "{}", a.name);
    }
    let patrol = g
        .units
        .iter()
        .find(|u| u.label == "alert")
        .unwrap()
        .classes
        .iter()
        .find(|c| c.label == "rifle")
        .unwrap();
    graph.check(["patrol", "rifle", "ar"], 4, "move_front", &patrol.slots, 8);
    graph.check(
        ["patrol", "rifle", "ar"],
        12,
        "aim_move_up",
        &patrol.slots,
        26,
    );

    let mut classes = BTreeSet::new();
    for t in out.tags().iter().filter(|t| t.group == FourCC(*b"weap")) {
        let Some(h1_tag) = t
            .name
            .as_deref()
            .and_then(|name| h1.find(h1map::Group::new(b"weap"), name))
        else {
            continue;
        };
        let w = h1objects::h1::Weapon::read(&h1, h1_tag).unwrap();
        let Some(fp) = w.first_person_animations else {
            continue;
        };
        let at = t.location.unwrap().address;
        let label = |offset: u32| {
            strings[(rw(&mem, at + offset) & 0xffffff) as usize]
                .as_deref()
                .unwrap()
        };
        let (class, kind) = (label(0x288), label(0x28c));
        assert_eq!(kind, w.label);
        classes.insert((class, kind));
        check_player(&g, &graph, class, kind);
        let entries = rw(&mem, at + 0x2ac);
        assert!(rw(&mem, at + 0x2a8) > 0);
        check_first_person(
            &source(fp),
            &Lookup {
                graph: addr(rw(&mem, entries + 12)),
                ..graph
            },
        );
    }
    for pair in [
        ("rifle", "ar"),
        ("rifle", "pr"),
        ("rifle", "sg"),
        ("rifle", "sr"),
        ("pistol", "hp"),
        ("pistol", "pp"),
        ("pistol", "ne"),
        ("missile", "rl"),
    ] {
        assert!(classes.contains(&pair), "missing {pair:?}");
    }
}

/// The Halo 2 sandbox uses both retail player representations. Follow
/// each biped and compare each reachable action and overlay with the
/// source graph, resolving SIDs separately in the two maps.
#[test]
#[ignore = "reads Blood Gulch from H1_ISO and Coagulation from H2_DISC"]
fn bloodgulch_h2_player_graphs_match_coagulation() {
    fn player_paths(
        map: &mut Map<Cursor<Vec<u8>>>,
        rep: u32,
    ) -> (
        String,
        std::collections::BTreeMap<(String, String, String, u32, String), u32>,
    ) {
        let mem = memory(map);
        let at = |id: u32| map.tag(TagId(id)).unwrap().location.unwrap().address;
        let globals = map.globals().unwrap().location.unwrap().address;
        let reps = rw(&mem, globals + PLAYER_REPRESENTATION as u32 + 4);
        let bipd = at(rw(
            &mem,
            reps + rep * PLAYER_REPRESENTATION_SIZE as u32 + REPRESENTATION_UNIT as u32 + 4,
        ));
        let hlmt = at(rw(&mem, bipd + 0x38));
        let id = TagId(rw(&mem, hlmt + 0x14));
        let tag = map.tag(id).unwrap();
        let g = Lookup {
            mem: &mem,
            strings: map.strings(),
            graph: at(id.0),
        };
        let level = |block, stride: u32| {
            (0..rw(&mem, block))
                .map(|i| {
                    let entry = rw(&mem, block + 4) + i * stride;
                    let sid = rw(&mem, entry);
                    let name = g.strings[(sid & 0xffffff) as usize].clone().unwrap();
                    (name, entry)
                })
                .collect::<Vec<_>>()
        };
        let mut paths = std::collections::BTreeMap::new();
        for (mode, m) in level(g.graph + 0x34, 20) {
            for (class, c) in level(m + 4, 20) {
                for (kind, t) in level(c + 4, 52) {
                    for list in [4, 12] {
                        for (key, entry) in level(t + list, 8) {
                            assert_eq!(g.leaf([&mode, &class, &kind], list, 8, &key), Some(entry));
                            paths.insert(
                                (mode.clone(), class.clone(), kind.clone(), list, key),
                                rw(&mem, entry + 4),
                            );
                        }
                    }
                }
            }
        }
        assert!(!paths.is_empty());
        (tag.name.clone().unwrap(), paths)
    }

    let h1 = bloodgulch();
    let mut base = h2_map("coagulation");
    let templates = Base::read(&mut base).unwrap();
    let mut objects = Objects::new(&h1, &templates);
    let p = h1convert::convert::halo1::prepare_halo2(
        &h1,
        content::Sha256::of(b"bloodgulch"),
        &mut base,
        &mut objects,
    )
    .unwrap();
    let built = p
        .build_fresh(&mut base, &structure_mopp(&p.collision).unwrap())
        .unwrap();
    let mut out = Map::from_reader(Cursor::new(built.bytes)).unwrap();
    for rep in [2, 3] {
        assert_eq!(player_paths(&mut out, rep), player_paths(&mut base, rep));
    }
}
