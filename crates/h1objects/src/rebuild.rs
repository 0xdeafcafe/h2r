//! Objects written field by field from typed definitions
//! ([`tags::defs`]), with no retail template bytes. Every field starts at
//! its definition's default (zero, null or empty; unknown spans zero) and
//! only what the conversion knows is set, plus, for unknown spans where
//! zero is not neutral, the words most retail tags of the same role hold
//! (computed from the base map at conversion time). Scenery: the `scen`,
//! its `hlmt` and its `mode`'s material list, the only scenery path since
//! converter version 50. The field-by-field comparison with the removed
//! template path is in `tests/rebuild_disc.rs` and
//! `docs/formats/tag-definitions.md`.

use map::build::{Block, FixupKind};
use map::{FourCC, TagId};
use tags::defs::{self, Defs, Record, StructDef, Val};

use crate::coll::sid_name;
use crate::h1;
use crate::{Error, Result};

/// `obje +0` for scenery: its index in the engine's object type table
/// (docs/formats/objects.md).
pub const SCENERY_TYPE: u32 = 6;

fn def(group: &[u8; 4]) -> Result<&'static StructDef> {
    Defs::builtin()
        .group(FourCC(*group))
        .ok_or_else(|| Error::Invalid(format!("no definition for {}", FourCC(*group))))
}

fn set(r: &mut Record, path: &str, v: Val) -> Result<()> {
    r.set(path, v).map_err(|e| Error::Invalid(e.to_string()))
}

fn built(d: &StructDef, r: &Record) -> Result<Block> {
    defs::to_built(d, r, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))
}

/// The `scen` root: the object header with Halo 1's bounding sphere, the
/// model and the predicted resources, the role's words ([`SceneryRole`])
/// in its unknown spans, everything else at its default.
pub fn scenery(
    obj: &h1::Object,
    hlmt: Option<TagId>,
    predicted: &[Predicted],
    words: &[(String, u32)],
) -> Result<Block> {
    let d = def(b"scen")?;
    let mut r = defs::default_record(d);
    role_words(&mut r, words)?;
    set(&mut r, "object.object_type", Val::Enum(SCENERY_TYPE))?;
    set(
        &mut r,
        "object.bounding_radius",
        Val::Float(obj.bounding_radius),
    )?;
    set(
        &mut r,
        "object.bounding_offset",
        Val::floats(&obj.bounding_offset),
    )?;
    set(
        &mut r,
        "object.model",
        Val::tag(FourCC(*b"hlmt"), hlmt.unwrap_or(TagId::NULL)),
    )?;
    predicted_resources(b"scen", &mut r, "object", predicted)?;
    built(d, &r)
}

/// Scenery's role words: per 4-byte unknown span of the `scen` and of the
/// `hlmt` its model is, the word at least half the base map's scenery
/// holds there when it is not zero (the rule of [`TURRET_ROLE`] and
/// [`RoleWords`]). Computed from the map at conversion time, so no retail
/// value is kept in the source.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneryRole {
    /// Scenery tags the words were taken from, and how many had a model.
    pub members: usize,
    pub models: usize,
    pub scen: Vec<(String, u32)>,
    pub hlmt: Vec<(String, u32)>,
}

/// [`SceneryRole`] over every `scen` the base map holds; `None` when it
/// has none.
pub fn scenery_role(base: &crate::base::Base) -> Result<Option<SceneryRole>> {
    let (ds, dh) = (def(b"scen")?, def(b"hlmt")?);
    let (mut ss, mut hs) = (Vec::new(), Vec::new());
    word_spans(ds, "", 0, &mut ss);
    word_spans(dh, "", 0, &mut hs);
    let model = crate::convert::h2::MODEL;
    let (mut st, mut ht) = (Tally::new(), Tally::new());
    let (mut members, mut models) = (0, 0);
    for t in base.tags.iter().filter(|t| t.1 == FourCC(*b"scen")) {
        let bytes = base.root(t.0)?;
        tally_words(&bytes, &ss, &mut st);
        members += 1;
        if let Some(h) = base.reference(&bytes, model) {
            tally_words(&base.root(h)?, &hs, &mut ht);
            models += 1;
        }
    }
    if members == 0 {
        return Ok(None);
    }
    Ok(Some(SceneryRole {
        members,
        models,
        scen: majority(&st),
        hlmt: majority(&ht),
    }))
}

/// One `hlmt` region: name, collision region (-1 none), physics region,
/// and per permutation its name, collision and physics permutation.
pub struct Region {
    pub name: String,
    pub collision: i8,
    pub physics: i8,
    pub permutations: Vec<(String, i8, i8)>,
}

/// What an `hlmt` built from scratch holds.
pub struct Model {
    pub render: TagId,
    pub collision: Option<TagId>,
    pub animation: Option<TagId>,
    pub physics: Option<TagId>,
    /// Node count and the node array ([`h1models::mode::hlmt_nodes`]).
    pub nodes: (u32, Block),
    pub regions: Vec<Region>,
    /// Role words for the unknown spans ([`SceneryRole::hlmt`]); empty
    /// leaves them zero.
    pub words: Vec<(String, u32)>,
    /// The materials ([`model_materials`]).
    pub materials: Vec<ModelMaterial>,
}

/// `hlmt +0x58`: the materials, elements of [`HLMT_MATERIAL_SIZE`]. The
/// engine maps a ray's hit on an object to a global material through it:
/// `0x000d8ae0` (its one caller, `0x001695fb`, tests a ray against an
/// object's model) reads element `i`'s `+0x10` for the hit's material
/// index `i`, with no bound check, so an empty block faults reading
/// `[0 + i*0x14 + 0x10]` (the Halo 1 sandbox's `0x000d8b33` crash).
pub const HLMT_MATERIALS: u32 = 0x58;
pub const HLMT_MATERIAL_SIZE: usize = 0x14;

/// One `hlmt` material. V (`tests/hlmt_materials_disc.rs`, coagulation,
/// beavercreek and 05a): on every retail `hlmt` with a `coll`, element
/// `k` is named as the `coll`'s material `k` and there are at least as
/// many; `+0x10` is the index in the globals' materials of the `+0xc`
/// name (of the `+0` name when `+0xc` is empty) on all 197 elements;
/// `+8`'s low half is `k`.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelMaterial {
    /// `+0`: the `coll` material's (or the `phmo` material's) name.
    pub name: String,
    /// `+0xc`: the global material's name.
    pub global: String,
    /// `+0x10`: its index in the globals' materials.
    pub global_index: i16,
}

/// The global material `name` with its index in `globals` (the base's
/// [`crate::base::Base::materials`]).
pub fn model_material(name: &str, global: &str, globals: &[String]) -> Result<ModelMaterial> {
    let i = globals
        .iter()
        .position(|g| g == global)
        .ok_or_else(|| Error::Invalid(format!("no global material {global}")))?;
    Ok(ModelMaterial {
        name: sid_name(name),
        global: global.to_owned(),
        global_index: i as i16,
    })
}

/// The materials of an `hlmt` whose collision model is converted from
/// Halo 1's `coll`: one per collision material, in order, each mapped to
/// the Halo 2 global material of its Halo 1 type
/// ([`crate::arsenal::MATERIALS`]).
pub fn model_materials(coll: &h1::Collision, globals: &[String]) -> Result<Vec<ModelMaterial>> {
    coll.materials
        .iter()
        .enumerate()
        .map(|(k, name)| {
            let t = coll.material_types.get(k).copied().unwrap_or(0);
            let (h1_type, global) = crate::arsenal::MATERIALS
                .get(t as usize)
                .ok_or_else(|| Error::Invalid(format!("coll material {name}: type {t}")))?;
            model_material(name, global, globals)
                .map_err(|e| Error::Invalid(format!("coll material {name} ({h1_type}): {e}")))
        })
        .collect()
}

/// The `hlmt +0x58` elements for `materials`: element `k` holds the name,
/// `+4` `0xffff0000` and `+8` `k | 0xffff0000` (retail's most common
/// words: 67 and 185 of 197 elements), the global name and its index.
pub fn model_materials_block(materials: &[ModelMaterial]) -> Block {
    let mut b = Block::zeroed(HLMT_MATERIAL_SIZE * materials.len());
    for (k, m) in materials.iter().enumerate() {
        let at = HLMT_MATERIAL_SIZE * k;
        if !m.name.is_empty() {
            b.string_id(at as u32, &m.name);
        }
        b.put_u32(at + 4, 0xffff_0000);
        b.put_u32(at + 8, 0xffff_0000 | k as u32);
        b.string_id((at + 0xc) as u32, &m.global);
        b.put_u32(at + 0x10, m.global_index as u16 as u32);
    }
    b
}

/// Replaces `hlmt`'s materials with `materials`.
pub fn set_model_materials(hlmt: &mut Block, materials: &[ModelMaterial]) {
    let a = HLMT_MATERIALS as usize;
    hlmt.fixups.retain(|f| f.offset != HLMT_MATERIALS);
    hlmt.bytes[a..a + 8].fill(0);
    if !materials.is_empty() {
        hlmt.block(
            HLMT_MATERIALS,
            materials.len() as u32,
            model_materials_block(materials),
        );
    }
}

/// The fixup child of `root`'s block field at `offset`, with its count.
fn block_child(root: &Block, offset: u32) -> Option<(u32, &Block)> {
    root.fixups.iter().find_map(|f| match &f.kind {
        FixupKind::Block { count, child } if f.offset == offset => Some((*count, child)),
        _ => None,
    })
}

/// The string ID fixup of `b` at `offset`, if any.
fn sid_at(b: &Block, offset: u32) -> Option<&str> {
    b.fixups.iter().find_map(|f| match &f.kind {
        FixupKind::StringId(s) if f.offset == offset => Some(s.as_str()),
        _ => None,
    })
}

/// `phmo +0x40`: the materials, elements of `0xc`, the name at `+0` and
/// the global material's name at `+4` ([`sphere_physics`],
/// [`physics_model`] write both).
pub const PHMO_MATERIALS: u32 = 0x40;
pub const PHMO_MATERIAL_SIZE: usize = 0xc;

