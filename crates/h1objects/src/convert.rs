//! Halo 1 objects as Halo 2 tags, built on retail Halo 2 templates.
//!
//! Each object starts from the root of the retail Halo 2 tag that plays
//! the same role ([`TEMPLATES`]). The fields known in both games are
//! overwritten with Halo 1's values ([`Tuning`] rows), the model chain is
//! replaced with converted tags, and the rest stays the template's: its
//! effects, sounds, HUD, first-person model and physics model are the
//! stopgaps listed in each [`ObjectReport`]. Template child blocks that are
//! not rewritten keep their retail addresses, so a [`Tree`] is installed
//! into a `TagSet` built from the same base map (`TagSet::from_map`), which
//! pins them in place. Halo 2 offsets and evidence:
//! `docs/formats/h1-objects.md`.

use std::collections::HashMap;

use h1map::{H1Map, Tag};
use h1models::Converted;
use h1models::mode::ModelOptions;
use map::build::{Block, FixupKind, RawId, RawRef, TagEntry, TagPayload, TagSet};
use map::{FourCC, TagId};

use crate::base::Base;
use crate::coll::{convert_collision, put_sid};
use crate::h1::{self, Equipment, Projectile, Unit, Weapon};
use crate::health_hud;
use crate::{Error, Result};

/// Retail Halo 2 template per Halo 1 object: Halo 1 group, words that must
/// all be in the Halo 1 tag path, Halo 2 tag. First match wins.
pub const TEMPLATES: &[(&[u8; 4], &[&str], &str)] = &[
    (
        b"weap",
        &["assault rifle"],
        "objects\\weapons\\rifle\\smg\\smg",
    ),
    (
        b"weap",
        &["plasma pistol"],
        "objects\\weapons\\pistol\\plasma_pistol\\plasma_pistol",
    ),
    (
        b"weap",
        &["plasma rifle"],
        "objects\\weapons\\rifle\\plasma_rifle\\plasma_rifle",
    ),
    (
        b"weap",
        &["plasma_cannon"],
        "objects\\weapons\\support_high\\rocket_launcher\\rocket_launcher",
    ),
    (
        b"weap",
        &["pistol"],
        "objects\\weapons\\pistol\\magnum\\magnum",
    ),
    (
        b"weap",
        &["needler"],
        "objects\\weapons\\pistol\\needler\\needler",
    ),
    (
        b"weap",
        &["sniper"],
        "objects\\weapons\\rifle\\sniper_rifle\\sniper_rifle",
    ),
    (
        b"weap",
        &["rocket"],
        "objects\\weapons\\support_high\\rocket_launcher\\rocket_launcher",
    ),
    (
        b"weap",
        &["shotgun"],
        "objects\\weapons\\rifle\\shotgun\\shotgun",
    ),
    (
        b"weap",
        &["flamethrower"],
        "objects\\weapons\\rifle\\shotgun\\shotgun",
    ),
    (
        b"weap",
        &["flag"],
        "objects\\weapons\\multiplayer\\flag\\flag",
    ),
    (
        b"weap",
        &["ball"],
        "objects\\weapons\\multiplayer\\ball\\ball",
    ),
    (
        b"eqip",
        &["health"],
        "objects\\powerups\\over_shield\\over_shield",
    ),
    (
        b"eqip",
        &["frag grenade"],
        "objects\\weapons\\grenade\\frag_grenade\\frag_grenade",
    ),
    (
        b"eqip",
        &["plasma grenade"],
        "objects\\weapons\\grenade\\plasma_grenade\\plasma_grenade",
    ),
    (
        b"eqip",
        &["shield"],
        "objects\\powerups\\over_shield\\over_shield",
    ),
    (
        b"eqip",
        &["camo"],
        "objects\\powerups\\active_camouflage\\active_camouflage",
    ),
    (b"eqip", &["ammo"], "objects\\powerups\\smg_ammo\\smg_ammo"),
    (
        b"proj",
        &["frag grenade"],
        "objects\\weapons\\grenade\\frag_grenade\\frag_grenade",
    ),
    (
        b"proj",
        &["plasma grenade"],
        "objects\\weapons\\grenade\\plasma_grenade\\plasma_grenade",
    ),
    (
        b"bipd",
        &["cyborg"],
        "objects\\characters\\masterchief\\masterchief_mp",
    ),
    // Placed bipeds (bodies, cinematic characters): the nearest biped a
    // campaign base carries (05a_deltaapproach has all of these). Humans
    // take the Chief: the template's physics model is borrowed, and the
    // marine's reaches nodes past a Halo 1 human's 20.
    (b"bipd", &["elite"], "objects\\characters\\elite\\elite"),
    (b"bipd", &["grunt"], "objects\\characters\\grunt\\grunt"),
    (b"bipd", &["jackal"], "objects\\characters\\jackal\\jackal"),
    (
        b"bipd",
        &["engineer"],
        "objects\\characters\\bugger\\bugger",
    ),
    (
        b"bipd",
        &[],
        "objects\\characters\\masterchief\\masterchief_mp",
    ),
    // Campaign bases have no multiplayer biped or powerups: the
    // campaign's own biped, and a grenade for the health pack (its powerup
    // type and time are Halo 1's either way).
    (
        b"bipd",
        &[],
        "objects\\characters\\masterchief\\masterchief",
    ),
    (
        b"eqip",
        &["health"],
        "objects\\weapons\\grenade\\frag_grenade\\frag_grenade",
    ),
    // Scenery: a static rock of Delta Halo's with collision and no physics
    // model, so a converted model never inherits physics built for other
    // nodes.
    (
        b"scen",
        &[],
        "scenarios\\objects\\solo\\deltatemple\\statue_shard_large\\statue_shard_large",
    ),
];

/// Every template row that matches a Halo 1 tag of `group` at `path`, in
/// table order: the converter takes the first the base map holds.
pub fn template_candidates(group: &[u8; 4], path: &str) -> Vec<&'static str> {
    let path = path.to_ascii_lowercase();
    TEMPLATES
        .iter()
        .filter(|(g, words, _)| *g == group && words.iter().all(|w| path.contains(w)))
        .map(|t| t.2)
        .collect()
}

/// The template for a Halo 1 tag of `group` at `path`.
pub fn template_for(group: &[u8; 4], path: &str) -> Option<&'static str> {
    let path = path.to_ascii_lowercase();
    TEMPLATES
        .iter()
        .find(|(g, words, _)| *g == group && words.iter().all(|w| path.contains(w)))
        .map(|t| t.2)
}

/// Halo 2 offsets this crate writes (S unless the doc says E).
pub mod h2 {
    /// `obje`.
    pub const BOUNDING_RADIUS: usize = 0x04;
    pub const BOUNDING_OFFSET: usize = 0x08;
    pub const DEFAULT_VARIANT: usize = 0x30;
    pub const MODEL: usize = 0x34;
    /// Attachments: `0x18` each, an 8-byte reference first (E,
    /// `docs/formats/objects.md`).
    pub const ATTACHMENTS: usize = 0x94;
    pub const ATTACHMENT: usize = 0x18;
    /// `weap` root.
    pub const HEAT_RECOVERY_THRESHOLD: usize = 0x150;
    pub const OVERHEATED_THRESHOLD: usize = 0x154;
    pub const HEAT_DETONATION_THRESHOLD: usize = 0x158;
    pub const HEAT_DETONATION_FRACTION: usize = 0x15c;
    pub const HEAT_LOSS_PER_SECOND: usize = 0x160;
    pub const HEAT_ILLUMINATION: usize = 0x164;
    pub const AUTOAIM_ANGLE: usize = 0x208;
    pub const AUTOAIM_RANGE: usize = 0x20c;
    pub const MAGNETISM_ANGLE: usize = 0x210;
    pub const MAGNETISM_RANGE: usize = 0x214;
    pub const DEVIATION_ANGLE: usize = 0x218;
    pub const MAGAZINES: usize = 0x2c0;
    pub const BARRELS: usize = 0x2d0;
    pub const MAGAZINE_SIZE: usize = 0x5c;
    pub const BARREL_SIZE: usize = 0xec;
    /// Magazine element.
    pub const MAG_ROUNDS_RECHARGED: usize = 0x04;
    pub const MAG_ROUNDS_TOTAL_INITIAL: usize = 0x06;
    pub const MAG_ROUNDS_TOTAL_MAXIMUM: usize = 0x08;
    pub const MAG_ROUNDS_LOADED_MAXIMUM: usize = 0x0a;
    pub const MAG_RELOAD_TIME: usize = 0x10;
    pub const MAG_ROUNDS_RELOADED: usize = 0x14;
    /// Barrel element.
    pub const BARREL_ROUNDS_PER_SECOND: usize = 0x04;
    pub const BARREL_ACCELERATION_TIME: usize = 0x0c;
    pub const BARREL_DECELERATION_TIME: usize = 0x10;
    pub const BARREL_MAGAZINE: usize = 0x28;
    pub const BARREL_ROUNDS_PER_SHOT: usize = 0x2a;
    pub const BARREL_MINIMUM_ROUNDS_LOADED: usize = 0x2c;
    pub const BARREL_ROUNDS_BETWEEN_TRACERS: usize = 0x2e;
    pub const BARREL_ERROR_ACCELERATION_TIME: usize = 0x38;
    pub const BARREL_ERROR_DECELERATION_TIME: usize = 0x3c;
    pub const BARREL_DISTRIBUTION_FUNCTION: usize = 0x68;
    pub const BARREL_PROJECTILES_PER_SHOT: usize = 0x6a;
    pub const BARREL_DISTRIBUTION_ANGLE: usize = 0x6c;
    pub const BARREL_MINIMUM_ERROR: usize = 0x70;
    pub const BARREL_ERROR_ANGLE: usize = 0x74;
    pub const BARREL_FIRST_PERSON_OFFSET: usize = 0x7c;
    pub const BARREL_PROJECTILE: usize = 0x8c;
    pub const BARREL_HEAT_PER_ROUND: usize = 0xa4;
    /// `proj` root.
    pub const PROJ_DETONATION_TIMER_STARTS: usize = 0xc0;
    pub const PROJ_IMPACT_NOISE: usize = 0xc2;
    pub const PROJ_AI_PERCEPTION_RADIUS: usize = 0xc4;
    pub const PROJ_COLLISION_RADIUS: usize = 0xc8;
    pub const PROJ_ARMING_TIME: usize = 0xcc;
    pub const PROJ_DANGER_RADIUS: usize = 0xd0;
    pub const PROJ_TIMER: usize = 0xd4;
    pub const PROJ_MINIMUM_VELOCITY: usize = 0xdc;
    pub const PROJ_MAXIMUM_RANGE: usize = 0xe0;
    pub const PROJ_DETONATION_NOISE: usize = 0xe4;
    pub const PROJ_AIR_GRAVITY_SCALE: usize = 0x164;
    pub const PROJ_AIR_DAMAGE_RANGE: usize = 0x168;
    pub const PROJ_WATER_GRAVITY_SCALE: usize = 0x170;
    pub const PROJ_WATER_DAMAGE_RANGE: usize = 0x174;
    pub const PROJ_INITIAL_VELOCITY: usize = 0x17c;
    pub const PROJ_FINAL_VELOCITY: usize = 0x180;
    pub const PROJ_GUIDED_ANGULAR_VELOCITY: usize = 0x184;
    /// `eqip` root (docs/game/health.md).
    pub const EQIP_POWERUP_TYPE: usize = 0x12c;
    pub const EQIP_POWERUP_TIME: usize = 0x130;
    /// `hlmt` root.
    pub const HLMT_RENDER: usize = 0x00;
    pub const HLMT_COLLISION: usize = 0x08;
    pub const HLMT_ANIMATION: usize = 0x10;
    pub const HLMT_PHYSICS_MODEL: usize = 0x20;
    /// `phmo`: rigid bodies (`0x90` each, node `i16` at `+0`).
    pub const PHMO_RIGID_BODIES: usize = 0x38;
    pub const HLMT_VARIANTS: usize = 0x50;
    pub const HLMT_REGIONS: usize = 0x70;
    /// `hlmt` region (`0x10`): `i8` collision and physics region (-1
    /// none); permutation (`8`): `i8` collision and physics permutation.
    /// D: retail regions of objects with no `coll` hold -1 in the
    /// collision byte; the physics byte is never -1 without a `phmo`.
    pub const HLMT_REGION_COLLISION: usize = 4;
    pub const HLMT_REGION_PHYSICS: usize = 5;
    pub const HLMT_PERMUTATION_COLLISION: usize = 5;
    pub const HLMT_PERMUTATION_PHYSICS: usize = 6;
    pub const HLMT_NODES: usize = 0x78;
}

