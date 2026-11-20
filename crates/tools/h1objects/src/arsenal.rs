//! Weapons, equipment, projectiles and damage effects written from their
//! typed definitions ([`tags::defs`]: `weap`, `eqip`, `proj`, `jpt!`).
//! Halo 1 supplies the source values and references. Fields without a mapping
//! keep the definition default.
//!
//! Evidence and the field-by-field proof: `docs/formats/tag-definitions.md`
//! (Weapons, equipment, projectiles and damage) and
//! `tests/weapon_rebuild_disc.rs`.

use map::build::Block;
use map::{FourCC, TagId};
use tags::defs::{self, Defs, Record, StructDef, Val};

use crate::h1;
use crate::rebuild::{self, Predicted};
use crate::{Error, Result};

/// `obje +0` for each group (docs/formats/objects.md).
pub const WEAPON_TYPE: u32 = 2;
pub const EQUIPMENT_TYPE: u32 = 3;
pub const PROJECTILE_TYPE: u32 = 5;

/// Disable the binocular fallback when the weapon has no scope.
const NO_BINOCULAR_ZOOM: u32 = 1 << 16;

/// `weap` root words written from Halo 1 by offset, as the removed template path
/// did: (path, Halo 1 `weap` offset). The aim assist is E (`0x1a50a0`
/// reads `+0x208..+0x218`); the heat words are S, V2
/// (docs/halo1/objects.md).
pub const WEAPON_WORDS: &[(&str, usize)] = &[
    ("unknown_23c", 0x418), // light power-on time
    ("unknown_240", 0x41c), // light power-off time
    ("unknown_13c", 0x338), // ready time, immediately before the ready effect
    ("unknown_150", 0x34c), // heat recovery threshold
    ("unknown_154", 0x350), // overheated threshold
    ("unknown_158", 0x354), // heat detonation threshold
    ("unknown_15c", 0x358), // heat detonation fraction
    ("unknown_160", 0x35c), // heat loss per second
    ("unknown_164", 0x360), // heat illumination
    ("unknown_254", 0x440), // battery age slows heat recovery
    ("unknown_258", 0x444), // battery age slows firing
    ("unknown_25c", 0x448), // age at which misfires start
    ("unknown_260", 0x44c), // misfire chance at full age
    ("unknown_208", 0x3e4), // aim cone angle
    ("unknown_20c", 0x3e8), // aim cone range
    ("unknown_210", 0x3ec), // pull angle
    ("unknown_214", 0x3f0), // pull range
    ("unknown_218", 0x3f4), // bend angle
];

/// Barrel words from the Halo 1 trigger by offset (S, V2; the template
/// path's rows): (path, Halo 1 trigger offset). Two `i16`s share a word
/// where the offsets say so.
pub const BARREL_WORDS: &[(&str, usize)] = &[
    ("unknown_4", 0x4),   // rounds per second, from
    ("unknown_8", 0x8),   // rounds per second, to
    ("unknown_c", 0xc),   // firing acceleration time
    ("unknown_10", 0x10), // firing deceleration time
    ("unknown_18", 0x14), // spin fraction for the firing transition
    ("unknown_28", 0x20), // i16 magazine, i16 rounds per shot
    ("unknown_2c", 0x24), // i16 minimum rounds loaded, i16 rounds between tracers
    ("unknown_38", 0x38), // error acceleration time
    ("unknown_3c", 0x3c), // error deceleration time
    // H2 blends two error profiles (0xff474, 0x104dd1); H1 has one.
    ("unknown_48", 0x38),
    ("unknown_4c", 0x3c),
    ("unknown_58", 0x78),
    ("unknown_5c", 0x7c),
    ("unknown_60", 0x80),
    ("unknown_68", 0x6c), // i16 distribution function, i16 projectiles per shot
    ("unknown_6c", 0x70), // distribution angle
    ("unknown_70", 0x78), // minimum error
    ("unknown_74", 0x7c), // error angle, from
    ("unknown_78", 0x80), // error angle, to
    ("unknown_7c", 0x84), // first-person offset
    ("unknown_80", 0x88),
    ("unknown_84", 0x8c),
    ("unknown_9c", 0xa4), // ejection port recovery time
    ("unknown_a0", 0xa8), // illumination recovery time
    ("unknown_a4", 0xb8), // heat per round
    ("unknown_a8", 0xbc), // battery age per round
];

/// `proj` words from Halo 1 by offset (S, V2; the removed template path's rows):
/// (path, Halo 1 `proj` offset, per tick to per second). Halo 1 keeps
/// speeds per tick ([`crate::convert::TICKS_PER_SECOND`]).
pub const PROJECTILE_WORDS: &[(&str, usize, bool)] = &[
    ("unknown_bc", 0x17c, false), // projectile flags; both grenade pairs agree
    ("unknown_c0", 0x180, false), // i16 detonation timer starts, i16 impact noise
    ("unknown_c4", 0x19c, false), // AI perception radius
    ("unknown_cc", 0x1a4, false), // arming time
    ("unknown_d0", 0x1a8, false), // danger radius
    ("unknown_d4", 0x1bc, false), // timer, from
    ("unknown_d8", 0x1c0, false), // timer, to
    ("unknown_dc", 0x1c4, true),  // minimum velocity
    ("unknown_164", 0x1cc, false), // air gravity scale
    ("unknown_168", 0x1d0, false), // air damage range, from
    ("unknown_16c", 0x1d4, false), // air damage range, to
    ("unknown_170", 0x1d8, false), // water gravity scale
    ("unknown_174", 0x1dc, false), // water damage range, from
    ("unknown_178", 0x1e0, false), // water damage range, to
    ("unknown_17c", 0x1e4, true), // initial velocity
    ("unknown_180", 0x1e8, true), // final velocity
    ("unknown_184", 0x1ec, true), // the two bounds H2 samples at 0xf82e8
    ("unknown_188", 0x1ec, true),
];

/// Halo 1's `jpt!` damage: lower bound `+0x1d0`, upper bound `+0x1d4`
/// (from, to). V: tag-definitions.md, five retail pairs.
pub const H1_DAMAGE_LOWER: usize = 0x1d0;
pub const H1_DAMAGE_UPPER: usize = 0x1d4;

fn def(group: &[u8; 4]) -> Result<&'static StructDef> {
    Defs::builtin()
        .group(FourCC(*group))
        .ok_or_else(|| Error::Invalid(format!("no definition for {}", FourCC(*group))))
}

fn set(r: &mut Record, path: &str, v: Val) -> Result<()> {
    r.set(path, v)
        .map_err(|e| Error::Invalid(format!("{path}: {e}")))
}

fn built(d: &StructDef, r: &Record) -> Result<Block> {
    defs::to_built(d, r, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))
}

