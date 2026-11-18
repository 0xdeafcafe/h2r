use super::*;
use engine::memory::FlatMemory;
use engine::tags::{
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
        content::HudSwap { owner: CYBORG, halo1: H1_HUD, halo2: H2_HUD },
        content::HudSwap { owner: RIFLE, halo1: H1_HUD, halo2: H2_HUD },
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
    m.set_u32(BIPED + UNIT_HUD_INTERFACES + 4, INTERFACES).unwrap();
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
    let bad = [content::HudSwap { owner: CYBORG, halo1: H1_HUD, halo2: CYBORG }];
    assert_eq!(apply(&mut m, &bad, HudStyle::Halo2).unwrap(), 0);
    let gone = [content::HudSwap { owner: 0xe1f7_0007, halo1: H1_HUD, halo2: H2_HUD }];
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
    let file = content::LevelHud { hud: swaps() };
    std::fs::write(level.join(content::LevelHud::file_name("A10")), file.to_toml()).unwrap();
    std::fs::write(level.join("a30.hud.toml"), content::LevelHud::default().to_toml()).unwrap();
    let t = read_campaign_table(&dir);
    assert_eq!(t.len(), 1, "an empty file names nothing");
    assert_eq!(t["a10"], (false, swaps()));
    // A Halo 2-assets map beside it (written by the import) is not read as
    // a level of its own, and marks the level as having one.
    assert!(campaign_halo2_levels(&dir).is_empty());
    std::fs::create_dir_all(level.join("h2")).unwrap();
    std::fs::write(level.join("h2").join("A10.map"), b"map").unwrap();
    assert_eq!(read_campaign_table(&dir).len(), 1);
    assert_eq!(
        campaign_halo2_levels(&dir),
        std::collections::HashSet::from(["a10".to_owned()])
    );
    let _ = std::fs::remove_dir_all(&dir);
}
