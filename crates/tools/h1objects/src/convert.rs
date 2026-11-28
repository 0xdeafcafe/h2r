//! Halo 1 objects as Halo 2 tags.
//!
//! Object definitions carry source fields and references. The native program
//! and physics layouts are described in `docs/halo1/objects.md`.

use std::collections::HashMap;

use h1convert::shad::program::model_defaults::ModelDefaults;
use h1map::{H1Map, Tag};
use h1_assets::models::Converted;
use h1_assets::models::mode::ModelOptions;
use map::build::{Block, FixupKind, RawId, RawRef, TagEntry, TagPayload, TagSet};
use map::{FourCC, TagId};

use crate::base::Base;
use crate::coll::convert_collision;
use crate::h1::{self, Unit};
use crate::{health_hud, scripted_props};
use crate::{Error, Result};

#[path = "convert_widgets.rs"]
mod widgets_conv;
#[path = "convert_attachments.rs"]
mod attachments_conv;
#[path = "convert_arsenal.rs"]
mod arsenal_conv;
#[path = "convert_scripted_props.rs"]
mod scripted_props_conv;
#[path = "convert_powerups.rs"]
mod powerups_conv;
#[path = "convert_model_defaults.rs"]
mod model_defaults_conv;
#[path = "convert_model_effects.rs"]
mod model_effects_conv;
#[path = "convert_shader_images.rs"]
mod shader_images;
#[path = "convert_scope.rs"]
mod scope_conv;
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

/// The rebuilt player-emblem instance, using Halo 2's emblem draw program.
pub const EMBLEM_SHADER: &str = r"h1\player_emblem";

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
    /// A failed shader prevents installation even when a placement caller
    /// collects object errors and continues preparing the rest of the palette.
    pub model_shader_error: Option<String>,
    pub tags: Vec<(String, Converted)>,
    pub reports: Vec<ObjectReport>,
    /// Halo 1 sounds the tags play (effect parts), converted, by path;
    /// the tags name them by [`sound`] placeholders. They need the map's
    /// sound gestalt, so [`Tree::install`] adds them first.
    pub sounds: Vec<(String, h1_assets::sounds::ConvertedSound)>,
    /// Halo 1 looping sounds the tags attach, read and converted; named
    /// by [`looping`] placeholders.
    pub loops: Vec<h1_assets::sounds::looping::Looping>,
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
    /// Body and shield scale pairs referenced by converted damage effects.
    pub(crate) player_damage_scales: std::collections::BTreeSet<crate::player_damage::Scales>,
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

/// Mass per cubic world unit of a built physics model with no Halo 1
/// mass: water's 1000 kg/m^3 over a world unit of 3.048 m.
pub const DENSITY: f32 = 28_317.0;

/// What a built physics model is for ([`Converter::object_physics`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicsKind {
    /// One body from the collision hulls or source mass points. Its wheels and
    /// treads are held in the body above the hull's lowest point
    /// ([`held_wheels`]),
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
    nodes: &[h1_assets::models::h1::Node],
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
            r: h1_assets::models::mode::quat_matrix(n.rotation),
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

fn visit_ids(b: &mut Block, resolve: &mut impl FnMut(&mut TagId) -> Result<()>) -> Result<()> {
    for f in &mut b.fixups {
        match &mut f.kind {
            FixupKind::Block { child, .. } | FixupKind::Data(child) | FixupKind::Pointer(child) => {
                visit_ids(child, resolve)?;
            }
            FixupKind::TagId(id) | FixupKind::TagRef(id) => resolve(id)?,
            _ => {}
        }
    }
    Ok(())
}

fn rewrite_audio(b: &mut Block, kind: u32, ids: &[TagId]) -> Result<()> {
    visit_ids(b, &mut |id| {
        if id.0 & 0xffff_0000 == kind {
            *id = *ids
                .get(id.index())
                .ok_or_else(|| Error::Invalid(format!("unresolved tree datum {id:?}")))?;
        }
        Ok(())
    })
}

impl Tree {
    /// Install all object audio or return the error to a campaign conversion.
    /// The caller discards its in-progress tag set on failure.
    pub fn try_install_audio(&mut self, set: &mut TagSet) -> Result<()> {
        let (sounds, loops) = self.add_audio(set).map_err(Error::Invalid)?;
        self.resolve_audio(&sounds, &loops)?;
        self.sounds.clear();
        self.loops.clear();
        Ok(())
    }

    fn resolve_audio(&mut self, sounds: &[TagId], loops: &[TagId]) -> Result<()> {
        for (_, t) in &mut self.tags {
            rewrite_audio(&mut t.root, SOUND, sounds)?;
            rewrite_audio(&mut t.root, LOOP, loops)?;
        }
        Ok(())
    }

    fn add_audio(&self, set: &mut TagSet) -> std::result::Result<(Vec<TagId>, Vec<TagId>), String> {
        let snd = FourCC(*b"snd!");
        let new: Vec<(String, h1_assets::sounds::ConvertedSound)> = self
            .sounds
            .iter()
            .filter(|(n, _)| set.converted(snd, n).is_none())
            .cloned()
            .collect();
        if !new.is_empty() {
            h1_assets::sounds::install_sounds(set, new).map_err(|e| e.to_string())?;
        }
        let sounds = self
            .sounds
            .iter()
            .map(|(n, _)| set.converted(snd, n).ok_or(format!("{n}: not installed")))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let loops = h1_assets::sounds::looping::install_looping(set, &self.loops).map_err(|e| e.to_string())?;
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
        self.install_audio_for(set, &ids)?;
        Ok(ids)
    }