/// The global material name of each of a built `phmo`'s materials (`+4`,
/// else `+0`).
pub fn phmo_material_globals(phmo: &Block) -> Vec<String> {
    let Some((n, child)) = block_child(phmo, PHMO_MATERIALS) else {
        return Vec::new();
    };
    (0..n)
        .map(|k| {
            let at = k * PHMO_MATERIAL_SIZE as u32;
            sid_at(child, at + 4)
                .or_else(|| sid_at(child, at))
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

/// A built `hlmt`'s materials, read back ([`set_model_materials`]).
pub fn model_materials_of(hlmt: &Block) -> Vec<ModelMaterial> {
    let Some((n, child)) = block_child(hlmt, HLMT_MATERIALS) else {
        return Vec::new();
    };
    (0..n as usize)
        .map(|k| {
            let at = HLMT_MATERIAL_SIZE * k;
            ModelMaterial {
                name: sid_at(child, at as u32).unwrap_or_default().to_owned(),
                global: sid_at(child, (at + 0xc) as u32).unwrap_or_default().to_owned(),
                global_index: i16::from_le_bytes([child.bytes[at + 0x10], child.bytes[at + 0x11]]),
            }
        })
        .collect()
}

/// The number of `hlmt`'s materials.
pub fn model_material_count(hlmt: &Block) -> u32 {
    block_child(hlmt, HLMT_MATERIALS).map_or(0, |b| b.0)
}

/// The `hlmt`.
pub fn model(m: &Model) -> Result<Block> {
    let d = def(b"hlmt")?;
    let mut r = defs::default_record(d);
    role_words(&mut r, &m.words)?;
    set(&mut r, "render_model", Val::tag(FourCC(*b"mode"), m.render))?;
    set(
        &mut r,
        "collision_model",
        Val::tag(FourCC(*b"coll"), m.collision.unwrap_or(TagId::NULL)),
    )?;
    let jmad = FourCC(*b"jmad");
    match m.animation {
        Some(id) => {
            set(
                &mut r,
                "animation_group",
                Val::Int(i64::from(jmad.as_u32())),
            )?;
            set(&mut r, "animation", Val::Datum(defs::Target::Id(id)))?;
        }
        None => set(&mut r, "animation_group", Val::Int(i64::from(u32::MAX)))?,
    }
    set(
        &mut r,
        "physics_model",
        Val::tag(FourCC(*b"phmo"), m.physics.unwrap_or(TagId::NULL)),
    )?;
    let node_def = d.child("nodes").expect("hlmt nodes");
    let nodes = defs::from_built_array(node_def, &m.nodes.1, m.nodes.0)
        .map_err(|e| Error::Invalid(format!("hlmt nodes: {e}")))?;
    set(&mut r, "nodes", Val::Block(nodes))?;
    let region_def = d.child("regions").expect("hlmt regions");
    let perm_def = d.child("regions.permutations").expect("hlmt permutations");
    let mut regions = Vec::new();
    for g in &m.regions {
        let mut e = defs::default_record(region_def);
        set(&mut e, "name", Val::text(&sid_name(&g.name)))?;
        set(&mut e, "collision_region", Val::Int(g.collision.into()))?;
        set(&mut e, "physics_region", Val::Int(g.physics.into()))?;
        let mut perms = Vec::new();
        for (name, c, p) in &g.permutations {
            let mut q = defs::default_record(perm_def);
            set(&mut q, "name", Val::text(&sid_name(name)))?;
            set(&mut q, "collision_permutation", Val::Int((*c).into()))?;
            set(&mut q, "physics_permutation", Val::Int((*p).into()))?;
            perms.push(q);
        }
        set(&mut e, "permutations", Val::Block(perms))?;
        regions.push(e);
    }
    set(&mut r, "regions", Val::Block(regions))?;
    let mut root = built(d, &r)?;
    set_model_materials(&mut root, &m.materials);
    Ok(root)
}

/// `mode +0x60`, the material list.
pub const MODE_MATERIALS: u32 = 0x60;

/// Replaces a `mode` root's material list with one written from the
/// definition: one material per shader, `shader` set (null where the
/// converter has none), every other field at its default.
pub fn mode_materials(root: &mut Block, shaders: &[TagId]) -> Result<()> {
    let d = def(b"mode")?;
    let m = d.child("materials").expect("mode materials");
    let mut elems = Vec::new();
    for s in shaders {
        let mut e = defs::default_record(m);
        set(&mut e, "shader", Val::tag(FourCC(*b"shad"), *s))?;
        elems.push(e);
    }
    root.fixups.retain(|f| f.offset != MODE_MATERIALS);
    let a = MODE_MATERIALS as usize;
    root.bytes[a..a + 8].fill(0);
    if elems.is_empty() {
        return Ok(());
    }
    let mut child = Block::zeroed(m.size as usize * elems.len()).aligned(m.align.max(1));
    for (i, e) in elems.iter().enumerate() {
        let one = built(m, e)?;
        let at = i * m.size as usize;
        child.bytes[at..at + m.size as usize].copy_from_slice(&one.bytes);
        for mut f in one.fixups {
            f.offset += at as u32;
            child.fixups.push(f);
        }
    }
    root.fixup(
        MODE_MATERIALS,
        FixupKind::Block {
            count: elems.len() as u32,
            child,
        },
    );
    Ok(())
}

/// `obje +0` for a vehicle (docs/formats/objects.md).
pub const VEHICLE_TYPE: u32 = 1;

/// `vehi +0x1f0`, the vehicle type a Halo 1 unit with no Halo 2
/// counterpart is built as: a turret, the stationary vehicle (Halo 2's
/// turrets hold no wheels, treads, hover points or engine raw, and their
/// one physics element is all but zero). a10's cryotube, chairs,
/// fighterbomber and Halo ring are props the scripts seat units in.
pub const ROLE_VEHICLE_TYPE: u32 = 6;

/// Unknown spans a vehicle built from scratch takes from its role rather
/// than zero, as (path in the `vehi` record, little-endian word): every
/// 4-byte unknown span where at least half of Halo 2's turret-type
/// vehicles (`vehicle_type` turret, 25 distinct tags on the disc) hold one
/// nonzero value, and that value. Zero is not neutral in these: most are
/// scales and distances that retail never leaves at zero, and a few are
/// `-1` indices where zero would name element 0. The table is derived
/// from the disc, not copied from one tag: `tests/unit_rebuild_disc.rs`
/// recomputes it and fails if it differs. Fields not listed stay at the
/// definition default (zero, null, empty).
pub const TURRET_ROLE: &[(&str, u32)] = &[
    ("unit.obje.unknown_18", 0x0001_0000),
    ("unit.obje.unknown_1c", 0x0000_0002),
    ("unit.obje.unknown_6c", 0x3f80_0000),
    ("unit.obje.unknown_70", 0x4020_0000),
    ("unit.obje.unknown_74", 0x4090_0000),
    ("unit.obje.unknown_78", 0x3e4c_cccd),
    ("unit.obje.unknown_7c", 0x3fa0_0000),
    ("unit.obje.unknown_80", 0x4020_0000),
    ("unit.obje.unknown_84", 0x4120_0000),
    ("unit.obje.unknown_88", 0x3e4c_cccd),
    ("unit.obje.unknown_8c", 0x3fa0_0000),
    ("unit.unknown_144", 0x4049_0fdb),
    ("unknown_210", 0x3f80_0000),
    ("unknown_23c", 0x40c0_0000),
    ("unknown_240", 0x41b4_0000),
    ("unknown_25c", 0x447a_0000),
    ("unknown_270", 0x3f80_0000),
    ("unknown_280", 0x3f80_0000),
    ("unknown_2c0", 0x3f80_0000),
    ("unknown_2c4", 0x3f7f_be77),
    ("unknown_2e0", 0x3f80_0000),
];

/// [`TURRET_ROLE`] for each seat (`vehi +0x1c8` elements).
pub const TURRET_SEAT_ROLE: &[(&str, u32)] = &[
    ("unknown_24", 0x3de3_8e39),
    ("unknown_28", 0x3de3_8e39),
    ("unknown_58", 0x3f80_0000),
    ("unknown_5c", 0x3f80_0000),
    ("unknown_ac", 0xffff_ffff),
];

/// [`TURRET_ROLE`] for the one physics element every retail vehicle has
/// (`vehi +0x2f8`, `0x2a0`): four `-1` halves; the rest of a turret's is
/// zero.
pub const TURRET_PHYSICS_ROLE: &[(&str, u32)] = &[
    ("unknown_18", 0xffff_0000),
    ("unknown_20", 0xffff_0000),
    ("unknown_28", 0xffff_0000),
    ("unknown_30", 0xffff_0000),
];

/// `vehi +0x1c8`, the seats block.
pub const VEHI_SEATS: &str = "block_1c8";
/// `vehi +0x2f8`, the physics block (one element on every retail vehicle).
pub const VEHI_PHYSICS: &str = "block_2f8";

fn role(r: &mut Record, table: &[(&str, u32)]) -> Result<()> {
    for (path, word) in table {
        set(r, path, Val::Bytes(word.to_le_bytes().to_vec()))?;
    }
    Ok(())
}

/// One element of an object's predicted resources (`obje +0xb4`, `8`
/// each: a `u16` kind and `u16` index at `+0`, the tag's datum at `+4`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Predicted {
    /// A render model: kind 2, index -1 (V: every retail vehicle's `mode`).
    RenderModel(TagId),
    /// A bitmap: kind 0, index 0 (V: every retail vehicle's bitmaps).
    Bitmap(TagId),
}

impl Predicted {
    fn word(self) -> (u32, TagId) {
        match self {
            Predicted::RenderModel(t) => (0xffff_0002, t),
            Predicted::Bitmap(t) => (0, t),
        }
    }
}

/// Writes `list` as the predicted resources of the object header at `path`
/// in a record of `group`.
pub(crate) fn predicted_resources(
    group: &[u8; 4],
    r: &mut Record,
    path: &str,
    list: &[Predicted],
) -> Result<()> {
    let e = def(group)?
        .child(&format!("{path}.predicted_resources"))
        .expect("obje predicted resources");
    let mut out = Vec::new();
    for p in list {
        let (w, id) = p.word();
        let mut x = defs::default_record(e);
        set(&mut x, "unknown_0", Val::Bytes(w.to_le_bytes().to_vec()))?;
        set(&mut x, "tag", Val::Datum(defs::Target::Id(id)))?;
        out.push(x);
    }
    set(r, &format!("{path}.predicted_resources"), Val::Block(out))
}

/// `vehi +0x1f0` read from Halo 1's `vehi +0x2f4` (`u16`). V: the same
/// value on all eight pairs of a Halo 1 vehicle and its Halo 2 counterpart
/// (warthog 1, scorpion 0, ghost and wraith 4, banshee 5, pelican and
/// dropship/phantom 3, Covenant gun turret 6), so the two games share the
/// enumeration.
pub const H1_VEHICLE_TYPE: usize = 0x2f4;
/// Halo 1's `vehi +0x2f0` flags (`u32`), the low half of Halo 2's
/// `vehicle_flags` (`vehi +0x1ec`). V: the low halves are equal on five of
/// the eight pairs (warthog, scorpion, wraith, banshee, gun turret); the
/// ghost differs in bit 8, the pelican and dropship in the bits Halo 2's
/// fliers add. Halo 2's high half has no Halo 1 counterpart and comes from
/// the vehicle type's role ([`VehicleRole::flags_high`]).
pub const H1_VEHICLE_FLAGS: usize = 0x2f0;

/// A seat as the rebuilt vehicle writes it: Halo 2's names for Halo 1's
/// seat ([`seat_names`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeatNames {
    /// `label`: the rider's animation mode, the name the converted biped
    /// graphs give Halo 1's seat label (`h1models::names::map_name`):
    /// `W-driver` is `warthog_d`, an unlisted label its lower-case,
    /// `-` to `_` form.
    pub label: String,
    /// `string_8`, the rider's marker, as the converted model names it
    /// (Halo 1's name).
    pub marker: String,
    /// `string_c`, the entry marker: `<marker> enter` when the model has
    /// one (V: Halo 2's seats hold `driver_enter` beside `driver`; Halo
    /// 1's models carry `driver enter`, `gunner enter`, `passenger
    /// enter`), else empty.
    pub enter_marker: String,
    /// `string_60`, the camera marker, empty for none.
    pub camera_marker: String,
}

/// Halo 2's names for a Halo 1 seat on a model with `markers` (Halo 1's
/// marker names, which the converted model keeps as they are).
pub fn seat_names(seat: &h1::Seat, markers: &[String]) -> SeatNames {
    // The converted model keeps Halo 1's marker names as they are
    // (`h1models::mode`), so a seat names them the same way.
    let found = |n: &str| {
        markers
            .iter()
            .find(|m| m.eq_ignore_ascii_case(n))
            .cloned()
            .unwrap_or_else(|| n.to_owned())
    };
    let enter = format!("{} enter", seat.marker);
    SeatNames {
        label: h1models::names::map_name(&seat.label).mode,
        marker: found(&seat.marker),
        enter_marker: if markers.iter().any(|m| m.eq_ignore_ascii_case(&enter)) {
            found(&enter)
        } else {
            String::new()
        },
        camera_marker: found(&seat.camera_marker),
    }
}

/// The words a vehicle of one type takes from the retail vehicles of that
/// type in the base map: the turret rule ([`TURRET_ROLE`]) over every
/// member, computed at conversion time so no retail value is kept in the
/// source.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VehicleRole {
    /// The root's words and its plain-word blocks (the physics element,
    /// `block_2f8`; `unit.block_1a8`, `unit.block_1b8`).
    pub words: RoleWords,
    /// Each seat's words: the majority over every seat of every member.
    pub seat: Vec<(String, u32)>,
    /// The high half of `vehicle_flags` most members hold.
    pub flags_high: u32,
    /// The words of each friction (wheel and tread) point, `+0x2f0`: the
    /// majority over every point of every member; `None` when fewer than
    /// half the members have any (fliers, hover vehicles, turrets).
    pub friction: Option<Vec<(String, u32)>>,
    /// The same for the hover points, `+0x2e8`.
    pub hover: Option<Vec<(String, u32)>>,
    /// The engine gears, `+0x268`: the element count most members with
    /// gears hold and each element's majority words, every 4-byte field
    /// (raw curve pointers included, kept as the words they are: the
    /// curves live in the shared map the base uses). Empty when fewer than
    /// half the members have gears.
    pub gears: Vec<Vec<(String, u32)>>,
}

