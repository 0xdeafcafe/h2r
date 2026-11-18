//! Synthetic tag tables only: every value here is made up for the test.

use std::collections::BTreeMap;

use super::*;

#[derive(Default, Clone, PartialEq, Debug)]
pub(super) struct Fake(BTreeMap<u32, u8>);

impl Gm for Fake {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        for (i, b) in out.iter_mut().enumerate() {
            match self.0.get(&(addr + i as u32)) {
                Some(v) => *b = *v,
                None => return false,
            }
        }
        true
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        for (i, b) in data.iter().enumerate() {
            self.0.insert(addr + i as u32, *b);
        }
        true
    }
}

pub(super) fn f32_at(m: &Fake, a: u32) -> f32 {
    f32::from_bits(m.u32(a).unwrap())
}
pub(super) fn set_f32(m: &mut Fake, a: u32, v: f32) {
    m.set_u32(a, v.to_bits());
}

const INDEX: u32 = 0x10_0000;
const ENTRIES: u32 = 0x11_0000;
/// Synthetic weapon tags: slot -> (address, aim cone angle, aim cone range,
/// pull angle, pull range).
const WEAPONS: [(u32, u32, [f32; 4]); 3] = [
    (1, 0x20_0000, [0.05, 20.0, 0.1, 15.0]),
    (3, 0x21_0000, [0.02, 50.0, 0.4, 40.0]),
    (4, 0x22_0000, [0.3, 5.0, 0.0, 0.0]),
];
const GLOBALS: u32 = 0x30_0000;
const PLAYER_CONTROLS: u32 = 0x31_0000;
const SLOWDOWN_V: f32 = 0.6;
const FOLLOW_V: f32 = 0.9;
/// A biped tag whose bytes at the weapon offsets must stay put.
const BIPD: u32 = 0x40_0000;

fn entry(m: &mut Fake, slot: u32, group: &[u8; 4], addr: u32, size: u32) {
    let e = ENTRIES + slot * 16;
    m.set_u32(e, u32::from_be_bytes(*group));
    m.set_u32(e + 4, 0xe000_0000 | slot);
    m.set_u32(e + 8, addr);
    m.set_u32(e + 0xc, size);
}

/// Guest memory with a loaded map: the tag index, three weapons, a biped,
/// an empty slot and the globals with one player-control element.
fn world() -> Fake {
    let mut m = Fake::default();
    m.set_u32(TAG_INDEX, INDEX);
    m.set_u32(TAG_ENTRIES, ENTRIES);
    m.set_u32(INDEX + 0x18, 7);
    for slot in 0..7 {
        entry(&mut m, slot, &[0xff; 4], 0, 0);
    }
    for (slot, addr, v) in WEAPONS {
        entry(&mut m, slot, b"weap", addr, 0x31c);
        for (off, x) in [CONE_ANGLE, CONE_RANGE, PULL_ANGLE, PULL_RANGE]
            .into_iter()
            .zip(v)
        {
            set_f32(&mut m, addr + off, x);
        }
        // The bend angle next to them is never touched.
        set_f32(&mut m, addr + 0x218, 0.25);
    }
    entry(&mut m, 2, b"bipd", BIPD, 0x314);
    for off in (0x200..0x220).step_by(4) {
        set_f32(&mut m, BIPD + off, 0.125);
    }
    entry(&mut m, 5, b"matg", GLOBALS, 0x5000);
    m.set_u32(GLOBALS + MATG_PLAYER_CONTROL, 1);
    m.set_u32(GLOBALS + MATG_PLAYER_CONTROL + 4, PLAYER_CONTROLS);
    set_f32(&mut m, PLAYER_CONTROLS + SLOWDOWN, SLOWDOWN_V);
    set_f32(&mut m, PLAYER_CONTROLS + FOLLOW, FOLLOW_V);
    set_f32(&mut m, PLAYER_CONTROLS + 8, 0.5);
    m
}