/// For each render region, the collision region it selects and, per
/// render permutation, the collision permutation (-1 none). Regions and
/// permutations match by name (string ID form); a render region whose
/// name the `coll` lacks takes the `coll` region at the same index, and a
/// permutation likewise, then the region's first, so a renamed region
/// still collides.
pub fn region_map(
    render: &[(String, Vec<String>)],
    coll: &[(String, Vec<String>)],
) -> Vec<(i8, Vec<i8>)> {
    let find = |names: &[String], want: &str| {
        let want = crate::coll::sid_name(want);
        names.iter().position(|n| crate::coll::sid_name(n) == want)
    };
    let coll_regions: Vec<String> = coll.iter().map(|c| c.0.clone()).collect();
    render
        .iter()
        .enumerate()
        .map(|(i, (name, perms))| {
            let r = find(&coll_regions, name).or((i < coll.len()).then_some(i));
            let Some(r) = r else {
                return (-1, vec![-1; perms.len()]);
            };
            let cp = &coll[r].1;
            let perms = perms
                .iter()
                .enumerate()
                .map(|(j, p)| {
                    find(cp, p)
                        .or((j < cp.len()).then_some(j))
                        .or((!cp.is_empty()).then_some(0))
                        .map_or(-1, |k| k as i8)
                })
                .collect();
            (r as i8, perms)
        })
        .collect()
}

/// Halo 1's cache keeps speeds per tick; Halo 2's tags hold them per
/// second (S, docs/formats/h1-objects.md).
pub const TICKS_PER_SECOND: f32 = 30.0;

/// Halo 1's health powerup type, which Halo 2 handles the same way
/// (docs/game/health.md).
pub const POWERUP_HEALTH: i16 = 5;

/// One value carried from Halo 1 into the Halo 2 tag.
#[derive(Clone, Debug, PartialEq)]
pub struct Tuning {
    pub field: String,
    pub halo1: String,
    pub halo2: String,
    /// The template's value that was overwritten.
    pub template: String,
}

/// What one object's conversion did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjectReport {
    /// Halo 1 group and path.
    pub group: String,
    pub source: String,
    /// The retail Halo 2 tag it was built on.
    pub template: String,
    pub tuning: Vec<Tuning>,
    /// Template references kept as stopgaps: (offset in the root, group,
    /// retail name).
    pub stopgaps: Vec<(usize, String, String)>,
    /// Halo 1 data with no Halo 2 home here.
    pub no_home: Vec<String>,
    /// Where the physics model came from.
    pub physics: Option<String>,
}

/// Converted tags in install order, with placeholders for each other.
#[derive(Default)]
pub struct Tree {
    pub tags: Vec<(String, Converted)>,
    pub reports: Vec<ObjectReport>,
    /// The HUD each owner (the player biped) can switch between at game
    /// start (`boot::hud`; docs/game/hud.md).
    pub hud_swaps: Vec<HudSwap>,
}

/// One owner's two HUDs: Halo 1's ([`crate::h1hud`]), which the owner
/// names as built, and Halo 2's with the health strip
/// ([`crate::health_hud`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HudSwap {
    /// The `bipd` or `weap` whose HUD reference switches.
    pub owner: TagId,
    pub halo1: TagId,
    pub halo2: TagId,
}

const LOCAL: u32 = 0xfffd_0000;

/// The stand-in physics model of a vehicle with no solid collision
/// ([`crate::rebuild::sphere_physics`]): radius (world units), mass (kg),
/// material. The sphere sits on the model origin (its centre one radius
/// up), so a vehicle whose origin is on the floor stays put; a
/// `hard_metal_thin_hum` global material (on the disc).
pub const STAND_IN_SPHERE: (f32, f32, &str) = (0.05, 1000.0, "hard_metal_thin_hum");

/// Mass per cubic world unit of a built physics model with no Halo 1
/// mass: water's 1000 kg/m^3 over a world unit of 3.048 m.
pub const DENSITY: f32 = 28_317.0;

/// What a built physics model is for ([`Converter::object_physics`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicsKind {
    /// One body on the object, of Halo 1's `phys` mass; a stand-in
    /// sphere when the collision gives nothing.
    Vehicle,
    /// One body per collision node, riding that node.
    Device,
    /// As a device, with a biped's rigid body words
    /// ([`crate::rebuild::BIPED_BODY`]); no stand-in.
    Biped,
}

/// Stands for the `k`-th tag of a [`Tree`] until it is installed.
pub fn local(k: usize) -> TagId {
    TagId(LOCAL | k as u32)
}

/// The tree index `t` names, when it is one of the tree's own tags
/// ([`local`]) rather than a base-map tag.
pub fn local_index(t: TagId) -> Option<usize> {
    (t.0 & 0xffff_0000 == LOCAL).then_some((t.0 & 0xffff) as usize)
}

fn rewrite(b: &mut Block, ids: &[TagId]) {
    for f in &mut b.fixups {
        match &mut f.kind {
            FixupKind::Block { child, .. } | FixupKind::Data(child) | FixupKind::Pointer(child) => {
                rewrite(child, ids)
            }
            FixupKind::TagId(t) | FixupKind::TagRef(t)
                if t.0 & 0xffff_0000 == LOCAL && (t.0 as usize & 0xffff) < ids.len() =>
            {
                *t = ids[t.0 as usize & 0xffff]
            }
            _ => {}
        }
    }
}

impl Tree {
    /// Adds `tag` as `name`, or `name_2`, `name_3`, ... when a tag of its
    /// group already has that name: a map's tags are found by group and
    /// name, so two would leave one unreachable. Halo 1 objects of two
    /// groups can share a path (a10's `cryotube` is a `mach` and a `vehi`),
    /// and each gets its own `hlmt`.
    fn push(&mut self, name: &str, tag: Converted) -> TagId {
        let taken = |n: &str| {
            self.tags
                .iter()
                .any(|(m, t)| m == n && t.group == tag.group)
        };
        let mut unique = name.to_owned();
        let mut k = 2;
        while taken(&unique) {
            unique = format!("{name}_{k}");
            k += 1;
        }
        self.tags.push((unique, tag));
        local(self.tags.len() - 1)
    }

    /// Adds every tag to `set` (built from the base map the tree was made
    /// on), in order, and returns their datums.
    pub fn install(self, set: &mut TagSet) -> Result<Vec<TagId>> {
        let first = set.slots.len();
        let ids: Vec<TagId> = (0..self.tags.len())
            .map(|k| {
                let slot = first + k;
                TagId(u32::from(set.salt_base.wrapping_add(slot as u16)) << 16 | slot as u32)
            })
            .collect();
        for (k, (name, mut tag)) in self.tags.into_iter().enumerate() {
            rewrite(&mut tag.root, &ids);
            let got = tag.install(set, &name)?;
            if got != ids[k] {
                return Err(Error::Invalid(format!(
                    "{name}: installed as {got:?}, expected {:?}",
                    ids[k]
                )));
            }
        }
        Ok(ids)
    }
}

impl Tree {
    /// Puts every tag into an existing slot of `set` instead of adding
    /// slots: `donors[k]` takes tag `k` (its datum is kept; its group, name
    /// and payload are replaced). A pinned donor's old bytes stay where they
    /// were, as unowned metadata, so blocks other tags share with it remain
    /// valid. This is the form a pinned base needs: `TagSet::build` refuses
    /// to grow a pinned map's tag index. Anything else that referenced a
    /// donor now references the converted tag, so donors must be tags the
    /// converted map no longer uses.
    pub fn install_over(self, set: &mut TagSet, donors: &[TagId]) -> Result<Vec<TagId>> {
        if donors.len() < self.tags.len() {
            return Err(Error::Invalid(format!(
                "{} tags but {} donor slots",
                self.tags.len(),
                donors.len()
            )));
        }
        let ids = &donors[..self.tags.len()];
        for (k, (name, tag)) in self.tags.into_iter().enumerate() {
            let id = ids[k];
            let slot = id.0 as u16 as usize;
            let raw: Vec<RawId> = tag
                .raw
                .into_iter()
                .map(|(b, a)| set.add_raw(b, a))
                .collect();
            let mut root = tag.root;
            rewrite(&mut root, ids);
            finish(&mut root, &raw, id);
            set.add_group(tag.group, None, None);
            let s = set
                .slots
                .get_mut(slot)
                .ok_or_else(|| Error::Invalid(format!("no slot {slot}")))?;
            let entry = s
                .tag
                .as_mut()
                .filter(|e| e.id == id)
                .ok_or_else(|| Error::Invalid(format!("slot {slot} is not {id:?}")))?;
            let old = std::mem::replace(
                entry,
                TagEntry {
                    id,
                    group: tag.group,
                    payload: TagPayload::Built(root),
                },
            );
            if let TagPayload::Pinned { address, bytes } = old.payload
                && let Some(p) = set.pinned.as_mut()
            {
                p.unowned.push((address, bytes));
            }
            s.name = Some(name);
        }
        Ok(ids.to_vec())
    }
}

/// Points a tree's local raw fixups at placed raws and fills in its own
/// datum (what `Converted::install` does for a new slot).
fn finish(b: &mut Block, raw: &[RawId], id: TagId) {
    for f in &mut b.fixups {
        match &mut f.kind {
            FixupKind::Block { child, .. } | FixupKind::Data(child) | FixupKind::Pointer(child) => {
                finish(child, raw, id)
            }
            FixupKind::Raw(RawRef::Local(r)) => *r = raw[r.0],
            FixupKind::TagId(t) | FixupKind::TagRef(t) if *t == h1models::SELF_ID => *t = id,
            _ => {}
        }
    }
}

/// A powered mass point: its Halo 2 point, model node and position in
/// that node's space.
struct PoweredPoint {
    point: crate::rebuild::Point,
    node: i16,
    position: [f32; 3],
}

/// Adds a marker per powered point to a Halo 1 model before conversion:
/// named as the point, on its node, one instance per permutation of the
/// first region (D: so whichever permutation the object shows carries
/// it), no rotation.
fn add_point_markers(model: &mut h1models::h1::Model, points: &[PoweredPoint]) {
    let perms = model.regions.first().map_or(1, |r| r.permutations.len().max(1));
    for p in points {
        if model.markers.iter().any(|m| m.name == p.point.marker) {
            continue;
        }
        model.markers.push(h1models::h1::Marker {
            name: p.point.marker.clone(),
            instances: (0..perms)
                .map(|k| h1models::h1::MarkerInstance {
                    region: 0,
                    permutation: k as i8,
                    node: p.node as i8,
                    translation: p.position,
                    rotation: [0.0, 0.0, 0.0, 1.0],
                })
                .collect(),
        });
    }
}