fn word(r: &mut Record, path: &str, w: u32) -> Result<()> {
    set(r, path, Val::Bytes(w.to_le_bytes().to_vec()))
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

/// Converted references, including explicit nulls, by definition path.
pub type Halo1Refs = Vec<(String, FourCC, TagId)>;

pub(crate) fn refs(r: &mut Record, list: &[(String, FourCC, TagId)]) -> Result<()> {
    for (path, group, id) in list {
        set(r, path, Val::tag(*group, *id))?;
    }
    Ok(())
}

/// Halo 1 references in slots named by the retail tags they hold. V
/// (`tests/role_audit_disc.rs`, Blood Gulch on Coagulation, the
/// counterparts' references by slot):
///
/// - `weap +0x16c`: the plasma pistol's and plasma rifle's `overheated`
///   effects; Halo 1's `weap +0x374` overheated effect;
/// - `weap +0x1bc`: `smash_melee` or `strike_melee` on 8 of 8 weapons,
///   `+0x1c4` their `_melee_response` on 8 of 8; Halo 1's `weap +0x394`
///   melee damage and `+0x3a4` melee response. The five slots after
///   (`+0x1cc` .. `+0x1ec`, more `strike`/`smash` damage) are Halo 2
///   melee kinds Halo 1 has no counterpart for;
/// - `proj +0xe8`: the plasma grenade's `detonation_start`; Halo 1's
///   `proj +0x1f4` detonation started;
/// - `proj +0xf0`: `airborne_detonation` on all 3 that have one, `+0xf8`
///   the detonation (`detonation`, `standard_detonation`, the frag's
///   `airborne_detonation`): Halo 1 has one detonation effect
///   (`proj +0x1ac`), which takes both;
/// - `proj +0x110`: the needle's `super_detonation`; Halo 1's
///   `proj +0x18c` super detonation;
/// - `proj +0x138`: `bullet_flyby_*` on 8 bullets; Halo 1's `proj +0x204`
///   flyby sound.
pub const WEAPON_OVERHEATED: &str = "overheated_effect";
pub const WEAPON_MELEE_DAMAGE: &str = "ref_1bc";
pub const WEAPON_MELEE_RESPONSE: &str = "ref_1c4";
pub const PROJECTILE_DETONATION_STARTED: &str = "ref_e8";
pub const PROJECTILE_AIRBORNE_DETONATION: &str = "ref_f0";
pub const PROJECTILE_DETONATION: &str = "ref_f8";
pub const PROJECTILE_SUPER_DETONATION: &str = "ref_110";
pub const PROJECTILE_FLYBY_SOUND: &str = "ref_138";

/// `obje +0x54`, the material effects (`foot`), under each group's
/// object header.
pub const ITEM_FOOT: &str = "item.obje.foot_ref_54";
pub const PROJECTILE_FOOT: &str = "object.foot_ref_54";

/// One Halo 1 slot: `path` of `group` takes `id`, or none.
pub fn halo1_ref(path: &str, group: &[u8; 4], id: Option<TagId>) -> (String, FourCC, TagId) {
    (path.to_owned(), FourCC(*group), id.unwrap_or(TagId::NULL))
}

/// The item's pickup ordering word and the delay before a resting item detonates.
pub(crate) fn item_words(r: &mut Record, b: &[u8]) -> Result<()> {
    for (path, at) in [
        ("item.unknown_bc", 0x17c),
        ("item.unknown_c0", 0x180),
        ("item.unknown_114", 0x2e0),
        ("item.unknown_118", 0x2e4),
    ] {
        word(r, path, u32_at(b, at))?;
    }
    Ok(())
}

fn header(
    r: &mut Record,
    path: &str,
    kind: u32,
    obj: &h1::Object,
    hlmt: Option<TagId>,
) -> Result<()> {
    rebuild::object_tuning(r, path, obj)?;
    set(r, &format!("{path}.object_type"), Val::Enum(kind))?;
    set(
        r,
        &format!("{path}.bounding_radius"),
        Val::Float(obj.bounding_radius),
    )?;
    set(
        r,
        &format!("{path}.bounding_offset"),
        Val::floats(&obj.bounding_offset),
    )?;
    set(
        r,
        &format!("{path}.model"),
        Val::tag(FourCC(*b"hlmt"), hlmt.unwrap_or(TagId::NULL)),
    )
}

/// What a rebuilt weapon references.
#[derive(Clone, Debug, Default)]
pub struct WeaponParts {
    pub hlmt: Option<TagId>,
    pub predicted: Vec<Predicted>,
    /// Per Halo 1 trigger, its converted projectile.
    pub projectiles: Vec<Option<TagId>>,
    pub charging_effect: Option<TagId>,
    /// Halo 1's first-person model and animations, converted.
    pub first_person: (Option<TagId>, Option<TagId>),
    pub halo1: Halo1Refs,
    /// Per Halo 1 trigger, its firing effects, converted ([`FiringParts`]).
    pub firing_effects: Vec<Vec<FiringParts>>,
    /// Per Halo 1 magazine, its reloading and chambering effects,
    /// converted.
    pub magazine_effects: Vec<[Option<(FourCC, TagId)>; 2]>,
    pub magazine_objects: Vec<Vec<(i16, TagId)>>,
    /// The weapon class and type a unit's graph holds it by
    /// ([`WEAPON_CLASS`], [`WEAPON_TYPE_NAME`]); `None` leaves both null.
    pub animation: Option<(String, String)>,
}

/// One firing effect of a barrel (`barrels.block_e4`, `0x34`) from Halo
/// 1's trigger firing effect (`h1::FiringEffect`): its converted firing,
/// misfire and empty effects and their Halo 1 rumble and shake. A shot plays the effect: until converter
/// version 97 the effects were the counterpart's too, so the assault
/// rifle fired with the SMG's sound and a weapon with no counterpart (the
/// vehicle guns) had no firing effect and fired silent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FiringParts {
    /// Each with its group: Halo 1 names a `snd!` as well as an `effe`
    /// here (the plasma rifle's empty effect), and the Halo 2 slot takes
    /// either ([`FIRING_EFFECT_REFS`]).
    pub effects: [Option<(FourCC, TagId)>; 3],
    pub damage: [Option<TagId>; 3],
    pub shot_count: [i16; 2],
}

/// `barrels.block_e4` references, in Halo 1's order: firing, misfire and
/// empty effect (`effe` or `snd!`: retail Halo 2 names both there, and so
/// does Halo 1's element), then
/// firing, misfire and empty damage (`jpt!`). V: on every retail weapon
/// the first three hold `effe` or `snd!` and the last three `jpt!`
/// (`weapon_sounds_disc`); names S, by Halo 1's order.
pub const FIRING_EFFECT_REFS: [&str; 3] = ["ref_4", "ref_c", "ref_14"];
pub const FIRING_DAMAGE_REFS: [&str; 3] = ["ref_1c", "ref_24", "ref_2c"];
/// [`FIRING_DAMAGE_REFS`]' offsets in the element.
pub const FIRING_DAMAGE_AT: [usize; 3] = [0x1c, 0x24, 0x2c];

