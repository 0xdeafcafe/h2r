//! Object tags built from their Halo 2 definitions and Halo 1 source data.
//! Vehicle controller dependencies and their evidence are listed in
//! `docs/halo1/role-audit.md`.

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

/// Shared fields retained by both object formats. H1 inserts its origin
/// offset between the bounding centre and acceleration scale.
pub(crate) fn object_tuning(r: &mut Record, path: &str, object: &h1::Object) -> Result<()> {
    set(r, &format!("{path}.flags"), Val::Flags(u32::from(object.flags & 7)))?;
    set(r, &format!("{path}.unknown_14"), Val::Bytes(object.acceleration_scale.to_le_bytes().to_vec()))
}

/// The `scen` root: the object header with Halo 1's bounding sphere, the
/// model and the predicted resources, everything else at its default.
pub fn scenery(
    obj: &h1::Object,
    hlmt: Option<TagId>,
    predicted: &[Predicted],
) -> Result<Block> {
    let d = def(b"scen")?;
    let mut r = defs::default_record(d);
    object_tuning(&mut r, "object", obj)?;
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
    object_change_colors(b"scen", &mut r, "object", &obj.change_colours)?;
    built(d, &r)
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
    /// Node count and the node array ([`h1_assets::models::mode::hlmt_nodes`]).
    pub nodes: (u32, Block),
    pub regions: Vec<Region>,
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

/// Seat access parameters for H1 seats which have no authored entry
/// constraints. The checks at 0xc9040 reject zero entry reach and angles.
pub const TURRET_SEAT_ROLE: &[(&str, u32)] = &[
    ("unknown_98", 0x3f80_0000),
    ("unknown_9c", 0x4049_0fdb),
    ("unknown_a0", 0x3f9c_61aa),
    ("unknown_a4", 0x4000_0000),
    ("unknown_ac", 0xffff_ffff),
];

/// Unassigned shape indices in the one physics element every vehicle has
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
/// A seat's entry tests: `0x000c9040` admits a rider only within the
/// distance at `+0x98` and the angles at `+0x9c` and `+0xa0`, and within
/// the distance at `+0xa4` of the seat's marker; zero admits no one.
pub const SEAT_ENTRY: &[&str] = &["unknown_98", "unknown_9c", "unknown_a0", "unknown_a4"];
/// A seat's entry-marker facing test, `+0x9c` (radians): `0x000c9040`
/// admits a rider only when the angle between the entry marker's forward
/// (its first axis) and the direction from the rider's eye to the marker
/// is at least pi less this, so the marker must face the rider within it.
/// V: retail markers face out of the door they serve (the warthog's
/// `driver_enter` forward is (0.15, 0.85, 0.5) on the driver's side, with
/// 70 degrees); Halo 1's `driver enter` markers face the vehicle's forward,
/// (1, 0, 0), so a rider square beside the door, 90 degrees off, was refused
/// (`tests/vehicle_ground_disc.rs`). A rebuilt seat holds [`ANY_FACING`].
pub const SEAT_ENTRY_FACING: &str = "unknown_9c";
/// [`SEAT_ENTRY_FACING`] that admits a rider from any side: pi, as every
/// retail turret seat holds ([`TURRET_SEAT_ROLE`]).
pub const ANY_FACING: f32 = std::f32::consts::PI;
/// A seat's flags ([`SEAT_DRIVER`]), `+0`.
pub const SEAT_FLAGS: &str = "unknown_0";
/// `vehi +0x2f8`, the physics block (one element on every retail vehicle).
pub const VEHI_PHYSICS: &str = "block_2f8";
/// The physics element's first shape when it is the wheels' phantom
/// multisphere: its sphere count at `+0x6c` (V: the retail warthog 4,
/// the scorpion 8, the ghost 0), the spheres from `+0x70`, `0x10` each
/// (centre x, y, z in the vehicle's space, then radius; V: the retail
/// warthog's front pair first, left before right, then the back pair).
pub const PHANTOM_SPHERE_COUNT: u32 = 0x6c;
/// See [`PHANTOM_SPHERE_COUNT`].
pub const PHANTOM_SPHERES: u32 = 0x70;
/// The most spheres one multisphere holds (V: the scorpion's twelve are
/// eight and a second shape's four).
pub const PHANTOM_MAX_SPHERES: u32 = 8;

fn role(r: &mut Record, scope: &str, table: &[(&str, u32)]) -> Result<()> {
    crate::provenance::note_words(scope, table);
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
/// fliers add. Halo 2's high bits retain their definition defaults.
pub const H1_VEHICLE_FLAGS: usize = 0x2f0;

/// Seat flags (`vehi +0x1c8` element `+0`, the word the definition leaves
/// unknown) the engine's boarding code reads. The enter-seat search
/// (`0x000c8bb0`) skips a seat with [`SEAT_NOT_PLAYER`] for a unit with a
/// player (`+0x13c` set) and one with [`SEAT_NOT_NON_PLAYER`] for any other
/// unit, and a seat with [`SEAT_NEEDS_DRIVER`] while the vehicle has no
/// driver (`+0x248`); `0x000c92c0` treats a seat with [`SEAT_BOARDING`] as
/// a boarding seat, entered only to take the seat its `+0x3e` names from
/// its rider. V: on 05a every retail boarding seat (`warthog_b_d`,
/// `ghost_b_l`, ...) holds [`SEAT_BOARDING`] and no other seat does; every
/// driver seat (`warthog_d`, `ghost_d`, `c_turret_ap_d`) holds
/// [`SEAT_DRIVER`], and the touch layout reads the same bit for the
/// driver (`boot::touch_glue`).
pub const SEAT_DRIVER: u32 = 1 << 2;
/// The gunner seat (V: Halo 1's `W-gunner` and every retail turret seat).
pub const SEAT_GUNNER: u32 = 1 << 3;
/// See [`SEAT_DRIVER`].
pub const SEAT_NEEDS_DRIVER: u32 = 1 << 9;
/// See [`SEAT_DRIVER`].
pub const SEAT_BOARDING: u32 = 1 << 11;
/// See [`SEAT_DRIVER`].
pub const SEAT_NOT_PLAYER: u32 = 1 << 16;
/// See [`SEAT_DRIVER`].
pub const SEAT_NOT_NON_PLAYER: u32 = 1 << 17;
/// A seat whose entry and exit open and close the vehicle. E: when a unit
/// enters a seat the engine tests the seat's flags bit 15 (`0x000d12b0`,
/// `shr edx,0Fh` at `0x000d1302`; called from the seat change at
/// `0x000ea979`, `0x000eab64`, `0x000eb1ff`) and, set, sends the vehicle
/// unit action `0x17`; on leaving (`0x000d1360`, from `0x000ea7a5`,
/// `0x000ea8ba`) action `0x18`. Through the action table at `0x004677c8`
/// those are `0x000e9370` and `0x000e9500`, which start the animation
/// state string ID `0x0700005c` or `0x0700005d` (two seven-character
/// names, D: `opening` and `closing` by length; not read off the string
/// table) on the vehicle and the objects attached to it. No other reader
/// of bit 15 of a seat's flags was found (every `vehi +0x1cc` read
/// followed by a bit 15 test). The retail ghost's driver seat holds it
/// (`0x801c`). Halo 1 has no such seat flag (it sets none above bit 10)
/// and its graphs no such state, so a rebuilt seat never takes it from
/// the role. It has nothing to do with grenades.
pub const SEAT_OPENS: u32 = 1 << 15;
/// A seat whose rider is the vehicle's gunner when a player rides it. E:
/// the rider assignment (`0x000cc810`) makes a seat's rider the gunner
/// (`+0x24c`, who fires the vehicle's weapon) when the seat holds
/// [`SEAT_GUNNER`] (`0x000cc934`), or this bit and the rider has a player
/// (`0x000cc939..0x000cc951`: `+0x13c` set). V: the retail warthog's
/// driver seat holds it (`0x48014`), so its player driver sounds the
/// horn, the warthog's one weapon. Halo 1 has no such bit: its gunner is
/// its gunner seat's rider only.
pub const SEAT_PLAYER_GUNNER: u32 = 1 << 18;
/// The seat flag bits Halo 1 holds (its seat `+0`), which a rebuilt seat
/// takes from Halo 1: Halo 1 sets none above bit 10, and on the eight
/// pairs of `tests/vehicle_rebuild_disc.rs` its bits agree with the Halo 2
/// counterpart's seat of the same label on most seats and on the driver
/// bit on every one.
pub const H1_SEAT_FLAGS: u32 = 0x7ff;

/// A rebuilt seat's flags: Halo 1's bits ([`H1_SEAT_FLAGS`]) over the
/// rest of the role's word, less the bits that would keep a rider out
/// (Halo 1 seats are neither boarding seats nor closed to players or
/// non-players), [`SEAT_OPENS`] (no Halo 1 vehicle opens for its
/// rider) and the player gunner bit ([`SEAT_PLAYER_GUNNER`]): who fires
/// is Halo 1's gunner bit alone. Through converter version 83 the
/// warthog's driver seat took that bit from the retail warthog's, and a
/// player driving the converted warthog fired its chaingun.
pub fn seat_flags(h1_flags: u32, role: u32) -> u32 {
    let closed = H1_SEAT_FLAGS
        | SEAT_BOARDING
        | SEAT_NOT_PLAYER
        | SEAT_NOT_NON_PLAYER
        | SEAT_OPENS
        | SEAT_PLAYER_GUNNER;
    (role & !closed) | (h1_flags & H1_SEAT_FLAGS)
}

/// A seat as the rebuilt vehicle writes it: Halo 2's names for Halo 1's
/// seat ([`seat_names`]) and Halo 1's flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeatNames {
    /// `label`: the rider's animation mode, the name the converted biped
    /// graphs give Halo 1's seat label (`h1_assets::models::names::mode_name`, the
    /// whole label): `W-driver` is `warthog_d`, an unlisted label its
    /// lower-case, `-` to `_` form (a10's lifepod: `driver lifepod`).
    pub label: String,
    /// `string_8`, the rider's marker, as the converted model names it
    /// (Halo 1's name); empty when the model has no such marker.
    pub marker: String,
    /// `string_c`, the entry marker: `<marker> enter` when the model has
    /// one (V: Halo 2's seats hold `driver_enter` beside `driver`; Halo
    /// 1's models carry `driver enter`, `gunner enter`, `passenger
    /// enter`), else empty.
    pub enter_marker: String,
    /// `string_60`, the camera marker, empty for none.
    pub camera_marker: String,
    /// `block_74`, the seat's camera track: Halo 1's first ([`camera_track`],
    /// set by the converter), `None` for none. Without one the engine falls
    /// back to the globals' default unit track, whose points reach 3.9
    /// above the camera marker and 3.7 behind it: on a vehicle the camera
    /// sits far above the hull.
    pub camera_track: Option<TagId>,
    pub camera_angles: [u32; 5],
    /// Halo 1's seat flags ([`seat_flags`]).
    pub flags: u32,
}

/// `vehi` seat `+0x74`, the camera tracks (`tagref trak` each). V: one on
/// the retail warthog's and scorpion's driver seats
/// (`tests/vehicle_camera_disc.rs`); the warthog driver's camera marker
/// (`+0x60`) is empty, so its track's points are from the origin.
pub const SEAT_CAMERA_TRACKS: &str = "block_74";

/// A Halo 2 camera track (`trak`) with Halo 1's control points: the
/// same layout, each point's position as it is and its orientation in
/// Halo 2's convention, the conjugate ([`h1_assets::models::h1::h2_rotation`]).
///
/// The engine places the following camera by searching the track for the
/// look pitch (E: `0x0023c980` bisects the track parameter in `[0, 1]`,
/// samples the spline at `0x0023c300` and compares the elevation of the
/// sampled orientation's forward axis, `atan2(z, |xy|)`, with the pitch).
/// The search assumes the elevation rises along the track, as on every
/// retail track (V: -90 degrees at the first point, the camera above
/// looking down, to +90 at the last). Halo 1's stored orientations run the
/// other way (+90 to -90), so taken as they are the search walked off one
/// end of the track and left the camera at its first point, 3.5 above the
/// warthog's marker and 4.7 above the scorpion's, whatever the pitch.
/// (Halo 2's `globals\default_unit_camera_track` is Halo 1's default
/// track copied value for value and runs the same wrong way.)
pub fn camera_track(t: &h1::CameraTrack) -> Result<Block> {
    let d = def(b"trak")?;
    let mut r = defs::default_record(d);
    set(&mut r, "flags", Val::Int(t.flags.into()))?;
    let pd = d.child("control_points").expect("trak control points");
    let mut out = Vec::new();
    for (position, orientation) in &t.points {
        let mut x = defs::default_record(pd);
        set(&mut x, "position", Val::floats(position))?;
        set(
            &mut x,
            "orientation",
            Val::floats(&h1_assets::models::h1::h2_rotation(*orientation)),
        )?;
        out.push(x);
    }
    set(&mut r, "control_points", Val::Block(out))?;
    built(d, &r)
}

/// The key the engine searches a camera track by: the elevation, in
/// radians, of the forward axis `(1, 0, 0)` turned by a control point's
/// orientation `q` (i, j, k, w), `atan2(z, |xy|)` (E: `0x0023c300` turns
/// the axis, `0x0023c980` takes the angle and bisects on it). Rises along
/// every retail track ([`camera_track`]).
pub fn camera_elevation(q: [f32; 4]) -> f32 {
    let [x, y, z, w] = q;
    let fx = 2.0 * (x * x + w * w) - 1.0;
    let fy = 2.0 * (w * z + x * y);
    let fz = 2.0 * (x * z - w * y);
    fz.atan2(fx.hypot(fy))
}

/// Halo 2's names for a Halo 1 seat on a model with `markers` (Halo 1's
/// marker names, which the converted model keeps as they are).
pub fn seat_names(seat: &h1::Seat, markers: &[String]) -> SeatNames {
    // The converted model keeps Halo 1's marker names as they are
    // (`h1_assets::models::mode`), so a seat names them the same way.
    // A name the model lacks is written empty: the engine's marker
    // lookup (`0x000b8d30`) takes an empty name as the object's origin
    // but fails a name it cannot find, and the seat search (`0x000c7160`)
    // then never seats a rider (V: a10's fighterbomber and the dropships
    // name a `driver` their models do not have).
    let found = |n: &str| {
        markers
            .iter()
            .find(|m| m.eq_ignore_ascii_case(n))
            .cloned()
            .unwrap_or_default()
    };
    let enter = format!("{} enter", seat.marker);
    SeatNames {
        label: h1_assets::models::names::mode_name(&seat.label),
        marker: found(&seat.marker),
        enter_marker: if markers.iter().any(|m| m.eq_ignore_ascii_case(&enter)) {
            found(&enter)
        } else {
            String::new()
        },
        camera_marker: found(&seat.camera_marker),
        camera_track: None,
        camera_angles: seat.camera_angles,
        flags: seat.flags,
    }
}

/// `vehi +0x1f2`, the 16 bits after `vehicle_type`: how the driver's
/// stick drives the vehicle, selected by source type. V,
/// every `vehi` on the retail disc: 2 on the scorpion (the one type 0) and
/// its cannon turrets, 0 on every other vehicle of every type, so type 0
/// takes 2 and the rest 0. Proved in a game run (2026-11-19, converted
/// Blood Gulch): the converted scorpion, written with 0 through converter
/// version 75 (the role took 4-byte spans only), crept at 0.05 world
/// units a second with the stick held forward; set to 2 it drove at 2.5.
pub const VEHI_CONTROL: &str = "unknown_1f2";

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

/// `+0x48` of a friction point: the index of its region ([`FRICTION_REGION`])
/// among the model's `hlmt` regions, low half; the high half 0, or
/// `0xffffffff` for no region. V: the retail warthog's four hold 4, 7, 3
/// and 6, its `lf_wheel`, `rf_wheel`, `lb_wheel` and `rb_wheel`; the
/// scorpion's, which name none, `0xffffffff`. Taken from the role as a
/// word, as through converter version 76, the converted warthog's back
/// points held 6 and its front 7 of a model with three regions; the
/// converted warthog then stood on its nose at spawn once its tires were
/// out of its body (game runs of the 77 experiment), as if the front
/// points held nothing up.
pub const FRICTION_REGION_INDEX: &str = "unknown_48";

/// A friction point's flags (`+4`) and the bit its steering wheels hold.
/// V: on the retail warthog the two front points hold `0x26` and the two
/// back ones `0x32`: the front pair alone has bit 2, the back pair alone
/// bit 4 (a hand-brake wheel's). Taken as one majority over all four, as
/// through converter version 75, every converted wheel held the back
/// pair's word and none steered.
pub const FRICTION_FLAGS: &str = "unknown_4";
pub const FRICTION_STEERS: u32 = 1 << 2;

/// A Halo 1 powered mass point as a Halo 2 point: the marker the converted
/// model gains for it and the region it belongs to (empty for none).
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    /// Halo 1 mass-point centre in object space and its collision radius.
    pub sphere: [f32; 4],
    pub antigravity: Option<[f32; 6]>,
    pub marker: String,
    pub region: String,
    /// That region's index among the model's regions (the `hlmt`'s), -1
    /// for none ([`FRICTION_REGION_INDEX`]).
    pub region_index: i16,
    /// Powered group zero steers a jeep forward and drives the first tank tread.
    pub steered: bool,
}

