//! Halo 1 Xbox object tags, as stored in a cache file.
//!
//! Each reader takes the tag's bytes (its root and, after it, its own
//! blocks) through [`h1map::H1Map`] and keeps the fields the conversion
//! carries, plus every reference. Offsets and evidence are in
//! `docs/halo1/objects.md`.

use h1map::{Block, Group, H1Map, Tag, TagId, TagRef};

use crate::{Error, Result};

/// The shared `obje` header is `0x17c` bytes.
pub const OBJE_SIZE: usize = 0x17c;
/// Root sizes per group (V: the first child block starts here).
pub const WEAP_SIZE: usize = 0x508;
pub const EQIP_SIZE: usize = 0x3b0;
pub const PROJ_SIZE: usize = 0x24c;
pub const COLL_SIZE: usize = 0x298;
pub const PHYS_SIZE: usize = 0x80;
pub const MAGAZINE_SIZE: usize = 0x70;
pub const TRIGGER_SIZE: usize = 0x114;

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn i16_at(b: &[u8], at: usize) -> i16 {
    u16_at(b, at) as i16
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_bits(u32_at(b, at))
}
fn vec3(b: &[u8], at: usize) -> [f32; 3] {
    std::array::from_fn(|i| f32_at(b, at + 4 * i))
}
fn bounds(b: &[u8], at: usize) -> [f32; 2] {
    [f32_at(b, at), f32_at(b, at + 4)]
}
pub(crate) fn name32(b: &[u8], at: usize) -> String {
    let s = &b[at..at + 32];
    let end = s.iter().position(|&c| c == 0).unwrap_or(32);
    String::from_utf8_lossy(&s[..end]).into_owned()
}

/// A reference field: `None` when null.
pub fn reference(map: &H1Map, b: &[u8], at: usize) -> Option<TagId> {
    let r = TagRef::read(b, at)?;
    if r.id.is_none() {
        return None;
    }
    map.tag(r.id).ok().map(|t| t.id)
}

pub fn elements<'a>(map: &'a H1Map, b: &[u8], at: usize, stride: usize) -> Result<Vec<&'a [u8]>> {
    let blk = Block::read(b, at).ok_or(Error::Truncated("block field"))?;
    Ok(map.block(blk, stride)?.chunks_exact(stride).collect())
}

/// The tag's bytes, checked to be of `group` and at least `size` long.
pub fn tag_bytes<'a>(map: &'a H1Map, tag: &Tag, group: &[u8; 4], size: usize) -> Result<&'a [u8]> {
    if tag.group != Group::new(group) {
        return Err(Error::WrongGroup(tag.group.to_string(), *group));
    }
    let b = map.tag_data(tag)?;
    if b.len() < size {
        return Err(Error::Truncated("tag root"));
    }
    Ok(b)
}

/// Halo 1's object types (`obje +0`).
pub const OBJECT_TYPES: [&[u8; 4]; 12] = [
    b"bipd", b"vehi", b"weap", b"eqip", b"garb", b"proj", b"scen", b"mach", b"ctrl", b"lifi",
    b"plac", b"ssce",
];

/// The shared `obje` header.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Object {
    /// `0x00`: index into [`OBJECT_TYPES`].
    pub object_type: u16,
    pub flags: u16,
    pub bounding_radius: f32,
    pub acceleration_scale: f32,
    pub bounding_offset: [f32; 3],
    /// `0x28` render model (`mode`), `0x38` animation graph (`antr`),
    /// `0x70` collision model (`coll`), `0x80` physics (`phys`).
    pub model: Option<TagId>,
    pub animation: Option<TagId>,
    pub collision: Option<TagId>,
    pub physics: Option<TagId>,
    /// `0x90` modifier shader, `0xa0` creation effect.
    pub modifier_shader: Option<TagId>,
    pub creation_effect: Option<TagId>,
    /// `0x140` attachments (`0x48`), whose `+0` is a reference.
    pub attachments: Vec<Option<TagId>>,
    /// `0x14c` widgets (`0x20`), `+0` a reference.
    pub widgets: Vec<Option<TagId>>,
    /// `0x164` change colours (`0x2c`), A to D in Halo 1's order as read.
    /// The converter puts them in Halo 2's slots
    /// ([`ChangeColour::halo2_slots`]) before the object is rebuilt.
    pub change_colours: Vec<ChangeColour>,
}

/// One of an object's change colours (`obje +0x164`, `0x2c` each): `+0`
/// darken by and `+2` scale by (i16 function sources), `+4` flags, `+8`
/// and `+0x14` the lower and upper bound (RGB) a function scales
/// between, `+0x20` its permutations (`0x1c` each). Only the
/// permutations are kept: they are the colour an object starts with.
/// V (docs/halo1/objects.md "Change colours"): the boulders' and trees'
/// one permutation or three, the campaign cyborg's C (the Chief's green,
/// lower and upper bound equal); the multiplayer cyborg's A and B have a
/// function and no permutation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangeColour {
    pub permutations: Vec<ColourPermutation>,
}

/// A change colour's permutation (`0x1c`): `+0` weight, `+4` lower and
/// `+0x10` upper bound (RGB, 0..1); the object picks one by weight and a
/// colour between the bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColourPermutation {
    pub weight: f32,
    pub lower: [f32; 3],
    pub upper: [f32; 3],
}

impl ChangeColour {
    /// Stride of `obje +0x164`.
    pub const SIZE: usize = 0x2c;
    /// Stride of its permutations (`+0x20`).
    pub const PERMUTATION_SIZE: usize = 0x1c;

    fn read(map: &H1Map, e: &[u8]) -> Result<Self> {
        let permutations = elements(map, e, 0x20, Self::PERMUTATION_SIZE)?
            .into_iter()
            .map(|p| ColourPermutation {
                weight: f32_at(p, 0),
                lower: vec3(p, 4),
                upper: vec3(p, 0x10),
            })
            .collect();
        Ok(Self { permutations })
    }

    /// `h1` (A to D, as read) in Halo 2's four slots: slot `k` holds the
    /// Halo 1 colour whose source
    /// `h1convert::shad::halo2_change_colour_slot` sends to `k`. A and C
    /// both go to slot 0; `reads_c` (the object's model shaders read C,
    /// the player's colour in Halo 1) says which of them the object's
    /// slot 0 starts as. A slot with nothing (or the one of A and C not
    /// read) is empty: white, no tint. Always four, so a player's unit
    /// takes all four of the player's colours
    /// (`h1convert::shad::halo2_change_colour_slot`).
    pub fn halo2_slots(h1: &[ChangeColour], reads_c: bool) -> Vec<ChangeColour> {
        let mut out = vec![ChangeColour::default(); 4];
        for (k, c) in h1.iter().enumerate().take(4) {
            let source = k as u16 + 1;
            if (source == 1 && reads_c) || (source == 3 && !reads_c) {
                continue;
            }
            if let Some(slot) = h1convert::shad::halo2_change_colour_slot(source) {
                out[slot] = c.clone();
            }
        }
        out
    }
}

impl Object {
    pub fn read(map: &H1Map, b: &[u8]) -> Result<Self> {
        if b.len() < OBJE_SIZE {
            return Err(Error::Truncated("obje"));
        }
        let refs = |at, stride| -> Result<Vec<Option<TagId>>> {
            Ok(elements(map, b, at, stride)?
                .into_iter()
                .map(|e| reference(map, e, 0))
                .collect())
        };
        Ok(Self {
            object_type: u16_at(b, 0),
            flags: u16_at(b, 2),
            bounding_radius: f32_at(b, 4),
            acceleration_scale: f32_at(b, 0x20),
            bounding_offset: vec3(b, 8),
            model: reference(map, b, 0x28),
            animation: reference(map, b, 0x38),
            collision: reference(map, b, 0x70),
            physics: reference(map, b, 0x80),
            modifier_shader: reference(map, b, 0x90),
            creation_effect: reference(map, b, 0xa0),
            attachments: refs(0x140, 0x48)?,
            widgets: refs(0x14c, 0x20)?,
            change_colours: elements(map, b, 0x164, ChangeColour::SIZE)?
                .into_iter()
                .map(|e| ChangeColour::read(map, e))
                .collect::<Result<_>>()?,
        })
    }
}

