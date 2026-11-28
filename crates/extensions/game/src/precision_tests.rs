use super::*;
use guest_layout::memory::FlatMemory;
use guest_layout::tags::{
    TAG_ENTRIES_GLOBAL, TAG_INDEX_GLOBAL, entry_layout, group_layout, index_layout,
};

const ITMC: u32 = u32::from_be_bytes(*b"itmc");
const WEAP: u32 = u32::from_be_bytes(*b"weap");
const SMG: u32 = 0xe1f2_0002;
const BR: u32 = 0xe1f3_0003;
const PISTOL: u32 = 0xe1f4_0004;

const INDEX: u32 = 0x0050_0000;
const GROUPS: u32 = 0x0050_0100;
const ENTRIES: u32 = 0x0050_1000;
const SCNR: u32 = 0x0051_0000;
const ITEMS: u32 = 0x0051_1000;
const OPTIONS: u32 = 0x0052_0000;

fn swaps() -> Vec<content::Swap> {
    vec![content::Swap { from: SMG, to: BR }]
}

/// A loaded map: itmc tags SMG, BR and the pistol, and netgame items naming
/// `items` (each an itmc datum).
fn world(items: &[u32]) -> FlatMemory {
    let mut m = FlatMemory::new(0x0040_0000, 0x0018_0000);
    m.set_u32(TAG_INDEX_GLOBAL, INDEX).unwrap();
    m.set_u32(INDEX + index_layout::GROUPS, GROUPS).unwrap();
    m.set_u32(INDEX + index_layout::GROUP_COUNT, 1).unwrap();
    m.set_u32(INDEX + index_layout::TAG_COUNT, 8).unwrap();
    m.set_u32(GROUPS + group_layout::GROUP, ITMC).unwrap();
    m.set_u32(GROUPS + group_layout::PARENT, u32::MAX).unwrap();
    m.set_u32(GROUPS + group_layout::GRANDPARENT, u32::MAX)
        .unwrap();
    m.set_u32(TAG_ENTRIES_GLOBAL, ENTRIES).unwrap();
    for d in [SMG, BR, PISTOL] {
        let e = ENTRIES + (d & 0xffff) * entry_layout::STRIDE;
        m.set_u32(e + entry_layout::GROUP, ITMC).unwrap();
        m.set_u32(e + entry_layout::DATUM, d).unwrap();
        m.set_u32(e + entry_layout::ADDRESS, 0x0053_0000).unwrap();
    }
    m.set_u32(SCENARIO, SCNR).unwrap();
    m.set_u32(SCNR + NETGAME_EQUIPMENT, items.len() as u32)
        .unwrap();
    m.set_u32(SCNR + NETGAME_EQUIPMENT + 4, ITEMS).unwrap();
    for (i, &d) in items.iter().enumerate() {
        let at = ITEMS + i as u32 * NETGAME_ITEM_LEN + NETGAME_ITEM;
        m.set_u32(at, ITMC).unwrap();
        m.set_u32(at + 4, d).unwrap();
    }
    m
}

fn items(m: &FlatMemory, n: u32) -> Vec<u32> {
    (0..n)
        .map(|i| {
            m.u32(ITEMS + i * NETGAME_ITEM_LEN + NETGAME_ITEM + 4)
                .unwrap()
        })
        .collect()
}

#[test]
fn a_precision_variant_swaps_the_smg_for_the_battle_rifle() {
    let mut m = world(&[SMG, PISTOL, SMG]);
    assert_eq!(apply(&mut m, &swaps(), true).unwrap(), 2);
    assert_eq!(items(&m, 3), [BR, PISTOL, BR]);
    // Again: nothing left to swap.
    assert_eq!(apply(&mut m, &swaps(), true).unwrap(), 0);
}

#[test]
fn any_other_variant_puts_the_smg_back() {
    // Guest memory outlives a game when the map stays loaded.
    let mut m = world(&[BR, PISTOL, SMG]);
    assert_eq!(apply(&mut m, &swaps(), false).unwrap(), 1);
    assert_eq!(items(&m, 3), [SMG, PISTOL, SMG]);
}

#[test]
fn a_target_that_is_not_a_loaded_tag_of_the_group_changes_nothing() {
    let mut m = world(&[SMG]);
    // A stale salt.
    let stale = [content::Swap {
        from: SMG,
        to: 0xdead_0003,
    }];
    assert_eq!(apply(&mut m, &stale, true).unwrap(), 0);
    assert_eq!(items(&m, 1), [SMG]);
    // The right datum, but the item's group differs from the tag's.
    m.set_u32(ITEMS + NETGAME_ITEM, WEAP).unwrap();
    assert_eq!(apply(&mut m, &swaps(), true).unwrap(), 0);
    assert_eq!(items(&m, 1), [SMG]);
}

