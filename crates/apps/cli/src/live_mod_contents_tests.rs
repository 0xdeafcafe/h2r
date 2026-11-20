//! The contents report over invented censuses, and the walk over an
//! invented layout. No game data.

use std::sync::Arc;

use content::Sha256;
use tags::registry::{Field, Hint};

use super::*;
use crate::live_mod_features::TagPrint;

const RIFLE: &str = "objects\\weapons\\rifle\\battle_rifle\\battle_rifle";
const BULLET: &str = "objects\\weapons\\rifle\\battle_rifle\\projectiles\\battle_rifle_bullet";
const FUEL_ROD: &str = "objects\\weapons\\support_high\\fuel_rod\\projectiles\\fuel_rod";
const HOG: &str = "objects\\vehicles\\warthog\\warthog";
const GHOST: &str = "objects\\vehicles\\ghost\\ghost";
const BANSHEE: &str = "objects\\vehicles\\banshee\\banshee";

fn print(size: u32, address: u32, fill: u8) -> TagPrint {
    TagPrint {
        size,
        address,
        hash: Sha256([fill; 32]),
    }
}

fn key(g: &str, n: &str) -> (String, String) {
    (g.to_string(), n.to_string())
}

fn r(group: &str, name: &str) -> Val {
    Val::Ref(Some(key(group, name)))
}

fn leaf(label: &str, val: Val) -> Leaf {
    Leaf {
        label: label.into(),
        named: true,
        val,
    }
}

/// An invented level: a rifle firing bullets, a warthog with its model,
/// a shader with a bitmap, two warthogs placed, scripts.
fn base() -> Census {
    let mut c = Census::default();
    for (i, (g, n)) in [
        ("weap", RIFLE),
        ("proj", BULLET),
        ("proj", FUEL_ROD),
        ("vehi", HOG),
        ("vehi", GHOST),
        ("vehi", BANSHEE),
        ("hlmt", HOG),
        ("hlmt", GHOST),
        ("shad", "scenarios\\shaders\\rock"),
        ("bitm", "scenarios\\bitmaps\\rock"),
        ("bitm", "scenarios\\bitmaps\\snow"),
        ("sbsp", "scenarios\\multi\\x\\x"),
    ]
    .into_iter()
    .enumerate()
    {
        c.tags
            .insert(key(g, n), print(0x100, 0x1000 * (i as u32 + 1), i as u8));
    }
    c.fields.insert(
        key("weap", RIFLE),
        Fields::from([
            (
                "2d0.0/8c".into(),
                leaf("barrels[0].projectile", r("proj", BULLET)),
            ),
            (
                "!2c0.0/a".into(),
                leaf("magazines[0] magazine size", Val::Stat("36".into())),
            ),
            ("1c0".into(), leaf("+0x1c0", Val::Word(5))),
        ]),
    );
    c.fields.insert(
        key("vehi", HOG),
        Fields::from([("34".into(), leaf("model", r("hlmt", HOG)))]),
    );
    c.fields.insert(
        key("shad", "scenarios\\shaders\\rock"),
        Fields::from([(
            "20.0/0".into(),
            leaf("maps[0].bitmap", r("bitm", "scenarios\\bitmaps\\rock")),
        )]),
    );
    c.placed.insert(
        "vehicles",
        vec![(HOG.into(), [0, 0, 10]), (HOG.into(), [90, 0, 10])],
    );
    c.scenario = Fields::from([(
        "1e0#".into(),
        leaf("player_starting_locations", Val::Count(16)),
    )]);
    c.scripts = Some(Sha256([8; 32]));
    c
}

fn with(f: &dyn Fn(&mut Census)) -> Contents {
    let mut m = base();
    f(&mut m);
    contents(&base(), &m)
}

fn set(c: &mut Census, g: &str, n: &str, k: &str, v: Val) {
    c.fields
        .get_mut(&key(g, n))
        .unwrap()
        .get_mut(k)
        .unwrap()
        .val = v;
}

#[test]
fn the_same_map_contains_nothing_new() {
    assert!(contents(&base(), &base()).is_empty());
}

#[test]
fn a_barrel_naming_another_projectile_is_a_projectile_swap() {
    let c = with(&|m| set(m, "weap", RIFLE, "2d0.0/8c", r("proj", FUEL_ROD)));
    assert_eq!(
        c.swaps,
        [Swap {
            kind: "projectile".into(),
            tag: RIFLE.into(),
            field: "barrels[0].projectile".into(),
            from: Some(BULLET.into()),
            to: Some(FUEL_ROD.into()),
        }]
    );
    assert!(
        c.lines.contains(
            &"projectile swap: battle rifle fires fuel rod (was battle rifle bullet)".into()
        ),
        "{:?}",
        c.lines
    );
    assert!(
        c.lines.contains(&"weapons: 1 changed".into()),
        "{:?}",
        c.lines
    );
}

