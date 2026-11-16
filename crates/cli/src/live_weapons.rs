//! Weapon ballistics for the viewer: what a multiplayer weapon fires, how
//! fast, and how often, read from its `weap` tag and the `proj` tag its
//! barrel references.
//!
//! The layout below was pinned from the retail disc's data (no field names
//! are in the cache files); the `weapons_from_the_disc` test writes what
//! this reads. Evidence, by field:
//!
//! - **`weap +0x2d0`, barrels**: block (count, pointer) with stride `0xec`
//!   (docs/formats/objects.md lists the block; the stride is confirmed by every
//!   barrel of every multiplayer weapon carrying a `proj` reference at the
//!   same place). Single-barrel weapons (battle rifle, smg, magnum, needler,
//!   sniper, beam rifle, shotgun, brute shot) have one; the plasma pistol,
//!   carbine, rocket launcher and fuel rod have two; the energy blade, flag,
//!   ball and bomb have none.
//! - **`weap +0x2c8`, triggers** (stride `0x40`): `+0x8` is the i16 index of
//!   the primary barrel (`+0xa` the secondary, `-1` none). The carbine's
//!   trigger names barrel 1, every other readable one barrel 0. Some weapons
//!   share a trigger element stored in another tag's payload, which this
//!   reader cannot reach; those all have a single barrel, so barrel 0 is used
//!   when the trigger is out of reach.
//! - **barrel `+0x04`/`+0x08`, rounds per second min/max** (f32): smg 15/15,
//!   needler 8/8, plasma rifle 6/9 with `+0x0c`/`+0x10` 0.8/0.6 (its spin-up
//!   and spin-down times), battle rifle 15/15 (inside a burst). Zero on
//!   semi-automatic weapons (magnum, sniper, shotgun, rockets).
//! - **barrel `+0x1c`, shots per fire** (i16 min, max): battle rifle 3/3,
//!   semi-automatics 1/1, automatics 0/0.
//! - **barrel `+0x20`, fire recovery time** (f32 seconds): magnum 0.1,
//!   battle rifle 0.26, sniper 0.5, shotgun 1.0, rocket launcher 0.8, beam
//!   rifle 0.25, fuel rod 0.4; zero on the automatics.
//! - **barrel `+0x8c`, projectile reference**: the group FourCC (stored
//!   as a little-endian u32 of the big-endian tag, bytes `jorp` on disc)
//!   then the datum at `+0x90`.
//! - **`proj +0x17c`/`+0x180`, initial and final velocity** (f32 world units
//!   per second): every bullet 400/400, sniper and beam rifle 1200/1200,
//!   shotgun pellets 120, plasma rifle 14, plasma pistol 11, needles 6 (and
//!   guided: `+0x184` is non-zero), rocket 8 growing to 20, brute shot
//!   grenade 20 slowing to 10, fuel rod 15 to 6.5. The rocket's growth is the
//!   tell that `+0x17c` is initial and `+0x180` final; `+0x18c`/`+0x190`
//!   (an acceleration range, 0..20 on the rocket) follow.
//!
//! The rate reported is sustained rounds per second derived from those
//! fields (see [`sustained_rate`]), not the raw in-burst figure.

use std::fs::File;
use std::io::BufReader;

use map::{FourCC, Map, TagId};
use serde::{Deserialize, Serialize};
use tags::geometry::TagView;
use tags::wire::StringId;

use crate::live_models::is_id;

type MapReader = Map<BufReader<File>>;

const PROJ: FourCC = FourCC(*b"proj");

