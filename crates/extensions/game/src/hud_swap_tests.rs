use super::*;
use guest_layout::memory::FlatMemory;
use guest_layout::tags::{
    TAG_ENTRIES_GLOBAL, TAG_INDEX_GLOBAL, entry_layout, group_layout, index_layout,
};

const H1_HUD: u32 = 0xe1f2_0002;
const H2_HUD: u32 = 0xe1f3_0003;
const CYBORG: u32 = 0xe1f4_0004;
const RIFLE: u32 = 0xe1f5_0005;
const OTHER_HUD: u32 = 0xe1f6_0006;

const INDEX: u32 = 0x0050_0000;
const GROUPS: u32 = 0x0050_0100;
const ENTRIES: u32 = 0x0050_1000;
const BIPED: u32 = 0x0051_0000;
const INTERFACES: u32 = 0x0051_1000;
const WEAPON: u32 = 0x0052_0000;
const HUDS: u32 = 0x0053_0000;

fn swaps() -> Vec<content::HudSwap> {
    vec![
        content::HudSwap {
            owner: CYBORG,
            halo1: H1_HUD,
            halo2: H2_HUD,
        },
        content::HudSwap {
            owner: RIFLE,
            halo1: H1_HUD,
            halo2: H2_HUD,
        },
    ]
}

/// A loaded map: two `nhdt` (and a third), the cyborg naming `interfaces`
/// in its two HUD interfaces, a rifle naming `weapon`.
fn world(interfaces: u32, weapon: u32) -> FlatMemory {
    let mut m = FlatMemory::new(0x0040_0000, 0x0018_0000);
    m.set_u32(TAG_INDEX_GLOBAL, INDEX).unwrap();
    m.set_u32(INDEX + index_layout::GROUPS, GROUPS).unwrap();
    m.set_u32(INDEX + index_layout::GROUP_COUNT, 3).unwrap();
    m.set_u32(INDEX + index_layout::TAG_COUNT, 8).unwrap();
    // Sorted: the group lookup is a binary search.
    for (i, g) in [BIPD, NHDT, WEAP].iter().enumerate() {
        let r = GROUPS + i as u32 * group_layout::STRIDE;
        m.set_u32(r + group_layout::GROUP, *g).unwrap();
        m.set_u32(r + group_layout::PARENT, u32::MAX).unwrap();
        m.set_u32(r + group_layout::GRANDPARENT, u32::MAX).unwrap();
    }
    m.set_u32(TAG_ENTRIES_GLOBAL, ENTRIES).unwrap();
    for (d, g, at) in [
        (H1_HUD, NHDT, HUDS),
        (H2_HUD, NHDT, HUDS + 0x100),
        (OTHER_HUD, NHDT, HUDS + 0x200),
        (CYBORG, BIPD, BIPED),
        (RIFLE, WEAP, WEAPON),
    ] {
        let e = ENTRIES + (d & 0xffff) * entry_layout::STRIDE;
        m.set_u32(e + entry_layout::GROUP, g).unwrap();
        m.set_u32(e + entry_layout::DATUM, d).unwrap();
        m.set_u32(e + entry_layout::ADDRESS, at).unwrap();
    }
    m.set_u32(BIPED + UNIT_HUD_INTERFACES, 2).unwrap();
    m.set_u32(BIPED + UNIT_HUD_INTERFACES + 4, INTERFACES)
        .unwrap();
    for i in 0..2 {
        m.set_u32(INTERFACES + i * 8, NHDT).unwrap();
        m.set_u32(INTERFACES + i * 8 + 4, interfaces).unwrap();
    }
    m.set_u32(WEAPON + WEAPON_HUD, NHDT).unwrap();
    m.set_u32(WEAPON + WEAPON_HUD + 4, weapon).unwrap();
    m
}