/// `phmo +0xc0`: the physics model's regions, `0xc` each, the name a
/// string ID at `+0`.
pub const PHMO_REGIONS: usize = 0xc0;
pub const PHMO_REGION_SIZE: usize = 0xc;

/// Whether a base vehicle's physics model carries its wheels or treads:
/// most of its friction points ([`VEHI_FRICTION`]) either name no region
/// ([`FRICTION_REGION`] null: the point rides the body) or name a region
/// the `phmo` has ([`PHMO_REGIONS`]). A vehicle with friction points and
/// no `phmo` has none; `None` when it has no friction points.
///
/// V, coagulation: the warthog's four points name the wheel regions
/// `lf_wheel`, `rf_wheel`, `lb_wheel`, `rb_wheel` (the hlmt's regions 4,
/// 7, 3, 6), none of which its `phmo` has (its regions `bumper`, `hull`;
/// its bodies on nodes `hull` and `bumper`, none on a tire node), so no;
/// the scorpion's twelve points name none, and its `phmo` (one region,
/// `hull`; two bodies on no node, node -1) holds tread shapes reaching
/// 1.13 either side where its hull reaches 0.53, so yes. Every friction
/// marker of both hangs from node 0, the hull, so the markers' nodes do
/// not tell wheels from hull. E: `0x205fd1` turns a friction point off
/// once its region is destroyed, so a point with a region is a part that
/// can come off, which neither `phmo` gives a shape.
pub fn member_wheel_shapes(
    base: &crate::base::Base,
    root: &[u8],
    d: &StructDef,
    at: usize,
    e: &StructDef,
) -> Result<Option<bool>> {
    let region_at = field_offset(e, FRICTION_REGION)? as usize;
    let mut regions = Vec::new();
    while let Some(x) = base.element(root, at, e.size as usize, regions.len())? {
        regions.push(crate::base::u32_at(&x, region_at));
    }
    if regions.is_empty() {
        return Ok(None);
    }
    let Some(hlmt) = base.reference(root, path_offset(d, "unit.obje.model")? as usize) else {
        return Ok(Some(false));
    };
    let Some(phmo) = base.reference(&base.root(hlmt)?, crate::convert::h2::HLMT_PHYSICS_MODEL)
    else {
        return Ok(Some(false));
    };
    let p = base.root(phmo)?;
    let mut have = Vec::new();
    while let Some(x) = base.element(&p, PHMO_REGIONS, PHMO_REGION_SIZE, have.len())? {
        have.push(crate::base::u32_at(&x, 0));
    }
    let carried = regions
        .iter()
        .filter(|r| **r == 0 || have.contains(r))
        .count();
    Ok(Some(2 * carried > regions.len()))
}

/// The node (`+0x2`) of every marker of each marker group named in
/// `names` (string IDs of the base), in the render model of the base
/// vehicle whose root is `root`: one
/// list per name, empty when the model has no such group. `None` when the
/// vehicle has no render model.
pub fn marker_nodes(
    base: &crate::base::Base,
    root: &[u8],
    d: &StructDef,
    names: &[u32],
) -> Result<Option<Vec<Vec<u8>>>> {
    let Some(hlmt) = base.reference(root, path_offset(d, "unit.obje.model")? as usize) else {
        return Ok(None);
    };
    let h = base.root(hlmt)?;
    let Some(mode) = base.reference(&h, field_offset(def(b"hlmt")?, "render_model")? as usize)
    else {
        return Ok(None);
    };
    let m = base.root(mode)?;
    let md = def(b"mode")?;
    let gd = md
        .child("marker_groups")
        .ok_or_else(|| Error::Invalid("mode marker_groups".into()))?;
    let markers_def = gd
        .child("markers")
        .ok_or_else(|| Error::Invalid("mode markers".into()))?;
    let groups_at = field_offset(md, "marker_groups")? as usize;
    let (group_name, markers_at) = (
        field_offset(gd, "name")? as usize,
        field_offset(gd, "markers")? as usize,
    );
    let marker_node = field_offset(markers_def, "node")? as usize;
    let mut groups = Vec::new();
    while let Some(g) = base.element(&m, groups_at, gd.size as usize, groups.len())? {
        groups.push(g);
    }
    let mut out = Vec::new();
    for name in names {
        let mut nodes = Vec::new();
        for g in groups
            .iter()
            .filter(|g| crate::base::u32_at(g, group_name) == *name)
        {
            let mut i = 0;
            while let Some(x) = base.element(g, markers_at, markers_def.size as usize, i)? {
                nodes.push(x[marker_node]);
                i += 1;
            }
        }
        out.push(nodes);
    }
    Ok(Some(out))
}

/// A field's offset through embedded structs (`unit.obje.model`).
fn path_offset(d: &StructDef, path: &str) -> Result<u32> {
    let (head, rest) = path
        .split_once('.')
        .map_or((path, None), |(h, r)| (h, Some(r)));
    let f = d
        .fields
        .iter()
        .find(|f| f.name == head)
        .ok_or_else(|| Error::Invalid(format!("{}: no field {head}", d.name)))?;
    match (rest, &f.kind) {
        (None, _) => Ok(f.offset),
        (Some(r), defs::Kind::Struct(s)) => Ok(f.offset + path_offset(s, r)?),
        _ => Err(Error::Invalid(format!("{}: {path}", d.name))),
    }
}

fn field_offset(d: &StructDef, name: &str) -> Result<u32> {
    d.fields
        .iter()
        .find(|f| f.name == name)
        .map(|f| f.offset)
        .ok_or_else(|| Error::Invalid(format!("{}: no field {name}", d.name)))
}

/// A vehicle's bounding sphere from its render model: the centre of the
/// box around every vertex of every geometry (Halo 1 keeps them in model
/// space at the rest pose), the radius the farthest vertex from it.
/// `None` for a model without vertices.
///
/// Halo 1's own sphere is not used for vehicles: every Halo 1 vehicle on
/// the disc centres it on the origin (`+0x08` zero), which is where the
/// wheels or the hull touch the ground, and Halo 2 asks two things of
/// that point (E):
/// - the object's lighting (`0x000d1850`) is the lightmap where a ray
///   from 0.01 above the centre (`obj +0x30`) down 10 units meets the
///   structure (`0x000d2a50`, direction `0x004406e4` (0, 0, -10), start
///   offset `0x0045dc70` 0.001 of it); from a centre under the ground the
///   ray finds no floor and the vehicle is drawn unlit;
/// - the object's location (`0x000b8600`) is the cluster holding the
///   centre (`obj +0x40`), else the one holding the origin (`obj +0x64`);
///   with neither, the object is outside the map (`obj +4` bit 18) and the
///   player's action search (`0x00151110`, objects in the clusters around
///   the player, `0x000bb050`) never offers it.
/// Retail vehicles hold their centre inside the body: the warthog's 0.29
/// above an origin its wheels touch 0.02 below, the ghost's on an origin
/// 0.15 above its belly (`tests/vehicle_ground_disc.rs`).
pub fn model_sphere(model: &h1_assets::models::h1::Model) -> Option<(f32, [f32; 3])> {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    let mut any = false;
    let vertices = || model.geometries.iter().flat_map(|g| &g.parts).flat_map(|p| &p.vertices);
    for v in vertices() {
        any = true;
        for a in 0..3 {
            lo[a] = lo[a].min(v.position[a]);
            hi[a] = hi[a].max(v.position[a]);
        }
    }
    if !any {
        return None;
    }
    let centre: [f32; 3] = std::array::from_fn(|a| (lo[a] + hi[a]) / 2.0);
    let radius = vertices()
        .map(|v| (0..3).map(|a| (v.position[a] - centre[a]).powi(2)).sum::<f32>().sqrt())
        .fold(0.0, f32::max);
    Some((radius, centre))
}