/// A weapon's magazine (`0x70`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Magazine {
    pub flags: u32,
    /// `+4` rounds recharged per second, `+6` total initial, `+8` total
    /// maximum, `+0xa` loaded maximum.
    pub rounds_recharged: i16,
    pub rounds_total_initial: i16,
    pub rounds_total_maximum: i16,
    pub rounds_loaded_maximum: i16,
    /// `+0x14` seconds.
    pub reload_time: f32,
    /// `+0x18`.
    pub rounds_reloaded: i16,
    /// `+0x1c` seconds.
    pub chamber_time: f32,
    /// `+0x38` reloading and `+0x48` chambering effects.
    pub reloading_effect: Option<TagId>,
    pub chambering_effect: Option<TagId>,
    /// `+0x64` magazine objects (`0x1c`): i16 rounds, `+0xc` the `eqip`.
    pub objects: Vec<(i16, Option<TagId>)>,
}

/// A weapon's trigger (`0x114`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trigger {
    pub flags: u32,
    /// `+4` rounds per second, from and to.
    pub rounds_per_second: [f32; 2],
    /// `+0xc`, `+0x10` seconds.
    pub firing_acceleration_time: f32,
    pub firing_deceleration_time: f32,
    /// `+0x20` magazine index, `+0x22` rounds per shot, `+0x24` minimum
    /// rounds loaded, `+0x26` rounds between tracers.
    pub magazine: i16,
    pub rounds_per_shot: i16,
    pub minimum_rounds_loaded: i16,
    pub rounds_between_tracers: i16,
    /// `+0x2e`.
    pub firing_noise: i16,
    /// `+0x38`, `+0x3c` seconds.
    pub error_acceleration_time: f32,
    pub error_deceleration_time: f32,
    /// `+0x50` charging effect.
    pub charging_effect: Option<TagId>,
    /// `+0x6c` distribution function, `+0x6e` projectiles per shot, `+0x70`
    /// distribution angle (radians).
    pub distribution_function: i16,
    pub projectiles_per_shot: i16,
    pub distribution_angle: f32,
    /// `+0x78` minimum error, `+0x7c` error angle from and to (radians).
    pub minimum_error: f32,
    pub error_angle: [f32; 2],
    /// `+0x84` first person offset.
    pub first_person_offset: [f32; 3],
    /// `+0x94`.
    pub projectile: Option<TagId>,
    /// `+0xb8` heat generated per round.
    pub heat_per_round: f32,
    /// `+0x108` firing effects (`0x84`, [`FiringEffect`]).
    pub firing_effects: Vec<FiringEffect>,
}

/// A trigger's firing effect element (`FIRING_EFFECT_SIZE`): what a shot
/// plays. V (`weapon_sounds_disc`, every map): the only references in an
/// element are `effe` at [`FIRING_EFFECT_EFFECTS`] and `jpt!` at
/// [`FIRING_EFFECT_DAMAGE`]; names S.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FiringEffect {
    /// `+0`, `+2` (i16): the shot counts the element covers, lower and upper.
    pub shot_count: [i16; 2],
    /// Firing, misfire and empty effects (`effe`).
    pub effects: [Option<TagId>; 3],
    /// Firing, misfire and empty damage (`jpt!`: the shot's rumble and
    /// camera shake).
    pub damage: [Option<TagId>; 3],
}

pub const FIRING_EFFECT_SIZE: usize = 0x84;
/// The firing, misfire and empty effect references of a firing effect.
pub const FIRING_EFFECT_EFFECTS: [usize; 3] = [0x24, 0x34, 0x44];
/// The firing, misfire and empty damage references of a firing effect.
pub const FIRING_EFFECT_DAMAGE: [usize; 3] = [0x54, 0x64, 0x74];

impl FiringEffect {
    pub fn read(map: &H1Map, e: &[u8]) -> Self {
        Self {
            shot_count: [i16_at(e, 0), i16_at(e, 2)],
            effects: FIRING_EFFECT_EFFECTS.map(|o| reference(map, e, o)),
            damage: FIRING_EFFECT_DAMAGE.map(|o| reference(map, e, o)),
        }
    }
}

/// `weap`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Weapon {
    pub object: Object,
    pub flags: u32,
    /// `0x30c` (`char[32]`): the weapon's animation label, the weapon type
    /// of the units' graphs (`stand rifle ar idle`): `ar`, `hp`, `sg`,
    /// `pr`, `rl`, `sr`, `pp`, `ne`, `ft`; `cg`, `cannon`, `gun` on vehicle
    /// weapons. V (Blood Gulch, every `weap`).
    pub label: String,
    /// `0x330` to `0x336`: the A to D in sources (`u16` each), Halo 1's
    /// weapon enum; [`WEAPON_PRIMARY_AMMUNITION`] among them. D (Blood
    /// Gulch's assault rifle: A 14, B 2, C 12, D 0).
    pub inputs: [u16; 4],
    /// The object's functions ([`functions`]); function `k` is the
    /// object's output `k` (A to D out).
    pub functions: Vec<Function>,
    /// `0x338` seconds.
    pub ready_time: f32,
    pub ready_effect: Option<TagId>,
    /// `0x34c` recovery threshold, `0x350` overheated threshold, `0x354`
    /// detonation threshold, `0x358` detonation fraction, `0x35c` heat loss
    /// per second, `0x360` illumination.
    pub heat_recovery_threshold: f32,
    pub overheated_threshold: f32,
    pub heat_detonation_threshold: f32,
    pub heat_detonation_fraction: f32,
    pub heat_loss_per_second: f32,
    pub illumination: f32,
    /// `0x374` overheated, `0x384` detonation effects.
    pub overheated: Option<TagId>,
    pub detonation: Option<TagId>,
    /// `0x394` player melee damage, `0x3a4` melee response (`jpt!`).
    pub melee_damage: Option<TagId>,
    pub melee_response: Option<TagId>,
    /// `0x3da` magnification levels, `0x3dc` magnification range.
    pub magnification_levels: i16,
    pub magnification_range: [f32; 2],
    /// `0x3e4` aim cone angle, `0x3e8` aim cone range, `0x3ec` pull
    /// angle, `0x3f0` pull range, `0x3f4` bend angle.
    pub cone_angle: f32,
    pub cone_range: f32,
    pub pull_angle: f32,
    pub pull_range: f32,
    pub bend_angle: f32,
    /// `0x45c` first-person model, `0x46c` its animations, `0x480` HUD
    /// interface (`wphi`), `0x490` pickup sound.
    pub first_person_model: Option<TagId>,
    pub first_person_animations: Option<TagId>,
    pub hud: Option<TagId>,
    pub pickup_sound: Option<TagId>,
    /// `0x4e2`.
    pub weapon_type: i16,
    pub magazines: Vec<Magazine>,
    pub triggers: Vec<Trigger>,
}