fn put_f32(b: &mut [u8], at: usize, v: f32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_i16(b: &mut [u8], at: usize, v: i16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn get_f32(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn get_i16(b: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([b[at], b[at + 1]])
}

/// Writes Halo 1 values into a Halo 2 record, recording each one.
struct Writer<'r> {
    rep: &'r mut ObjectReport,
    prefix: String,
}
impl Writer<'_> {
    fn f32(&mut self, b: &mut [u8], at: usize, field: &str, h1: f32, h2: f32) {
        let old = get_f32(b, at);
        put_f32(b, at, h2);
        self.row(field, format!("{h1}"), format!("{h2}"), format!("{old}"));
    }
    fn i16(&mut self, b: &mut [u8], at: usize, field: &str, v: i16) {
        let old = get_i16(b, at);
        put_i16(b, at, v);
        self.row(field, v.to_string(), v.to_string(), old.to_string());
    }
    fn row(&mut self, field: &str, halo1: String, halo2: String, template: String) {
        self.rep.tuning.push(Tuning {
            field: format!("{}{field}", self.prefix),
            halo1,
            halo2,
            template,
        });
    }
}

/// How the [`Converter`] builds tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Build scenery from typed definitions ([`crate::rebuild`]): the
    /// `scen`, its `hlmt` and its `mode`'s material list written field by
    /// field with no retail template bytes. Off by default until the
    /// rebuilt scenery has run in the game.
    pub rebuild: bool,
    /// Build bipeds from the `bipd` definition ([`crate::rebuild::biped`])
    /// instead of on a template: Halo 1's model, collision size and
    /// physics, the role's words, and the references of the Halo 2
    /// counterpart ([`crate::rebuild::COUNTERPARTS`]). On by default; off
    /// builds them on templates.
    pub rebuild_bipeds: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            rebuild: false,
            rebuild_bipeds: true,
        }
    }
}

/// Builds a [`Tree`] from Halo 1 objects on one base map.
pub struct Converter<'a> {
    pub h1: &'a H1Map,
    pub base: &'a Base,
    pub options: Options,
    pub tree: Tree,
    /// Every converted `coll` and what it holds, in build order.
    pub coll_reports: Vec<(String, crate::coll::CollisionReport)>,
    done: HashMap<u32, TagId>,
    colls: HashMap<(u32, bool), TagId>,
    coll_names: HashMap<(u32, bool), Vec<(String, Vec<String>)>>,
    /// Each template `nhdt` given the health meter.
    health_huds: HashMap<TagId, TagId>,
    /// Widgets per converted `unhi` ([`Self::unit_hud`]).
    unit_hud_widgets: HashMap<u32, usize>,
    /// Biped role words, per role ([`Self::biped_role`]).
    roles: HashMap<crate::rebuild::BipedRole, Option<crate::rebuild::RoleWords>>,
    /// Vehicle role words, per vehicle type ([`Self::vehicle_role`]).
    vehicle_roles: HashMap<u16, Option<crate::rebuild::VehicleRole>>,
}

impl<'a> Converter<'a> {
    pub fn new(h1: &'a H1Map, base: &'a Base) -> Self {
        Self {
            h1,
            base,
            options: Options::default(),
            tree: Tree::default(),
            coll_reports: Vec::new(),
            done: HashMap::new(),
            colls: HashMap::new(),
            coll_names: HashMap::new(),
            health_huds: HashMap::new(),
            unit_hud_widgets: HashMap::new(),
            roles: HashMap::new(),
            vehicle_roles: HashMap::new(),
        }
    }

    fn template(&self, group: &[u8; 4], path: &str, h2_group: &[u8; 4]) -> Result<(TagId, String)> {
        let names = template_candidates(group, path);
        let Some(&first) = names.first() else {
            return Err(Error::NoTemplate(format!(
                "{} {path}",
                String::from_utf8_lossy(group)
            )));
        };
        names
            .iter()
            .find_map(|n| Some((self.base.find(h2_group, n)?, (*n).to_owned())))
            .ok_or_else(|| Error::NoTemplate(format!("{first} is not in the base map")))
    }

    /// Converts any supported object tag (once; later calls return the
    /// same tag).
    pub fn object(&mut self, tag: &Tag) -> Result<TagId> {
        self.object_with(tag, None)
    }

    fn object_with(&mut self, tag: &Tag, template: Option<TagId>) -> Result<TagId> {
        if let Some(id) = self.done.get(&tag.id.0) {
            return Ok(*id);
        }
        let id = match &tag.group.0.to_be_bytes() {
            b"weap" => self.weapon(tag)?,
            b"proj" => self.projectile(tag, template)?,
            b"eqip" => self.equipment(tag)?,
            b"bipd" | b"vehi" => self.unit(tag)?,
            b"scen" => self.scenery(tag)?,
            b"mach" | b"ctrl" | b"lifi" => self.device(tag)?,
            _ => return Err(Error::WrongGroup(tag.group.to_string(), *b"obje")),
        };
        self.done.insert(tag.id.0, id);
        Ok(id)
    }