/// What a rebuilt vehicle carries from Halo 1 ([`vehicle`]).
pub struct VehicleSpec<'a> {
    pub object: &'a h1::Object,
    pub source: &'a [u8],
    pub physics: Option<&'a [u8]>,
    pub references: &'a [(String, FourCC, TagId)],
    /// The bounding sphere (radius, centre) written in place of Halo 1's
    /// ([`model_sphere`]); `None` keeps Halo 1's.
    pub bounds: Option<(f32, [f32; 3])>,
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
    pub contacts: Option<&'a h1::GroundContacts>,
    pub seat_power: &'a [[f32; 2]],
    /// Source powered mass points, in source order.
    pub points: &'a [Point],
    /// Halo 1's attachments that convert ([`object_effects`]).
    pub attachments: &'a [AttachmentSpec],
    /// The object's functions, Halo 1's ([`crate::functions`]), which
    /// the attachments' primary scales name.
    pub functions: &'a [crate::functions::FunctionSpec],
    /// Halo 1's creation effect, converted.
    pub creation_effect: Option<TagId>,
}

/// The loader at 0x205550 sets vtables, pointer arrays and child indices
/// from +0x38. Each of the four inline shapes holds up to eight spheres.
fn write_vehicle_phantom(record: &mut Record, points: &[Point]) -> Result<()> {
    let mut write = |at: usize, bits: u32| set(record, &format!("unknown_{at:x}"), Val::Bytes(bits.to_le_bytes().to_vec()));
    if points.len() > 32 {
        return Err(Error::Invalid(format!("vehicle has {} wheel spheres; phantom capacity is 32", points.len())));
    }
    let groups = points.chunks(PHANTOM_MAX_SPHERES as usize);
    write(0x38, groups.len() as u32)?;
    write(0x3c, 3)?;
    let mut bounds = [[f32::INFINITY, f32::NEG_INFINITY]; 3];
    for (i, group) in groups.enumerate() {
        let start = 0x60 + i * 0x90;
        write(start + 0xc, group.len() as u32)?;
        for (j, point) in group.iter().enumerate() {
            let [x, y, z, radius] = point.sphere;
            for (axis, pos) in [x, y, z].into_iter().enumerate() {
                bounds[axis][0] = bounds[axis][0].min(pos - radius);
                bounds[axis][1] = bounds[axis][1].max(pos + radius);
            }
            for (k, value) in point.sphere.iter().enumerate() {
                write(start + 0x10 + j * 0x10 + k * 4, value.to_bits())?;
            }
        }
    }
    for (axis, [lo, hi]) in bounds.into_iter().enumerate() {
        write(0x48 + axis * 8, lo.to_bits())?;
        write(0x4c + axis * 8, hi.to_bits())?;
    }
    Ok(())
}