fn huds(m: &FlatMemory) -> [u32; 3] {
    [
        m.u32(INTERFACES + 4).unwrap(),
        m.u32(INTERFACES + 12).unwrap(),
        m.u32(WEAPON + WEAPON_HUD + 4).unwrap(),
    ]
}

#[test]
fn halo2_points_every_owner_at_halo2s_hud() {
    let mut m = world(H1_HUD, H1_HUD);
    assert_eq!(apply(&mut m, &swaps(), HudStyle::Halo2).unwrap(), 3);
    assert_eq!(huds(&m), [H2_HUD; 3]);
    // Again: nothing left to swap.
    assert_eq!(apply(&mut m, &swaps(), HudStyle::Halo2).unwrap(), 0);
}

#[test]
fn halo1_puts_halo1s_hud_back() {
    // Guest memory outlives a game when the map stays loaded.
    let mut m = world(H2_HUD, H2_HUD);
    assert_eq!(apply(&mut m, &swaps(), HudStyle::Halo1).unwrap(), 3);
    assert_eq!(huds(&m), [H1_HUD; 3]);
}

#[test]
fn a_reference_to_another_hud_is_left_alone() {
    let mut m = world(OTHER_HUD, H1_HUD);
    assert_eq!(apply(&mut m, &swaps(), HudStyle::Halo2).unwrap(), 1);
    assert_eq!(huds(&m), [OTHER_HUD, OTHER_HUD, H2_HUD]);
}

#[test]
fn a_target_that_is_not_a_loaded_nhdt_changes_nothing() {
    let mut m = world(H1_HUD, H1_HUD);
    let bad = [content::HudSwap {
        owner: CYBORG,
        halo1: H1_HUD,
        halo2: CYBORG,
    }];
    assert_eq!(apply(&mut m, &bad, HudStyle::Halo2).unwrap(), 0);
    let gone = [content::HudSwap {
        owner: 0xe1f7_0007,
        halo1: H1_HUD,
        halo2: H2_HUD,
    }];
    assert_eq!(apply(&mut m, &gone, HudStyle::Halo2).unwrap(), 0);
    assert_eq!(huds(&m), [H1_HUD; 3]);
}