/// Magazine `+0x34`: the reloading effect (`effe`). V: retail's SMG
/// names an effect here; Halo 1's magazine `+0x38`.
pub const MAGAZINE_RELOADING_EFFECT: &str = "reloading_effect";
/// Magazine `+0x44`: the chambering effect. S: it sits between two `jpt!`
/// group words (`+0x3c`, `+0x4c`) as the reloading effect does before
/// `+0x3c`; Halo 1's magazine `+0x48`.
pub const MAGAZINE_CHAMBERING_EFFECT: &str = "chambering_effect";

/// The weapon's own effects and sounds, written from Halo 1: (Halo 2
/// path, Halo 1 `weap` offset of the reference). The Halo 2 slots
/// (docs/halo1/objects.md, "Weapon effects and sounds"): every retail
/// weapon on Coagulation names only `effe` in the effects and `snd!` in
/// the sounds (`needler\ready`, `plasma_rifle\overheated`,
/// `smg\smg_ammo`, `sniper_rifle\zoom_in`, `zoom_out`); E: the zoom
/// sounds are read at `0x000c5ba0` (out, `+0x278`) and `0x000c5ba8` (in,
/// `+0x270`) by magnification, the pickup sound at `0x000a009a`
/// (`+0x268`, played by `0x001896c0` for a weapon), the overheated and
/// detonation effects at `0x000cf12c` (`+0x170`) and `0x000cf19f`
/// (`+0x180`); each skips a null datum. Until converter version 97 these
/// were the counterpart's: drawing the assault rifle played the needler's
/// ready effect (the SMG's), picking it up the SMG's ammunition sound.
pub const WEAPON_EFFECTS: &[(&str, usize)] = &[
    ("ready_effect", 0x33c),
    ("overheated_effect", 0x374),
    ("detonation_effect", 0x384),
    ("light_power_on_effect", 0x420),
    ("light_power_off_effect", 0x430),
    ("pickup_sound", 0x490),
    ("zoom_in_sound", 0x4a0),
    ("zoom_out_sound", 0x4b0),
    ("item.collision_sound", 0x258),
    ("item.detonating_effect", 0x2e8),
    ("item.detonation_effect", 0x2f8),
];

/// `weap +0x288`, `+0x28c`: the weapon class and type (string IDs) the
/// holder's animation graph is looked up by. A unit takes a weapon only
/// when its graph has an animation for them: `0x000cd6a0` (called by
/// `unit_add_weapon` `0x000cd0c0` at `0x000cd12e`, and by the pickup
/// paths at `0x000e8a63` and `0x000ea475`) passes them with mode
/// `current` (sid `0x101`, or the seat's) to `0x0010fcd0`, which asks
/// `0x001cc3f0` for action `idle` (sid `0xc`) and refuses on none. Retail:
/// the SMG `rifle`/`smg`, the magnum `pistol`/`hp`, the rocket launcher
/// `missile`/`rl`, vehicle guns `fixed`/`gun`.
pub const WEAPON_CLASS: &str = "string_288";
pub const WEAPON_TYPE_NAME: &str = "string_28c";

/// Magazine `+0xc` (i16): the most rounds held in reserve. E: the reserve
/// (`W+0x22a`) is capped with it (`0x101a10`, docs/game/skulls.md,
/// Unlimited Ammo). V: on all seven retail magazines on Coagulation it is
/// the total maximum (`+0x8`) less the loaded maximum (`+0xa`): 144 - 36
/// on the battle rifle, 60 - 12 on the magnum, 24 - 4 on the sniper rifle.
/// Until converter version 76 it was 0, so a converted weapon could hold
/// no reserve: the HUD's count read 0 and nothing reloaded.
pub const MAGAZINE_RESERVE_MAXIMUM: &str = "unknown_c";

/// [`MAGAZINE_RESERVE_MAXIMUM`] for a magazine of `total` rounds of which
/// `loaded` sit in the weapon.
pub fn reserve_maximum(total: i16, loaded: i16) -> u16 {
    total.saturating_sub(loaded).max(0) as u16
}

/// `weap +0x2c8`: the triggers (`0x40` each). A weapon fires only through
/// one: each names the barrels (`+0x2d0`) a press fires. Every retail
/// weapon on Coagulation that has a barrel has at least one trigger (V,
/// 27 of 27); the rebuild from definitions (`dac88db6`) wrote
/// none, so no converted Halo 1 weapon fired (`h2_triggers`).
pub const TRIGGERS: &str = "block_2c8";

/// One trigger's first 12 words (`+0x0..+0x30`; the two references after
/// stay null).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TriggerWords(pub [u32; 12]);

impl TriggerWords {
    fn i16s(lo: i16, hi: i16) -> u32 {
        u32::from(lo as u16) | u32::from(hi as u16) << 16
    }

    /// `+0x4` i16 input: 0 on every retail weapon's first trigger, 1 on the
    /// scorpion cannon's second (its machine gun), 2 on the banshee's. `+0x6` i16 firing behaviour: 0 on the
    /// automatic weapons (SMG, plasma rifle, needler, turrets), 1 on the
    /// ones that fire once per press (magnum, shotgun, sniper rifle, beam
    /// rifle, the scorpion's cannon), 2 on the plasma pistol, which
    /// charges. `+0x8` i16 primary barrel, `+0xa` i16 secondary barrel
    /// (-1 none; 1 on the plasma pistol, its charged shot). `+0xc` i16: 1
    /// with behaviour 0 (all but the wraith's minigun, 0), 0 with 1, 2
    /// with 2. All V, Coagulation's retail weapons.
    pub fn new(input: i16, behaviour: i16, primary: i16, secondary: i16) -> Self {
        let mut w = [0u32; 12];
        w[1] = Self::i16s(input, behaviour);
        w[2] = Self::i16s(primary, secondary);
        w[3] = Self::i16s(behaviour_pair(behaviour), 0);
        Self(w)
    }

    pub fn input(&self) -> i16 {
        self.0[1] as i16
    }
    pub fn behaviour(&self) -> i16 {
        (self.0[1] >> 16) as i16
    }
    pub fn primary(&self) -> i16 {
        self.0[2] as i16
    }
    pub fn secondary(&self) -> i16 {
        (self.0[2] >> 16) as i16
    }
    /// `+0x1c` f32: seconds to charge. V: the plasma pistol's 0.6, as Halo
    /// 1's first trigger `+0x48` holds on its plasma pistol.
    pub fn charging_time(&self) -> f32 {
        f32::from_bits(self.0[7])
    }
    /// `+0x20` f32: charged time, `+0x24` i16: overcharged action,
    /// `+0x28` f32: charged illumination, `+0x2c` f32: spew time. S for
    /// the names (the order Halo 1's trigger keeps from `+0x4c`,
    /// [`H1_TRIGGER_CHARGED`]); V: retail's plasma pistol, the only charging
    /// trigger on Coagulation, holds 0, 0, 1.0 and 0 there, and its
    /// charging time (`+0x1c`, 0.6) is Halo 1's `+0x48`.
    pub fn charged_time(&self) -> f32 {
        f32::from_bits(self.0[8])
    }
    pub fn overcharged_action(&self) -> i16 {
        self.0[9] as i16
    }
    pub fn charged_illumination(&self) -> f32 {
        f32::from_bits(self.0[10])
    }
    pub fn spew_time(&self) -> f32 {
        f32::from_bits(self.0[11])
    }
}