#[test]
fn model_shader_and_texture_swaps_say_what_they_swap() {
    let c = with(&|m| {
        set(m, "vehi", HOG, "34", r("hlmt", GHOST));
        set(
            m,
            "shad",
            "scenarios\\shaders\\rock",
            "20.0/0",
            r("bitm", "scenarios\\bitmaps\\snow"),
        );
    });
    let kinds: Vec<&str> = c.swaps.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(kinds, ["model", "texture"], "in the listed order");
    assert!(
        c.lines
            .contains(&"model swap: warthog uses the ghost model (was warthog)".into())
    );
    assert!(
        c.lines
            .contains(&"texture swap: shader rock uses bitmap snow (was rock)".into()),
        "{:?}",
        c.lines
    );
    // A model's render model swapped, and a reference to a tag the base
    // has not: marked new.
    let mut m = base();
    m.fields.insert(
        key("hlmt", HOG),
        Fields::from([("0".into(), leaf("render_model", r("mode", "custom\\tank")))]),
    );
    let mut b = base();
    b.fields.insert(
        key("hlmt", HOG),
        Fields::from([("0".into(), leaf("render_model", r("mode", HOG)))]),
    );
    let c = contents(&b, &m);
    assert_eq!(c.swaps[0].kind, "render_model");
    assert!(
        c.lines
            .contains(&"render model swap: model warthog draws tank (new) (was warthog)".into()),
        "{:?}",
        c.lines
    );
}

#[test]
fn stats_are_reported_and_other_numbers_counted() {
    let c = with(&|m| {
        set(m, "weap", RIFLE, "!2c0.0/a", Val::Stat("60".into()));
        set(m, "weap", RIFLE, "1c0", Val::Word(6));
    });
    assert_eq!(
        c.stats,
        [StatChange {
            tag: RIFLE.into(),
            stat: "magazines[0] magazine size".into(),
            from: "36".into(),
            to: "60".into(),
        }]
    );
    assert!(
        c.lines
            .contains(&"battle rifle: magazines[0] magazine size 36 -> 60".into())
    );
    assert!(
        c.lines
            .contains(&"other values changed: battle rifle (1)".into()),
        "{:?}",
        c.lines
    );
}

#[test]
fn placements_count_add_remove_move_and_replace() {
    let c = with(&|m| {
        m.placed.insert(
            "vehicles",
            vec![
                // Both warthogs' spots now hold banshees, and two more.
                (BANSHEE.into(), [0, 0, 10]),
                (BANSHEE.into(), [90, 0, 10]),
                (BANSHEE.into(), [10, 10, 10]),
                (BANSHEE.into(), [20, 10, 10]),
            ],
        );
    });
    assert_eq!(
        c.placements,
        [
            PlacementChange {
                feature: "vehicles".into(),
                tag: BANSHEE.into(),
                base: 0,
                now: 4,
                moved: 0,
            },
            PlacementChange {
                feature: "vehicles".into(),
                tag: HOG.into(),
                base: 2,
                now: 0,
                moved: 0,
            },
        ]
    );
    for line in [
        "vehicle replaced: warthog -> banshee at 2 spots",
        "new vehicle placements: 4 banshees",
        "vehicle placements removed: every warthog (2)",
    ] {
        assert!(c.lines.contains(&line.into()), "{line}: {:?}", c.lines);
    }
    assert!(
        c.swaps
            .iter()
            .any(|s| s.kind == "placement" && s.field == "2 spots")
    );

    let c = with(&|m| m.placed.get_mut("vehicles").unwrap()[1].1 = [95, 0, 10]);
    assert_eq!(c.placements[0].moved, 1);
    assert!(
        c.lines
            .contains(&"vehicle placements moved: 1 warthog moved".into()),
        "{:?}",
        c.lines
    );
}

