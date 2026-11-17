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
    (
        b"bipd",
        &[],
        "objects\\characters\\masterchief\\masterchief_mp",
    ),
    (b"vehi", &["warthog"], "objects\\vehicles\\warthog\\warthog"),
    (b"vehi", &["ghost"], "objects\\vehicles\\ghost\\ghost"),
    (
        b"vehi",
        &["scorpion"],
        "objects\\vehicles\\scorpion\\scorpion",
    ),
    (b"vehi", &["banshee"], "objects\\vehicles\\banshee\\banshee"),
];

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
    pub const HLMT_VARIANTS: usize = 0x50;
    pub const HLMT_REGIONS: usize = 0x70;
    pub const HLMT_NODES: usize = 0x78;
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
}

const LOCAL: u32 = 0xfffd_0000;

/// Stands for the `k`-th tag of a [`Tree`] until it is installed.
pub fn local(k: usize) -> TagId {
    TagId(LOCAL | k as u32)
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
    fn push(&mut self, name: &str, tag: Converted) -> TagId {
        self.tags.push((name.to_owned(), tag));
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

/// Builds a [`Tree`] from Halo 1 objects on one base map.
pub struct Converter<'a> {
    pub h1: &'a H1Map,
    pub base: &'a Base,
    pub tree: Tree,
    done: HashMap<u32, TagId>,
}

impl<'a> Converter<'a> {
    pub fn new(h1: &'a H1Map, base: &'a Base) -> Self {
        Self {
            h1,
            base,
            tree: Tree::default(),
            done: HashMap::new(),
        }
    }

    fn template(&self, group: &[u8; 4], path: &str, h2_group: &[u8; 4]) -> Result<(TagId, String)> {
        let name = template_for(group, path).ok_or_else(|| {
            Error::NoTemplate(format!("{} {path}", String::from_utf8_lossy(group)))
        })?;
        let id = self
            .base
            .find(h2_group, name)
            .ok_or_else(|| Error::NoTemplate(format!("{name} is not in the base map")))?;
        Ok((id, name.to_owned()))
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
        let hlmt = self.model(rep, obj, template_hlmt, name)?;
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
    /// object has no render model.
    fn model(
        &mut self,
        rep: &mut ObjectReport,
        obj: &h1::Object,
        template_hlmt: Option<TagId>,
        name: &str,
    ) -> Result<Option<TagId>> {
        let Some(mode_id) = obj.model else {
            return Ok(None);
        };
        let mode_tag = self.h1_tag(mode_id)?;
        let (h1_model, mode) =
            h1models::model_from_map(self.h1, mode_tag, &ModelOptions::default())?;
        let mode = self.cached(mode_tag, mode)?;
        let coll = match obj.collision {
            Some(c) => {
                let t = self.h1_tag(c)?;
                match self.done.get(&t.id.0) {
                    Some(id) => Some(*id),
                    None => {
                        let (conv, cr) = convert_collision(&h1::Collision::read(self.h1, t)?)?;
                        rep.no_home.push(format!(
                            "coll damage: body vitality {}, shield vitality {}, {} modifiers, {} damage effect references (Halo 2 keeps damage in the hlmt; not written)",
                            cr.body_vitality, cr.shield_vitality, cr.modifiers, cr.damage_references
                        ));
                        Some(self.cached(t, conv)?)
                    }
                }
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

        let mut hlmt = match template_hlmt {
            Some(t) => Block::new(self.base.root(t)?),
            None => Block::zeroed(
                tags::object::GroupDef::find(FourCC(*b"hlmt"))
                    .map_or(0xfc, |d| d.root_size as usize),
            ),
        };
        hlmt.tag_ref(h2::HLMT_RENDER as u32, mode);
        hlmt.tag_ref(h2::HLMT_COLLISION as u32, coll.unwrap_or(TagId::NULL));
        hlmt.tag_ref(h2::HLMT_ANIMATION as u32, jmad.unwrap_or(TagId::NULL));
        rep.physics = Some(
            match self.base.reference(&hlmt.bytes, h2::HLMT_PHYSICS_MODEL) {
                Some(p) => format!("borrowed: {}", self.base.name(p).map_or("?", |n| n.1)),
                None => "none (the template has no phmo)".into(),
            },
        );
        for at in [h2::HLMT_VARIANTS, h2::HLMT_NODES] {
            hlmt.bytes[at..at + 8].fill(0);
        }
        // Regions and their permutations, named as the converted model's.
        let mut regions = Block::zeroed(0x10 * h1_model.regions.len());
        for (i, r) in h1_model.regions.iter().enumerate() {
            put_sid(&mut regions, 0x10 * i, &r.name);
            let mut perms = Block::zeroed(8 * r.permutations.len());
            for (j, p) in r.permutations.iter().enumerate() {
                put_sid(&mut perms, 8 * j, &p.name);
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

    fn unit(&mut self, tag: &Tag) -> Result<TagId> {
        let u = Unit::read(self.h1, tag)?;
        let g: [u8; 4] = tag.group.0.to_be_bytes();
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