/// `+0xc` for a firing behaviour ([`TriggerWords::new`]).
fn behaviour_pair(behaviour: i16) -> i16 {
    match behaviour {
        0 => 1,
        2 => 2,
        _ => 0,
    }
}

/// H1's once-per-press flag, used by the rocket launcher, sniper rifle
/// and Scorpion cannon on Blood Gulch. The shotgun has its own weapon type.
pub const H1_TRIGGER_ONE_PER_PRESS: u32 = 0x8;
/// Halo 1 trigger `+0x48` f32: seconds to charge (V: 0.6 on the plasma
/// pistol's first trigger, the only nonzero one on Blood Gulch, and Halo
/// 2's plasma pistol charges for 0.6 at trigger `+0x1c`).
pub const H1_TRIGGER_CHARGING_TIME: usize = 0x48;
/// Halo 1 trigger `+0x4c..+0x5c`, after the charging time: f32 charged
/// time, i16 overcharged action (and 2 bytes of pad), f32 charged
/// illumination, f32 spew time; then the charging effect's reference at
/// `+0x5c`. S for the names; V for the values on Blood Gulch's plasma
/// pistol, the only trigger that charges: 45.0, 0, 1.0, 0, and no charging
/// effect. They become Halo 2's trigger `+0x20..+0x30` word for word
/// ([`TriggerWords::charged_time`]): the plasma pistol's charged
/// illumination is 1.0 in both games.
pub const H1_TRIGGER_CHARGED: usize = 0x4c;

/// The H1 shotgun is weapon type 1 in both games, including the shell
/// reload path (H1 0xbf70f, H2 0x10271d). Its trigger flags are zero.
const SHOTGUN: i16 = 1;

fn single_shot(weapon_type: i16, flags: u32, barrel: usize) -> bool {
    flags & H1_TRIGGER_ONE_PER_PRESS != 0 || (weapon_type == SHOTGUN && barrel == 0)
}

/// Halo 2 trigger inputs for an ordinary weapon. The rebuild also knows the
/// weapon type, which distinguishes the shotgun from an automatic rifle.
pub fn h2_triggers(h1: &[Vec<u8>]) -> Vec<TriggerWords> {
    weapon_triggers(0, h1)
}

fn weapon_triggers(weapon_type: i16, h1: &[Vec<u8>]) -> Vec<TriggerWords> {
    if let Some(first) = h1.first() {
        let charge = u32_at(first, H1_TRIGGER_CHARGING_TIME);
        if f32::from_bits(charge) > 0.0 {
            // The plasma pistol fires barrel 1 at full charge. The fuel
            // rods charge and discharge the same barrel (H1 0xc0d61).
            let mut t = TriggerWords::new(0, 2, 0, i16::from(h1.len() > 1));
            t.0[6] = 1; // charging action, checked by H2 at 0xfe955
            t.0[7] = charge;
            for k in 0..4 {
                t.0[8 + k] = u32_at(first, H1_TRIGGER_CHARGED + 4 * k);
            }
            t.0[9] &= 0xffff;
            return vec![t];
        }
    }
    h1.iter()
        .take(2)
        .enumerate()
        .map(|(i, trigger)| {
            let single = single_shot(weapon_type, u32_at(trigger, 0), i);
            TriggerWords::new(i as i16, i16::from(single), i as i16, -1)
        })
        .collect()
}

/// Bits used by the disc's triggers, excluding the once-per-press bit
/// which moves to the trigger behaviour. The bits change positions
/// between engines (docs/halo1/objects.md, "Firing a rebuilt weapon").
fn barrel_flags(flags: u32) -> u32 {
    (flags & 0x4) | ((flags & 0x20) >> 2) | ((flags & 0x1c00) >> 5)
}

/// Halo 2 caches inverse seconds. Its two error profiles share H1's
/// times. The H2-only gun climb has zero duration.
fn barrel_rates(e: &mut Record, trigger: &[u8]) -> Result<()> {
    for (path, source) in [
        ("unknown_50", Some(0x38)), ("unknown_54", Some(0x3c)),
        ("unknown_c4", None), ("unknown_c8", None),
        ("unknown_cc", Some(0xa8)), ("unknown_d0", Some(0xa4)),
        ("unknown_d4", Some(0xc)), ("unknown_d8", Some(0x10)),
        ("unknown_dc", Some(0x38)), ("unknown_e0", Some(0x3c)),
    ] {
        let seconds = source.map_or(0.0, |at| f32::from_bits(u32_at(trigger, at)));
        let rate = if seconds > 0.0 { seconds.recip() } else { 1000.0 };
        word(e, path, rate.to_bits())?;
    }
    Ok(())
}