impl Weapon {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let b = tag_bytes(map, tag, b"weap", WEAP_SIZE)?;
        let magazines = elements(map, b, 0x4f0, MAGAZINE_SIZE)?
            .into_iter()
            .map(|m| {
                Ok(Magazine {
                    flags: u32_at(m, 0),
                    rounds_recharged: i16_at(m, 4),
                    rounds_total_initial: i16_at(m, 6),
                    rounds_total_maximum: i16_at(m, 8),
                    rounds_loaded_maximum: i16_at(m, 0xa),
                    reload_time: f32_at(m, 0x14),
                    rounds_reloaded: i16_at(m, 0x18),
                    chamber_time: f32_at(m, 0x1c),
                    reloading_effect: reference(map, m, 0x38),
                    chambering_effect: reference(map, m, 0x48),
                    objects: elements(map, m, 0x64, 0x1c)?
                        .into_iter()
                        .map(|o| (i16_at(o, 0), reference(map, o, 0xc)))
                        .collect(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let triggers = elements(map, b, 0x4fc, TRIGGER_SIZE)?
            .into_iter()
            .map(|t| {
                Ok(Trigger {
                    flags: u32_at(t, 0),
                    rounds_per_second: bounds(t, 4),
                    firing_acceleration_time: f32_at(t, 0xc),
                    firing_deceleration_time: f32_at(t, 0x10),
                    magazine: i16_at(t, 0x20),
                    rounds_per_shot: i16_at(t, 0x22),
                    minimum_rounds_loaded: i16_at(t, 0x24),
                    rounds_between_tracers: i16_at(t, 0x26),
                    firing_noise: i16_at(t, 0x2e),
                    error_acceleration_time: f32_at(t, 0x38),
                    error_deceleration_time: f32_at(t, 0x3c),
                    charging_effect: reference(map, t, 0x50),
                    distribution_function: i16_at(t, 0x6c),
                    projectiles_per_shot: i16_at(t, 0x6e),
                    distribution_angle: f32_at(t, 0x70),
                    minimum_error: f32_at(t, 0x78),
                    error_angle: bounds(t, 0x7c),
                    first_person_offset: vec3(t, 0x84),
                    projectile: reference(map, t, 0x94),
                    heat_per_round: f32_at(t, 0xb8),
                    firing_effects: elements(map, t, 0x108, FIRING_EFFECT_SIZE)?
                        .into_iter()
                        .map(|e| FiringEffect::read(map, e))
                        .collect(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            object: Object::read(map, b)?,
            flags: u32_at(b, 0x308),
            label: name32(b, 0x30c),
            inputs: std::array::from_fn(|i| u16_at(b, 0x330 + 2 * i)),
            functions: functions(map, b)?,
            ready_time: f32_at(b, 0x338),
            ready_effect: reference(map, b, 0x33c),
            heat_recovery_threshold: f32_at(b, 0x34c),
            overheated_threshold: f32_at(b, 0x350),
            heat_detonation_threshold: f32_at(b, 0x354),
            heat_detonation_fraction: f32_at(b, 0x358),
            heat_loss_per_second: f32_at(b, 0x35c),
            illumination: f32_at(b, 0x360),
            overheated: reference(map, b, 0x374),
            detonation: reference(map, b, 0x384),
            melee_damage: reference(map, b, 0x394),
            melee_response: reference(map, b, 0x3a4),
            magnification_levels: i16_at(b, 0x3da),
            magnification_range: bounds(b, 0x3dc),
            cone_angle: f32_at(b, 0x3e4),
            cone_range: f32_at(b, 0x3e8),
            pull_angle: f32_at(b, 0x3ec),
            pull_range: f32_at(b, 0x3f0),
            bend_angle: f32_at(b, 0x3f4),
            first_person_model: reference(map, b, 0x45c),
            first_person_animations: reference(map, b, 0x46c),
            hud: reference(map, b, 0x480),
            pickup_sound: reference(map, b, 0x490),
            weapon_type: i16_at(b, 0x4e2),
            magazines,
            triggers,
        })
    }
}

/// Halo 1's weapon input source (`weap +0x330`) "primary ammunition":
/// the first magazine's rounds loaded over its loaded maximum. D: the
/// assault rifle's B in, which its A out passes on, and its first-person
/// counter (`h1convert::shad::H1Numeric`, A out × 60) shows the rounds
/// loaded. Its 12 (illumination) and 14 (integrated light) drive its
/// other two functions.
pub const WEAPON_PRIMARY_AMMUNITION: u16 = 2;

impl Weapon {
    /// The loaded maximum of the first magazine when the weapon's A out
    /// is its primary ammunition ([`WEAPON_PRIMARY_AMMUNITION`]) as it
    /// is: function 0 is "one" scaled by the input naming it, not
    /// inverted, with no period, map, steps, add, result scale or bounds
    /// of its own. Halo 1's numeric shaders count A out × their limit
    /// (`h1convert::shad::H1Numeric`), so with the limit at this maximum
    /// they count the rounds loaded. `None` otherwise.
    pub fn rounds_counter(&self) -> Option<i16> {
        let f = self.functions.first()?;
        let k = usize::from(f.scale_function_by)
            .checked_sub(1)
            .filter(|&k| k < 4)?;
        let plain = f.function == 0
            && f.flags & 1 == 0
            && f.scale_period_by == 0
            && f.map_to == 0
            && f.add == 0
            && f.scale_result_by == 0
            && f.step_count <= 1
            && f.sawtooth_count <= 1
            && f.bounds == [0.0, 1.0];
        (plain && self.inputs[k] == WEAPON_PRIMARY_AMMUNITION)
            .then(|| self.magazines.first().map(|m| m.rounds_loaded_maximum))
            .flatten()
            .filter(|&m| m > 0)
    }
}

/// `proj`.
#[derive(Clone, Debug, PartialEq)]
pub struct Projectile {
    pub object: Object,
    pub flags: u32,
    /// `0x180` detonation timer starts, `0x182` impact noise.
    pub detonation_timer_starts: i16,
    pub impact_noise: i16,
    /// `0x18c` super detonation, `0x1ac` effect.
    pub super_detonation: Option<TagId>,
    pub effect: Option<TagId>,
    /// `0x19c` AI perception radius, `0x1a0` collision radius, `0x1a4`
    /// arming time, `0x1a8` danger radius.
    pub ai_perception_radius: f32,
    pub collision_radius: f32,
    pub arming_time: f32,
    pub danger_radius: f32,
    /// `0x1bc` timer, seconds.
    pub timer: [f32; 2],
    /// `0x1c4` minimum velocity (per tick), `0x1c8` maximum range.
    pub minimum_velocity: f32,
    pub maximum_range: f32,
    /// `0x1cc` air gravity scale, `0x1d0` air damage range, `0x1d8` water
    /// gravity scale, `0x1dc` water damage range.
    pub air_gravity_scale: f32,
    pub air_damage_range: [f32; 2],
    pub water_gravity_scale: f32,
    pub water_damage_range: [f32; 2],
    /// `0x1e4` initial, `0x1e8` final velocity, `0x1ec` guided angular
    /// velocity: world units (radians) **per tick** in the cache.
    pub initial_velocity: f32,
    pub final_velocity: f32,
    pub guided_angular_velocity: f32,
    /// `0x1f0`.
    pub detonation_noise: i16,
    /// `0x1f4` detonation started, `0x204` flyby sound, `0x214` attached
    /// detonation damage, `0x224` impact damage.
    pub detonation_started: Option<TagId>,
    pub flyby_sound: Option<TagId>,
    pub attached_detonation_damage: Option<TagId>,
    pub impact_damage: Option<TagId>,
    /// `0x240` material responses (`0xa0`), references only.
    pub material_responses: Vec<Vec<Option<TagId>>>,
}

impl Projectile {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let b = tag_bytes(map, tag, b"proj", PROJ_SIZE)?;
        Ok(Self {
            object: Object::read(map, b)?,
            flags: u32_at(b, 0x17c),
            detonation_timer_starts: i16_at(b, 0x180),
            impact_noise: i16_at(b, 0x182),
            super_detonation: reference(map, b, 0x18c),
            effect: reference(map, b, 0x1ac),
            ai_perception_radius: f32_at(b, 0x19c),
            collision_radius: f32_at(b, 0x1a0),
            arming_time: f32_at(b, 0x1a4),
            danger_radius: f32_at(b, 0x1a8),
            timer: bounds(b, 0x1bc),
            minimum_velocity: f32_at(b, 0x1c4),
            maximum_range: f32_at(b, 0x1c8),
            air_gravity_scale: f32_at(b, 0x1cc),
            air_damage_range: bounds(b, 0x1d0),
            water_gravity_scale: f32_at(b, 0x1d8),
            water_damage_range: bounds(b, 0x1dc),
            initial_velocity: f32_at(b, 0x1e4),
            final_velocity: f32_at(b, 0x1e8),
            guided_angular_velocity: f32_at(b, 0x1ec),
            detonation_noise: i16_at(b, 0x1f0),
            detonation_started: reference(map, b, 0x1f4),
            flyby_sound: reference(map, b, 0x204),
            attached_detonation_damage: reference(map, b, 0x214),
            impact_damage: reference(map, b, 0x224),
            material_responses: elements(map, b, 0x240, 0xa0)?
                .into_iter()
                .map(|e| {
                    crate::refs::scan(map, e)
                        .into_iter()
                        .map(|f| f.tag)
                        .collect()
                })
                .collect(),
        })
    }
}

/// `eqip`.
#[derive(Clone, Debug, PartialEq)]
pub struct Equipment {
    pub object: Object,
    /// `0x308`: 0 none, 1 double speed, 2 overshield, 3 active camouflage,
    /// 4 full-spectrum vision, 5 health, 6 grenade.
    pub powerup_type: i16,
    /// `0x30a`: 0 frag, 1 plasma.
    pub grenade_type: i16,
    /// `0x30c` seconds.
    pub powerup_time: f32,
    /// `0x310`.
    pub pickup_sound: Option<TagId>,
}

impl Equipment {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let b = tag_bytes(map, tag, b"eqip", EQIP_SIZE)?;
        Ok(Self {
            object: Object::read(map, b)?,
            powerup_type: i16_at(b, 0x308),
            grenade_type: i16_at(b, 0x30a),
            powerup_time: f32_at(b, 0x30c),
            pickup_sound: reference(map, b, 0x310),
        })
    }
}

/// The unit layer's seats block (`unit +0x2e4`, V: every `vehi` and
/// `bipd` on a10, b30 and c10 reads with this stride, each seat's label,
/// marker and camera marker printable text).
pub const UNIT_SEATS: usize = 0x2e4;
/// A seat element.
pub const SEAT_SIZE: usize = 0x11c;

/// One seat of a unit (`0x11c`).
#[derive(Clone, Debug, PartialEq)]
pub struct Seat {
    /// `0x00` flags (Halo 1's; no Halo 2 meaning established).
    pub flags: u32,
    /// Pitch auto-level and range, then yaw range (raw float words).
    pub camera_angles: [u32; 5],
    /// `0x04` the label scripts name the seat by (`W-driver`, `CT-driver`),
    /// 32 characters.
    pub label: String,
    /// `0x24` the model marker the rider sits at (`driver`, `passenger`).
    pub marker: String,
    /// `0x84` the camera marker (`camera`, `camera rider`), empty for none.
    pub camera_marker: String,
    /// `0xd0` the camera tracks (`0x1c` each, a `trak` reference at `+0`),
    /// in order: on the warthog's driver the loose track, then the tight
    /// one; none on the scorpion's riders. V: Blood Gulch's vehicles.
    pub camera_tracks: Vec<TagId>,
}

/// The unit's own camera tracks (`0x1f4`, elements as a seat's): the
/// tracks a seat with none of its own uses, before the globals' default
/// unit track. V: the cyborg's one (`camera\\biped_small_tight`); none on
/// Blood Gulch's vehicles, whose seats hold theirs.
pub const UNIT_CAMERA_TRACKS: usize = 0x1f4;

/// A seat's camera tracks block (`0xd0`) and its element size.
pub const SEAT_CAMERA_TRACKS: usize = 0xd0;
pub const SEAT_CAMERA_TRACK_SIZE: usize = 0x1c;

/// `trak`: a camera track. The root is `0x30` bytes: flags at `+0`, the
/// control points at `+4` (`0x3c` each: position `+0`, orientation
/// quaternion `+0xc` as i, j, k, w). V: Blood Gulch's five tracks hold
/// nine points each, the orientations a pitch from -90 to +90 degrees.
pub const TRAK_SIZE: usize = 0x30;
pub const TRAK_POINT_SIZE: usize = 0x3c;

/// A camera track ([`TRAK_SIZE`]).
#[derive(Clone, Debug, PartialEq)]
pub struct CameraTrack {
    pub flags: u32,
    /// Each control point's position and orientation (i, j, k, w).
    pub points: Vec<([f32; 3], [f32; 4])>,
}

impl CameraTrack {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let b = tag_bytes(map, tag, b"trak", TRAK_SIZE)?;
        let points = elements(map, b, 4, TRAK_POINT_SIZE)?
            .into_iter()
            .map(|e| (vec3(e, 0), std::array::from_fn(|i| f32_at(e, 0xc + 4 * i))))
            .collect();
        Ok(Self {
            flags: u32_at(b, 0),
            points,
        })
    }
}

/// `bipd` or `vehi`: the object header, the seats and every reference in
/// the root; the rest of their tuning has no reviewed Halo 2 home yet
/// (see the doc).
#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    pub group: Group,
    pub object: Object,
    /// `0x1a0` the camera's field of view, radians (Halo 2's `unit +0xcc`).
    pub camera_field_of_view: f32,
    /// A biped's words that Halo 2 bipeds hold at a known offset
    /// ([`crate::rebuild::BIPED_WORDS`]), by Halo 1 offset; empty for a
    /// vehicle.
    pub biped_words: Vec<(usize, u32)>,
    pub seats: Vec<Seat>,
    /// The unit HUD interfaces at +0x2a8, in authored order.
    pub hud_interfaces: Vec<TagId>,
    /// The unit's own camera tracks ([`UNIT_CAMERA_TRACKS`]).
    pub camera_tracks: Vec<TagId>,
    /// A biped's collision size ([`BIPED_SIZE`]); `None` for a vehicle.
    pub size: Option<BipedSize>,
    /// Every reference in the tag's bytes (root and its blocks).
    pub references: Vec<crate::refs::Found>,
}

/// `bipd +0x424`, `+0x428`, `+0x42c`: the standing and crouching
/// collision heights and the collision radius, world units. V: on the 42
/// bipeds of the ten campaign levels, heights 0.2 to 1.0 and radii 0.03
/// to 0.5, crouching no higher than standing but on the flood combat
/// elite; seven hold zero heights (the pilot, the engineer, the flood
/// infection forms, the monitor, the sentinel) and keep their role's
/// (`tests/unit_rebuild_disc.rs`). Halo 2 holds the same three at `bipd
/// +0x268`, `+0x26c`, `+0x270` ([`crate::rebuild::BIPD_SIZE`]): the
/// Chief's 0.725, 0.5, 0.175 against the cyborg's 0.7, 0.5, 0.2.
pub const BIPED_SIZE: usize = 0x424;

/// A biped's collision size ([`BIPED_SIZE`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BipedSize {
    pub standing_height: f32,
    pub crouching_height: f32,
    pub radius: f32,
}

impl Unit {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let g = &tag.group.0.to_be_bytes();
        if g != b"bipd" && g != b"vehi" {
            return Err(Error::WrongGroup(tag.group.to_string(), *b"unit"));
        }
        let b = map.tag_data(tag)?;
        if b.len() < UNIT_SEATS + 12 || (g == b"bipd" && b.len() < 0x2f8) {
            return Err(Error::Truncated("unit seats"));
        }
        let seats = elements(map, b, UNIT_SEATS, SEAT_SIZE)?
            .into_iter()
            .map(|e| {
                let camera_tracks = elements(map, e, SEAT_CAMERA_TRACKS, SEAT_CAMERA_TRACK_SIZE)?
                    .into_iter()
                    .filter_map(|t| reference(map, t, 0))
                    .collect();
                Ok(Seat {
                    flags: u32_at(e, 0),
                    label: name32(e, 4),
                    camera_angles: [0xc4, 0xc8, 0xcc, 0xf0, 0xf4].map(|at| u32_at(e, at)),
                    marker: name32(e, 0x24),
                    camera_marker: name32(e, 0x84),
                    camera_tracks,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let biped_words = if g == b"bipd" {
            crate::rebuild::BIPED_WORDS.iter().chain(crate::rebuild::UNIT_WORDS)
                .chain(crate::rebuild::BIPED_RATES)
                .filter(|w| w.1 + 4 <= b.len())
                .map(|w| (w.1, u32_at(b, w.1)))
                .chain(std::iter::once((0x2f4, u32_at(b, 0x2f4))))
                .collect()
        } else {
            Vec::new()
        };
        Ok(Self {
            group: tag.group,
            object: Object::read(map, b)?,
            camera_field_of_view: f32_at(b, 0x1a0),
            biped_words,
            seats,
            hud_interfaces: elements(map, b, 0x2a8, 0x30)?.into_iter()
                .filter_map(|e| reference(map, e, 0)).collect(),
            camera_tracks: elements(map, b, UNIT_CAMERA_TRACKS, SEAT_CAMERA_TRACK_SIZE)?
                .into_iter()
                .filter_map(|t| reference(map, t, 0))
                .collect(),
            size: (g == b"bipd" && b.len() >= BIPED_SIZE + 12).then(|| BipedSize {
                standing_height: f32_at(b, BIPED_SIZE),
                crouching_height: f32_at(b, BIPED_SIZE + 4),
                radius: f32_at(b, BIPED_SIZE + 8),
            }),
            references: crate::refs::scan(map, b),
        })
    }
}

/// The device layer (`devi`) starts after the object header and ends at
/// `0x290`, where `mach` and `ctrl` start their own words. Root sizes
/// (V: the smallest tag of each group across the ten levels, one with no
/// blocks): `mach` `0x324`, `ctrl` `0x318`, `lifi` `0x338`.
pub const DEVI_SIZE: usize = 0x290;
pub const MACH_SIZE: usize = 0x324;
pub const CTRL_SIZE: usize = 0x318;
pub const LIFI_SIZE: usize = 0x338;

/// What a `mach` holds after the device layer.
#[derive(Clone, Debug, PartialEq)]
pub struct Machine {
    /// `0x290`: 0 door, 1 platform, 2 gear (V: the doors of every level
    /// hold 0 or 1, the holograms and spinning displays 2; Halo 2 holds the
    /// same three values at `mach +0x11c`).
    pub machine_type: u16,
    /// `0x292` (V: 1 or 3 on doors, 4 on lifts, as Halo 2's `+0x11e`).
    pub flags: u16,
    /// `0x294` seconds a door stays open (V: 1.5 on the small doors, as on
    /// Halo 2's).
    pub door_open_time: f32,
    /// `0x2e8` (V: 0 or 1 on doors and lifts).
    pub collision_response: u16,
    /// `0x2ea` (V: 1 or 2 on c10's falling lift and c20's platform).
    pub elevator_node: i16,
}

/// What a `ctrl` holds after the device layer: `0x290` type, `0x292`
/// trigger, `0x294` value (V: all zero on the disc's nine controls).
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    pub control_type: u16,
    pub trigger: u16,
    pub value: f32,
}

/// `mach`, `ctrl` or `lifi`: the object header, the device layer and the
/// group's own words.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub group: Group,
    pub object: Object,
    /// `0x17c` (V: bit 0 set on the gears whose position loops, as Halo
    /// 2's `devi +0xbc`).
    pub flags: u32,
    /// `0x180..0x198` seconds: power transition, power acceleration,
    /// position transition, position acceleration, depowered position
    /// transition, depowered position acceleration (V: a10's small door
    /// 2, 2, 0.8, 0.2, 0, 0; its postprocessed per-tick inverses at
    /// `0x274..0x28c` agree).
    pub times: [f32; 6],
    /// `0x200` seconds (V: 0.5 or 2 on a50's and b40's doors only).
    pub delay_time: f32,
    /// `0x21c` world units (V: 0.85 to 5 on doors only).
    pub activation_radius: f32,
    pub machine: Option<Machine>,
    pub control: Option<Control>,
}

impl Device {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let g = &tag.group.0.to_be_bytes();
        let size = match g {
            b"mach" => MACH_SIZE,
            b"ctrl" => CTRL_SIZE,
            b"lifi" => LIFI_SIZE,
            _ => return Err(Error::WrongGroup(tag.group.to_string(), *b"devi")),
        };
        let b = tag_bytes(map, tag, g, size)?;
        Ok(Self {
            group: tag.group,
            object: Object::read(map, b)?,
            flags: u32_at(b, 0x17c),
            times: std::array::from_fn(|i| f32_at(b, 0x180 + 4 * i)),
            delay_time: f32_at(b, 0x200),
            activation_radius: f32_at(b, 0x21c),
            machine: (g == b"mach").then(|| Machine {
                machine_type: u16_at(b, 0x290),
                flags: u16_at(b, 0x292),
                door_open_time: f32_at(b, 0x294),
                collision_response: u16_at(b, 0x2e8),
                elevator_node: i16_at(b, 0x2ea),
            }),
            control: (g == b"ctrl").then(|| Control {
                control_type: u16_at(b, 0x290),
                trigger: u16_at(b, 0x292),
                value: f32_at(b, 0x294),
            }),
        })
    }
}

/// A collision region (`0x54`).
#[derive(Clone, Debug, PartialEq)]
pub struct CollRegion {
    pub name: String,
    /// `+0x38` destroyed effect.
    pub destroyed_effect: Option<TagId>,
    /// `+0x48` permutations (`0x20`, a name each).
    pub permutations: Vec<String>,
}

/// A collision node (`0x40`).
#[derive(Clone, Debug, PartialEq)]
pub struct CollNode {
    pub name: String,
    /// `+0x20` region (-1 none), `+0x22` parent, `+0x24` next sibling,
    /// `+0x26` first child.
    pub region: i16,
    pub parent: i16,
    pub next_sibling: i16,
    pub first_child: i16,
    /// `+0x34` BSPs (`0x60`), one per permutation of the node's region.
    pub bsps: Vec<h1map::bsp::CollisionBsp>,
}

/// A pathfinding sphere (`0x20`).
#[derive(Clone, Debug, PartialEq)]
pub struct Sphere {
    pub node: i16,
    pub center: [f32; 3],
    pub radius: f32,
}

/// `coll`.
#[derive(Clone, Debug, PartialEq)]
pub struct Collision {
    /// `0x08` maximum body vitality, `0xcc` maximum shield vitality.
    pub body_vitality: f32,
    pub shield_vitality: f32,
    /// Every reference in the root (damage effects).
    pub references: Vec<crate::refs::Found>,
    /// `0x234` materials (`0x48`), a name each.
    pub materials: Vec<String>,
    /// Each material's type (`+0x24`, `u16`): an index into Halo 1's
    /// material types, [`crate::arsenal::MATERIALS`] order (V:
    /// `hlmt_materials_disc`, Blood Gulch: stone for boulders, wood for
    /// trees, cyborg armor for the cyborg, rubber, metal and glass for
    /// the warthog).
    pub material_types: Vec<u16>,
    /// `0x240`.
    pub regions: Vec<CollRegion>,
    /// `0x24c` modifiers (`0x34`): counted only.
    pub modifiers: u32,
    /// `0x280`.
    pub spheres: Vec<Sphere>,
    /// `0x28c`.
    pub nodes: Vec<CollNode>,
}

impl Collision {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let b = tag_bytes(map, tag, b"coll", COLL_SIZE)?;
        let space = map.metadata();
        let regions = elements(map, b, 0x240, 0x54)?
            .into_iter()
            .map(|r| {
                Ok(CollRegion {
                    name: name32(r, 0),
                    destroyed_effect: reference(map, r, 0x38),
                    permutations: elements(map, r, 0x48, 0x20)?
                        .into_iter()
                        .map(|p| name32(p, 0))
                        .collect(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let nodes = elements(map, b, 0x28c, 0x40)?
            .into_iter()
            .map(|n| {
                Ok(CollNode {
                    name: name32(n, 0),
                    region: i16_at(n, 0x20),
                    parent: i16_at(n, 0x22),
                    next_sibling: i16_at(n, 0x24),
                    first_child: i16_at(n, 0x26),
                    bsps: elements(map, n, 0x34, 0x60)?
                        .into_iter()
                        .map(|e| h1map::bsp::CollisionBsp::read(&space, e).map_err(Error::from))
                        .collect::<Result<Vec<_>>>()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            body_vitality: f32_at(b, 0x08),
            shield_vitality: f32_at(b, 0xcc),
            references: crate::refs::scan(map, &b[..COLL_SIZE]),
            materials: elements(map, b, 0x234, 0x48)?
                .into_iter()
                .map(|m| name32(m, 0))
                .collect(),
            material_types: elements(map, b, 0x234, 0x48)?
                .into_iter()
                .map(|m| i16_at(m, 0x24) as u16)
                .collect(),
            regions,
            modifiers: Block::read(b, 0x24c).map_or(0, |b| b.count),
            spheres: elements(map, b, 0x280, 0x20)?
                .into_iter()
                .map(|s| Sphere {
                    node: i16_at(s, 0),
                    center: vec3(s, 0x10),
                    radius: f32_at(s, 0x1c),
                })
                .collect(),
            nodes,
        })
    }
}

/// A `phys` mass point (`0x80`).
#[derive(Clone, Debug, PartialEq)]
pub struct MassPoint {
    pub name: String,
    /// `+0x20` powered mass point, `+0x22` model node.
    pub powered_mass_point: i16,
    pub model_node: i16,
    /// `+0x2c` mass, `+0x34` density.
    pub mass: f32,
    pub density: f32,
    /// `+0x38` position, `+0x68` radius.
    pub position: [f32; 3],
    pub radius: f32,
}

/// `phys` (vehicle physics).
#[derive(Clone, Debug, PartialEq)]
pub struct Physics {
    /// `0x08` mass, `0x0c` centre of mass, `0x18` density, `0x1c` gravity
    /// scale.
    pub mass: f32,
    pub center_of_mass: [f32; 3],
    pub density: f32,
    pub gravity_scale: f32,
    /// `0x50` moments xx, yy, zz.
    pub moments: [f32; 3],
    /// `0x68` powered mass points (`0x80`): counted only.
    pub powered_mass_points: u32,
    /// Each powered mass point's flags (`+0x20`): bit 0
    /// ([`POWERED_GROUND_FRICTION`]) on the warthog's and scorpion's
    /// (wheels and treads), bit 6 on the ghost's (antigravity). V: Blood
    /// Gulch's three vehicles.
    pub powered_flags: Vec<u32>,
    /// Strength, offset, height, damping per tick and the two lift slope cosines.
    pub antigravity: Vec<[f32; 6]>,
    /// `0x74`.
    pub mass_points: Vec<MassPoint>,
}

impl Physics {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let b = tag_bytes(map, tag, b"phys", PHYS_SIZE)?;
        Ok(Self {
            mass: f32_at(b, 0x08),
            center_of_mass: vec3(b, 0x0c),
            density: f32_at(b, 0x18),
            gravity_scale: f32_at(b, 0x1c),
            moments: vec3(b, 0x50),
            powered_mass_points: Block::read(b, 0x68).map_or(0, |b| b.count),
            antigravity: elements(map, b, 0x68, 0x80)?.into_iter()
                .map(|p| [0x24, 0x28, 0x2c, 0x30, 0x34, 0x38].map(|at| f32_at(p, at))).collect(),
            powered_flags: elements(map, b, 0x68, 0x80)?
                .into_iter()
                .map(|p| u32_at(p, 0x20))
                .collect(),
            mass_points: elements(map, b, 0x74, 0x80)?
                .into_iter()
                .map(|m| MassPoint {
                    name: name32(m, 0),
                    powered_mass_point: i16_at(m, 0x20),
                    model_node: i16_at(m, 0x22),
                    mass: f32_at(m, 0x2c),
                    density: f32_at(m, 0x34),
                    position: vec3(m, 0x38),
                    radius: f32_at(m, 0x68),
                })
                .collect(),
        })
    }
}

/// A powered mass point's ground friction flag ([`Physics::powered_flags`]):
/// the point is a wheel or a tread that grips the ground.
pub const POWERED_GROUND_FRICTION: u32 = 1;

/// Any object tag this crate reads.
#[derive(Clone, Debug, PartialEq)]
pub enum H1Object {
    Weapon(Box<Weapon>),
    Projectile(Box<Projectile>),
    Equipment(Box<Equipment>),
    Unit(Box<Unit>),
}

impl H1Object {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        Ok(match &tag.group.0.to_be_bytes() {
            b"weap" => Self::Weapon(Box::new(Weapon::read(map, tag)?)),
            b"proj" => Self::Projectile(Box::new(Projectile::read(map, tag)?)),
            b"eqip" => Self::Equipment(Box::new(Equipment::read(map, tag)?)),
            b"bipd" | b"vehi" => Self::Unit(Box::new(Unit::read(map, tag)?)),
            _ => return Err(Error::WrongGroup(tag.group.to_string(), *b"obje")),
        })
    }
    pub fn object(&self) -> &Object {
        match self {
            Self::Weapon(w) => &w.object,
            Self::Projectile(p) => &p.object,
            Self::Equipment(e) => &e.object,
            Self::Unit(u) => &u.object,
        }
    }
}

/// Halo 1 `effe` root size (V: every effect's first child block starts
/// here, on every map of the disc).
pub const EFFE_SIZE: usize = 0x40;
/// `effe +0x28` locations (`0x20`: a marker name), `+0x34` events.
pub const EFFE_LOCATION: usize = 0x20;
pub const EFFE_EVENT: usize = 0x44;
/// An event's parts (`event +0x2c`) and particles (`event +0x38`).
pub const EFFE_PART: usize = 0x68;
pub const EFFE_PARTICLE: usize = 0xe8;

/// One part of a Halo 1 effect event (`0x68`). V (every part on every
/// map, `tests/effects_disc.rs`): the reference at `+0x18` resolves to a
/// tag of its own group word (`snd!`, `deca`, `ligh`, `jpt!`, `pctl`,
/// `garb`, `proj`), and the floats from `+0x40` on hold the same
/// quantities in the same order as Halo 2's part from `+0x14` (velocity
/// bounds, cone angle in radians, angular velocity bounds, radius
/// modifier bounds, then the two scale words, `0x20` on the same parts in
/// both).
#[derive(Clone, Debug, PartialEq)]
pub struct EffectPart {
    /// `+0` (0 or 1 on the disc), as Halo 2's `create_in`.
    pub create_in: u16,
    /// `+2` (0 or 1): Halo 1 only; no Halo 2 home.
    pub violence_mode: u16,
    /// `+4` index into the effect's locations.
    pub location: i16,
    /// `+6` (bit 0 on 289 parts), as Halo 2's part flags.
    pub flags: u16,
    /// `+0x18` the reference's group word and tag.
    pub group: Group,
    pub reference: Option<TagId>,
    pub velocity_bounds: [f32; 2],
    pub velocity_cone_angle: f32,
    pub angular_velocity_bounds: [f32; 2],
    pub radius_modifier_bounds: [f32; 2],
    pub a_scales: u32,
    pub b_scales: u32,
}

/// One event of a Halo 1 effect (`0x44`): `+4` skip fraction, `+8`
/// delay bounds, `+0x10` duration bounds (V: the order and ranges of
/// Halo 2's event; `+0..4` and `+0x18..0x2c` zero everywhere).
#[derive(Clone, Debug, PartialEq)]
pub struct EffectEvent {
    pub skip_fraction: f32,
    pub delay_bounds: [f32; 2],
    pub duration_bounds: [f32; 2],
    pub parts: Vec<EffectPart>,
    /// `+0x38` particles (`0xe8`, a `part` reference at `+0x54`): counted,
    /// not converted (Halo 2's particles are `prt3` on shader templates).
    pub particles: usize,
}

/// A Halo 1 effect: `+0` flags, `+4`/`+6` loop start and stop events
/// (-1 none), the locations' marker names and the events.
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub flags: u32,
    pub loop_start_event: i16,
    pub loop_stop_event: i16,
    pub locations: Vec<String>,
    pub events: Vec<EffectEvent>,
}

impl Effect {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let b = tag_bytes(map, tag, b"effe", EFFE_SIZE)?;
        let locations = elements(map, b, 0x28, EFFE_LOCATION)?
            .into_iter()
            .map(|e| name32(e, 0))
            .collect();
        let mut events = Vec::new();
        for e in elements(map, b, 0x34, EFFE_EVENT)? {
            let parts = elements(map, e, 0x2c, EFFE_PART)?
                .into_iter()
                .map(|p| EffectPart {
                    create_in: u16_at(p, 0),
                    violence_mode: u16_at(p, 2),
                    location: i16_at(p, 4),
                    flags: u16_at(p, 6),
                    group: Group(u32_at(p, 0x18)),
                    reference: reference(map, p, 0x18),
                    velocity_bounds: bounds(p, 0x40),
                    velocity_cone_angle: f32_at(p, 0x48),
                    angular_velocity_bounds: bounds(p, 0x4c),
                    radius_modifier_bounds: bounds(p, 0x54),
                    a_scales: u32_at(p, 0x60),
                    b_scales: u32_at(p, 0x64),
                })
                .collect();
            let particles = Block::read(e, 0x38).map_or(0, |k| k.count as usize);
            events.push(EffectEvent {
                skip_fraction: f32_at(e, 4),
                delay_bounds: bounds(e, 8),
                duration_bounds: bounds(e, 0x10),
                parts,
                particles,
            });
        }
        Ok(Self {
            flags: u32_at(b, 0),
            loop_start_event: i16_at(b, 4),
            loop_stop_event: i16_at(b, 6),
            locations,
            events,
        })
    }
}

/// One attachment of a Halo 1 object (`obje +0x140`, `0x48`): the
/// attached tag at `+0`, the model marker it rides at `+0x10` (32
/// characters), two output scales and a change-colour selector.
#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub group: Group,
    pub tag: Option<TagId>,
    pub marker: String,
    /// `+0x30`: 0 none, 1 to 4 the object's A to D output (its function
    /// of that index, [`Function`]). V (`tests/object_functions_disc.rs`
    /// `halo1_vehicle_functions_survey`): the warthog's headlights take
    /// 1, its taillights 2 and its engine sound 3, which are the outputs
    /// its functions name `headlights`, `taillights` and `speed`.
    pub primary_scale: u16,
    /// `+0x32`: same output indices as the primary scale.
    pub secondary_scale: u16,
    /// `+0x34`: 0 none, 1 to 4 change colour A to D.
    pub change_colour: u16,
}

/// Every attachment of the object in `b` (a tag's bytes from `obje +0`).
pub fn attachments(map: &H1Map, b: &[u8]) -> Result<Vec<Attachment>> {
    Ok(elements(map, b, 0x140, 0x48)?
        .into_iter()
        .map(|e| Attachment {
            group: Group(u32_at(e, 0)),
            tag: reference(map, e, 0),
            marker: name32(e, 0x10),
            primary_scale: u16_at(e, 0x30),
            secondary_scale: u16_at(e, 0x32),
            change_colour: u16_at(e, 0x34),
        })
        .collect())
}

/// One of a Halo 1 object's functions (`obje +0x158`, `0x168` each): the
/// output of that index (A to D). V (`halo1_vehicle_functions_survey`,
/// every vehicle of five levels): each is `function` 0 (one) scaled by
/// one input (`scale_function_by` 1 to 4, A to D in), mapped linearly,
/// with its bounds; `turn_off_with` names another function by index (-1
/// none; the ghost's and scorpion's engines name the one that is the
/// driver's seat power). Names D.
#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    /// `+0`: bit 0 invert, bit 1 additive, bit 2 always active.
    pub flags: u32,
    /// `+4`: period in seconds.
    pub period: f32,
    /// Authored wobble shape, period or magnitude at +0xe..+0x18.
    pub wobble: bool,
    /// `+0xa`: 0 one, 1 zero, then the periodic shapes.
    pub function: u16,
    /// `+0xc`: 0 none, 1 to 4 A to D in, 5 to 8 A to D out.
    pub scale_function_by: u16,
    /// `+0x8` scale period by, `+0x1e` map to (0 linear), `+0x22` add,
    /// `+0x24` scale result by: any nonzero one is a shape this
    /// conversion doesn't carry.
    pub scale_period_by: u16,
    pub map_to: u16,
    pub add: u16,
    pub scale_result_by: u16,
    /// `+0x1c` step count, `+0x20` sawtooth count (0 or 1: none).
    pub step_count: i16,
    pub sawtooth_count: i16,
    /// `+0x26`: 0 clip, 1 clip and normalize, 2 scale to fit, to
    /// `bounds` (`+0x28`).
    pub bounds_mode: u16,
    pub bounds: [f32; 2],
    /// `+0x36`: the index of the function that turns this one off.
    pub turn_off_with: i16,
}

/// Every function of the object in `b` (a tag's bytes from `obje +0`).
pub fn functions(map: &H1Map, b: &[u8]) -> Result<Vec<Function>> {
    Ok(elements(map, b, 0x158, 0x168)?
        .into_iter()
        .map(|f| Function {
            flags: u32_at(f, 0),
            period: f32_at(f, 4),
            wobble: f[0xe..0x18].iter().any(|&b| b != 0),
            scale_period_by: u16_at(f, 8),
            function: u16_at(f, 0xa),
            scale_function_by: u16_at(f, 0xc),
            step_count: i16_at(f, 0x1c),
            map_to: u16_at(f, 0x1e),
            sawtooth_count: i16_at(f, 0x20),
            add: u16_at(f, 0x22),
            scale_result_by: u16_at(f, 0x24),
            bounds_mode: u16_at(f, 0x26),
            bounds: bounds(f, 0x28),
            turn_off_with: i16_at(f, 0x36),
        })
        .collect())
}

/// Where an object's A to D inputs come from: its `obje` sources
/// (`+0x108`), a unit's (`unit +0x198`) and a vehicle's (`vehi +0x31c`),
/// or a weapon's (`weap +0x330`), each four 16-bit enums. V
/// (`halo1_vehicle_functions_survey`): a
/// vehicle's input takes the vehicle's source where it names one, else
/// the unit's: the warthog's A (headlights) is the unit's 1 (driver's
/// seat power) with the vehicle's 0, its B (taillights) the vehicle's 12
/// (jump), its C (engine) the vehicle's 35 (engine hack); the ghost's B
/// and the scorpion's D (engines) are 35, the pelican's A to C 33, 34
/// and 35 (hover, thrust, engine hack). No vehicle names an `obje`
/// source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FunctionInputs {
    pub object: [u16; 4],
    pub unit: [u16; 4],
    pub vehicle: [u16; 4],
    pub weapon: [u16; 4],
    pub device: [u16; 4],
}

impl FunctionInputs {
    /// The inputs of the object in `b` whose type is `object_type`
    /// ([`OBJECT_TYPES`]): units, vehicles, weapons and devices read their own.
    pub fn read(b: &[u8], object_type: u16) -> Self {
        let four = |at: usize| -> [u16; 4] {
            if b.len() >= at + 8 {
                std::array::from_fn(|i| u16_at(b, at + 2 * i))
            } else {
                [0; 4]
            }
        };
        let unit = matches!(object_type, 0 | 1);
        Self {
            object: four(0x108),
            unit: if unit { four(0x198) } else { [0; 4] },
            vehicle: if object_type == 1 { four(0x31c) } else { [0; 4] },
            // E (Halo 1) 0x000bec67 and 0x00117c49, respectively.
            weapon: if object_type == 2 {
                four(0x330)
            } else {
                [0; 4]
            },
            device: if matches!(object_type, 7..=9) {
                four(0x198)
            } else {
                [0; 4]
            },
        }
    }
}

/// Halo 1 `ligh` (`0x160`, no blocks). V (`effects_disc.rs`
/// `halo1_group_word_survey`, 304 lights on six maps): `+4` a radius in
/// world units (0.05 to 15) on every light; `+0x10`, `+0x14` two angles in
/// radians (20 to 180 degrees; 180 on most, a full sphere); `+0x38` and
/// `+0x48` two colours as alpha then red, green, blue (each 0 to 1);
/// `+0xac` a `lens` reference; `+0xf4` a duration on 59 lights, values
/// that are multiples of three ticks (3 to 180). Names D.
pub const LIGH_SIZE: usize = 0x160;

#[derive(Clone, Debug, PartialEq)]
pub struct Light {
    pub flags: u32,
    pub radius: f32,
    /// `+0x10` where a cone starts to fade, `+0x14` where it ends
    /// (radians, full angle; pi for a sphere).
    pub falloff_angle: f32,
    pub cutoff_angle: f32,
    /// `+0x38` and `+0x48`: alpha, red, green, blue.
    pub colour_lower: [f32; 4],
    pub colour_upper: [f32; 4],
    /// `+0xf4` ticks (0: not timed).
    pub duration_ticks: f32,
    pub lens_flare: Option<TagId>,
}

impl Light {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let b = tag_bytes(map, tag, b"ligh", LIGH_SIZE)?;
        let argb = |at| std::array::from_fn(|i| f32_at(b, at + 4 * i));
        Ok(Self {
            flags: u32_at(b, 0),
            radius: f32_at(b, 4),
            falloff_angle: f32_at(b, 0x10),
            cutoff_angle: f32_at(b, 0x14),
            colour_lower: argb(0x38),
            colour_upper: argb(0x48),
            duration_ticks: f32_at(b, 0xf4),
            lens_flare: reference(map, b, 0xac),
        })
    }