#[test]
fn campaign_levels_read_their_hud_files() {
    let dir = std::env::temp_dir().join(format!("h1-hud-table-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(read_table(&dir).is_empty());
    assert!(read_campaign_table(&dir).is_empty());
    let level = dir.join("campaigns").join("halo1");
    std::fs::create_dir_all(&level).unwrap();
    let file = content::LevelHud {
        hud: swaps(),
        sensor: Some([45, -43]),
    };
    std::fs::write(
        level.join(content::LevelHud::file_name("A10")),
        file.to_toml(),
    )
    .unwrap();
    std::fs::write(
        level.join("a30.hud.toml"),
        content::LevelHud::default().to_toml(),
    )
    .unwrap();
    let t = read_campaign_table(&dir);
    assert_eq!(t.len(), 1, "an empty file names nothing");
    assert_eq!(
        t["a10"],
        MapHud {
            sandboxed: false,
            swaps: swaps(),
            sensor: Some([45, -43])
        }
    );
    // A Halo 2-assets map beside it (written by the import) is not read as
    // a level of its own, and marks the level as having one.
    assert!(campaign_halo2_levels(&dir).is_empty());
    crate::campaigns::store_fixture(&dir.join("tags"), "halo1_campaign_a10_h2", b"map");
    assert_eq!(read_campaign_table(&dir).len(), 1);
    assert_eq!(
        campaign_halo2_levels(&dir),
        std::collections::HashSet::from(["a10".to_owned()])
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Halo 2's own sensor table, as the XBE holds it.
fn retail_sensor(m: &mut FlatMemory) {
    for (i, [x, y]) in [[41i16, -42], [31, -31], [31, -31]].iter().enumerate() {
        m.set_u16(SENSOR_CENTRES + 4 * i as u32, *x as u16).unwrap();
        m.set_u16(SENSOR_CENTRES + 4 * i as u32 + 2, *y as u16)
            .unwrap();
    }
    m.set_u32(SENSOR_CENTRES + 12, 0).unwrap();
}

#[test]
fn halo1s_hud_centres_the_motion_sensor_and_halo2s_puts_it_back() {
    let mut m = world(H1_HUD, H1_HUD);
    retail_sensor(&mut m);
    let mut s = Sensor::default();
    assert!(
        s.activate(&mut m, Some([45, -43]), HudStyle::Halo1)
            .unwrap()
    );
    assert_eq!(read_sensor(&m).unwrap(), [[45, -43]; 3]);
    assert_eq!(
        m.u32(SENSOR_CENTRES + 12).unwrap(),
        0,
        "the fourth entry is left alone"
    );
    // Again: already there.
    assert!(
        !s.activate(&mut m, Some([45, -43]), HudStyle::Halo1)
            .unwrap()
    );
    // The setting changed between games: Halo 2's table back.
    assert!(
        s.activate(&mut m, Some([45, -43]), HudStyle::Halo2)
            .unwrap()
    );
    assert_eq!(read_sensor(&m).unwrap(), [[41, -42], [31, -31], [31, -31]]);
    // And Halo 1's again.
    assert!(
        s.activate(&mut m, Some([45, -43]), HudStyle::Halo1)
            .unwrap()
    );
    assert_eq!(read_sensor(&m).unwrap(), [[45, -43]; 3]);
}

#[test]
fn a_map_without_a_halo1_sensor_gets_halo2s_back() {
    let mut m = world(H1_HUD, H1_HUD);
    retail_sensor(&mut m);
    let mut s = Sensor::default();
    // A retail map first: nothing written, Halo 2's table remembered.
    assert!(!s.activate(&mut m, None, HudStyle::Halo2).unwrap());
    assert!(
        s.activate(&mut m, Some([45, -43]), HudStyle::Halo1)
            .unwrap()
    );
    // Then a retail map (or a converted one without a centre) under either style.
    assert!(s.activate(&mut m, None, HudStyle::Halo1).unwrap());
    assert_eq!(read_sensor(&m).unwrap(), [[41, -42], [31, -31], [31, -31]]);
}

#[test]
fn the_swap_plans_after_the_live_options() {
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    r.add(&crate::variant_ext::Transport { hle: None }).unwrap();
    r.add(&HudSwap {
        table: Arc::default(),
        sensor: Arc::new(std::sync::Mutex::new(Sensor { retail: None })),
        halo2_levels: Arc::default(),
    })
    .unwrap();
    let text = r.build(&hooks::Base).unwrap().text();
    let line = text.lines().find(|l| l.contains("GAME_ACTIVATE")).unwrap();
    let ext = line.find(crate::variant_ext::FEATURE).unwrap();
    assert!(ext < line.find(FEATURE).unwrap(), "{line}");
}

#[test]
fn a_snapshot_keeps_halo2s_sensor_table() {
    let mut m = world(H1_HUD, H1_HUD);
    retail_sensor(&mut m);
    let mut s = Sensor::default();
    assert!(
        s.activate(&mut m, Some([45, -43]), HudStyle::Halo1)
            .unwrap()
    );
    let bytes = s.save_state();
    // A fresh boot loads the snapshot, whose guest table is Halo 1's.
    let mut b = Sensor::default();
    b.load_state(&bytes).unwrap();
    assert_eq!(b.save_state(), bytes, "the same state again");
    assert!(b.activate(&mut m, None, HudStyle::Halo2).unwrap());
    assert_eq!(read_sensor(&m).unwrap(), [[41, -42], [31, -31], [31, -31]]);
    // Nothing remembered round-trips as nothing.
    let none = Sensor::default().save_state();
    b.load_state(&none).unwrap();
    assert_eq!(b.save_state(), none);
    assert!(b.load_state(&bytes[..bytes.len() - 1]).is_err());
}