/// The `weap` root from Halo 1's weapon `w` (the bytes of its tag: `b`).
pub fn weapon(w: &h1::Weapon, b: &[u8], parts: &WeaponParts) -> Result<Block> {
    let trig = h1_triggers(b)?;
    if trig.len() != w.triggers.len() || trig.len() > 2 {
        return Err(Error::Invalid(format!(
            "expected at most two matching triggers, got {} decoded and {} raw",
            w.triggers.len(), trig.len()
        )));
    }
    let charging = trig.first().is_some_and(|t| {
        f32::from_bits(u32_at(t, H1_TRIGGER_CHARGING_TIME)) > 0.0
    });
    let d = def(b"weap")?;
    let mut r = defs::default_record(d);
    refs(&mut r, &parts.halo1)?;
    item_words(&mut r, b)?;
    header(&mut r, "item.obje", WEAPON_TYPE, &w.object, parts.hlmt)?;
    if let Some((class, kind)) = &parts.animation {
        set(&mut r, WEAPON_CLASS, Val::text(class))?;
        set(&mut r, WEAPON_TYPE_NAME, Val::text(kind))?;
    }
    rebuild::predicted_resources(b"weap", &mut r, "item.obje", &parts.predicted)?;
    for (path, at) in WEAPON_WORDS {
        word(&mut r, path, u32_at(b, *at))?;
    }
    set(&mut r, "weapon_type", Val::Int(w.weapon_type.into()))?;
    let levels = w.magnification_levels.max(0);
    set(&mut r, "magnification_levels", Val::Int(levels.into()))?;
    let flags = (w.flags & 0xffff) | if levels == 0 { NO_BINOCULAR_ZOOM } else { 0 };
    set(&mut r, "weapon_flags", Val::Int(flags.into()))?;
    if levels > 0 {
        set(
            &mut r,
            "magnification_range",
            Val::floats(&w.magnification_range),
        )?;
    }

    // Magazines.
    let md = d.child("magazines").expect("weap magazines");
    let mut mags = Vec::new();
    for m in &w.magazines {
        let mut e = defs::default_record(md);
        set(
            &mut e,
            "unknown_4",
            Val::Bytes(m.rounds_recharged.to_le_bytes().to_vec()),
        )?;
        set(
            &mut e,
            "rounds_total_initial",
            Val::Int(m.rounds_total_initial.into()),
        )?;
        set(
            &mut e,
            "rounds_total_maximum",
            Val::Int(m.rounds_total_maximum.into()),
        )?;
        set(
            &mut e,
            "rounds_loaded_maximum",
            Val::Int(m.rounds_loaded_maximum.into()),
        )?;
        set(&mut e, "reload_time", Val::Float(m.reload_time))?;
        set(
            &mut e,
            "rounds_reloaded",
            Val::Int(m.rounds_reloaded.into()),
        )?;
        word(
            &mut e,
            MAGAZINE_RESERVE_MAXIMUM,
            u32::from(reserve_maximum(
                m.rounds_total_maximum,
                m.rounds_loaded_maximum,
            )),
        )?;
        word(&mut e, "unknown_0", m.flags)?;
        word(&mut e, "unknown_18", m.chamber_time.to_bits())?;
        if let Some(objects) = parts.magazine_objects.get(mags.len()) {
            let od = md.child("block_54").expect("magazine objects");
            let mut entries = Vec::new();
            for (rounds, id) in objects {
                let mut entry = defs::default_record(od);
                word(&mut entry, "unknown_0", u32::from(*rounds as u16))?;
                set(&mut entry, "ref_4", Val::tag(FourCC(*b"eqip"), *id))?;
                entries.push(entry);
            }
            set(&mut e, "block_54", Val::Block(entries))?;
        }
        if let Some(fx) = parts.magazine_effects.get(mags.len()) {
            for (path, x) in [MAGAZINE_RELOADING_EFFECT, MAGAZINE_CHAMBERING_EFFECT]
                .iter()
                .zip(fx)
            {
                let (g, id) = x.unwrap_or((FourCC(*b"effe"), TagId::NULL));
                set(&mut e, path, Val::tag(g, id))?;
            }
        }
        mags.push(e);
    }
    set(&mut r, "magazines", Val::Block(mags))?;
    // Barrels: one per Halo 1 trigger.
    let bd = d.child("barrels").expect("weap barrels");
    let fd = d.child("barrels.block_e4").expect("barrel firing effects");
    let mut barrels = Vec::new();
    for (i, t) in w.triggers.iter().enumerate() {
        let mut e = defs::default_record(bd);
        for (path, at) in BARREL_WORDS {
            word(&mut e, path, u32_at(&trig[i], *at))?;
        }
        barrel_rates(&mut e, &trig[i])?;
        let flags = u32_at(&trig[i], 0);
        if flags & !0x1c2c != 0 {
            return Err(Error::Invalid(format!("unsupported flags {flags:#x} on trigger {i}")));
        }
        word(&mut e, "unknown_0", barrel_flags(flags))?;
        let single = single_shot(w.weapon_type, flags, i);
        if single || charging {
            word(&mut e, "unknown_1c", 0x0001_0001)?; // one shot, regardless of pellet count
            let rate = f32::from_bits(u32_at(&trig[i], 4));
            if rate > 0.0 {
                word(&mut e, "unknown_20", rate.recip().to_bits())?;
            }
        }
        // The MP firing path at 0x10429b dispatches on prediction type.
        let prediction = if charging { 0 } else if single { 2 } else { 1 };
        set(&mut e, "prediction_type", Val::Int(prediction))?;
        set(&mut e, "firing_noise", Val::Int(t.firing_noise.into()))?;
        set(
            &mut e,
            "projectile",
            Val::tag(
                FourCC(*b"proj"),
                parts
                    .projectiles
                    .get(i)
                    .copied()
                    .flatten()
                    .unwrap_or(TagId::NULL),
            ),
        )?;
        let mut effects = Vec::new();
        for f in parts.firing_effects.get(i).into_iter().flatten() {
            let mut x = defs::default_record(fd);
            let [lo, hi] = f.shot_count;
            word(&mut x, "unknown_0", u32::from(lo as u16) | (u32::from(hi as u16) << 16))?;
            for (path, fx) in FIRING_EFFECT_REFS.iter().zip(f.effects) {
                let (g, id) = fx.unwrap_or((FourCC(*b"effe"), TagId::NULL));
                set(&mut x, path, Val::tag(g, id))?;
            }
            for (path, id) in FIRING_DAMAGE_REFS.iter().zip(f.damage) {
                set(&mut x, path, Val::tag(FourCC(*b"jpt!"), id.unwrap_or(TagId::NULL)))?;
            }
            effects.push(x);
        }
        set(&mut e, "block_e4", Val::Block(effects))?;
        barrels.push(e);
    }
    set(&mut r, "barrels", Val::Block(barrels))?;
    let td = d.child(TRIGGERS).expect("weap triggers");
    let mut triggers = Vec::new();
    for t in weapon_triggers(w.weapon_type, &trig) {
        let mut e = defs::default_record(td);
        for (k, w) in t.0.iter().enumerate() {
            if *w != 0 {
                word(&mut e, &format!("unknown_{:x}", 4 * k), *w)?;
            }
        }
        if t.behaviour() == 2 {
            if let Some(id) = parts.charging_effect {
                set(&mut e, "ref_30", Val::tag(FourCC(*b"effe"), id))?;
            }
        }
        triggers.push(e);
    }
    set(&mut r, TRIGGERS, Val::Block(triggers))?;
    // First person: one element per player character, as retail (the
    // Chief's, then the Arbiter's), both Halo 1's.
    let (model, anim) = parts.first_person;
    let pd = d.child("first_person").expect("weap first person");
    let mut fp = Vec::new();
    if model.is_some() || anim.is_some() {
        for _ in 0..2 {
            let mut e = defs::default_record(pd);
            set(
                &mut e,
                "model",
                Val::tag(FourCC(*b"mode"), model.unwrap_or(TagId::NULL)),
            )?;
            set(
                &mut e,
                "animations",
                Val::tag(FourCC(*b"jmad"), anim.unwrap_or(TagId::NULL)),
            )?;
            fp.push(e);
        }
    }
    set(&mut r, "first_person", Val::Block(fp))?;
    built(d, &r)
}