/// The six values a level should leave: per weapon its four, then
/// slowdown and follow.
fn expect(m: &Fake, level: Level) {
    let f = level.factors();
    for (_, addr, v) in WEAPONS {
        let want = [
            scale(v[0], f.angle, MAX_ANGLE),
            scale(v[1], f.range, f32::MAX),
            scale(v[2], f.angle, MAX_ANGLE),
            scale(v[3], f.range, f32::MAX),
        ];
        let got = [CONE_ANGLE, CONE_RANGE, PULL_ANGLE, PULL_RANGE].map(|o| f32_at(m, addr + o));
        assert_eq!(got, want, "{level:?} weapon at {addr:#x}");
        assert_eq!(f32_at(m, addr + 0x218), 0.25);
    }
    assert_eq!(
        f32_at(m, PLAYER_CONTROLS),
        scale(SLOWDOWN_V, f.stick, MAX_STICK)
    );
    assert_eq!(
        f32_at(m, PLAYER_CONTROLS + 4),
        scale(FOLLOW_V, f.stick, MAX_STICK)
    );
    assert_eq!(
        f32_at(m, PLAYER_CONTROLS + 8),
        0.5,
        "inconsequential target scale stays"
    );
}

#[test]
fn the_offsets_are_the_tag_layouts() {
    let weap = &tags::object::layouts::WEAP;
    let at = |name: &str| {
        weap.fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.offset)
    };
    assert_eq!(at("cone_angle"), Some(CONE_ANGLE));
    assert_eq!(at("cone_range"), Some(CONE_RANGE));
    assert_eq!(at("pull_angle"), Some(PULL_ANGLE));
    assert_eq!(at("pull_range"), Some(PULL_RANGE));
    assert_eq!(
        (CONE_ANGLE, CONE_RANGE, PULL_ANGLE, PULL_RANGE),
        (0x208, 0x20c, 0x210, 0x214)
    );
    assert_eq!((MATG_PLAYER_CONTROL, PLAYER_CONTROL_STRIDE), (0xf0, 0x80));
    assert_eq!((SLOWDOWN, FOLLOW), (0, 4));
    assert_eq!((TAG_INDEX, TAG_ENTRIES), (0x0054_7f00, 0x004e_3b44));
    assert_eq!(WEAP, 0x7765_6170);
}

#[test]
fn scaling_caps_angles_and_never_weakens() {
    assert_eq!(scale(0.1, 1.5, MAX_ANGLE), 0.1 * 1.5);
    assert_eq!(scale(0.4, 2.0, MAX_ANGLE), MAX_ANGLE, "capped");
    assert_eq!(
        scale(0.7, 2.0, MAX_ANGLE),
        0.7,
        "already past the cap: kept"
    );
    assert_eq!(scale(0.9, 1.5, MAX_STICK), MAX_STICK);
    assert_eq!(scale(20.0, 1.25, f32::MAX), 25.0);
    for odd in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(scale(odd, 2.0, MAX_ANGLE).to_bits(), odd.to_bits());
    }
    // A factor of 1 is retail bit for bit, whatever the value.
    for v in [0.05f32, 3.0, 1.0e-30] {
        assert_eq!(scale(v, 1.0, MAX_ANGLE).to_bits(), v.to_bits());
    }
    const { assert!(MAX_ANGLE < std::f32::consts::FRAC_PI_4) };
    assert_eq!(
        Level::Strong.factors(),
        Factors {
            angle: 1.5,
            range: 1.25,
            stick: 1.25
        }
    );
    assert_eq!(
        Level::VeryStrong.factors(),
        Factors {
            angle: 2.0,
            range: 1.5,
            stick: 1.5
        }
    );
    assert_eq!(
        Level::Default.factors(),
        Factors {
            angle: 1.0,
            range: 1.0,
            stick: 1.0
        }
    );
}

#[test]
fn default_leaves_every_byte_alone() {
    let mut m = world();
    let before = m.clone();
    let mut r = Remembered::default();
    let done = r.apply(&mut m, Level::Default);
    assert_eq!(m, before);
    assert_eq!(
        done,
        Applied {
            weapons: 3,
            player_controls: 1,
            writes: 0
        }
    );
    assert!(r.is_empty());
}