fn write_ground_contacts(
    record: &mut Record,
    contacts: &h1::GroundContacts,
    points: &[Point],
) -> Result<()> {
    use tags::object::direct_drive::*;
    if contacts.wheels.len() != points.len() {
        return Err(Error::Invalid(
            "powered mass points and friction markers differ".into(),
        ));
    }
    let required = CONTACT_DATA as usize
        + CONTACT_HEADER as usize
        + points.len() * WHEEL_BYTES as usize
        + contacts.surfaces.len() * SURFACE_BYTES as usize;
    if required > CONTACT_END as usize {
        return Err(Error::Invalid(format!(
            "wheel and material data exceed the vehicle phantom by {} bytes",
            required - CONTACT_END as usize
        )));
    }
    let mut at = CONTACT_DATA;
    let mut word = |value: u32| {
        let field = format!("unknown_{at:x}");
        at += 4;
        set(record, &field, Val::Bytes(value.to_le_bytes().to_vec()))
    };
    word(CONTACT_MAGIC)?;
    word(
        points.len() as u32
            | ((contacts.surfaces.len() as u32) << 8)
            | (u32::from(contacts.equal_slopes) << 16),
    )?;
    for ((marker, w), point) in contacts.wheels.iter().zip(points) {
        if marker != &point.marker {
            return Err(Error::Invalid(format!(
                "wheel {marker} does not match friction marker {}",
                point.marker
            )));
        }
        for bits in [
            w.mode,
            w.rate.to_bits(),
            w.parallel.to_bits(),
            w.perpendicular.to_bits(),
        ] {
            word(bits)?;
        }
    }
    for &(index, factors) in &contacts.surfaces {
        word(u32::from(index))?;
        for value in factors {
            word(value.to_bits())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod phantom_tests {
    use super::*;

    #[test]
    fn excess_ground_material_rows_report_an_error_before_encoding() {
        use tags::object::direct_drive::WheelDrag;
        let d = def(b"vehi").unwrap().child(VEHI_PHYSICS).unwrap();
        let mut record = defs::default_record(d);
        let point = Point {
            sphere: [0.0; 4],
            antigravity: None,
            marker: "tire".into(),
            region: String::new(),
            region_index: -1,
            steered: false,
        };
        let contacts = h1::GroundContacts {
            wheels: vec![(
                point.marker.clone(),
                WheelDrag {
                    mode: 1,
                    rate: 1.0,
                    parallel: 0.5,
                    perpendicular: 0.5,
                },
            )],
            surfaces: (0..11).map(|i| (i, [0.5; 5])).collect(),
            equal_slopes: false,
        };
        let before = record.clone();
        assert!(write_ground_contacts(&mut record, &contacts, &[point]).is_err());
        assert_eq!(record, before);
    }

    #[test]
    fn wheel_spheres_span_inline_shapes_in_source_order() {
        let d = def(b"vehi").unwrap().child(VEHI_PHYSICS).unwrap();
        let mut r = defs::default_record(d);
        let mut points: Vec<_> = (0..9).map(|i| Point {
            sphere: [i as f32, 1.0, -2.0, 0.5],
            antigravity: None,
            marker: format!("wheel_{i}"),
            region: String::new(),
            region_index: -1,
            steered: false,
        }).collect();
        write_vehicle_phantom(&mut r, &points).unwrap();
        let word = |at| {
            let Some(Val::Bytes(b)) = r.get(&format!("unknown_{at:x}")) else { panic!("phantom +{at:x}") };
            u32::from_le_bytes(b.as_slice().try_into().unwrap())
        };
        assert_eq!((word(0x38), word(0x6c), word(0xfc)), (2, 8, 1));
        assert_eq!((word(0x48), word(0x4c)), ((-0.5f32).to_bits(), 8.5f32.to_bits()));
        assert_eq!((word(0x100), word(0x10c)), (8f32.to_bits(), 0.5f32.to_bits()));
        let before = r.clone();
        points.resize(33, points[0].clone());
        assert!(write_vehicle_phantom(&mut r, &points).is_err());
        assert_eq!(r, before);
    }
}

/// Builds the vehicle from its Halo 1 drive and contact data, with one
/// seat per source seat and one contact per powered mass point.
pub fn vehicle(v: &VehicleSpec) -> Result<Block> {
    vehicle_with_tuning(v, None)
}

pub(crate) fn vehicle_with_tuning(v: &VehicleSpec, flight: Option<[f32; 11]>) -> Result<Block> {
    let d = def(b"vehi")?;
    let mut r = defs::default_record(d);
    use tags::object::direct_drive::GRAVITY_RATIO;
    let contacts = v.contacts;
    let power = v.seat_power;
    if v.points
        .iter()
        .any(|p| p.antigravity.is_none() != contacts.is_some())
    {
        return Err(Error::Invalid(
            "powered point lacks its source ground or antigravity law".into(),
        ));
    }
    // The H2 AI compares both vehicle ranks (+244, 0x1ab48a). The
    // source drive decisions use distances (+384/+388, 0x16f954 and
    // 0x16fbc1), with no ordering of vehicle definitions in that test.
    // Equal ranks avoid introducing a vehicle-class preference.
    // H1 0x394a1 stops attached vehicles. H2 0xe538c can instead push
    // a parented vehicle via its rider. Disable both additional impulses.
    for field in ["unknown_244", "unknown_274", "unknown_278"] {
        set(&mut r, field, Val::Bytes(0u32.to_le_bytes().to_vec()))?;
    }
    // H1 0x394be writes the crouch bit in the same tick; H2 0xf045b
    // turns +25c into a per-tick step by multiplying by tick length.
    set(
        &mut r,
        "unknown_25c",
        Val::Bytes(30f32.to_le_bytes().to_vec()),
    )?;
    let powered_groups = if let Some(physics) = v.physics {
        if physics.len() < h1::PHYS_SIZE {
            return Err(Error::Truncated("vehicle physics"));
        }
        // H1 0x83df4 and H2 0x2069b6 both compute gravity * penetration /
        // depth minus damping * normal velocity. H1 integrates per tick.
        // Both slope responses interpolate on the contact normal's z
        // component (0x83ea2, 0x206f96). Gravity is a dimensionless
        // multiplier at 0x8399d. Scale depth so g/depth agrees.
        for (field, source, factor) in [
            ("unknown_2b4", 0x24, 1.0 / GRAVITY_RATIO),
            ("unknown_2b8", 0x28, 30.0),
            ("unknown_2c0", 0x2c, 1.0),
            ("unknown_2e0", 0x1c, 1.0),
        ] {
            let value = f32::from_bits(crate::base::u32_at(physics, source)) * factor;
            set(&mut r, field, Val::Bytes(value.to_le_bytes().to_vec()))?;
        }
        let full = f32::from_bits(crate::base::u32_at(physics, 0x2c));
        let cutoff = f32::from_bits(crate::base::u32_at(physics, 0x30));
        // Equal endpoints in H1's 0x123b4 give full grip at or below
        // the endpoint. H2 0x206fad divides by the difference. Adjacent
        // floats express the same step for every representable normal.
        let cutoff = if cutoff == full { full.next_up() } else { cutoff };
        set(&mut r, "unknown_2c4", Val::Bytes(cutoff.to_le_bytes().to_vec()))?;
        crate::base::u32_at(physics, 0x68)
    } else {
        0
    };
    let mut phantom = defs::default_record(d.child(VEHI_PHYSICS).expect("vehicle phantom"));
    if let Some(contacts) = contacts {
        if v.points.len() > 16 {
            return Err(Error::Invalid(
                "vehicle exceeds the 16 friction contacts at 0x20678d".into(),
            ));
        }
        write_vehicle_phantom(&mut phantom, v.points)?;
        write_ground_contacts(&mut phantom, contacts, v.points)?;
    } else {
        role(
            &mut phantom,
            &format!("{VEHI_PHYSICS}[0]."),
            TURRET_PHYSICS_ROLE,
        )?;
    }
    if matches!(v.vehicle_type, 3 | 5)
        && let Some(physics) = v.physics
    {
        use tags::object::h1_flight::*;
        let source = |at| f32::from_bits(crate::base::u32_at(physics, at));
        let vehicle = |at: usize| -> Result<f32> {
            let b = v
                .source
                .get(at..at + 4)
                .ok_or(Error::Truncated("aircraft controller"))?;
            Ok(f32::from_le_bytes(b.try_into().unwrap()))
        };
        let radius = source(0);
        let kind = if v.vehicle_type == 3 {
            DROPSHIP
        } else if powered_groups != 2 {
            PASSIVE
        } else if radius > 0.0 {
            LIFEPOD
        } else {
            BANSHEE
        };
        if contacts.is_some() || !(source(8).is_finite() && source(8) > 0.0) {
            return Err(Error::Invalid(
                "unsupported aircraft contact layout or mass".into(),
            ));
        }
        let inertia = (source(0x50) + source(0x54) + source(0x58)) / (3.0 * source(8));
        let params = [
            MAGIC,
            kind,
            radius.to_bits(),
            inertia.to_bits(),
            vehicle(0x364)?.to_bits(),
            vehicle(0x308)?.to_bits(),
            vehicle(0x314)?.to_bits(),
        ];
        for (i, word) in params.into_iter().enumerate() {
            set(
                &mut phantom,
                &format!("unknown_{:x}", DATA + i as u32 * 4),
                Val::Bytes(word.to_le_bytes().to_vec()),
            )?;
        }
    }
    if matches!(v.vehicle_type, 3 | 5) && v.physics.is_none() {
        // H1 0x398df..0x398ea skips the flight dispatch for a null phys.
        use tags::object::h1_flight::{DATA, MAGIC, PASSIVE};
        for (offset, value) in [(DATA, MAGIC), (DATA + 4, PASSIVE)] {
            set(
                &mut phantom,
                &format!("unknown_{offset:x}"),
                Val::Bytes(value.to_le_bytes().to_vec()),
            )?;
        }
    }
    if !power.is_empty() || v.physics.is_some() {
        use tags::object::direct_drive::{HAS_SOURCE_GRAVITY, POWER_DATA, POWER_MAGIC};
        if power.len() > 2 {
            return Err(Error::Invalid(format!(
                "{} powered seats exceed the two unit power channels",
                power.len()
            )));
        }
        let flags = if v.physics.is_some() {
            HAS_SOURCE_GRAVITY
        } else {
            0
        };
        for (i, word) in [POWER_MAGIC, power.len() as u32 | flags]
            .into_iter()
            .chain(power.iter().flatten().map(|v| v.to_bits()))
            .enumerate()
        {
            set(
                &mut phantom,
                &format!("unknown_{:x}", POWER_DATA + 4 * i as u32),
                Val::Bytes(word.to_le_bytes().to_vec()),
            )?;
        }
    }
    set(&mut r, VEHI_PHYSICS, Val::Block(vec![phantom]))?;
    for &(path, at) in UNIT_WORDS {
        if let Some(bytes) = v.source.get(at..at + 4) {
            set(&mut r, path, Val::Bytes(bytes.to_vec()))?;
        }
    }
    // H1 cache speeds are in units/tick and accelerations in units/tick².
    for (path, at, scale) in [
        ("unknown_1f4", 0x2f8, 30.0), ("unknown_1f8", 0x2fc, 30.0),
        ("unknown_1fc", 0x300, 900.0), ("unknown_200", 0x304, 900.0),
        ("unknown_204", 0x308, 1.0), ("unknown_208", 0x30c, 1.0),
        ("unknown_20c", 0x310, 1.0), ("unknown_210", 0x314, 1.0),
        ("unknown_214", 0x318, 30.0),
        ("unknown_22c", 0x330, 30.0), ("unknown_230", 0x334, 30.0),
        ("unknown_234", 0x338, 900.0), ("unknown_238", 0x33c, 900.0),
        ("unknown_23c", 0x340, 30.0), ("unknown_240", 0x344, 30.0),
    ] {
        if let Some(bytes) = v.source.get(at..at + 4) {
            let value = f32::from_le_bytes(bytes.try_into().expect("four bytes")) * scale;
            set(&mut r, path, Val::Bytes(value.to_le_bytes().to_vec()))?;
        }
    }
    // The H1 hover solver (0x38152..0x38232) uses the same speed
    // and acceleration in either direction on both horizontal axes.
    // H2 selects four directional cap/rate pairs at 0xf32f5/0xf3562.
    if v.vehicle_type == 4 {
        for (source, scale, paths) in [
            (0x2f8, 30.0, ["unknown_1f8", "unknown_22c", "unknown_230"]),
            (0x300, 900.0, ["unknown_200", "unknown_234", "unknown_238"]),
        ] {
            if let Some(bytes) = v.source.get(source..source + 4) {
                let value = f32::from_le_bytes(bytes.try_into().unwrap()) * scale;
                for path in paths {
                    set(&mut r, path, Val::Bytes(value.to_le_bytes().to_vec()))?;
                }
            }
        }
    }
    if v.vehicle_type == 0 {
        // H1 differential turn speed at full steering is the forward cap
        // (0x3988b..0x398d6). H2 normalizes it by +0x22c at 0xf1d5b.
        if let Some(bytes) = v.source.get(0x2f8..0x2fc) {
            let cap = f32::from_le_bytes(bytes.try_into().unwrap()) * 30.0;
            set(&mut r, "unknown_22c", Val::Bytes(cap.to_le_bytes().to_vec()))?;
        }
    }
    if let Some(flight) = flight {
        for (field, value) in [
            "unknown_1f4",
            "unknown_1f8",
            "unknown_1fc",
            "unknown_200",
            "unknown_204",
            "unknown_208",
            "unknown_210",
            "unknown_22c",
            "unknown_230",
            "unknown_234",
            "unknown_238",
        ]
        .into_iter()
        .zip(flight)
        {
            set(&mut r, field, Val::Bytes(value.to_le_bytes().to_vec()))?;
        }
    }
    crate::arsenal::refs(&mut r, v.references)?;
    let (obj, hlmt, predicted) = (v.object, v.hlmt, v.predicted);
    set(
        &mut r,
        "unit.camera_field_of_view",
        Val::Float(v.camera_field_of_view),
    )?;
    object_tuning(&mut r, "unit.obje", obj)?;
    set(&mut r, "unit.obje.object_type", Val::Enum(VEHICLE_TYPE))?;
    let (radius, centre) = v.bounds.unwrap_or((obj.bounding_radius, obj.bounding_offset));
    set(&mut r, "unit.obje.bounding_radius", Val::Float(radius))?;
    set(&mut r, "unit.obje.bounding_offset", Val::floats(&centre))?;
    set(
        &mut r,
        "unit.obje.model",
        Val::tag(FourCC(*b"hlmt"), hlmt.unwrap_or(TagId::NULL)),
    )?;
    set(&mut r, "vehicle_type", Val::Enum(v.vehicle_type.into()))?;
    // 0xef747 selects the differential controller for mode 2.
    let control: u16 = if v.vehicle_type == 0 { 2 } else { 0 };
    set(&mut r, VEHI_CONTROL, Val::Bytes(control.to_le_bytes().to_vec()))?;
    set(
        &mut r,
        "vehicle_flags",
        Val::Int(i64::from(v.flags & 0xffff)),
    )?;
    predicted_resources(b"vehi", &mut r, "unit.obje", predicted)?;
    object_change_colors(b"vehi", &mut r, "unit.obje", &obj.change_colours)?;
    object_effects(
        b"vehi",
        &mut r,
        "unit.obje",
        v.attachments,
        v.creation_effect,
    )?;
    object_functions(b"vehi", &mut r, "unit.obje", v.functions)?;
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
    for (i, s) in v.seats.iter().enumerate() {
        let scope = format!("{VEHI_SEATS}[{i}].");
        let mut e = defs::default_record(seat_def);
        role(&mut e, &scope, TURRET_SEAT_ROLE)?;
        set(&mut e, SEAT_FLAGS, Val::Bytes(seat_flags(s.flags, 0).to_le_bytes().to_vec()))?;
        // Halo 1's entry markers face the vehicle's forward, not the way a
        // rider comes from, so no facing test applies to them.
        set(
            &mut e,
            SEAT_ENTRY_FACING,
            Val::Bytes(ANY_FACING.to_bits().to_le_bytes().to_vec()),
        )?;
        for (path, bits) in ["unknown_68", "unknown_6c", "unknown_70", "unknown_88", "unknown_8c"]
            .into_iter().zip(s.camera_angles) {
            set(&mut e, path, Val::Bytes(bits.to_le_bytes().to_vec()))?;
        }
        set(&mut e, "label", Val::text(&s.label))?;
        // V: Halo 2's seats hold the rider's marker at +8 (`driver`,
        // `gunner`), the entry marker at +0xc (`driver_enter`) and the
        // camera marker at +0x60 (`banshee_d_camera`), each a marker of the
        // vehicle's model.
        set(&mut e, "string_8", Val::text(&s.marker))?;
        set(&mut e, "string_c", Val::text(&s.enter_marker))?;
        set(&mut e, "string_60", Val::text(&s.camera_marker))?;
        if let Some(t) = s.camera_track {
            let td = seat_def.child(SEAT_CAMERA_TRACKS).expect("seat camera tracks");
            let mut x = defs::default_record(td);
            set(&mut x, "ref_0", Val::tag(FourCC(*b"trak"), t))?;
            set(&mut e, SEAT_CAMERA_TRACKS, Val::Block(vec![x]))?;
        }
        out.push(e);
    }
    set(&mut r, VEHI_SEATS, Val::Block(out))?;
    if !v.points.is_empty() {
        let (block, marker, region) = if contacts.is_some() {
            (VEHI_FRICTION, FRICTION_MARKER, FRICTION_REGION)
        } else {
            (VEHI_HOVER, HOVER_MARKER, HOVER_REGION)
        };
        let e = d.child(block).expect("vehicle points");
        let mut out = Vec::new();
        for p in v.points {
            let mut x = defs::default_record(e);
            if block == VEHI_HOVER
                && let Some(lift) = p.antigravity
            {
                // 0x84376/0x20634c measure height above the point radius.
                // Both lift laws subtract damping * normal velocity and
                // interpolate on the same two slope cosines (0x84418,
                // 0x84477, 0x206420, 0x206550). H1 has fixed lift height;
                // the H2 bank modifiers +0x2d8/+0x2dc stay zero.
                // The H2 solver multiplies both spring and damping by
                // strength at 0x20655e. Preserve their independent gains.
                let [strength, offset, height, damping, full, cutoff] = lift;
                for (field, value) in [
                    ("unknown_8", strength * GRAVITY_RATIO),
                    ("unknown_c", offset),
                    ("unknown_10", height),
                    ("unknown_14", damping * 30.0 / GRAVITY_RATIO),
                    ("unknown_18", full),
                    ("unknown_1c", cutoff),
                    ("unknown_20", p.sphere[3]),
                ] {
                    set(&mut x, field, Val::Bytes(value.to_le_bytes().to_vec()))?;
                }
            }
            set(&mut x, marker, Val::text(&p.marker))?;
            set(&mut x, region, Val::text(&p.region))?;
            if block == VEHI_FRICTION {
                if matches!(v.vehicle_type, 0 | 1) {
                    let mut flags = 2u32;
                    if v.vehicle_type == 1 {
                        // H1 0x3925c..0x392af gives the two powered
                        // groups opposite wheel quaternions. H2 tests
                        // bits 2 and 3 at 0xf20f7/0xf2178.
                        flags |= if p.steered { FRICTION_STEERS } else { 8 };
                    } else if !p.steered {
                        flags |= tags::object::direct_drive::H1_SECOND_TRACK;
                    }
                    set(
                        &mut x,
                        FRICTION_FLAGS,
                        Val::Bytes(flags.to_le_bytes().to_vec()),
                    )?;
                }
                set(
                    &mut x,
                    "unknown_c",
                    Val::Bytes(p.sphere[3].to_le_bytes().to_vec()),
                )?;
                let index: u32 = if p.region_index >= 0 {
                    p.region_index as u32
                } else {
                    u32::MAX
                };
                set(
                    &mut x,
                    FRICTION_REGION_INDEX,
                    Val::Bytes(index.to_le_bytes().to_vec()),
                )?;
            }
            out.push(x);
        }
        set(&mut r, block, Val::Block(out))?;
    }
    // H1 passes drive commands to the solver only for two powered groups
    // (jeep 0x39256, tank 0x393dc). Other layouts coast passively.
    if matches!(v.vehicle_type, 0 | 1) {
        // The H2 wheel loops index gear 0 for reverse and 1 for forward.
        // H1 drives both directly in world units, with no engine inertia.
        let gear = d.child(VEHI_GEARS).expect("vehicle gear");
        let mut directions = Vec::new();
        // Even a passive tank selects gear 0 or 1 at 0xef40d. It still
        // needs two valid records when 0xf152c reads the selected ratio.
        let ratios = if powered_groups == 2 { [-1.0, 1.0] } else { [0.0; 2] };
        for ratio in ratios {
            let mut e = defs::default_record(gear);
            set(&mut e, "speed_ratio", Val::Float(ratio))?;
            directions.push(e);
        }
        set(&mut r, VEHI_GEARS, Val::Block(directions))?;
        if powered_groups == 2 {
            let mode = tags::object::direct_drive::H1_DIRECT_DRIVE.to_le_bytes();
            set(&mut r, "unknown_260", Val::Bytes(mode.to_vec()))?;
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
/// (`devi` in `crates/formats/tags/src/defs/devi.txt`). A zero transition time is
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
    object_tuning(&mut r, "devi.obje", obj)?;
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
    object_change_colors(group, &mut r, "devi.obje", &obj.change_colours)?;
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
    let i = 0.4 * mass * radius * radius;
    compound_sphere_physics(
        &[SphereSpec {
            centre,
            radius,
            mass,
        }],
        mass,
        MassProperties {
            centre,
            inertia: [[i, 0.0, 0.0], [0.0, i, 0.0], [0.0, 0.0, i]],
        },
        material,
    )
}

/// One source sphere attached to an object-space rigid body.
pub(crate) struct SphereSpec {
    pub centre: [f32; 3],
    pub radius: f32,
    pub mass: f32,
}

#[derive(Clone, Copy)]
pub(crate) struct MassProperties {
    pub centre: [f32; 3],
    pub inertia: [[f32; 3]; 3],
}

pub(crate) fn compound_sphere_physics(
    spheres: &[SphereSpec],
    mass: f32,
    properties: MassProperties,
    material: &str,
) -> Result<Block> {
    if spheres.is_empty() {
        return Err(Error::Invalid(
            "a sphere body has no positive-radius source mass points".into(),
        ));
    }
    let d = def(b"phmo")?;
    let mut r = defs::default_record(d);
    let f = |v: f32| Val::Bytes(v.to_le_bytes().to_vec());
    let u16_hi = |v: u16| Val::Bytes([0, 0, v as u8, (v >> 8) as u8].to_vec());
    set(&mut r, "mass", Val::Float(mass))?;
    set(&mut r, "unknown_8", f(1.0))?;
    set(&mut r, "unknown_c", f(1.0))?;

    let extent = |axis: usize, sign: f32| {
        spheres
            .iter()
            .map(|s| sign * s.centre[axis] + s.radius)
            .fold(f32::NEG_INFINITY, f32::max)
    };
    let (centre, radius) = if let [sphere] = spheres {
        (sphere.centre, sphere.radius)
    } else {
        let centre = std::array::from_fn(|axis| 0.5 * (extent(axis, 1.0) - extent(axis, -1.0)));
        let radius = spheres
            .iter()
            .map(|s| {
                let distance_squared: f32 = s
                    .centre
                    .iter()
                    .zip(centre)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum();
                distance_squared.sqrt() + s.radius
            })
            .fold(0.0, f32::max);
        (centre, radius)
    };
    let rb_def = d.child("rigid_bodies").expect("phmo rigid bodies");
    let mut rb = defs::default_record(rb_def);
    set(&mut rb, "node", Val::Int(-1))?;
    set(&mut rb, "bounding_sphere_offset", Val::floats(&centre))?;
    set(&mut rb, "bounding_sphere_radius", Val::Float(radius))?;
    let (motion, class) = if spheres.len() == 1 { (1, 4) } else { (2, 6) };
    set(&mut rb, "unknown_18", u16_hi(motion))?;
    set(&mut rb, "unknown_1c", u16_hi(class))?;
    set(&mut rb, "unknown_20", f(1.0))?;
    set(&mut rb, "unknown_28", f(0.05))?;
    let shape = if spheres.len() == 1 { 0 } else { 0xe };
    set(&mut rb, "shape_type", Val::Int(shape))?;
    set(&mut rb, "shape_index", Val::Int(0))?;
    set(&mut rb, "mass", Val::Float(mass))?;
    set(&mut rb, "center_of_mass", Val::floats(&properties.centre))?;
    for (field, row) in ["inertia_tensor_x", "inertia_tensor_y", "inertia_tensor_z"]
        .into_iter()
        .zip(properties.inertia)
    {
        set(&mut rb, field, Val::floats(&row))?;
    }
    set(&mut r, "rigid_bodies", Val::Block(vec![rb]))?;

    let mat_def = d.child("materials").expect("phmo materials");
    let mut m = defs::default_record(mat_def);
    set(&mut m, "name", Val::text(material))?;
    set(&mut m, "string_4", Val::text(material))?;
    set(&mut m, "unknown_8", Val::Bytes(vec![0xff, 0xff, 0, 0]))?;
    set(&mut r, "materials", Val::Block(vec![m]))?;

    let sphere_def = d.child("spheres").expect("phmo spheres");
    let mut records = Vec::with_capacity(spheres.len());
    for &SphereSpec {
        centre,
        radius,
        mass,
    } in spheres
    {
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
        records.push(s);
    }
    set(&mut r, "spheres", Val::Block(records))?;

    if spheres.len() > 1 {
        let list_def = d.child("lists").expect("phmo lists");
        let mut list = defs::default_record(list_def);
        set(&mut list, "unknown_4", Val::Bytes(vec![0, 0, 0x80, 0]))?;
        set(&mut list, "child_count", Val::Int(spheres.len() as i64))?;
        for i in 0..4 {
            set(&mut list, &format!("inline_child_type_{i}"), Val::Int(0))?;
            let index = if spheres.len() <= 4 && i < spheres.len() {
                i
            } else {
                0xffff
            };
            set(
                &mut list,
                &format!("inline_child_index_{i}"),
                Val::Int(index as i64),
            )?;
        }
        set(
            &mut list,
            "child_capacity",
            Val::Int(0x8000_0000 | spheres.len().max(4) as i64),
        )?;
        if spheres.len() > 4 {
            let child_def = d.child("list_children").expect("phmo list children");
            let mut children = Vec::with_capacity(spheres.len());
            for i in 0..spheres.len() {
                let mut child = defs::default_record(child_def);
                set(&mut child, "shape_type", Val::Int(0))?;
                set(&mut child, "shape_index", Val::Int(i as i64))?;
                children.push(child);
            }
            set(&mut r, "list_children", Val::Block(children))?;
        }
        set(&mut r, "lists", Val::Block(vec![list]))?;
    }

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

/// The words of a built rigid body the engine reads to add it to the
/// Havok world (docs/formats/collision.md, "Rigid bodies").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyWords {
    /// `+0x18`. Bit 1 puts the body on collision layer 2, which collides
    /// with layers 0 to 2 only (E: `0x001d5bf0` passes it to
    /// `0x001d70d0`, which takes layer 2 over the default; the layer pairs
    /// are disabled at `0x001c302d`). Bit 0 is read at `0x001d5d81`; its
    /// effect is not established.
    pub flags: u16,
    /// `+0x1a`, the motion: 0 to 5 are Havok motions 2 to 7 (E: the table
    /// at `0x001d5bd4`); 2 is box inertia.
    pub motion: u16,
    /// `+0x1e`, the body's size class: V, every rigid
    /// body of every `phmo` on coagulation, beavercreek and
    /// 05a_deltaapproach, it rises with the body's bounding sphere radius
    /// (`+0x14`): 3 from 0.30 to 0.70, 4 from 0.73 to 1.23, 5 from 1.39 to
    /// 1.96, 6 from 2.13 (1 and 2 overlap below 0.48). E: `0x001d5ec0`
    /// takes the largest over an object's bodies (`0x001d63fd`) and
    /// `0x001d69d0` picks a creature's collision layer by it (`0x001d6a32`:
    /// up to 1 layer 4, 5 and over layer 5, else 3).
    pub word_1e: u16,
}

/// A built device's body words (V: the disc's polyhedron physics models,
/// `high_small_door`, `ghost`, `pelican`).
pub const DEVICE_BODY: BodyWords = BodyWords {
    flags: 0,
    motion: 2,
    word_1e: 5,
};
/// A built biped's body words. V: every rigid body of every retail biped
/// physics model on the base holds motion 2 and `+0x1e` 1, and every body
/// of the Chief's and the Covenant's (`masterchief`, `elite`, `brute`,
/// `hunter`, `jackal`, `bugger`) flags 3 (`tests/unit_rebuild_disc.rs`,
/// `tests/biped_physics_disc.rs`). Without bit 1 a living biped's bodies
/// sit on layer 1 when fixed or 9 or 11 when keyframed (E: `0x001d69d0`
/// and the table at `0x001d6ab0`), which collide with most other layers.
pub const BIPED_BODY: BodyWords = BodyWords {
    flags: 3,
    motion: 2,
    word_1e: 1,
};

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
/// - rigid body `+0x18`, `+0x1a` and `+0x1e` as `words` ([`DEVICE_BODY`],
///   [`BIPED_BODY`]), `+0x20` 1.0, `+0x28` 0.05.
/// - node `+4` flags (0), `+6` parent, `+8` next sibling, `+0xa` first
///   child, each `-1` none.
///
/// Every runtime word (shape vtables and pointers) stays zero: the engine
/// writes them at load (`0x001ea9c0`).
pub fn physics_model(
    bodies: &[Body],
    nodes: &[PhysNode],
    words: BodyWords,
    joints: &[crate::ragdoll::Constraint],
) -> Result<Block> {
    physics_model_with_mass(bodies, nodes, words, joints, None)
}

pub(crate) fn physics_model_with_mass(
    bodies: &[Body],
    nodes: &[PhysNode],
    words: BodyWords,
    joints: &[crate::ragdoll::Constraint],
    source: Option<MassProperties>,
) -> Result<Block> {
    debug_assert!(source.is_none() || bodies.len() == 1);
    let d = def(b"phmo")?;
    let mut r = defs::default_record(d);
    let f = |v: f32| Val::Bytes(v.to_le_bytes().to_vec());
    let u16_pair = |lo: u16, hi: u16| Val::Bytes([lo.to_le_bytes(), hi.to_le_bytes()].concat());
    let u16_hi = |v: u16| u16_pair(0, v);
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
        let (com, inertia) = source.map_or((com, inertia), |s| (s.centre, s.inertia));
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
        set(&mut rb, "unknown_18", u16_pair(words.flags, words.motion))?;
        set(&mut rb, "unknown_1c", u16_hi(words.word_1e))?;
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
        let ns = nodes
            .iter()
            .map(|n| {
                let mut e = defs::default_record(node_def);
                set(&mut e, "name", Val::text(&n.name))?;
                set(&mut e, "flags", Val::Int(0))?;
                set(&mut e, "parent", Val::Int(n.parent.into()))?;
                set(&mut e, "next_sibling", Val::Int(n.next_sibling.into()))?;
                set(&mut e, "first_child", Val::Int(n.first_child.into()))?;
                Ok(e)
            })
            .collect::<Result<Vec<_>>>()?;
        set(&mut r, "nodes", Val::Block(ns))?;
    }
    ragdoll(d, &mut r, joints)?;
    built(d, &r)
}

/// The friction every built ragdoll edge carries (`node_edges` constraint
/// `+8`, which the engine uses in place of the constraint's own when not
/// zero, E `0x001d796f`). V: 0.25 on every edge of the retail Chief's and
/// Elite's ragdolls.
pub const JOINT_FRICTION: f32 = 0.25;

/// Writes `joints` into `r`: each a ragdoll or limited-hinge constraint
/// (constraint types 2 and 1, E `0x001eb1b0`) with its own node edge
/// naming it, as retail's ragdolls are laid out (V: one edge per
/// constraint, the edge's nodes the constraint's, `+0x70` the edge).
fn ragdoll(d: &StructDef, r: &mut Record, joints: &[crate::ragdoll::Constraint]) -> Result<()> {
    use crate::ragdoll::{Frame, Joint};
    if joints.is_empty() {
        return Ok(());
    }
    let child = |p: &str| {
        d.child(p)
            .ok_or_else(|| Error::Invalid(format!("phmo {p}")))
    };
    let (rag_def, hinge_def) = (child("ragdoll_constraints")?, child("limited_hinge_constraints")?);
    let (edge_def, ref_def) = (child("node_edges")?, child("node_edges.constraints")?);
    let frame = |e: &mut Record, at: &str, f: &Frame| -> Result<()> {
        set(e, &format!("header.{at}.unknown_0"), Val::Bytes(1.0f32.to_le_bytes().to_vec()))?;
        set(e, &format!("header.{at}.forward"), Val::floats(&f.forward))?;
        set(e, &format!("header.{at}.left"), Val::floats(&f.left))?;
        set(e, &format!("header.{at}.up"), Val::floats(&f.up))?;
        set(e, &format!("header.{at}.position"), Val::floats(&f.position))
    };
    let (mut rags, mut hinges, mut edges) = (vec![], vec![], vec![]);
    for c in joints {
        let (kind, def, list) = match c.joint {
            Joint::Ragdoll { .. } => (2, rag_def, &mut rags),
            Joint::Hinge { .. } => (1, hinge_def, &mut hinges),
        };
        let mut e = defs::default_record(def);
        set(&mut e, "header.name", Val::text(&format!("{:?}", c.class).to_lowercase()))?;
        set(&mut e, "header.node_a", Val::Int(c.node_a.into()))?;
        set(&mut e, "header.node_b", Val::Int(c.node_b.into()))?;
        frame(&mut e, "frame_a", &c.frame_a)?;
        frame(&mut e, "frame_b", &c.frame_b)?;
        set(&mut e, "header.edge", Val::Int(edges.len() as i64))?;
        match c.joint {
            Joint::Ragdoll { twist, cone, plane } => {
                set(&mut e, "min_twist", Val::Float(twist.0))?;
                set(&mut e, "max_twist", Val::Float(twist.1))?;
                set(&mut e, "min_cone", Val::Float(-cone))?;
                set(&mut e, "max_cone", Val::Float(cone))?;
                set(&mut e, "min_plane", Val::Float(plane.0))?;
                set(&mut e, "max_plane", Val::Float(plane.1))?;
            }
            Joint::Hinge { min, max } => {
                set(&mut e, "min_angle", Val::Float(min))?;
                set(&mut e, "max_angle", Val::Float(max))?;
            }
        }
        let mut cref = defs::default_record(ref_def);
        set(&mut cref, "constraint_type", Val::Int(kind))?;
        set(&mut cref, "constraint_index", Val::Int(list.len() as i64))?;
        set(&mut cref, "friction", Val::Float(JOINT_FRICTION))?;
        let mut edge = defs::default_record(edge_def);
        set(&mut edge, "node_a", Val::Int(c.node_a.into()))?;
        set(&mut edge, "node_b", Val::Int(c.node_b.into()))?;
        set(&mut edge, "constraints", Val::Block(vec![cref]))?;
        edges.push(edge);
        list.push(e);
    }
    set(r, "ragdoll_constraints", Val::Block(rags))?;
    set(r, "limited_hinge_constraints", Val::Block(hinges))?;
    set(r, "node_edges", Val::Block(edges))
}

/// `obje +0` for bipeds (docs/formats/objects.md).
pub const BIPED_TYPE: u32 = 0;

/// Shared unit tuning. The two sequences at +0x220 and +0x264 are
/// contiguous on both discs (cyborg, crewman, marine, elite and grunt).
pub const UNIT_WORDS: &[(&str, usize)] = &[
    ("unit.unknown_bc", 0x17c), ("unit.unknown_c0", 0x180), // default team and motion sensor blip
    ("unit.unknown_dc", 0x1e8), ("unit.unknown_e0", 0x1ec), ("unit.unknown_e4", 0x1f0),
    ("unit.unknown_10c", 0x220), ("unit.unknown_110", 0x224),
    ("unit.unknown_114", 0x228), ("unit.unknown_118", 0x22c),
    ("unit.unknown_11c", 0x230), ("unit.unknown_120", 0x234),
    ("unit.unknown_124", 0x238), ("unit.unknown_144", 0x264),
    ("unit.unknown_148", 0x268), ("unit.unknown_14c", 0x26c),
    ("unit.unknown_1b0", 0x2c0), ("unit.unknown_1b4", 0x2c4),
];

/// Biped turning, landing, camera heights, ground slopes and their cached
/// trigonometric values. The crewman's distinct cutoff and falloff angles
/// distinguish the source fields (a30 and 05a_deltaapproach).
pub const BIPED_WORDS: &[(&str, usize)] = &[
    ("unknown_1ec", 0x2f0), ("unknown_1f4", 0x2f8),
    ("unknown_1fc", 0x3d4), ("unknown_200", 0x3d8), ("unknown_204", 0x3dc),
    ("unknown_208", 0x3e0), ("unknown_20c", 0x3e4),
    ("unknown_218", 0x400), ("unknown_21c", 0x404), ("unknown_220", 0x408),
    ("unknown_24c", 0x4c8), ("unknown_29c", 0x358), ("unknown_2a0", 0x35c),
    ("unknown_2a4", 0x360), ("unknown_2a8", 0x368), ("unknown_2ac", 0x36c),
    ("unknown_2b0", 0x364), ("unknown_2b4", 0x370), ("unknown_2b8", 0x4d0),
    ("unknown_2bc", 0x4d4), ("unknown_2c0", 0x4d8), ("unknown_2c4", 0x4dc),
    ("unknown_2c8", 0x4e0),
    ("unknown_2cc", 0x324), ("unknown_2d0", 0x328), ("unknown_2d4", 0x32c),
    ("unknown_2d8", 0x330), ("unknown_2dc", 0x334), ("unknown_2e0", 0x338),
    ("unknown_2ec", 0x344), ("unknown_2f4", 0x34c),
];

/// H1 caches these rates per tick. The c20 monitor and its 04b counterpart
/// agree on all three flying accelerations after conversion to seconds.
pub const BIPED_RATES: &[(&str, usize)] = &[
    ("unknown_1f8", 0x3b4), ("unknown_250", 0x4cc), ("unknown_2e4", 0x33c),
    ("unknown_2e8", 0x340), ("unknown_2f0", 0x348),
];

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

/// `bipd +0x1a0`, the HUD interfaces (one `nhdt` reference each,
/// [`crate::health_hud::UNIT_HUD_INTERFACES`]).
pub const BIPD_HUD: &str = "unit.block_1a0";
/// Halo 1 resources converted for the unit.
#[derive(Clone, Debug, Default)]
pub struct BipedParts {
    pub sentinel: bool,
    pub tracks: Vec<TagId>,
    pub weapons: Vec<TagId>,
    pub integrated_light: Option<TagId>,
    pub blood: Option<TagId>,
    pub melee: Option<TagId>,
    pub attachments: Vec<AttachmentSpec>,
    pub functions: Vec<crate::functions::FunctionSpec>,
    pub creation: Option<TagId>,
    /// The mass of the rebuilt rigid bodies, when present.
    pub mass: Option<f32>,
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

/// Build the biped and its unit layer from its source object and resources.
pub fn biped(
    unit: &h1::Unit,
    hlmt: Option<TagId>,
    predicted: &[Predicted],
    parts: &BipedParts,
) -> Result<Block> {
    let d = def(b"bipd")?;
    let mut r = defs::default_record(d);
    object_effects(b"bipd", &mut r, "unit.obje", &parts.attachments, parts.creation)?;
    object_functions(b"bipd", &mut r, "unit.obje", &parts.functions)?;
    for (path, group, target) in [
        ("unit.effe_ref_c4", b"effe", parts.integrated_light),
        ("ref_25c", b"effe", parts.blood),
        ("unit.ref_164", b"jpt!", parts.melee),
    ] {
        if let Some(id) = target {
            set(&mut r, path, Val::tag(FourCC(*group), id))?;
        }
    }
    for (path, field, group, ids) in [
        ("unit.block_e8", "ref_0", b"trak", &parts.tracks),
        ("unit.weapons", "weapon", b"weap", &parts.weapons),
    ] {
        let mut elements = Vec::new();
        for id in ids {
            let mut e = defs::default_record(d.child(path).expect("unit reference block"));
            set(&mut e, field, Val::tag(FourCC(*group), *id))?;
            elements.push(e);
        }
        set(&mut r, path, Val::Block(elements))?;
    }
    if let Some(mass) = parts.mass {
        set(&mut r, "unknown_274", Val::Bytes(mass.to_le_bytes().to_vec()))?;
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
    object_tuning(&mut r, "unit.obje", obj)?;
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
    object_change_colors(b"bipd", &mut r, "unit.obje", &obj.change_colours)?;
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
    for (path, at) in BIPED_WORDS.iter().chain(UNIT_WORDS) {
        if let Some((_, w)) = unit.biped_words.iter().find(|x| x.0 == *at) {
            set(&mut r, path, Val::Bytes(w.to_le_bytes().to_vec()))?;
        }
    }
    for (path, at) in BIPED_RATES {
        if let Some((_, ticks)) = unit.biped_words.iter().find(|w| w.0 == *at) {
            let rate = 30.0 * f32::from_bits(*ticks);
            set(&mut r, path, Val::Bytes(rate.to_le_bytes().to_vec()))?;
        }
    }
    if let Some((_, flags)) = unit.biped_words.iter().find(|w| w.0 == 0x2f4) {
        // H1 cyborgs and both H2 player characters use the player bit.
        // Both flying controller pairs (monitor, sentinel) select 0x13.
        let controller = ((flags & 2) << 1) | if flags & 4 != 0 { 0x13 } else { 0 };
        set(&mut r, "unknown_264", Val::Bytes(controller.to_le_bytes().to_vec()))?;
    }
    if parts.sentinel {
        // Both retail aggressors use this biped and flying-controller flag pair.
        set(&mut r, "unknown_1f0", Val::Bytes(0xe00u32.to_le_bytes().to_vec()))?;
        set(&mut r, "unknown_264", Val::Bytes(0x13u32.to_le_bytes().to_vec()))?;
    }
    if let Some(size) = unit.size.filter(usable_size) {
        for (path, v) in biped_shapes(&size)? {
            set(&mut r, &path, v)?;
        }
    }
    if let Some(size) = unit.size.filter(|s| !usable_size(s)) {
        // 0x1c2460 selects one of two spheres when the pill array is empty.
        let radius = if size.radius > 0.0 { size.radius } else { obj.bounding_radius };
        let mut sphere = defs::default_record(d.child("block_294").expect("character sphere"));
        for (path, word) in [("unknown_4", 0xffff_u32), ("unknown_1c", 0xffff_0000)] {
            set(&mut sphere, path, Val::Bytes(word.to_le_bytes().to_vec()))?;
        }
        for (path, value) in [
            ("unknown_2c", radius), ("unknown_40", 1.0), ("unknown_54", 1.0),
            ("unknown_68", 1.0), ("unknown_70", obj.bounding_offset[0]),
            ("unknown_74", obj.bounding_offset[1]), ("unknown_78", obj.bounding_offset[2]),
        ] {
            set(&mut sphere, path, Val::Bytes(value.to_le_bytes().to_vec()))?;
        }
        set(&mut r, BIPD_SIZE[2], Val::Bytes(radius.to_le_bytes().to_vec()))?;
        let mut probes = Vec::new();
        for scale in SPHERE_SCALES {
            let mut probe = sphere.clone();
            set(&mut probe, "unknown_2c", Val::Bytes((scale * radius).to_le_bytes().to_vec()))?;
            probes.push(probe);
        }
        set(&mut r, BIPD_SPHERES, Val::Block(probes))?;
        set(&mut r, "block_294", Val::Block(vec![sphere.clone(), sphere]))?;
    }
    built(d, &r)
}

/// What a Halo 2 effect part creates: the group word the engine dispatches
/// on (`effe` part `+8`: the reference's group, `obje` for any object;
/// docs/formats/effects.md), the reference's group, and the converted tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartTarget {
    pub type_group: [u8; 4],
    pub group: [u8; 4],
    pub tag: TagId,
}

/// The group word an effect part dispatches on for a tag of `group`:
/// objects create through `obje` (V: on the disc every part whose
/// reference is an object group holds `obje` at `+8`).
pub fn part_type_group(group: [u8; 4]) -> [u8; 4] {
    const OBJECTS: [&[u8; 4]; 13] = [
        b"bipd", b"vehi", b"weap", b"eqip", b"garb", b"proj", b"scen", b"mach", b"ctrl", b"lifi",
        b"ssce", b"bloc", b"crea",
    ];
    if OBJECTS.iter().any(|g| **g == group) {
        *b"obje"
    } else {
        group
    }
}

/// A Halo 1 effect written from the `effe` definition, no retail template
/// read: the locations' marker names, and per event Halo 1's skip
/// fraction, delay and duration and the parts `target` gives a converted
/// tag for (`None` leaves the part out), each with Halo 1's creation
/// medium, location, flags, velocity, cone, angular velocity, radius
/// modifier and scale words (the two parts hold them in the same order,
/// `h1::EffectPart`). The loop start is shared (E 0x001791ae reads it);
/// Halo 1's loop stop and flags have no established Halo 2 equivalent.
/// Particle systems are written by the caller.
pub fn effect(
    e: &h1::Effect,
    target: &mut dyn FnMut(&h1::EffectPart) -> Option<PartTarget>,
) -> Result<Block> {
    let d = def(b"effe")?;
    let mut r = defs::default_record(d);
    set(
        &mut r,
        "loop_start_event",
        Val::Int(e.loop_start_event.into()),
    )?;
    let loc_def = d.child("locations").expect("effe locations");
    let locations = e
        .locations
        .iter()
        .map(|m| {
            let mut x = defs::default_record(loc_def);
            set(&mut x, "marker", Val::text(m))?;
            Ok(x)
        })
        .collect::<Result<Vec<_>>>()?;
    set(&mut r, "locations", Val::Block(locations))?;
    let ev_def = d.child("events").expect("effe events");
    let part_def = d.child("events.parts").expect("effe parts");
    let mut events = Vec::new();
    for ev in &e.events {
        let mut x = defs::default_record(ev_def);
        set(&mut x, "skip_fraction", Val::Float(ev.skip_fraction))?;
        set(&mut x, "delay_bounds", Val::floats(&ev.delay_bounds))?;
        set(&mut x, "duration_bounds", Val::floats(&ev.duration_bounds))?;
        let mut parts = Vec::new();
        for p in &ev.parts {
            let Some(t) = target(p) else { continue };
            let mut y = defs::default_record(part_def);
            set(&mut y, "create_in", Val::Int(p.create_in.into()))?;
            set(&mut y, "location", Val::Int(p.location.into()))?;
            set(&mut y, "flags", Val::Int(p.flags.into()))?;
            set(
                &mut y,
                "type_group",
                Val::Int(u32::from_be_bytes(t.type_group).into()),
            )?;
            set(&mut y, "reference", Val::tag(FourCC(t.group), t.tag))?;
            set(&mut y, "velocity_bounds", Val::floats(&p.velocity_bounds))?;
            set(
                &mut y,
                "velocity_cone_angle",
                Val::Float(p.velocity_cone_angle),
            )?;
            set(
                &mut y,
                "angular_velocity_bounds",
                Val::floats(&p.angular_velocity_bounds),
            )?;
            set(
                &mut y,
                "radius_modifier_bounds",
                Val::floats(&p.radius_modifier_bounds),
            )?;
            set(&mut y, "a_scales", Val::Int(p.a_scales.into()))?;
            set(&mut y, "b_scales", Val::Int(p.b_scales.into()))?;
            parts.push(y);
        }
        set(&mut x, "parts", Val::Block(parts))?;
        events.push(x);
    }
    set(&mut r, "events", Val::Block(events))?;
    built(d, &r)
}

/// One attachment of a rebuilt object: the attached tag (`ligh`, `lsnd`,
/// `effe` or `cont`; E `0x000bed2a` builds these) and its marker,
/// change-colour slot and function names.
#[derive(Clone, Debug, PartialEq)]
pub struct AttachmentSpec {
    pub group: [u8; 4],
    pub tag: TagId,
    pub marker: String,
    /// `+0x10`: the name the attachment is scaled by
    /// ([`crate::functions`]); empty for none (1, always on). Only an
    /// object whose functions block holds the name may set it: a name
    /// nothing evaluates is 0, and a looping sound at 0 is off.
    pub primary_scale: String,
    pub secondary_scale: String,
    /// Halo 2's zero-based object change-colour slot (-1 on lights without one).
    pub change_colour: i16,
}

/// The attachment primary scale field (`obje +0x94` element `+0x10`).
pub const ATTACHMENT_PRIMARY_SCALE: &str = "string_10";

/// Sets `path.change_colors` (`obje +0xac`, `0x10` each) on a rebuilt
/// record from `colours`, already in Halo 2's slots
/// ([`h1::ChangeColour::halo2_slots`]); none when `colours` is empty.
/// Each element's first block is its initial permutations (`0x20` each:
/// `+0` weight, `+4` lower and `+0x10` upper bound RGB, `+0x1c` a variant
/// name, 0 for any). H2 reads the device's colour functions at 0xbe9a7
/// (interpolate by +0x24) and 0xbea15 (multiply by +0x20).
///
/// E: `object_new` reserves `0x18` bytes per element (two RGB colours,
/// the initial and the current); `0x000be240` sets each slot white
/// (`[0x468710]`), then, when the slot has permutations, picks one by
/// weight among those whose `+0x1c` is 0 or the object's variant
/// (`0x000be40e..0x000be44b`) and a colour between its bounds. The
/// player's colours are written over slots 0 to 3 by `0x0014bfc0`
/// through `0x000bacc0`, which leaves a slot past the object's count
/// alone, so a player's unit needs all four.
pub fn object_change_colors(
    group: &[u8; 4],
    r: &mut Record,
    path: &str,
    colours: &[h1::ChangeColour],
) -> Result<()> {
    if colours.is_empty() {
        return Ok(());
    }
    let field = format!("{path}.change_colors");
    let e = def(group)?
        .child(&field)
        .ok_or_else(|| Error::Invalid(format!("{} {field}", String::from_utf8_lossy(group))))?;
    let pe = e
        .child("block_0")
        .ok_or_else(|| Error::Invalid(format!("{field}.block_0")))?;
    let f = |v: f32| Val::Bytes(v.to_le_bytes().to_vec());
    let mut out = Vec::new();
    for c in colours {
        let mut x = defs::default_record(e);
        let mut perms = Vec::new();
        for p in &c.permutations {
            let mut y = defs::default_record(pe);
            set(&mut y, "unknown_0", f(p.weight))?;
            for (i, (lo, hi)) in p.lower.iter().zip(&p.upper).enumerate() {
                set(&mut y, &format!("unknown_{:x}", 4 + 4 * i), f(*lo))?;
                set(&mut y, &format!("unknown_{:x}", 0x10 + 4 * i), f(*hi))?;
            }
            perms.push(y);
        }
        set(&mut x, "block_0", Val::Block(perms))?;
        if let Some(source) = &c.function {
            let child = e.child("block_8").ok_or_else(|| Error::Invalid(format!("{field}.block_8")))?;
            let mut row = defs::default_record(child);
            set(&mut row, "unknown_4", Val::Bytes(source.flags.to_le_bytes().to_vec()))?;
            for (k, (lo, hi)) in source.lower.iter().zip(source.upper).enumerate() {
                set(&mut row, &format!("unknown_{:x}", 8 + 4*k), f(*lo))?;
                set(&mut row, &format!("unknown_{:x}", 0x14 + 4*k), f(hi))?;
            }
            if let Some(input) = source.scale {
                set(&mut row, "string_24", Val::text(input))?;
            }
            if let Some(input) = source.darken {
                set(&mut row, "darken_by", Val::text(input))?;
            }
            set(&mut x, "block_8", Val::Block(vec![row]))?;
        }
        out.push(x);
    }
    set(r, &field, Val::Block(out))
}

/// Sets `path.functions` (`obje +0x64`) on a rebuilt record.
pub fn object_functions(
    group: &[u8; 4],
    r: &mut Record,
    path: &str,
    functions: &[crate::functions::FunctionSpec],
) -> Result<()> {
    let e = def(group)?
        .child(&format!("{path}.functions"))
        .expect("obje functions");
    let mut out = Vec::new();
    for f in functions {
        let mut x = defs::default_record(e);
        set(&mut x, "unknown_0", Val::Bytes(f.flags.to_le_bytes().to_vec()))?;
        set(&mut x, "string_4", Val::text(&f.import))?;
        set(&mut x, "output", Val::text(&f.export))?;
        set(&mut x, "string_c", Val::text(&f.turn_off_with))?;
        set(&mut x, "data_14", Val::Data(f.data.clone()))?;
        out.push(x);
    }
    set(r, &format!("{path}.functions"), Val::Block(out))
}

/// Sets `path.attachments` (the object header's, `obje +0x94`) and
/// `path.creation_effect` on a rebuilt record.
pub fn object_effects(
    group: &[u8; 4],
    r: &mut Record,
    path: &str,
    attachments: &[AttachmentSpec],
    creation_effect: Option<TagId>,
) -> Result<()> {
    let e = def(group)?
        .child(&format!("{path}.attachments"))
        .expect("obje attachments");
    let mut out = Vec::new();
    for a in attachments {
        let mut x = defs::default_record(e);
        set(&mut x, "type", Val::tag(FourCC(a.group), a.tag))?;
        set(&mut x, "marker", Val::text(&a.marker))?;
        set(&mut x, "change_colour", Val::Int(a.change_colour.into()))?;
        set(&mut x, ATTACHMENT_PRIMARY_SCALE, Val::text(&a.primary_scale))?;
        set(&mut x, "string_14", Val::text(&a.secondary_scale))?;
        out.push(x);
    }
    set(r, &format!("{path}.attachments"), Val::Block(out))?;
    set(
        r,
        &format!("{path}.creation_effect"),
        Val::tag(FourCC(*b"effe"), creation_effect.unwrap_or(TagId::NULL)),
    )
}

/// `ligh +0` bit 3: the light casts no shadow (E,
/// docs/graphics/e3-lighting.md). Halo 1's dynamic lights cast none.
pub const LIGHT_NO_SHADOW: u32 = 1 << 3;
/// `ligh +4` kinds written: sphere and frustum (a projected cone).
pub const LIGHT_SPHERE: u32 = 0;
pub const LIGHT_FRUSTUM: u32 = 2;

/// A Halo 1 light written from the `ligh` definition, no retail template
/// read: a sphere of Halo 1's radius, or a frustum (Halo 1's cutoff angle
/// as the field of view, its radius as the range), Halo 1's two colours as
/// the diffuse and specular colours at the two intensity bounds, no
/// shadow, Halo 1's duration in seconds (ticks over 30), and unity
/// brightness. Everything else takes the
/// definition's default.
pub fn light(l: &h1::Light) -> Result<Block> {
    let d = def(b"ligh")?;
    let mut r = defs::default_record(d);
    // 0x000c0dc9 interpolates RGB and a separate brightness multiplier.
    // Halo 1 specifies RGB directly, so its multiplier is unity.
    set(&mut r, "brightness", Val::floats(&[1.0, 1.0]))?;
    set(&mut r, "flags", Val::Int(LIGHT_NO_SHADOW.into()))?;
    if l.is_cone() {
        set(&mut r, "kind", Val::Enum(LIGHT_FRUSTUM))?;
        set(&mut r, "height_stretch", Val::Float(1.0))?;
        set(&mut r, "field_of_view", Val::Float(l.cutoff_angle))?;
        set(&mut r, "falloff_distance", Val::Float(l.radius))?;
        set(&mut r, "cutoff_distance", Val::Float(l.radius))?;
    } else {
        set(&mut r, "kind", Val::Enum(LIGHT_SPHERE))?;
        set(&mut r, "radius", Val::Float(l.radius))?;
        set(&mut r, "specular_radius", Val::Float(l.radius))?;
    }
    let rgb = |c: &[f32; 4]| Val::floats(&[c[1], c[2], c[3]]);
    for (name, c) in [
        ("diffuse_colour_lower", &l.colour_lower),
        ("diffuse_colour_upper", &l.colour_upper),
        ("specular_colour_lower", &l.colour_lower),
        ("specular_colour_upper", &l.colour_upper),
    ] {
        set(&mut r, name, rgb(c))?;
    }
    if l.duration_ticks > 0.0 {
        set(
            &mut r,
            "duration",
            Val::Float(l.duration_ticks / crate::convert::TICKS_PER_SECOND),
        )?;
    }
    built(d, &r)
}

/// Write the authored placement's constant RGB and brightness to the
/// rebuilt light while keeping its shape and flare.
pub fn fixture_light(root: &Block, lighting: h1convert::fixture::Lighting) -> Result<Block> {
    let d = def(b"ligh")?;
    let mut r = defs::from_built(d, root).map_err(|e| Error::Invalid(e.to_string()))?;
    for field in [
        "diffuse_colour_lower",
        "diffuse_colour_upper",
        "specular_colour_lower",
        "specular_colour_upper",
    ] {
        set(&mut r, field, Val::floats(&lighting.rgb))?;
    }
    set(&mut r, "brightness", Val::floats(&[lighting.intensity; 2]))?;
    built(d, &r)
}

/// A Halo 1 decal written from the `deca` definition, no retail template
/// read: Halo 1's flags, kind, radius, colours, lifetime, decay time and
/// sprite extent (the two hold them in the same order), the converted
/// next decal and bitmap, and the corresponding Halo 2 layer.
pub fn decal(
    x: &h1::Decal,
    next: Option<TagId>,
    bitmap: Option<TagId>,
) -> Result<Block> {
    let d = def(b"deca")?;
    let mut r = defs::default_record(d);
    set(&mut r, "flags", Val::Int(x.flags.into()))?;
    set(&mut r, "kind", Val::Int(x.kind.into()))?;
    // H1's solid, inner glow, outer glow and environment layers.
    // See the matched impact, plasma-burn and airlock-floor decal chains
    // in role-audit.md. A layer outside the H1 enumeration is malformed.
    let layer = [2, 4, 5, 1].get(usize::from(x.layer))
        .ok_or_else(|| Error::Invalid(format!("unknown Halo 1 decal layer {}", x.layer)))?;
    set(&mut r, "layer", Val::Int(*layer))?;
    set(&mut r, "next_decal", Val::tag(FourCC(*b"deca"), next.unwrap_or(TagId::NULL)))?;
    set(&mut r, "radius", Val::floats(&x.radius))?;
    set(&mut r, "colour_lower", Val::floats(&x.colour_lower))?;
    set(&mut r, "colour_upper", Val::floats(&x.colour_upper))?;
    set(&mut r, "lifetime_bounds", Val::floats(&x.lifetime))?;
    set(&mut r, "decay_time_bounds", Val::floats(&x.decay_time))?;
    set(&mut r, "bitmap", Val::tag(FourCC(*b"bitm"), bitmap.unwrap_or(TagId::NULL)))?;
    set(
        &mut r,
        "maximum_sprite_extent",
        Val::Float(x.maximum_sprite_extent),
    )?;
    built(d, &r)
}

/// Halo 1 dialogue slot (`udlg` offset, [`h1::UDLG_SIZE`]) to the Halo 2
/// vocalization that plays its sound, in priority order: where two slots
/// name one vocalization the first that holds a sound wins.
///
/// The slot names (in the comments) are V: the sound names Halo 1's
/// dialogue tags hold there. The Halo 2 names are retail vocalization
/// names (V: every one is the string of an element of the base maps'
/// `udlg` tags, `halo2_dialogue_survey`). Pairing them is D, by meaning,
/// but for pain and death, where the multiplayer elite's and the
/// Chief's dialogue tags give the bridge (V): their `death` sound plays
/// for `dth`, `dth_mjr`, `dth_slnt`, `dth_slw` and `dth_hdsht`, their
/// `deathfall` / `fall_death` for `dth_fall`, their `pain` for
/// `pain_mjr`. Source-only weapon kill lines use the winning kill
/// candidate in the runtime scheduler instead.
pub const DIALOGUE: &[(usize, &str)] = &[
    (0x010, "look_noncmbt"),     // idle_non_combat
    (0x020, "look_cmbt"),        // idle_combat
    (0x030, "panic"),            // panic_run_away
    (0x070, "pain"),             // painminor
    (0x080, "pain_mjr"),         // painmajor
    (0x090, "pain_shld"),        // painshield
    (0x0a0, "pain_fall"),        // painfalling
    (0x0c0, "scrn"),             // screampain
    (0x0f0, "dth_slnt"),         // deathquiet
    (0x100, "dth_mjr"),          // deathviolent
    (0x100, "dth"),              // deathviolent
    (0x110, "dth_fall"),         // deathfalling
    (0x120, "dth_slw"),          // deathagonizing
    (0x130, "dth_hdsht"),        // deathinstant
    (0x100, "dth_hdsht"),        // deathviolent
    (0x200, "scld_plr_hrtme"),   // hurtfriendplayer, event 3
    (0x360, "chr_kllfoe"),       // killedenemy
    (0x420, "chr_kllfoe_blt"),   // killedenemybullet
    (0x460, "chr_kllfoe_bm"),    // killedenemygrenade, event 0 hostile attacker, category 3
    (0x550, "prs_plr_sniping"),  // playerkillsnipercm, adlg event 0 pattern 27
    (0x610, "lmnt_deadally"),    // frienddied
    (0x620, "lmnt_deadplr"),     // friendplayerdied
    (0x6a0, "betray"),           // friendbetrayed
    (0x6d0, "seefoe"),           // newcombatalone
    (0x6f0, "foundfoe"),         // oldenemysighted
    (0x700, "seefoe_srprs"),     // unexpectedenemy
    (0x710, "chckallybdy"),      // deadfriendfound
    (0x730, "forgive"),          // alliancereformed, event 196
    (0x740, "strk_grnd"),        // grenadethrowing
    (0x750, "warn_incmn_grnd"),  // grenadesighted
    (0x840, "srchstart"),        // searchstart
    (0x870, "srch_allclr"),      // searchreport, event 46 search completion
    (0x870, "prst_allclr"),      // searchreport, event 48 pursuit completion
    (0x880, "prstfail_nospc"),   // searchabandon, cannot take the pursuit position
    (0x880, "prstfail_agg"),     // aggressive variant
    (0x880, "prstfail_notme"),   // searchabandon, another actor takes the pursuit
    (0x880, "prstfail_tim"),     // timid variant
    (0x880, "prstfail_nopts"),   // searchabandon, no valid point
    (0x880, "prstfail_nopts_tim"),
    (0x8c0, "newordr_advance"),  // advance
    (0x8e0, "newordr_retreat"),  // retreat
    (0x950, "grt_plr"),          // sightedfriendlyplayer, event 163
    (0x9b0, "tnt"),              // taunt
    (0x9d0, "flee"),             // flee
    (0x9f0, "fear_rtrt_ldrdead"), // flee_leader_died, adlg event 63 pattern 110
    (0xa40, "entervcl"),         // vehicleentry
    (0xa60, "prs_plr_vclchr"),   // vehiclewoohoo, event 85
    (0xa70, "scld_plr_vclcrazy"), // vehiclescared, event 87
    (0xb20, "srprs"),            // surprise
    (0xb30, "brsrk"),            // berserk
    (0xb40, "melee"),            // meleeattack
    (0xb50, "dive"),             // dive
    (0xbd0, "pstcmbt"),          // celebration
    (0xbe0, "chkfoebdy"),        // checkbodyenemy
    (0xc00, "shotfoebdy"),       // shootingdeadenemy
];

/// The first authored slot without a native, rebuilt, or dormant source rule.
pub fn unbound_dialogue_line(source: &h1::Dialogue) -> Option<(usize, h1map::TagId)> {
    let routed = dialogue_lines(source);
    let generic_kill = routed.iter().any(|&(voc, _, _)| voc == "chr_kllfoe");
    source.lines.iter().copied().find(|(slot, _)| {
        let native = routed.iter().any(|(_, at, _)| at == slot);
        let kill_variant = generic_kill
            && (map::H1_WEAPON_KILL_LINES.iter().any(|&(_, at)| at == *slot)
                || map::h1_species_kill_line(*slot));
        !native
            && !kill_variant
            && !map::h1_injury_line(*slot)
            && !map::h1_friendly_kill_line(*slot)
            && !map::h1_kill_witness_line(*slot)
            && !map::h1_actor_notice_line(*slot)
            && !map::h1_firing_line(*slot)
            && !map::h1_tactical_dialogue_line(*slot)
            && !map::h1_grenade_dialogue_line(*slot)
            && !map::h1_postcombat_line(*slot)
            && !map::h1_unselected_dialogue_line(*slot)
            && !map::h1_region_dialogue_line(*slot)
            && !map::h1_allied_reply_line(*slot)
            && !matches!(
                *slot,
                map::H1_KILLING_SPREE_LINE
                    | map::H1_KILLING_SPREE_REPLY_LINE
                    | map::H1_DEATH_FLYING_LINE
                    | map::H1_REVIVAL_LINE
                    | map::H1_FREEFALL_LINE
                    | map::H1_SEARCH_QUERY_LINE
                    | map::H1_SEARCH_REPLY_LINE
            )
    })
}

/// The vocalizations a Halo 1 dialogue tag gives, each with the slot its
/// sound is in ([`DIALOGUE`]), first slot holding a sound winning.
pub fn dialogue_lines(d: &h1::Dialogue) -> Vec<(&'static str, usize, h1map::TagId)> {
    let mut out: Vec<(&'static str, usize, h1map::TagId)> = Vec::new();
    for &(at, voc) in DIALOGUE {
        if out.iter().any(|l| l.0 == voc) {
            continue;
        }
        if let Some(s) = d.line(at) {
            out.push((voc, at, s));
        }
    }
    out
}

/// `udlg +0`: the 'adlg' group word of a null reference every retail
/// dialogue tag holds (V: the 42 distinct ones, `+4` -1).
const UDLG_GROUP: [u8; 4] = *b"glda";
/// `udlg` element `+8`: the 'snd!' group word of the datum that follows
/// (V: every retail element).
const UDLG_SOUND_GROUP: [u8; 4] = *b"!dns";

/// The native prefix is indexed by Halo 2's global vocabulary. Rows
/// after that prefix keep each authored Halo 1 slot, including lines
/// whose source events do not have a native vocalization.
pub fn dialogue(
    vocabulary: &[String],
    native: &[(&str, usize, TagId)],
    source: &[(usize, TagId)],
) -> Result<Block> {
    let d = def(b"udlg")?;
    let mut r = defs::default_record(d);
    set(&mut r, "unknown_0", Val::Bytes(UDLG_GROUP.to_vec()))?;
    set(&mut r, "unknown_4", Val::Bytes(vec![0xff; 4]))?;
    let el = d.child("vocalizations").expect("udlg vocalizations");
    let row = |name: &str, sound: TagId, marker: u32| -> Result<Record> {
        let mut x = defs::default_record(el);
        set(&mut x, "vocalization", Val::text(name))?;
        set(&mut x, "unknown_0", Val::Bytes(marker.to_le_bytes().to_vec()))?;
        set(&mut x, "unknown_8", Val::Bytes(UDLG_SOUND_GROUP.to_vec()))?;
        set(&mut x, "sound", Val::Datum(defs::Target::Id(sound)))?;
        Ok(x)
    };
    let mut items = vocabulary
        .iter()
        .map(|name| {
            let Some(&(_, offset, sound)) = native.iter().find(|(voc, _, _)| *voc == name.as_str()) else {
                return row(name, TagId::NULL, 0);
            };
            let slot = (offset / 0x10 - 1) as u32;
            row(name, sound, map::H1_NATIVE_DIALOGUE_SLOT | slot)
        })
        .collect::<Result<Vec<_>>>()?;
    for &(offset, sound) in source {
        let slot = (offset / 0x10 - 1) as u32;
        let marker = map::H1_DIALOGUE_SLOT | slot;
        items.push(row(&format!("h1_slot_{slot:03x}"), sound, marker)?);
    }
    set(&mut r, "vocalizations", Val::Block(items))?;
    built(d, &r)
}

/// H2 speech setup (0x114ec0) honors variant dialogue at +0x34. Voice
/// variants are selected only by the speech hook, using the marked
/// Halo 1 selector. The default visual variant keeps its own index.
pub(crate) fn model_dialogue(r: &mut Record, voices: &[(i16, TagId)]) -> Result<()> {
    let udlg = FourCC(*b"udlg");
    match voices {
        [] => return Ok(()),
        [(0, id)] => return set(r, crate::convert::DIALOGUE_FIELD, Val::tag(udlg, *id)),
        _ => {}
    }

    let d = def(b"hlmt")?.child("variants").expect("hlmt variants");
    let mut variants = match r.at("variants") {
        Some(Val::Block(rows)) => rows.clone(),
        _ => return Err(Error::Invalid("hlmt variants".into())),
    };
    let visual = |name: &str| -> Result<Record> {
        let mut row = defs::default_record(d);
        set(&mut row, "name", Val::text(name))?;
        set(&mut row, "region_map", Val::List(vec![Val::Int(-1); 16]))?;
        Ok(row)
    };
    if variants.is_empty() {
        variants.push(visual("default")?);
    }
    for (i, &(selector, id)) in voices.iter().enumerate() {
        let mut row = visual(&format!("h1_voice_{i}"))?;
        let marker = map::H1_VOICE_VARIANT | u32::from(selector as u16);
        set(&mut row, "h1_voice_selector", Val::Int(marker.into()))?;
        set(&mut row, "udlg_ref_30", Val::tag(udlg, id))?;
        variants.push(row);
    }
    set(r, "variants", Val::Block(variants))
}

/// Halo 1's foot kinds Halo 2 has at the same index (V: on every Halo 2
/// character's `foot` kinds 0, 1, 3 and 4 play walk, run, shuffle and
/// jump sounds and kind 2 none, as Halo 1's; `halo2_foot_survey`); kind 5
/// (Halo 1's landing) is kept at 5 (D).
pub const FOOT_KINDS: usize = 6;

/// One `foot` element: the tag, its global material name and index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FootLine {
    pub material: &'static str,
    pub index: i16,
    pub tag: TagId,
}

/// One kind of a `foot`: its sounds and effects.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FootKind {
    pub sounds: Vec<FootLine>,
    pub effects: Vec<FootLine>,
}

/// `foot` elements' `+0x16`: 2 on a sound element, 1 on an effect element
/// (V: the retail majority on each; what reads it is not established).
const FOOT_SOUND_WORD: u16 = 2;
const FOOT_EFFECT_WORD: u16 = 1;

/// The `foot` root: one kind per element of `kinds`, each with its sounds
/// and effects by global material.
pub fn foot(kinds: &[FootKind]) -> Result<Block> {
    let d = def(b"foot")?;
    let mut r = defs::default_record(d);
    let kd = d.child("effects").expect("foot effects");
    let sd = d.child("effects.sounds").expect("foot sounds");
    let ed = d.child("effects.effects").expect("foot effects.effects");
    let mut out = Vec::new();
    for k in kinds {
        let mut x = defs::default_record(kd);
        let mut sounds = Vec::new();
        for l in &k.sounds {
            let mut y = defs::default_record(sd);
            set(&mut y, "sound", Val::tag(FourCC(*b"snd!"), l.tag))?;
            set(&mut y, "unknown_8", Val::Bytes(vec![0xff; 8]))?;
            set(&mut y, "material", Val::text(l.material))?;
            set(&mut y, "material_index", Val::Int(l.index.into()))?;
            set(&mut y, "unknown_16", Val::Bytes(FOOT_SOUND_WORD.to_le_bytes().to_vec()))?;
            sounds.push(y);
        }
        let mut effects = Vec::new();
        for l in &k.effects {
            let mut y = defs::default_record(ed);
            set(&mut y, "effect", Val::tag(FourCC(*b"effe"), l.tag))?;
            set(&mut y, "material", Val::text(l.material))?;
            set(&mut y, "material_index", Val::Int(l.index.into()))?;
            set(&mut y, "unknown_16", Val::Bytes(FOOT_EFFECT_WORD.to_le_bytes().to_vec()))?;
            effects.push(y);
        }
        set(&mut x, "sounds", Val::Block(sounds))?;
        set(&mut x, "effects", Val::Block(effects))?;
        out.push(x);
    }
    set(&mut r, "effects", Val::Block(out))?;
    built(d, &r)
}