/// Halo 1's triggers' bytes, each `h1::TRIGGER_SIZE`, read by the caller
/// into `b`'s tail: this takes them from [`h1::Weapon`]'s reader layout.
fn h1_triggers(b: &[u8]) -> Result<Vec<Vec<u8>>> {
    // The caller passes the concatenated trigger elements after the root
    // ([`weapon_bytes`]).
    let root = h1::WEAP_SIZE;
    let bytes = b.get(root..).ok_or(Error::Truncated("weap root"))?;
    if bytes.len() % h1::TRIGGER_SIZE != 0 {
        return Err(Error::Truncated("weap trigger"));
    }
    Ok(bytes
        .chunks_exact(h1::TRIGGER_SIZE)
        .map(<[u8]>::to_vec)
        .collect())
}

/// Halo 1's `weap` root followed by its trigger elements, the form
/// [`weapon`] reads its by-offset words from.
pub fn weapon_bytes(map: &h1map::H1Map, tag: &h1map::Tag) -> Result<Vec<u8>> {
    let b = h1::tag_bytes(map, tag, b"weap", h1::WEAP_SIZE)?;
    let mut out = b[..h1::WEAP_SIZE].to_vec();
    let blk = h1map::Block::read(b, 0x4fc).ok_or(Error::Truncated("weap triggers"))?;
    out.extend_from_slice(map.block(blk, h1::TRIGGER_SIZE)?);
    Ok(out)
}