    /// A cone rather than a sphere: a cutoff angle under a half turn.
    pub fn is_cone(&self) -> bool {
        self.cutoff_angle > 0.0 && self.cutoff_angle < std::f32::consts::PI - 1e-3
    }
}

/// Halo 1 `deca` (`0x10c`, no blocks). V (`halo1_group_word_survey`, 270
/// decals on six maps): `+0` flags (u16) and `+2` kind (u16, 0 to 3),
/// `+4` layer (0 to 3), `+8` the next decal, `+0x18` radius bounds,
/// `+0x34` and `+0x40` two colours (red, green, blue), `+0x78` lifetime
/// and `+0x80` decay time bounds (seconds), `+0xd8` the bitmap, `+0xfc`
/// 16.0 on every decal. The order is Halo 2's (`tags::defs` `deca`).
pub const DECA_SIZE: usize = 0x10c;

#[derive(Clone, Debug, PartialEq)]
pub struct Decal {
    pub flags: u16,
    pub kind: u16,
    pub layer: u16,
    pub next: Option<TagId>,
    pub radius: [f32; 2],
    pub colour_lower: [f32; 3],
    pub colour_upper: [f32; 3],
    pub lifetime: [f32; 2],
    pub decay_time: [f32; 2],
    pub bitmap: Option<TagId>,
    pub maximum_sprite_extent: f32,
}