    fn h1_tag(&self, id: h1map::TagId) -> Result<&'a Tag> {
        Ok(self.h1.tag(id)?)
    }

    /// The shared header: bounds, default variant, and the model chain.
    fn header(
        &mut self,
        rep: &mut ObjectReport,
        root: &mut Block,
        obj: &h1::Object,
        name: &str,
    ) -> Result<()> {
        let mut w = Writer {
            rep,
            prefix: "obje.".into(),
        };
        w.f32(
            &mut root.bytes,
            h2::BOUNDING_RADIUS,
            "bounding_radius",
            obj.bounding_radius,
            obj.bounding_radius,
        );
        for i in 0..3 {
            let v = obj.bounding_offset[i];
            w.f32(
                &mut root.bytes,
                h2::BOUNDING_OFFSET + 4 * i,
                &format!("bounding_offset[{i}]"),
                v,
                v,
            );
        }
        // The converted model has no variants; the engine then uses none.
        root.bytes[h2::DEFAULT_VARIANT..h2::DEFAULT_VARIANT + 4].fill(0);
        let template_hlmt = self.base.reference(&root.bytes, h2::MODEL);
        let hlmt = self.model(rep, obj, template_hlmt, name, false)?;
        root.tag_ref(h2::MODEL as u32, hlmt.unwrap_or(TagId::NULL));
        if obj.physics.is_some() {
            rep.no_home.push(
                "phys (Halo 1 vehicle physics): Halo 2 runs its own; the template's phmo is used"
                    .into(),
            );
        }
        Ok(())
    }

    /// The `hlmt` and the converted tags under it. `None` when the Halo 1
    /// object has no render model. `rebuild`: the `hlmt` and the `mode`'s
    /// materials from the definitions ([`crate::rebuild`]), no template.
    fn model(
        &mut self,
        rep: &mut ObjectReport,
        obj: &h1::Object,
        template_hlmt: Option<TagId>,
        name: &str,
        rebuild: bool,
    ) -> Result<Option<TagId>> {
        let Some(mode_id) = obj.model else {
            return Ok(None);
        };
        let scenery = h1::OBJECT_TYPES.get(obj.object_type as usize) == Some(&b"scen");
        let mode_tag = self.h1_tag(mode_id)?;
        let (h1_model, mode) = match self.done.get(&mode_tag.id.0) {
            // Converted already (shared by two objects): only the Halo 1
            // model is needed again.
            Some(_) => h1models::model_from_map(self.h1, mode_tag, &ModelOptions::default())?,
            None => {
                let (mut h1_model, _) =
                    h1models::model_from_map(self.h1, mode_tag, &ModelOptions::default())?;
                // A vehicle's powered mass points become markers, which
                // its friction or hover points name (crate::rebuild::Point).
                if h1::OBJECT_TYPES.get(obj.object_type as usize) == Some(&b"vehi") {
                    let points = self.powered_points(obj, &h1_model)?;
                    add_point_markers(&mut h1_model, &points);
                }
                let shaders = self.model_shaders(rep, &h1_model);
                // One material per Halo 1 shader, as convert_model writes.
                let mut padded = shaders.clone();
                padded.resize(h1_model.shaders.len(), TagId::NULL);
                let opts = ModelOptions { shaders };
                let mut mode = h1models::mode::convert_model(
                    &h1_model,
                    h1models::short_name(&mode_tag.name),
                    &opts,
                )?;
                if rebuild {
                    crate::rebuild::mode_materials(&mut mode.root, &padded)?;
                }
                (h1_model, mode)
            }
        };
        let mode = self.cached(mode_tag, mode)?;
        let mut hlmt = match template_hlmt {
            Some(t) => Block::new(self.base.root(t)?),
            None => Block::zeroed(
                tags::object::GroupDef::find(FourCC(*b"hlmt"))
                    .map_or(0xfc, |d| d.root_size as usize),
            ),
        };
        let (count, nodes) = h1models::mode::hlmt_nodes(&h1_model);
        let mut has_phmo = false;
        rep.physics = Some(
            match self.base.reference(&hlmt.bytes, h2::HLMT_PHYSICS_MODEL) {
                Some(p) => {
                    // The physics model writes one matrix per rigid body's node;
                    // the object holds the hlmt's count. A template whose rigid
                    // bodies reach past the converted nodes would write past
                    // them, so it is not borrowed.
                    let top = self.phmo_top_node(p)?;
                    let name = self.base.name(p).map_or("?", |n| n.1).to_owned();
                    if top >= i64::from(count.max(1)) {
                        hlmt.tag_ref(h2::HLMT_PHYSICS_MODEL as u32, TagId::NULL);
                        format!(
                            "none: the template's {name} reaches node {top}, the model has {count}"
                        )
                    } else {
                        has_phmo = true;
                        format!("borrowed: {name}")
                    }
                }
                None => "none (the template has no phmo)".into(),
            },
        );
        // Havok sees static scenery through the phmo or, with none, through
        // the coll's BSP physics slots (crate::coll). Retail writes slots
        // only for scenery: its weapons, equipment, devices and bipeds
        // without a phmo have none.
        let coll = match obj.collision {
            Some(c) => {
                let t = self.h1_tag(c)?;
                Some(self.collision(rep, t, scenery && !has_phmo)?)
            }
            None => None,
        };
        let jmad = match obj.animation {
            Some(a) => {
                let t = self.h1_tag(a)?;
                match self.done.get(&t.id.0) {
                    Some(id) => Some(*id),
                    None => {
                        let (_, _, conv) = h1models::graph_from_map(self.h1, t, Some(mode_tag))?;
                        Some(self.cached(t, conv)?)
                    }
                }
            }
            None => None,
        };

        if rebuild {
            return self.rebuilt_model(rep, obj, &h1_model, mode, coll, jmad, has_phmo, name);
        }
        hlmt.tag_ref(h2::HLMT_RENDER as u32, mode);
        hlmt.tag_ref(h2::HLMT_COLLISION as u32, coll.map_or(TagId::NULL, |c| c.0));
        hlmt.tag_ref(h2::HLMT_ANIMATION as u32, jmad.unwrap_or(TagId::NULL));
        hlmt.bytes[h2::HLMT_VARIANTS..h2::HLMT_VARIANTS + 8].fill(0);
        // One node per render-model node: the engine sizes an object's node
        // matrices by this count, and the converted model and graph write
        // one per their node (h1models::mode::hlmt_nodes).
        if count == 0 {
            hlmt.bytes[h2::HLMT_NODES..h2::HLMT_NODES + 8].fill(0);
        } else {
            hlmt.block(h2::HLMT_NODES as u32, count, nodes);
        }
        // Regions and their permutations, named as the converted model's,
        // each with the collision and physics region and permutation it
        // uses (-1 none).
        let render: Vec<(String, Vec<String>)> = h1_model
            .regions
            .iter()
            .map(|r| {
                (
                    r.name.clone(),
                    r.permutations.iter().map(|p| p.name.clone()).collect(),
                )
            })
            .collect();
        let coll_names = coll.map_or(&[][..], |c| &self.coll_names[&c.1][..]);
        let map = region_map(&render, coll_names);
        for (i, name) in coll_names.iter().enumerate() {
            if !map.iter().any(|(r, _)| *r == i as i8) {
                rep.no_home.push(format!(
                    "coll region {} has no render region to select it",
                    name.0
                ));
            }
        }
        // Physics bytes stay 0, as before: retail never holds -1 there on
        // an object with no phmo (the scenery template's statue: 0).
        let physics: i8 = 0;
        let mut regions = Block::zeroed(0x10 * h1_model.regions.len());
        for (i, r) in h1_model.regions.iter().enumerate() {
            put_sid(&mut regions, 0x10 * i, &r.name);
            regions.bytes[0x10 * i + h2::HLMT_REGION_COLLISION] = map[i].0 as u8;
            regions.bytes[0x10 * i + h2::HLMT_REGION_PHYSICS] = physics as u8;
            let mut perms = Block::zeroed(8 * r.permutations.len());
            for (j, p) in r.permutations.iter().enumerate() {
                put_sid(&mut perms, 8 * j, &p.name);
                perms.bytes[8 * j + h2::HLMT_PERMUTATION_COLLISION] = map[i].1[j] as u8;
                perms.bytes[8 * j + h2::HLMT_PERMUTATION_PHYSICS] = physics as u8;
            }
            if !r.permutations.is_empty() {
                regions.block((0x10 * i + 8) as u32, r.permutations.len() as u32, perms);
            }
        }
        if h1_model.regions.is_empty() {
            hlmt.bytes[h2::HLMT_REGIONS..h2::HLMT_REGIONS + 8].fill(0);
        } else {
            hlmt.block(
                h2::HLMT_REGIONS as u32,
                h1_model.regions.len() as u32,
                regions,
            );
        }
        if let Some(t) = template_hlmt {
            for (at, g, n) in self.base.references(&self.base.root(t)?) {
                if ![h2::HLMT_RENDER, h2::HLMT_COLLISION, h2::HLMT_ANIMATION].contains(&at) {
                    rep.stopgaps.push((at, format!("hlmt {g}"), n));
                }
            }
        }
        if obj.animation.is_none() {
            rep.no_home
                .push("no Halo 1 animation graph on the object; hlmt animation left null".into());
        }
        Ok(Some(self.tree.push(
            name,
            Converted {
                group: FourCC(*b"hlmt"),
                root: hlmt,
                raw: Vec::new(),
            },
        )))
    }

    /// The `hlmt` from its typed definition ([`crate::rebuild::model`]):
    /// the same model chain, nodes and region map the template path
    /// writes, every other field at its default.
    #[allow(clippy::too_many_arguments)]
    fn rebuilt_model(
        &mut self,
        rep: &mut ObjectReport,
        obj: &h1::Object,
        h1_model: &h1models::h1::Model,
        mode: TagId,
        coll: Option<(TagId, (u32, bool))>,
        jmad: Option<TagId>,
        has_phmo: bool,
        name: &str,
    ) -> Result<Option<TagId>> {
        debug_assert!(!has_phmo, "the rebuilt path borrows no physics model");
        let (count, nodes) = h1models::mode::hlmt_nodes(h1_model);
        let render: Vec<(String, Vec<String>)> = h1_model
            .regions
            .iter()
            .map(|r| {
                (
                    r.name.clone(),
                    r.permutations.iter().map(|p| p.name.clone()).collect(),
                )
            })
            .collect();
        let coll_names = coll.map_or(&[][..], |c| &self.coll_names[&c.1][..]);
        let map = region_map(&render, coll_names);
        let regions = h1_model
            .regions
            .iter()
            .enumerate()
            .map(|(i, r)| crate::rebuild::Region {
                name: r.name.clone(),
                collision: map[i].0,
                physics: 0,
                permutations: r
                    .permutations
                    .iter()
                    .enumerate()
                    .map(|(j, p)| (p.name.clone(), map[i].1[j], 0))
                    .collect(),
            })
            .collect();
        let root = crate::rebuild::model(&crate::rebuild::Model {
            render: mode,
            collision: coll.map(|c| c.0),
            animation: jmad,
            physics: None,
            nodes: (count, nodes),
            regions,
        })?;
        if obj.animation.is_none() {
            rep.no_home
                .push("no Halo 1 animation graph on the object; hlmt animation left null".into());
        }
        Ok(Some(self.tree.push(
            name,
            Converted {
                group: FourCC(*b"hlmt"),
                root,
                raw: Vec::new(),
            },
        )))
    }

    /// Scenery from typed definitions: no template.
    fn scenery_rebuilt(&mut self, tag: &Tag) -> Result<TagId> {
        let object = h1::Object::read(self.h1, self.h1.tag_data(tag)?)?;
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        rep.physics = Some("none (rebuilt scenery borrows no phmo)".into());
        let hlmt = self.model(&mut rep, &object, None, &tag.name, true)?;
        let root = crate::rebuild::scenery(&object, hlmt)?;
        rep.no_home.push(
            "scen beyond the object header's bounds and model: definition defaults (no template)"
                .into(),
        );
        self.tree.reports.push(rep);
        Ok(self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"scen"),
                root,
                raw: Vec::new(),
            },
        ))
    }

    /// The converted `coll` for Halo 1's `t`, with or without BSP physics
    /// slots (each built once; a second form, when two objects need both,
    /// gets a suffixed name). Returns the tag and its key in `coll_names`.
    fn collision(
        &mut self,
        rep: &mut ObjectReport,
        t: &Tag,
        bsp_physics: bool,
    ) -> Result<(TagId, (u32, bool))> {
        let key = (t.id.0, bsp_physics);
        if let Some(id) = self.colls.get(&key) {
            return Ok((*id, key));
        }
        let h1c = h1::Collision::read(self.h1, t)?;
        let (conv, cr) = convert_collision(&h1c, bsp_physics)?;
        rep.no_home.push(format!(
            "coll damage: body vitality {}, shield vitality {}, {} modifiers, {} damage effect references (Halo 2 keeps damage in the hlmt; not written)",
            cr.body_vitality, cr.shield_vitality, cr.modifiers, cr.damage_references
        ));
        let name = if self.colls.contains_key(&(t.id.0, !bsp_physics)) {
            format!(
                "{}{}",
                t.name,
                if bsp_physics { "_bsp_physics" } else { "_phmo" }
            )
        } else {
            t.name.clone()
        };
        let id = self.tree.push(&name, conv);
        self.colls.insert(key, id);
        self.coll_names.insert(
            key,
            h1c.regions
                .iter()
                .map(|r| (r.name.clone(), r.permutations.clone()))
                .collect(),
        );
        self.coll_reports.push((name, cr));
        Ok((id, key))
    }

    /// The highest node a `phmo`'s rigid bodies name (`+0x38`, `0x90`
    /// each, node `i16` first), `-1` with none.
    fn phmo_top_node(&self, phmo: TagId) -> Result<i64> {
        let root = self.base.head(phmo, h2::PHMO_RIGID_BODIES as u64 + 8)?;
        let mut top = -1i64;
        let mut i = 0;
        while let Some(e) = self.base.element(&root, h2::PHMO_RIGID_BODIES, 0x90, i)? {
            top = top.max(i64::from(i16::from_le_bytes([e[0], e[1]])));
            i += 1;
        }
        Ok(top)
    }

    /// A `shad` per Halo 1 shader of `model`: a copy of the base's model
    /// shader ([`Base::model_shader`]) drawing the Halo 1 shader's own
    /// base and detail maps, converted by `h1bitmaps` (each once). A
    /// shader with no base map this reads gets the base's model shader
    /// itself, never null: the shader draw begin `0x1bbf0` does not skip a
    /// NONE shader (`0x1bcb9` only clears its cached words) and the draw
    /// `0x16b90` then reads tag slot `0xffff`'s entry as a `shad`. With no
    /// template at all the materials stay null.
    fn model_shaders(&mut self, rep: &mut ObjectReport, model: &h1models::h1::Model) -> Vec<TagId> {
        let Some(template) = self.base.model_shader.clone() else {
            rep.no_home
                .push("model shaders: the base has no model shader to copy; materials null".into());
            return Vec::new();
        };
        let mut out = Vec::new();
        for s in &model.shaders {
            let id = (|| -> Option<TagId> {
                let tag = self.h1.tag(s.tag).ok()?;
                if let Some(id) = self.done.get(&tag.id.0) {
                    return Some(*id);
                }
                let maps = h1convert::textures::h1_maps_of(self.h1, tag)?;
                let base = self.bitmap(maps.base?)?;
                let detail = maps.detail.and_then(|d| self.bitmap(d));
                let (tree, _) = template.copy(base, detail);
                let conv = Converted {
                    group: FourCC(*b"shad"),
                    root: tree,
                    raw: Vec::new(),
                };
                self.cached(tag, conv).ok()
            })();
            if id.is_none() {
                rep.no_home.push(format!(
                    "model shader {}: no base map read; the base's model shader stands in",
                    self.h1.tag(s.tag).map_or("?".into(), |t| t.name.clone())
                ));
            }
            out.push(id.unwrap_or(template.shader));
        }
        out
    }

    /// The Halo 1 bitmap `id`, converted once.
    fn bitmap(&mut self, id: h1map::TagId) -> Option<TagId> {
        let tag = self.h1.tag(id).ok()?;
        if let Some(t) = self.done.get(&tag.id.0) {
            return Some(*t);
        }
        let (_, conv, _) = h1bitmaps::convert_bitmap(self.h1, tag).ok()?;
        self.cached(tag, conv).ok()
    }

    fn cached(&mut self, t: &Tag, conv: Converted) -> Result<TagId> {
        if let Some(id) = self.done.get(&t.id.0) {
            return Ok(*id);
        }
        let id = self.tree.push(&t.name, conv);
        self.done.insert(t.id.0, id);
        Ok(id)
    }

    fn stopgaps(&self, rep: &mut ObjectReport, root: &[u8], replaced: &[usize]) {
        for (at, g, n) in self.base.references(root) {
            if !replaced.contains(&at) {
                rep.stopgaps.push((at, g.to_string(), n));
            }
        }
    }

    fn report(tag: &Tag, template: &str) -> ObjectReport {
        ObjectReport {
            group: tag.group.to_string(),
            source: tag.name.clone(),
            template: template.to_owned(),
            ..Default::default()
        }
    }

    fn weapon(&mut self, tag: &Tag) -> Result<TagId> {
        let w = Weapon::read(self.h1, tag)?;
        let (tpl, tpl_name) = self.template(b"weap", &tag.name, b"weap")?;
        let tpl_root = self.base.root(tpl)?;
        let mut rep = Self::report(tag, &tpl_name);
        let mut root = Block::new(tpl_root.clone());
        self.header(&mut rep, &mut root, &w.object, &tag.name)?;
        let mut wr = Writer {
            rep: &mut rep,
            prefix: "weap.".into(),
        };
        let b = &mut root.bytes;
        wr.f32(
            b,
            h2::AUTOAIM_ANGLE,
            "autoaim_angle",
            w.autoaim_angle,
            w.autoaim_angle,
        );
        wr.f32(
            b,
            h2::AUTOAIM_RANGE,
            "autoaim_range",
            w.autoaim_range,
            w.autoaim_range,
        );
        wr.f32(
            b,
            h2::MAGNETISM_ANGLE,
            "magnetism_angle",
            w.magnetism_angle,
            w.magnetism_angle,
        );
        wr.f32(
            b,
            h2::MAGNETISM_RANGE,
            "magnetism_range",
            w.magnetism_range,
            w.magnetism_range,
        );
        wr.f32(
            b,
            h2::DEVIATION_ANGLE,
            "deviation_angle",
            w.deviation_angle,
            w.deviation_angle,
        );
        wr.f32(
            b,
            h2::HEAT_RECOVERY_THRESHOLD,
            "heat_recovery_threshold",
            w.heat_recovery_threshold,
            w.heat_recovery_threshold,
        );
        wr.f32(
            b,
            h2::OVERHEATED_THRESHOLD,
            "overheated_threshold",
            w.overheated_threshold,
            w.overheated_threshold,
        );
        wr.f32(
            b,
            h2::HEAT_DETONATION_THRESHOLD,
            "heat_detonation_threshold",
            w.heat_detonation_threshold,
            w.heat_detonation_threshold,
        );
        wr.f32(
            b,
            h2::HEAT_DETONATION_FRACTION,
            "heat_detonation_fraction",
            w.heat_detonation_fraction,
            w.heat_detonation_fraction,
        );
        wr.f32(
            b,
            h2::HEAT_LOSS_PER_SECOND,
            "heat_loss_per_second",
            w.heat_loss_per_second,
            w.heat_loss_per_second,
        );
        wr.f32(
            b,
            h2::HEAT_ILLUMINATION,
            "heat_illumination",
            w.illumination,
            w.illumination,
        );

        // Magazines: each a copy of the template's first, with Halo 1's
        // counts and times.
        let tpl_mag = self
            .base
            .element(&tpl_root, h2::MAGAZINES, h2::MAGAZINE_SIZE, 0)?;
        let mut mags = Vec::new();
        for (i, m) in w.magazines.iter().enumerate() {
            let Some(mut e) = tpl_mag.clone() else {
                wr.rep.no_home.push(format!(
                    "magazine {i}: the template has no magazine to build on"
                ));
                continue;
            };
            wr.prefix = format!("magazine[{i}].");
            wr.i16(
                &mut e,
                h2::MAG_ROUNDS_RECHARGED,
                "rounds_recharged",
                m.rounds_recharged,
            );
            wr.i16(
                &mut e,
                h2::MAG_ROUNDS_TOTAL_INITIAL,
                "rounds_total_initial",
                m.rounds_total_initial,
            );
            wr.i16(
                &mut e,
                h2::MAG_ROUNDS_TOTAL_MAXIMUM,
                "rounds_total_maximum",
                m.rounds_total_maximum,
            );
            wr.i16(
                &mut e,
                h2::MAG_ROUNDS_LOADED_MAXIMUM,
                "rounds_loaded_maximum",
                m.rounds_loaded_maximum,
            );
            wr.f32(
                &mut e,
                h2::MAG_RELOAD_TIME,
                "reload_time",
                m.reload_time,
                m.reload_time,
            );
            wr.i16(
                &mut e,
                h2::MAG_ROUNDS_RELOADED,
                "rounds_reloaded",
                m.rounds_reloaded,
            );
            if m.chamber_time != 0.0 {
                wr.rep.no_home.push(format!(
                    "magazine {i} chamber time {} (Halo 2 offset not established)",
                    m.chamber_time
                ));
            }
            mags.push(e);
        }
        if !mags.is_empty() {
            let n = mags.len();
            root.block(h2::MAGAZINES as u32, n as u32, Block::new(mags.concat()));
        } else {
            root.bytes[h2::MAGAZINES..h2::MAGAZINES + 8].fill(0);
        }

        // Barrels: Halo 1 triggers, each on the template's barrel of the
        // same index (or its last).
        let tpl_barrels =
            u32::from_le_bytes(tpl_root[h2::BARRELS..h2::BARRELS + 4].try_into().unwrap()) as usize;
        let mut barrels = Block::zeroed(0);
        for (i, t) in w.triggers.iter().enumerate() {
            let Some(mut e) = self.base.element(
                &tpl_root,
                h2::BARRELS,
                h2::BARREL_SIZE,
                i.min(tpl_barrels.saturating_sub(1)),
            )?
            else {
                wr.rep.no_home.push(format!(
                    "trigger {i}: the template has no barrel to build on"
                ));
                continue;
            };
            wr.prefix = format!("barrel[{i}].");
            for (k, at) in [
                h2::BARREL_ROUNDS_PER_SECOND,
                h2::BARREL_ROUNDS_PER_SECOND + 4,
            ]
            .into_iter()
            .enumerate()
            {
                wr.f32(
                    &mut e,
                    at,
                    &format!("rounds_per_second[{k}]"),
                    t.rounds_per_second[k],
                    t.rounds_per_second[k],
                );
            }
            wr.f32(
                &mut e,
                h2::BARREL_ACCELERATION_TIME,
                "acceleration_time",
                t.firing_acceleration_time,
                t.firing_acceleration_time,
            );
            wr.f32(
                &mut e,
                h2::BARREL_DECELERATION_TIME,
                "deceleration_time",
                t.firing_deceleration_time,
                t.firing_deceleration_time,
            );
            wr.i16(&mut e, h2::BARREL_MAGAZINE, "magazine", t.magazine);
            wr.i16(
                &mut e,
                h2::BARREL_ROUNDS_PER_SHOT,
                "rounds_per_shot",
                t.rounds_per_shot,
            );
            wr.i16(
                &mut e,
                h2::BARREL_MINIMUM_ROUNDS_LOADED,
                "minimum_rounds_loaded",
                t.minimum_rounds_loaded,
            );
            wr.i16(
                &mut e,
                h2::BARREL_ROUNDS_BETWEEN_TRACERS,
                "rounds_between_tracers",
                t.rounds_between_tracers,
            );
            wr.f32(
                &mut e,
                h2::BARREL_ERROR_ACCELERATION_TIME,
                "error_acceleration_time",
                t.error_acceleration_time,
                t.error_acceleration_time,
            );
            wr.f32(
                &mut e,
                h2::BARREL_ERROR_DECELERATION_TIME,
                "error_deceleration_time",
                t.error_deceleration_time,
                t.error_deceleration_time,
            );
            wr.i16(
                &mut e,
                h2::BARREL_DISTRIBUTION_FUNCTION,
                "distribution_function",
                t.distribution_function,
            );
            wr.i16(
                &mut e,
                h2::BARREL_PROJECTILES_PER_SHOT,
                "projectiles_per_shot",
                t.projectiles_per_shot,
            );
            wr.f32(
                &mut e,
                h2::BARREL_DISTRIBUTION_ANGLE,
                "distribution_angle",
                t.distribution_angle,
                t.distribution_angle,
            );
            wr.f32(
                &mut e,
                h2::BARREL_MINIMUM_ERROR,
                "minimum_error",
                t.minimum_error,
                t.minimum_error,
            );
            for k in 0..2 {
                wr.f32(
                    &mut e,
                    h2::BARREL_ERROR_ANGLE + 4 * k,
                    &format!("error_angle[{k}]"),
                    t.error_angle[k],
                    t.error_angle[k],
                );
            }
            for k in 0..3 {
                let v = t.first_person_offset[k];
                wr.f32(
                    &mut e,
                    h2::BARREL_FIRST_PERSON_OFFSET + 4 * k,
                    &format!("first_person_offset[{k}]"),
                    v,
                    v,
                );
            }
            wr.f32(
                &mut e,
                h2::BARREL_HEAT_PER_ROUND,
                "heat_per_round",
                t.heat_per_round,
                t.heat_per_round,
            );
            let mut el = Block::new(e);
            if let Some(p) = t.projectile {
                let tpl_proj = self.base.reference(&el.bytes, h2::BARREL_PROJECTILE);
                let ptag = self.h1_tag(p)?;
                let id = self.object_with(ptag, tpl_proj)?;
                el.tag_ref(h2::BARREL_PROJECTILE as u32, id);
            }
            let at = barrels.bytes.len() as u32;
            barrels.bytes.extend_from_slice(&el.bytes);
            for mut f in el.fixups {
                f.offset += at;
                barrels.fixups.push(f);
            }
        }
        let n = barrels.bytes.len() / h2::BARREL_SIZE;
        if n > 0 {
            root.block(h2::BARRELS as u32, n as u32, barrels);
        }
        rep.no_home.extend(weapon_no_home(&w));
        self.stopgaps(&mut rep, &tpl_root, &[h2::MODEL]);
        self.tree.reports.push(rep);
        Ok(self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"weap"),
                root,
                raw: Vec::new(),
            },
        ))
    }

    fn projectile(&mut self, tag: &Tag, template: Option<TagId>) -> Result<TagId> {
        let p = Projectile::read(self.h1, tag)?;
        let (tpl, tpl_name) = match template {
            Some(t) => (
                t,
                self.base.name(t).map_or(String::new(), |n| n.1.to_owned()),
            ),
            None => self.template(b"proj", &tag.name, b"proj")?,
        };
        let tpl_root = self.base.root(tpl)?;
        let mut rep = Self::report(tag, &tpl_name);
        let mut root = Block::new(tpl_root.clone());
        self.header(&mut rep, &mut root, &p.object, &tag.name)?;
        let mut wr = Writer {
            rep: &mut rep,
            prefix: "proj.".into(),
        };
        let b = &mut root.bytes;
        wr.i16(
            b,
            h2::PROJ_DETONATION_TIMER_STARTS,
            "detonation_timer_starts",
            p.detonation_timer_starts,
        );
        wr.i16(b, h2::PROJ_IMPACT_NOISE, "impact_noise", p.impact_noise);
        wr.f32(
            b,
            h2::PROJ_AI_PERCEPTION_RADIUS,
            "ai_perception_radius",
            p.ai_perception_radius,
            p.ai_perception_radius,
        );
        wr.f32(
            b,
            h2::PROJ_COLLISION_RADIUS,
            "collision_radius",
            p.collision_radius,
            p.collision_radius,
        );
        wr.f32(
            b,
            h2::PROJ_ARMING_TIME,
            "arming_time",
            p.arming_time,
            p.arming_time,
        );
        wr.f32(
            b,
            h2::PROJ_DANGER_RADIUS,
            "danger_radius",
            p.danger_radius,
            p.danger_radius,
        );
        for k in 0..2 {
            wr.f32(
                b,
                h2::PROJ_TIMER + 4 * k,
                &format!("timer[{k}]"),
                p.timer[k],
                p.timer[k],
            );
        }
        let t = TICKS_PER_SECOND;
        wr.f32(
            b,
            h2::PROJ_MINIMUM_VELOCITY,
            "minimum_velocity",
            p.minimum_velocity,
            p.minimum_velocity * t,
        );
        wr.f32(
            b,
            h2::PROJ_MAXIMUM_RANGE,
            "maximum_range",
            p.maximum_range,
            p.maximum_range,
        );
        wr.i16(
            b,
            h2::PROJ_DETONATION_NOISE,
            "detonation_noise",
            p.detonation_noise,
        );
        wr.f32(
            b,
            h2::PROJ_AIR_GRAVITY_SCALE,
            "air_gravity_scale",
            p.air_gravity_scale,
            p.air_gravity_scale,
        );
        wr.f32(
            b,
            h2::PROJ_WATER_GRAVITY_SCALE,
            "water_gravity_scale",
            p.water_gravity_scale,
            p.water_gravity_scale,
        );
        for k in 0..2 {
            wr.f32(
                b,
                h2::PROJ_AIR_DAMAGE_RANGE + 4 * k,
                &format!("air_damage_range[{k}]"),
                p.air_damage_range[k],
                p.air_damage_range[k],
            );
            wr.f32(
                b,
                h2::PROJ_WATER_DAMAGE_RANGE + 4 * k,
                &format!("water_damage_range[{k}]"),
                p.water_damage_range[k],
                p.water_damage_range[k],
            );
        }
        wr.f32(
            b,
            h2::PROJ_INITIAL_VELOCITY,
            "initial_velocity",
            p.initial_velocity,
            p.initial_velocity * t,
        );
        wr.f32(
            b,
            h2::PROJ_FINAL_VELOCITY,
            "final_velocity",
            p.final_velocity,
            p.final_velocity * t,
        );
        wr.f32(
            b,
            h2::PROJ_GUIDED_ANGULAR_VELOCITY,
            "guided_angular_velocity",
            p.guided_angular_velocity,
            p.guided_angular_velocity * t,
        );
        if p.impact_damage.is_some() || p.attached_detonation_damage.is_some() {
            rep.no_home.push("damage (jpt!): the template's damage effects are kept; Halo 1 damage amounts are not carried yet".into());
        }
        rep.no_home.push(format!(
            "{} material responses: the template's are kept",
            p.material_responses.len()
        ));
        self.stopgaps(&mut rep, &tpl_root, &[h2::MODEL]);
        self.tree.reports.push(rep);
        Ok(self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"proj"),
                root,
                raw: Vec::new(),
            },
        ))
    }

    fn equipment(&mut self, tag: &Tag) -> Result<TagId> {
        let e = Equipment::read(self.h1, tag)?;
        let (tpl, tpl_name) = self.template(b"eqip", &tag.name, b"eqip")?;
        let tpl_root = self.base.root(tpl)?;
        let mut rep = Self::report(tag, &tpl_name);
        let mut root = Block::new(tpl_root.clone());
        self.header(&mut rep, &mut root, &e.object, &tag.name)?;
        let mut wr = Writer {
            rep: &mut rep,
            prefix: "eqip.".into(),
        };
        if e.powerup_type == POWERUP_HEALTH {
            wr.i16(
                &mut root.bytes,
                h2::EQIP_POWERUP_TYPE,
                "powerup_type",
                e.powerup_type,
            );
            // Health needs a positive amount (docs/game/health.md).
            let v = if e.powerup_time > 0.0 {
                e.powerup_time
            } else {
                1.0
            };
            wr.f32(
                &mut root.bytes,
                h2::EQIP_POWERUP_TIME,
                "powerup_time",
                e.powerup_time,
                v,
            );
        } else {
            rep.no_home.push(format!(
                "powerup type {} / grenade type {}: the template's are kept (Halo 2's values for them are not established)",
                e.powerup_type, e.grenade_type
            ));
        }
        self.stopgaps(&mut rep, &tpl_root, &[h2::MODEL]);
        self.tree.reports.push(rep);
        Ok(self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"eqip"),
                root,
                raw: Vec::new(),
            },
        ))
    }

    fn scenery(&mut self, tag: &Tag) -> Result<TagId> {
        if self.options.rebuild {
            return self.scenery_rebuilt(tag);
        }
        let object = h1::Object::read(self.h1, self.h1.tag_data(tag)?)?;
        let (tpl, tpl_name) = self.template(b"scen", &tag.name, b"scen")?;
        let tpl_root = self.base.root(tpl)?;
        let mut rep = Self::report(tag, &tpl_name);
        let mut root = Block::new(tpl_root.clone());
        self.header(&mut rep, &mut root, &object, &tag.name)?;
        rep.no_home
            .push("scen tuning (pathfinding and lightmapping policy): the template's".into());
        self.stopgaps(&mut rep, &tpl_root, &[h2::MODEL]);
        self.tree.reports.push(rep);
        Ok(self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"scen"),
                root,
                raw: Vec::new(),
            },
        ))
    }

    /// A Halo 1 machine, control or light fixture, from the `mach` or
    /// `ctrl` definition ([`crate::rebuild::device`]); a light fixture is
    /// a machine (Halo 2 has no `lifi` tags). There is no template path.
    fn device(&mut self, tag: &Tag) -> Result<TagId> {
        let dev = h1::Device::read(self.h1, tag)?;
        let lifi = dev.machine.is_none() && dev.control.is_none();
        let group = if dev.control.is_some() {
            b"ctrl"
        } else {
            b"mach"
        };
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        let hlmt = self.model(&mut rep, &dev.object, None, &tag.name, true)?;
        rep.physics = Some("none (no Halo 1 collision to build from)".into());
        self.object_physics(&mut rep, hlmt, &dev.object, &tag.name, PhysicsKind::Device)?;
        let predicted = self.predicted(&dev.object)?;
        let root = crate::rebuild::device(&dev, hlmt, &predicted)?;
        rep.no_home.push(
            "devi sounds and effects (Halo 1 effe references): null; the device is silent".into(),
        );
        if lifi {
            rep.no_home.push(
                "lifi: built as a mach (platform); its light (the placement's colour and intensity) is not carried".into(),
            );
        }
        self.tree.reports.push(rep);
        Ok(self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*group),
                root,
                raw: Vec::new(),
            },
        ))
    }

    /// A Halo 1 vehicle from the `vehi` definition
    /// ([`crate::rebuild::vehicle`]), no template: every vehicle (the
    /// template path was removed in converter version 48).
    fn vehicle_rebuilt(&mut self, tag: &Tag, u: &Unit) -> Result<TagId> {
        let b = self.h1.tag_data(tag)?;
        let word = |at: usize| {
            b.get(at..at + 4)
                .map_or(0, |x| u32::from_le_bytes(x.try_into().expect("4")))
        };
        let vehicle_type = word(crate::rebuild::H1_VEHICLE_TYPE) as u16;
        let flags = word(crate::rebuild::H1_VEHICLE_FLAGS);
        let role = self.vehicle_role(vehicle_type)?;
        let mut rep = Self::report(
            tag,
            &match &role {
                Some(r) => format!(
                    "none: rebuilt from tags::defs (vehicle type {vehicle_type}, role over {} base vehicles)",
                    r.words.members.len()
                ),
                None => format!(
                    "none: rebuilt from tags::defs (vehicle type {vehicle_type}; the base has none of the type: turret role)"
                ),
            },
        );
        let hlmt = self.model(&mut rep, &u.object, None, &tag.name, true)?;
        self.object_physics(&mut rep, hlmt, &u.object, &tag.name, PhysicsKind::Vehicle)?;
        let predicted = self.predicted(&u.object)?;
        let markers = match u.object.model {
            Some(m) => {
                h1models::model_from_map(self.h1, self.h1_tag(m)?, &ModelOptions::default())?
                    .0
                    .markers
                    .into_iter()
                    .map(|m| m.name)
                    .collect()
            }
            None => Vec::new(),
        };
        let seats: Vec<_> = u
            .seats
            .iter()
            .map(|s| crate::rebuild::seat_names(s, &markers))
            .collect();
        let points: Vec<crate::rebuild::Point> = match u.object.model {
            Some(m) => {
                let (model, _) =
                    h1models::model_from_map(self.h1, self.h1_tag(m)?, &ModelOptions::default())?;
                self.powered_points(&u.object, &model)?
                    .into_iter()
                    .map(|p| p.point)
                    .collect()
            }
            None => Vec::new(),
        };
        // The unit's weapons: Halo 1's, converted where they convert.
        let mut weapons = Vec::new();
        for r in &u.references {
            let Some(id) = r.tag else { continue };
            if r.group.0.to_be_bytes() != *b"weap" {
                continue;
            }
            let t = self.h1_tag(id)?;
            match self.object(t) {
                Ok(w) if !weapons.contains(&w) => weapons.push(w),
                Ok(_) => {}
                Err(e) => rep
                    .no_home
                    .push(format!("unit weapon {}: not converted ({e})", t.name)),
            }
        }
        let root = crate::rebuild::vehicle(&crate::rebuild::VehicleSpec {
            object: &u.object,
            hlmt,
            seats: &seats,
            predicted: &predicted,
            camera_field_of_view: u.camera_field_of_view,
            vehicle_type,
            flags,
            weapons: &weapons,
            role: role.as_ref(),
            points: &points,
        })?;
        rep.no_home.push(format!(
            "vehi: Halo 1's type, flags (low half), camera, {} seats (label as the rider's mode, markers), {} weapons and {} powered mass points as friction or hover points; the type's role words and gears; HUD, sounds, effects and camera tracks null; {} Halo 1 references",
            u.seats.len(),
            weapons.len(),
            points.len(),
            u.references.iter().filter(|r| r.tag.is_some()).count()
        ));
        self.tree.reports.push(rep);
        Ok(self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"vehi"),
                root,
                raw: Vec::new(),
            },
        ))
    }

    /// The vehicle type's role over the base's vehicles
    /// ([`crate::rebuild::vehicle_role`]), computed once per type. The
    /// turret type keeps the disc-wide turret tables
    /// ([`crate::rebuild::TURRET_ROLE`]): `None`.
    pub fn vehicle_role(&mut self, vehicle_type: u16) -> Result<Option<crate::rebuild::VehicleRole>> {
        if u32::from(vehicle_type) == crate::rebuild::ROLE_VEHICLE_TYPE {
            return Ok(None);
        }
        if let Some(w) = self.vehicle_roles.get(&vehicle_type) {
            return Ok(w.clone());
        }
        let w = crate::rebuild::vehicle_role(self.base, vehicle_type)?;
        self.vehicle_roles.insert(vehicle_type, w.clone());
        Ok(w)
    }

    /// Halo 1's powered mass points (`phys` mass points with a powered
    /// mass point: the warthog's four tires, the ghost's hull and wings),
    /// each as a marker on its model node and a Halo 2 point. V: Halo 1's
    /// mass point positions are in the object's space (each warthog tire
    /// point lies within its radius of its tire node's default
    /// position, `tests/vehicle_rebuild_disc.rs`), so the marker holds the
    /// position in its node's space. The region is the collision region
    /// of the collision node named as the model node, else none.
    fn powered_points(
        &self,
        obj: &h1::Object,
        model: &h1models::h1::Model,
    ) -> Result<Vec<PoweredPoint>> {
        let Some(p) = obj.physics else {
            return Ok(Vec::new());
        };
        let phys = h1::Physics::read(self.h1, self.h1_tag(p)?)?;
        let coll = match obj.collision {
            Some(c) => Some(h1::Collision::read(self.h1, self.h1_tag(c)?)?),
            None => None,
        };
        let world = h1models::mode::world_transforms(
            &model
                .nodes
                .iter()
                .map(|n| (n.parent, n.rotation, n.translation))
                .collect::<Vec<_>>(),
        );
        let mut out: Vec<PoweredPoint> = Vec::new();
        for m in phys.mass_points.iter().filter(|m| m.powered_mass_point >= 0) {
            let node = if (m.model_node as usize) < model.nodes.len() && m.model_node >= 0 {
                m.model_node
            } else {
                0
            };
            let (r, t) = world
                .get(node as usize)
                .copied()
                .unwrap_or(([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], [0.0; 3]));
            let d: [f32; 3] = std::array::from_fn(|a| m.position[a] - t[a]);
            // The inverse of a rotation is its transpose.
            let local = std::array::from_fn(|a| (0..3).map(|k| r[k][a] * d[k]).sum());
            let node_name = model.nodes.get(node as usize).map(|n| n.name.as_str());
            let region = coll
                .as_ref()
                .and_then(|c| {
                    let n = c.nodes.iter().find(|n| Some(n.name.as_str()) == node_name)?;
                    c.regions.get(usize::try_from(n.region).ok()?)
                })
                .map(|r| crate::coll::sid_name(&r.name))
                .unwrap_or_default();
            let marker = crate::coll::sid_name(&m.name);
            if out.iter().any(|p| p.point.marker == marker) {
                continue;
            }
            out.push(PoweredPoint {
                point: crate::rebuild::Point { marker, region },
                node,
                position: local,
            });
        }
        Ok(out)
    }

    /// The role's words over the base's bipeds ([`crate::rebuild::biped_role`]),
    /// computed once per role.
    pub fn biped_role(
        &mut self,
        role: crate::rebuild::BipedRole,
    ) -> Result<Option<crate::rebuild::RoleWords>> {
        if let Some(w) = self.roles.get(&role) {
            return Ok(w.clone());
        }
        let w = crate::rebuild::biped_role(self.base, role)?;
        self.roles.insert(role, w.clone());
        Ok(w)
    }

    /// The Halo 2 biped a Halo 1 biped takes its references from: the
    /// first [`crate::rebuild::COUNTERPARTS`] row the base holds, else the
    /// template path's template. `None` when the base has neither.
    pub fn biped_counterpart(&self, path: &str) -> Option<(TagId, &'static str)> {
        crate::rebuild::counterpart_candidates(path)
            .into_iter()
            .chain(template_candidates(b"bipd", path))
            .find_map(|n| Some((self.base.find(b"bipd", n)?, n)))
    }

    /// The role a Halo 1 biped is rebuilt with: its own
    /// ([`crate::rebuild::BipedRole::of_h1`]), else its counterpart's (the
    /// flood, sentinels and the monitor take the template path's template,
    /// the Chief, as that path does).
    pub fn biped_rebuild_role(&self, path: &str) -> Option<crate::rebuild::BipedRole> {
        crate::rebuild::BipedRole::of_h1(path).or_else(|| {
            self.biped_counterpart(path)
                .and_then(|(_, n)| crate::rebuild::BipedRole::of_h2(n))
        })
    }

    /// A Halo 1 biped from the `bipd` definition ([`crate::rebuild::biped`]):
    /// Halo 1's model chain with a physics model built from its collision
    /// ([`PhysicsKind::Biped`]), the counterpart's references on the
    /// `bipd` and the `hlmt` (damage, dialogue, camouflage), and for the
    /// player the counterpart's HUD with the health meter.
    fn biped_rebuilt(
        &mut self,
        tag: &Tag,
        u: &Unit,
        role: crate::rebuild::BipedRole,
        words: &crate::rebuild::RoleWords,
    ) -> Result<TagId> {
        use crate::rebuild::{BIPED_OWN, BIPED_WITH_REFERENCES, BipedParts, MODEL_OWN};
        let (cid, cname) = self
            .biped_counterpart(&tag.name)
            .ok_or_else(|| Error::NoTemplate(format!("bipd {}: no counterpart", tag.name)))?;
        let mut rep = Self::report(
            tag,
            &format!(
                "none: rebuilt from tags::defs ({role:?} role over {} base bipeds; references from {cname})",
                words.members.len()
            ),
        );
        let bipd_def = tags::defs::Defs::builtin()
            .group(FourCC(*b"bipd"))
            .ok_or_else(|| Error::Invalid("no bipd definition".into()))?;
        let counterpart = self.base.record(bipd_def, cid)?;
        let hlmt = self.model(&mut rep, &u.object, None, &tag.name, true)?;
        let counterpart_hlmt = self.base.reference(&self.base.root(cid)?, h2::MODEL);
        if let (Some(h), Some(ch)) = (hlmt, counterpart_hlmt) {
            let hd = tags::defs::Defs::builtin()
                .group(FourCC(*b"hlmt"))
                .ok_or_else(|| Error::Invalid("no hlmt definition".into()))?;
            let c = self.base.record(hd, ch)?;
            let fields = crate::rebuild::counterpart_fields(hd, &c, MODEL_OWN, &[]);
            let k = h.0 as usize & 0xffff;
            let root = &mut self.tree.tags[k].1.root;
            let mut r = tags::defs::from_built(hd, root)
                .map_err(|e| Error::Invalid(format!("hlmt: {e}")))?;
            for (path, v) in &fields {
                r.set(path, v.clone())
                    .map_err(|e| Error::Invalid(e.to_string()))?;
            }
            *root = tags::defs::to_built(hd, &r, &mut |_| None)
                .map_err(|e| Error::Invalid(e.to_string()))?;
            rep.no_home.push(format!(
                "hlmt: {} reference fields from {} (damage, dialogue, camouflage shaders)",
                fields.len(),
                self.base.name(ch).map_or("?", |n| n.1)
            ));
        }
        self.object_physics(&mut rep, hlmt, &u.object, &tag.name, PhysicsKind::Biped)?;
        let predicted = self.predicted(&u.object)?;
        let mut swap = None;
        let hud = if health_hud::is_player_biped(&tag.name) {
            let tpl_root = self.base.root(cid)?;
            match self.health_interfaces(&tpl_root) {
                Ok(ids) => {
                    rep.no_home.push(format!(
                        "HUD: {cname}'s with Halo 1's health meter ({} interfaces; docs/game/health.md)",
                        ids.len()
                    ));
                    match self.unit_hud(u) {
                        Ok((h1, n)) => {
                            rep.no_home.push(format!(
                                "HUD: Halo 1's ({n} widgets; docs/game/hud.md), Halo 2's at the switch"
                            ));
                            swap = Some((h1, ids[0]));
                            Some(vec![h1; ids.len()])
                        }
                        Err(e) => {
                            rep.no_home.push(format!("HUD: no Halo 1 HUD: {e}"));
                            Some(ids)
                        }
                    }
                }
                Err(e) => {
                    rep.no_home.push(format!("HUD: no health meter: {e}"));
                    None
                }
            }
        } else {
            None
        };
        let parts = BipedParts {
            counterpart: Some(counterpart),
            hud,
        };
        let root = crate::rebuild::biped(u, hlmt, &predicted, words, &parts)?;
        let taken = crate::rebuild::reference_fields(bipd_def)
            .into_iter()
            .filter(|p| !BIPED_OWN.contains(&p.as_str()))
            .count()
            + BIPED_WITH_REFERENCES.len();
        rep.no_home.push(format!(
            "bipd: the role's words, Halo 1's camera field of view, {} matched words and collision size (character shapes sized to it); {taken} reference fields from {cname}; {} Halo 1 references not converted (their groups have no converter here)",
            crate::rebuild::BIPED_WORDS.len(),
            u.references.iter().filter(|r| r.tag.is_some()).count()
        ));
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"bipd"),
                root,
                raw: Vec::new(),
            },
        );
        if let Some((halo1, halo2)) = swap {
            self.tree.hud_swaps.push(HudSwap {
                owner: id,
                halo1,
                halo2,
            });
        }
        Ok(id)
    }

    /// Gives an object's `hlmt` (a tree tag) a physics model built from
    /// Halo 1's collision when it has none ([`Self::built_physics`]).
    /// Vehicles need one: `vehicle_new` (`0x000eeb80`) refuses a vehicle
    /// whose model has none (`cmp [hlmt+0x24],-1` at `0x000eebcb`) and
    /// `object_new` deletes it; a vehicle with no usable collision gets
    /// the stand-in sphere ([`crate::rebuild::sphere_physics`],
    /// [`STAND_IN_SPHERE`]). Devices need one to block: Havok, which moves
    /// the player, sees a non-scenery object only through its physics
    /// model, so without one a door is walked through (retail doors have a
    /// rigid body per moving node).
    fn object_physics(
        &mut self,
        rep: &mut ObjectReport,
        hlmt: Option<TagId>,
        obj: &h1::Object,
        name: &str,
        kind: PhysicsKind,
    ) -> Result<()> {
        let Some(h) = hlmt.filter(|h| h.0 & 0xffff_0000 == LOCAL) else {
            return Ok(());
        };
        let k = h.0 as usize & 0xffff;
        let at = h2::HLMT_PHYSICS_MODEL as u32;
        let root = &self.tree.tags[k].1.root;
        let has = match root.fixups.iter().find(|f| f.offset == at) {
            Some(f) => !matches!(f.kind, FixupKind::TagRef(TagId::NULL)),
            None => {
                let o = at as usize + 4;
                root.bytes.len() >= o + 4 && root.bytes[o..o + 4] != [0xff; 4]
            }
        };
        if has {
            return Ok(());
        }
        let (phmo, how) = match self.built_physics(obj, kind)? {
            Some((block, how)) => (block, how),
            None if kind == PhysicsKind::Vehicle => {
                let (radius, mass, material) = STAND_IN_SPHERE;
                (
                    crate::rebuild::sphere_physics(radius, [0.0, 0.0, radius], mass, material)?,
                    format!(
                        "stand-in: a {radius} wu sphere of {mass} kg on the origin (rebuild::sphere_physics); no Halo 1 collision to build from"
                    ),
                )
            }
            None => return Ok(()),
        };
        let phmo = self.tree.push(
            name,
            Converted {
                group: FourCC(*b"phmo"),
                root: phmo,
                raw: Vec::new(),
            },
        );
        let root = &mut self.tree.tags[k].1.root;
        root.fixups.retain(|f| f.offset != at);
        root.tag_ref(at, phmo);
        rep.physics = Some(how);
        Ok(())
    }

    /// A physics model from Halo 1's collision: each collision node's
    /// first BSP cut into its solid convex cells ([`crate::physics`]).
    /// A vehicle gets one rigid body on the object (node -1) holding every
    /// node's cells placed by the model's default pose, of Halo 1's `phys`
    /// mass; a device one body per collision node, on the model node of
    /// that name, so each moves with its node as the door opens. A body
    /// over [`crate::physics::MAX_PIECES`] cells keeps one enclosing piece
    /// per node. `None` with no collision, no model or no solid cell.
    fn built_physics(
        &mut self,
        obj: &h1::Object,
        kind: PhysicsKind,
    ) -> Result<Option<(Block, String)>> {
        use crate::physics::{self, Transform};
        let (Some(c), Some(m)) = (obj.collision, obj.model) else {
            return Ok(None);
        };
        let coll = h1::Collision::read(self.h1, self.h1_tag(c)?)?;
        let (model, _) =
            h1models::model_from_map(self.h1, self.h1_tag(m)?, &ModelOptions::default())?;
        let world = h1models::mode::world_transforms(
            &model
                .nodes
                .iter()
                .map(|n| (n.parent, n.rotation, n.translation))
                .collect::<Vec<_>>(),
        );
        // (model node, cells) per collision node with any.
        let mut parts: Vec<(i16, Vec<physics::Hull>)> = Vec::new();
        for (i, n) in coll.nodes.iter().enumerate() {
            let Some(src) = n.bsps.first() else { continue };
            let cells = physics::solid_cells(&h1convert::collision::convert(src)?);
            if cells.is_empty() {
                continue;
            }
            let node = model
                .nodes
                .iter()
                .position(|m| m.name == n.name)
                .or((i < model.nodes.len()).then_some(i))
                .unwrap_or(0);
            parts.push((node as i16, cells));
        }
        if parts.is_empty() {
            return Ok(None);
        }
        let mut enclosed = false;
        let mut cap = |cells: Vec<physics::Hull>, total: usize| {
            if total > physics::MAX_PIECES {
                enclosed = true;
                physics::enclosing(&cells).map_or(cells, |h| vec![h])
            } else {
                cells
            }
        };
        let volume: f32 = parts.iter().flat_map(|p| &p.1).map(|h| h.volume).sum();
        let bodies = match kind {
            PhysicsKind::Vehicle => {
                let total = parts.iter().map(|p| p.1.len()).sum();
                let mut pieces = Vec::new();
                for (node, cells) in parts {
                    let (r, t) = world
                        .get(node as usize)
                        .copied()
                        .unwrap_or((Transform::IDENTITY.r, Transform::IDENTITY.t));
                    let m = Transform { r, t };
                    pieces.extend(cap(cells, total).iter().map(|h| h.transformed(&m)));
                }
                let phys = match obj.physics {
                    Some(p) => Some(h1::Physics::read(self.h1, self.h1_tag(p)?)?),
                    None => None,
                };
                let mass = phys
                    .map(|p| p.mass)
                    .filter(|m| m.is_finite() && *m > 0.0)
                    .unwrap_or(volume * DENSITY);
                vec![crate::rebuild::Body {
                    node: -1,
                    pieces,
                    mass,
                }]
            }
            PhysicsKind::Device | PhysicsKind::Biped => parts
                .into_iter()
                .map(|(node, cells)| {
                    let total = cells.len();
                    let pieces = cap(cells, total);
                    let mass = pieces.iter().map(|h| h.volume).sum::<f32>() * DENSITY;
                    crate::rebuild::Body { node, pieces, mass }
                })
                .collect(),
        };
        let nodes: Vec<_> = model
            .nodes
            .iter()
            .map(|n| crate::rebuild::PhysNode {
                name: n.name.clone(),
                parent: n.parent,
                next_sibling: n.next_sibling,
                first_child: n.first_child,
            })
            .collect();
        let pieces: usize = bodies.iter().map(|b| b.pieces.len()).sum();
        let how = format!(
            "built from Halo 1's coll: {} rigid bodies, {pieces} convex pieces{}",
            bodies.len(),
            if enclosed {
                " (over the piece cap: one enclosing piece per node)"
            } else {
                ""
            }
        );
        let words = if kind == PhysicsKind::Biped {
            crate::rebuild::BIPED_BODY
        } else {
            crate::rebuild::DEVICE_BODY
        };
        Ok(Some((
            crate::rebuild::physics_model(&bodies, &nodes, words)?,
            how,
        )))
    }

    /// The tree's tags of `group` that `b` references, in order, each once.
    fn locals_in(&self, b: &Block, group: FourCC, out: &mut Vec<TagId>) {
        for f in &b.fixups {
            match &f.kind {
                FixupKind::Block { child, .. }
                | FixupKind::Data(child)
                | FixupKind::Pointer(child) => self.locals_in(child, group, out),
                FixupKind::TagId(t) | FixupKind::TagRef(t) => {
                    let k = t.0 as usize & 0xffff;
                    if t.0 & 0xffff_0000 == LOCAL
                        && self.tree.tags.get(k).is_some_and(|c| c.1.group == group)
                        && !out.contains(t)
                    {
                        out.push(*t);
                    }
                }
                _ => {}
            }
        }
    }

    /// The resources an object built from scratch predicts
    /// ([`crate::rebuild::Predicted`]): its converted render model, then
    /// every converted bitmap its shaders draw. V: every retail vehicle
    /// lists its `mode` once (`+0` 2, `+2` -1) and its bitmaps (`+0` 0,
    /// `+2` 0), the model first.
    fn predicted(&self, obj: &h1::Object) -> Result<Vec<crate::rebuild::Predicted>> {
        let Some(m) = obj.model else {
            return Ok(Vec::new());
        };
        let Some(&mode) = self.done.get(&self.h1_tag(m)?.id.0) else {
            return Ok(Vec::new());
        };
        let mut out = vec![crate::rebuild::Predicted::RenderModel(mode)];
        let Some((_, conv)) = self.tree.tags.get(mode.0 as usize & 0xffff) else {
            return Ok(out);
        };
        let mut shaders = Vec::new();
        self.locals_in(&conv.root, FourCC(*b"shad"), &mut shaders);
        let mut bitmaps = Vec::new();
        for s in shaders {
            let (_, c) = &self.tree.tags[s.0 as usize & 0xffff];
            self.locals_in(&c.root, FourCC(*b"bitm"), &mut bitmaps);
        }
        out.extend(bitmaps.into_iter().map(crate::rebuild::Predicted::Bitmap));
        Ok(out)
    }

    fn unit(&mut self, tag: &Tag) -> Result<TagId> {
        let u = Unit::read(self.h1, tag)?;
        let g: [u8; 4] = tag.group.0.to_be_bytes();
        // Every vehicle is built from the `vehi` definition: there is no
        // vehicle template path.
        if &g == b"vehi" {
            return self.vehicle_rebuilt(tag, &u);
        }
        if &g == b"bipd"
            && self.options.rebuild_bipeds
            && let Some(role) = self.biped_rebuild_role(&tag.name)
            && let Some(words) = self.biped_role(role)?
        {
            return self.biped_rebuilt(tag, &u, role, &words);
        }
        let (tpl, tpl_name) = self.template(&g, &tag.name, &g)?;
        let tpl_root = self.base.root(tpl)?;
        let mut rep = Self::report(tag, &tpl_name);
        let mut root = Block::new(tpl_root.clone());
        self.header(&mut rep, &mut root, &u.object, &tag.name)?;
        rep.no_home.push(format!(
            "{} tuning (speeds, seats, camera, damage): the template's; {} Halo 1 references not converted",
            tag.group,
            u.references.iter().filter(|r| r.tag.is_some()).count()
        ));
        if &g == b"bipd" && health_hud::is_player_biped(&tag.name) {
            match self.health_hud(&mut root, &tpl_root) {
                Ok(n) => rep.no_home.push(format!(
                    "HUD: the template's with Halo 1's health meter ({n} interfaces; docs/game/health.md)"
                )),
                Err(e) => rep.no_home.push(format!("HUD: no health meter: {e}")),
            }
        }
        self.stopgaps(&mut rep, &tpl_root, &[h2::MODEL]);
        self.tree.reports.push(rep);
        Ok(self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(g),
                root,
                raw: Vec::new(),
            },
        ))
    }
}

