//! Halo 1 Xbox object tags, as stored in a cache file.
//!
//! Each reader takes the tag's bytes (its root and, after it, its own
//! blocks) through [`h1map::H1Map`] and keeps the fields the conversion
//! carries, plus every reference. Offsets and evidence are in
//! `docs/formats/h1-objects.md`.

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
fn name32(b: &[u8], at: usize) -> String {
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

fn elements<'a>(map: &'a H1Map, b: &[u8], at: usize, stride: usize) -> Result<Vec<&'a [u8]>> {
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
#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    /// `0x00`: index into [`OBJECT_TYPES`].
    pub object_type: u16,
    pub flags: u16,
    pub bounding_radius: f32,
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
            bounding_offset: vec3(b, 8),
            model: reference(map, b, 0x28),
            animation: reference(map, b, 0x38),
            collision: reference(map, b, 0x70),
            physics: reference(map, b, 0x80),
            modifier_shader: reference(map, b, 0x90),
            creation_effect: reference(map, b, 0xa0),
            attachments: refs(0x140, 0x48)?,
            widgets: refs(0x14c, 0x20)?,
        })
    }
}

/// A weapon's magazine (`0x70`).
#[derive(Clone, Debug, PartialEq)]
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
#[derive(Clone, Debug, PartialEq)]
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
    /// `+0x108` firing effects (`0x84`): each holds references only read
    /// here.
    pub firing_effects: Vec<Vec<Option<TagId>>>,
}

/// `weap`.
#[derive(Clone, Debug, PartialEq)]
pub struct Weapon {
    pub object: Object,
    pub flags: u32,
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
    /// `0x3e4` autoaim angle, `0x3e8` autoaim range, `0x3ec` magnetism
    /// angle, `0x3f0` magnetism range, `0x3f4` deviation angle.
    pub autoaim_angle: f32,
    pub autoaim_range: f32,
    pub magnetism_angle: f32,
    pub magnetism_range: f32,
    pub deviation_angle: f32,
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
                    firing_effects: elements(map, t, 0x108, 0x84)?
                        .into_iter()
                        .map(|e| {
                            crate::refs::scan(map, e)
                                .into_iter()
                                .map(|f| f.tag)
                                .collect()
                        })
                        .collect(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            object: Object::read(map, b)?,
            flags: u32_at(b, 0x308),
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
            autoaim_angle: f32_at(b, 0x3e4),
            autoaim_range: f32_at(b, 0x3e8),
            magnetism_angle: f32_at(b, 0x3ec),
            magnetism_range: f32_at(b, 0x3f0),
            deviation_angle: f32_at(b, 0x3f4),
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
    /// `0x04` the label scripts name the seat by (`W-driver`, `CT-driver`),
    /// 32 characters.
    pub label: String,
    /// `0x24` the model marker the rider sits at (`driver`, `passenger`).
    pub marker: String,
    /// `0x84` the camera marker (`camera`, `camera rider`), empty for none.
    pub camera_marker: String,
}

/// `bipd` or `vehi`: the object header, the seats and every reference in
/// the root; the rest of their tuning has no reviewed Halo 2 home yet
/// (see the doc).
#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    pub group: Group,
    pub object: Object,
    pub seats: Vec<Seat>,
    /// Every reference in the tag's bytes (root and its blocks).
    pub references: Vec<crate::refs::Found>,
}

impl Unit {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let g = &tag.group.0.to_be_bytes();
        if g != b"bipd" && g != b"vehi" {
            return Err(Error::WrongGroup(tag.group.to_string(), *b"unit"));
        }
        let b = map.tag_data(tag)?;
        if b.len() < UNIT_SEATS + 12 {
            return Err(Error::Truncated("unit seats"));
        }
        let seats = elements(map, b, UNIT_SEATS, SEAT_SIZE)?
            .into_iter()
            .map(|e| Seat {
                flags: u32_at(e, 0),
                label: name32(e, 4),
                marker: name32(e, 0x24),
                camera_marker: name32(e, 0x84),
            })
            .collect();
        Ok(Self {
            group: tag.group,
            object: Object::read(map, b)?,
            seats,
            references: crate::refs::scan(map, b),
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