/// `vehi +0x2f0`, the friction points (wheels and treads: V, the
/// warthog's four hold `left_front_tire` .. `right_back_tire`, the
/// scorpion's twelve its suspension and tread markers).
pub const VEHI_FRICTION: &str = "block_2f0";
/// `vehi +0x2e8`, the hover points (V: the ghost's three hold
/// `back_antigrav`, `left_front_antigrav`, `right_front_antigrav`; the
/// wraith's six, the spectre's four).
pub const VEHI_HOVER: &str = "block_2e8";
/// `vehi +0x268`, the engine gears (V: four on the warthog, two on the
/// scorpion; none on hover vehicles, fliers or turrets).
pub const VEHI_GEARS: &str = "block_268";
/// A point's marker (`+0`, both point blocks) and the region whose
/// destruction removes it (friction `+0x44`, hover `+0x34`). V: the
/// warthog's `left_front_tire` with region `lf_wheel`, the ghost's
/// `left_front_antigrav` with `l_wing`; the scorpion's regions are null.
pub const FRICTION_MARKER: &str = "string_0";
pub const FRICTION_REGION: &str = "string_44";
pub const HOVER_MARKER: &str = "string_0";
pub const HOVER_REGION: &str = "string_34";

/// A Halo 1 powered mass point as a Halo 2 point: the marker the converted
/// model gains for it and the region it belongs to (empty for none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Point {
    pub marker: String,
    pub region: String,
}

/// Per element of the block at `at` (element `e`) of every member: the
/// majority words over all elements together, `None` when fewer than half
/// the members have any element.
fn all_elements(
    base: &crate::base::Base,
    members: &[(String, Vec<u8>)],
    at: usize,
    e: &StructDef,
) -> Result<Option<Vec<(String, u32)>>> {
    let mut spans = Vec::new();
    word_spans(e, "", 0, &mut spans);
    let mut t = Tally::new();
    let mut having = 0;
    for (_, bytes) in members {
        let mut i = 0;
        while let Some(x) = base.element(bytes, at, e.size as usize, i)? {
            tally_words(&x, &spans, &mut t);
            i += 1;
        }
        having += usize::from(i > 0);
    }
    Ok((2 * having >= members.len() && having > 0).then(|| majority(&t)))
}

/// Every 4-byte field of `e` that holds a plain word or a raw pointer, as
/// (path, offset).
fn gear_spans(e: &StructDef) -> Vec<(String, u32)> {
    e.fields
        .iter()
        .filter(|f| {
            matches!(f.kind, defs::Kind::Raw) || matches!(f.kind, defs::Kind::Unknown { len: 4, .. })
        })
        .map(|f| (f.name.clone(), f.offset))
        .collect()
}

/// [`VehicleRole`] over the base map's vehicles whose `vehicle_type` is
/// `vehicle_type`; `None` when the base has none of the type.
pub fn vehicle_role(base: &crate::base::Base, vehicle_type: u16) -> Result<Option<VehicleRole>> {
    let d = def(b"vehi")?;
    let ty = field_offset(d, "vehicle_type")? as usize;
    let flags_at = field_offset(d, "vehicle_flags")? as usize;
    let seat_at = field_offset(d, VEHI_SEATS)? as usize;
    let seat_def = d.child(VEHI_SEATS).expect("vehi seats");
    let mut members = Vec::new();
    for t in base.tags.iter().filter(|t| t.1 == FourCC(*b"vehi")) {
        let bytes = base.root(t.0)?;
        if u16::from_le_bytes([bytes[ty], bytes[ty + 1]]) == vehicle_type {
            members.push((t.2.clone(), bytes));
        }
    }
    if members.is_empty() {
        return Ok(None);
    }
    let words = role_over(base, d, &members)?;
    let mut sspans = Vec::new();
    word_spans(seat_def, "", 0, &mut sspans);
    let mut seats = Tally::new();
    let mut flags = std::collections::BTreeMap::<u32, usize>::new();
    for (_, bytes) in &members {
        *flags
            .entry(crate::base::u32_at(bytes, flags_at) & 0xffff_0000)
            .or_default() += 1;
        let mut i = 0;
        while let Some(x) = base.element(bytes, seat_at, seat_def.size as usize, i)? {
            tally_words(&x, &sspans, &mut seats);
            i += 1;
        }
    }
    let flags_high = flags
        .iter()
        .max_by_key(|(w, c)| (**c, std::cmp::Reverse(**w)))
        .map_or(0, |(w, _)| *w);
    let block = |name: &str| -> Result<(usize, &StructDef)> {
        Ok((
            field_offset(d, name)? as usize,
            d.child(name)
                .ok_or_else(|| Error::Invalid(format!("vehi {name}")))?,
        ))
    };
    let (fat, fdef) = block(VEHI_FRICTION)?;
    let (hat, hdef) = block(VEHI_HOVER)?;
    let (gat, gdef) = block(VEHI_GEARS)?;
    // Gears: the count most members with gears hold, each element's
    // majority over the members of that count.
    let gspans = gear_spans(gdef);
    let mut counts = std::collections::BTreeMap::<usize, usize>::new();
    let mut per: Vec<Vec<Vec<u8>>> = Vec::new();
    for (_, bytes) in &members {
        let mut es = Vec::new();
        while let Some(x) = base.element(bytes, gat, gdef.size as usize, es.len())? {
            es.push(x);
        }
        if !es.is_empty() {
            *counts.entry(es.len()).or_default() += 1;
        }
        per.push(es);
    }
    let having: usize = counts.values().sum();
    let mut gears = Vec::new();
    if having > 0 && 2 * having >= members.len() {
        let n = *counts.iter().max_by_key(|(n, c)| (**c, **n)).expect("some").0;
        for k in 0..n {
            let mut t = Tally::new();
            for es in per.iter().filter(|es| es.len() == n) {
                tally_words(&es[k], &gspans, &mut t);
            }
            // Every gear field, zero included: a curve pointer or scale
            // left out would read as none.
            let mut words = Vec::new();
            for (path, vals) in &t {
                let (w, _) = vals.iter().max_by_key(|(_, c)| **c).expect("some");
                words.push((path.clone(), *w));
            }
            gears.push(words);
        }
    }
    Ok(Some(VehicleRole {
        words,
        seat: majority(&seats),
        flags_high,
        friction: all_elements(base, &members, fat, fdef)?,
        hover: all_elements(base, &members, hat, hdef)?,
        gears,
    }))
}

fn field_offset(d: &StructDef, name: &str) -> Result<u32> {
    d.fields
        .iter()
        .find(|f| f.name == name)
        .map(|f| f.offset)
        .ok_or_else(|| Error::Invalid(format!("{}: no field {name}", d.name)))
}

