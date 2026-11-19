//! Weapons, equipment, projectiles and damage effects written from their
//! typed definitions ([`tags::defs`]: `weap`, `eqip`, `proj`, `jpt!`), with
//! no retail template bytes. Every field starts at its definition default;
//! then, in order:
//!
//! - the group's **role words** ([`Role`]): every 4-byte unknown span where
//!   at least half of the base map's tags of the group hold one nonzero
//!   value takes it (the turret and biped rule, `crate::rebuild`), and the
//!   same over every element of the blocks the conversion writes, pooled.
//!   Computed from the base at conversion time, so no retail value is kept
//!   in the source;
//! - Halo 1's values: the named fields, the material responses, and the
//!   words the removed template path wrote by offset ([`WEAPON_WORDS`], [`BARREL_WORDS`],
//!   [`PROJECTILE_WORDS`]; offsets from `docs/halo1/objects.md`);
//! - references: converted Halo 1 tags (model chain, first-person model
//!   and animations, projectiles, damage effects), and, for slots Halo 1
//!   has no converted tag for (HUD, effects, sounds, melee damage), the
//!   Halo 2 counterpart's reference ([`Halo2Refs`]). Only references are
//!   taken from the counterpart, never its bytes.
//!
//! Evidence and the field-by-field proof: `docs/formats/tag-definitions.md`
//! (Weapons, equipment, projectiles and damage) and
//! `tests/weapon_rebuild_disc.rs`.

use map::build::Block;
use map::{FourCC, TagId};
use tags::defs::{self, Defs, Kind, Record, StructDef, Val};

use crate::base::Base;
use crate::h1;
use crate::rebuild::{self, Predicted, Tally};
use crate::{Error, Result};

/// `obje +0` for each group (docs/formats/objects.md).
pub const WEAPON_TYPE: u32 = 2;
pub const EQUIPMENT_TYPE: u32 = 3;
pub const PROJECTILE_TYPE: u32 = 5;

/// The blocks whose elements take pooled role words, per group.
pub const WEAPON_BLOCKS: &[&str] = &["magazines", "barrels", "barrels.block_e4"];

/// `weap` root words written from Halo 1 by offset, as the removed template path
/// did: (path, Halo 1 `weap` offset). The aim assist is E (`0x1a50a0`
/// reads `+0x208..+0x218`); the heat words are S, V2
/// (docs/halo1/objects.md).
pub const WEAPON_WORDS: &[(&str, usize)] = &[
    ("unknown_150", 0x34c), // heat recovery threshold
    ("unknown_154", 0x350), // overheated threshold
    ("unknown_158", 0x354), // heat detonation threshold
    ("unknown_15c", 0x358), // heat detonation fraction
    ("unknown_160", 0x35c), // heat loss per second
    ("unknown_164", 0x360), // heat illumination
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
    ("unknown_28", 0x20), // i16 magazine, i16 rounds per shot
    ("unknown_2c", 0x24), // i16 minimum rounds loaded, i16 rounds between tracers
    ("unknown_38", 0x38), // error acceleration time
    ("unknown_3c", 0x3c), // error deceleration time
    ("unknown_68", 0x6c), // i16 distribution function, i16 projectiles per shot
    ("unknown_6c", 0x70), // distribution angle
    ("unknown_70", 0x78), // minimum error
    ("unknown_74", 0x7c), // error angle, from
    ("unknown_78", 0x80), // error angle, to
    ("unknown_7c", 0x84), // first-person offset
    ("unknown_80", 0x88),
    ("unknown_84", 0x8c),
    ("unknown_a4", 0xb8), // heat per round
];