/// What a rebuilt projectile references.
#[derive(Clone, Debug, Default)]
pub struct ProjectileParts {
    pub hlmt: Option<TagId>,
    /// Converted Halo 1 damage: impact and detonation.
    pub impact_damage: Option<TagId>,
    pub detonation_damage: Option<TagId>,
    /// Halo 1's converted tags over those ([`Halo1Refs`]: detonation
    /// effects, flyby sound, material effects).
    pub halo1: Halo1Refs,
    /// The material responses ([`responses`]).
    pub responses: Vec<(&'static str, i16, Response)>,
}

/// Halo 1 computes air drag from the two velocities over its authored
/// air interval (0x000c1ac0). Halo 2 blends speed by distance over that
/// same interval (0x000f9527..0x000f95b4).
fn velocity_change_distance(b: &[u8]) -> ([f32; 2], f32) {
    let f = |at| f32::from_bits(u32_at(b, at));
    let bounds = [f(0x1d0), f(0x1d4)];
    let length = bounds[1] - bounds[0];
    if f(0x1e4) == f(0x1e8) {
        ([0.0, 0.0], 1.0)
    } else if length.is_finite() && length > 0.0 {
        (bounds, length.recip())
    } else {
        ([0.0, 0.0], 0.0)
    }
}

#[cfg(test)]
#[test]
fn projectile_velocity_transition_uses_air_distance() {
    let mut bytes = [0; h1::PROJ_SIZE];
    for (at, value) in [(0x1d0, 20.0f32), (0x1d4, 50.0), (0x1e4, 2.0), (0x1e8, 1.0)] {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let (bounds, inverse) = velocity_change_distance(&bytes);
    assert_eq!(bounds, [20.0, 50.0]);
    assert_eq!(inverse, 1.0 / 30.0);
    assert_eq!((35.0 - bounds[0]) * inverse, 0.5);
}

/// The `proj` root from Halo 1's projectile (its tag's bytes `b`).
pub fn projectile(
    p: &h1::Projectile,
    b: &[u8],
    parts: &ProjectileParts,
) -> Result<Block> {
    let d = def(b"proj")?;
    let mut r = defs::default_record(d);
    refs(&mut r, &parts.halo1)?;
    header(&mut r, "object", PROJECTILE_TYPE, &p.object, parts.hlmt)?;
    for (path, at, per_tick) in PROJECTILE_WORDS {
        let w = if *per_tick {
            (f32::from_bits(u32_at(b, *at)) * crate::convert::TICKS_PER_SECOND).to_bits()
        } else {
            u32_at(b, *at)
        };
        word(&mut r, path, w)?;
    }
    let (change_distance, inverse) = velocity_change_distance(b);
    set(&mut r, "velocity_change_distance", Val::floats(&change_distance))?;
    set(
        &mut r,
        "inverse_velocity_change_distance",
        Val::Float(inverse),
    )?;
    // Halo 1's detonation noise (`+0x1f0`, i16) in the low half of `+0xe4`.
    word(
        &mut r,
        "unknown_e4",
        u32::from(p.detonation_noise as u16),
    )?;
    set(
        &mut r,
        "material_responses",
        Val::Block(response_elements(d, &parts.responses)?),
    )?;
    set(&mut r, "collision_radius", Val::Float(p.collision_radius))?;
    set(&mut r, "maximum_range", Val::Float(p.maximum_range))?;
    if p.initial_velocity != p.final_velocity {
        // H2 compares this guidance scalar numerically against zero at
        // 0xf88df. Its unused sign distinguishes the H1 flight law.
        word(&mut r, "unknown_198", tags::H1_PROJECTILE_VELOCITY_CURVE)?;
    }
    let jpt = FourCC(*b"jpt!");
    set(
        &mut r,
        "impact_damage",
        Val::tag(jpt, parts.impact_damage.unwrap_or(TagId::NULL)),
    )?;
    set(
        &mut r,
        "detonation_damage",
        Val::tag(jpt, parts.detonation_damage.unwrap_or(TagId::NULL)),
    )?;
    built(d, &r)
}

/// Halo 1's projectile material responses: `proj +0x240`, elements of
/// `0xa0`, one per Halo 1 material type in [`MATERIALS`] order.
pub const H1_RESPONSES: usize = 0x240;
pub const H1_RESPONSE_SIZE: usize = 0xa0;

pub use h1convert::materials::MATERIALS;

/// The material every element not written for one material covers.
pub const DEFAULT_MATERIAL: &str = "default_material";

/// Halo 2's response for Halo 1's: disappear and detonate (0, 1) impact
/// (0), reflect (2) bounce (4), overpenetrate (3) overpenetrate (2),
/// attach (4) attach (3). V: on the retail pairs the mapped Halo 1
/// response of most materials is the Halo 2 default material's (frag and
/// plasma grenades 4, needle 3, rocket and bullets 0; Halo 1's water
/// overpenetration is retail's 2 on the liquids; `weapon_rebuild_disc`).
pub fn response(h1: u16) -> Option<u16> {
    match h1 {
        0 | 1 => Some(0),
        2 => Some(4),
        3 => Some(2),
        4 => Some(3),
        _ => None,
    }
}

/// One material response's values, in Halo 2's terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Response {
    pub effects: [Option<TagId>; 3],
    pub response: u16,
    pub potential_response: u16,
    pub potential_flags: u16,
    pub chance: f32,
    pub angle_bounds: [f32; 2],
    /// World units per second (Halo 1's per tick, times 30).
    pub velocity_bounds: [f32; 2],
    pub scale_effects_by: u16,
    pub angular_noise: f32,
    pub velocity_noise: f32,
    pub initial_friction: f32,
    pub maximum_distance: f32,
    pub parallel_friction: f32,
    pub perpendicular_friction: f32,
}

impl Response {
    /// From one Halo 1 element (`0xa0`): flags `+0` (zero on the disc),
    /// response `+2`, effect `+4`, potential response `+0x24` and its
    /// flags `+0x26`, skip fraction `+0x28` (the chance is one less it),
    /// angle bounds `+0x2c`,
    /// velocity bounds `+0x34` (per tick), potential effect `+0x3c`, scale
    /// effects by `+0x5c`, angular and velocity noise `+0x60`, `+0x64`,
    /// detonation effect `+0x68`, initial friction `+0x90`, maximum
    /// distance `+0x94`, parallel and perpendicular friction `+0x98`,
    /// `+0x9c`. A response Halo 2 has no counterpart for is an error.
    pub fn from_h1(e: &[u8]) -> Result<Self> {
        let h = |o: usize| u16::from_le_bytes([e[o], e[o + 1]]);
        let f = |o: usize| f32::from_bits(u32_at(e, o));
        let tick = crate::convert::TICKS_PER_SECOND;
        let map =
            |v: u16| response(v).ok_or_else(|| Error::Invalid(format!("material response {v}")));
        // A potential response is set when Halo 1's is not disappear (0)
        // or has flags; Halo 2 holds the chance it happens, Halo 1 the
        // fraction it is skipped. V: the plasma grenade's (attach, only
        // against units, skip 0) is retail's (3, 1, chance 1). Unset, it
        // is retail's unused one (0, 0, chance 0).
        let potential = h(0x24) != 0 || h(0x26) != 0;
        Ok(Self {
            effects: [None; 3],
            response: map(h(2))?,
            potential_response: if potential { map(h(0x24))? } else { 0 },
            potential_flags: if potential { h(0x26) } else { 0 },
            chance: if potential { 1.0 - f(0x28) } else { 0.0 },
            angle_bounds: [f(0x2c), f(0x30)],
            velocity_bounds: [f(0x34) * tick, f(0x38) * tick],
            scale_effects_by: h(0x5c),
            angular_noise: f(0x60),
            velocity_noise: f(0x64) * tick,
            initial_friction: f(0x90),
            maximum_distance: f(0x94),
            parallel_friction: f(0x98),
            perpendicular_friction: f(0x9c),
        })
    }
}

/// Halo 1's responses (`b` its `proj` bytes, `+0x240`) as Halo 2
/// elements: the response most Halo 1 materials share (the first such
/// on a tie) for [`DEFAULT_MATERIAL`], then one element per Halo 1
/// material whose response differs, for its [`MATERIALS`] name (the
/// first Halo 1 material wins where two share a name). Each as
/// (material name, its index in `materials`, values).
pub fn responses(
    h1: &h1map::H1Map,
    b: &[u8],
    materials: &[String],
    mut convert: impl FnMut(h1map::TagId) -> Result<TagId>,
) -> Result<Vec<(&'static str, i16, Response)>> {
    let blk =
        h1map::Block::read(b, H1_RESPONSES).ok_or(Error::Truncated("proj material responses"))?;
    let mut all = Vec::new();
    for row in h1
        .block(blk, H1_RESPONSE_SIZE)?
        .chunks_exact(H1_RESPONSE_SIZE)
    {
        let mut response = Response::from_h1(row)?;
        for (slot, at) in response.effects.iter_mut().zip([4, 0x3c, 0x68]) {
            let reference =
                h1map::TagRef::read(row, at).ok_or(Error::Truncated("response effect"))?;
            if !reference.id.is_none() {
                *slot = Some(convert(reference.id)?);
            }
        }
        all.push(response);
    }
    responses_from(&all, materials)
}

/// [`responses`] from Halo 1's responses already read.
pub fn responses_from(
    all: &[Response],
    materials: &[String],
) -> Result<Vec<(&'static str, i16, Response)>> {
    let index = |name: &str| -> Result<i16> {
        materials
            .iter()
            .position(|m| m == name)
            .map(|i| i as i16)
            .ok_or_else(|| Error::Invalid(format!("the base has no global material {name}")))
    };
    let Some(common) = all
        .iter()
        .max_by_key(|r| {
            let n = all.iter().filter(|x| x == r).count();
            // The first on a tie: max_by_key keeps the last maximum.
            (n, std::cmp::Reverse(all.iter().position(|x| x == *r)))
        })
        .copied()
    else {
        return Ok(Vec::new());
    };
    let mut out = vec![(DEFAULT_MATERIAL, index(DEFAULT_MATERIAL)?, common)];
    for (r, (_, name)) in all.iter().zip(MATERIALS.iter()) {
        if *r != common && !out.iter().any(|o| o.0 == *name) {
            out.push((name, index(name)?, *r));
        }
    }
    Ok(out)
}

/// Write each material response with its three authored effect references.
fn response_elements(d: &StructDef, list: &[(&'static str, i16, Response)]) -> Result<Vec<Record>> {
    let ed = d
        .child("material_responses")
        .ok_or_else(|| Error::Invalid("proj material_responses".into()))?;
    let mut out = Vec::new();
    for (name, index, r) in list {
        let mut e = defs::default_record(ed);
        let int = |v: u16| Val::Int(i64::from(v));
        let pair = |v: [f32; 2]| Val::floats(&v);
        for (field, id) in ["effect", "potential_effect", "detonation_effect"]
            .into_iter()
            .zip(r.effects)
        {
            if let Some(id) = id {
                set(&mut e, field, Val::tag(FourCC(*b"effe"), id))?;
            }
        }
        set(&mut e, "response", int(r.response))?;
        set(&mut e, "material_name", Val::text(name))?;
        set(&mut e, "material", Val::Int(i64::from(*index)))?;
        set(&mut e, "potential_response", int(r.potential_response))?;
        set(&mut e, "potential_flags", int(r.potential_flags))?;
        set(&mut e, "chance", Val::Float(r.chance))?;
        set(&mut e, "angle_bounds", pair(r.angle_bounds))?;
        set(&mut e, "velocity_bounds", pair(r.velocity_bounds))?;
        set(&mut e, "scale_effects_by", int(r.scale_effects_by))?;
        set(&mut e, "angular_noise", Val::Float(r.angular_noise))?;
        set(&mut e, "velocity_noise", Val::Float(r.velocity_noise))?;
        set(&mut e, "initial_friction", Val::Float(r.initial_friction))?;
        set(&mut e, "maximum_distance", Val::Float(r.maximum_distance))?;
        set(&mut e, "parallel_friction", Val::Float(r.parallel_friction))?;
        set(
            &mut e,
            "perpendicular_friction",
            Val::Float(r.perpendicular_friction),
        )?;
        out.push(e);
    }
    Ok(out)
}

/// What rebuilt equipment references.
#[derive(Clone, Debug, Default)]
pub struct EquipmentParts {
    pub hlmt: Option<TagId>,
    pub predicted: Vec<Predicted>,
    /// Halo 1's converted tags over those ([`Halo1Refs`]: material
    /// effects).
    pub halo1: Halo1Refs,
}

/// The `eqip` root from Halo 1's equipment. A health pack needs a positive
/// amount (docs/halo1/health.md): a zero time becomes 1.
pub fn equipment(e: &h1::Equipment, b: &[u8], parts: &EquipmentParts) -> Result<Block> {
    let d = def(b"eqip")?;
    let mut r = defs::default_record(d);
    refs(&mut r, &parts.halo1)?;
    item_words(&mut r, b)?;
    header(&mut r, "item.obje", EQUIPMENT_TYPE, &e.object, parts.hlmt)?;
    rebuild::predicted_resources(b"eqip", &mut r, "item.obje", &parts.predicted)?;
    set(&mut r, "powerup_type", Val::Int(e.powerup_type.into()))?;
    set(&mut r, "grenade_type", Val::Int(e.grenade_type.into()))?;
    let time = if e.powerup_type == crate::convert::POWERUP_HEALTH && e.powerup_time <= 0.0 {
        1.0
    } else {
        e.powerup_time
    };
    set(&mut r, "powerup_time", Val::Float(time))?;
    built(d, &r)
}

/// A pure impulse may have a radius without changing body or shield vitality.
pub(crate) fn has_vitality_damage(b: &[u8]) -> bool {
    [H1_DAMAGE_LOWER, H1_DAMAGE_UPPER, H1_DAMAGE_UPPER + 4]
        .into_iter()
        .any(|at| f32::from_bits(u32_at(b, at)) > 0.0)
}

#[cfg(test)]
#[test]
fn shock_wave_radius_does_not_count_as_vitality_damage() {
    let mut bytes = [0; H1_JPT_MIN];
    bytes[4..8].copy_from_slice(&8.0f32.to_le_bytes());
    assert!(!has_vitality_damage(&bytes));
    bytes[H1_DAMAGE_UPPER + 4..H1_DAMAGE_UPPER + 8].copy_from_slice(&1.0f32.to_le_bytes());
    assert!(has_vitality_damage(&bytes));
}

/// Halo 1 damage, camera impulse, shake and player feedback.
pub fn damage(b: &[u8], sound: Option<TagId>) -> Result<Block> {
    damage_with_scale(b, sound, None)
}

pub(crate) fn damage_with_scale(
    b: &[u8],
    sound: Option<TagId>,
    scale: Option<&str>,
) -> Result<Block> {
    let d = def(b"jpt!")?;
    let mut r = defs::default_record(d);
    set(&mut r, "ref_a8", Val::tag(FourCC(*b"snd!"), sound.unwrap_or(TagId::NULL)))?;
    if let Some(scale) = scale {
        set(&mut r, "string_50", Val::text(scale))?;
    }
    if b.len() < H1_JPT_MIN { return Err(Error::Truncated("jpt! root")); }
    for (path, at) in [
        ("unknown_0", 0), ("unknown_4", 4), ("unknown_8", 8),
        ("unknown_c", 0xc), ("unknown_18", 0x1cc), ("unknown_28", 0x1dc),
        ("unknown_30", 0x1e0), ("unknown_74", 0x98), ("unknown_78", 0x9c),
        ("unknown_7c", 0xa0), ("unknown_80", 0xa4), ("unknown_84", 0xb8),
        ("unknown_88", 0xbc), ("unknown_8c", 0xcc), ("unknown_90", 0xd0),
        ("unknown_94", 0xd4), ("unknown_98", 0xd8), ("unknown_9c", 0xe8),
        ("unknown_a4", 0xf0),
    ] {
        word(&mut r, path, u32_at(b, at))?;
    }
    let f = |at: usize| f32::from_bits(u32_at(b, at));
    word(&mut r, "unknown_a0", (f(0xec) / 30.0).to_bits())?;
    let category = u32_at(b, 0x1c4);
    set(
        &mut r,
        "h1_damage_category",
        Val::Int((map::H1_DAMAGE_CATEGORY | (category >> 16)).into()),
    )?;
    set(&mut r, "h1_death_impulse", Val::Float(f(0x1f4)))?;
    // H1 sniper rounds have a distinct category. H2 uses the bullet category.
    let category = if category >> 16 == 5 { (category & 0xffff) | (2 << 16) } else { category };
    word(&mut r, "unknown_10", category)?;
    word(&mut r, "unknown_14", u32_at(b, 0x1c8))?;
    if [0x34, 0x60, 0x74].iter().any(|at| f(*at) > 0.0) {
        let rd = d.child("block_6c").expect("damage response");
        let mut response = defs::default_record(rd);
        for (path, at) in [
            ("unknown_4", 0x24), ("unknown_8", 0x34), ("unknown_c", 0x38),
            ("unknown_10", 0x44), ("unknown_14", 0x48), ("unknown_18", 0x4c),
            ("unknown_1c", 0x50), ("unknown_20", 0x54),
            ("unknown_24", 0x60), ("unknown_30", 0x74),
        ] {
            word(&mut response, path, u32_at(b, at))?;
        }
        for (path, magnitude, shape) in [("data_28", 0x5c, 0x64), ("data_34", 0x70, 0x78)] {
            use tags::effects::function::{Function, FunctionType};
            let envelope = Function {
                kind: FunctionType::Transition, flags: 0,
                sub_types: [b[shape], 0], range: [0.0; 2], unknown_0c: [0; 8],
                payload: [f(magnitude), 0.0, 0.0, 1.0].into_iter()
                    .flat_map(f32::to_le_bytes).collect(),
            };
            set(&mut response, path, Val::Data(envelope.to_bytes()))?;
        }
        // H1 authors one response. H2 selects by shield, body or local firing
        // feedback. Both projectile and firing slots may name the same tag.
        let mut responses = Vec::new();
        for selector in 0..3 {
            let mut entry = response.clone();
            word(&mut entry, "unknown_0", selector)?;
            responses.push(entry);
        }
        set(&mut r, "block_6c", Val::Block(responses))?;
    }
    set(&mut r, "damage_lower_bound", Val::Float(f(H1_DAMAGE_LOWER)))?;
    set(
        &mut r,
        "damage_upper_bound",
        Val::floats(&[f(H1_DAMAGE_UPPER), f(H1_DAMAGE_UPPER + 4)]),
    )?;
    built(d, &r)
}

/// Minimum Halo 1 `jpt!` bytes for the scalar damage and feedback fields.
/// The last of them this converter reads is the damage impulse at `+0x1f4`
/// (Halo 1 dialogue selection at `0x4721b`). The source material
/// multipliers follow at `+0x200`.
pub const H1_JPT_MIN: usize = 0x1f8;