#[test]
fn no_scenario_no_items() {
    let mut m = world(&[SMG]);
    m.set_u32(SCENARIO, 0).unwrap();
    assert_eq!(apply(&mut m, &swaps(), true).unwrap(), 0);
}

#[test]
fn precision_is_a_multiplayer_variant_starting_with_a_precision_weapon() {
    let mut m = world(&[]);
    let weapon = OPTIONS + OPTIONS_VARIANT + STARTING_WEAPON;
    m.set_u32(OPTIONS + OPTIONS_TYPE, MULTIPLAYER).unwrap();
    for (w, want) in [
        (3, true),
        (10, true),
        (13, true),
        (5, false),
        (2, false),
        (0, false),
    ] {
        m.set_u8(weapon, w).unwrap();
        assert_eq!(is_precision(&m, OPTIONS).unwrap(), want, "weapon {w}");
    }
    // Campaign options are never a precision variant.
    m.set_u8(weapon, 3).unwrap();
    m.set_u32(OPTIONS + OPTIONS_TYPE, 1).unwrap();
    assert!(!is_precision(&m, OPTIONS).unwrap());
}

fn manifest(map: &str, sandbox: &str, precision: &str) -> String {
    let sha = "0".repeat(64);
    format!(
        "[package]\nformat = 1\nname = \"Blood Gulch\"\n\n[base]\nmap = \"{map}\"\nderived = true\n\n\
         [list]\nbase = \"coagulation\"\n\n[[variant]]\nfile = \"{map}.map\"\nsha256 = \"{sha}\"\n\
         origin = {{ game = \"halo1\", map = \"bloodgulch\", sandbox = \"{sandbox}\", source_sha256 = \"{sha}\" }}\n{precision}"
    )
}

#[test]
fn the_table_holds_the_h2_sandbox_swaps_by_map() {
    let dir = std::env::temp_dir().join(format!("precision-table-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let swaps = "precision = [{ from = 3790733314, to = 3790798851 }]\n";
    for (pkg, map, sandbox, p) in [
        ("h1_bloodgulch", "H1_Bloodgulch", "h2", swaps),
        ("h1_hangemhigh", "h1_hangemhigh", "h1", swaps),
        ("h1_chillout", "h1_chillout", "h2", ""),
    ] {
        std::fs::create_dir_all(dir.join(pkg)).unwrap();
        std::fs::write(
            dir.join(pkg).join("manifest.toml"),
            manifest(map, sandbox, p),
        )
        .unwrap();
    }
    std::fs::create_dir_all(dir.join("broken")).unwrap();
    std::fs::write(dir.join("broken/manifest.toml"), "not toml [").unwrap();
    let t = read_table(&dir);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(t.len(), 1, "{t:?}");
    assert_eq!(t["h1_bloodgulch"], [content::Swap { from: SMG, to: BR }]);
    assert!(read_table(&dir).is_empty());
}

/// Aim assist and the precision swap both hook game activation before
/// it: the plan must order them, or every boot refuses its features.
#[test]
fn precision_and_aim_assist_plan_together() {
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    let store = settings::Store::defaults(crate::setting_defs::ALL);
    r.add(&crate::aim_assist::AimAssist::new(&store)).unwrap();
    assert!(install(&mut r, std::path::Path::new("/nonexistent")).unwrap());
    let plan = r.build(&hooks::Base).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(plan.text().contains(FEATURE), "{}", plan.text());
}

#[test]
fn the_swap_and_aim_assist_plan_after_the_live_options() {
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    let store = settings::Store::defaults(crate::setting_defs::ALL);
    r.add(&crate::variant_ext::Transport { hle: None }).unwrap();
    r.add(&crate::aim_assist::AimAssist::new(&store)).unwrap();
    r.add(&Precision { table: Arc::default() }).unwrap();
    let text = r.build(&hooks::Base).unwrap().text();
    let line = text.lines().find(|l| l.contains("GAME_ACTIVATE")).unwrap();
    let ext = line.find(crate::variant_ext::FEATURE).unwrap();
    assert!(ext < line.find(crate::aim_assist::FEATURE).unwrap(), "{line}");
    assert!(ext < line.find(FEATURE).unwrap(), "{line}");
}