/// [`RoleWords`] over `members` (name, root bytes) of the group `d`
/// defines: per 4-byte unknown span of the root and its embedded structs,
/// the majority word; per plain-word block, the majority element count and
/// each element's majority words.
fn role_over(
    base: &crate::base::Base,
    d: &StructDef,
    members: &[(String, Vec<u8>)],
) -> Result<RoleWords> {
    let mut spans = Vec::new();
    word_spans(d, "", 0, &mut spans);
    let mut blocks = Vec::new();
    word_blocks(d, "", 0, &mut blocks);
    let mut root = Tally::new();
    let mut counts: Vec<std::collections::BTreeMap<usize, usize>> =
        vec![Default::default(); blocks.len()];
    let mut elements: Vec<Vec<Tally>> = vec![Vec::new(); blocks.len()];
    for (_, bytes) in members {
        tally_words(bytes, &spans, &mut root);
        for (k, (_, at, e)) in blocks.iter().enumerate() {
            let mut espans = Vec::new();
            word_spans(e, "", 0, &mut espans);
            let mut i = 0;
            while let Some(x) = base.element(bytes, *at as usize, e.size as usize, i)? {
                if elements[k].len() <= i {
                    elements[k].push(Tally::new());
                }
                tally_words(&x, &espans, &mut elements[k][i]);
                i += 1;
            }
            if i > 0 {
                *counts[k].entry(i).or_default() += 1;
            }
        }
    }
    let mut out_blocks = Vec::new();
    for (k, (path, _, _)) in blocks.iter().enumerate() {
        let having: usize = counts[k].values().sum();
        if 2 * having < members.len() {
            continue;
        }
        let (n, _) = counts[k]
            .iter()
            .max_by_key(|(n, c)| (**c, **n))
            .expect("some");
        out_blocks.push((
            path.clone(),
            elements[k][..*n].iter().map(majority).collect(),
        ));
    }
    Ok(RoleWords {
        members: members.iter().map(|m| m.0.clone()).collect(),
        root: majority(&root),
        blocks: out_blocks,
    })
}

/// What a rebuilt vehicle carries from Halo 1 ([`vehicle`]).
pub struct VehicleSpec<'a> {
    pub object: &'a h1::Object,
    pub hlmt: Option<TagId>,
    pub seats: &'a [SeatNames],
    pub predicted: &'a [Predicted],
    pub camera_field_of_view: f32,
    /// Halo 1's vehicle type ([`H1_VEHICLE_TYPE`]).
    pub vehicle_type: u16,
    /// Halo 1's flags ([`H1_VEHICLE_FLAGS`]); the low half is written.
    pub flags: u32,
    /// The converted weapons of Halo 1's unit weapons, in order (`unit
    /// weapons`, `+0x1c0`).
    pub weapons: &'a [TagId],
    /// The type's role over the base's vehicles; `None` takes the turret
    /// tables ([`TURRET_ROLE`]), as for a type the base has none of.
    pub role: Option<&'a VehicleRole>,
    /// Halo 1's powered mass points: friction points where the role has
    /// them, else hover points where it has those, else none.
    pub points: &'a [Point],
}

/// A Halo 1 vehicle written from the `vehi` definition, no retail template
/// read: the object header (type, bounds, model, predicted resources),
/// Halo 1's vehicle type, flags (low half) and camera field of view, one
/// seat per Halo 1 seat ([`SeatNames`]), the unit's weapons, and the
/// type's role words ([`VehicleRole`]; the turret tables when there is
/// none). Everything else is the definition default.
pub fn vehicle(v: &VehicleSpec) -> Result<Block> {
    let d = def(b"vehi")?;
    let mut r = defs::default_record(d);
    match v.role {
        Some(role) => {
            role_words(&mut r, &role.words.root)?;
            for (path, elems) in &role.words.blocks {
                let e = d
                    .child(path)
                    .ok_or_else(|| Error::Invalid(format!("vehi {path}")))?;
                let mut out = Vec::new();
                for words in elems {
                    let mut x = defs::default_record(e);
                    role_words(&mut x, words)?;
                    out.push(x);
                }
                set(&mut r, path, Val::Block(out))?;
            }
        }
        None => {
            role(&mut r, TURRET_ROLE)?;
            let phys_def = d.child(VEHI_PHYSICS).expect("vehi physics");
            let mut p = defs::default_record(phys_def);
            role(&mut p, TURRET_PHYSICS_ROLE)?;
            set(&mut r, VEHI_PHYSICS, Val::Block(vec![p]))?;
        }
    }
    // Every retail vehicle has exactly one physics element.
    if !matches!(r.at(VEHI_PHYSICS), Some(Val::Block(b)) if b.len() == 1) {
        let phys_def = d.child(VEHI_PHYSICS).expect("vehi physics");
        let mut p = defs::default_record(phys_def);
        role(&mut p, TURRET_PHYSICS_ROLE)?;
        set(&mut r, VEHI_PHYSICS, Val::Block(vec![p]))?;
    }
    let (obj, hlmt, predicted) = (v.object, v.hlmt, v.predicted);
    set(
        &mut r,
        "unit.camera_field_of_view",
        Val::Float(v.camera_field_of_view),
    )?;
    set(&mut r, "unit.obje.object_type", Val::Enum(VEHICLE_TYPE))?;
    set(
        &mut r,
        "unit.obje.bounding_radius",
        Val::Float(obj.bounding_radius),
    )?;
    set(
        &mut r,
        "unit.obje.bounding_offset",
        Val::floats(&obj.bounding_offset),
    )?;
    set(
        &mut r,
        "unit.obje.model",
        Val::tag(FourCC(*b"hlmt"), hlmt.unwrap_or(TagId::NULL)),
    )?;
    set(&mut r, "vehicle_type", Val::Enum(v.vehicle_type.into()))?;
    let high = v.role.map_or(0, |x| x.flags_high);
    set(
        &mut r,
        "vehicle_flags",
        Val::Int(i64::from(high | (v.flags & 0xffff))),
    )?;
    predicted_resources(b"vehi", &mut r, "unit.obje", predicted)?;
    let wdef = d.child("unit.weapons").expect("unit weapons");
    let weapons = v
        .weapons
        .iter()
        .map(|w| {
            let mut e = defs::default_record(wdef);
            // The builder writes a reference fixup's group word itself.
            set(
                &mut e,
                "weapon",
                Val::Ref {
                    group: 0,
                    target: defs::Target::Id(*w),
                },
            )?;
            Ok(e)
        })
        .collect::<Result<Vec<_>>>()?;
    set(&mut r, "unit.weapons", Val::Block(weapons))?;
    let seat_def = d.child(VEHI_SEATS).expect("vehi seats");
    let mut out = Vec::new();
    for s in v.seats {
        let mut e = defs::default_record(seat_def);
        match v.role {
            Some(x) => role_words(&mut e, &x.seat)?,
            None => role(&mut e, TURRET_SEAT_ROLE)?,
        }
        set(&mut e, "label", Val::text(&s.label))?;
        // V: Halo 2's seats hold the rider's marker at +8 (`driver`,
        // `gunner`), the entry marker at +0xc (`driver_enter`) and the
        // camera marker at +0x60 (`banshee_d_camera`), each a marker of the
        // vehicle's model.
        set(&mut e, "string_8", Val::text(&s.marker))?;
        set(&mut e, "string_c", Val::text(&s.enter_marker))?;
        set(&mut e, "string_60", Val::text(&s.camera_marker))?;
        out.push(e);
    }
    set(&mut r, VEHI_SEATS, Val::Block(out))?;
    if let Some(role) = v.role {
        let kinds = [
            (VEHI_FRICTION, &role.friction, FRICTION_MARKER, FRICTION_REGION),
            (VEHI_HOVER, &role.hover, HOVER_MARKER, HOVER_REGION),
        ];
        if let Some((block, Some(words), marker, region)) =
            kinds.into_iter().find(|k| k.1.is_some())
        {
            let e = d.child(block).expect("vehi points");
            let mut out = Vec::new();
            for p in v.points {
                let mut x = defs::default_record(e);
                role_words(&mut x, words)?;
                set(&mut x, marker, Val::text(&p.marker))?;
                set(&mut x, region, Val::text(&p.region))?;
                out.push(x);
            }
            set(&mut r, block, Val::Block(out))?;
        }
        if !role.gears.is_empty() {
            let e = d.child(VEHI_GEARS).expect("vehi gears");
            let mut out = Vec::new();
            for words in &role.gears {
                let mut x = defs::default_record(e);
                for (path, w) in words {
                    let raw = e
                        .fields
                        .iter()
                        .any(|f| f.name == *path && matches!(f.kind, defs::Kind::Raw));
                    let val = if raw {
                        Val::Raw(defs::RawVal::Word(*w))
                    } else {
                        Val::Bytes(w.to_le_bytes().to_vec())
                    };
                    set(&mut x, path, val)?;
                }
                out.push(x);
            }
            set(&mut r, VEHI_GEARS, Val::Block(out))?;
        }
    }
    built(d, &r)
}

/// `obje +0` for machines and controls: their indices in the engine's
/// object type table (docs/formats/objects.md).
pub const MACHINE_TYPE: u32 = 7;
pub const CONTROL_TYPE: u32 = 8;
/// `mach +0x11c` for a Halo 1 light fixture: Halo 2 has no `lifi` tags,
/// so a fixture is a platform, the machine type with no behaviour of its
/// own (a door runs its open timer, a gear loops its position).
pub const FIXTURE_MACHINE_TYPE: u32 = 1;
/// A device acceleration rate standing for "at once" (V: no retail
/// device holds 0 in an acceleration slot; 900 is in the power
/// acceleration slot of 143 of the disc's 149 devices and in the
/// depowered one of all 149).
pub const INSTANT_RATE: f32 = 900.0;

/// Halo 1 holds device timings as seconds, Halo 2 as rates per second
/// (`devi` in `crates/tags/src/defs/devi.txt`). A zero transition time is
/// a zero rate, as retail holds for devices that never move; a zero
/// acceleration time is [`INSTANT_RATE`].
pub fn device_rates(times: &[f32; 6]) -> [f32; 6] {
    std::array::from_fn(|i| {
        let t = times[i];
        if t > 0.0 {
            1.0 / t
        } else if i % 2 == 1 {
            INSTANT_RATE
        } else {
            0.0
        }
    })
}