impl Decal {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let b = tag_bytes(map, tag, b"deca", DECA_SIZE)?;
        Ok(Self {
            flags: u16_at(b, 0),
            kind: u16_at(b, 2),
            layer: u16_at(b, 4),
            next: reference(map, b, 8),
            radius: bounds(b, 0x18),
            colour_lower: vec3(b, 0x34),
            colour_upper: vec3(b, 0x40),
            lifetime: bounds(b, 0x78),
            decay_time: bounds(b, 0x80),
            bitmap: reference(map, b, 0xd8),
            maximum_sprite_extent: f32_at(b, 0xfc),
        })
    }
}

/// `udlg`: `0x1010` bytes, a fixed table of `snd!` references, one per
/// line a character can say, `0x10` apart from `+0x10`. V: all 102
/// distinct dialogue tags on the disc are `0x1010` bytes and hold only
/// `snd!` references; the slot names below are the last component of
/// the sound names the tags put there (`tests/effects_disc.rs`
/// `halo1_dialogue_survey`), so each offset is one line whatever the
/// character (`0x0f0` holds `deathquiet` on 68 tags and `death_quiet`
/// on 17).
pub const UDLG_SIZE: usize = 0x1010;

/// A Halo 1 dialogue tag: its sounds by slot offset ([`UDLG_SIZE`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Dialogue {
    /// The slots that name a sound, by offset.
    pub lines: Vec<(usize, TagId)>,
}