#[test]
fn classes_count_new_changed_and_removed_tags() {
    let c = with(&|m| {
        m.tags.insert(
            key("weap", "objects\\weapons\\gravity_rifle\\gravity_rifle"),
            print(0x300, 0x90000, 1),
        );
        for i in 0..10 {
            m.tags.insert(
                key("bitm", &format!("custom\\b{i}")),
                print(0x40, 0xa0000 + i, 1),
            );
        }
        m.tags
            .get_mut(&key("sbsp", "scenarios\\multi\\x\\x"))
            .unwrap()
            .size = 0x200;
        m.tags.remove(&key("vehi", BANSHEE));
        m.pixels
            .insert("scenarios\\bitmaps\\rock".into(), Sha256([1; 32]));
    });
    let line = |l: &str| assert!(c.lines.contains(&l.into()), "{l}: {:?}", c.lines);
    line("weapons: 1 new (gravity rifle)");
    line("vehicles: 1 removed");
    line("bitmaps: 10 new");
    line("BSP geometry: 1 changed");
    let groups: Vec<&str> = c.classes.iter().map(|c| c.group.as_str()).collect();
    assert_eq!(groups, ["weap", "vehi", "bitm", "sbsp"]);
    assert!(c.classes[2].added_names.is_empty(), "too many to name");
    assert_eq!(c.classes[2].added, 10);

    // Repainted pixels change the bitmap; the tag's own bytes did not.
    let mut m = base();
    m.pixels
        .insert("scenarios\\bitmaps\\rock".into(), Sha256([1; 32]));
    let mut b = base();
    b.pixels
        .insert("scenarios\\bitmaps\\rock".into(), Sha256([2; 32]));
    assert_eq!(contents(&b, &m).lines, ["bitmaps: 1 changed"]);
}

#[test]
fn a_tag_that_moved_compares_by_what_it_says() {
    let mut m = base();
    let t = m.tags.get_mut(&key("weap", RIFLE)).unwrap();
    t.address += 0x100;
    t.hash = Sha256([0x42; 32]);
    assert!(contents(&base(), &m).is_empty());
}

#[test]
fn the_scenarios_named_blocks_scripts_and_sky_are_reported() {
    let c = with(&|m| {
        m.scenario.get_mut("1e0#").unwrap().val = Val::Count(20);
        m.scripts = None;
        m.scenario
            .insert("50".into(), leaf("sky", r("sky", "sky\\night")));
    });
    assert!(
        c.lines
            .contains(&"scenario player starting locations: 16 -> 20".into())
    );
    assert!(c.lines.contains(&"scripts changed".into()) && c.scripts);
    // A reference only the mod's scenario has is not a swap.
    assert!(c.swaps.is_empty());
    let c = {
        let mut b = base();
        b.scenario
            .insert("50".into(), leaf("sky", r("sky", "sky\\day")));
        let mut m = base();
        m.scenario
            .insert("50".into(), leaf("sky", r("sky", "sky\\night")));
        contents(&b, &m)
    };
    assert_eq!(
        c.lines,
        ["sky swap: the level's sky is night (new) (was day)"],
        "a sky the base has not is new"
    );
}

#[test]
fn swap_kinds_follow_what_is_pointed_at() {
    for (owner, target, kind) in [
        ("weap", "proj", "projectile"),
        ("vehi", "hlmt", "model"),
        ("hlmt", "mode", "render_model"),
        ("hlmt", "coll", "collision"),
        ("hlmt", "phmo", "physics"),
        ("mode", "shad", "shader"),
        ("shad", "bitm", "texture"),
        ("proj", "jpt!", "damage"),
        ("weap", "snd!", "sound"),
        ("vehi", "weap", "weapon"),
        ("itmc", "weap", "weapon"),
        ("scnr", "vehi", "palette"),
        ("scnr", "sky", "sky"),
        ("weap", "nhdt", "reference"),
    ] {
        assert_eq!(swap_kind(owner, target), kind, "{owner} -> {target}");
    }
    assert_eq!(words(RIFLE), "battle rifle");
    assert_eq!(words("x"), "x");
    assert_eq!(short_float(12.0), "12");
    assert_eq!(short_float(0.25), "0.25");
    assert_eq!(short_float(-0.0), "0");
    assert_eq!(short_float(1.0 / 3.0), "0.333");
}

/// An invented weapon layout: a root of 0x2d8 bytes with a model reference
/// at 0x34, a float at 0x40 and the barrels block at 0x2d0, each barrel
/// 0xec bytes with its projectile reference at 0x8c.
fn rifle_layout() -> Layout {
    let field = |offset: u32, name: &str, kind: Kind| Field {
        offset,
        name: name.into(),
        section: String::new(),
        named: !name.starts_with('+'),
        kind,
        meaning: String::new(),
    };
    let barrel = Layout {
        name: "barrels".into(),
        size: 0xec,
        align: 4,
        fields: vec![
            field(0x4, "+0x4", Kind::Unknown { len: 4 }),
            field(0x8c, "projectile", Kind::TagRef { groups: vec![] }),
        ],
    };
    Layout {
        name: "weap".into(),
        size: 0x2d8,
        align: 4,
        fields: vec![
            field(0x34, "model", Kind::TagRef { groups: vec![] }),
            field(
                0x40,
                "speed",
                Kind::Float {
                    count: 1,
                    hint: Hint::Plain,
                },
            ),
            field(0x2d0, "barrels", Kind::Block(Arc::new(barrel))),
        ],
    }
}