/// A Halo 1 `mach`, `ctrl` or `lifi` written from the `mach` or `ctrl`
/// definition: the object header (type, bounds, model, predicted
/// resources), the device layer's flags, rates, delay and activation
/// radius, and the machine's type, flags, door open time, collision
/// response and elevator node (a light fixture: a platform). Sounds and
/// effects stay null (Halo 1's are `effe` tags this crate does not
/// convert). No retail template is read.
pub fn device(dev: &h1::Device, hlmt: Option<TagId>, predicted: &[Predicted]) -> Result<Block> {
    let control = dev.control.is_some();
    let group = if control { b"ctrl" } else { b"mach" };
    let d = def(group)?;
    let mut r = defs::default_record(d);
    let obj = &dev.object;
    let ty = if control { CONTROL_TYPE } else { MACHINE_TYPE };
    set(&mut r, "devi.obje.object_type", Val::Enum(ty))?;
    set(
        &mut r,
        "devi.obje.bounding_radius",
        Val::Float(obj.bounding_radius),
    )?;
    set(
        &mut r,
        "devi.obje.bounding_offset",
        Val::floats(&obj.bounding_offset),
    )?;
    set(
        &mut r,
        "devi.obje.model",
        Val::tag(FourCC(*b"hlmt"), hlmt.unwrap_or(TagId::NULL)),
    )?;
    predicted_resources(group, &mut r, "devi.obje", predicted)?;
    set(&mut r, "devi.device_flags", Val::Int(dev.flags.into()))?;
    let rates = device_rates(&dev.times);
    for (name, v) in [
        "power_rate",
        "power_acceleration_rate",
        "position_rate",
        "position_acceleration_rate",
        "depowered_position_rate",
        "depowered_position_acceleration_rate",
    ]
    .iter()
    .zip(rates)
    {
        set(&mut r, &format!("devi.{name}"), Val::Float(v))?;
    }
    set(&mut r, "devi.delay_time", Val::Float(dev.delay_time))?;
    set(
        &mut r,
        "devi.activation_radius",
        Val::Float(dev.activation_radius),
    )?;
    if let Some(c) = &dev.control {
        set(&mut r, "control_type", Val::Int(c.control_type.into()))?;
        set(&mut r, "control_trigger", Val::Int(c.trigger.into()))?;
        set(&mut r, "control_value", Val::Float(c.value))?;
    } else if let Some(m) = &dev.machine {
        set(&mut r, "machine_type", Val::Enum(m.machine_type.into()))?;
        set(&mut r, "machine_flags", Val::Int(m.flags.into()))?;
        set(&mut r, "door_open_time", Val::Float(m.door_open_time))?;
        set(
            &mut r,
            "collision_response",
            Val::Int(m.collision_response.into()),
        )?;
        set(&mut r, "elevator_node", Val::Int(m.elevator_node.into()))?;
    } else {
        set(&mut r, "machine_type", Val::Enum(FIXTURE_MACHINE_TYPE))?;
    }
    built(d, &r)
}

/// The smallest physics model Halo 2 accepts, written from the `phmo`
/// definition: one rigid body (no node, as on the disc's one-sphere
/// physics model) holding one sphere of `radius` at `centre`, of `mass`,
/// with one material. `vehicle_new` (`0x000eeb80`) refuses a vehicle whose
/// model has no physics model (`cmp [hlmt+0x24],-1` at `0x000eebcb`), so
/// `object_new` deletes it and its name stays `none`; a vehicle with no
/// solid Halo 1 collision to build one from ([`physics_model`]) gets this
/// one. A sphere this small barely touches anything.
///
/// The engine writes every shape's Havok header at load (`0x001ea9c0`:
/// the sphere's at `+0x20`, the transform's at `+0x30` with the sphere as
/// its child), so those words stay zero. The rest follows the disc's
/// one-sphere model (V): root `+8`/`+0xc` 1.0; rigid body `+0x1a` 1,
/// `+0x1e` 4, `+0x20` 1.0, `+0x28` 0.05; sphere header `+6` 0x25, `+8`
/// and `+0xc` 1.0, `+0x10` 0.99, `+0x14` the volume, `+0x18` the mass,
/// `+0x1e` 0xff; the inertia of a solid sphere (2/5 m r^2, 400 for its
/// 1000 kg and radius 1); the material's `+8` 0xffff.
pub fn sphere_physics(radius: f32, centre: [f32; 3], mass: f32, material: &str) -> Result<Block> {
    let d = def(b"phmo")?;
    let mut r = defs::default_record(d);
    let f = |v: f32| Val::Bytes(v.to_le_bytes().to_vec());
    let u16_hi = |v: u16| Val::Bytes([0, 0, v as u8, (v >> 8) as u8].to_vec());
    set(&mut r, "mass", Val::Float(mass))?;
    set(&mut r, "unknown_8", f(1.0))?;
    set(&mut r, "unknown_c", f(1.0))?;

    let rb_def = d.child("rigid_bodies").expect("phmo rigid bodies");
    let mut rb = defs::default_record(rb_def);
    set(&mut rb, "node", Val::Int(-1))?;
    set(&mut rb, "bounding_sphere_offset", Val::floats(&centre))?;
    set(&mut rb, "bounding_sphere_radius", Val::Float(radius))?;
    set(&mut rb, "unknown_18", u16_hi(1))?;
    set(&mut rb, "unknown_1c", u16_hi(4))?;
    set(&mut rb, "unknown_20", f(1.0))?;
    set(&mut rb, "unknown_28", f(0.05))?;
    set(&mut rb, "shape_type", Val::Int(0))?;
    set(&mut rb, "shape_index", Val::Int(0))?;
    set(&mut rb, "mass", Val::Float(mass))?;
    set(&mut rb, "center_of_mass", Val::floats(&centre))?;
    let i = 0.4 * mass * radius * radius;
    set(&mut rb, "inertia_tensor_x", Val::floats(&[i, 0.0, 0.0]))?;
    set(&mut rb, "inertia_tensor_y", Val::floats(&[0.0, i, 0.0]))?;
    set(&mut rb, "inertia_tensor_z", Val::floats(&[0.0, 0.0, i]))?;
    set(&mut r, "rigid_bodies", Val::Block(vec![rb]))?;

    let mat_def = d.child("materials").expect("phmo materials");
    let mut m = defs::default_record(mat_def);
    set(&mut m, "name", Val::text(material))?;
    set(&mut m, "string_4", Val::text(material))?;
    set(&mut m, "unknown_8", Val::Bytes(vec![0xff, 0xff, 0, 0]))?;
    set(&mut r, "materials", Val::Block(vec![m]))?;

    let sphere_def = d.child("spheres").expect("phmo spheres");
    let mut s = defs::default_record(sphere_def);
    let volume = 4.0 / 3.0 * std::f32::consts::PI * radius * radius * radius;
    set(&mut s, "header.name", Val::text("sphere"))?;
    set(&mut s, "header.material", Val::Int(0))?;
    set(&mut s, "header.unknown_6", Val::Bytes(vec![0x25, 0]))?;
    set(&mut s, "header.unknown_8", f(1.0))?;
    set(&mut s, "header.unknown_c", f(1.0))?;
    set(&mut s, "header.unknown_10", f(0.99))?;
    set(&mut s, "header.unknown_14", f(volume))?;
    set(&mut s, "header.unknown_18", f(mass))?;
    set(&mut s, "header.unknown_1c", Val::Bytes(vec![0, 0, 0xff, 0]))?;
    set(&mut s, "reference_count", Val::Int(0x80))?;
    set(&mut s, "radius", Val::Float(radius))?;
    set(&mut s, "rotation_x", Val::floats(&[1.0, 0.0, 0.0]))?;
    set(&mut s, "rotation_y", Val::floats(&[0.0, 1.0, 0.0]))?;
    set(&mut s, "rotation_z", Val::floats(&[0.0, 0.0, 1.0]))?;
    set(&mut s, "translation", Val::floats(&centre))?;
    set(&mut r, "spheres", Val::Block(vec![s]))?;

    let region_def = d.child("regions").expect("phmo regions");
    let perm_def = d.child("regions.permutations").expect("phmo permutations");
    let body_def = d
        .child("regions.permutations.block_4")
        .expect("phmo permutation bodies");
    let mut body = defs::default_record(body_def);
    set(&mut body, "unknown_0", Val::Bytes(vec![0, 0]))?;
    let mut perm = defs::default_record(perm_def);
    set(&mut perm, "name", Val::text("default"))?;
    set(&mut perm, "block_4", Val::Block(vec![body]))?;
    let mut region = defs::default_record(region_def);
    set(&mut region, "name", Val::text("default"))?;
    set(&mut region, "permutations", Val::Block(vec![perm]))?;
    set(&mut r, "regions", Val::Block(vec![region]))?;
    built(d, &r)
}

/// One rigid body of a built physics model: the node it rides (`-1` the
/// object itself), its convex pieces in that node's space, and its mass.
#[derive(Clone, Debug)]
pub struct Body {
    pub node: i16,
    pub pieces: Vec<crate::physics::Hull>,
    pub mass: f32,
}

/// A physics model node: name and its parent, next sibling and first
/// child (`-1` none), as the model's own nodes.
#[derive(Clone, Debug)]
pub struct PhysNode {
    pub name: String,
    pub parent: i16,
    pub next_sibling: i16,
    pub first_child: i16,
}

/// Rigid body `+0x1a` and `+0x1e` of a built device's bodies (V: the
/// disc's polyhedron physics models, `high_small_door`, `ghost`,
/// `pelican`).
pub const DEVICE_BODY: (u16, u16) = (2, 5);
/// Rigid body `+0x1a` and `+0x1e` of a built biped's bodies. V: every
/// rigid body of every retail biped physics model on the base holds 2 and
/// 1 there (`tests/unit_rebuild_disc.rs`).
pub const BIPED_BODY: (u16, u16) = (2, 1);

/// The global material every built piece names, with the index retail
/// shapes of that material hold at header `+6` (V: the disc's
/// `high_small_door`; the index's table is not established).
pub const PIECE_MATERIAL: (&str, u16) = ("hard_metal_thick_cov", 76);