/// `weap`: the string ID naming the animation class it is held with (the
/// same field `live_models` reads as `WEAPON_CLASS`).
const WEAPON_CLASS: usize = 0x288;
/// `weap`: triggers block and stride; primary barrel index in a trigger.
const TRIGGERS: usize = 0x2c8;
const TRIGGER_STRIDE: usize = 0x40;
const TRIGGER_PRIMARY_BARREL: usize = 0x8;
/// `weap`: barrels block and stride.
const BARRELS: usize = 0x2d0;
const BARREL_STRIDE: usize = 0xec;
/// Barrel fields: rounds per second (min, max f32), shots per fire (min,
/// max i16), fire recovery time (f32), projectile reference (FourCC, datum).
const BARREL_ROUNDS_PER_SECOND: usize = 0x04;
const BARREL_SHOTS_PER_FIRE: usize = 0x1c;
const BARREL_FIRE_RECOVERY: usize = 0x20;
const BARREL_PROJECTILE: usize = 0x8c;
/// `proj`: initial and final velocity, world units per second.
const PROJ_INITIAL_VELOCITY: usize = 0x17c;
const PROJ_FINAL_VELOCITY: usize = 0x180;

/// Projectiles at least this fast (world units per second) look instant at
/// viewer scale: bullets are 400, sniper and beam rounds 1200, plasma tens.
/// (The viewer decides from `kind` and `speed` itself; the disc test reports it.)
#[cfg(test)]
pub const HITSCAN_SPEED: f32 = 300.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ballistics {
    /// "bullet" | "plasma" | "needle" | "rocket" | "grenade" | "beam" | "melee"
    pub kind: String,
    /// Initial projectile speed, world units per second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    /// Final projectile speed, world units per second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_speed: Option<f32>,
    /// Sustained rounds per second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<f32>,
}

#[cfg(test)]
impl Ballistics {
    /// Whether the round is fast enough to draw as an instant tracer.
    pub fn hitscan(&self) -> bool {
        self.speed.is_some_and(|s| s >= HITSCAN_SPEED)
    }
}

/// The kind of round a weapon fires, from its tag file stem, its animation
/// class and its projectile's initial speed.
pub fn kind_for(stem: &str, class: Option<&str>, speed: Option<f32>) -> String {
    let stem = stem.to_ascii_lowercase();
    let has = |s: &str| stem.contains(s);
    let kind = if class == Some("sword") || has("energy_blade") || has("sword") {
        "melee"
    } else if has("needle") {
        "needle"
    } else if has("beam") {
        "beam"
    } else if has("rocket") || has("flak") || has("fuel_rod") {
        "rocket"
    } else if has("brute_shot") || has("grenade") {
        "grenade"
    } else if has("plasma") || speed.is_some_and(|s| s < 100.0) {
        "plasma"
    } else {
        "bullet"
    };
    kind.to_owned()
}