#[test]
fn the_walk_reads_references_numbers_blocks_and_stats() {
    const AT: u32 = 0x8000_0000;
    let mut bytes = vec![0u8; 0x2d8 + 2 * 0xec];
    let put = |b: &mut Vec<u8>, at: usize, v: &[u8]| b[at..at + v.len()].copy_from_slice(v);
    put(&mut bytes, 0x34 + 4, &0xe001_0002u32.to_le_bytes());
    put(&mut bytes, 0x40, &2.5f32.to_le_bytes());
    put(&mut bytes, 0x2d0, &2u32.to_le_bytes());
    put(&mut bytes, 0x2d4, &(AT + 0x2d8).to_le_bytes());
    let b0 = 0x2d8;
    let b1 = 0x2d8 + 0xec;
    put(&mut bytes, b0 + 0x4, &9.0f32.to_le_bytes());
    put(&mut bytes, b0 + 0x6a, &3i16.to_le_bytes());
    put(&mut bytes, b0 + 0x8c + 4, &0xe003_0004u32.to_le_bytes());
    put(&mut bytes, b1 + 0x8c + 4, &u32::MAX.to_le_bytes());
    let names: Names = [
        (0xe001_0002, key("hlmt", HOG)),
        (0xe003_0004, key("proj", BULLET)),
    ]
    .into_iter()
    .collect();
    let f = walk_with(&rifle_layout(), "weap", true, &bytes, AT, &names);
    assert_eq!(f["34"], leaf("model", r("hlmt", HOG)));
    assert_eq!(f["40"].val, Val::Word(2.5f32.to_bits()));
    assert_eq!(f["2d0#"].val, Val::Count(2));
    assert_eq!(
        f["2d0.0/8c"],
        leaf("barrels[0].projectile", r("proj", BULLET))
    );
    assert_eq!(f["2d0.1/8c"].val, Val::Ref(None), "a null reference");
    assert_eq!(f["2d0.0/4"].val, Val::Word(9.0f32.to_bits()));
    assert!(!f["2d0.0/4"].named);
    assert_eq!(
        f["!2d0.0/4"],
        leaf(
            "barrels[0] rate of fire, min (shots/s)",
            Val::Stat("9".into())
        )
    );
    assert_eq!(f["!2d0.0/6a"].val, Val::Stat("3".into()));
    assert_eq!(f["!2d0.1/6a"].val, Val::Stat("0".into()));

    // References and counts only: no numbers.
    let f = walk_with(&rifle_layout(), "weap", false, &bytes, AT, &names);
    assert!(!f.contains_key("40") && !f.contains_key("2d0.0/4"));
    assert!(f.contains_key("2d0.0/8c"));

    // A datum the map has no tag for is named by its hex.
    let f = walk_with(&rifle_layout(), "weap", false, &bytes, AT, &Names::new());
    assert_eq!(
        f["34"].val,
        Val::Ref(Some((String::new(), "e0010002".into())))
    );

    // A barrels pointer outside the payload is skipped, not followed.
    put(&mut bytes, 0x2d4, &(AT + 0x10_0000).to_le_bytes());
    let f = walk_with(&rifle_layout(), "weap", true, &bytes, AT, &names);
    assert!(!f.contains_key("2d0#") && f.contains_key("34"));
    // Too short for its root: nothing.
    assert!(walk_with(&rifle_layout(), "weap", true, &bytes[..0x10], AT, &names).is_empty());
}

#[test]
fn every_walked_group_is_in_the_registry() {
    for g in [
        "weap", "proj", "jpt!", "vehi", "bipd", "eqip", "scen", "mach", "ctrl", "bloc", "hlmt",
        "mode", "shad", "itmc", "vehc",
    ] {
        let fourcc = map::FourCC(g.as_bytes().try_into().unwrap());
        assert!(walked(g).is_some(), "{g}");
        assert!(
            Registry::builtin().group(fourcc).is_some(),
            "{g} has no registry layout"
        );
    }
    assert_eq!(walked("bitm"), None);
}

#[test]
fn an_analysis_file_round_trips() {
    let a = Analysis {
        map_id: Some("ab".repeat(32)),
        base: Some("zanzibar".into()),
        features: vec!["weapons".into()],
        contents: with(&|m| set(m, "weap", RIFLE, "2d0.0/8c", r("proj", FUEL_ROD))),
    };
    let text = toml::to_string(&a).unwrap();
    assert_eq!(toml::from_str::<Analysis>(&text).unwrap(), a);
}