/// A physics model of convex pieces, written from the `phmo` definition:
/// one rigid body per [`Body`], its shape a polyhedron when it has one
/// piece and a list of polyhedra otherwise; `nodes` (the model's own, so
/// a body's node index means the same node in both) written when any body
/// names one. Each body's mass spreads over its pieces by volume; its
/// centre of mass and inertia are the pieces' (crate::physics).
///
/// The layout follows the disc's polyhedron physics models (V, the
/// `high_small_door`, `ghost` and `pelican` physics models):
/// - a polyhedron of up to 12 corners keeps them inline at `+0x60` as
///   up to three groups of four transposed points (capacity `0x80000003`);
///   a bigger one takes its groups from the root `four_vectors` block, in
///   polyhedron order, with the group count in the capacity. A group's
///   unused lanes repeat its last corner. Planes always come from the
///   root `plane_equations` block, in order; `+0xfc` holds the plane count
///   with bit 31 set.
/// - shape header: `+8` 1.0, `+0xc` 0.9, `+0x10` 0.6, `+0x14` the piece's
///   volume, `+0x18` its mass, `+0x1e` 0xff; `+0x26` 0x80.
/// - a list of up to four children keeps them inline (capacity
///   `0x80000004`, unused slots type 0 index 0xffff); a longer one takes
///   them from the root `list_children` block, capacity `0x80000000 | n`.
///   `+6` 0x80.
/// - rigid body `+0x1a` and `+0x1e` as `words` ([`DEVICE_BODY`],
///   [`BIPED_BODY`]), `+0x20` 1.0, `+0x28` 0.05.
/// - node `+4` flags (0), `+6` parent, `+8` next sibling, `+0xa` first
///   child, each `-1` none.
///
/// Every runtime word (shape vtables and pointers) stays zero: the engine
/// writes them at load (`0x001ea9c0`).
pub fn physics_model(bodies: &[Body], nodes: &[PhysNode], words: (u16, u16)) -> Result<Block> {
    let d = def(b"phmo")?;
    let mut r = defs::default_record(d);
    let f = |v: f32| Val::Bytes(v.to_le_bytes().to_vec());
    let u16_hi = |v: u16| Val::Bytes([0, 0, v as u8, (v >> 8) as u8].to_vec());
    let u32b = |v: u32| Val::Bytes(v.to_le_bytes().to_vec());
    let child = |p: &str| {
        d.child(p)
            .ok_or_else(|| Error::Invalid(format!("phmo {p}")))
    };
    let (mat_name, mat_index) = PIECE_MATERIAL;
    let total: f32 = bodies.iter().map(|b| b.mass).sum();
    set(&mut r, "mass", Val::Float(total))?;
    set(&mut r, "unknown_8", f(1.0))?;
    set(&mut r, "unknown_c", f(1.0))?;

    let mut m = defs::default_record(child("materials")?);
    set(&mut m, "name", Val::text(mat_name))?;
    set(&mut m, "string_4", Val::text(mat_name))?;
    set(&mut r, "materials", Val::Block(vec![m]))?;

    let poly_def = child("polyhedra")?;
    let fv_def = child("four_vectors")?;
    let plane_def = child("plane_equations")?;
    let list_def = child("lists")?;
    let lc_def = child("list_children")?;
    let rb_def = child("rigid_bodies")?;
    let (mut polys, mut fvs, mut planes, mut lists, mut lcs, mut rbs) =
        (vec![], vec![], vec![], vec![], vec![], vec![]);
    for body in bodies {
        let volume: f32 = body.pieces.iter().map(|p| p.volume).sum();
        if body.pieces.is_empty() || volume <= 0.0 {
            return Err(Error::Invalid("a physics body with no volume".into()));
        }
        let density = body.mass / volume;
        let first = polys.len();
        for piece in &body.pieces {
            let mut p = defs::default_record(poly_def);
            set(&mut p, "header.name", Val::text("piece"))?;
            set(&mut p, "header.material", Val::Int(0))?;
            set(
                &mut p,
                "header.unknown_6",
                Val::Bytes(mat_index.to_le_bytes().to_vec()),
            )?;
            set(&mut p, "header.unknown_8", f(1.0))?;
            set(&mut p, "header.unknown_c", f(0.9))?;
            set(&mut p, "header.unknown_10", f(0.6))?;
            set(&mut p, "header.unknown_14", f(piece.volume))?;
            set(&mut p, "header.unknown_18", f(piece.volume * density))?;
            set(&mut p, "header.unknown_1c", Val::Bytes(vec![0, 0, 0xff, 0]))?;
            set(&mut p, "reference_count", Val::Int(0x80))?;
            let (centre, half) = piece.aabb();
            set(&mut p, "aabb_half_extents", Val::floats(&half))?;
            set(&mut p, "aabb_center", Val::floats(&centre))?;
            let groups: Vec<[[f32; 4]; 3]> = piece
                .vertices
                .chunks(4)
                .map(|g| {
                    std::array::from_fn(|a| {
                        std::array::from_fn(|k| g.get(k).unwrap_or(&g[g.len() - 1])[a])
                    })
                })
                .collect();
            set(&mut p, "four_vector_count", Val::Int(groups.len() as i64))?;
            if groups.len() <= 3 {
                set(&mut p, "four_vector_capacity", Val::Int(0x8000_0003))?;
                for (g, xyz) in groups.iter().enumerate() {
                    for (a, axis) in ["x", "y", "z"].iter().enumerate() {
                        set(&mut p, &format!("inline_{axis}_{g}"), Val::floats(&xyz[a]))?;
                    }
                }
            } else {
                set(
                    &mut p,
                    "four_vector_capacity",
                    Val::Int(0x8000_0000 | groups.len() as i64),
                )?;
                for xyz in &groups {
                    let mut e = defs::default_record(fv_def);
                    set(&mut e, "x", Val::floats(&xyz[0]))?;
                    set(&mut e, "y", Val::floats(&xyz[1]))?;
                    set(&mut e, "z", Val::floats(&xyz[2]))?;
                    fvs.push(e);
                }
            }
            set(
                &mut p,
                "vertex_count",
                Val::Int(piece.vertices.len() as i64),
            )?;
            set(&mut p, "plane_count", Val::Int(piece.planes.len() as i64))?;
            set(
                &mut p,
                "unknown_fc",
                u32b(0x8000_0000 | piece.planes.len() as u32),
            )?;
            for pl in &piece.planes {
                let mut e = defs::default_record(plane_def);
                set(&mut e, "normal", Val::floats(&pl[..3]))?;
                set(&mut e, "w", Val::Float(pl[3]))?;
                planes.push(e);
            }
            polys.push(p);
        }
        let (shape_type, shape_index) = if body.pieces.len() == 1 {
            (4, first)
        } else {
            let n = body.pieces.len();
            let mut l = defs::default_record(list_def);
            set(&mut l, "unknown_4", Val::Bytes(vec![0, 0, 0x80, 0]))?;
            set(&mut l, "child_count", Val::Int(n as i64))?;
            for k in 0..4 {
                set(&mut l, &format!("inline_child_type_{k}"), Val::Int(0))?;
                set(&mut l, &format!("inline_child_index_{k}"), Val::Int(0xffff))?;
            }
            if n <= 4 {
                set(&mut l, "child_capacity", Val::Int(0x8000_0004))?;
                for k in 0..n {
                    set(&mut l, &format!("inline_child_type_{k}"), Val::Int(4))?;
                    set(
                        &mut l,
                        &format!("inline_child_index_{k}"),
                        Val::Int((first + k) as i64),
                    )?;
                }
            } else {
                set(&mut l, "child_capacity", Val::Int(0x8000_0000 | n as i64))?;
                for k in 0..n {
                    let mut c = defs::default_record(lc_def);
                    set(&mut c, "shape_type", Val::Int(4))?;
                    set(&mut c, "shape_index", Val::Int((first + k) as i64))?;
                    lcs.push(c);
                }
            }
            lists.push(l);
            (0xe, lists.len() - 1)
        };
        let (mass, com, inertia) = crate::physics::mass_properties(&body.pieces, density);
        let all: Vec<[f32; 3]> = body
            .pieces
            .iter()
            .flat_map(|p| p.vertices.iter().copied())
            .collect();
        let (centre, _) = crate::physics::aabb(&all);
        let radius = all
            .iter()
            .map(|v| {
                ((v[0] - centre[0]).powi(2)
                    + (v[1] - centre[1]).powi(2)
                    + (v[2] - centre[2]).powi(2))
                .sqrt()
            })
            .fold(0.0f32, f32::max);
        let mut rb = defs::default_record(rb_def);
        set(&mut rb, "node", Val::Int(body.node.into()))?;
        set(&mut rb, "bounding_sphere_offset", Val::floats(&centre))?;
        set(&mut rb, "bounding_sphere_radius", Val::Float(radius))?;
        set(&mut rb, "unknown_18", u16_hi(words.0))?;
        set(&mut rb, "unknown_1c", u16_hi(words.1))?;
        set(&mut rb, "unknown_20", f(1.0))?;
        set(&mut rb, "unknown_28", f(0.05))?;
        set(&mut rb, "shape_type", Val::Int(shape_type))?;
        set(&mut rb, "shape_index", Val::Int(shape_index as i64))?;
        set(&mut rb, "mass", Val::Float(mass))?;
        set(&mut rb, "center_of_mass", Val::floats(&com))?;
        set(&mut rb, "inertia_tensor_x", Val::floats(&inertia[0]))?;
        set(&mut rb, "inertia_tensor_y", Val::floats(&inertia[1]))?;
        set(&mut rb, "inertia_tensor_z", Val::floats(&inertia[2]))?;
        rbs.push(rb);
    }
    set(&mut r, "rigid_bodies", Val::Block(rbs))?;
    set(&mut r, "polyhedra", Val::Block(polys))?;
    set(&mut r, "four_vectors", Val::Block(fvs))?;
    set(&mut r, "plane_equations", Val::Block(planes))?;
    set(&mut r, "lists", Val::Block(lists))?;
    set(&mut r, "list_children", Val::Block(lcs))?;

    let body_def = child("regions.permutations.block_4")?;
    let members = (0..bodies.len())
        .map(|i| {
            let mut b = defs::default_record(body_def);
            set(
                &mut b,
                "unknown_0",
                Val::Bytes((i as u16).to_le_bytes().to_vec()),
            )?;
            Ok(b)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut perm = defs::default_record(child("regions.permutations")?);
    set(&mut perm, "name", Val::text("default"))?;
    set(&mut perm, "block_4", Val::Block(members))?;
    let mut region = defs::default_record(child("regions")?);
    set(&mut region, "name", Val::text("default"))?;
    set(&mut region, "permutations", Val::Block(vec![perm]))?;
    set(&mut r, "regions", Val::Block(vec![region]))?;

    if bodies.iter().any(|b| b.node >= 0) {
        let node_def = child("nodes")?;
        let w = |a: i16, b: i16| {
            let mut v = a.to_le_bytes().to_vec();
            v.extend(b.to_le_bytes());
            Val::Bytes(v)
        };
        let ns = nodes
            .iter()
            .map(|n| {
                let mut e = defs::default_record(node_def);
                set(&mut e, "name", Val::text(&n.name))?;
                set(&mut e, "unknown_4", w(0, n.parent))?;
                set(&mut e, "unknown_8", w(n.next_sibling, n.first_child))?;
                Ok(e)
            })
            .collect::<Result<Vec<_>>>()?;
        set(&mut r, "nodes", Val::Block(ns))?;
    }
    built(d, &r)
}

/// `obje +0` for bipeds (docs/formats/objects.md).
pub const BIPED_TYPE: u32 = 0;

/// Biped words carried from Halo 1 as they are, as (`bipd` record path,
/// Halo 1 `bipd` offset). V: on the five pairs of a Halo 1 biped and its
/// Halo 2 counterpart (cyborg and masterchief, marine_armored and
/// marine, elite, grunt, jackal) the two words agree within 1 % on every
/// pair, and no other Halo 1 word does (`tests/unit_rebuild_disc.rs`
/// checks it). Their meaning is not established: `+0x1f4` holds 60 to 90
/// degrees in radians, `+0x220` 0.2, `+0x26c` 0.45 to 0.5, `+0x2a0` and
/// `+0x2a8` both 45 degrees. The camera's field of view (`unit +0xcc`,
/// Halo 1 `+0x1a0`) is named and written separately.
pub const BIPED_WORDS: &[(&str, usize)] = &[
    ("unknown_1f4", 0x2f8),
    ("unknown_220", 0x408),
    ("unknown_26c", 0x428),
    ("unknown_2a0", 0x360),
    ("unknown_2a8", 0x360),
];

/// Which retail bipeds a rebuilt biped takes its unknown spans from: the
/// Chief, humans, or the Covenant. A Halo 1 biped of none (flood,
/// sentinels, the monitor) takes its counterpart's
/// (`Converter::biped_rebuild_role`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BipedRole {
    Chief,
    Human,
    Covenant,
}

impl BipedRole {
    /// The role of a Halo 2 biped, by its path.
    pub fn of_h2(path: &str) -> Option<Self> {
        let n = path.rsplit('\\').next().unwrap_or(path);
        let has = |ks: &[&str]| ks.iter().any(|k| path.contains(k));
        if has(&["flood", "sentinel", "monitor", "gravemind", "prophet"]) {
            return None;
        }
        if n.starts_with("masterchief") {
            Some(Self::Chief)
        } else if has(&[
            "\\marine",
            "\\miranda",
            "\\crewman",
            "\\lord_hood",
            "\\johnson",
            "\\cortana",
        ]) {
            Some(Self::Human)
        } else if has(&[
            "\\elite",
            "\\grunt",
            "\\jackal",
            "\\brute",
            "\\bugger",
            "\\hunter",
            "\\dervish",
            "\\heretic",
        ]) {
            Some(Self::Covenant)
        } else {
            None
        }
    }

    /// The role of a Halo 1 biped, by its path.
    pub fn of_h1(path: &str) -> Option<Self> {
        let has = |ks: &[&str]| ks.iter().any(|k| path.contains(k));
        if has(&["flood", "sentinel", "monitor", "infection"]) {
            None
        } else if has(&["\\cyborg"]) {
            Some(Self::Chief)
        } else if has(&[
            "\\marine",
            "\\crewman",
            "\\captain",
            "\\pilot",
            "\\cortana",
            "\\keyes",
            "\\johnson",
            "\\bisenti",
            "\\mendoza",
            "\\jenkins",
        ]) {
            Some(Self::Human)
        } else if has(&["\\elite", "\\grunt", "\\jackal", "\\hunter", "\\engineer"]) {
            Some(Self::Covenant)
        } else {
            None
        }
    }
}

/// A role's words: per record path, the word at least half the role's
/// retail bipeds hold there when it is not zero (the turret rule,
/// [`TURRET_ROLE`]); and for each block whose elements are plain words
/// (every field an unknown span), the element count most of the role's
/// bipeds that have any hold, with each element's majority words.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoleWords {
    pub members: Vec<String>,
    pub root: Vec<(String, u32)>,
    pub blocks: Vec<(String, Vec<RoleElement>)>,
}

