//! Halo 1 objects as Halo 2 tags.
//!
//! Scenery, vehicles, devices and (by default) bipeds are written from
//! typed definitions ([`crate::rebuild`]), with no template. Weapons,
//! equipment, projectiles and bipeds with no rebuilt role still start
//! from the root of the retail Halo 2 tag that plays the same role
//! ([`TEMPLATES`]). The fields known in both games are
//! overwritten with Halo 1's values ([`Tuning`] rows), the model chain is
//! replaced with converted tags, and the rest stays the template's: its
//! effects, sounds, HUD, first-person model and physics model are the
//! stopgaps listed in each [`ObjectReport`]. Template child blocks that are
//! not rewritten keep their retail addresses, so a [`Tree`] is installed
//! into a `TagSet` built from the same base map (`TagSet::from_map`), which
//! pins them in place. Halo 2 offsets and evidence:
//! `docs/halo1/objects.md`.

use std::collections::HashMap;

use h1map::{H1Map, Tag};
use h1models::Converted;
use h1models::mode::ModelOptions;
use map::build::{Block, FixupKind, RawId, RawRef, TagEntry, TagPayload, TagSet};
use map::{FourCC, TagId};

use crate::base::Base;
use crate::coll::{convert_collision, put_sid};
use crate::h1::{self, Unit};
use crate::{health_hud, scripted_props};
use crate::{Error, Result};

#[path = "convert_attachments.rs"]
mod attachments_conv;
#[path = "convert_arsenal.rs"]
mod arsenal_conv;
#[path = "convert_scripted_props.rs"]
mod scripted_props_conv;
#[path = "convert_powerups.rs"]
mod powerups_conv;
#[path = "convert_particles.rs"]
mod particles_conv;
#[path = "convert_flares.rs"]
pub mod flares_conv;

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
    pub const CONE_ANGLE: usize = 0x208;
    pub const CONE_RANGE: usize = 0x20c;
    pub const PULL_ANGLE: usize = 0x210;
    pub const PULL_RANGE: usize = 0x214;
    pub const BARRELS: usize = 0x2d0;
    pub const MAGAZINE_SIZE: usize = 0x5c;
    /// Barrel element.
    pub const BARREL_ROUNDS_PER_SECOND: usize = 0x04;
    pub const BARREL_PROJECTILE: usize = 0x8c;
    /// `proj` root.
    pub const PROJ_INITIAL_VELOCITY: usize = 0x17c;
    pub const PROJ_FINAL_VELOCITY: usize = 0x180;
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
/// second (S, docs/halo1/objects.md).
pub const TICKS_PER_SECOND: f32 = 30.0;

/// Halo 1's health powerup type, which Halo 2 handles the same way
/// (docs/halo1/health.md).
pub const POWERUP_HEALTH: i16 = 5;

/// The Chief's emblem material (stem `opaque\emblem_overlay_simple`): the
/// engine's emblem bitmap, its frame and colours the drawn player's
/// (`0x0022ace7`). Player bipeds' emblem parts draw with it.
pub const EMBLEM_SHADER: &str = "objects\\characters\\masterchief\\shaders\\mc_emblem";

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
    /// Halo 1 sounds the tags play (effect parts), converted, by path;
    /// the tags name them by [`sound`] placeholders. They need the map's
    /// sound gestalt, so [`Tree::install`] adds them first.
    pub sounds: Vec<(String, h1sounds::ConvertedSound)>,
    /// Halo 1 looping sounds the tags attach, read and converted; named
    /// by [`looping`] placeholders.
    pub loops: Vec<h1sounds::looping::Looping>,
    /// What installing the sounds left out, and why.
    pub audio_notes: Vec<String>,
    /// The HUD each owner (the player biped) can switch between at game
    /// start (`boot::hud`; docs/halo1/hud.md).
    pub hud_swaps: Vec<HudSwap>,
    /// Where the player's Halo 1 HUD centres the motion sensor's blips
    /// ([`crate::h1hud::sensor_centre`]), for `boot::hud_swap`.
    pub hud_sensor: Option<[i16; 2]>,
    /// Per tag (its index in [`Tree::tags`]), the values its builder took
    /// from a role ([`crate::provenance`]), for the role audit.
    pub role_fields: Vec<(usize, Vec<crate::provenance::RoleField>)>,
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
const SOUND: u32 = 0xfffc_0000;
const LOOP: u32 = 0xfffb_0000;

/// Stands for the `k`-th sound of a [`Tree`] until it is installed.
pub fn sound(k: usize) -> TagId {
    TagId(SOUND | k as u32)
}

/// Stands for the `k`-th looping sound of a [`Tree`] until it is installed.
pub fn looping(k: usize) -> TagId {
    TagId(LOOP | k as u32)
}

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
    /// sphere when the collision gives nothing. `wheels`: its wheels and
    /// treads are held in the body above the hull's lowest point
    /// ([`held_wheels`]; the role's [`crate::rebuild::VehicleRole::wheel_shapes`]),
    /// else left out.
    Vehicle { wheels: bool },
    /// A scripted flyer, built from its collision or its own mass
    /// points, and with the source inertia tensor when it has one.
    ScriptedFlyer,
    /// A rigid crate that falls under gravity.
    Crate,
    /// One body per collision node, riding that node.
    Device,
    /// As a device, with a biped's rigid body words
    /// ([`crate::rebuild::BIPED_BODY`]); no stand-in.
    Biped,
}

/// The ragdoll constraints of a built biped: its bodies joined over
/// Halo 1's model nodes ([`crate::ragdoll`]), the frames from the nodes'
/// rest transforms. Empty when the skeleton does not map.
pub fn biped_ragdoll(
    nodes: &[h1models::h1::Node],
    bodies: &[crate::rebuild::Body],
) -> Vec<crate::ragdoll::Constraint> {
    let named: Vec<(&str, i16)> = nodes.iter().map(|n| (n.name.as_str(), n.parent)).collect();
    let body_nodes: Vec<i16> = bodies.iter().map(|b| b.node).collect();
    let Some(links) = crate::ragdoll::links(&named, &body_nodes) else {
        return Vec::new();
    };
    let parents: Vec<i16> = nodes.iter().map(|n| n.parent).collect();
    let rest: Vec<crate::physics::Transform> = nodes
        .iter()
        .map(|n| crate::physics::Transform {
            r: h1models::mode::quat_matrix(n.rotation),
            t: n.translation,
        })
        .collect();
    crate::ragdoll::constraints(&links, &body_nodes, &parents, &rest)
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
    rewrite_audio(b, LOCAL, ids);
}

/// Rewrites every placeholder of `kind` (`LOCAL`, `SOUND`, `LOOP`) in `b`
/// to its datum in `ids`; one past the end becomes null.
fn rewrite_audio(b: &mut Block, kind: u32, ids: &[TagId]) {
    for f in &mut b.fixups {
        match &mut f.kind {
            FixupKind::Block { child, .. } | FixupKind::Data(child) | FixupKind::Pointer(child) => {
                rewrite_audio(child, kind, ids)
            }
            FixupKind::TagId(t) | FixupKind::TagRef(t) if t.0 & 0xffff_0000 == kind => {
                *t = ids
                    .get(t.0 as usize & 0xffff)
                    .copied()
                    .unwrap_or(TagId::NULL)
            }
            _ => {}
        }
    }
}

impl Tree {
    /// Adds the tree's sounds and looping sounds to `set` (with its sound
    /// gestalt) and points the tags' placeholders at them. A set that has
    /// a tag of the path already keeps it. When the gestalt refuses them
    /// (16-bit indices full) or `add` is false, none is added and every
    /// placeholder becomes null ([`Tree::audio_notes`] says why).
    /// Done once: the sounds are taken out of the tree.
    pub fn install_audio(&mut self, set: &mut TagSet, add: bool) {
        let (sounds, loops) = self.audio_ids(set, add);
        self.resolve_audio(&sounds, &loops);
    }

    /// Install all object audio or return the error to a campaign conversion.
    /// The caller discards its in-progress tag set on failure.
    pub fn try_install_audio(&mut self, set: &mut TagSet) -> Result<()> {
        let (sounds, loops) = self.add_audio(set).map_err(Error::Invalid)?;
        self.resolve_audio(&sounds, &loops);
        self.sounds.clear();
        self.loops.clear();
        Ok(())
    }

    fn resolve_audio(&mut self, sounds: &[TagId], loops: &[TagId]) {
        for (_, t) in &mut self.tags {
            rewrite_audio(&mut t.root, SOUND, sounds);
            rewrite_audio(&mut t.root, LOOP, loops);
        }
    }

    /// Adds the sounds and looping sounds to `set` (all or none) and
    /// returns their datums, in [`Tree::sounds`] and [`Tree::loops`]
    /// order; empty when none was added. Takes them out of the tree.
    fn audio_ids(&mut self, set: &mut TagSet, add: bool) -> (Vec<TagId>, Vec<TagId>) {
        let got = if self.sounds.is_empty() && self.loops.is_empty() {
            Ok((Vec::new(), Vec::new()))
        } else if add {
            // All or none: a refusal leaves `set` as it was.
            let mut trial = set.clone();
            self.add_audio(&mut trial).map(|ids| {
                *set = trial;
                ids
            })
        } else {
            Err("the tags take over existing slots: no room for sounds".to_owned())
        };
        let (sounds, loops) = match got {
            Ok(ids) => ids,
            Err(e) => {
                self.audio_notes.push(format!(
                    "{} sounds and {} looping sounds left out: {e}",
                    self.sounds.len(),
                    self.loops.len()
                ));
                (Vec::new(), Vec::new())
            }
        };
        self.sounds.clear();
        self.loops.clear();
        (sounds, loops)
    }

    fn add_audio(&self, set: &mut TagSet) -> std::result::Result<(Vec<TagId>, Vec<TagId>), String> {
        let snd = FourCC(*b"snd!");
        let new: Vec<(String, h1sounds::ConvertedSound)> = self
            .sounds
            .iter()
            .filter(|(n, _)| set.find(snd, n).is_none())
            .cloned()
            .collect();
        if !new.is_empty() {
            h1sounds::install_sounds(set, new).map_err(|e| e.to_string())?;
        }
        let sounds = self
            .sounds
            .iter()
            .map(|(n, _)| set.find(snd, n).ok_or(format!("{n}: not installed")))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let loops = h1sounds::looping::install_looping(set, &self.loops).map_err(|e| e.to_string())?;
        Ok((sounds, loops))
    }
}

impl Tree {
    /// Keeps the role values ([`crate::provenance::capture`]) of the tree's
    /// tag `id` (a [`local`] datum); nothing for any other datum.
    pub fn note_roles(&mut self, id: TagId, fields: Vec<crate::provenance::RoleField>) {
        if let Some(k) = local_index(id)
            && !fields.is_empty()
        {
            match self.role_fields.iter_mut().find(|e| e.0 == k) {
                Some(e) => e.1.extend(fields),
                None => self.role_fields.push((k, fields)),
            }
        }
    }

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
    /// on), in order, in the slots after the set's last, and returns their
    /// datums; then the sounds after them ([`Tree::install_audio_for`]).
    pub fn install(mut self, set: &mut TagSet) -> Result<Vec<TagId>> {
        let ids = self.install_tags(set)?;
        self.install_audio_for(set, &ids);
        Ok(ids)
    }