impl Dialogue {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let b = tag_bytes(map, tag, b"udlg", UDLG_SIZE)?;
        let snd = Group::new(b"snd!");
        let lines = (0x10..UDLG_SIZE)
            .step_by(0x10)
            .filter(|at| TagRef::read(b, *at).is_some_and(|r| r.group == snd))
            .filter_map(|at| reference(map, b, at).map(|t| (at, t)))
            .collect();
        Ok(Self { lines })
    }

    /// The sound in slot `at`.
    pub fn line(&self, at: usize) -> Option<TagId> {
        self.lines.iter().find(|l| l.0 == at).map(|l| l.1)
    }
}

/// `foot`: material effects. `+0` a block of kinds (`0x1c` each), each
/// with `+0` a block of materials (`0x30` each: `+0` an `effe`, `+0x10`
/// a `snd!`), one per Halo 1 material type in
/// [`crate::arsenal::MATERIALS`] order. V (`halo1_foot_survey`): every
/// unit's holds six kinds of 32 materials, the kinds named by their
/// sounds (0 `walk`, 1 `run`, 2 none, 3 `shuffle`, 4 `jump_up` /
/// `genericjump`, 5 `jump`, the landing), the water material (28) the
/// one with `footsteps water`.
pub const FOOT_KIND_SIZE: usize = 0x1c;
pub const FOOT_MATERIAL_SIZE: usize = 0x30;

/// One material's effect and sound in a [`Foot`] kind.
#[derive(Clone, Debug, PartialEq)]
pub struct FootMaterial {
    pub effect: Option<TagId>,
    pub sound: Option<TagId>,
}

/// A Halo 1 `foot`: per kind, per material.
#[derive(Clone, Debug, PartialEq)]
pub struct Foot {
    pub kinds: Vec<Vec<FootMaterial>>,
}

impl Foot {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let b = tag_bytes(map, tag, b"foot", 0xc)?;
        let kinds = elements(map, b, 0, FOOT_KIND_SIZE)?
            .into_iter()
            .map(|k| {
                Ok(elements(map, k, 0, FOOT_MATERIAL_SIZE)?
                    .into_iter()
                    .map(|m| FootMaterial {
                        effect: reference(map, m, 0),
                        sound: reference(map, m, 0x10),
                    })
                    .collect())
            })
            .collect::<Result<_>>()?;
        Ok(Self { kinds })
    }
}