/// One block element's role words, as (path in the element, word).
pub type RoleElement = Vec<(String, u32)>;

pub(crate) type Tally = std::collections::BTreeMap<String, std::collections::BTreeMap<u32, usize>>;

/// The 4-byte unknown spans of `d` and the structs it embeds (not its
/// blocks), as (path, offset).
pub(crate) fn word_spans(d: &StructDef, prefix: &str, at: u32, out: &mut Vec<(String, u32)>) {
    for f in &d.fields {
        let p = format!("{prefix}{}", f.name);
        match &f.kind {
            defs::Kind::Struct(s) => word_spans(s, &format!("{p}."), at + f.offset, out),
            defs::Kind::Unknown { len: 4, .. } => out.push((p, at + f.offset)),
            _ => {}
        }
    }
}

/// The plain-word blocks of `d` and the structs it embeds: (path, offset
/// of the block field, element definition).
fn word_blocks<'d>(
    d: &'d StructDef,
    prefix: &str,
    at: u32,
    out: &mut Vec<(String, u32, &'d StructDef)>,
) {
    for f in &d.fields {
        let p = format!("{prefix}{}", f.name);
        match &f.kind {
            defs::Kind::Struct(s) => word_blocks(s, &format!("{p}."), at + f.offset, out),
            defs::Kind::Block(e)
                if e.fields
                    .iter()
                    .all(|x| matches!(x.kind, defs::Kind::Unknown { len: 4, .. })) =>
            {
                out.push((p, at + f.offset, e))
            }
            _ => {}
        }
    }
}

pub(crate) fn tally_words(bytes: &[u8], spans: &[(String, u32)], t: &mut Tally) {
    for (p, o) in spans {
        let o = *o as usize;
        if o + 4 <= bytes.len() {
            let w = u32::from_le_bytes(bytes[o..o + 4].try_into().expect("4"));
            *t.entry(p.clone()).or_default().entry(w).or_default() += 1;
        }
    }
}

pub(crate) fn majority(t: &Tally) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    for (p, vals) in t {
        let n: usize = vals.values().sum();
        if let Some((w, c)) = vals.iter().max_by_key(|(_, c)| **c)
            && *w != 0
            && 2 * c >= n
        {
            out.push((p.clone(), *w));
        }
    }
    out
}

/// [`RoleWords`] over the bipeds of `role` the base map holds. Computed
/// from the map at conversion time, so no retail value is kept in the
/// source; `None` when the base has none of the role.
pub fn biped_role(base: &crate::base::Base, role: BipedRole) -> Result<Option<RoleWords>> {
    let d = def(b"bipd")?;
    let mut spans = Vec::new();
    word_spans(d, "", 0, &mut spans);
    let mut blocks = Vec::new();
    word_blocks(d, "", 0, &mut blocks);
    let members: Vec<(TagId, String)> = base
        .tags
        .iter()
        .filter(|t| t.1 == FourCC(*b"bipd") && BipedRole::of_h2(&t.2) == Some(role))
        .map(|t| (t.0, t.2.clone()))
        .collect();
    if members.is_empty() {
        return Ok(None);
    }
    let mut root = Tally::new();
    let mut counts: Vec<std::collections::BTreeMap<usize, usize>> =
        vec![Default::default(); blocks.len()];
    let mut elements: Vec<Vec<Tally>> = vec![Vec::new(); blocks.len()];
    for (id, _) in &members {
        let bytes = base.root(*id)?;
        tally_words(&bytes, &spans, &mut root);
        for (k, (_, at, e)) in blocks.iter().enumerate() {
            let mut espans = Vec::new();
            word_spans(e, "", 0, &mut espans);
            let mut i = 0;
            while let Some(x) = base.element(&bytes, *at as usize, e.size as usize, i)? {
                if elements[k].len() <= i {
                    elements[k].push(Tally::new());
                }
                tally_words(&x, &espans, &mut elements[k][i]);
                i += 1;
            }
            if i > 0 {
                *counts[k].entry(i).or_default() += 1;
            }
        }
    }
    let mut out_blocks = Vec::new();
    for (k, (path, _, _)) in blocks.iter().enumerate() {
        let having: usize = counts[k].values().sum();
        if 2 * having < members.len() {
            continue;
        }
        let (n, _) = counts[k]
            .iter()
            .max_by_key(|(n, c)| (**c, **n))
            .expect("some");
        out_blocks.push((
            path.clone(),
            elements[k][..*n].iter().map(majority).collect(),
        ));
    }
    Ok(Some(RoleWords {
        members: members.into_iter().map(|m| m.1).collect(),
        root: majority(&root),
        blocks: out_blocks,
    }))
}

fn role_words(r: &mut Record, words: &[(String, u32)]) -> Result<()> {
    for (path, word) in words {
        set(r, path, Val::Bytes(word.to_le_bytes().to_vec()))?;
    }
    Ok(())
}

/// `bipd +0x268`, `+0x26c`, `+0x270`: the standing and crouching
/// collision heights and the collision radius ([`h1::BIPED_SIZE`] holds
/// Halo 1's). V: on every retail biped of the base with character shapes,
/// the shapes below are these three's ([`biped_shapes`],
/// `tests/unit_rebuild_disc.rs`).
pub const BIPD_SIZE: [&str; 3] = ["unknown_268", "unknown_26c", "unknown_270"];
/// `bipd +0x284`: spheres (`0x80` each), `bipd +0x28c`: pills (`0x50`
/// each). E: the `bipd` post-process (`0x001c2390`, called from the tag
/// load dispatch `0x0011c840` on `bipd +0x264`) walks `+0x284` at stride
/// `0x80` writing a sphere's Havok header at element `+0x20` and a
/// transform's at `+0x30` (its child, at `+0x3c`, the sphere), `+0x28c`
/// at `0x50` writing a pill's at `+0x20`, and `+0x294` as `+0x284`;
/// `0x001c2460` returns pill `i` (standing 0, crouching 1) as the
/// character's shape, or sphere `i` of `+0x294` with no pills. Every one
/// of those runtime words stays zero here.
pub const BIPD_SPHERES: &str = "block_284";
pub const BIPD_PILLS: &str = "block_28c";
/// The spheres' radii as multiples of the collision radius, each resting
/// on the ground (centre one radius up). V: every retail biped of the base
/// with character shapes holds these three.
pub const SPHERE_SCALES: [f32; 3] = [1.0, 1.35, 1.7];

/// The character shapes of a biped of `size` ([`BIPD_SPHERES`],
/// [`BIPD_PILLS`]) as (path, value) pairs for the `bipd` record. A pill
/// runs from one radius up to its height less a radius (both ends carry
/// the radius as `w`), standing then crouching; each shape names no
/// material (`+4` -1) and holds `-1` at `+0x1e`, as retail's all do.
pub fn biped_shapes(size: &h1::BipedSize) -> Result<Vec<(String, Val)>> {
    let d = def(b"bipd")?;
    let r = size.radius;
    let w = |v: u32| Val::Bytes(v.to_le_bytes().to_vec());
    let f = |v: f32| w(v.to_bits());
    let header = |e: &mut Record| -> Result<()> {
        set(e, "unknown_4", w(0x0000_ffff))?;
        set(e, "unknown_1c", w(0xffff_0000))
    };
    let sphere_def = d.child(BIPD_SPHERES).expect("bipd spheres");
    let mut spheres = Vec::new();
    for k in SPHERE_SCALES {
        let mut e = defs::default_record(sphere_def);
        header(&mut e)?;
        set(&mut e, "unknown_2c", f(k * r))?;
        // The transform: identity rotation rows, then the centre.
        for at in ["unknown_40", "unknown_54", "unknown_68"] {
            set(&mut e, at, f(1.0))?;
        }
        set(&mut e, "unknown_78", f(k * r))?;
        spheres.push(e);
    }
    let pill_def = d.child(BIPD_PILLS).expect("bipd pills");
    let mut pills = Vec::new();
    for h in [size.standing_height, size.crouching_height] {
        let mut e = defs::default_record(pill_def);
        header(&mut e)?;
        set(&mut e, "unknown_2c", f(r))?;
        set(&mut e, "unknown_38", f((h - r).max(r)))?;
        set(&mut e, "unknown_3c", f(r))?;
        set(&mut e, "unknown_48", f(r))?;
        set(&mut e, "unknown_4c", f(r))?;
        pills.push(e);
    }
    let mut out = vec![
        (BIPD_SIZE[0].to_owned(), f(size.standing_height)),
        (BIPD_SIZE[1].to_owned(), f(size.crouching_height)),
        (BIPD_SIZE[2].to_owned(), f(r)),
    ];
    out.push((BIPD_SPHERES.to_owned(), Val::Block(spheres)));
    out.push((BIPD_PILLS.to_owned(), Val::Block(pills)));
    Ok(out)
}