#[test]
fn strong_and_very_strong_scale_by_their_factors() {
    for level in [Level::Strong, Level::VeryStrong] {
        let mut m = world();
        let mut r = Remembered::default();
        r.apply(&mut m, level);
        expect(&m, level);
        // Spot checks against hand-worked numbers.
        let f = level.factors();
        assert_eq!(f32_at(&m, 0x20_0000 + CONE_ANGLE), 0.05 * f.angle);
        assert_eq!(f32_at(&m, 0x20_0000 + PULL_RANGE), 15.0 * f.range);
        assert_eq!(f32_at(&m, 0x22_0000 + PULL_ANGLE), 0.0, "zero stays zero");
        // The biped's bytes at the same offsets are not a weapon's.
        for off in (0x200..0x220).step_by(4) {
            assert_eq!(f32_at(&m, BIPD + off), 0.125);
        }
    }
    let mut m = world();
    Remembered::default().apply(&mut m, Level::VeryStrong);
    assert_eq!(
        f32_at(&m, 0x21_0000 + PULL_ANGLE),
        MAX_ANGLE,
        "0.4 x 2 capped"
    );
    assert_eq!(
        f32_at(&m, PLAYER_CONTROLS + 4),
        MAX_STICK,
        "0.9 x 1.5 capped at 1"
    );
}

#[test]
fn reapplying_and_switching_never_compound_and_default_restores() {
    let retail = world();
    let mut m = world();
    let mut r = Remembered::default();
    r.apply(&mut m, Level::Strong);
    let strong = m.clone();
    // The next map load with the same level: nothing moves.
    assert_eq!(r.apply(&mut m, Level::Strong).writes, 0);
    assert_eq!(m, strong);
    // Very strong is from retail, not from Strong's values.
    r.apply(&mut m, Level::VeryStrong);
    expect(&m, Level::VeryStrong);
    r.apply(&mut m, Level::Strong);
    assert_eq!(m, strong);
    // Default puts retail back, byte for byte, and forgets.
    r.apply(&mut m, Level::Default);
    assert_eq!(m, retail);
    assert!(r.is_empty());
}

#[test]
fn a_reloaded_map_is_its_own_retail() {
    let mut m = world();
    let mut r = Remembered::default();
    r.apply(&mut m, Level::Strong);
    // The cache reloads: retail is back in the same place (a revert), and one
    // weapon's slot now holds a tag with other values (another map).
    let mut reloaded = world();
    // (Not 0.03: that is what Strong wrote there, which would read as ours.)
    set_f32(&mut reloaded, 0x21_0000 + CONE_ANGLE, 0.07);
    let fresh = reloaded.clone();
    m = reloaded;
    r.apply(&mut m, Level::Strong);
    assert_eq!(
        f32_at(&m, 0x20_0000 + CONE_ANGLE),
        0.05 * 1.5,
        "not 0.05 x 1.5 x 1.5"
    );
    assert_eq!(f32_at(&m, 0x21_0000 + CONE_ANGLE), 0.07 * 1.5);
    r.apply(&mut m, Level::Default);
    assert_eq!(m, fresh);
}

#[test]
fn missing_or_odd_tables_are_left_alone() {
    let mut empty = Fake::default();
    assert_eq!(
        Remembered::default().apply(&mut empty, Level::VeryStrong),
        Applied::default()
    );
    // A short weapon tag and a globals tag with an absurd block count.
    let mut m = world();
    entry(&mut m, 1, b"weap", 0x20_0000, 0x100);
    m.set_u32(GLOBALS + MATG_PLAYER_CONTROL, 1000);
    let before = m.clone();
    let done = Remembered::default().apply(&mut m, Level::VeryStrong);
    assert_eq!((done.weapons, done.player_controls), (2, 0));
    assert_eq!(
        m.u32(0x20_0000 + CONE_ANGLE),
        before.u32(0x20_0000 + CONE_ANGLE)
    );
    assert_eq!(m.u32(PLAYER_CONTROLS), before.u32(PLAYER_CONTROLS));
}