impl Converter<'_> {
    /// Halo 1's unit HUD of `u` (its `unhi`) as an `nhdt` written from
    /// scratch ([`crate::h1hud`]), built once per `unhi`. Returns the tag
    /// and its widget count.
    pub fn unit_hud(&mut self, u: &crate::h1::Unit) -> Result<(TagId, usize)> {
        let unhi = u
            .references
            .iter()
            .filter(|r| r.group == h1map::Group::new(b"unhi"))
            .find_map(|r| r.tag)
            .ok_or_else(|| Error::Invalid("no unhi".into()))?;
        let tag = self.h1.tag(unhi)?.clone();
        if let Some(id) = self.done.get(&tag.id.0) {
            let n = self.unit_hud_widgets.get(&tag.id.0).copied().unwrap_or(0);
            return Ok((*id, n));
        }
        let b = crate::h1::tag_bytes(self.h1, &tag, b"unhi", crate::h1hud::unhi::SIZE)?.to_vec();
        let elements = crate::h1hud::unit_elements(self.h1, &b)?;
        let find = |group: &[u8; 4], name: &str| {
            self.base
                .find(group, name)
                .ok_or_else(|| Error::NoTemplate(name.into()))
        };
        let meter = find(b"shad", crate::h1hud::METER_SHADER)?;
        let plain = find(b"shad", crate::h1hud::STATIC_SHADER)?;
        let mut widgets = Vec::new();
        for e in &elements {
            let bt = self.h1.tag(e.bitmap)?.clone();
            let bm = h1map::bitmap::Bitmap::read(self.h1, &bt)?;
            if crate::h1hud::sprite(&bm, e.sequence).is_none() {
                return Err(Error::Invalid(format!(
                    "{}: {} draws nothing in sequence {}",
                    e.name, bt.name, e.sequence
                )));
            }
            let bitmap = self
                .bitmap(e.bitmap)
                .ok_or_else(|| Error::Invalid(format!("bitmap {} unconverted", bt.name)))?;
            let reg = crate::h1hud::registration(e.anchor);
            widgets.push(crate::h1hud::widget_for(e, bitmap, reg, meter, plain));
        }
        if widgets.is_empty() {
            return Err(Error::Invalid(format!("{}: no drawable element", tag.name)));
        }
        let n = widgets.len();
        let id = self.tree.push(
            &format!("{}{}", tag.name, crate::h1hud::NHDT_SUFFIX),
            Converted {
                group: FourCC(*b"nhdt"),
                root: crate::h1hud::nhdt(&widgets),
                raw: Vec::new(),
            },
        );
        self.done.insert(tag.id.0, id);
        self.unit_hud_widgets.insert(tag.id.0, n);
        Ok((id, n))
    }

    /// Points the player biped's HUD interfaces (`unit +0x1a0`) at copies
    /// of their `nhdt` with the health meter ([`health_hud`]). Returns how
    /// many interfaces it points.
    fn health_hud(&mut self, root: &mut Block, tpl_root: &[u8]) -> Result<usize> {
        use health_hud::{HUD_INTERFACE_SIZE, UNIT_HUD_INTERFACES};
        let ids = self.health_interfaces(tpl_root)?;
        let count = ids.len();
        let mut elements = Block::new(vec![0; count * HUD_INTERFACE_SIZE]);
        for (i, id) in ids.into_iter().enumerate() {
            elements.tag_ref((i * HUD_INTERFACE_SIZE) as u32, id);
        }
        root.block(UNIT_HUD_INTERFACES as u32, count as u32, elements);
        Ok(count)
    }

    /// The health-meter copies ([`Self::health_nhdt`]) of the HUD
    /// interfaces (`unit +0x1a0`) of the unit whose root is `tpl_root`, in
    /// order.
    fn health_interfaces(&mut self, tpl_root: &[u8]) -> Result<Vec<TagId>> {
        use health_hud::{HUD_INTERFACE_SIZE, UNIT_HUD_INTERFACES};
        let count = crate::base::u32_at(tpl_root, UNIT_HUD_INTERFACES) as usize;
        if count == 0 {
            return Err(Error::Invalid("the template unit has no HUD".into()));
        }
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let e = self
                .base
                .element(tpl_root, UNIT_HUD_INTERFACES, HUD_INTERFACE_SIZE, i)?
                .ok_or(Error::Truncated("unit HUD interfaces"))?;
            // Unchanged unless every interface resolves: a null here would
            // leave the player with no HUD at all.
            let nhdt = self.base.reference(&e, 0).ok_or_else(|| {
                Error::Invalid(format!("HUD interface {i} names no nhdt in the base map"))
            })?;
            out.push(self.health_nhdt(nhdt)?);
        }
        Ok(out)
    }

    /// The template `nhdt` with the health widget added, built once.
    fn health_nhdt(&mut self, nhdt: TagId) -> Result<TagId> {
        use health_hud::{BITMAP_WIDGET_SIZE, NHDT_BITMAP_WIDGETS, NHDT_ROOT};
        if let Some(id) = self.health_huds.get(&nhdt) {
            return Ok(*id);
        }
        let name = self
            .base
            .name(nhdt)
            .map(|(_, n)| n.to_owned())
            .ok_or_else(|| Error::NoTemplate(format!("nhdt {nhdt:?}")))?;
        let root = self.base.head(nhdt, NHDT_ROOT as u64)?;
        let count = crate::base::u32_at(&root, NHDT_BITMAP_WIDGETS) as usize;
        let address = crate::base::u32_at(&root, NHDT_BITMAP_WIDGETS + 4);
        let widgets = self
            .base
            .meta
            .slice(address, (count * BITMAP_WIDGET_SIZE) as u64)?
            .to_vec();
        let find = |group: &[u8; 4], name: &str| {
            self.base
                .find(group, name)
                .ok_or_else(|| Error::NoTemplate(name.into()))
        };
        let bitmap = find(b"bitm", health_hud::STRIP_BITMAP)?;
        let shader = find(b"shad", health_hud::STRIP_SHADER)?;
        let (block, n) = health_hud::widgets_with_health(&widgets, bitmap, shader)
            .ok_or_else(|| Error::Invalid(format!("{name} has no shield meter")))?;
        let mut out = Block::new(root);
        out.block(NHDT_BITMAP_WIDGETS as u32, n, block);
        let id = self.tree.push(
            &format!("{name}{}", health_hud::NHDT_SUFFIX),
            Converted {
                group: FourCC(*b"nhdt"),
                root: out,
                raw: Vec::new(),
            },
        );
        self.health_huds.insert(nhdt, id);
        Ok(id)
    }
}

/// Halo 1 weapon data this conversion does not carry.
fn weapon_no_home(w: &Weapon) -> Vec<String> {
    let mut out = Vec::new();
    if w.magnification_levels > 0 {
        out.push(format!(
            "zoom: {} levels, {:?}x (the template's zoom is kept; Halo 2 offset not established)",
            w.magnification_levels, w.magnification_range
        ));
    }
    if w.first_person_model.is_some() {
        out.push(
            "first-person model and animations: the template's (Halo 1's not converted yet)".into(),
        );
    }
    if w.hud.is_some() {
        out.push("HUD (wphi): the template's nhdt".into());
    }
    if w.ready_time != 0.0 {
        out.push(format!(
            "ready time {} (Halo 2 offset not established)",
            w.ready_time
        ));
    }
    if w.triggers.iter().any(|t| t.charging_effect.is_some()) {
        out.push("trigger charging (plasma pistol): the template's".into());
    }
    out
}