/// Whether a Halo 1 biped's size can size its shapes: all three positive
/// and finite. A biped with none (Halo 1 leaves some at zero) keeps its
/// role's size and shapes.
pub fn usable_size(s: &h1::BipedSize) -> bool {
    [s.standing_height, s.crouching_height, s.radius]
        .iter()
        .all(|v| v.is_finite() && *v > 0.0)
}

/// Whether elements of `d` hold a tag reference at any depth.
fn holds_reference(d: &StructDef) -> bool {
    d.fields.iter().any(|f| match &f.kind {
        defs::Kind::TagRef { .. } => true,
        defs::Kind::Block(e) | defs::Kind::Struct(e) => holds_reference(e),
        _ => false,
    })
}

/// The fields of `d` that carry references, as record paths: every tag
/// reference of the root and its embedded structs, and every block whose
/// elements hold one at any depth (the block is the unit that carries
/// them: attachments with their markers, the HUD interfaces, camera
/// tracks, damage sections). Blocks of plain words never do, so these
/// never meet [`RoleWords::blocks`].
pub fn reference_fields(d: &StructDef) -> Vec<String> {
    fn walk(d: &StructDef, prefix: &str, out: &mut Vec<String>) {
        for f in &d.fields {
            let p = format!("{prefix}{}", f.name);
            match &f.kind {
                defs::Kind::Struct(s) => walk(s, &format!("{p}."), out),
                defs::Kind::TagRef { .. } => out.push(p),
                defs::Kind::Block(e) if holds_reference(e) => out.push(p),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(d, "", &mut out);
    out
}

/// `bipd` fields a rebuilt biped never takes from its counterpart: the
/// model is Halo 1's.
pub const BIPED_OWN: &[&str] = &["unit.obje.model"];
/// Taken from the counterpart with its references though they hold
/// none: attachments and change colours name the object's functions
/// (`obje +0x64`, matched by name, `0x000bac20`), so the functions come
/// with them.
pub const BIPED_WITH_REFERENCES: &[&str] = &["unit.obje.functions"];
/// `bipd +0x1a0`, the HUD interfaces (one `nhdt` reference each,
/// [`crate::health_hud::UNIT_HUD_INTERFACES`]).
pub const BIPD_HUD: &str = "unit.block_1a0";
/// `hlmt` fields a rebuilt model never takes from a counterpart: its
/// chain, nodes and regions are Halo 1's, and variants name the
/// counterpart's regions (the template path clears them too).
pub const MODEL_OWN: &[&str] = &[
    "render_model",
    "collision_model",
    "phys_ref_18",
    "physics_model",
    "variants",
    "regions",
    "nodes",
];

/// The fields of `counterpart` (a record of `d`) a rebuilt tag takes:
/// [`reference_fields`] less `own`, plus `with`.
pub fn counterpart_fields(
    d: &StructDef,
    counterpart: &Record,
    own: &[&str],
    with: &[&str],
) -> Vec<(String, Val)> {
    reference_fields(d)
        .into_iter()
        .filter(|p| !own.contains(&p.as_str()))
        .chain(with.iter().map(|p| p.to_string()))
        .filter_map(|p| counterpart.at(&p).cloned().map(|v| (p, v)))
        .collect()
}

/// What a rebuilt biped takes beyond Halo 1's header and its role's words.
#[derive(Clone, Debug, Default)]
pub struct BipedParts {
    /// The Halo 2 counterpart's `bipd` record ([`COUNTERPARTS`]): its
    /// references ([`counterpart_fields`]).
    pub counterpart: Option<Record>,
    /// HUD interfaces in place of the counterpart's ([`BIPD_HUD`]): the
    /// player's, with the health meter.
    pub hud: Option<Vec<TagId>>,
    /// The converted model's marker names ([`UNIT_MARKERS`]).
    pub markers: Vec<String>,
}

/// The unit's marker names, each the Halo 1 marker the Halo 1 engine uses
/// for it, written when the converted model has that marker:
///
/// - `unit +0xd4`, the camera marker: `head`. V: `head` on the retail
///   `masterchief` and `masterchief_mp`, whose models have a `head`
///   marker. Left empty, the camera sits at the biped's origin, its feet
///   (a converted cyborg through converter version 49: "the camera sits
///   low, the feet are visible").
/// - `unit +0x158` and `+0x15c`, the markers a held weapon attaches to,
///   right and left hand: `right hand`, `left hand`. E `0x000d0870`: a
///   weapon is attached with `0x0010cec0` (to `0x000b8ee0`) at the unit's
///   `+0x158` marker, or `+0x15c` for the left hand, the weapon's own
///   marker named by unit `+0x160` (else weapon `+0x284`). V: retail
///   `right_hand`/`left_hand` (Chief) and `right_hand_elite`/
///   `left_hand_elite`, each a marker of their models. Halo 1's units hold
///   no marker names (the cyborg's `bipd` has none: its engine uses fixed
///   ones), and its models name them with spaces.
///
/// `+0x160` stays empty: the weapon's own marker (`right_hand_mc` on the
/// Chief) is not on a Halo 1 weapon model.
pub const UNIT_MARKERS: [(&str, &str); 3] = [
    ("unit.string_d4", "head"),
    ("unit.string_158", "right hand"),
    ("unit.string_15c", "left hand"),
];

/// The Halo 2 biped a Halo 1 biped takes its references from, by words in
/// the Halo 1 path (all must appear), first match the base holds. The
/// pairs `tests/unit_rebuild_disc.rs` compares, and each role's nearest.
/// A biped none names takes the template path's template
/// ([`crate::convert::template_candidates`]).
pub const COUNTERPARTS: &[(&[&str], &str)] = &[
    (
        &["cyborg"],
        "objects\\characters\\masterchief\\masterchief_mp",
    ),
    (&["cyborg"], "objects\\characters\\masterchief\\masterchief"),
    (&["elite"], "objects\\characters\\elite\\elite"),
    (&["grunt"], "objects\\characters\\grunt\\grunt"),
    (&["jackal"], "objects\\characters\\jackal\\jackal"),
    (&["engineer"], "objects\\characters\\bugger\\bugger"),
    (&["hunter"], "objects\\characters\\hunter\\hunter"),
    (&["marine"], "objects\\characters\\marine\\marine"),
    (
        &["crewman"],
        "objects\\characters\\marine\\crewman\\crewman",
    ),
    (&["captain"], "objects\\characters\\marine\\marine"),
    (&["pilot"], "objects\\characters\\marine\\marine"),
    (&["johnson"], "objects\\characters\\marine\\marine"),
    (&["bisenti"], "objects\\characters\\marine\\marine"),
    (&["mendoza"], "objects\\characters\\marine\\marine"),
    (&["jenkins"], "objects\\characters\\marine\\marine"),
    (&["keyes"], "objects\\characters\\marine\\marine"),
    (&["cortana"], "objects\\characters\\marine\\marine"),
];

/// [`COUNTERPARTS`] rows matching the Halo 1 biped at `path`, in order.
pub fn counterpart_candidates(path: &str) -> Vec<&'static str> {
    let path = path.to_ascii_lowercase();
    COUNTERPARTS
        .iter()
        .filter(|(words, _)| words.iter().all(|w| path.contains(w)))
        .map(|r| r.1)
        .collect()
}

/// A Halo 1 biped written from the `bipd` definition: the object header
/// (type 0, Halo 1's bounding sphere, the model and its predicted
/// resources), its role's words ([`biped_role`]), Halo 1's camera field
/// of view, [`BIPED_WORDS`] and collision size with the character shapes
/// sized to it ([`biped_shapes`]), and every reference its counterpart
/// carries ([`BipedParts`]), the HUD interfaces replaced when given.
/// Blocks other than those stay empty.
pub fn biped(
    unit: &h1::Unit,
    hlmt: Option<TagId>,
    predicted: &[Predicted],
    role: &RoleWords,
    parts: &BipedParts,
) -> Result<Block> {
    let d = def(b"bipd")?;
    let mut r = defs::default_record(d);
    role_words(&mut r, &role.root)?;
    for (path, elems) in &role.blocks {
        let e = d
            .child(path)
            .ok_or_else(|| Error::Invalid(format!("bipd {path}")))?;
        let mut out = Vec::new();
        for words in elems {
            let mut x = defs::default_record(e);
            role_words(&mut x, words)?;
            out.push(x);
        }
        set(&mut r, path, Val::Block(out))?;
    }
    if let Some(c) = &parts.counterpart {
        for (path, v) in counterpart_fields(d, c, BIPED_OWN, BIPED_WITH_REFERENCES) {
            set(&mut r, &path, v)?;
        }
    }
    if let Some(hud) = &parts.hud {
        let e = d.child(BIPD_HUD).expect("bipd HUD interfaces");
        let mut out = Vec::new();
        for id in hud {
            let mut x = defs::default_record(e);
            set(&mut x, "ref_0", Val::tag(FourCC(*b"nhdt"), *id))?;
            out.push(x);
        }
        set(&mut r, BIPD_HUD, Val::Block(out))?;
    }
    let obj = &unit.object;
    set(&mut r, "unit.obje.object_type", Val::Enum(BIPED_TYPE))?;
    set(
        &mut r,
        "unit.obje.bounding_radius",
        Val::Float(obj.bounding_radius),
    )?;
    set(
        &mut r,
        "unit.obje.bounding_offset",
        Val::floats(&obj.bounding_offset),
    )?;
    set(
        &mut r,
        "unit.obje.model",
        Val::tag(FourCC(*b"hlmt"), hlmt.unwrap_or(TagId::NULL)),
    )?;
    predicted_resources(b"bipd", &mut r, "unit.obje", predicted)?;
    set(
        &mut r,
        "unit.camera_field_of_view",
        Val::Float(unit.camera_field_of_view),
    )?;
    for (path, marker) in UNIT_MARKERS {
        if parts.markers.iter().any(|m| m == marker) {
            set(&mut r, path, Val::text(marker))?;
        }
    }
    for (path, at) in BIPED_WORDS {
        if let Some((_, w)) = unit.biped_words.iter().find(|x| x.0 == *at) {
            set(&mut r, path, Val::Bytes(w.to_le_bytes().to_vec()))?;
        }
    }
    if let Some(size) = unit.size.filter(usable_size) {
        for (path, v) in biped_shapes(&size)? {
            set(&mut r, &path, v)?;
        }
    }
    built(d, &r)
}