/// `proj` words from Halo 1 by offset (S, V2; the removed template path's rows):
/// (path, Halo 1 `proj` offset, per tick to per second). Halo 1 keeps
/// speeds per tick ([`crate::convert::TICKS_PER_SECOND`]).
pub const PROJECTILE_WORDS: &[(&str, usize, bool)] = &[
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
    ("unknown_184", 0x1ec, true), // guided angular velocity
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

fn words(r: &mut Record, ws: &[(String, u32)]) -> Result<()> {
    for (p, w) in ws {
        word(r, p, *w)?;
    }
    Ok(())
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

/// The offset of the field `name` in `d`.
fn offset(d: &StructDef, name: &str) -> Option<u32> {
    d.fields.iter().find(|f| f.name == name).map(|f| f.offset)
}

/// A group's role words over the base map ([`role`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Role {
    /// How many of the base's tags of the group it rests on.
    pub members: usize,
    /// Root words, as (path, word).
    pub root: Vec<(String, u32)>,
    /// Per block path, the pooled element words.
    pub elements: Vec<(String, Vec<(String, u32)>)>,
}

impl Role {
    /// The element words of the block at `path` (none when no member has
    /// one).
    pub fn element(&self, path: &str) -> &[(String, u32)] {
        self.elements
            .iter()
            .find(|e| e.0 == path)
            .map_or(&[], |e| &e.1[..])
    }
}

/// Every element of the block at `path` (dotted: a block inside a block's
/// elements) under `bytes`, an element of `d`.
fn elements_at(base: &Base, d: &StructDef, bytes: &[u8], path: &str) -> Result<Vec<Vec<u8>>> {
    let (head, rest) = path.split_once('.').unwrap_or((path, ""));
    let (Some(at), Some(e)) = (offset(d, head), d.child(head)) else {
        return Err(Error::Invalid(format!("{}: no block {head}", d.name)));
    };
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(x) = base.element(bytes, at as usize, e.size as usize, i)? {
        if rest.is_empty() {
            out.push(x);
        } else {
            out.extend(elements_at(base, e, &x, rest)?);
        }
        i += 1;
    }
    Ok(out)
}

/// The base's tags of `group` and their roots.
fn members(base: &Base, group: &[u8; 4]) -> Result<Vec<(TagId, String, Vec<u8>)>> {
    let d = def(group)?;
    base.tags
        .iter()
        .filter(|t| t.1 == FourCC(*group))
        .map(|t| Ok((t.0, t.2.clone(), base.head(t.0, d.size.into())?)))
        .collect()
}

/// [`Role`] over every tag of `group` the base holds, with `blocks`'
/// elements pooled. `None` when the base has none.
pub fn role(base: &Base, group: &[u8; 4], blocks: &[&str]) -> Result<Option<Role>> {
    let d = def(group)?;
    let all = members(base, group)?;
    if all.is_empty() {
        return Ok(None);
    }
    let mut spans = Vec::new();
    rebuild::word_spans(d, "", 0, &mut spans);
    let mut root = Tally::new();
    for (_, _, bytes) in &all {
        rebuild::tally_words(bytes, &spans, &mut root);
    }
    let mut elements = Vec::new();
    for path in blocks {
        let e = d
            .child(path)
            .ok_or_else(|| Error::Invalid(format!("{} {path}", d.name)))?;
        let mut espans = Vec::new();
        rebuild::word_spans(e, "", 0, &mut espans);
        let mut t = Tally::new();
        for (_, _, bytes) in &all {
            for x in elements_at(base, d, bytes, path)? {
                rebuild::tally_words(&x, &espans, &mut t);
            }
        }
        elements.push(((*path).to_owned(), rebuild::majority(&t)));
    }
    Ok(Some(Role {
        members: all.len(),
        root: rebuild::majority(&root),
        elements,
    }))
}

/// References taken from a Halo 2 counterpart: (path in the record,
/// group, tag). Only live references of the definition's own tagref
/// fields, through embedded structs.
pub type Halo2Refs = Vec<(String, FourCC, TagId)>;

/// The counterpart's references in `bytes` (an element of `d`), but for
/// the paths in `skip`.
pub fn halo2_refs(base: &Base, d: &StructDef, bytes: &[u8], skip: &[&str]) -> Halo2Refs {
    fn walk(base: &Base, d: &StructDef, bytes: &[u8], at: u32, prefix: &str, out: &mut Halo2Refs) {
        for f in &d.fields {
            let p = format!("{prefix}{}", f.name);
            let o = (at + f.offset) as usize;
            match &f.kind {
                Kind::Struct(s) => walk(base, s, bytes, at + f.offset, &format!("{p}."), out),
                Kind::TagRef { .. } if o + 8 <= bytes.len() => {
                    let id = TagId(u32_at(bytes, o + 4));
                    if let Some((g, _)) = base.name(id)
                        && u32_at(bytes, o) == u32::from_be_bytes(g.0)
                    {
                        out.push((p, g, id));
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(base, d, bytes, 0, "", &mut out);
    out.retain(|r| !skip.contains(&r.0.as_str()));
    out
}

fn refs(r: &mut Record, list: &Halo2Refs) -> Result<()> {
    for (p, g, id) in list {
        set(r, p, Val::tag(*g, *id))?;
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
    /// Halo 1's first-person model and animations, converted.
    pub first_person: (Option<TagId>, Option<TagId>),
    /// From the Halo 2 counterpart: root references (HUD, melee damage,
    /// effects, sounds) and per barrel its references.
    pub halo2: Halo2Refs,
    pub barrel_halo2: Vec<Halo2Refs>,
    /// Per Halo 1 trigger, its firing effects, converted ([`FiringParts`]).
    pub firing_effects: Vec<Vec<FiringParts>>,
    /// Per Halo 1 magazine, its reloading effect, converted.
    pub magazine_effects: Vec<Option<TagId>>,
    /// The weapon class and type a unit's graph holds it by
    /// ([`WEAPON_CLASS`], [`WEAPON_TYPE_NAME`]); `None` leaves both null.
    pub animation: Option<(String, String)>,
}

/// One firing effect of a barrel (`barrels.block_e4`, `0x34`) from Halo
/// 1's trigger firing effect (`h1::FiringEffect`): its converted firing,
/// misfire and empty effects, and the damage (rumble and shake) the Halo 2
/// counterpart's barrel names. A shot plays the effect: until converter
/// version 84 the effects were the counterpart's too, so the assault
/// rifle fired with the SMG's sound and a weapon with no counterpart (the
/// vehicle guns) had no firing effect and fired silent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FiringParts {
    pub effects: [Option<TagId>; 3],
    pub damage: [Option<TagId>; 3],
}

/// `barrels.block_e4` references, in Halo 1's order: firing, misfire and
/// empty effect (`effe`; retail Halo 2 also names a `snd!` there), then
/// firing, misfire and empty damage (`jpt!`). V: on every retail weapon
/// the first three hold `effe` or `snd!` and the last three `jpt!`
/// (`weapon_sounds_disc`); names S, by Halo 1's order.
pub const FIRING_EFFECT_REFS: [&str; 3] = ["ref_4", "ref_c", "ref_14"];
pub const FIRING_DAMAGE_REFS: [&str; 3] = ["ref_1c", "ref_24", "ref_2c"];
/// [`FIRING_DAMAGE_REFS`]' offsets in the element.
pub const FIRING_DAMAGE_AT: [usize; 3] = [0x1c, 0x24, 0x2c];

/// Magazine `+0x34`: the reloading effect (`effe`). V: retail's SMG
/// names an effect here; Halo 1's magazine `+0x38`. The chambering
/// effect (`+0x44` by the group words' pattern: `+0x3c` and `+0x4c` hold
/// `jpt!` group words) has no definition field yet and is not written.
pub const MAGAZINE_RELOADING_EFFECT: &str = "ref_34";

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

/// Halo 1 trigger flag `0x8`: set on exactly the Halo 1 weapons that fire
/// once per press on Blood Gulch (rocket launcher, sniper rifle, the
/// scorpion's cannon) and on none of the automatic ones. S for the name;
/// V for that split.
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
/// illumination is 1.0 in both games. The charging effect is not carried
/// (no Halo 1 trigger on the disc's multiplayer maps names one).
pub const H1_TRIGGER_CHARGED: usize = 0x4c;

/// The Halo 2 triggers for Halo 1's (`h1`, each `h1::TRIGGER_SIZE`
/// bytes), whose barrels are one per Halo 1 trigger in order. Halo 1
/// fires each trigger from its own input; Halo 2 needs a trigger naming
/// the barrel:
///
/// - a first trigger that charges (`+0x48` > 0) with a second one is one
///   charging trigger, barrel 0 tapped and barrel 1 charged (Halo 1's
///   plasma pistol: trigger 1 is its overcharge, as Halo 2's plasma
///   pistol's secondary barrel);
/// - otherwise each of the first two Halo 1 triggers is a trigger on input
///   0 and 1 firing its barrel, once per press when it has
///   [`H1_TRIGGER_ONE_PER_PRESS`], else automatically.
pub fn h2_triggers(h1: &[Vec<u8>]) -> Vec<TriggerWords> {
    let flags = |t: &Vec<u8>| u32_at(t, 0);
    let charging = |t: &Vec<u8>| f32::from_bits(u32_at(t, H1_TRIGGER_CHARGING_TIME));
    match h1 {
        [] => Vec::new(),
        [first, _, ..] if charging(first) > 0.0 => {
            let mut t = TriggerWords::new(0, 2, 0, 1);
            t.0[7] = charging(first).to_bits();
            // Charged time, overcharged action (its i16 alone), charged
            // illumination, spew time.
            for k in 0..4 {
                t.0[8 + k] = u32_at(first, H1_TRIGGER_CHARGED + 4 * k);
            }
            t.0[9] &= 0xffff;
            vec![t]
        }
        _ => h1
            .iter()
            .take(2)
            .enumerate()
            .map(|(i, t)| {
                let behaviour = i16::from(flags(t) & H1_TRIGGER_ONE_PER_PRESS != 0);
                TriggerWords::new(i as i16, behaviour, i as i16, -1)
            })
            .collect(),
    }
}

/// The `weap` root from Halo 1's weapon `w` (the bytes of its tag: `b`).
pub fn weapon(w: &h1::Weapon, b: &[u8], parts: &WeaponParts, role: &Role) -> Result<Block> {
    let d = def(b"weap")?;
    let mut r = defs::default_record(d);
    words(&mut r, &role.root)?;
    refs(&mut r, &parts.halo2)?;
    header(&mut r, "item.obje", WEAPON_TYPE, &w.object, parts.hlmt)?;
    if let Some((class, kind)) = &parts.animation {
        set(&mut r, WEAPON_CLASS, Val::text(class))?;
        set(&mut r, WEAPON_TYPE_NAME, Val::text(kind))?;
    }
    rebuild::predicted_resources(b"weap", &mut r, "item.obje", &parts.predicted)?;
    for (path, at) in WEAPON_WORDS {
        word(&mut r, path, u32_at(b, *at))?;
    }
    // Magazines.
    let md = d.child("magazines").expect("weap magazines");
    let mut mags = Vec::new();
    for m in &w.magazines {
        let mut e = defs::default_record(md);
        words(&mut e, role.element("magazines"))?;
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
        if let Some(reloading) = parts.magazine_effects.get(mags.len()) {
            set(
                &mut e,
                MAGAZINE_RELOADING_EFFECT,
                Val::tag(FourCC(*b"effe"), reloading.unwrap_or(TagId::NULL)),
            )?;
        }
        mags.push(e);
    }
    set(&mut r, "magazines", Val::Block(mags))?;
    // Barrels: one per Halo 1 trigger.
    let bd = d.child("barrels").expect("weap barrels");
    let fd = d.child("barrels.block_e4").expect("barrel firing effects");
    let trig = h1_triggers(b)?;
    let mut barrels = Vec::new();
    for (i, t) in w.triggers.iter().enumerate() {
        let mut e = defs::default_record(bd);
        words(&mut e, role.element("barrels"))?;
        if let Some(h) = parts.barrel_halo2.get(i).or(parts.barrel_halo2.last()) {
            refs(&mut e, h)?;
        }
        for (path, at) in BARREL_WORDS {
            word(&mut e, path, u32_at(&trig[i], *at))?;
        }
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
            words(&mut x, role.element("barrels.block_e4"))?;
            for (path, id) in FIRING_EFFECT_REFS.iter().zip(f.effects) {
                set(&mut x, path, Val::tag(FourCC(*b"effe"), id.unwrap_or(TagId::NULL)))?;
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
    for t in h2_triggers(&trig) {
        let mut e = defs::default_record(td);
        for (k, w) in t.0.iter().enumerate() {
            if *w != 0 {
                word(&mut e, &format!("unknown_{:x}", 4 * k), *w)?;
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
    Ok(b[root..]
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
    /// The Halo 2 counterpart's references (effects, sounds) for the
    /// slots Halo 1 has nothing converted for.
    pub halo2: Halo2Refs,
    /// The material responses ([`responses`]).
    pub responses: Vec<(&'static str, i16, Response)>,
}

/// The `proj` root from Halo 1's projectile (its tag's bytes `b`).
pub fn projectile(
    p: &h1::Projectile,
    b: &[u8],
    parts: &ProjectileParts,
    role: &Role,
) -> Result<Block> {
    let d = def(b"proj")?;
    let mut r = defs::default_record(d);
    words(&mut r, &role.root)?;
    refs(&mut r, &parts.halo2)?;
    header(&mut r, "object", PROJECTILE_TYPE, &p.object, parts.hlmt)?;
    for (path, at, per_tick) in PROJECTILE_WORDS {
        let w = if *per_tick {
            (f32::from_bits(u32_at(b, *at)) * crate::convert::TICKS_PER_SECOND).to_bits()
        } else {
            u32_at(b, *at)
        };
        word(&mut r, path, w)?;
    }
    // Halo 1's detonation noise (`+0x1f0`, i16) in the low half of `+0xe4`.
    let hi = match r.at("unknown_e4") {
        Some(Val::Bytes(x)) if x.len() == 4 => u32_at(x, 0) & 0xffff_0000,
        _ => 0,
    };
    word(
        &mut r,
        "unknown_e4",
        hi | u32::from(p.detonation_noise as u16),
    )?;
    set(
        &mut r,
        "material_responses",
        Val::Block(response_elements(d, &parts.responses)?),
    )?;
    set(&mut r, "collision_radius", Val::Float(p.collision_radius))?;
    set(&mut r, "maximum_range", Val::Float(p.maximum_range))?;
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

/// Halo 1's material types in the order of its responses, each with the
/// Halo 2 global material its response is written for: the Halo 2
/// material of the same substance and, where Halo 2 has one, the same
/// character. Every name is one of the base's global materials (V:
/// `weapon_rebuild_disc` on every base).
pub const MATERIALS: [(&str, &str); 33] = [
    ("dirt", "tough_terrain_dirt"),
    ("sand", "tough_terrain_sand"),
    ("stone", "hard_terrain_stone"),
    ("snow", "soft_terrain_snow"),
    ("wood", "tough_organic_wood"),
    ("metal (hollow)", "hard_metal_thin"),
    ("metal (thin)", "hard_metal_thin"),
    ("metal (thick)", "hard_metal_thick"),
    ("rubber", "tough_inorganic_rubber"),
    ("glass", "brittle_glass"),
    ("force field", "energy_shield_thick_for"),
    ("grunt", "soft_organic_flesh_grunt"),
    ("hunter armor", "hard_metal_thick_cov_hunter"),
    ("hunter skin", "soft_organic_flesh_hunter"),
    ("elite", "soft_organic_flesh_elite"),
    ("jackal", "soft_organic_flesh_jackal"),
    ("jackal energy shield", "energy_shield_thick_cov_jackal"),
    ("engineer skin", "soft_organic_flesh"),
    ("engineer force field", "energy_shield_thin"),
    ("flood combat form", "tough_floodflesh_combatform"),
    ("flood carrier form", "tough_floodflesh_carrierform"),
    ("cyborg armor", "hard_metal_thin_hum_masterchief"),
    ("cyborg energy shield", "energy_shield_thin_hum_masterchief"),
    ("human armor", "tough_inorganic_armor_hum"),
    ("human skin", "soft_organic_flesh_human"),
    ("sentinel", "hard_metal_thin_for_sentinel"),
    ("monitor", "hard_metal_thin_for_monitor"),
    ("plastic", "tough_inorganic_plastic"),
    ("water", "liquid_thin_water"),
    ("leaves", "soft_organic_plant_leafy"),
    ("elite energy shield", "energy_shield_thin_cov_elite"),
    ("ice", "hard_terrain_ice"),
    ("hunter shield", "hard_metal_solid_cov_hunter"),
];

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
) -> Result<Vec<(&'static str, i16, Response)>> {
    let blk = h1map::Block::read(b, H1_RESPONSES)
        .ok_or_else(|| Error::Invalid("proj material responses".into()))?;
    let all: Vec<Response> = h1
        .block(blk, H1_RESPONSE_SIZE)?
        .chunks_exact(H1_RESPONSE_SIZE)
        .map(Response::from_h1)
        .collect::<Result<_>>()?;
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

/// [`responses`] as `proj +0x19c` elements; flags 0 and the effects
/// null (Halo 1's effects are not converted).
fn response_elements(d: &StructDef, list: &[(&'static str, i16, Response)]) -> Result<Vec<Record>> {
    let ed = d
        .child("material_responses")
        .ok_or_else(|| Error::Invalid("proj material_responses".into()))?;
    let mut out = Vec::new();
    for (name, index, r) in list {
        let mut e = defs::default_record(ed);
        let int = |v: u16| Val::Int(i64::from(v));
        let pair = |v: [f32; 2]| Val::floats(&v);
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
    pub halo2: Halo2Refs,
}

/// The `eqip` root from Halo 1's equipment. A health pack needs a positive
/// amount (docs/halo1/health.md): a zero time becomes 1.
pub fn equipment(e: &h1::Equipment, parts: &EquipmentParts, role: &Role) -> Result<Block> {
    let d = def(b"eqip")?;
    let mut r = defs::default_record(d);
    words(&mut r, &role.root)?;
    refs(&mut r, &parts.halo2)?;
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

/// The `jpt!` root from Halo 1's damage effect bytes `b`: the role's
/// words and Halo 1's damage bounds.
pub fn damage(b: &[u8], role: &Role) -> Result<Block> {
    let d = def(b"jpt!")?;
    let mut r = defs::default_record(d);
    words(&mut r, &role.root)?;
    let f = |at: usize| f32::from_bits(u32_at(b, at));
    set(&mut r, "damage_lower_bound", Val::Float(f(H1_DAMAGE_LOWER)))?;
    set(
        &mut r,
        "damage_upper_bound",
        Val::floats(&[f(H1_DAMAGE_UPPER), f(H1_DAMAGE_UPPER + 4)]),
    )?;
    built(d, &r)
}

/// Halo 1's `jpt!` root size: where its first child block starts (V1,
/// docs/halo1/objects.md); the damage bounds end at `+0x1dc`.
pub const H1_JPT_MIN: usize = 0x1dc;