    /// The tags alone of [`Tree::install`]: added in order in the slots
    /// after the set's last, their datums returned, their sound
    /// placeholders left for [`Tree::install_audio_for`]. The Halo 1
    /// sandbox adds its collections between the two, so the tags and the
    /// collections hold the datums its placements name.
    pub fn install_tags(&mut self, set: &mut TagSet) -> Result<Vec<TagId>> {
        if let Some(reason) = &self.model_shader_error {
            return Err(Error::Invalid(reason.clone()));
        }
        crate::player_damage::install(set, &self.player_damage_scales)?;
        let first = set.slots.len();
        let ids: Vec<TagId> = (0..self.tags.len())
            .map(|k| {
                let slot = first + k;
                TagId(u32::from(set.salt_base.wrapping_add(slot as u16)) << 16 | slot as u32)
            })
            .collect();
        for (k, (name, mut tag)) in std::mem::take(&mut self.tags).into_iter().enumerate() {
            rewrite_audio(&mut tag.root, LOCAL, &ids)?;
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
    pub fn install_audio_for(&mut self, set: &mut TagSet, ids: &[TagId]) -> Result<()> {
        let (sounds, loops) = self.add_audio(set).map_err(Error::Invalid)?;
        self.sounds.clear();
        self.loops.clear();
        for id in ids {
            if let Some(TagPayload::Built(root)) =
                set.slots[id.index()].tag.as_mut().map(|e| &mut e.payload)
            {
                rewrite_audio(root, SOUND, &sounds)?;
                rewrite_audio(root, LOOP, &loops)?;
            }
        }
        Ok(())
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
        if let Some(reason) = self.model_shader_error.take() {
            return Err(Error::Invalid(reason));
        }
        if let Some(name) = self
            .sounds
            .first()
            .map(|s| &s.0)
            .or_else(|| self.loops.first().map(|l| &l.name))
        {
            return Err(h1convert::Error::no_rebuild(
                &set.name,
                name,
                "pinned installation needs slots for source audio",
            )
            .into());
        }
        // No source audio rides along, so a sound or looping-sound
        // placeholder left in a tag has nothing to point at: a conversion
        // error naming the tag, never a null reference. Checked before the
        // set changes.
        for (name, tag) in &mut self.tags {
            rewrite_audio(&mut tag.root, SOUND, &[])
                .and_then(|()| rewrite_audio(&mut tag.root, LOOP, &[]))
                .map_err(|e| h1convert::Error::no_rebuild(&set.name, name.as_str(), e))?;
        }
        if donors.len() < self.tags.len() {
            return Err(Error::Invalid(format!(
                "{} tags but {} donor slots",
                self.tags.len(),
                donors.len()
            )));
        }
        crate::player_damage::install(set, &self.player_damage_scales)?;
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
            rewrite_audio(&mut root, LOCAL, ids)?;
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
            s.name = Some(name.clone());
            set.record_conversion(tag.group, &name, id);
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
            FixupKind::TagId(t) | FixupKind::TagRef(t) if *t == h1_assets::models::SELF_ID => *t = id,
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
fn add_point_markers(model: &mut h1_assets::models::h1::Model, points: &[PoweredPoint]) {
    let perms = model.regions.first().map_or(1, |r| r.permutations.len().max(1));
    for p in points {
        if model.markers.iter().any(|m| m.name == p.point.marker) {
            continue;
        }
        model.markers.push(h1_assets::models::h1::Marker {
            name: p.point.marker.clone(),
            instances: (0..perms)
                .map(|k| h1_assets::models::h1::MarkerInstance {
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

/// The `hlmt` field a rebuilt biped's converted dialogue goes in: the
/// model's dialogue (`+0x8c`, `udlg`; V: where retail models name theirs,
/// `docs/formats/objects.md`).
pub const DIALOGUE_FIELD: &str = "udlg_ref_8c";

/// A rebuilt unit's material effects (`obje +0x54`, `foot`).
pub const FOOT_FIELD: &str = "unit.obje.material_effects";

/// How the [`Converter`] builds tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// The campaign HUD supplies the unit panels' authored tint at draw time.
    pub unit_hud_draw_colour: bool,
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
    /// Per rebuilt machine or light, the variants used by fixture placements.
    fixture_variants: HashMap<(TagId, [u32; 4]), TagId>,
    /// Render models by source, weapon counter maximum and object texture override.
    render_models: HashMap<(h1map::TagId, Option<i16>, i16), TagId>,
    permuted_shaders: HashMap<(h1map::TagId, i16), TagId>,
    emblem_shader: Option<TagId>,
    colls: HashMap<(u32, bool), TagId>,
    coll_names: HashMap<(u32, bool), Vec<(String, Vec<String>)>>,
    /// Per converted `coll`, the `hlmt` materials it needs
    /// ([`crate::rebuild::model_materials`]).
    coll_materials: HashMap<(u32, bool), Vec<crate::rebuild::ModelMaterial>>,
    /// Each template `nhdt` given the health meter.
    health_huds: HashMap<TagId, TagId>,
    /// Lower and upper native panels per source flag cloth.
    flag_panels: HashMap<u32, [TagId; 2]>,
    /// Widgets per converted `unhi` ([`Self::unit_hud`]).
    unit_hud_widgets: HashMap<u32, usize>,
    /// Direct-image sequences for crosshair overlays with source flag 1 set.
    direct_hud_bitmaps: HashMap<u32, TagId>,
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
    counter_shaders: HashMap<(u32, i16, Option<i16>), TagId>,
    /// Converted sounds and looping sounds by Halo 1 tag, as indices
    /// into [`Tree::sounds`] and [`Tree::loops`] (`None`: did not convert).
    sound_index: HashMap<u32, usize>,
    loop_index: HashMap<u32, usize>,
    /// Compiled shader bindings shared by particles on this base.
    particle_shaders: Option<Vec<crate::particles::ParticleShader>>,
    /// Active conversions. A back edge names its source slot until the
    /// completed tag has a local datum and can resolve that reference.
    active: HashMap<u32, bool>,
    /// Projectile detonation effects being converted
    /// ([`Self::effect_after_detonation`]), so one that leads back to
    /// itself is a named error instead of a recursion.
    detonations_open: Vec<u32>,
    /// Projectile effects whose primary damage is applied by the projectile.
    detonation_effects: HashMap<(u32, usize), TagId>,
    /// Per Halo 1 `pctl`, its emitter types and converted particle states.
    pctl_done: HashMap<u32, particles_conv::ParticleTypes>,
    /// Looping effects that carry a particle system at a given object marker.
    attached_particles: HashMap<(u32, String), Vec<TagId>>,
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
            fixture_variants: HashMap::new(),
            render_models: HashMap::new(),
            permuted_shaders: HashMap::new(),
            emblem_shader: None,
            colls: HashMap::new(),
            coll_names: HashMap::new(),
            coll_materials: HashMap::new(),
            health_huds: HashMap::new(),
            unit_hud_widgets: HashMap::new(),
            flag_panels: HashMap::new(),
            direct_hud_bitmaps: HashMap::new(),
            hud_bitmaps: HashMap::new(),
            sprite_bitmaps: HashMap::new(),
            meter_bitmaps: HashMap::new(),
            masked_bitmaps: HashMap::new(),
            brightest_bitmaps: HashMap::new(),
            cutout_bumps: HashMap::new(),
            change_colour_bitmaps: HashMap::new(),
            counter_shaders: HashMap::new(),
            sound_index: HashMap::new(),
            loop_index: HashMap::new(),
            particle_shaders: None,
            active: HashMap::new(),
            detonations_open: Vec::new(),
            detonation_effects: HashMap::new(),
            pctl_done: HashMap::new(),
            attached_particles: HashMap::new(),
            dialogue_bytes: 0,
            weapon_classes: None,
        }
    }

    /// Rebuilds the native vocalization table and the exact source slots.
    pub fn dialogue(&mut self, tag: &Tag, rep: &mut ObjectReport) -> Result<TagId> {
        if let Some(id) = self.done.get(&tag.id.0) {
            return Ok(*id);
        }
        let d = self.read_dialogue(tag)?;
        let wanted = crate::rebuild::dialogue_lines(&d);
        for (name, _, _) in &wanted {
            if !self.base.dialogue_names.iter().any(|entry| entry.as_str() == *name) {
                return Err(h1convert::Error::no_rebuild(
                    &self.h1.header().name,
                    &tag.name,
                    format!("base dialogue vocabulary lacks {name}"),
                )
                .into());
            }
        }
        let mut source = Vec::with_capacity(d.lines.len());
        for &(offset, id) in &d.lines {
            let line = self.h1_tag(id)?;
            let sound = self.dialogue_sound(line)?.ok_or_else(|| {
                h1convert::Error::no_rebuild(
                    &self.h1.header().name,
                    &line.name,
                    "dialogue exceeds the configured sound budget",
                )
            })?;
            source.push((offset, sound));
        }
        let native = wanted
            .iter()
            .map(|&(name, offset, _)| {
                let index = source
                    .binary_search_by_key(&offset, |&(at, _)| at)
                    .expect("source slot read from this dialogue");
                (name, offset, source[index].1)
            })
            .collect::<Vec<_>>();
        let root = crate::rebuild::dialogue(&self.base.dialogue_names, &native, &source)?;
        rep.no_home.push(format!(
            "{}: {} native vocalizations, {} source slots",
            tag.name,
            native.len(),
            source.len()
        ));
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"udlg"),
                root,
                raw: Vec::new(),
            },
        );
        self.done.insert(tag.id.0, id);
        Ok(id)
    }

    fn read_dialogue(&self, tag: &Tag) -> Result<h1::Dialogue> {
        h1::Dialogue::read(self.h1, tag)
            .map_err(|e| h1convert::Error::no_rebuild(&self.h1.header().name, &tag.name, e).into())
    }

    fn biped_dialogue(
        &mut self,
        tag: &Tag,
        bytes: &[u8],
        rep: &mut ObjectReport,
    ) -> Result<Vec<(i16, TagId)>> {
        let variants = h1::dialogue_variants(self.h1, bytes)
            .map_err(|e| h1convert::Error::no_rebuild(&self.h1.header().name, &tag.name, e))?;
        let mut voices = Vec::with_capacity(variants.len());
        for (selector, id) in variants {
            let voice = self.h1_tag(id)?;
            let source = self.read_dialogue(voice)?;
            if let Some((slot, id)) = crate::rebuild::unbound_dialogue_line(&source) {
                let sound = self.h1_tag(id)?;
                return Err(h1convert::Error::no_rebuild(
                    &self.h1.header().name,
                    &sound.name,
                    format!(
                        "{} slot {slot:#x} has no automatic dialogue event on {}",
                        voice.name, tag.name
                    ),
                )
                .into());
            }
            voices.push((selector, self.dialogue(voice, rep)?));
        }
        Ok(voices)
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
        for k in f.kinds.iter().take(crate::rebuild::FOOT_KINDS) {
            let mut out = crate::rebuild::FootKind::default();
            for (m, (_, name)) in k.iter().zip(crate::arsenal::MATERIALS.iter()) {
                let Some(index) = self.base.materials.iter().position(|x| x == name) else {
                    if m.effect.is_some() || m.sound.is_some() {
                        return Err(h1convert::Error::no_rebuild(
                            &self.h1.header().name,
                            &tag.name,
                            format!("missing global material {name}"),
                        )
                        .into());
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
                    out.sounds.push(line(self.sound(self.h1_tag(s)?, rep)?));
                }
                if let Some(e) = m.effect
                    && !out.effects.iter().any(|l| l.material == *name)
                {
                    out.effects.push(line(self.effect(self.h1_tag(e)?)?));
                }
            }
            kinds.push(out);
        }
        rep.no_home.push(format!(
            "foot {}: {} kinds, {} sounds and {} effects by Halo 2 material",
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
            .find(|r| r.group == h1map::Group::new(b"foot") && (group != *b"bipd" || r.offset == 0x38c))
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
    fn dialogue_sound(&mut self, tag: &Tag) -> Result<Option<TagId>> {
        if let Some(&k) = self.sound_index.get(&tag.id.0) {
            return Ok(Some(sound(k)));
        }
        let c = h1_assets::sounds::Sound::read(self.h1, tag)
            .and_then(|s| h1_assets::sounds::convert_sound(&s, self.h1.bytes()))
            .map_err(|e| h1convert::Error::no_rebuild(&self.h1.header().name, &tag.name, e))?;
        let size: u64 = c
            .ranges
            .iter()
            .flat_map(|r| &r.permutations)
            .map(|p| p.size() as u64)
            .sum();
        if self.dialogue_bytes + size > self.options.dialogue_budget {
            return Ok(None);
        }
        self.dialogue_bytes += size;
        self.queue_sound(tag, c).map(Some)
    }

    /// A Halo 1 decal from the `deca` definition
    /// ([`crate::rebuild::decal`]), converted once, with its next decal and
    /// its bitmap (`h1bitmaps`).
    pub fn decal(&mut self, tag: &Tag) -> Result<TagId> {
        self.with_dependencies(tag, |c| c.decal_uncached(tag))
    }

    fn decal_uncached(&mut self, tag: &Tag) -> Result<TagId> {
        let x = h1::Decal::read(self.h1, tag)?;
        let next = x.next.map(|id| self.decal(self.h1_tag(id)?)).transpose()?;
        let rep = Self::report(tag, "none: rebuilt from tags::defs");
        let bitmap = self.source_bitmap(x.bitmap)?;
        let root = crate::rebuild::decal(&x, next, bitmap)?;
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"deca"),
                root,
                raw: Vec::new(),
            },
        );
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
        let mut root = crate::rebuild::light(&l)?;
        let rep = Self::report(tag, "none: rebuilt from tags::defs");
        if let Some(id) = l.lens_flare {
            let lens = self.lens_flare(self.h1_tag(id)?)?;
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
        self.done.insert(tag.id.0, id);
        Ok(id)
    }

    /// A Halo 1 `snd!` converted with `h1sounds` into [`Tree::sounds`]
    /// (once), as its placeholder ([`sound`]); `None` when it does not
    /// convert (the reason in `rep`).
    pub fn sound(&mut self, tag: &Tag, _rep: &mut ObjectReport) -> Result<TagId> {
        if let Some(&k) = self.sound_index.get(&tag.id.0) {
            return Ok(sound(k));
        }
        let converted = h1_assets::sounds::Sound::read(self.h1, tag)
            .and_then(|s| h1_assets::sounds::convert_sound(&s, self.h1.bytes()))
            .map_err(|e| h1convert::Error::no_rebuild(&self.h1.header().name, &tag.name, e))?;
        self.queue_sound(tag, converted)
    }

    fn queue_sound(&mut self, tag: &Tag, converted: h1_assets::sounds::ConvertedSound) -> Result<TagId> {
        let target = converted.promotion.as_ref().and_then(|p| p.target.clone());
        let dependencies = h1_assets::sounds::convert::read_source_sounds(self.h1, target, |name| {
            self.h1.find(h1_assets::sounds::h1::SND, name).is_some_and(|source| {
                source.id == tag.id || self.sound_index.contains_key(&source.id.0)
            })
        })
        .map_err(|e| h1convert::Error::no_rebuild(&self.h1.header().name, &tag.name, e))?;
        let k = self.tree.sounds.len();
        self.tree.sounds.push((tag.name.clone(), converted));
        self.sound_index.insert(tag.id.0, k);
        for (name, sound) in dependencies {
            let source = self
                .h1
                .find(h1_assets::sounds::h1::SND, &name)
                .expect("read source sound");
            self.sound_index.insert(source.id.0, self.tree.sounds.len());
            self.tree.sounds.push((name, sound));
        }
        Ok(sound(k))
    }

    pub fn looping_sound(&mut self, tag: &Tag) -> Result<TagId> {
        if let Some(&k) = self.loop_index.get(&tag.id.0) {
            return Ok(looping(k));
        }
        let converted = h1_assets::sounds::looping::read_looping(self.h1, tag)
            .map_err(|e| h1convert::Error::no_rebuild(&self.h1.header().name, &tag.name, e))?;
        let k = self.tree.loops.len();
        self.tree.loops.push(converted);
        self.loop_index.insert(tag.id.0, k);
        Ok(looping(k))
    }

    /// A Halo 1 effect from the `effe` definition
    /// ([`crate::rebuild::effect`]), converted once. Its parts keep what
    /// converts here: sounds, projectiles, decals, lights, debris and
    /// particle systems. Parts without a converter are listed in the report.
    pub fn effect(&mut self, tag: &Tag) -> Result<TagId> {
        self.with_dependencies(tag, |c| c.effect_uncached(tag, None))
    }

    fn with_dependencies(
        &mut self,
        tag: &Tag,
        build: impl FnOnce(&mut Self) -> Result<TagId>,
    ) -> Result<TagId> {
        if let Some(&id) = self.done.get(&tag.id.0) {
            return Ok(id);
        }
        let forward = TagId(0xfffa_0000 | (tag.id.0 & 0xffff));
        if let Some(referenced) = self.active.get_mut(&tag.id.0) {
            *referenced = true;
            return Ok(forward);
        }
        self.active.insert(tag.id.0, false);
        let result = build(self);
        let referenced = self.active.remove(&tag.id.0).expect("active source tag");
        let id = result
            .map_err(|e| h1convert::Error::no_rebuild(&self.h1.header().name, &tag.name, e))?;
        if referenced {
            for (_, tag) in &mut self.tree.tags {
                visit_ids(&mut tag.root, &mut |target| {
                    if *target == forward {
                        *target = id;
                    }
                    Ok(())
                })?;
            }
        }
        self.done.insert(tag.id.0, id);
        Ok(id)
    }

    fn effect_after_detonation(&mut self, tag: &Tag, primary: usize) -> Result<TagId> {
        let key = (tag.id.0, primary);
        if let Some(id) = self.detonation_effects.get(&key) {
            return Ok(*id);
        }
        if self.detonations_open.contains(&tag.id.0) {
            return Err(h1convert::Error::no_rebuild(
                &self.h1.header().name,
                &tag.name,
                "detonation effect converts through itself",
            )
            .into());
        }
        self.detonations_open.push(tag.id.0);
        let got = self.effect_uncached(tag, Some(primary));
        self.detonations_open.pop();
        let id = got.map_err(|e| h1convert::Error::no_rebuild(&self.h1.header().name, &tag.name, e))?;
        self.detonation_effects.insert(key, id);
        Ok(id)
    }

    fn effect_uncached(
        &mut self,
        tag: &Tag,
        primary_detonation_part: Option<usize>,
    ) -> Result<TagId> {
        let e = h1::Effect::read(self.h1, tag)?;
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        let mut targets = Vec::new();
        for (index, p) in e.events.iter().flat_map(|ev| &ev.parts).enumerate() {
            let g = p.group.0.to_be_bytes();
            let got = match p.reference {
                None => None,
                // The projectile applies its primary damage itself.
                Some(_) if &g == b"jpt!" && primary_detonation_part == Some(index) => {
                    targets.push(None);
                    continue;
                }
                Some(id) => {
                    let source = self.h1_tag(id)?;
                    match &g {
                        b"snd!" => Some(self.sound(source, &mut rep)?),
                        b"deca" => Some(self.decal(source)?),
                        b"ligh" => Some(self.light(source)?),
                        b"garb" | b"proj" => Some(self.object(source)?),
                        b"jpt!" => Some(self.damage(id)?),
                        b"pctl" => None, // Emitted in the event's particle systems below.
                        _ => {
                            return Err(h1convert::Error::no_rebuild(
                                &self.h1.header().name,
                                &source.name,
                                format!("unsupported {} effect part", p.group),
                            )
                            .into());
                        }
                    }
                }
            };
            targets.push(got.map(|tag| crate::rebuild::PartTarget {
                type_group: crate::rebuild::part_type_group(g),
                group: g,
                tag,
            }));
        }
        let mut next = targets.into_iter();
        let root = crate::rebuild::effect(&e, &mut |_| next.next().flatten())?;
        let (root, roles) =
            crate::provenance::capture(|| self.effect_particle_systems(tag, root, &mut rep));
        let root = root?;
        self.tree.reports.push(rep);
        let name = if primary_detonation_part.is_some() {
            format!("{}_detonation", tag.name)
        } else {
            tag.name.clone()
        };
        let id = self.tree.push(
            &name,
            Converted {
                group: FourCC(*b"effe"),
                root,
                raw: Vec::new(),
            },
        );
        self.tree.note_roles(id, roles);
        Ok(id)
    }

    /// Converts any supported object tag (once; later calls return the
    /// same tag).
    pub fn object(&mut self, tag: &Tag) -> Result<TagId> {
        self.with_dependencies(tag, |c| c.object_uncached(tag))
    }

    fn object_uncached(&mut self, tag: &Tag) -> Result<TagId> {
        let group = tag.group.0.to_be_bytes();
        if !h1::OBJECT_TYPES.contains(&&group) { return Err(Error::WrongGroup(tag.group.to_string(), *b"obje")); }
        let widgets = self.native_widgets(tag)?;
        let id = match &tag.group.0.to_be_bytes() {
            b"weap" => self.weapon(tag),
            b"proj" => self.projectile(tag),
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
        }?;
        if let Some(field) = widgets {
            let owner = local_index(id).ok_or_else(|| Error::Invalid("widget owner is not local".into()))?;
            let root = &mut self.tree.tags[owner].1.root;
            root.fixups.retain(|f| f.offset != field.offset);
            root.fixups.push(field);
        }
        Ok(id)
    }

    fn h1_tag(&self, id: h1map::TagId) -> Result<&'a Tag> {
        Ok(self.h1.tag(id)?)
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
        name: &str,
        rounds: Option<i16>,
    ) -> Result<Option<TagId>> {
        let is_biped = h1::OBJECT_TYPES.get(obj.object_type as usize) == Some(&b"bipd");
        if let Some(id) = obj.modifier_shader.filter(|_| !is_biped) {
            let dependency = self.h1_tag(id)?;
            return Err(h1convert::Error::no_rebuild(
                &self.h1.header().name,
                &dependency.name,
                format!(
                    "object modifier shader {} on {name} has no output binding",
                    dependency.group
                ),
            )
            .into());
        }
        let Some(mode_id) = obj.model else {
            return Ok(None);
        };
        let scenery = h1::OBJECT_TYPES.get(obj.object_type as usize) == Some(&b"scen");
        let mode_tag = self.h1_tag(mode_id)?;
        let (mut h1_model, _) =
            h1_assets::models::model_from_map(self.h1, mode_tag, &ModelOptions::default())?;
        let key = (mode_id, rounds, obj.texture_override);
        let mode = match self.render_models.get(&key) {
            Some(&mode) => mode,
            None => {
                // A vehicle's powered mass points become markers, which
                // its friction or hover points name (crate::rebuild::Point).
                if h1::OBJECT_TYPES.get(obj.object_type as usize) == Some(&b"vehi") {
                    let points = self.powered_points(obj, &h1_model)?;
                    add_point_markers(&mut h1_model, &points);
                }
                let mut shaders =
                    self.model_shaders(rep, &h1_model, rounds, obj.texture_override)?;
                // A player biped's body carries the player's emblem, as the
                // Chief's does (h1_assets::models::emblem); after model_shaders, so
                // its shader entry (no Halo 1 tag) is not looked up there.
                let player = h1::OBJECT_TYPES.get(obj.object_type as usize) == Some(&b"bipd")
                    && health_hud::is_player_biped(name);
                if player {
                    self.emblem(&rep.source, &mut h1_model, &mut shaders)?;
                }
                // One material per Halo 1 shader, as convert_model writes.
                let opts = ModelOptions {
                    shaders,
                    ..ModelOptions::default()
                };
                let mut mode = h1_assets::models::mode::convert_model(
                    &h1_model,
                    h1_assets::models::short_name(&mode_tag.name),
                    &opts,
                )?;
                crate::rebuild::mode_materials(&mut mode.root, &opts.shaders)?;
                let render_name = if obj.texture_override == 0 {
                    mode_tag.name.clone()
                } else {
                    format!("{}_texture_{}", mode_tag.name, obj.texture_override)
                };
                let mode = self.tree.push(&render_name, mode);
                self.render_models.insert(key, mode);
                mode
            }
        };
        // Retail writes the coll's BSP physics slots (crate::coll) exactly
        // for scenery with no phmo: its weapons, equipment, devices and
        // bipeds without a phmo have none. Converted scenery has no phmo:
        // the engine gives scenery no Havok body, so a placed scenery
        // object's collision joins the level's instead
        // (h1convert::scenery_collision).
        let slots = scenery;
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
                        let opts = h1_assets::models::jmad::GraphOptions {
                            mass_point_markers: if vehicle {
                                self.mass_point_markers(obj)?
                            } else {
                                Vec::new()
                            },
                        };
                        let (_, report, mut conv) =
                            h1_assets::models::graph_from_map_with(self.h1, t, Some(mode_tag), &opts)?;
                        // Halo 2's suspension names a marker; one whose
                        // Halo 1 mass point has none (not powered, or out
                        // of range) has no rebuild yet.
                        if let Some(a) = report.suspension_without_marker.first() {
                            return Err(h1convert::Error::no_rebuild(
                                &self.h1.header().name,
                                &t.name,
                                format!("suspension {a:?}: its mass point has no marker"),
                            )
                            .into());
                        }
                        self.graph_sounds(t, &mut conv)?;
                        Some(self.cached(t, conv)?)
                    }
                }
            }
            None => None,
        };

        self.rebuilt_model(rep, obj, &h1_model, mode, coll, jmad, name)
    }

    /// The `hlmt` from its typed definition ([`crate::rebuild::model`]):
    /// the same model chain, nodes and region map the template path
    /// writes, every other field at its default.
    fn rebuilt_model(
        &mut self,
        rep: &mut ObjectReport,
        obj: &h1::Object,
        h1_model: &h1_assets::models::h1::Model,
        mode: TagId,
        coll: Option<(TagId, (u32, bool))>,
        jmad: Option<TagId>,
        name: &str,
    ) -> Result<Option<TagId>> {
        let (count, nodes) = h1_assets::models::mode::hlmt_nodes(h1_model);
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
    /// from their definitions; Halo 1 supplies the object header. Its `coll` has BSP
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
        let hlmt = self.model(&mut rep, &object, &tag.name, None)?;
        rep.physics = Some(match object.collision {
            Some(_) => {
                "none: the coll's BSP physics slots; placements' collision joins the level's".into()
            }
            None => "none (no Halo 1 collision)".into(),
        });
        let predicted = self.predicted(hlmt);
        let root = crate::rebuild::scenery(&object, hlmt, &predicted)?;
        let root = self.with_object_attachments(*b"scen", "object", root, bytes, &object, &mut rep)?;
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"scen"),
                root,
                raw: Vec::new(),
            },
        );
        Ok(id)
    }

    /// The player graph's standing class for the weapon label. Other units
    /// supply the classes of weapons the player graph does not list.
    pub fn weapon_class(&mut self, label: &str) -> Result<Option<String>> {
        if self.weapon_classes.is_none() {
            let mut votes: HashMap<(String, String), usize> = HashMap::new();
            let antr = h1map::Group::new(b"antr");
            for t in self.h1.tags().iter().filter(|t| t.group == antr) {
                for name in h1_assets::models::graph_names(self.h1, t)? {
                    let n = h1_assets::models::names::map_name(&name);
                    if n.class != "any" && n.kind != "any" {
                        *votes.entry((n.kind, n.class)).or_default() += 1;
                    }
                }
            }
            let mut classes = weapon_classes(votes);
            // Vehicle-mounted weapons may have no clip named for the gun.
            for t in self.h1.tags().iter().filter(|t| t.group == antr) {
                for (kind, class) in h1_assets::models::graph_weapon_types(self.h1, t)? {
                    if h1_assets::models::names::CLASSES.contains(&class.as_str()) {
                        classes.entry(kind).or_insert(class);
                    }
                }
            }
            classes.extend(h1_assets::models::player_weapon_types(self.h1)?);
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

    /// Adds the player's emblem part and builds its material once per tree.
    fn emblem(
        &mut self,
        source: &str,
        model: &mut h1_assets::models::h1::Model,
        shaders: &mut Vec<TagId>,
    ) -> Result<()> {
        use h1convert::shad::{EMBLEM_BITMAP, EMBLEM_STEM, emblem};
        let error = |why: String| h1convert::Error::no_rebuild(&self.h1.header().name, source, why);
        let at = &h1_assets::models::emblem::LEFT_SHOULDER;
        let index = h1_assets::models::emblem::add_emblem(model, at)
            .ok_or_else(|| error(format!("emblem: no surface bound to {}", at.node)))?;
        let shader = if let Some(id) = self.emblem_shader {
            id
        } else {
            let stem = self
                .base
                .find(b"stem", EMBLEM_STEM)
                .ok_or_else(|| error(format!("emblem program {EMBLEM_STEM} is missing")))?;
            let bitmap = self
                .base
                .find(b"bitm", EMBLEM_BITMAP)
                .ok_or_else(|| error(format!("emblem atlas {EMBLEM_BITMAP} is missing")))?;
            let root = emblem(stem, bitmap).map_err(|e| error(e.to_string()))?;
            let id = self.tree.push(
                EMBLEM_SHADER,
                Converted {
                    group: FourCC(*b"shad"),
                    root,
                    raw: Vec::new(),
                },
            );
            self.emblem_shader = Some(id);
            id
        };
        debug_assert_eq!(shaders.len(), index);
        shaders.push(shader);
        Ok(())
    }

    /// One rebuilt material per Halo 1 shader. Null-base model shaders
    /// use the rasterizer defaults and their own colours. Other supported
    /// shaders use their own maps and the appropriate lighting or transparent
    /// template. An unresolved material is a conversion error.
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
        model: &h1_assets::models::h1::Model,
        rounds: Option<i16>,
        texture_override: i16,
    ) -> Result<Vec<TagId>> {
        let mut shaders = Vec::with_capacity(model.shaders.len());
        for (slot, entry) in model.shaders.iter().enumerate() {
            // The object's texture override selects the image of every
            // multi-image material (shader_images::material_permutation).
            let entry = &h1_assets::models::h1::ShaderRef {
                permutation: shader_images::material_permutation(
                    texture_override,
                    entry.permutation,
                ),
                ..*entry
            };
            let source = self.h1.tag(entry.tag);
            let result = match source {
                Ok(tag) => self.model_shader(tag, entry, rounds).map_err(|e| format!(
                    "{}: {} {}: {e}", self.h1.header().name, tag.group, tag.name
                )),
                Err(e) => Err(format!("{}: {} material {slot}, {} {}: {e}",
                    self.h1.header().name, rep.source, entry.group, entry.tag)),
            };
            match result {
                Ok(id) => shaders.push(id),
                Err(reason) => {
                    self.tree.model_shader_error.get_or_insert_with(|| reason.clone());
                    return Err(Error::Invalid(reason));
                }
            }
        }
        Ok(shaders)
    }

    fn model_shader(
        &mut self,
        tag: &Tag,
        entry: &h1_assets::models::h1::ShaderRef,
        rounds: Option<i16>,
    ) -> Result<TagId> {
        if !h1convert::shad::H1_GROUPS.contains(&tag.group.to_string().as_str()) {
            return Err(Error::Invalid("unsupported model shader group".into()));
        }
        let base = self.base;
        let template = &base.model_shader;
        if let Some(id) = self.counter_shader(&template.library, tag, entry.permutation, rounds)? {
            return Ok(id);
        }
        if let Some(&id) = self.done.get(&tag.id.0) {
            return Ok(id);
        }
        if let Some(id) = self.permuted_shader(tag, entry.permutation, &template.library)? {
            return Ok(id);
        }
        if let Some(source) = ModelDefaults::read(tag.group, self.h1.tag_data(tag)?)? {
            return self.model_default_shader(&template.library, tag, source);
        }
        if let Some(layout) = h1convert::shad::textureless_layout(tag.group, self.h1.tag_data(tag)?)
        {
            use h1convert::shad::program::glass;
            let source = self.h1;
            let map = &source.header().name;
            let fail = |why| h1convert::Error::no_rebuild(map, &tag.name, why);
            let root = if layout.stem == glass::CLEAR.stem {
                let vertex = base
                    .find(b"vrtx", glass::GLASS_VERTEX)
                    .ok_or_else(|| fail(glass::GLASS_VERTEX))?;
                let pass = self.program_tag(glass::CLEAR.stem, b"spas", glass::clear_pass(vertex)?);
                let stem =
                    self.program_tag(glass::CLEAR.stem, b"stem", glass::clear_template(pass)?);
                glass::clear_instance(stem)?
            } else {
                if !template.library.has(layout) {
                    return Err(fail(layout.stem).into());
                }
                template.library.build(layout, &HashMap::new())?
            };
            return self.cached(
                tag,
                Converted {
                    group: FourCC(*b"shad"),
                    root,
                    raw: vec![],
                },
            );
        }
        if let Some(id) = self.model_effect_shader(&template.library, tag)? {
            return Ok(id);
        }
        if let Some(id) = self.transparent_model_shader(&template.library, tag)? {
            return Ok(id);
        }
        if !matches!(&tag.group.0.to_be_bytes(), b"soso" | b"senv") {
            return Err(Error::Invalid("no supported model shader rebuild".into()));
        }
        let maps = h1convert::textures::h1_maps_of(self.h1, tag)
            .ok_or_else(|| Error::Invalid("cannot read shader maps".into()))?;
        let mut bitmap = |id| {
            if h1convert::shad::h1_texture_kind(self.h1, id) != Some(h1map::bitmap::ImageKind::D2) {
                return Err(Error::Invalid(format!("unreadable or non-2D opaque bitmap {id}")));
            }
            self.bitmap(id).ok_or_else(|| Error::Invalid(format!("unreadable bitmap {id}")))
        };
        let base = bitmap(maps.base.ok_or_else(|| Error::Invalid("missing base map".into()))?)?;
        let detail = maps.detail.map(bitmap).transpose()?;
        let scale = h1convert::textures::h1_scales_of(self.h1, tag)?.detail;
        let root = template.build_opaque(base, detail, scale)?;
        self.cached(tag, Converted { group: FourCC(*b"shad"), root, raw: Vec::new() })
    }

    /// A Halo 1 model shader that isn't `senv` or `soso` (glass, chicago,
    /// generic, meter, plasma, water) rebuilt as the structure's are
    /// ([`h1convert::shad::read_h1`]): its own stem (alpha blended,
    /// additive or self-illuminated) and maps, converted once. An ordinary
    /// opaque model material is built by the caller.
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
    ) -> Result<Option<TagId>> {
        let group = tag.group.to_string();
        if group == "senv" {
            return Ok(None);
        }
        let Some(s) = h1convert::shad::read_h1(self.h1, tag) else { return Ok(None) };
        let mut chosen = s.choose(library)
            .ok_or_else(|| Error::NoTemplate(s.layout.stem.into()))?.clone();
        let change_colour = h1convert::shad::soso_change_colour(self.h1, tag);
        // A `soso` is rebuilt for its reflection (converter version 76)
        // or its cut-out base (84, `shad::soso_alpha_tested`): otherwise
        // the caller builds the ordinary opaque shader.
        let env = [
            h1convert::shad::TEX_BUMP_ENV.stem,
            h1convert::shad::TEX_BUMP_ENV_FOUR_CHANGE_COLOR.stem,
        ];
        if group == "soso"
            && !env.contains(&chosen.layout.stem)
            && chosen.layout.stem != h1convert::shad::TEX_BUMP_ALPHA_TEST.stem
        {
            return Ok(None);
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
                    self.masked_bitmap(m.bitmap, mask)
                }
                // A plasma's picture with its brightness as alpha
                // (converter version 90, `shad::plasma_shader`).
                (h1convert::shad::Role::Mask, _, Some(gain)) => self.brightest_bitmap(m.bitmap, gain),
                // The multipurpose map's blue in the slot's channel; no
                // change-colour map is synthesized without its source.
                (h1convert::shad::Role::ChangeColour, _, _) => match change_colour {
                    Some((slot, _)) => self.change_colour_bitmap(m.bitmap, slot),
                    None => None,
                },
                _ => self.bitmap(m.bitmap),
            };
            let id = id.ok_or_else(|| Error::Invalid(format!("cannot convert {role:?} bitmap {}", m.bitmap)))?;
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
            return Ok(None);
        }
        // The cut-out stem's depth and shadow passes kill by its bump
        // map's alpha, not the base's (`h1_assets::bitmaps::mask::cutout_bump`):
        // a `soso` has no bump map, so it takes a flat one cut out where
        // its base is (converter version 92; the stem's default bump is
        // opaque, and the leaves drew as whole dark cards).
        if chosen.layout.stem == h1convert::shad::TEX_BUMP_ALPHA_TEST.stem
            && !maps.contains_key(&h1convert::shad::Role::Bump)
        {
            let (_, base) = chosen.maps.iter().find(|(role, _)| *role == h1convert::shad::Role::Base)
                .ok_or_else(|| Error::Invalid("cutout shader has no base map".into()))?;
            let id = self.cutout_bump(base.bitmap)
                .ok_or_else(|| Error::Invalid(format!("cannot convert cutout bitmap {}", base.bitmap)))?;
            maps.insert(
                h1convert::shad::Role::Bump,
                h1convert::shad::Bound {
                    id,
                    scale: base.scale,
                    lod_offset: h1convert::shad::h1_lod_offset(self.h1, base.bitmap),
                },
            );
        }
        chosen.extras.cinematic_reflection = group == "soso" && env.contains(&chosen.layout.stem);
        let tree = library.build_with(chosen.layout, &maps, &chosen.extras)?;
        self.cached(
            tag,
            Converted {
                group: FourCC(*b"shad"),
                root: tree,
                raw: Vec::new(),
            },
        )
        .map(Some)
    }

    /// Rebuild a numeric `schi` as one animated shader per digit using
    /// the model's shader permutation. The digits come from either the
    /// weapon's ammunition function or the converted campaign countdown.
    fn counter_shader(
        &mut self,
        library: &h1convert::shad::Library,
        tag: &Tag,
        permutation: i16,
        loaded: Option<i16>,
    ) -> Result<Option<TagId>> {
        if tag.group != h1map::Group::new(b"schi") {
            return Ok(None);
        }
        let key = (tag.id.0, permutation, loaded);
        if let Some(id) = self.counter_shaders.get(&key) {
            return Ok(Some(*id));
        }
        let flags = self.h1.tag_data(tag)?.get(h1convert::shad::CHICAGO_FLAGS)
            .ok_or(Error::Truncated("Chicago shader flags"))?;
        if flags & h1convert::shad::CHICAGO_NUMERIC == 0 {
            return Ok(None);
        }
        let n = h1convert::shad::chicago_numeric(self.h1, tag)
            .ok_or_else(|| Error::Invalid("unreadable numeric Chicago stage".into()))?;
        let Some(input) = n.countdown_digit(permutation)
            .or_else(|| n.ammunition_digit(permutation, loaded?)) else {
            return Ok(None);
        };
        let s = h1convert::shad::counter_shader(&tag.name, &n, input);
        let digits = self.bitmap(n.bitmap)
            .ok_or_else(|| Error::Invalid(format!("unreadable counter bitmap {}", n.bitmap)))?;
        let maps = HashMap::from([(
            h1convert::shad::Role::Base,
            h1convert::shad::Bound {
                id: digits,
                scale: n.scale,
                lod_offset: h1convert::shad::h1_lod_offset(self.h1, n.bitmap),
            },
        )]);
        let root = library.build_with(s.layout, &maps, &s.extras)?;
        let place = input.rsplit('_').next().unwrap_or(input);
        let id = self.tree.push(
            &format!("{}_{place}", tag.name),
            Converted { group: FourCC(*b"shad"), root, raw: Vec::new() },
        );
        self.counter_shaders.insert(key, id);
        Ok(Some(id))
    }

    /// The Halo 1 bitmap `id`, converted once.
    fn source_bitmap(&mut self, id: Option<h1map::TagId>) -> Result<Option<TagId>> {
        let Some(id) = id else { return Ok(None) };
        let t = self.h1_tag(id)?;
        self.bitmap(id).map(Some).ok_or_else(|| {
            h1convert::Error::no_rebuild(&self.h1.header().name, &t.name, "bitmap conversion")
                .into()
        })
    }

    fn bitmap(&mut self, id: h1map::TagId) -> Option<TagId> {
        let tag = self.h1.tag(id).ok()?;
        if tag.group != h1map::Group::new(b"bitm") {
            return None;
        }
        if let Some(t) = self.done.get(&tag.id.0) {
            return Some(*t);
        }
        let (_, conv, _) = h1_assets::bitmaps::convert_bitmap(self.h1, tag).ok()?;
        self.cached(tag, conv).ok()
    }

    /// The Halo 1 base map `base` with the red channel of `mask` (a
    /// `soso`'s multipurpose map) as its alpha, DXT5, on every image
    /// (`h1_assets::bitmaps::mask::convert_reflection_mask`), `<base
    /// name>_reflection_<mask leaf>`, built once per pair. `None` when
    /// either does not read or decode, or their images do not pair.
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
    /// alpha, DXT5 (`h1_assets::bitmaps::mask::convert_brightest`),
    /// `<name>_brightness_<gain × 1000>`, built once per pair. `None` when
    /// it does not read or decode.
    fn brightest_bitmap(&mut self, id: h1map::TagId, gain: f32) -> Option<TagId> {
        let key = (id.0, gain.to_bits());
        if let Some(t) = self.brightest_bitmaps.get(&key) {
            return *t;
        }
        let tag = self.h1.tag(id).ok()?.clone();
        let out = h1_assets::bitmaps::mask::convert_brightest(self.h1, &tag, gain)
            .ok()
            .map(|conv| {
                let name = h1convert::shad::brightness_mask_name(&tag.name, gain);
                self.tree.push(&name, conv)
            });
        self.brightest_bitmaps.insert(key, out);
        out
    }

    /// The flat bump map cut out by the Halo 1 base map `base`'s alpha
    /// (`h1_assets::bitmaps::mask::convert_cutout_bump`), `<base
    /// name>_alpha_test_bump`, built once per base. `None` when the base
    /// does not read or decode.
    fn cutout_bump(&mut self, base: h1map::TagId) -> Option<TagId> {
        if let Some(t) = self.cutout_bumps.get(&base.0) {
            return *t;
        }
        let tag = self.h1.tag(base).ok()?.clone();
        let out = h1_assets::bitmaps::mask::convert_cutout_bump(self.h1, &tag)
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
        let conv = h1_assets::bitmaps::mask::convert_reflection_mask(self.h1, &tag, &mtag).ok()?;
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
    /// on every image (`h1_assets::bitmaps::mask::convert_change_colour`), `<mask
    /// name>_change_color_<slot>`, built once per pair. `None` when the
    /// mask does not read or decode.
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
        let conv = h1_assets::bitmaps::mask::convert_change_colour(
            self.h1,
            &mtag,
            h1convert::shad::SOSO_CHANGE_COLOUR_CHANNEL,
            slot,
        )
        .ok()?;
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
            h1_assets::bitmaps::convert::convert_with(&scaled, |i| Ok(std::mem::take(&mut pixels[i]))).ok()?;
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
            h1_assets::bitmaps::convert::convert_with(&cleared, |i| Ok(std::mem::take(&mut pixels[i]))).ok()?;
        let out = self.tree.push(&format!("{}_meter", tag.name), conv);
        self.meter_bitmaps.insert(id.0, out);
        Some(out)
    }

    /// A copy of bitmap `id` whose sequence `seq` starts at its sprite
    /// `first`, for a Halo 2 widget, which draws its sequence's first
    /// sprite. Isolate even sprite 0: the native target-colour branch can
    /// otherwise substitute sprite 1. Cache by source, sequence and frame.
    /// `Ok(None)` only past the sequence's sprites (the weapon has no such
    /// zoom level); a bitmap that does not read, decode or convert is an
    /// error naming it.
    fn sprite_bitmap(&mut self, id: h1map::TagId, seq: i16, first: usize) -> Result<Option<TagId>> {
        let tag = self.h1.tag(id)?.clone();
        let source = self.h1;
        let fail = |why: String| -> Error {
            h1convert::Error::no_rebuild(&source.header().name, &tag.name, format!("zoom sprite {first}: {why}")).into()
        };
        let key = (id.0, seq, first);
        if let Some(t) = self.sprite_bitmaps.get(&key) {
            return Ok(Some(*t));
        }
        let bm = h1map::bitmap::Bitmap::read(source, &tag).map_err(|e| fail(e.to_string()))?;
        let mut shifted = bm.clone();
        let sprites = &mut usize::try_from(seq)
            .ok()
            .and_then(|s| shifted.sequences.get_mut(s))
            .ok_or_else(|| fail(format!("no sequence {seq}")))?
            .sprites;
        if first >= sprites.len() {
            return Ok(None);
        }
        let selected = sprites[first];
        *sprites = vec![selected];
        let mut pixels = (0..bm.images.len())
            .map(|i| bm.pixels(source, i).map(|p| p.to_vec()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| fail(e.to_string()))?;
        let (conv, _) =
            h1_assets::bitmaps::convert::convert_with(&shifted, |i| Ok(std::mem::take(&mut pixels[i])))
                .map_err(|e| fail(e.to_string()))?;
        let out = self.tree.push(&format!("{}_seq{seq}_sprite{first}", tag.name), conv);
        self.sprite_bitmaps.insert(key, out);
        Ok(Some(out))
    }

    fn program_tag(&mut self, name: &str, group: &[u8; 4], root: Block) -> TagId {
        self.tree.push(
            name,
            Converted {
                group: FourCC(*group),
                root,
                raw: Vec::new(),
            },
        )
    }

    fn cached(&mut self, t: &Tag, conv: Converted) -> Result<TagId> {
        if let Some(id) = self.done.get(&t.id.0) {
            return Ok(*id);
        }
        let id = self.tree.push(&t.name, conv);
        self.done.insert(t.id.0, id);
        Ok(id)
    }

    fn report(tag: &Tag, template: &str) -> ObjectReport {
        ObjectReport {
            group: tag.group.to_string(),
            source: tag.name.clone(),
            template: template.to_owned(),
            ..Default::default()
        }
    }

    /// Rebuild the placement-specific light on the fixture's original
    /// markers. Identical overrides share a machine and its light tags.
    pub fn fixture(
        &mut self,
        tag: &Tag,
        lighting: Option<h1convert::fixture::Lighting>,
    ) -> Result<TagId> {
        use tags::defs::{self, Defs, Target, Val};

        if tag.group != h1map::Group::new(b"lifi") {
            return Err(Error::WrongGroup(tag.group.to_string(), *b"lifi"));
        }
        let machine = self.object(tag)?;
        let Some(lighting) = lighting else {
            return Ok(machine);
        };
        let key = (machine, lighting.key());
        if let Some(&id) = self.fixture_variants.get(&key) {
            return Ok(id);
        }
        let machine_group = FourCC(*b"mach");
        let light_group = FourCC(*b"ligh");
        let machine_def = Defs::builtin()
            .group(machine_group)
            .expect("machine definition");
        let owner = local_index(machine).expect("rebuilt fixture is local");
        let mut record = defs::from_built(machine_def, &self.tree.tags[owner].1.root)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        let Some(Val::Block(attachments)) = record.at_mut("devi.obje.attachments") else {
            return Err(Error::Invalid(format!(
                "{}: fixture attachments missing",
                tag.name
            )));
        };
        let mut light_count = 0;
        for attachment in attachments {
            let Some(Val::Ref { group, target }) = attachment.at_mut("type") else {
                return Err(Error::Invalid(format!(
                    "{}: invalid fixture attachment",
                    tag.name
                )));
            };
            if *group != light_group.as_u32() {
                continue;
            }
            let Target::Id(light) = target else {
                return Err(Error::Invalid(format!(
                    "{}: fixture light is unresolved",
                    tag.name
                )));
            };
            let light_key = (*light, lighting.key());
            let variant = if let Some(&id) = self.fixture_variants.get(&light_key) {
                id
            } else {
                let index = local_index(*light).ok_or_else(|| {
                    Error::Invalid(format!("{}: fixture light is not rebuilt", tag.name))
                })?;
                let (name, existing) = &self.tree.tags[index];
                let name = format!("{name}_fixture");
                let root = crate::rebuild::fixture_light(&existing.root, lighting)?;
                let id = self.tree.push(
                    &name,
                    Converted {
                        group: light_group,
                        root,
                        raw: Vec::new(),
                    },
                );
                self.fixture_variants.insert(light_key, id);
                id
            };
            *light = variant;
            light_count += 1;
        }
        if light_count == 0 {
            return Err(Error::Invalid(format!(
                "{}: authored fixture lighting has no attached light",
                tag.name
            )));
        }
        let root = defs::to_built(machine_def, &record, &mut |_| None)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        let id = self.tree.push(
            &format!("{}_placed", tag.name),
            Converted {
                group: machine_group,
                root,
                raw: Vec::new(),
            },
        );
        self.fixture_variants.insert(key, id);
        Ok(id)
    }

    /// A Halo 1 machine, control or light fixture, from the `mach` or
    /// `ctrl` definition ([`crate::rebuild::device`]); a light fixture is
    /// a machine (Halo 2 has no `lifi` tags). There is no template path.
    fn device(&mut self, tag: &Tag) -> Result<TagId> {
        let mut dev = h1::Device::read(self.h1, tag)?;
        self.halo2_change_colours(&mut dev.object);
        let group = if dev.control.is_some() {
            b"ctrl"
        } else {
            b"mach"
        };
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        let hlmt = self.model(&mut rep, &dev.object, &tag.name, None)?;
        rep.physics = Some("none (no Halo 1 collision to build from)".into());
        self.object_physics(&mut rep, hlmt, &dev.object, &tag.name, PhysicsKind::Device)?;
        let predicted = self.predicted(hlmt);
        // The shader may consume a device function even when it has no attachments.
        let (attachments, creation, functions) =
            self.object_attachments(self.h1.tag_data(tag)?, &dev.object, &mut rep)?;
        // A colour bound to an object output whose Halo 1 function exists
        // but did not convert is an error. One bound to an output the
        // device has no function for reads zero in Halo 1: the object is
        // allocated zeroed (0x8b8d8..0x8b8e6), and only the function loop
        // writes outputs (0x8c092), for its functions alone (count at def
        // +0x158, 0x8c0d5). The colour update applies that zero (0x8c162
        // scale-by, 0x8c1be darken-by). Halo 2 reads an unresolved name as
        // zero (0xbe9bc; 0xbab40 clears the value first), so it binds
        // `zero`, which no object exports.
        let bytes = self.h1.tag_data(tag)?;
        let source = h1::functions(self.h1, bytes)?;
        let source_functions = source.len();
        // Each output's reason, for the error below.
        let converted = crate::functions::convert(
            &source,
            &h1::FunctionInputs::read(bytes, dev.object.object_type),
            &mut Vec::new(),
        );
        for (slot, colour) in dev.object.change_colours.iter_mut().enumerate() {
            let Some(binding) = colour.function.as_mut() else { continue };
            for name in [&mut binding.darken, &mut binding.scale].into_iter().flatten() {
                let Some(k) = crate::functions::OUTPUT_NAMES.iter().position(|o| o == name) else {
                    continue;
                };
                if functions.iter().any(|f| f.export == *name) {
                    continue;
                }
                if k >= source_functions {
                    *name = "zero";
                    continue;
                }
                return Err(h1convert::Error::no_rebuild(
                    &self.h1.header().name,
                    &tag.name,
                    format!(
                        "change colour {slot}: output {name}'s function did not convert: {}",
                        converted.get(k).and_then(|f| f.as_ref().err()).map_or("no reason", String::as_str)
                    ),
                )
                .into());
            }
        }
        let root = crate::rebuild::device(&dev, hlmt, &predicted)?;
        let root = attachments_conv::with_attachments(
            *group, "devi.obje", root, &attachments, creation, &functions, &dev.object.change_colours,
        )?;
        let bytes = self.h1.tag_data(tag)?;
        let d = tags::defs::Defs::builtin()
            .group(FourCC(*group))
            .expect("device definition");
        let mut record =
            tags::defs::from_built(d, &root).map_err(|e| Error::Invalid(e.to_string()))?;
        for (at, field) in [
            (0x1a0, "open"),
            (0x1b0, "close"),
            (0x1c0, "opened"),
            (0x1d0, "closed"),
            (0x1e0, "depowered"),
            (0x1f0, "repowered"),
            (0x20c, "delay_effect"),
        ] {
            let Some(id) = h1::reference(self.h1, bytes, at) else {
                continue;
            };
            let t = self.h1_tag(id)?;
            let g = FourCC(t.group.0.to_be_bytes());
            let id = match &g.0 {
                b"effe" => self.effect(t)?,
                b"snd!" => self.sound(t, &mut rep)?,
                _ => {
                    return Err(h1convert::Error::no_rebuild(
                        &self.h1.header().name,
                        &t.name,
                        "device transition group",
                    )
                    .into());
                }
            };
            record
                .set(&format!("devi.{field}"), tags::defs::Val::tag(g, id))
                .map_err(|e| Error::Invalid(e.to_string()))?;
        }
        let root = tags::defs::to_built(d, &record, &mut |_| None)
            .map_err(|e| Error::Invalid(e.to_string()))?;
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
                    h1_assets::models::model_from_map(self.h1, self.h1_tag(m)?, &ModelOptions::default())?;
                self.powered_points(&u.object, &model)?
            }
            None => Vec::new(),
        };
        let scripted = scripted_props::ScriptedVehicle::of(&tag.name);
        if !matches!(vehicle_type, 0 | 1 | 3 | 4 | 5 | 6) {
            return Err(h1convert::Error::no_rebuild(
                &self.h1.header().name,
                &tag.name,
                format!("vehicle controller for type {vehicle_type}"),
            )
            .into());
        }
        let hover = powered
            .iter()
            .filter(|p| p.point.hover)
            .count();
        if hover != 0 && hover != powered.len() {
            return Err(h1convert::Error::no_rebuild(
                &self.h1.header().name,
                &tag.name,
                "mixed antigravity and friction powered points",
            )
            .into());
        }
        let wheels = vehicle_type == 0;
        let mut rep = Self::report(
            tag,
            "none: rebuilt from source vehicle type and powered mass points",
        );
        let hlmt = self.model(&mut rep, &u.object, &tag.name, None)?;
        let airborne_flags = *b.get(0x17c).ok_or_else(|| {
            h1convert::Error::no_rebuild(
                &self.h1.header().name,
                &tag.name,
                "vehicle tag too short for its dialogue flags (+0x17c)",
            )
        })?;
        if airborne_flags & 0x40 != 0 && let Some(h) = hlmt {
            let bits = map::H1_PHYSICAL_DIALOGUE | map::H1_DIALOGUE_AIRBORNE_PASSENGERS;
            let root = &mut self.tree.tags[h.0 as usize & 0xffff].1.root;
            root.bytes[0x80..0x84].copy_from_slice(&bits.to_le_bytes());
        }
        let kind = match scripted {
            Some(scripted_props::ScriptedVehicle::Seat) => PhysicsKind::Device,
            Some(scripted_props::ScriptedVehicle::Flyer) => PhysicsKind::ScriptedFlyer,
            None => PhysicsKind::Vehicle { wheels },
        };
        let has_physics = self.object_physics(&mut rep, hlmt, &u.object, &tag.name, kind)?;
        if !has_physics {
            return Err(h1convert::Error::no_rebuild(
                &self.h1.header().name,
                &tag.name,
                "vehicle has no source physics to build",
            )
            .into());
        }
        let predicted = self.predicted(hlmt);
        let (markers, bounds) = match u.object.model {
            Some(m) => {
                let model =
                    h1_assets::models::model_from_map(self.h1, self.h1_tag(m)?, &ModelOptions::default())?.0;
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
            names.camera_track = Some(self.camera_track(self.h1_tag(t)?)?);
            tracks += 1;
        }
        let points: Vec<_> = powered.into_iter().map(|p| p.point).collect();
        let weapons = self.unit_weapons(b, &mut rep)?;
        let mut references = Vec::new();
        for (path, at) in [("suspension_sound", 0x3b0), ("crash_sound", 0x3c0),
                           ("foot_ref_294", 0x3d0), ("special_effect", 0x3e0),
                           ("unit.integrated_light_toggle", 0x188), ("unit.melee_damage", 0x288)] {
            let Some(id) = h1::reference(self.h1, b, at) else { continue };
            let t = self.h1_tag(id)?;
            let group = t.group.0.to_be_bytes();
            let out = match &group {
                b"snd!" => Some(self.sound(t, &mut rep)?),
                b"foot" => Some(self.foot(t, &mut rep)?),
                b"effe" => Some(self.effect(t)?),
                b"jpt!" => Some(self.damage(id)?),
                _ => return Err(Error::Invalid(format!("{} at vehicle +{at:#x}", t.group))),
            };
            references.push(crate::arsenal::halo1_ref(path, &group, out));
        }
        let (attachments, creation_effect, functions) =
            self.object_attachments(b, &u.object, &mut rep)?;
        let fail = |why: String| {
            Error::from(h1convert::Error::no_rebuild(
                &self.h1.header().name,
                &tag.name,
                why,
            ))
        };
        // The Halo 2 vehicle whose drive tuning this one takes.
        let drive = match crate::rebuild::drive_counterpart(&tag.name) {
            Some(names) => {
                let id = names
                    .iter()
                    .find_map(|n| self.base.find(b"vehi", n))
                    .ok_or_else(|| {
                        fail(format!("the base holds none of its drive counterparts {names:?}"))
                    })?;
                let d = tags::defs::Defs::builtin()
                    .group(FourCC(*b"vehi"))
                    .expect("vehi definition");
                Some(self.base.record(d, id)?)
            }
            // A prop an animation moves, a scripted set piece, a dropship
            // or a turret has no drive to take.
            None if scripted.is_some()
                || crate::rebuild::drive_exempt(&tag.name)
                || matches!(vehicle_type, 3 | 6) =>
            {
                None
            }
            None => {
                return Err(fail(
                    "no Halo 2 vehicle is its drive counterpart (rebuild::DRIVE_COUNTERPARTS)"
                        .into(),
                ));
            }
        };
        let (root, roles) = crate::provenance::capture(|| {
            crate::rebuild::vehicle_with_tuning(&crate::rebuild::VehicleSpec {
                source: b,
                references: &references,
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
                points: &points,
            }, scripted.and_then(|kind| kind.flight_tuning(b)), drive.as_ref())
        });
        let root = root?;
        if drive.is_some() {
            rep.no_home.push(
                "vehi: drive tuning (gears, engine, steering, ground and hover laws, friction and hover points' tuning) from the matching Halo 2 vehicle"
                    .into(),
            );
        }
        rep.no_home.push(format!(
            "vehi: Halo 1's type, flags (low half), camera, {} seats (label as the rider's mode, markers), {} weapons and {} powered mass points as friction or hover points; the counterpart's drive tuning when it has one; {} attachments (looping sounds and effects) and the creation effect from Halo 1; {} seat camera tracks Halo 1's; HUD and damage null; {} Halo 1 references",
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
        model: &h1_assets::models::h1::Model,
    ) -> Result<Vec<PoweredPoint>> {
        let Some(p) = obj.physics else {
            return Ok(Vec::new());
        };
        let phys = h1::Physics::read(self.h1, self.h1_tag(p)?)?;
        let coll = match obj.collision {
            Some(c) => Some(h1::Collision::read(self.h1, self.h1_tag(c)?)?),
            None => None,
        };
        let world = h1_assets::models::mode::world_transforms(
            &model
                .nodes
                .iter()
                .map(|n| (n.parent, n.rotation, n.translation))
                .collect::<Vec<_>>(),
        );
        let mut out: Vec<PoweredPoint> = Vec::new();
        for m in phys.mass_points.iter().filter(|m| m.powered_mass_point >= 0) {
            let flags = phys
                .powered_flags
                .get(m.powered_mass_point as usize)
                .copied();
            if !matches!(flags, Some(1 | 0x40)) {
                return Err(h1convert::Error::no_rebuild(
                    &self.h1.header().name,
                    &self.h1_tag(p)?.name,
                    format!(
                        "mass point {} has unsupported powered group flags {flags:?}",
                        m.name
                    ),
                )
                .into());
            }
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
            let grips = flags == Some(h1::POWERED_GROUND_FRICTION);
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
                return Err(h1convert::Error::no_rebuild(
                    &self.h1.header().name,
                    &self.h1_tag(p)?.name,
                    format!("duplicate powered mass point marker {marker}"),
                )
                .into());
            }
            let region_index = model
                .regions
                .iter()
                .position(|r| !region.is_empty() && crate::coll::sid_name(&r.name) == region)
                .map_or(-1, |i| i as i16);
            out.push(PoweredPoint {
                point: crate::rebuild::Point {
                    sphere: [m.position[0], m.position[1], m.position[2], m.radius],
                    hover: phys
                        .powered_flags
                        .get(m.powered_mass_point as usize)
                        .is_some_and(|flags| *flags & (1 << 6) != 0),
                    marker,
                    region,
                    region_index,
                },
                node,
                position: local,
            });
        }
        Ok(out)
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

    fn unit_weapons(&mut self, bytes: &[u8], _rep: &mut ObjectReport) -> Result<Vec<TagId>> {
        let mut weapons = Vec::new();
        for entry in h1::elements(self.h1, bytes, 0x2d8, 0x24)? {
            if let Some(id) = h1::reference(self.h1, entry, 0) {
                let t = self.h1_tag(id)?;
                weapons.push(self.object(t)?);
            }
        }
        Ok(weapons)
    }

    /// Rebuild a character's unit with its actor-variant colours while sharing
    /// the source unit's rebuilt model, collision, physics and animation tags.
    pub fn actor_biped(
        &mut self,
        tag: &Tag,
        name: &str,
        colours: &[h1::ChangeColour],
    ) -> Result<TagId> {
        let id = self.object(tag)?;
        if colours.is_empty() {
            return Ok(id);
        }
        let d = tags::defs::Defs::builtin()
            .group(FourCC(*b"bipd"))
            .expect("bipd definition");
        let mut r = tags::defs::from_built(
            d,
            &self.tree.tags[local_index(id).expect("rebuilt unit")]
                .1
                .root,
        )
        .map_err(|e| Error::Invalid(e.to_string()))?;
        let mut obj = h1::Object::read(self.h1, self.h1.tag_data(tag)?)?;
        obj.change_colours.resize(
            obj.change_colours.len().max(colours.len()),
            Default::default(),
        );
        obj.change_colours[..colours.len()].clone_from_slice(colours);
        self.halo2_change_colours(&mut obj);
        crate::rebuild::object_change_colors(b"bipd", &mut r, "unit.obje", &obj.change_colours)?;
        let root = tags::defs::to_built(d, &r, &mut |_| None)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(self.tree.push(
            name,
            Converted {
                group: FourCC(*b"bipd"),
                root,
                raw: Vec::new(),
            },
        ))
    }

    /// Rebuild the biped. The matching H2 model supplies shield shaders
    /// and damage responses for NPCs, and the switchable H2 HUD for the player.
    fn biped_rebuilt(&mut self, tag: &Tag, u: &Unit) -> Result<TagId> {
        use tags::defs::{self, Defs, Val};
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        let b = self.h1.tag_data(tag)?;
        let cid = self.biped_counterpart(&tag.name).map(|c| c.0);
        let counterpart = cid.map(|id| self.base.root(id)).transpose()?;
        let counterpart_hlmt = counterpart.as_deref().and_then(|r| self.base.reference(r, h2::MODEL));
        let hlmt = self.model(&mut rep, &u.object, &tag.name, None)?;
        self.object_physics(&mut rep, hlmt, &u.object, &tag.name, PhysicsKind::Biped)?;
        let (markers, nodes) = match u.object.model {
            Some(m) => {
                let model = h1_assets::models::model_from_map(self.h1, self.h1_tag(m)?, &ModelOptions::default())?.0;
                let markers: Vec<String> = model
                    .markers
                    .into_iter()
                    .filter(|m| !m.instances.is_empty())
                    .map(|m| m.name)
                    .collect();
                (markers, model.nodes.into_iter().map(|n| n.name).collect())
            }
            None => (Vec::new(), Vec::new()),
        };
        let player = health_hud::is_player_biped(&tag.name);
        let voices = self.biped_dialogue(tag, b, &mut rep)?;
        if !voices.is_empty() && hlmt.is_none() {
            return Err(h1convert::Error::no_rebuild(
                &self.h1.header().name,
                &tag.name,
                "unit voices require a model",
            )
            .into());
        }
        let source_damage = u.object.collision.map(|id| -> Result<_> {
            h1::tag_bytes(self.h1, self.h1_tag(id)?, b"coll", h1::COLL_SIZE)
        }).transpose()?;
        let shield = u.object.modifier_shader.map(|id| -> Result<TagId> {
            let shader = self.h1_tag(id)?;
            if hlmt.is_none() || source_damage.is_none() {
                return Err(h1convert::Error::no_rebuild(
                    &self.h1.header().name, &shader.name, "shield has no destination damage record",
                ).into());
            }
            let entry = h1_assets::models::h1::ShaderRef { group: shader.group, tag: shader.id, permutation: 0 };
            self.model_shader(shader, &entry, None)
        }).transpose()?;
        let mut blood = None;
        if let Some(h) = hlmt {
            let hd = Defs::builtin().group(FourCC(*b"hlmt")).expect("hlmt");
            let native = counterpart_hlmt.map(|id| self.base.record(hd, id)).transpose()?;
            let mut r = defs::from_built(hd, &self.tree.tags[h.0 as usize & 0xffff].1.root)
                .map_err(|e| Error::Invalid(e.to_string()))?;
            let mut copied = Vec::new();
            if player {
                crate::biped_hits::write_player_targets(hd, &mut r, &markers, &tag.name)?;
            }
            if !voices.is_empty() {
                let biped_flags = *b.get(0x2f4).ok_or_else(|| {
                    h1convert::Error::no_rebuild(
                        &self.h1.header().name,
                        &tag.name,
                        "biped tag too short for its dialogue flags (+0x2f4)",
                    )
                })?;
                let freefall = if biped_flags & 0x84 == 0 {
                    map::H1_DIALOGUE_CAN_SCREAM_IN_FREEFALL
                } else {
                    0
                };
                r.set("h1_dialogue_flags", Val::Int(i64::from(map::H1_PHYSICAL_DIALOGUE | freefall)))
                    .map_err(|e| Error::Invalid(e.to_string()))?;
            }
            if let Some(v) = native.as_ref().and_then(|c| c.at("shad_ref_94")) {
                r.set("shad_ref_94", v.clone()).map_err(|e| Error::Invalid(e.to_string()))?;
            }
            if let Some(src) = source_damage {
                if !voices.is_empty() {
                    let regions = h1::elements(self.h1, src, 0x240, 0x54)?;
                    let region_def = hd.child("h1_dialogue_regions").expect("physical dialogue");
                    let mut physical_dialogue = Vec::new();
                    let mut has_threshold = false;
                    for region in regions {
                        let threshold = f32::from_le_bytes(region[0x28..0x2c].try_into().unwrap());
                        let flags = u32::from_le_bytes(region[0x20..0x24].try_into().unwrap());
                        has_threshold |= threshold > 0.0;
                        let mut out = defs::default_record(region_def);
                        out.set("threshold", Val::Float(threshold))
                            .map_err(|e| Error::Invalid(e.to_string()))?;
                        out.set("flags", Val::Int(i64::from(flags)))
                            .map_err(|e| Error::Invalid(e.to_string()))?;
                        physical_dialogue.push(out);
                    }
                    if has_threshold {
                        r.set("h1_dialogue_regions", Val::Block(physical_dialogue))
                            .map_err(|e| Error::Invalid(e.to_string()))?;
                    }
                }
                let mut damage = defs::default_record(hd.child("block_60").expect("model damage"));
                // Native shield shaders and NPC damage responses are part of the
                // H2 damage system (object_new indexes +0xbc). H1 has no overshield shader slot.
                if let Some(id) = shield {
                    for path in ["tagref_7c", "tagref_84"] {
                        damage
                            .set(path, Val::tag(FourCC(*b"shad"), id))
                            .map_err(|e| Error::Invalid(e.to_string()))?;
                    }
                }
                for path in ["tagref_e8", "tagref_f0", "block_bc"] {
                    if player && path == "block_bc" {
                        continue;
                    }
                    let field = format!("block_60[0].{path}");
                    if let Some(v) = native.as_ref().and_then(|c| c.at(&field)) {
                        damage.set(path, v.clone()).map_err(|e| Error::Invalid(e.to_string()))?;
                        copied.push(crate::provenance::RoleField { path: field, value: v.clone() });
                    }
                }
                for (path, at) in [("unknown_28", 0x08), ("unknown_8c", 0xcc),
                                   ("unknown_98", 0x10c), ("unknown_9c", 0x110)] {
                    damage.set(path, Val::Bytes(src[at..at+4].to_vec()))
                        .map_err(|e| Error::Invalid(e.to_string()))?;
                }
                let half = |at: usize| u16::from_le_bytes([src[at], src[at + 1]]);
                let materials = h1::elements(self.h1, src, 0x234, 0x48)?;
                if player {
                    crate::biped_hits::write_player_damage_sections(
                        hd,
                        &mut r,
                        &mut damage,
                        &materials,
                    )?;
                }
                let body_type = materials.get(usize::from(half(4)))
                    .map(|m| u16::from_le_bytes([m[0x24], m[0x25]]));
                let shield_type = (f32::from_le_bytes(src[0xcc..0xd0].try_into().unwrap()) > 0.0)
                    .then(|| half(0xd2));
                let mut material_indices = [u16::MAX; 2];
                for (i, (path, kind)) in [("string_90", shield_type), ("string_4", body_type)].into_iter().enumerate() {
                    if let Some((_, global)) = kind.and_then(|k| crate::arsenal::MATERIALS.get(usize::from(k))) {
                        let m = crate::rebuild::model_material("", global, &self.base.materials)?;
                        damage.set(path, Val::text(&m.global)).map_err(|e| Error::Invalid(e.to_string()))?;
                        material_indices[i] = m.global_index as u16;
                    }
                }
                let indices = material_indices.into_iter().flat_map(u16::to_le_bytes).collect();
                damage.set("unknown_cc", Val::Bytes(indices)).map_err(|e| Error::Invalid(e.to_string()))?;
                let time = f32::from_le_bytes(src[0x110..0x114].try_into().expect("recharge time"));
                let rate = if time > 0.0 { time.recip() } else { 1000.0 };
                damage.set("unknown_d0", Val::Bytes(rate.to_le_bytes().to_vec()))
                    .map_err(|e| Error::Invalid(e.to_string()))?;
                for (path, at) in [("tagref_a4", 0x188), ("tagref_ac", 0x198), ("tagref_b4", 0x1a8)] {
                    if let Some(id) = h1::reference(self.h1, src, at) {
                        let out = self.effect(self.h1_tag(id)?)?;
                        damage.set(path, Val::tag(FourCC(*b"effe"), out)).map_err(|e| Error::Invalid(e.to_string()))?;
                    }
                }
                if let Some(id) = h1::reference(self.h1, src, 0x84) {
                    blood = Some(self.effect(self.h1_tag(id)?)?);
                }
                r.set("block_60", Val::Block(vec![damage])).map_err(|e| Error::Invalid(e.to_string()))?;
            }
            crate::rebuild::model_dialogue(&mut r, &voices)?;
            self.tree.tags[h.0 as usize & 0xffff].1.root =
                defs::to_built(hd, &r, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))?;
            self.tree.note_roles(h, copied);
        }
        let predicted = self.predicted(hlmt);
        let mut swap = None;
        let mut hud = None;
        if health_hud::is_player_biped(&tag.name) {
            let source = self.h1;
            let named = |what: &str, e: Error| {
                h1convert::Error::no_rebuild(&source.header().name, &tag.name, format!("{what}: {e}"))
            };
            // The switchable Halo 2 view needs every interface of the
            // counterpart; one that doesn't read is an error, not a HUD
            // without the view.
            let native = counterpart
                .as_deref()
                .map(|r| self.health_interfaces(r))
                .transpose()
                .map_err(|e| named("Halo 2 HUD view", e))?;
            let (id, _) = self.unit_hud(u).map_err(|e| named("unit HUD", e))?;
            // Halo 1's motion sensor centre (unhi +MOTION_SENSOR): the
            // converted HUD draws the sensor there, so a short record or a
            // scale that is not finite stops the conversion.
            let centre = self.sensor_centre(u).map_err(|e| named("motion sensor", e))?;
            self.tree.hud_sensor = Some(centre);
            if let Some(other) = native.as_ref().and_then(|ids| ids.first()) {
                swap = Some((id, *other));
            }
            hud = Some(vec![id; native.as_ref().map_or(1, |ids| ids.len().max(1))]);
        }
        let mut tracks = Vec::new();
        for id in &u.camera_tracks {
            tracks.push(self.camera_track(self.h1_tag(*id)?)?);
        }
        let weapons = self.unit_weapons(b, &mut rep)?;
        let integrated_light = h1::reference(self.h1, b, 0x188)
            .map(|id| self.effect(self.h1_tag(id)?)).transpose()?;
        let melee = h1::reference(self.h1, b, 0x288).map(|id| self.damage(id)).transpose()?;
        let (attachments, creation, functions) = self.object_attachments(b, &u.object, &mut rep)?;
        let mut physics = Vec::new();
        if let Some(h) = hlmt {
            self.locals_in(&self.tree.tags[h.0 as usize & 0xffff].1.root, FourCC(*b"phmo"), &mut physics);
        }
        let mass = physics.first().map(|p| {
            let bytes = &self.tree.tags[p.0 as usize & 0xffff].1.root.bytes;
            f32::from_le_bytes(bytes[4..8].try_into().expect("phmo mass"))
        });
        let parts = crate::rebuild::BipedParts {
            sentinel: tag.name.eq_ignore_ascii_case(r"characters\sentinel\sentinel"),
            hud, markers, nodes, tracks, weapons, melee, integrated_light, blood, attachments, functions, creation, mass,
        };
        let root = crate::rebuild::biped(u, hlmt, &predicted, &parts)?;
        let root = self.unit_foot(*b"bipd", root, u, &mut rep)?;
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
    /// `object_new` deletes it. Devices need one to block: Havok, which moves
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
        let physics = self.built_physics(obj, kind).map_err(|e| match kind {
            PhysicsKind::Vehicle { .. } | PhysicsKind::ScriptedFlyer => {
                h1convert::Error::no_rebuild(&self.h1.header().name, name, e).into()
            }
            _ => e,
        })?;
        let (phmo, how) = match physics {
            Some((block, how)) => (block, how),
            None if matches!(kind, PhysicsKind::Vehicle { .. }) => {
                return Err(h1convert::Error::no_rebuild(
                    &self.h1.header().name,
                    name,
                    "vehicle has no collision hull or source mass points",
                )
                .into());
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
        let source = if matches!(
            kind,
            PhysicsKind::Vehicle { .. } | PhysicsKind::ScriptedFlyer
        ) {
            obj.physics
                .map(|p| self.h1_tag(p))
                .transpose()?
                .map(|t| scripted_props::mass_properties(self.h1, t))
                .transpose()?
        } else {
            None
        };
        let (model, _) =
            h1_assets::models::model_from_map(self.h1, self.h1_tag(m)?, &ModelOptions::default())?;
        let from_points = || -> Result<Option<(Block, String)>> {
            if !matches!(
                kind,
                PhysicsKind::Vehicle { .. } | PhysicsKind::ScriptedFlyer
            ) {
                return Ok(None);
            }
            let Some((physics, mass)) = &source else {
                return Ok(None);
            };
            Ok(Some((
                scripted_props::mass_point_physics(&model, physics, mass)?,
                "built from Halo 1's phys: one body with source mass point spheres".into(),
            )))
        };
        let Some(c) = obj.collision else {
            return from_points();
        };
        let coll = h1::Collision::read(self.h1, self.h1_tag(c)?)?;
        let world = h1_assets::models::mode::world_transforms(
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
            return from_points();
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
                // point for the tracked chassis (the
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
                        " ({} wheel and tread nodes carried by suspension contacts)",
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
        self.biped_rebuilt(tag, &u)
    }
}

impl Converter<'_> {
    /// Halo 1's unit HUD of `u` (its `unhi`) as an `nhdt` written from
    /// scratch ([`crate::h1hud`]), built once per `unhi`. Returns the tag
    /// and its widget count.
    /// Where the unit's Halo 1 HUD (`unhi`) centres the motion sensor
    /// ([`crate::h1hud::sensor_centre`]).
    pub fn sensor_centre(&self, u: &crate::h1::Unit) -> Result<[i16; 2]> {
        let unhi = u.hud_interfaces.first().copied()
            .ok_or_else(|| Error::Invalid("no unhi".into()))?;
        let tag = self.h1.tag(unhi)?;
        let b = crate::h1::tag_bytes(self.h1, tag, b"unhi", crate::h1hud::unhi::SIZE)?;
        crate::h1hud::sensor_centre(b)
    }

    pub fn unit_hud(&mut self, u: &crate::h1::Unit) -> Result<(TagId, usize)> {
        let unhi = u.hud_interfaces.first().copied()
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
            crate::h1hud::check_scale(e.scale)
                .map_err(|why| Error::Invalid(format!("{}: {why}", e.name)))?;
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
                if !crate::h1hud::unscaled(crate::h1hud::number_scale(n.scale)) {
                    return Err(Error::Invalid(format!("{} count: scaled {:?}", gt.name, n.scale)));
                }
                let w = crate::h1hud::digit_width(self.h1)?;
                let h = crate::h1hud::digit_height(self.h1)?;
                texts.push(crate::h1hud::grenade_count(&n, k, w, h, text_shader)?);
            }
            for e in crate::h1hud::grenade_elements(self.h1, &gb)? {
                crate::h1hud::check_scale(e.scale)
                    .map_err(|why| Error::Invalid(format!("{}: {why}", gt.name)))?;
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

    /// Build all entries before publishing the interface, including those
    /// inherited from child HUDs. Empty retail chains produce an empty nhdt.
    pub fn weapon_hud(&mut self, wphi: h1map::TagId) -> Result<TagId> {
        let tag = self
            .h1
            .tag(wphi)
            .map_err(|err| {
                h1convert::Error::no_rebuild(
                    &self.h1.header().name,
                    format!("wphi {:#010x}", wphi.0),
                    err,
                )
            })?
            .clone();
        if tag.group != h1map::Group::new(b"wphi") {
            return Err(h1convert::Error::no_rebuild(
                &self.h1.header().name,
                &tag.name,
                format!("expected wphi, found {}", tag.group),
            )
            .into());
        }
        if let Some(&id) = self.done.get(&tag.id.0) {
            return Ok(id);
        }
        let result = self.rebuild_weapon_hud(&tag);
        result.map_err(|err| {
            h1convert::Error::no_rebuild(&self.h1.header().name, &tag.name, err).into()
        })
    }

    fn rebuild_weapon_hud(&mut self, tag: &h1map::Tag) -> Result<TagId> {
        use crate::h1hud::{self, WeaponEntryKind as Kind, weapon::Interface};
        let entries = h1hud::weapon_entries(self.h1, tag.id)?;
        let shader = |name: &str| {
            self.base
                .find(b"shad", name)
                .ok_or_else(|| Error::NoTemplate(name.into()))
        };
        let (meter, plain, text) = (
            shader(h1hud::METER_SHADER)?,
            shader(h1hud::STATIC_SHADER)?,
            shader(h1hud::TEXT_SHADER)?,
        );
        let mut hud = Interface::new();
        for (index, e) in entries.iter().enumerate() {
            let source = self.h1.tag(e.wphi)?.name.clone();
            let failure =
                |why: String| Error::Invalid(format!("{source} entry {index} {:?}: {why}", e.kind));
            if matches!(e.kind, Kind::Number(..)) {
                let widget = h1hud::weapon_number_with_spacing(
                    e,
                    h1hud::digit_width(self.h1)?,
                    h1hud::digit_height(self.h1)?,
                    h1hud::decimal_spacing(self.h1)?,
                    text,
                )
                .map_err(|err| failure(format!("{err:?}")))?;
                hud.text(e, widget)?;
                continue;
            }
            let id = e.bitmap.ok_or_else(|| failure("missing bitmap".into()))?;
            let bt = self.h1.tag(id)?.clone();
            let bm = h1map::bitmap::Bitmap::read(self.h1, &bt)?;
            if let Kind::Crosshair(h1hud::crosshair_type::ZOOM_LEVEL, flags) = e.kind {
                let seq = usize::try_from(e.sequence)
                    .ok()
                    .and_then(|seq| bm.sequences.get(seq))
                    .ok_or_else(|| {
                        failure(format!("missing sequence {} of {}", e.sequence, bt.name))
                    })?;
                let count = seq.sprites.len();
                let first_zoomed = flags & h1hud::OVERLAY_WHEN_ZOOMED != 0;
                let single_frame = flags & (h1hud::OVERLAY_WHEN_UNZOOMED | 0x20) != 0;
                let levels = if single_frame {
                    1
                } else {
                    h1hud::ZOOM_LEVELS + usize::from(!first_zoomed)
                };
                if count == 0 {
                    return Err(failure(format!(
                        "empty zoom sequence {} of {}",
                        e.sequence, bt.name
                    )));
                }
                for level in 0..count.min(levels) {
                    let image = self.sprite_bitmap(id, e.sequence, level)?;
                    let widget = h1hud::zoom_widget(e, level, image, plain)
                        .map_err(|err| failure(format!("{err:?}")))?;
                    hud.bitmap(e, widget)?;
                }
                continue;
            }
            let direct =
                matches!(e.kind, Kind::Crosshair(_, f) | Kind::Warning(_, f, _) if f & 2 != 0);
            let image = if direct {
                if usize::try_from(e.sequence)
                    .ok()
                    .and_then(|i| bm.images.get(i))
                    .is_none()
                {
                    return Err(failure(format!(
                        "missing image {} of {}",
                        e.sequence, bt.name
                    )));
                }
                Some(self.direct_hud_bitmap(&bt, bm)?)
            } else {
                if h1hud::sprite(&bm, e.sequence).is_none() {
                    return Err(failure(format!(
                        "missing sequence {} of {}",
                        e.sequence, bt.name
                    )));
                }
                match e.kind {
                    Kind::Meter(..) => self.meter_bitmap(id),
                    Kind::Static(_) | Kind::Overlay(..) => {
                        match h1hud::static_alpha(e.colour) {
                            Some(alpha) => self.hud_bitmap(id, alpha),
                            None => self.bitmap(id),
                        }
                    }
                    _ => self.bitmap(id),
                }
            };
            let image = image.ok_or_else(|| failure(format!("could not rebuild {}", bt.name)))?;
            let widget = h1hud::weapon_widget(e, Some(image), meter, plain)
                .map_err(|err| failure(format!("{err:?}")))?;
            hud.bitmap(e, widget)?;
        }
        let mut root = hud.build()?;
        self.weapon_scope(tag.id, &tag.name, &mut root)?;
        let id = self.tree.push(
            &format!("{}{}", tag.name, h1hud::WEAPON_NHDT_SUFFIX),
            Converted {
                group: FourCC(*b"nhdt"),
                root,
                raw: Vec::new(),
            },
        );
        self.done.insert(tag.id.0, id);
        Ok(id)
    }

    /// H1 0xdaeb2 and 0xdb12b index the image directly when bit 1 is set.
    fn direct_hud_bitmap(
        &mut self,
        tag: &h1map::Tag,
        mut bm: h1map::bitmap::Bitmap,
    ) -> Result<TagId> {
        if let Some(&id) = self.direct_hud_bitmaps.get(&tag.id.0) {
            return Ok(id);
        }
        let mut sequences = Vec::with_capacity(bm.images.len());
        for i in 0..bm.images.len() {
            sequences.push(h1map::bitmap::Sequence {
                name: format!("image_{i}"),
                first_image: i16::try_from(i)
                    .map_err(|_| Error::Invalid("HUD image index".into()))?,
                image_count: 1,
                sprites: Vec::new(),
            });
        }
        bm.sequences = sequences;
        let pixels = (0..bm.images.len())
            .map(|i| bm.pixels(self.h1, i))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let (converted, _) = h1_assets::bitmaps::convert::convert_with(&bm, |i| Ok(pixels[i].to_vec()))
            .map_err(|e| Error::Invalid(format!("{}: {e}", tag.name)))?;
        let id = self.tree.push(&format!("{}_direct", tag.name), converted);
        self.direct_hud_bitmaps.insert(tag.id.0, id);
        Ok(id)
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
pub fn wheel_nodes(phys: &h1::Physics, nodes: &[h1_assets::models::h1::Node]) -> Vec<i16> {
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

/// Solid pieces of the tracked chassis, cut at the hull floor. The input
/// holds one list of object-space convex pieces per wheel node. At `floor`,
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