/// The one test touching the global touch flag, so none race on it.
#[test]
fn auto_follows_the_touch_controls_and_multiplayer_is_retail() {
    assert_eq!(level_for(SETTING_AUTO, true), Level::Strong);
    assert_eq!(level_for(SETTING_AUTO, false), Level::Default);
    for touch in [false, true] {
        assert_eq!(level_for(SETTING_DEFAULT, touch), Level::Default);
        assert_eq!(level_for(SETTING_STRONG, touch), Level::Strong);
        assert_eq!(level_for(SETTING_VERY_STRONG, touch), Level::VeryStrong);
        assert_eq!(level_for(99, touch), Level::Default);
    }
    let was = input::touch::controls_shown();
    input::touch::set_controls_shown(true);
    assert_eq!(level_now(SETTING_AUTO, Some(1)), Level::Strong);
    assert_eq!(
        level_now(SETTING_AUTO, Some(MODE_MULTIPLAYER)),
        Level::Default
    );
    assert_eq!(
        level_now(SETTING_VERY_STRONG, Some(MODE_MULTIPLAYER)),
        Level::Default
    );
    let mut m = world();
    let mut r = Remembered::default();
    r.apply(&mut m, level_now(SETTING_AUTO, Some(1)));
    expect(&m, Level::Strong);
    input::touch::set_controls_shown(false);
    assert_eq!(level_now(SETTING_AUTO, Some(1)), Level::Default);
    assert_eq!(level_now(SETTING_STRONG, Some(1)), Level::Strong);
    r.apply(&mut m, level_now(SETTING_AUTO, Some(1)));
    assert_eq!(m, world(), "touch hidden: retail again");
    input::touch::set_controls_shown(was);
}

/// Plausibility on the retail disc (values printed, never committed):
/// every weapon's angles are finite, positive or zero and under the cap,
/// ranges finite and non-negative, and slowdown and follow in [0, 1].
/// `H2_DISC=/path/halo2.iso cargo test -p boot aim_assist -- --ignored --nocapture`
#[test]
#[ignore = "needs the retail disc in $H2_DISC"]
fn retail_tags_hold_plausible_aim_assist_values() {
    use std::io::BufReader;
    let disc = std::env::var_os("H2_DISC").expect("set H2_DISC");
    let open = || BufReader::new(std::fs::File::open(&disc).unwrap());
    let mut iso = xiso::Xiso::open(open()).unwrap();
    let base = iso.partition_offset();
    let mut weapons = 0;
    for e in iso
        .entries()
        .unwrap()
        .iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
    {
        let at = base + u64::from(e.sector) * xiso::SECTOR;
        let Ok(mut m) = map::Map::from_region(open(), at, e.size.into()) else {
            continue;
        };
        let f = |b: &[u8], o: u32| {
            f32::from_le_bytes(b[o as usize..o as usize + 4].try_into().unwrap())
        };
        let ids: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group.0 == *b"weap")
            .map(|t| (t.id, t.name.clone()))
            .collect();
        for (id, name) in ids {
            let b = m.read_tag_bytes(id).unwrap();
            let v = [CONE_ANGLE, CONE_RANGE, PULL_ANGLE, PULL_RANGE].map(|o| f(&b, o));
            eprintln!("{} {:?}: {v:?}", e.path, name);
            assert!(v.iter().all(|x| x.is_finite() && *x >= 0.0), "{name:?}");
            assert!(v[0] < MAX_ANGLE && v[2] < MAX_ANGLE, "{name:?}");
            weapons += 1;
        }
        if let Some(id) = m.globals().map(|t| t.id) {
            let g = m.read_tag_bytes(id).unwrap();
            let n = u32::from_le_bytes(g[0xf0..0xf4].try_into().unwrap());
            let addr = u32::from_le_bytes(g[0xf4..0xf8].try_into().unwrap());
            if n > 0 {
                let off = m.file_offset(addr, n * PLAYER_CONTROL_STRIDE).unwrap();
                let pc = m
                    .read_file_range(off, (n * PLAYER_CONTROL_STRIDE).into())
                    .unwrap();
                let (fr, ad) = (f(&pc, 0), f(&pc, 4));
                eprintln!(
                    "{} globals: {n} player controls, slowdown {fr}, follow {ad}",
                    e.path
                );
                assert!((0.0..=1.0).contains(&fr) && (0.0..=1.0).contains(&ad));
            }
        }
    }
    assert!(weapons > 0);
}