    /// The tags alone of [`Tree::install`]: added in order in the slots
    /// after the set's last, their datums returned, their sound
    /// placeholders left for [`Tree::install_audio_for`]. The Halo 1
    /// sandbox adds its collections between the two, so the tags and the
    /// collections hold the datums its placements name.
    pub fn install_tags(&mut self, set: &mut TagSet) -> Result<Vec<TagId>> {
        let first = set.slots.len();
        let ids: Vec<TagId> = (0..self.tags.len())
            .map(|k| {
                let slot = first + k;
                TagId(u32::from(set.salt_base.wrapping_add(slot as u16)) << 16 | slot as u32)
            })
            .collect();
        for (k, (name, mut tag)) in std::mem::take(&mut self.tags).into_iter().enumerate() {
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

    /// Adds the sounds and looping sounds after whatever `set` holds now
    /// and points the placeholders of the tags installed at `ids`
    /// ([`Tree::install_tags`]) at them.
    pub fn install_audio_for(&mut self, set: &mut TagSet, ids: &[TagId]) {
        let (sounds, loops) = self.audio_ids(set, true);
        for id in ids {
            if let Some(TagPayload::Built(root)) =
                set.slots[id.index()].tag.as_mut().map(|e| &mut e.payload)
            {
                rewrite_audio(root, SOUND, &sounds);
                rewrite_audio(root, LOOP, &loops);
            }
        }
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
    pub fn install_over(mut self, set: &mut TagSet, donors: &[TagId]) -> Result<Vec<TagId>> {
        self.install_audio(set, false);
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
    /// The point's object-space position (x forward, y left).
    object: [f32; 3],
}

/// The order to write a vehicle's friction points in: point `i` of the
/// result rides the role's wheel sphere `i` ([`crate::rebuild::role_spheres`]).
/// Each sphere in turn takes the nearest point not yet taken (object
/// space); points past the last sphere follow in their own order. V: the
/// engine reads friction point `i`'s suspension from the phantom's sphere
/// `i` (the per-point loop at `0x2067dd` takes its compression from
/// contact slot `i`, `0x207ab0`), so a point paired with a sphere at the
/// other end of the vehicle pushes up where the other end sinks. Halo 1's
/// warthog lists its back tires first and the retail spheres run front
/// first; so paired, the converted warthog pitched onto its nose at spawn
/// (forward z -0.93) and stood level once paired by position (forward z
/// 0.08 and 0.14 on Blood Gulch's slopes, both warthogs).
pub fn sphere_order(points: &[[f32; 3]], spheres: &[[f32; 3]]) -> Vec<usize> {
    let mut left: Vec<usize> = (0..points.len()).collect();
    let mut out = Vec::with_capacity(points.len());
    for s in spheres {
        let d = |i: &usize| (0..3).map(|a| (points[*i][a] - s[a]).powi(2)).sum::<f32>();
        let Some(k) = (0..left.len()).min_by(|a, b| d(&left[*a]).total_cmp(&d(&left[*b]))) else {
            break;
        };
        out.push(left.remove(k));
    }
    out.extend(left);
    out
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
fn get_f32(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
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
    fn row(&mut self, field: &str, halo1: String, halo2: String, template: String) {
        self.rep.tuning.push(Tuning {
            field: format!("{}{field}", self.prefix),
            halo1,
            halo2,
            template,
        });
    }
}

/// The `hlmt` field a rebuilt biped's converted dialogue goes in: the
/// model's dialogue (`+0x8c`, `udlg`; V: where retail models name theirs,
/// `docs/formats/objects.md`).
pub const DIALOGUE_FIELD: &str = "udlg_ref_8c";

/// A rebuilt unit's material effects (`obje +0x54`, `foot`).
pub const FOOT_FIELD: &str = "unit.obje.foot_ref_54";

/// How the [`Converter`] builds tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// The campaign HUD supplies the unit panels' authored tint at draw time.
    pub unit_hud_draw_colour: bool,
    /// Build bipeds from the `bipd` definition ([`crate::rebuild::biped`])
    /// instead of on a template: Halo 1's model, collision size and
    /// physics, the role's words, and the references of the Halo 2
    /// counterpart ([`crate::rebuild::COUNTERPARTS`]). On by default; off
    /// builds them on templates.
    pub rebuild_bipeds: bool,
    /// The most sample bytes the rebuilt bipeds' dialogue
    /// ([`Converter::dialogue`]) may add in sounds no other object
    /// plays; a line whose sound would pass it is left out. `u64::MAX`
    /// (the default) for all, 0 for none.
    pub dialogue_budget: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            unit_hud_draw_colour: false,
            rebuild_bipeds: true,
            dialogue_budget: u64::MAX,
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
    /// Sample bytes of the sounds first converted for dialogue
    /// ([`Self::dialogue`]).
    pub dialogue_bytes: u64,
    done: HashMap<u32, TagId>,
    /// Render models by source tag and weapon counter's loaded maximum.
    render_models: HashMap<(h1map::TagId, Option<i16>), TagId>,
    colls: HashMap<(u32, bool), TagId>,
    coll_names: HashMap<(u32, bool), Vec<(String, Vec<String>)>>,
    /// Per converted `coll`, the `hlmt` materials it needs
    /// ([`crate::rebuild::model_materials`]).
    coll_materials: HashMap<(u32, bool), Vec<crate::rebuild::ModelMaterial>>,
    /// Each template `nhdt` given the health meter.
    health_huds: HashMap<TagId, TagId>,
    /// Widgets per converted `unhi` ([`Self::unit_hud`]).
    unit_hud_widgets: HashMap<u32, usize>,
    /// Per converted `wphi`, its entries with no widget
    /// ([`Self::weapon_hud`]).
    weapon_hud_skips: HashMap<u32, Vec<String>>,
    /// HUD bitmaps with an element colour's alpha ([`Self::hud_bitmap`]).
    hud_bitmaps: HashMap<(u32, u8), TagId>,
    /// Copies of a bitmap whose sequence starts at a later sprite, by
    /// (bitmap, sequence, sprite) ([`Self::sprite_bitmap`]).
    sprite_bitmaps: HashMap<(u32, i16, usize), TagId>,
    /// Meter copies of Halo 1 bitmaps ([`Self::meter_bitmap`]), by tag.
    meter_bitmaps: HashMap<u32, TagId>,
    /// Base maps with a reflection mask as their alpha
    /// ([`Self::masked_bitmap`]), by (base, mask) tag.
    masked_bitmaps: HashMap<(u32, u32), Option<TagId>>,
    /// Plasma pictures with their brightness as alpha
    /// ([`Self::brightest_bitmap`]), by (bitmap tag, gain bits).
    brightest_bitmaps: HashMap<(u32, u32), Option<TagId>>,
    /// Flat bump maps cut out by a base map's alpha
    /// ([`Self::cutout_bump`]), by base map tag.
    cutout_bumps: HashMap<u32, Option<TagId>>,
    /// Change-colour maps ([`Self::change_colour_bitmap`]), by
    /// (multipurpose map, Halo 2 slot).
    change_colour_bitmaps: HashMap<(u32, usize), Option<TagId>>,
    /// Ammunition counter digits ([`Self::counter_shader`]), by (Halo 1
    /// `schi`, permutation, loaded maximum).
    counter_shaders: HashMap<(u32, i16, Option<i16>), Option<TagId>>,
    /// Biped role words, per role ([`Self::biped_role`]).
    roles: HashMap<crate::rebuild::BipedRole, Option<crate::rebuild::RoleWords>>,
    /// Weapon, equipment, projectile and damage role words, per group
    /// ([`crate::arsenal::role`]).
    item_roles: HashMap<[u8; 4], crate::arsenal::Role>,
    /// Vehicle role words, per vehicle type and powered point count
    /// ([`Self::vehicle_role`]).
    vehicle_roles: HashMap<(u16, usize), Option<crate::rebuild::VehicleRole>>,
    /// Converted sounds and looping sounds by Halo 1 tag, as indices
    /// into [`Tree::sounds`] and [`Tree::loops`] (`None`: did not convert).
    sound_index: HashMap<u32, Option<usize>>,
    loop_index: HashMap<u32, Option<usize>>,
    /// Light role words per kind ([`crate::rebuild::light_role`]).
    light_roles: HashMap<u32, Option<crate::rebuild::LightRole>>,
    /// The base's decal role ([`crate::rebuild::decal_role`]), once.
    decal_role: Option<Option<crate::rebuild::DecalRole>>,
    /// Decals being converted (a next-decal chain that loops ends there).
    decals_open: Vec<u32>,
    /// The base's particle role ([`crate::particles::particle_role`]), once.
    particle_role: Option<Option<crate::particles::ParticleRole>>,
    /// The base's emitter role ([`crate::particles::emitter_role`]), once.
    emitter_role: Option<Option<crate::particles::EmitterRole>>,
    /// Effects being converted, including those reached through an object
    /// the effect itself creates.
    effects_open: Vec<u32>,
    /// Objects being converted ([`Self::object`]), so one whose effects
    /// lead back to it is refused instead of recursed into.
    objects_open: Vec<u32>,
    /// Per Halo 1 `pctl`, its emitter types and converted particle states.
    pctl_done: HashMap<u32, particles_conv::ParticleTypes>,
    /// Looping effects that carry a particle system at a given object marker.
    attached_particles: HashMap<(u32, String), Vec<TagId>>,
    /// Scenery role words ([`Self::scenery_role`]), computed once.
    scenery_role: Option<Option<crate::rebuild::SceneryRole>>,
    /// Per Halo 1 weapon label, the weapon class the map's graphs hold it
    /// under ([`Self::weapon_class`]), computed once.
    weapon_classes: Option<HashMap<String, String>>,
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
            render_models: HashMap::new(),
            colls: HashMap::new(),
            coll_names: HashMap::new(),
            coll_materials: HashMap::new(),
            health_huds: HashMap::new(),
            unit_hud_widgets: HashMap::new(),
            weapon_hud_skips: HashMap::new(),
            hud_bitmaps: HashMap::new(),
            sprite_bitmaps: HashMap::new(),
            meter_bitmaps: HashMap::new(),
            masked_bitmaps: HashMap::new(),
            brightest_bitmaps: HashMap::new(),
            cutout_bumps: HashMap::new(),
            change_colour_bitmaps: HashMap::new(),
            counter_shaders: HashMap::new(),
            roles: HashMap::new(),
            item_roles: HashMap::new(),
            vehicle_roles: HashMap::new(),
            sound_index: HashMap::new(),
            loop_index: HashMap::new(),
            light_roles: HashMap::new(),
            decal_role: None,
            particle_role: None,
            emitter_role: None,
            effects_open: Vec::new(),
            objects_open: Vec::new(),
            pctl_done: HashMap::new(),
            attached_particles: HashMap::new(),
            decals_open: Vec::new(),
            dialogue_bytes: 0,
            scenery_role: None,
            weapon_classes: None,
        }
    }

    /// A Halo 1 dialogue tag as a Halo 2 `udlg`
    /// ([`crate::rebuild::dialogue`]): one vocalization per slot of
    /// [`crate::rebuild::DIALOGUE`] holding a sound, the sound converted
    /// with `h1sounds`, within [`Options::dialogue_budget`]. Converted
    /// once; `None` when no line converts.
    pub fn dialogue(&mut self, tag: &Tag, rep: &mut ObjectReport) -> Result<Option<TagId>> {
        if let Some(id) = self.done.get(&tag.id.0) {
            return Ok(Some(*id));
        }
        let d = h1::Dialogue::read(self.h1, tag)?;
        let wanted = crate::rebuild::dialogue_lines(&d);
        let mut lines = Vec::new();
        let mut over = 0;
        for (voc, _, s) in &wanted {
            let st = self.h1_tag(*s)?;
            match self.dialogue_sound(st, rep) {
                Some(id) => lines.push((*voc, id)),
                None => over += 1,
            }
        }
        rep.no_home.push(format!(
            "udlg {}: {} of {} slots holding a sound as Halo 2 vocalizations ({} lines; {over} left out)",
            tag.name,
            wanted.len(),
            d.lines.len(),
            lines.len(),
        ));
        if lines.is_empty() {
            return Ok(None);
        }
        let root = crate::rebuild::dialogue(&lines)?;
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"udlg"),
                root,
                raw: Vec::new(),
            },
        );
        self.done.insert(tag.id.0, id);
        Ok(Some(id))
    }

    /// A Halo 1 `foot` as a Halo 2 one ([`crate::rebuild::foot`]): its
    /// first [`crate::rebuild::FOOT_KINDS`] kinds at the same index, each
    /// material's effect and sound under the Halo 2 global material
    /// [`crate::arsenal::MATERIALS`] names for it (the first Halo 1
    /// material wins where two share one). Converted once.
    pub fn foot(&mut self, tag: &Tag, rep: &mut ObjectReport) -> Result<TagId> {
        if let Some(id) = self.done.get(&tag.id.0) {
            return Ok(*id);
        }
        let f = h1::Foot::read(self.h1, tag)?;
        let mut kinds = Vec::new();
        let (mut missing, mut failed) = (Vec::new(), 0usize);
        for k in f.kinds.iter().take(crate::rebuild::FOOT_KINDS) {
            let mut out = crate::rebuild::FootKind::default();
            for (m, (_, name)) in k.iter().zip(crate::arsenal::MATERIALS.iter()) {
                let Some(index) = self.base.materials.iter().position(|x| x == name) else {
                    if (m.effect.is_some() || m.sound.is_some()) && !missing.contains(name) {
                        missing.push(*name);
                    }
                    continue;
                };
                let line = |tag| crate::rebuild::FootLine {
                    material: name,
                    index: index as i16,
                    tag,
                };
                if let Some(s) = m.sound
                    && !out.sounds.iter().any(|l| l.material == *name)
                {
                    match self.sound(self.h1_tag(s)?, rep) {
                        Some(id) => out.sounds.push(line(id)),
                        None => failed += 1,
                    }
                }
                if let Some(e) = m.effect
                    && !out.effects.iter().any(|l| l.material == *name)
                {
                    match self.effect(self.h1_tag(e)?) {
                        Ok(id) => out.effects.push(line(id)),
                        Err(err) => {
                            failed += 1;
                            rep.no_home.push(format!("foot effect: not converted ({err})"));
                        }
                    }
                }
            }
            kinds.push(out);
        }
        rep.no_home.push(format!(
            "foot {}: {} kinds, {} sounds and {} effects by Halo 2 material; {failed} did not convert; materials not in the base: {missing:?}",
            tag.name,
            kinds.len(),
            kinds.iter().map(|k| k.sounds.len()).sum::<usize>(),
            kinds.iter().map(|k| k.effects.len()).sum::<usize>(),
        ));
        let root = crate::rebuild::foot(&kinds)?;
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"foot"),
                root,
                raw: Vec::new(),
            },
        );
        self.done.insert(tag.id.0, id);
        Ok(id)
    }

    /// A rebuilt unit's root (`group`) with its object's material effects
    /// (`obje +0x54`) Halo 1's ([`Self::foot`]): the first `foot` the
    /// Halo 1 unit names, in place of the counterpart's. Unchanged when it
    /// names none.
    fn unit_foot(&mut self, group: [u8; 4], root: Block, u: &Unit, rep: &mut ObjectReport) -> Result<Block> {
        let Some(t) = u
            .references
            .iter()
            .find(|r| r.group == h1map::Group::new(b"foot"))
            .and_then(|r| r.tag)
        else {
            return Ok(root);
        };
        let id = self.foot(self.h1_tag(t)?, rep)?;
        let d = tags::defs::Defs::builtin()
            .group(FourCC(group))
            .ok_or_else(|| Error::Invalid("no unit definition".into()))?;
        let mut r = tags::defs::from_built(d, &root).map_err(|e| Error::Invalid(e.to_string()))?;
        r.set(FOOT_FIELD, tags::defs::Val::tag(FourCC(*b"foot"), id))
            .map_err(|e| Error::Invalid(e.to_string()))?;
        tags::defs::to_built(d, &r, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))
    }

    /// [`Self::sound`] for a dialogue line: a sound not yet converted is
    /// taken only while [`Self::dialogue_bytes`] stays within the budget.
    fn dialogue_sound(&mut self, tag: &Tag, rep: &mut ObjectReport) -> Option<TagId> {
        if let Some(k) = self.sound_index.get(&tag.id.0) {
            return k.map(sound);
        }
        let c = match h1sounds::Sound::read(self.h1, tag)
            .and_then(|s| h1sounds::convert_sound(&s, self.h1.bytes()))
        {
            Ok(c) => c,
            Err(e) => {
                rep.no_home.push(format!("snd! {}: not converted ({e})", tag.name));
                self.sound_index.insert(tag.id.0, None);
                return None;
            }
        };
        let size: u64 = c
            .ranges
            .iter()
            .flat_map(|r| &r.permutations)
            .map(|p| p.size() as u64)
            .sum();
        if self.dialogue_bytes + size > self.options.dialogue_budget {
            return None;
        }
        self.dialogue_bytes += size;
        self.tree.sounds.push((tag.name.clone(), c));
        let k = self.tree.sounds.len() - 1;
        self.sound_index.insert(tag.id.0, Some(k));
        Some(sound(k))
    }

    /// A Halo 1 decal from the `deca` definition
    /// ([`crate::rebuild::decal`]), converted once, with its next decal and
    /// its bitmap (`h1bitmaps`).
    pub fn decal(&mut self, tag: &Tag) -> Result<TagId> {
        if let Some(id) = self.done.get(&tag.id.0) {
            return Ok(*id);
        }
        let x = h1::Decal::read(self.h1, tag)?;
        self.decals_open.push(tag.id.0);
        let next = match x.next {
            Some(n) if !self.decals_open.contains(&n.0) => {
                let t = self.h1_tag(n)?;
                Some(self.decal(t)?)
            }
            _ => None,
        };
        self.decals_open.pop();
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        let bitmap = x.bitmap.and_then(|b| self.bitmap(b));
        if x.bitmap.is_some() && bitmap.is_none() {
            rep.no_home.push("bitmap: did not convert".into());
        }
        if self.decal_role.is_none() {
            self.decal_role = Some(crate::rebuild::decal_role(self.base)?);
        }
        let (root, roles) = crate::provenance::capture(|| {
            crate::rebuild::decal(&x, next, bitmap, self.decal_role.flatten().as_ref())
        });
        let root = root?;
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"deca"),
                root,
                raw: Vec::new(),
            },
        );
        self.tree.note_roles(id, roles);
        self.done.insert(tag.id.0, id);
        Ok(id)
    }

    /// A Halo 1 light from the `ligh` definition
    /// ([`crate::rebuild::light`]), converted once.
    pub fn light(&mut self, tag: &Tag) -> Result<TagId> {
        if let Some(id) = self.done.get(&tag.id.0) {
            return Ok(*id);
        }
        let l = h1::Light::read(self.h1, tag)?;
        let kind = if l.is_cone() {
            crate::rebuild::LIGHT_FRUSTUM
        } else {
            crate::rebuild::LIGHT_SPHERE
        };
        if !self.light_roles.contains_key(&kind) {
            let role = crate::rebuild::light_role(self.base, kind)?;
            self.light_roles.insert(kind, role);
        }
        let role = self.light_roles[&kind].as_ref();
        let (root, roles) = crate::provenance::capture(|| crate::rebuild::light(&l, role));
        let mut root = root?;
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        if let Some(lens) = self.flare_for_light(l.lens_flare, &mut rep) {
            root = flares_conv::light_with_flare(&root, lens)?;
        }
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"ligh"),
                root,
                raw: Vec::new(),
            },
        );
        self.tree.note_roles(id, roles);
        self.done.insert(tag.id.0, id);
        Ok(id)
    }

    /// A Halo 1 `snd!` converted with `h1sounds` into [`Tree::sounds`]
    /// (once), as its placeholder ([`sound`]); `None` when it does not
    /// convert (the reason in `rep`).
    pub fn sound(&mut self, tag: &Tag, rep: &mut ObjectReport) -> Option<TagId> {
        if let Some(k) = self.sound_index.get(&tag.id.0) {
            return k.map(sound);
        }
        let got = h1sounds::Sound::read(self.h1, tag)
            .and_then(|s| h1sounds::convert_sound(&s, self.h1.bytes()));
        let k = match got {
            Ok(c) => {
                self.tree.sounds.push((tag.name.clone(), c));
                Some(self.tree.sounds.len() - 1)
            }
            Err(e) => {
                rep.no_home.push(format!("snd! {}: not converted ({e})", tag.name));
                None
            }
        };
        self.sound_index.insert(tag.id.0, k);
        k.map(sound)
    }

    /// A Halo 1 `lsnd` read and converted with `h1sounds::looping` into
    /// [`Tree::loops`] (once), as its placeholder ([`looping`]).
    pub fn looping_sound(&mut self, tag: &Tag, rep: &mut ObjectReport) -> Option<TagId> {
        if let Some(k) = self.loop_index.get(&tag.id.0) {
            return k.map(looping);
        }
        let k = match h1sounds::looping::read_looping(self.h1, tag) {
            Ok(l) => {
                rep.no_home.extend(l.notes.iter().map(|n| format!("lsnd {}: {n}", tag.name)));
                self.tree.loops.push(l);
                Some(self.tree.loops.len() - 1)
            }
            Err(e) => {
                rep.no_home.push(format!("lsnd {}: not converted ({e})", tag.name));
                None
            }
        };
        self.loop_index.insert(tag.id.0, k);
        k.map(looping)
    }

    /// A Halo 1 effect from the `effe` definition
    /// ([`crate::rebuild::effect`]), converted once. Its parts keep what
    /// converts here: sounds, projectiles, decals, lights, debris and
    /// particle systems. Parts without a converter are listed in the report.
    pub fn effect(&mut self, tag: &Tag) -> Result<TagId> {
        if let Some(id) = self.done.get(&tag.id.0) {
            return Ok(*id);
        }
        if self.effects_open.contains(&tag.id.0) {
            return Err(Error::Invalid(format!(
                "{}: converting it needs itself",
                tag.name
            )));
        }
        self.effects_open.push(tag.id.0);
        let got = self.effect_uncached(tag);
        self.effects_open.pop();
        let id = got?;
        self.done.insert(tag.id.0, id);
        Ok(id)
    }

    fn effect_uncached(&mut self, tag: &Tag) -> Result<TagId> {
        let e = h1::Effect::read(self.h1, tag)?;
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        let mut left: std::collections::BTreeMap<String, usize> = Default::default();
        let mut targets = Vec::new();
        for p in e.events.iter().flat_map(|ev| &ev.parts) {
            let g = p.group.0.to_be_bytes();
            let got = match (p.reference, &g) {
                (None, _) => None,
                (Some(id), b"snd!") => {
                    let t = self.h1_tag(id)?;
                    self.sound(t, &mut rep)
                }
                (Some(id), b"deca") => {
                    let t = self.h1_tag(id)?;
                    match self.decal(t) {
                        Ok(x) => Some(x),
                        Err(err) => {
                            rep.no_home.push(format!("deca {}: not converted ({err})", t.name));
                            None
                        }
                    }
                }
                (Some(id), b"ligh") => {
                    let t = self.h1_tag(id)?;
                    match self.light(t) {
                        Ok(x) => Some(x),
                        Err(err) => {
                            rep.no_home.push(format!("ligh {}: not converted ({err})", t.name));
                            None
                        }
                    }
                }
                (Some(id), b"garb") => self.effect_garbage(id, &mut rep),
                (Some(id), b"proj") => {
                    let t = self.h1_tag(id)?;
                    match self.object(t) {
                        Ok(x) => Some(x),
                        Err(err) => {
                            rep.no_home.push(format!("proj {}: not converted ({err})", t.name));
                            None
                        }
                    }
                }
                _ => None,
            };
            // Particle systems become the event's (Self::effect_particle_systems).
            if got.is_none() && p.group != h1map::Group::new(b"pctl") {
                *left.entry(p.group.to_string()).or_default() += 1;
            }
            targets.push(got.map(|tag| crate::rebuild::PartTarget {
                type_group: crate::rebuild::part_type_group(g),
                group: g,
                tag,
            }));
        }
        let mut next = targets.into_iter();
        let root = crate::rebuild::effect(&e, &mut |_| next.next().flatten())?;
        for (g, n) in left {
            rep.no_home.push(format!("{n} {g} parts left out (no converter here, or did not convert)"));
        }
        let (root, roles) =
            crate::provenance::capture(|| self.effect_particle_systems(tag, root, &mut rep));
        let root = root?;
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"effe"),
                root,
                raw: Vec::new(),
            },
        );
        self.tree.note_roles(id, roles);
        Ok(id)
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
        // An object whose conversion reaches itself (a projectile whose
        // detonation effect creates it) is refused there, not recursed into.
        if self.objects_open.contains(&tag.id.0) {
            return Err(Error::Invalid(format!(
                "{}: converting it needs itself",
                tag.name
            )));
        }
        self.objects_open.push(tag.id.0);
        let got = match &tag.group.0.to_be_bytes() {
            b"weap" => self.weapon(tag),
            b"proj" => self.projectile(tag, template),
            b"eqip" => self.equipment(tag),
            b"garb" => self.garbage_uncached(tag),
            b"vehi" if scripted_props::replacement_group(&tag.name).is_some() => {
                self.scripted_prop(tag)
            }
            b"bipd" | b"vehi" => self.unit(tag),
            b"scen" => self.scenery(tag),
            b"ssce" => self.sound_scenery(tag),
            b"mach" | b"ctrl" | b"lifi" => self.device(tag),
            _ => Err(Error::WrongGroup(tag.group.to_string(), *b"obje")),
        };
        self.objects_open.pop();
        let id = got?;
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
        let hlmt = self.model(rep, obj, template_hlmt, name, false, None)?;
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
    /// `rounds` is the weapon's loaded maximum when its counters count
    /// the rounds loaded ([`h1::Weapon::rounds_counter`]).
    fn model(
        &mut self,
        rep: &mut ObjectReport,
        obj: &h1::Object,
        template_hlmt: Option<TagId>,
        name: &str,
        rebuild: bool,
        rounds: Option<i16>,
    ) -> Result<Option<TagId>> {
        let Some(mode_id) = obj.model else {
            return Ok(None);
        };
        let scenery = h1::OBJECT_TYPES.get(obj.object_type as usize) == Some(&b"scen");
        let mode_tag = self.h1_tag(mode_id)?;
        let (mut h1_model, _) =
            h1models::model_from_map(self.h1, mode_tag, &ModelOptions::default())?;
        let key = (mode_id, rounds);
        let mode = match self.render_models.get(&key) {
            Some(&mode) => mode,
            None => {
                // A vehicle's powered mass points become markers, which
                // its friction or hover points name (crate::rebuild::Point).
                if h1::OBJECT_TYPES.get(obj.object_type as usize) == Some(&b"vehi") {
                    let points = self.powered_points(obj, &h1_model)?;
                    add_point_markers(&mut h1_model, &points);
                }
                let mut shaders = self.model_shaders(rep, &h1_model, rounds);
                // A player biped's body carries the player's emblem, as the
                // Chief's does (h1models::emblem); after model_shaders, so
                // its shader entry (no Halo 1 tag) is not looked up there.
                let player = h1::OBJECT_TYPES.get(obj.object_type as usize) == Some(&b"bipd")
                    && health_hud::is_player_biped(name);
                if player {
                    self.emblem(rep, &mut h1_model, &mut shaders);
                }
                // One material per Halo 1 shader, as convert_model writes.
                let mut padded = shaders.clone();
                padded.resize(h1_model.shaders.len(), TagId::NULL);
                let opts = ModelOptions {
                    shaders,
                    ..ModelOptions::default()
                };
                let mut mode = h1models::mode::convert_model(
                    &h1_model,
                    h1models::short_name(&mode_tag.name),
                    &opts,
                )?;
                if rebuild {
                    crate::rebuild::mode_materials(&mut mode.root, &padded)?;
                }
                let mode = self.tree.push(&mode_tag.name, mode);
                self.render_models.insert(key, mode);
                mode
            }
        };
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
        // Retail writes the coll's BSP physics slots (crate::coll) exactly
        // for scenery with no phmo: its weapons, equipment, devices and
        // bipeds without a phmo have none. Converted scenery has no phmo:
        // the engine gives scenery no Havok body, so a placed scenery
        // object's collision joins the level's instead
        // (h1convert::scenery_collision).
        let slots = scenery && !has_phmo;
        let coll = match obj.collision {
            Some(c) => {
                let t = self.h1_tag(c)?;
                Some(self.collision(rep, t, slots)?)
            }
            None => None,
        };
        let jmad = match obj.animation {
            Some(a) => {
                let t = self.h1_tag(a)?;
                match self.done.get(&t.id.0) {
                    Some(id) => Some(*id),
                    None => {
                        let vehicle =
                            h1::OBJECT_TYPES.get(obj.object_type as usize) == Some(&b"vehi");
                        let opts = h1models::jmad::GraphOptions {
                            mass_point_markers: if vehicle {
                                self.mass_point_markers(obj)?
                            } else {
                                Vec::new()
                            },
                        };
                        let (_, report, conv) =
                            h1models::graph_from_map_with(self.h1, t, Some(mode_tag), &opts)?;
                        for a in &report.suspension_without_marker {
                            rep.no_home.push(format!(
                                "suspension {a:?}: its mass point has no marker; left out"
                            ));
                        }
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
        // The materials of the converted coll, not the template's: a
        // hit's material index reads them (crate::rebuild::HLMT_MATERIALS).
        // A borrowed phmo's shapes index the template's materials: those
        // past the coll's stay, each named as its global material.
        let mut materials = coll.map_or_else(Vec::new, |c| self.coll_materials[&c.1].clone());
        if has_phmo {
            let mut k = materials.len();
            while let Some(e) = self.base.element(&hlmt.bytes, crate::rebuild::HLMT_MATERIALS as usize, crate::rebuild::HLMT_MATERIAL_SIZE, k)? {
                let i = i16::from_le_bytes([e[0x10], e[0x11]]);
                let g = usize::try_from(i)
                    .ok()
                    .and_then(|i| self.base.materials.get(i))
                    .ok_or_else(|| Error::Invalid(format!("{name}: template hlmt material {k}: global {i}")))?;
                materials.push(crate::rebuild::model_material(g, g, &self.base.materials)?);
                k += 1;
            }
        }
        crate::rebuild::set_model_materials(&mut hlmt, &materials);
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
        // Scenery's unknown spans take the role's words; other groups'
        // stay zero (their lanes own them).
        let words = if h1::OBJECT_TYPES.get(obj.object_type as usize) == Some(&b"scen") {
            self.scenery_role()?.map(|r| r.hlmt).unwrap_or_default()
        } else {
            Vec::new()
        };
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
        let (root, roles) = crate::provenance::capture(|| {
            crate::rebuild::model(&crate::rebuild::Model {
                render: mode,
                collision: coll.map(|c| c.0),
                animation: jmad,
                physics: None,
                nodes: (count, nodes),
                regions,
                words,
                materials: coll.map_or_else(Vec::new, |c| self.coll_materials[&c.1].clone()),
            })
        });
        let root = root?;
        if obj.animation.is_none() {
            rep.no_home
                .push("no Halo 1 animation graph on the object; hlmt animation left null".into());
        }
        let id = self.tree.push(
            name,
            Converted {
                group: FourCC(*b"hlmt"),
                root,
                raw: Vec::new(),
            },
        );
        self.tree.note_roles(id, roles);
        Ok(Some(id))
    }

    /// Scenery from the `scen` definition ([`crate::rebuild::scenery`]),
    /// no template: every scenery object (the template path was removed in
    /// converter version 50). Its `hlmt` and `mode` materials are written
    /// from their definitions; its unknown spans take the base's scenery
    /// role words ([`crate::rebuild::SceneryRole`]). Its `coll` has BSP
    /// physics slots, as retail static scenery with no `phmo`; neither
    /// makes scenery solid (the engine gives scenery no Havok body), so
    /// each placement's collision is merged into the level's
    /// (`h1convert::scenery_collision`; docs/halo1/objects.md, "Collision
    /// for Havok").
    fn scenery(&mut self, tag: &Tag) -> Result<TagId> {
        let bytes = self.h1.tag_data(tag)?;
        let mut object = h1::Object::read(self.h1, bytes)?;
        self.halo2_change_colours(&mut object);
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        let hlmt = self.model(&mut rep, &object, None, &tag.name, true, None)?;
        rep.physics = Some(match object.collision {
            Some(_) => {
                "none: the coll's BSP physics slots; placements' collision joins the level's".into()
            }
            None => "none (no Halo 1 collision)".into(),
        });
        let predicted = self.predicted(hlmt);
        let words = self.scenery_role()?.map(|r| r.scen).unwrap_or_default();
        let (root, roles) = crate::provenance::capture(|| {
            crate::rebuild::scenery(&object, hlmt, &predicted, &words)
        });
        let root =
            self.with_object_attachments(*b"scen", "object", root?, bytes, &object, &mut rep)?;
        let widgets = object.widgets.iter().flatten().count();
        if widgets > 0 {
            rep.no_home.push(format!(
                "obje widgets: {widgets} Halo 1 widgets (flags, antennas) not carried"
            ));
        }
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"scen"),
                root,
                raw: Vec::new(),
            },
        );
        self.tree.note_roles(id, roles);
        Ok(id)
    }

    /// The base map's scenery role words ([`crate::rebuild::scenery_role`]),
    /// computed once. `None` when the base holds no scenery: the unknown
    /// spans then stay zero.
    pub fn scenery_role(&mut self) -> Result<Option<crate::rebuild::SceneryRole>> {
        if let Some(r) = &self.scenery_role {
            return Ok(r.clone());
        }
        let r = crate::rebuild::scenery_role(self.base)?;
        self.scenery_role = Some(r.clone());
        Ok(r)
    }

    /// The weapon class a Halo 1 weapon `label` (its animation label,
    /// the units' weapon type) is held under: of every animation of every
    /// graph on the map named `<unit> <class> <label> <action>`
    /// ([`h1models::names::map_name`], which names the converted `jmad`
    /// tree too), the class most of them name (ties: the first by name).
    /// `None` when no graph holds the label.
    pub fn weapon_class(&mut self, label: &str) -> Result<Option<String>> {
        if self.weapon_classes.is_none() {
            let mut votes: HashMap<(String, String), usize> = HashMap::new();
            let antr = h1map::Group::new(b"antr");
            for t in self.h1.tags().iter().filter(|t| t.group == antr) {
                for name in h1models::graph_names(self.h1, t)? {
                    let n = h1models::names::map_name(&name);
                    if n.class != "any" && n.kind != "any" {
                        *votes.entry((n.kind, n.class)).or_default() += 1;
                    }
                }
            }
            let mut classes = weapon_classes(votes);
            // A label no animation is named for takes the class whose
            // slot table lists it as a type (V: the warthog's gun, `cg`,
            // under the warthog's `fixed`; the converted vehicle graph's
            // aim overlays are found under that class, h1models
            // `jmad::vehicle_plan`). Until converter version 84 the gun had
            // no class, and the warthog no aim. Vehicle graphs' tables only
            // (`h1models::h1::weapon_types`): a biped's converted graph holds
            // just the types its animation names hold.
            for t in self.h1.tags().iter().filter(|t| t.group == antr) {
                for (kind, class) in h1models::graph_weapon_types(self.h1, t)? {
                    if h1models::names::CLASSES.contains(&class.as_str()) {
                        classes.entry(kind).or_insert(class);
                    }
                }
            }
            self.weapon_classes = Some(classes);
        }
        Ok(self
            .weapon_classes
            .as_ref()
            .and_then(|m| m.get(label).cloned()))
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
        self.coll_materials.insert(
            key,
            crate::rebuild::model_materials(&h1c, &self.base.materials)?,
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

    /// Adds the player's emblem to a player biped's `model`
    /// ([`h1models::emblem::add_emblem`] at its default, the left
    /// shoulder) and names its material: the base's [`EMBLEM_SHADER`],
    /// which draws the emblem of the player being drawn. `shaders` (one
    /// per Halo 1 shader so far) gains it at the emblem's index. Without
    /// the shader in the base, or a spot on the model, the model is left
    /// as it is and the report says so.
    fn emblem(
        &mut self,
        rep: &mut ObjectReport,
        model: &mut h1models::h1::Model,
        shaders: &mut Vec<TagId>,
    ) {
        let Some(shader) = self.base.find(b"shad", EMBLEM_SHADER) else {
            rep.no_home.push(format!(
                "emblem: the base has no {EMBLEM_SHADER}; none drawn"
            ));
            return;
        };
        let at = &h1models::emblem::LEFT_SHOULDER;
        let Some(index) = h1models::emblem::add_emblem(model, at) else {
            rep.no_home.push(format!(
                "emblem: no surface bound to {} met the ray; none drawn",
                at.node
            ));
            return;
        };
        shaders.resize(index, TagId::NULL);
        shaders.push(shader);
    }

    /// A `shad` per Halo 1 shader of `model`: a transparent one rebuilt on
    /// its own stem ([`Self::transparent_model_shader`]), else a copy of
    /// the base's model shader ([`Base::model_shader`]) drawing the Halo 1 shader's own
    /// base and detail maps, converted by `h1bitmaps` (each once). A
    /// shader with no base map this reads gets the base's model shader
    /// itself, never null: the shader draw begin `0x1bbf0` does not skip a
    /// NONE shader (`0x1bcb9` only clears its cached words) and the draw
    /// `0x16b90` then reads tag slot `0xffff`'s entry as a `shad`. With no
    /// template at all the materials stay null.
    ///
    /// `rounds` is the loaded maximum of the weapon whose model this is
    /// when Halo 1's counters on it count its rounds
    /// ([`h1::Weapon::rounds_counter`]): a numeric `schi` entry then
    /// becomes the Halo 2 digit its permutation names
    /// ([`Self::counter_shader`]), one `shad` per digit. The level countdown
    /// uses a separate animation input for each digit.
    fn model_shaders(
        &mut self,
        rep: &mut ObjectReport,
        model: &h1models::h1::Model,
        rounds: Option<i16>,
    ) -> Vec<TagId> {
        let Some(template) = self.base.model_shader.clone() else {
            rep.no_home
                .push("model shaders: the base has no model shader to copy; materials null".into());
            return Vec::new();
        };
        let mut out = Vec::new();
        for s in &model.shaders {
            if let Some(id) = self.counter_shader(&template.library, s.tag, s.permutation, rounds) {
                out.push(id);
                continue;
            }
            let id = (|| -> Option<TagId> {
                let tag = self.h1.tag(s.tag).ok()?;
                if let Some(id) = self.done.get(&tag.id.0) {
                    return Some(*id);
                }
                if let Some(id) = self.transparent_model_shader(&template.library, tag) {
                    return Some(id);
                }
                let maps = h1convert::textures::h1_maps_of(self.h1, tag)?;
                let base = self.bitmap(maps.base?)?;
                let detail = maps.detail.and_then(|d| self.bitmap(d));
                let scale = h1convert::textures::h1_scales_of(self.h1, tag).detail;
                let (tree, _) = template.copy(base, detail, scale);
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

    /// A Halo 1 model shader that isn't `senv` or `soso` (glass, chicago,
    /// generic, meter, plasma, water) rebuilt as the structure's are
    /// ([`h1convert::shad::read_h1`]): its own stem (alpha blended,
    /// additive or self-illuminated) and maps, converted once. `None` when
    /// it isn't one, no map of it reads or converts, or `library` lacks
    /// every stem it could draw with; the tex_bump copy then follows.
    ///
    /// A `soso` whose base is cut out (an alpha Halo 1 tests,
    /// `h1convert::shad::soso_alpha_tested`: the leaves of plants and
    /// trees, decals) takes this path onto `tex_bump_alpha_test_single_pass`
    /// (converter version 84; they drew as solid cards before). Its bump
    /// slot takes a flat bump map cut out where the base is
    /// ([`Self::cutout_bump`], converter version 92): the stem's depth
    /// and shadow passes kill by the bump's alpha, so with the default
    /// bump the cards stayed whole and dark.
    ///
    /// A `soso` with a reflection cube map takes this path too, onto
    /// `tex_bump_env` with Halo 1's cube map and tints
    /// (`h1convert::shad::read_h1`, `soso_reflection`): Halo 1's weapons
    /// take their metal from that reflection over dark base maps, and
    /// drew near black without it (the sniper rifle in first person,
    /// converter version 76).
    ///
    /// An additive `schi` whose maps multiply together (Blood Gulch's
    /// teleporter cone, the pelican's thrusters) takes this path onto
    /// `two_add_clouds`, scrolling and faded by the view angle
    /// (`h1convert::shad::multiply_chain`, converter version 90; the cone
    /// drew its first grey dust map alone before).
    ///
    /// A `soso` with a change colour (`h1convert::shad::soso_change_colour`)
    /// takes this path onto `tex_bump_env_four_change_color`, its
    /// multipurpose map's blue baked into the channel of the Halo 2 slot
    /// its colour goes to ([`Self::change_colour_bitmap`]): the cyborg's
    /// armour takes the player's colour (converter version 84).
    ///
    /// Before converter version 73 these took the opaque tex_bump copy
    /// of whatever bitmap sat at a fixed offset, or, with none there, the
    /// base's stand-in model shader: a10's cryo tube glass (`sotr`
    /// `levels\a10\shaders\cryo glass`) drew as a marine's head shader, an
    /// opaque pane over the player's view from inside the tube.
    fn transparent_model_shader(
        &mut self,
        library: &h1convert::shad::Library,
        tag: &Tag,
    ) -> Option<TagId> {
        let group = tag.group.to_string();
        if group == "senv" {
            return None;
        }
        let s = h1convert::shad::read_h1(self.h1, tag)?;
        let mut chosen = s.choose(library)?.clone();
        // A change colour whose map doesn't bake draws as it did without
        // one: the change-colour stem's default map would tint by colours
        // nobody set.
        let change_colour = h1convert::shad::soso_change_colour(self.h1, tag);
        if chosen.layout.stem == h1convert::shad::TEX_BUMP_ENV_FOUR_CHANGE_COLOR.stem
            && change_colour
                .and_then(|(slot, m)| self.change_colour_bitmap(m, slot))
                .is_none()
        {
            chosen = chosen.fallback.as_deref()?.choose(library)?.clone();
        }
        // A `soso` is rebuilt for its reflection (converter version 76)
        // or its cut-out base (84, `shad::soso_alpha_tested`): otherwise
        // the tex_bump copy draws it as before.
        let env = [
            h1convert::shad::TEX_BUMP_ENV.stem,
            h1convert::shad::TEX_BUMP_ENV_FOUR_CHANGE_COLOR.stem,
        ];
        if group == "soso"
            && !env.contains(&chosen.layout.stem)
            && chosen.layout.stem != h1convert::shad::TEX_BUMP_ALPHA_TEST.stem
        {
            return None;
        }
        // On tex_bump_env the base's alpha masks the reflection: a `soso`'s
        // base takes its multipurpose map's red there (converter version
        // 80; before, the base's own alpha, 1 on Halo 1's DXT1 weapon
        // bases, so they reflected everywhere at full strength). The
        // change-colour stem masks its reflection the same way.
        let mask = env
            .contains(&chosen.layout.stem)
            .then(|| h1convert::shad::soso_multipurpose(self.h1, tag))
            .flatten();
        let mut maps = HashMap::new();
        for (role, m) in &chosen.maps {
            let id = match (role, mask, chosen.extras.brightest_mask) {
                (h1convert::shad::Role::Base, Some(mask), _) => {
                    self.masked_bitmap(m.bitmap, mask).or_else(|| self.bitmap(m.bitmap))
                }
                // A plasma's picture with its brightness as alpha
                // (converter version 90, `shad::plasma_shader`).
                (h1convert::shad::Role::Mask, _, Some(gain)) => self
                    .brightest_bitmap(m.bitmap, gain)
                    .or_else(|| self.bitmap(m.bitmap)),
                // The multipurpose map's blue in the slot's channel; no
                // change-colour map, no tint (the layout's default).
                (h1convert::shad::Role::ChangeColour, _, _) => match change_colour {
                    Some((slot, _)) => self.change_colour_bitmap(m.bitmap, slot),
                    None => None,
                },
                _ => self.bitmap(m.bitmap),
            };
            let Some(id) = id else { continue };
            maps.insert(
                *role,
                h1convert::shad::Bound {
                    id,
                    scale: m.scale,
                    lod_offset: h1convert::shad::h1_lod_offset(self.h1, m.bitmap),
                },
            );
        }
        if maps.is_empty() {
            return None;
        }
        // The cut-out stem's depth and shadow passes kill by its bump
        // map's alpha, not the base's (`h1bitmaps::mask::cutout_bump`):
        // a `soso` has no bump map, so it takes a flat one cut out where
        // its base is (converter version 92; the stem's default bump is
        // opaque, and the leaves drew as whole dark cards).
        if chosen.layout.stem == h1convert::shad::TEX_BUMP_ALPHA_TEST.stem
            && !maps.contains_key(&h1convert::shad::Role::Bump)
            && let Some((_, base)) = chosen
                .maps
                .iter()
                .find(|(r, _)| *r == h1convert::shad::Role::Base)
            && let Some(id) = self.cutout_bump(base.bitmap)
        {
            maps.insert(
                h1convert::shad::Role::Bump,
                h1convert::shad::Bound {
                    id,
                    scale: base.scale,
                    lod_offset: h1convert::shad::h1_lod_offset(self.h1, base.bitmap),
                },
            );
        }
        let tree = library.build_with(chosen.layout, &maps, &chosen.extras).ok()?;
        self.cached(
            tag,
            Converted {
                group: FourCC(*b"shad"),
                root: tree,
                raw: Vec::new(),
            },
        )
        .ok()
    }

    /// Rebuild a numeric `schi` as one animated shader per digit using
    /// the model's shader permutation. The digits come from either the
    /// weapon's ammunition function or the converted campaign countdown.
    fn counter_shader(
        &mut self,
        library: &h1convert::shad::Library,
        id: h1map::TagId,
        permutation: i16,
        loaded: Option<i16>,
    ) -> Option<TagId> {
        let tag = self.h1.tag(id).ok()?;
        if tag.group != h1map::Group::new(b"schi") {
            return None;
        }
        let key = (id.0, permutation, loaded);
        if let Some(t) = self.counter_shaders.get(&key) {
            return *t;
        }
        let tag = tag.clone();
        let out = (|| {
            let n = h1convert::shad::chicago_numeric(self.h1, &tag)?;
            let input = n
                .countdown_digit(permutation)
                .or_else(|| n.ammunition_digit(permutation, loaded?))?;
            let s = h1convert::shad::counter_shader(&tag.name, &n, input);
            let digits = self.bitmap(n.bitmap)?;
            let maps = HashMap::from([(
                h1convert::shad::Role::Base,
                h1convert::shad::Bound {
                    id: digits,
                    scale: n.scale,
                    lod_offset: h1convert::shad::h1_lod_offset(self.h1, n.bitmap),
                },
            )]);
            let tree = library.build_with(s.layout, &maps, &s.extras).ok()?;
            let place = input.rsplit('_').next().unwrap_or(input);
            Some(self.tree.push(
                &format!("{}_{place}", tag.name),
                Converted {
                    group: FourCC(*b"shad"),
                    root: tree,
                    raw: Vec::new(),
                },
            ))
        })();
        self.counter_shaders.insert(key, out);
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

    /// The Halo 1 base map `base` with the red channel of `mask` (a
    /// `soso`'s multipurpose map) as its alpha, DXT5
    /// (`h1bitmaps::mask::red_into_alpha`), `<base name>_reflection_<mask
    /// leaf>`, built once per pair. `None` when either does not read or
    /// decode (the caller then takes the base as it is), or for a base of
    /// more than one image.
    fn masked_bitmap(&mut self, base: h1map::TagId, mask: h1map::TagId) -> Option<TagId> {
        let key = (base.0, mask.0);
        if let Some(t) = self.masked_bitmaps.get(&key) {
            return *t;
        }
        let out = self.bake_mask(base, mask);
        self.masked_bitmaps.insert(key, out);
        out
    }

    /// The Halo 1 bitmap `id` with `gain` times its brightest channel as
    /// alpha, DXT5 (`h1bitmaps::mask::convert_brightest`),
    /// `<name>_brightness_<gain × 1000>`, built once per pair. `None` when
    /// it does not read or decode, or has more than one image.
    fn brightest_bitmap(&mut self, id: h1map::TagId, gain: f32) -> Option<TagId> {
        let key = (id.0, gain.to_bits());
        if let Some(t) = self.brightest_bitmaps.get(&key) {
            return *t;
        }
        let tag = self.h1.tag(id).ok()?.clone();
        let out = h1bitmaps::mask::convert_brightest(self.h1, &tag, gain)
            .ok()
            .map(|conv| {
                let name = h1convert::shad::brightness_mask_name(&tag.name, gain);
                self.tree.push(&name, conv)
            });
        self.brightest_bitmaps.insert(key, out);
        out
    }

    /// The flat bump map cut out by the Halo 1 base map `base`'s alpha
    /// (`h1bitmaps::mask::convert_cutout_bump`), `<base
    /// name>_alpha_test_bump`, built once per base. `None` when the base
    /// does not read or decode, or has more than one image: the stem's
    /// default bump then stands, uncut, as before.
    fn cutout_bump(&mut self, base: h1map::TagId) -> Option<TagId> {
        if let Some(t) = self.cutout_bumps.get(&base.0) {
            return *t;
        }
        let tag = self.h1.tag(base).ok()?.clone();
        let out = h1bitmaps::mask::convert_cutout_bump(self.h1, &tag)
            .ok()
            .map(|conv| {
                let name = h1convert::shad::cutout_bump_name(&tag.name);
                self.tree.push(&name, conv)
            });
        self.cutout_bumps.insert(base.0, out);
        out
    }

    fn bake_mask(&mut self, base: h1map::TagId, mask: h1map::TagId) -> Option<TagId> {
        let tag = self.h1.tag(base).ok()?.clone();
        let mtag = self.h1.tag(mask).ok()?.clone();
        let bm = h1map::bitmap::Bitmap::read(self.h1, &tag).ok()?;
        let mm = h1map::bitmap::Bitmap::read(self.h1, &mtag).ok()?;
        if bm.images.len() != 1 || mm.images.is_empty() {
            return None;
        }
        let (img, px) = h1bitmaps::mask::red_into_alpha(
            &bm.images[0],
            bm.pixels(self.h1, 0).ok()?,
            &mm.images[0],
            mm.pixels(self.h1, 0).ok()?,
        )
        .ok()?;
        let mut baked = bm.clone();
        baked.images[0] = img;
        // Halo 1's format word: compressed with interpolated alpha (DXT5).
        baked.encoding = h1bitmaps::mask::INTERPOLATED_ALPHA;
        let mut px = Some(px);
        let (conv, _) =
            h1bitmaps::convert::convert_with(&baked, |_| Ok(px.take().unwrap_or_default())).ok()?;
        let leaf = mtag.name.rsplit('\\').next().unwrap_or(&mtag.name);
        Some(self.tree.push(&format!("{}_reflection_{leaf}", tag.name), conv))
    }

    /// Puts `obj`'s change colours in Halo 2's slots
    /// ([`h1::ChangeColour::halo2_slots`]), slot 0 starting as C when its
    /// model's shaders read C ([`Self::model_reads_c`]), else as A. An
    /// object with no Halo 1 change colour and no shader reading one keeps
    /// none.
    fn halo2_change_colours(&self, obj: &mut h1::Object) {
        let reads_c = self.model_reads_c(obj.model);
        if obj.change_colours.is_empty() && !reads_c {
            return;
        }
        obj.change_colours = h1::ChangeColour::halo2_slots(&obj.change_colours, reads_c);
    }

    /// Whether a shader of the Halo 1 model `model` (`mode +0xdc`, `0x20`
    /// each, a reference at `+0`) is a `soso` reading change colour C
    /// (`h1convert::shad::SOSO_CHANGE_COLOUR`): the cyborg's armour, visor
    /// and hands, on the cyborg, the cryo tube and the helmet scenery.
    fn model_reads_c(&self, model: Option<h1map::TagId>) -> bool {
        let Some(tag) = model.and_then(|m| self.h1.tag(m).ok()) else {
            return false;
        };
        let Ok(d) = self.h1.tag_data(tag) else {
            return false;
        };
        let Ok(shaders) = h1::elements(self.h1, d, 0xdc, 0x20) else {
            return false;
        };
        shaders.iter().any(|e| {
            h1::reference(self.h1, e, 0)
                .and_then(|id| self.h1.tag(id).ok())
                .filter(|t| t.group == h1map::Group::new(b"soso"))
                .and_then(|t| self.h1.tag_data(t).ok())
                .and_then(h1convert::shad::soso_change_colour_source)
                == Some(3)
        })
    }

    /// Halo 2's change-colour map for a `soso` whose change colour goes to
    /// slot `slot` (`h1convert::shad::halo2_change_colour_slot`): its
    /// multipurpose map `mask`'s blue (`SOSO_CHANGE_COLOUR_CHANNEL`) in
    /// channel `slot`, every other channel 0
    /// (`h1bitmaps::mask::channel_into`), `<mask name>_change_color_<slot>`,
    /// built once per pair. `None` when the mask does not read or decode.
    fn change_colour_bitmap(&mut self, mask: h1map::TagId, slot: usize) -> Option<TagId> {
        let key = (mask.0, slot);
        if let Some(t) = self.change_colour_bitmaps.get(&key) {
            return *t;
        }
        let out = self.bake_change_colour(mask, slot);
        self.change_colour_bitmaps.insert(key, out);
        out
    }

    fn bake_change_colour(&mut self, mask: h1map::TagId, slot: usize) -> Option<TagId> {
        let mtag = self.h1.tag(mask).ok()?.clone();
        let mm = h1map::bitmap::Bitmap::read(self.h1, &mtag).ok()?;
        if mm.images.len() != 1 {
            return None;
        }
        let (img, px) = h1bitmaps::mask::channel_into(
            &mm.images[0],
            mm.pixels(self.h1, 0).ok()?,
            h1convert::shad::SOSO_CHANGE_COLOUR_CHANNEL,
            slot,
        )
        .ok()?;
        let mut baked = mm.clone();
        baked.images[0] = img;
        baked.encoding = h1bitmaps::mask::INTERPOLATED_ALPHA;
        let mut px = Some(px);
        let (conv, _) =
            h1bitmaps::convert::convert_with(&baked, |_| Ok(px.take().unwrap_or_default())).ok()?;
        Some(
            self.tree
                .push(&format!("{}_change_color_{slot}", mtag.name), conv),
        )
    }

    /// The Halo 1 bitmap `id` for a HUD element whose colour's alpha is
    /// `alpha`: the bitmap itself at 255, else a copy with its alpha scaled
    /// (`h1hud::scale_alpha`, or `h1hud::alpha_scaled_argb` for a format
    /// without an alpha byte), `<name>_alpha_<alpha>`, built once.
    fn hud_bitmap(&mut self, id: h1map::TagId, alpha: u8) -> Option<TagId> {
        if alpha == 0xff {
            return self.bitmap(id);
        }
        let key = (id.0, alpha);
        if let Some(t) = self.hud_bitmaps.get(&key) {
            return Some(*t);
        }
        let tag = self.h1.tag(id).ok()?.clone();
        let bm = h1map::bitmap::Bitmap::read(self.h1, &tag).ok()?;
        // Byte-alpha formats scale in place; the rest (DXT, 16-bit, no
        // alpha) are decoded and rewritten as linear A8R8G8B8.
        let mut scaled = bm.clone();
        let mut pixels = Vec::with_capacity(bm.images.len());
        for (i, img) in bm.images.iter().enumerate() {
            let mut px = bm.pixels(self.h1, i).ok()?.to_vec();
            if crate::h1hud::scale_alpha(img.format, &mut px, alpha).is_none() {
                let (argb, out) = crate::h1hud::alpha_scaled_argb(img, &px, alpha).ok()?;
                scaled.images[i] = argb;
                px = out;
            }
            pixels.push(px);
        }
        let (conv, _) =
            h1bitmaps::convert::convert_with(&scaled, |i| Ok(std::mem::take(&mut pixels[i]))).ok()?;
        let out = self.tree.push(&format!("{}_alpha_{alpha:02x}", tag.name), conv);
        self.hud_bitmaps.insert(key, out);
        Some(out)
    }

    /// The Halo 1 bitmap `id` for a HUD meter: a copy with the colour of
    /// every clear texel made black (`h1hud::clear_meter_colour`; a format
    /// without an alpha byte is decoded and rewritten as linear A8R8G8B8
    /// first, `h1hud::alpha_scaled_argb` at 255), so Halo 2's legacy meter
    /// shader leaves clear what Halo 1 leaves clear
    /// (`h1hud::legacy_meter_alpha`). `<name>_meter`, built once.
    fn meter_bitmap(&mut self, id: h1map::TagId) -> Option<TagId> {
        if let Some(t) = self.meter_bitmaps.get(&id.0) {
            return Some(*t);
        }
        let tag = self.h1.tag(id).ok()?.clone();
        let bm = h1map::bitmap::Bitmap::read(self.h1, &tag).ok()?;
        let mut cleared = bm.clone();
        let mut pixels = Vec::with_capacity(bm.images.len());
        for (i, img) in bm.images.iter().enumerate() {
            let mut px = bm.pixels(self.h1, i).ok()?.to_vec();
            if crate::h1hud::clear_meter_colour(img.format, &mut px).is_none() {
                let (argb, mut out) = crate::h1hud::alpha_scaled_argb(img, &px, 0xff).ok()?;
                crate::h1hud::clear_meter_colour(argb.format, &mut out)?;
                cleared.images[i] = argb;
                px = out;
            }
            pixels.push(px);
        }
        let (conv, _) =
            h1bitmaps::convert::convert_with(&cleared, |i| Ok(std::mem::take(&mut pixels[i]))).ok()?;
        let out = self.tree.push(&format!("{}_meter", tag.name), conv);
        self.meter_bitmaps.insert(id.0, out);
        Some(out)
    }

    /// A copy of bitmap `id` whose sequence `seq` starts at its sprite
    /// `first`, for a Halo 2 widget, which draws its sequence's first
    /// sprite, to draw a later one (`h1hud::zoom_widget`). Sprite 0 is the
    /// bitmap itself; `<name>_seq<seq>_sprite<first>`, built once. None past
    /// the sequence's sprites.
    fn sprite_bitmap(&mut self, id: h1map::TagId, seq: i16, first: usize) -> Option<TagId> {
        if first == 0 {
            return self.bitmap(id);
        }
        let key = (id.0, seq, first);
        if let Some(t) = self.sprite_bitmaps.get(&key) {
            return Some(*t);
        }
        let tag = self.h1.tag(id).ok()?.clone();
        let bm = h1map::bitmap::Bitmap::read(self.h1, &tag).ok()?;
        let mut shifted = bm.clone();
        let sprites = &mut shifted.sequences.get_mut(usize::try_from(seq).ok()?)?.sprites;
        if first >= sprites.len() {
            return None;
        }
        sprites.drain(..first);
        let mut pixels = (0..bm.images.len())
            .map(|i| bm.pixels(self.h1, i).map(|p| p.to_vec()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok()?;
        let (conv, _) =
            h1bitmaps::convert::convert_with(&shifted, |i| Ok(std::mem::take(&mut pixels[i]))).ok()?;
        let out = self.tree.push(&format!("{}_seq{seq}_sprite{first}", tag.name), conv);
        self.sprite_bitmaps.insert(key, out);
        Some(out)
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

    /// A Halo 1 machine, control or light fixture, from the `mach` or
    /// `ctrl` definition ([`crate::rebuild::device`]); a light fixture is
    /// a machine (Halo 2 has no `lifi` tags). There is no template path.
    fn device(&mut self, tag: &Tag) -> Result<TagId> {
        let mut dev = h1::Device::read(self.h1, tag)?;
        self.halo2_change_colours(&mut dev.object);
        let lifi = dev.machine.is_none() && dev.control.is_none();
        let group = if dev.control.is_some() {
            b"ctrl"
        } else {
            b"mach"
        };
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        let hlmt = self.model(&mut rep, &dev.object, None, &tag.name, true, None)?;
        rep.physics = Some("none (no Halo 1 collision to build from)".into());
        self.object_physics(&mut rep, hlmt, &dev.object, &tag.name, PhysicsKind::Device)?;
        let predicted = self.predicted(hlmt);
        let root = crate::rebuild::device(&dev, hlmt, &predicted)?;
        let bytes = self.h1.tag_data(tag)?;
        let root =
            self.with_object_attachments(*group, "devi.obje", root, bytes, &dev.object, &mut rep)?;
        rep.no_home.push(
            "devi sounds and effects (Halo 1 effe references): null; the device is silent".into(),
        );
        if lifi {
            rep.no_home.push(
                "lifi: built as a mach (platform) with its attachments; the placement's colour and intensity are not carried".into(),
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
        // Halo 1's powered mass points, with the model they sit on.
        let powered = match u.object.model {
            Some(m) => {
                let (model, _) =
                    h1models::model_from_map(self.h1, self.h1_tag(m)?, &ModelOptions::default())?;
                self.powered_points(&u.object, &model)?
            }
            None => Vec::new(),
        };
        let scripted = scripted_props::ScriptedVehicle::of(&tag.name);
        let role = self.vehicle_role(vehicle_type, powered.len())?;
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
        let hlmt = self.model(&mut rep, &u.object, None, &tag.name, true, None)?;
        let wheels = role.as_ref().is_some_and(|r| r.wheel_shapes);
        let kind = match scripted {
            Some(scripted_props::ScriptedVehicle::Seat) => PhysicsKind::Device,
            Some(scripted_props::ScriptedVehicle::Flyer) => PhysicsKind::ScriptedFlyer,
            None => PhysicsKind::Vehicle { wheels },
        };
        let has_physics = self.object_physics(&mut rep, hlmt, &u.object, &tag.name, kind)?;
        if scripted.is_some() && !has_physics {
            return Err(Error::Invalid(format!(
                "{}: no source physics to build",
                tag.name
            )));
        }
        let predicted = self.predicted(hlmt);
        let (markers, bounds) = match u.object.model {
            Some(m) => {
                let model =
                    h1models::model_from_map(self.h1, self.h1_tag(m)?, &ModelOptions::default())?.0;
                let bounds = crate::rebuild::model_sphere(&model);
                (model.markers.into_iter().map(|m| m.name).collect(), bounds)
            }
            None => (Vec::new(), None),
        };
        let mut seats: Vec<_> = u
            .seats
            .iter()
            .map(|s| crate::rebuild::seat_names(s, &markers))
            .collect();
        // Each seat's camera track: Halo 1's first (the loose one where
        // it has two), converted. A seat with none takes what Halo 1 falls
        // back to: the unit's own track, else Halo 1's default unit track.
        // Left empty, the engine would fall back to Halo 2's globals
        // default, Halo 1's default stored unconverted, which its track
        // search reads backwards (crate::rebuild::camera_track).
        let fallback = u.camera_tracks.first().copied().or_else(|| {
            self.h1
                .tags()
                .iter()
                .find(|t| {
                    t.group.0.to_be_bytes() == *b"trak"
                        && t.name == DEFAULT_UNIT_CAMERA_TRACK
                })
                .map(|t| t.id)
        });
        let mut tracks = 0;
        for (names, seat) in seats.iter_mut().zip(&u.seats) {
            let Some(t) = seat.camera_tracks.first().copied().or(fallback) else { continue };
            match self.camera_track(self.h1_tag(t)?) {
                Ok(id) => {
                    names.camera_track = Some(id);
                    tracks += 1;
                }
                Err(e) => rep
                    .no_home
                    .push(format!("seat {}: camera track not converted ({e})", seat.label)),
            }
        }
        let points: Vec<crate::rebuild::Point> = {
            // Friction points follow the role's phantom spheres; hover
            // points (a role with no friction points) the member's hover
            // markers, so each takes the words of the retail point nearest
            // it (`VehicleRole::hover_each`).
            let spheres = match &role {
                Some(r)
                    if r.friction.is_none()
                        && r.hover.is_some()
                        && r.hover_at.len() == powered.len() =>
                {
                    r.hover_at.clone()
                }
                Some(r) => crate::rebuild::role_spheres(r),
                None => Vec::new(),
            };
            let at: Vec<[f32; 3]> = powered.iter().map(|p| p.object).collect();
            let order = sphere_order(&at, &spheres);
            order.into_iter().map(|i| powered[i].point.clone()).collect()
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
        let (attachments, creation_effect, functions) =
            self.object_attachments(b, &u.object, &mut rep)?;
        let (root, roles) = crate::provenance::capture(|| {
            crate::rebuild::vehicle_with_tuning(&crate::rebuild::VehicleSpec {
                attachments: &attachments,
                functions: &functions,
                creation_effect,
                object: &u.object,
                bounds,
                hlmt,
                seats: &seats,
                predicted: &predicted,
                camera_field_of_view: u.camera_field_of_view,
                vehicle_type,
                flags,
                weapons: &weapons,
                role: role.as_ref(),
                points: &points,
            }, scripted.and_then(|kind| kind.flight_tuning(b)))
        });
        let root = self.unit_foot(*b"vehi", root?, u, &mut rep)?;
        rep.no_home.push(format!(
            "vehi: Halo 1's type, flags (low half), camera, {} seats (label as the rider's mode, markers), {} weapons and {} powered mass points as friction or hover points; the type's role words and gears; {} attachments and the creation effect from Halo 1; {} seat camera tracks Halo 1's; HUD and damage null; {} Halo 1 references",
            u.seats.len(),
            weapons.len(),
            points.len(),
            attachments.len(),
            tracks,
            u.references.iter().filter(|r| r.tag.is_some()).count()
        ));
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"vehi"),
                root,
                raw: Vec::new(),
            },
        );
        self.tree.note_roles(id, roles);
        Ok(id)
    }

    /// A Halo 1 camera track as a Halo 2 `trak` with the same control
    /// points ([`crate::rebuild::camera_track`]), converted once.
    fn camera_track(&mut self, tag: &Tag) -> Result<TagId> {
        if let Some(id) = self.done.get(&tag.id.0) {
            return Ok(*id);
        }
        let t = h1::CameraTrack::read(self.h1, tag)?;
        let root = crate::rebuild::camera_track(&t)?;
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"trak"),
                root,
                raw: Vec::new(),
            },
        );
        self.done.insert(tag.id.0, id);
        Ok(id)
    }

    /// The vehicle type's role over the base's vehicles of that type
    /// whose friction and hover points are nearest `points`, Halo 1's
    /// powered mass points ([`crate::rebuild::vehicle_role`]), computed
    /// once per type and count. The turret type keeps the disc-wide
    /// turret tables ([`crate::rebuild::TURRET_ROLE`]): `None`.
    pub fn vehicle_role(
        &mut self,
        vehicle_type: u16,
        points: usize,
    ) -> Result<Option<crate::rebuild::VehicleRole>> {
        if u32::from(vehicle_type) == crate::rebuild::ROLE_VEHICLE_TYPE {
            return Ok(None);
        }
        if let Some(w) = self.vehicle_roles.get(&(vehicle_type, points)) {
            return Ok(w.clone());
        }
        let w = crate::rebuild::vehicle_role(self.base, vehicle_type, Some(points))?;
        self.vehicle_roles.insert((vehicle_type, points), w.clone());
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
    /// Per mass point of a vehicle's Halo 1 physics, the marker its
    /// converted model holds for it: a powered point's, named as
    /// [`Self::powered_points`] names it; `None` for the others, and empty
    /// for an object without physics. A vehicle graph's suspension names
    /// its mass points by index (V: the warthog's four tire points, the
    /// scorpion's eight tread points, all powered).
    fn mass_point_markers(&self, obj: &h1::Object) -> Result<Vec<Option<String>>> {
        let Some(p) = obj.physics else {
            return Ok(Vec::new());
        };
        let phys = h1::Physics::read(self.h1, self.h1_tag(p)?)?;
        Ok(phys
            .mass_points
            .iter()
            .map(|m| (m.powered_mass_point >= 0).then(|| crate::coll::sid_name(&m.name)))
            .collect())
    }

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
            // Every point rides the root node, not its own. A wheel's or
            // tread's node turns and rises with the spin and suspension
            // animations, and a contact point that moves with it pushes
            // the vehicle about. A hover point on any other node lifts
            // nothing: the converted ghost's back point ("hull back
            // seat", on its seat node) gave no lift at any strength, so
            // the ghost rode nose up on its tail with a driver, and the
            // same point moved live to node 0 at the same place held it
            // level (game run, 2026-11-19). V: every friction and hover
            // marker of every retail vehicle is on node 0
            // (`tests/vehicle_rebuild_disc.rs`,
            // `every_powered_point_rides_the_root_node`).
            let node = 0;
            // The region is unchanged by that: a gripping point's is the
            // root node's collision region (as through version 92), any
            // other point's its own node's.
            let grips = phys
                .powered_flags
                .get(m.powered_mass_point as usize)
                .is_some_and(|f| f & h1::POWERED_GROUND_FRICTION != 0);
            let own = if grips {
                0
            } else if (m.model_node as usize) < model.nodes.len() && m.model_node >= 0 {
                m.model_node
            } else {
                0
            };
            // The marker keeps no rotation, as the retail scorpion's
            // treads (V), which drive this way. The retail warthog's
            // wheels face out along their axles, but Halo 1's scorpion
            // treads grip too, so one rule would turn them as well, and
            // no game run has shown the turn helping the warthog (a live
            // edit before it spawned left its path unchanged to the
            // hundredth, as a phmo edit did, so the engine likely reads
            // both at load).
            let (r, t) = world
                .get(node as usize)
                .copied()
                .unwrap_or(([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], [0.0; 3]));
            let d: [f32; 3] = std::array::from_fn(|a| m.position[a] - t[a]);
            // The inverse of a rotation is its transpose.
            let local = std::array::from_fn(|a| (0..3).map(|k| r[k][a] * d[k]).sum());
            let node_name = model.nodes.get(own as usize).map(|n| n.name.as_str());
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
            let region_index = model
                .regions
                .iter()
                .position(|r| !region.is_empty() && crate::coll::sid_name(&r.name) == region)
                .map_or(-1, |i| i as i16);
            out.push(PoweredPoint {
                point: crate::rebuild::Point {
                    marker,
                    region,
                    region_index,
                    steered: false,
                },
                node,
                position: local,
                object: m.position,
            });
        }
        // A point ahead of the points' middle steers (the warthog's front
        // tires, Halo 1's object space has x forward).
        if !out.is_empty() {
            let middle = out.iter().map(|p| p.object[0]).sum::<f32>() / out.len() as f32;
            for p in &mut out {
                p.point.steered = p.object[0] > middle + 1e-3;
            }
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
        let hlmt = self.model(&mut rep, &u.object, None, &tag.name, true, None)?;
        let counterpart_hlmt = self.base.reference(&self.base.root(cid)?, h2::MODEL);
        // Halo 1's dialogue: the first `udlg` the unit names (its
        // dialogue variants block), in place of the counterpart's. V:
        // 125 of the 169 bipeds over the campaign and multiplayer maps
        // name one; 21 name more than one (variants Halo 2's single
        // model dialogue cannot hold; `every_dialogue_converts`).
        let udlg = match u
            .references
            .iter()
            .find(|r| r.group == h1map::Group::new(b"udlg"))
            .and_then(|r| r.tag)
        {
            Some(t) => {
                let t = self.h1_tag(t)?;
                self.dialogue(t, &mut rep)?
            }
            None => None,
        };
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
            if let Some(d) = udlg {
                r.set(DIALOGUE_FIELD, tags::defs::Val::tag(FourCC(*b"udlg"), d))
                    .map_err(|e| Error::Invalid(e.to_string()))?;
                rep.no_home.push("hlmt: dialogue converted from Halo 1's".into());
            }
            *root = tags::defs::to_built(hd, &r, &mut |_| None)
                .map_err(|e| Error::Invalid(e.to_string()))?;
            let copied = fields
                .iter()
                .map(|(path, value)| crate::provenance::RoleField {
                    path: path.clone(),
                    value: value.clone(),
                })
                .collect();
            self.tree.note_roles(h, copied);
            rep.no_home.push(format!(
                "hlmt: {} reference fields from {} (damage, dialogue, camouflage shaders)",
                fields.len(),
                self.base.name(ch).map_or("?", |n| n.1)
            ));
        }
        self.object_physics(&mut rep, hlmt, &u.object, &tag.name, PhysicsKind::Biped)?;
        let predicted = self.predicted(hlmt);
        let mut swap = None;
        let hud = if health_hud::is_player_biped(&tag.name) {
            let tpl_root = self.base.root(cid)?;
            match self.health_interfaces(&tpl_root) {
                Ok(ids) => {
                    rep.no_home.push(format!(
                        "HUD: {cname}'s with Halo 1's health meter ({} interfaces; docs/halo1/health.md)",
                        ids.len()
                    ));
                    match self.unit_hud(u) {
                        Ok((h1, n)) => {
                            match self.sensor_centre(u) {
                                Ok(c) => self.tree.hud_sensor = Some(c),
                                Err(e) => rep.no_home.push(format!("HUD: motion sensor centre: {e}")),
                            }
                            rep.no_home.push(format!(
                                "HUD: Halo 1's ({n} widgets; docs/halo1/hud.md), Halo 2's at the switch"
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
        let parts = BipedParts {
            counterpart: Some(counterpart),
            hud,
            markers,
        };
        let (root, roles) = crate::provenance::capture(|| {
            crate::rebuild::biped(u, hlmt, &predicted, words, &parts)
        });
        let root = self.unit_foot(*b"bipd", root?, u, &mut rep)?;
        let bytes = self.h1.tag_data(tag)?;
        let root =
            self.with_object_attachments(*b"bipd", "unit.obje", root, bytes, &u.object, &mut rep)?;
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
        self.tree.note_roles(id, roles);
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
    ) -> Result<bool> {
        let Some(h) = hlmt.filter(|h| h.0 & 0xffff_0000 == LOCAL) else {
            return Ok(false);
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
            return Ok(true);
        }
        let (phmo, how) = match self.built_physics(obj, kind)? {
            Some((block, how)) => (block, how),
            None if matches!(kind, PhysicsKind::Vehicle { .. }) => {
                let (radius, mass, material) = STAND_IN_SPHERE;
                (
                    crate::rebuild::sphere_physics(radius, [0.0, 0.0, radius], mass, material)?,
                    format!(
                        "stand-in: a {radius} wu sphere of {mass} kg on the origin (rebuild::sphere_physics); no Halo 1 collision to build from"
                    ),
                )
            }
            None => return Ok(false),
        };
        let phmo = self.tree.push(
            name,
            Converted {
                group: FourCC(*b"phmo"),
                root: phmo,
                raw: Vec::new(),
            },
        );
        // Every phmo material has an hlmt material: the built shapes name
        // material 0, and a hit on one reads the hlmt's element of that
        // index (crate::rebuild::HLMT_MATERIALS).
        let mut extra = Vec::new();
        let have = crate::rebuild::model_material_count(&self.tree.tags[k].1.root) as usize;
        for g in crate::rebuild::phmo_material_globals(&self.tree.tags[phmo.0 as usize & 0xffff].1.root)
            .iter()
            .skip(have)
        {
            extra.push(crate::rebuild::model_material(g, g, &self.base.materials)?);
        }
        let root = &mut self.tree.tags[k].1.root;
        root.fixups.retain(|f| f.offset != at);
        root.tag_ref(at, phmo);
        if !extra.is_empty() {
            let mut all = crate::rebuild::model_materials_of(root);
            all.extend(extra);
            crate::rebuild::set_model_materials(root, &all);
        }
        rep.physics = Some(how);
        Ok(true)
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
        let Some(m) = obj.model else {
            return Ok(None);
        };
        let source = if kind == PhysicsKind::ScriptedFlyer {
            obj.physics
                .map(|p| self.h1_tag(p))
                .transpose()?
                .map(|t| scripted_props::mass_properties(self.h1, t))
                .transpose()?
        } else {
            None
        };
        let (model, _) =
            h1models::model_from_map(self.h1, self.h1_tag(m)?, &ModelOptions::default())?;
        let Some(c) = obj.collision else {
            return match source {
                Some((physics, mass)) => Ok(Some((
                    scripted_props::mass_point_physics(&model, &physics, &mass)?,
                    "built from Halo 1's phys: one body with source mass point spheres".into(),
                ))),
                None => Ok(None),
            };
        };
        let coll = h1::Collision::read(self.h1, self.h1_tag(c)?)?;
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
        let mut wheels_note = String::new();
        let bodies = match kind {
            PhysicsKind::Vehicle { .. } | PhysicsKind::ScriptedFlyer | PhysicsKind::Crate => {
                let hold = matches!(kind, PhysicsKind::Vehicle { wheels: true });
                let phys = match &source {
                    Some((physics, _)) => Some(physics.clone()),
                    None => obj
                        .physics
                        .map(|p| h1::Physics::read(self.h1, self.h1_tag(p)?))
                        .transpose()?,
                };
                // Wheels and treads are held above the hull's lowest
                // point where the role's physics models have them (the
                // scorpion's treads), else left out (the warthog's tires):
                // Halo 2 holds a vehicle up on its friction points
                // (springs at the wheel markers) and its body rides clear
                // of the ground (`held_wheels`).
                let wheels = if matches!(kind, PhysicsKind::Vehicle { .. }) {
                    phys.as_ref()
                        .map(|p| wheel_nodes(p, &model.nodes))
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let place = |node: i16| {
                    let (r, t) = world
                        .get(node as usize)
                        .copied()
                        .unwrap_or((Transform::IDENTITY.r, Transform::IDENTITY.t));
                    Transform { r, t }
                };
                let (hull, wheel_parts): (Vec<_>, Vec<_>) =
                    parts.into_iter().partition(|(n, _)| !wheels.contains(n));
                let (parts, wheel_parts) = if hull.is_empty() {
                    (wheel_parts, Vec::new())
                } else {
                    (hull, wheel_parts)
                };
                // One piece per wheel node at most.
                let total = parts.iter().map(|p| p.1.len()).sum::<usize>()
                    + if hold { wheel_parts.len() } else { 0 };
                let mut pieces = Vec::new();
                for (node, cells) in parts {
                    let m = place(node);
                    pieces.extend(cap(cells, total).iter().map(|h| h.transformed(&m)));
                }
                if !hold && !wheel_parts.is_empty() {
                    wheels_note = format!(
                        " ({} wheel and tread nodes left out: the role's physics models have no wheel shapes)",
                        wheel_parts.len()
                    );
                }
                if hold && !wheel_parts.is_empty() {
                    let wheel_parts: Vec<_> = wheel_parts
                        .into_iter()
                        .map(|(node, cells)| {
                            let m = place(node);
                            cells.iter().map(|h| h.transformed(&m)).collect::<Vec<_>>()
                        })
                        .collect();
                    let floor = pieces
                        .iter()
                        .flat_map(|h| h.vertices.iter())
                        .map(|v| v[2])
                        .fold(f32::INFINITY, f32::min);
                    let held = held_wheels(&wheel_parts, floor);
                    wheels_note = format!(
                        " ({} wheel and tread pieces, held above the hull's lowest point {floor:.3})",
                        held.len()
                    );
                    pieces.extend(held);
                }
                let mass = phys
                    .as_ref()
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
                    crate::rebuild::Body {
                        node,
                        pieces,
                        mass,
                    }
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
            "built from Halo 1's coll: {} rigid bodies, {pieces} convex pieces{}{}",
            bodies.len(),
            if enclosed {
                " (over the piece cap: one enclosing piece per node)"
            } else {
                ""
            },
            wheels_note
        );
        let words = match kind {
            PhysicsKind::Biped => crate::rebuild::BIPED_BODY,
            PhysicsKind::Crate => scripted_props::CRATE_BODY,
            PhysicsKind::Vehicle { .. } | PhysicsKind::ScriptedFlyer | PhysicsKind::Device => {
                crate::rebuild::DEVICE_BODY
            }
        };
        let joints = if kind == PhysicsKind::Biped {
            biped_ragdoll(&model.nodes, &bodies)
        } else {
            Vec::new()
        };
        let how = match (kind, joints.len()) {
            (PhysicsKind::Biped, 0) => format!("{how}; no ragdoll (skeleton not mapped, crate::ragdoll::links)"),
            (PhysicsKind::Biped, n) => format!("{how}; ragdoll of {n} constraints (crate::ragdoll)"),
            _ => how,
        };
        Ok(Some((
            crate::rebuild::physics_model_with_mass(
                &bodies,
                &nodes,
                words,
                &joints,
                source.map(|(_, mass)| mass),
            )?,
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
    /// ([`crate::rebuild::Predicted`]): the render model its `hlmt` names,
    /// then every converted bitmap its shaders draw. V: every retail vehicle
    /// lists its `mode` once (`+0` 2, `+2` -1) and its bitmaps (`+0` 0,
    /// `+2` 0), the model first.
    fn predicted(&self, hlmt: Option<TagId>) -> Vec<crate::rebuild::Predicted> {
        let Some((_, hlmt)) = hlmt.and_then(local_index).and_then(|k| self.tree.tags.get(k)) else {
            return Vec::new();
        };
        let Some(mode) = hlmt.root.fixups.iter().find_map(|f| match &f.kind {
            FixupKind::TagRef(id) if f.offset == h2::HLMT_RENDER as u32 => Some(*id),
            _ => None,
        }) else {
            return Vec::new();
        };
        let mut out = vec![crate::rebuild::Predicted::RenderModel(mode)];
        let Some((_, conv)) = local_index(mode).and_then(|k| self.tree.tags.get(k)) else {
            return out;
        };
        let mut shaders = Vec::new();
        self.locals_in(&conv.root, FourCC(*b"shad"), &mut shaders);
        let mut bitmaps = Vec::new();
        for s in shaders {
            let (_, c) = &self.tree.tags[s.0 as usize & 0xffff];
            self.locals_in(&c.root, FourCC(*b"bitm"), &mut bitmaps);
        }
        out.extend(bitmaps.into_iter().map(crate::rebuild::Predicted::Bitmap));
        out
    }

    fn unit(&mut self, tag: &Tag) -> Result<TagId> {
        let mut u = Unit::read(self.h1, tag)?;
        self.halo2_change_colours(&mut u.object);
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
                    "HUD: the template's with Halo 1's health meter ({n} interfaces; docs/halo1/health.md)"
                )),
                Err(e) => rep.no_home.push(format!("HUD: no health meter: {e}")),
            }
        }
        let bytes = self.h1.tag_data(tag)?;
        let root =
            self.with_object_attachments(g, "unit.obje", root, bytes, &u.object, &mut rep)?;
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
    /// Where the unit's Halo 1 HUD (`unhi`) centres the motion sensor
    /// ([`crate::h1hud::sensor_centre`]).
    pub fn sensor_centre(&self, u: &crate::h1::Unit) -> Result<[i16; 2]> {
        let unhi = u
            .references
            .iter()
            .filter(|r| r.group == h1map::Group::new(b"unhi"))
            .find_map(|r| r.tag)
            .ok_or_else(|| Error::Invalid("no unhi".into()))?;
        let tag = self.h1.tag(unhi)?;
        let b = crate::h1::tag_bytes(self.h1, tag, b"unhi", crate::h1hud::unhi::SIZE)?;
        crate::h1hud::sensor_centre(b)
    }

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
        let text_shader = find(b"shad", crate::h1hud::TEXT_SHADER)?;
        let mut widgets = Vec::new();
        for e in &elements {
            if !crate::h1hud::unscaled(e.scale) {
                return Err(Error::Invalid(format!("{}: scaled {:?}", e.name, e.scale)));
            }
            let bt = self.h1.tag(e.bitmap)?.clone();
            let bm = h1map::bitmap::Bitmap::read(self.h1, &bt)?;
            if crate::h1hud::sprite(&bm, e.sequence).is_none() {
                return Err(Error::Invalid(format!(
                    "{}: {} draws nothing in sequence {}",
                    e.name, bt.name, e.sequence
                )));
            }
            // A static element's colour alpha goes into its sprite (the
            // motion sensor's background is a faint disc in Halo 1); a
            // meter's colour is elsewhere in its element and stays out.
            // Colour 0 draws nothing in Halo 1 (`h1hud::static_alpha`).
            let alpha = match e.role {
                crate::h1hud::Role::Static
                    if self.options.unit_hud_draw_colour
                        && tag.name == "ui\\hud\\cyborg"
                        && e.name != "h1_hud_background" => 0xff,
                crate::h1hud::Role::Static => match crate::h1hud::static_alpha(e.colour) {
                    Some(a) => a,
                    None => continue,
                },
                crate::h1hud::Role::Meter(_) => 0xff,
            };
            let bitmap = match e.role {
                crate::h1hud::Role::Static => self.hud_bitmap(e.bitmap, alpha),
                crate::h1hud::Role::Meter(_) => self.meter_bitmap(e.bitmap),
            }
            .ok_or_else(|| Error::Invalid(format!("bitmap {} unconverted", bt.name)))?;
            let reg = crate::h1hud::registration(e.anchor);
            widgets.push(crate::h1hud::widget_for(e, bitmap, reg, meter, plain));
        }
        if widgets.is_empty() {
            return Err(Error::Invalid(format!("{}: no drawable element", tag.name)));
        }
        // Halo 1's grenade HUDs (docs/halo1/hud.md "Grenades"): each type's
        // grhi, shown while that type is selected, after the unit's.
        let grenades = crate::h1hud::grenade_huds(self.h1)?;
        let mut texts = Vec::new();
        for (k, g) in grenades.iter().enumerate().take(crate::h1hud::GRENADE_STATES.len()) {
            let Some(g) = g else { continue };
            let gt = self.h1.tag(*g)?.clone();
            let gb = crate::h1::tag_bytes(self.h1, &gt, b"grhi", crate::h1hud::grhi::SIZE)?.to_vec();
            // Its count, a Halo 2 number text on the type's input.
            if let Some(n) = crate::h1hud::grenade_number(&gb)? {
                if !crate::h1hud::unscaled(n.scale) {
                    return Err(Error::Invalid(format!("{} count: scaled {:?}", gt.name, n.scale)));
                }
                let w = crate::h1hud::digit_width(self.h1)?;
                let h = crate::h1hud::digit_height(self.h1)?;
                texts.push(crate::h1hud::grenade_count(&n, k, w, h, text_shader)?);
            }
            for e in crate::h1hud::grenade_elements(self.h1, &gb)? {
                if !crate::h1hud::unscaled(e.scale) {
                    return Err(Error::Invalid(format!("{}: scaled {:?}", gt.name, e.scale)));
                }
                let bt = self.h1.tag(e.bitmap)?.clone();
                let bm = h1map::bitmap::Bitmap::read(self.h1, &bt)?;
                if crate::h1hud::sprite(&bm, e.sequence).is_none() {
                    return Err(Error::Invalid(format!(
                        "{}: {} draws nothing in sequence {}",
                        gt.name, bt.name, e.sequence
                    )));
                }
                let Some(alpha) = crate::h1hud::static_alpha(e.colour) else {
                    continue;
                };
                let bitmap = self
                    .hud_bitmap(e.bitmap, alpha)
                    .ok_or_else(|| Error::Invalid(format!("bitmap {} unconverted", bt.name)))?;
                let reg = crate::h1hud::registration(e.anchor);
                let mut w = crate::h1hud::widget_for(&e, bitmap, reg, meter, plain);
                w.state = crate::h1hud::GRENADE_STATES[k];
                widgets.push(w);
            }
        }
        let n = widgets.len();
        let id = self.tree.push(
            &format!("{}{}", tag.name, crate::h1hud::NHDT_SUFFIX),
            Converted {
                group: FourCC(*b"nhdt"),
                root: crate::h1hud::nhdt(&widgets, &texts),
                raw: Vec::new(),
            },
        );
        self.done.insert(tag.id.0, id);
        self.unit_hud_widgets.insert(tag.id.0, n);
        Ok((id, n))
    }

    /// Halo 1's weapon HUD `wphi` (and its children) as an `nhdt` written
    /// from scratch ([`crate::h1hud::weapon_widget`]), built once per
    /// `wphi`: the weapons' part of Halo 1's HUD, for whoever builds the
    /// `weap` to name at `+0x2b0`. Returns the tag and, per entry with no
    /// widget, why.
    ///
    /// A chain with no element at all draws nothing in Halo 1 and gets an
    /// `nhdt` with no widget (V: the flamethrower's on the multiplayer
    /// maps, `weapons\flamethrower\flame thrower` and its child `ui\hud\master
    /// fuel`, list no static, meter, number, crosshair or overlay). Until
    /// converter version 87 it was refused, and the converted flamethrower
    /// kept the shotgun's Halo 2 HUD (`ui\hud\shotgun`, `role_audit_disc`).
    /// A chain whose elements all fail to convert is still refused.
    pub fn weapon_hud(&mut self, wphi: h1map::TagId) -> Result<(TagId, Vec<String>)> {
        use crate::h1hud;
        let tag = self.h1.tag(wphi)?.clone();
        if let Some(id) = self.done.get(&tag.id.0) {
            let skipped = self.weapon_hud_skips.get(&tag.id.0).cloned().unwrap_or_default();
            return Ok((*id, skipped));
        }
        let entries = h1hud::weapon_entries(self.h1, wphi)?;
        let find = |group: &[u8; 4], name: &str| {
            self.base
                .find(group, name)
                .ok_or_else(|| Error::NoTemplate(name.into()))
        };
        let meter = find(b"shad", h1hud::METER_SHADER)?;
        let plain = find(b"shad", h1hud::STATIC_SHADER)?;
        let text_shader = find(b"shad", h1hud::TEXT_SHADER)?;
        let mut widgets = Vec::new();
        let mut texts = Vec::new();
        let mut skipped = Vec::new();
        for e in &entries {
            // Numbers: Halo 2 text, placed as the grenade count is.
            if matches!(e.kind, h1hud::WeaponEntryKind::Number(..)) {
                let w = h1hud::digit_width(self.h1)?;
                let h = h1hud::digit_height(self.h1)?;
                match h1hud::weapon_number(e, w, h, text_shader) {
                    Ok(t) => texts.push(t),
                    Err(why) => skipped.push(format!("{:?}: {why:?}", e.kind)),
                }
                continue;
            }
            let drawn = match e.bitmap {
                Some(b) => {
                    let bt = self.h1.tag(b)?.clone();
                    let bm = h1map::bitmap::Bitmap::read(self.h1, &bt)?;
                    h1hud::sprite(&bm, e.sequence).and_then(|_| match e.kind {
                        h1hud::WeaponEntryKind::Meter(..) => self.meter_bitmap(b),
                        // A static's colour alpha goes into its sprite, as
                        // the unit HUD's; colour 0 draws nothing.
                        h1hud::WeaponEntryKind::Static(_) | h1hud::WeaponEntryKind::Overlay(..) => {
                            h1hud::static_alpha(e.colour).and_then(|a| self.hud_bitmap(b, a))
                        }
                        _ => self.bitmap(b),
                    })
                }
                None => None,
            };
            match h1hud::weapon_widget(e, drawn, meter, plain) {
                Ok(w) => widgets.push(w),
                // The zoom-level readout: a widget per level, each on a copy
                // of the bitmap that starts at the level's sprite.
                Err(h1hud::Skip::ZoomLevels) => {
                    for level in 0..h1hud::ZOOM_LEVELS {
                        let level_drawn = drawn
                            .and(e.bitmap)
                            .and_then(|b| self.sprite_bitmap(b, e.sequence, level));
                        if level > 0 && level_drawn.is_none() {
                            // No sprite for the level: the weapon has none.
                            break;
                        }
                        match h1hud::zoom_widget(e, level, level_drawn, plain) {
                            Ok(w) => widgets.push(w),
                            Err(why) => {
                                skipped.push(format!("{:?}: {why:?}", e.kind));
                                break;
                            }
                        }
                    }
                }
                Err(why) => skipped.push(format!("{:?}: {why:?}", e.kind)),
            }
        }
        if widgets.is_empty() && texts.is_empty() && !entries.is_empty() {
            return Err(Error::Invalid(format!("{}: no drawable element", tag.name)));
        }
        let id = self.tree.push(
            &format!("{}{}", tag.name, h1hud::WEAPON_NHDT_SUFFIX),
            Converted {
                group: FourCC(*b"nhdt"),
                root: h1hud::nhdt(&widgets, &texts),
                raw: Vec::new(),
            },
        );
        self.done.insert(tag.id.0, id);
        self.weapon_hud_skips.insert(tag.id.0, skipped.clone());
        Ok((id, skipped))
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

/// Per weapon type, the class with the most `votes` ((type, class) ->
/// animations), ties to the first class by name.
pub fn weapon_classes(votes: HashMap<(String, String), usize>) -> HashMap<String, String> {
    let mut best: HashMap<String, (usize, String)> = HashMap::new();
    for ((kind, class), n) in votes {
        let e = best.entry(kind).or_insert((0, String::new()));
        if n > e.0 || (n == e.0 && class < e.1) {
            *e = (n, class);
        }
    }
    best.into_iter().map(|(k, (_, c))| (k, c)).collect()
}

/// The model nodes of a vehicle's wheels and treads: each node a gripping
/// powered mass point rides ([`h1::POWERED_GROUND_FRICTION`]; the
/// warthog's four tire nodes, the scorpion's four tread nodes) and every
/// node under one.
///
/// Halo 1 moves a vehicle on its mass points alone, so its collision may
/// reach below them: the warthog's tires go 0.02 below the origin and its
/// scenario spots stand them on the ground. Halo 2 holds a vehicle up on
/// its friction points, springs at the wheel markers, and keeps the body's
/// shapes clear of the ground (V: the retail warthog's and scorpion's
/// physics models' lowest points are 0.18 and 0.22 above the origin; the
/// warthog's has no wheel shape, the scorpion's has its treads above
/// that, [`held_wheels`], by the role). Built whole into the body, the converted
/// warthog's tires spawned up to 0.14 into Blood Gulch's ground (a game
/// run, `havok.contacts`), and Havok threw it over.
pub fn wheel_nodes(phys: &h1::Physics, nodes: &[h1models::h1::Node]) -> Vec<i16> {
    let grips: Vec<i16> = phys
        .mass_points
        .iter()
        .filter(|m| {
            m.powered_mass_point >= 0
                && phys
                    .powered_flags
                    .get(m.powered_mass_point as usize)
                    .is_some_and(|f| f & h1::POWERED_GROUND_FRICTION != 0)
        })
        .map(|m| m.model_node)
        .filter(|&n| n > 0)
        .collect();
    (0..nodes.len() as i16)
        .filter(|&n| {
            let mut at = n;
            for _ in 0..nodes.len() {
                if grips.contains(&at) {
                    return true;
                }
                match nodes.get(at as usize) {
                    Some(x) if x.parent >= 0 => at = x.parent,
                    _ => break,
                }
            }
            false
        })
        .collect()
}

/// A vehicle's wheels and treads in its body, where its role's physics
/// models have wheel shapes ([`crate::rebuild::VehicleRole::wheel_shapes`];
/// object space, `wheels` one list of convex pieces per wheel node): each node's pieces cut at `floor`,
/// the lowest point of the rest of the body, and what is left enclosed in
/// one piece; a node with nothing above `floor` gives none.
///
/// Halo 2 holds a vehicle up on its friction points and keeps its body's
/// shapes clear of the ground, but the parts of the wheels above the hull
/// are solid. V, the retail scorpion's `phmo` (coagulation): two tread
/// shapes out to 1.13 either side, where its hull reaches 0.53, from 0.22
/// above the origin, where the hull's lowest point is 0.25; with the
/// converted scorpion's treads left out (versions 77 to 85) the player
/// walked through them (a game run on Blood Gulch, 2026-11-19), its `coll`
/// carrying them. Under the hull a wheel would meet the ground before the
/// springs did: built whole into the body, the converted warthog's tires
/// sank up to 0.144 into the ground at spawn and the depenetration threw
/// it about 4.8 units (game run, 2026-11-19).
pub fn held_wheels(wheels: &[Vec<crate::physics::Hull>], floor: f32) -> Vec<crate::physics::Hull> {
    if !floor.is_finite() {
        return Vec::new();
    }
    wheels
        .iter()
        .filter_map(|cells| {
            let cut: Vec<_> = cells.iter().filter_map(|h| h.above(floor)).collect();
            match cut.len() {
                0 => None,
                1 => cut.into_iter().next(),
                _ => crate::physics::enclosing(&cut),
            }
        })
        .collect()
}

/// Halo 1's default unit camera track, in every Halo 1 map: the track a
/// seat uses when neither it nor its unit names one.
pub const DEFAULT_UNIT_CAMERA_TRACK: &str = "globals\\default unit camera track";