/// Sustained rounds per second from a barrel's rounds per second (max),
/// shots per fire (max; 0 = automatic) and fire recovery time: automatic
/// fire is the rounds per second; a burst of `n` takes `n / rps` plus the
/// recovery; a semi-automatic barrel (no rounds per second) fires its shots
/// once per recovery time. `None` when nothing bounds it.
pub fn sustained_rate(rounds_per_second: f32, shots_per_fire: i16, recovery: f32) -> Option<f32> {
    let rps =
        (rounds_per_second.is_finite() && rounds_per_second > 0.0).then_some(rounds_per_second);
    let rec = if recovery.is_finite() && recovery > 0.0 {
        recovery
    } else {
        0.0
    };
    let shots = f32::from(shots_per_fire.max(1));
    match rps {
        Some(r) if shots_per_fire <= 0 => Some(r),
        Some(r) => Some(shots / (shots / r + rec)),
        None if rec > 0.0 => Some(shots / rec),
        None => None,
    }
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn f32_at(b: &[u8], o: usize) -> Option<f32> {
    u32_at(b, o).map(f32::from_bits)
}
fn i16_at(b: &[u8], o: usize) -> Option<i16> {
    Some(i16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn finite(v: Option<f32>) -> Option<f32> {
    v.filter(|v| v.is_finite())
}

/// A weapon's animation class (see [`WEAPON_CLASS`]), when it reads as an id.
fn weapon_class(map: &MapReader, weap: &[u8]) -> Option<String> {
    let id = StringId(u32_at(weap, WEAPON_CLASS)?);
    let name = id.resolve(map.strings()).ok().flatten()?;
    is_id(name).then(|| name.to_owned())
}

/// The weapon's file stem, lower case.
fn stem(map: &MapReader, weapon: TagId) -> String {
    let name = map
        .tag(weapon)
        .ok()
        .and_then(|t| t.name.clone())
        .unwrap_or_default();
    name.rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// The ballistics of a weapon: its primary trigger's barrel (barrel 0 when
/// the trigger is out of reach), that barrel's projectile and rate. A weapon
/// without barrels is `melee` when it is a sword, otherwise `None`.
pub fn ballistics(map: &mut MapReader, weapon: TagId) -> Option<Ballistics> {
    let address = map.tag(weapon).ok()?.location?.address;
    let b = map.read_tag_bytes(weapon).ok()?;
    let view = TagView::new(&b, address);
    let class = weapon_class(map, &b);
    let stem = stem(map, weapon);
    let barrels = view.block(&b, BARRELS, BARREL_STRIDE).ok()?;
    if barrels.is_empty() {
        let kind = kind_for(&stem, class.as_deref(), None);
        return (kind == "melee").then_some(Ballistics {
            kind,
            speed: None,
            final_speed: None,
            rate: None,
        });
    }
    let primary = view
        .block(&b, TRIGGERS, TRIGGER_STRIDE)
        .ok()
        .and_then(|t| i16_at(t.first()?, TRIGGER_PRIMARY_BARREL))
        .and_then(|i| usize::try_from(i).ok())
        .filter(|&i| i < barrels.len())
        .unwrap_or(0);
    let barrel = barrels[primary];
    let rate = sustained_rate(
        f32_at(barrel, BARREL_ROUNDS_PER_SECOND + 4).unwrap_or(0.0),
        i16_at(barrel, BARREL_SHOTS_PER_FIRE + 2).unwrap_or(0),
        f32_at(barrel, BARREL_FIRE_RECOVERY).unwrap_or(0.0),
    );
    let (speed, final_speed) = projectile_speeds(map, barrel).unwrap_or((None, None));
    Some(Ballistics {
        kind: kind_for(&stem, class.as_deref(), speed),
        speed,
        final_speed,
        rate,
    })
}

/// The initial and final velocity of the projectile a barrel references.
fn projectile_speeds(map: &mut MapReader, barrel: &[u8]) -> Option<(Option<f32>, Option<f32>)> {
    if u32_at(barrel, BARREL_PROJECTILE)? != u32::from_be_bytes(PROJ.0) {
        return None;
    }
    let id = TagId(u32_at(barrel, BARREL_PROJECTILE + 4)?);
    if map.tag(id).ok()?.group != PROJ {
        return None;
    }
    let p = map.read_tag_bytes(id).ok()?;
    Some((
        finite(f32_at(&p, PROJ_INITIAL_VELOCITY)),
        finite(f32_at(&p, PROJ_FINAL_VELOCITY)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_from_invented_weapons() {
        assert_eq!(kind_for("anything", Some("sword"), None), "melee");
        assert_eq!(kind_for("energy_blade", None, None), "melee");
        assert_eq!(kind_for("needler", Some("pistol"), Some(6.0)), "needle");
        assert_eq!(kind_for("beam_rifle", Some("rifle"), Some(1200.0)), "beam");
        assert_eq!(kind_for("rocket_launcher", None, Some(8.0)), "rocket");
        assert_eq!(kind_for("flak_cannon", None, Some(15.0)), "rocket");
        assert_eq!(kind_for("brute_shot", None, Some(20.0)), "grenade");
        assert_eq!(kind_for("plasma_rifle", None, Some(14.0)), "plasma");
        assert_eq!(kind_for("Plasma_Pistol", None, None), "plasma");
        assert_eq!(kind_for("mystery_gun", None, Some(30.0)), "plasma");
        assert_eq!(kind_for("mystery_gun", None, Some(400.0)), "bullet");
        assert_eq!(kind_for("mystery_gun", None, None), "bullet");
    }

    #[test]
    fn sustained_rates_from_invented_barrels() {
        // Automatic: the rounds per second.
        assert_eq!(sustained_rate(12.0, 0, 0.0), Some(12.0));
        // A burst of 4 at 20/s (0.2 s) then 0.3 s recovery: 4 in 0.5 s.
        assert_eq!(sustained_rate(20.0, 4, 0.3), Some(8.0));
        // Semi-automatic: one per recovery.
        assert_eq!(sustained_rate(0.0, 1, 0.25), Some(4.0));
        assert_eq!(sustained_rate(0.0, 0, 0.5), Some(2.0));
        // Unbounded or garbage.
        assert_eq!(sustained_rate(0.0, 1, 0.0), None);
        assert_eq!(sustained_rate(f32::NAN, 1, f32::NAN), None);
    }

    #[test]
    fn ballistics_serialises_without_missing_fields() {
        let b = Ballistics {
            kind: "melee".into(),
            speed: None,
            final_speed: None,
            rate: None,
        };
        assert_eq!(serde_json::to_string(&b).unwrap(), r#"{"kind":"melee"}"#);
        let back: Ballistics = serde_json::from_str(r#"{"kind":"bullet","speed":400.0}"#).unwrap();
        assert_eq!(back.speed, Some(400.0));
        assert!(back.hitscan());
        assert!(!b.hitscan());
    }

    /// Writes every multiplayer weapon's ballistics to
    /// `$RUSH_KEEP/viewer-models-fix/weapons.txt`, outside the repository:
    /// `cargo test --release -p cli --bin h2 weapons_from_the_disc -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn weapons_from_the_disc() {
        use crate::live_art::Disc;
        use map::CacheType;
        use std::fmt::Write as _;
        use std::path::Path;

        let disc_path = std::env::var("H2_DISC").unwrap_or_else(|_| {
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../../disc/halo2.iso").into()
        });
        let keep = std::env::var("RUSH_KEEP")
            .unwrap_or_else(|_| "/Users/afr/.config/rush/keep/h2recomp-b5afe71a/bed6c8dc".into());
        let dir = Path::new(&keep).join("viewer-models-fix");
        std::fs::create_dir_all(&dir).unwrap();
        let disc = Disc::scan(Path::new(&disc_path)).unwrap();
        let weap = FourCC(*b"weap");
        let mut seen: Vec<String> = Vec::new();
        let mut out = String::new();
        let maps = disc
            .maps_of(CacheType::Shared)
            .chain(disc.maps_of(CacheType::Multiplayer));
        for m in maps {
            let Ok((mut map, _)) = disc.open(m) else {
                continue;
            };
            let weapons: Vec<(TagId, String)> = map
                .tags()
                .iter()
                .filter(|t| t.group == weap && t.location.is_some())
                .filter_map(|t| Some((t.id, t.name.clone()?)))
                .filter(|(_, n)| {
                    let n = n.to_ascii_lowercase();
                    n.starts_with("objects\\weapons\\") && !n.contains("\\fixed\\")
                })
                .collect();
            for (id, name) in weapons {
                let s = stem(&map, id);
                if seen.contains(&s) || !is_id(&s) {
                    continue;
                }
                seen.push(s.clone());
                let b = map.read_tag_bytes(id).unwrap();
                let class = weapon_class(&map, &b).unwrap_or_default();
                let got = ballistics(&mut map, id);
                let json = got
                    .as_ref()
                    .map(|g| serde_json::to_string(g).unwrap())
                    .unwrap_or_else(|| "none".into());
                let hitscan = got.as_ref().is_some_and(Ballistics::hitscan);
                let _ = writeln!(
                    out,
                    "{s:<24} class {class:<10} hitscan {hitscan:<5} {json}  ({name})"
                );
            }
        }
        print!("{out}");
        std::fs::write(dir.join("weapons.txt"), &out).unwrap();
        assert!(seen.iter().any(|s| s == "battle_rifle"), "{seen:?}");
    }
}
