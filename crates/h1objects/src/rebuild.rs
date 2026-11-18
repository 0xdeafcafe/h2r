//! Scenery written field by field from typed definitions
//! ([`tags::defs`]), with no retail template bytes: the `scen`, its
//! `hlmt`, and its `mode`'s material list. Every field starts at its
//! definition's default (zero, null or empty; unknown spans zero) and only
//! what the conversion knows is set. Opt-in ([`crate::convert::Options`]);
//! the template path stays the default until this one has run in the game.
//! The field-by-field comparison with the template path is in
//! `tests/rebuild_disc.rs` and `docs/formats/tag-definitions.md`.

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

/// The `scen` root: the object header with Halo 1's bounding sphere and
/// the model, everything else at its default.
pub fn scenery(obj: &h1::Object, hlmt: Option<TagId>) -> Result<Block> {
    let d = def(b"scen")?;
    let mut r = defs::default_record(d);
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
    /// Node count and the node array ([`h1models::mode::hlmt_nodes`]).
    pub nodes: (u32, Block),
    pub regions: Vec<Region>,
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
    built(d, &r)
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

/// A Halo 1 vehicle with no Halo 2 counterpart, written from the `vehi`
/// definition: the object header (type, bounds, model), the turret role
/// ([`ROLE_VEHICLE_TYPE`], [`TURRET_ROLE`]), and one seat per Halo 1 seat
/// with its label, marker and camera marker. Everything else is the
/// definition default. No retail template is read.
pub fn vehicle(
    obj: &h1::Object,
    hlmt: Option<TagId>,
    seats: &[h1::Seat],
    predicted: &[Predicted],
    camera_field_of_view: f32,
) -> Result<Block> {
    let d = def(b"vehi")?;
    let mut r = defs::default_record(d);
    role(&mut r, TURRET_ROLE)?;
    set(
        &mut r,
        "unit.camera_field_of_view",
        Val::Float(camera_field_of_view),
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
    set(&mut r, "vehicle_type", Val::Enum(ROLE_VEHICLE_TYPE))?;
    predicted_resources(b"vehi", &mut r, "unit.obje", predicted)?;
    let seat_def = d.child(VEHI_SEATS).expect("vehi seats");
    let mut out = Vec::new();
    for s in seats {
        let mut e = defs::default_record(seat_def);
        role(&mut e, TURRET_SEAT_ROLE)?;
        set(&mut e, "label", Val::text(&s.label))?;
        // D: Halo 2's seats hold the rider's marker at +8 (retail:
        // `driver`, `gunner`, `passenger`, the words Halo 1's seat marker
        // holds) and the camera marker at +0x60 (`camera_rider`).
        set(&mut e, "string_8", Val::text(&s.marker))?;
        set(&mut e, "string_60", Val::text(&s.camera_marker))?;
        out.push(e);
    }
    set(&mut r, VEHI_SEATS, Val::Block(out))?;
    let phys_def = d.child(VEHI_PHYSICS).expect("vehi physics");
    let mut p = defs::default_record(phys_def);
    role(&mut p, TURRET_PHYSICS_ROLE)?;
    set(&mut r, VEHI_PHYSICS, Val::Block(vec![p]))?;
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
/// - rigid body `+0x1a` 2, `+0x1e` 5, `+0x20` 1.0, `+0x28` 0.05.
/// - node `+4` flags (0), `+6` parent, `+8` next sibling, `+0xa` first
///   child, each `-1` none.
///
/// Every runtime word (shape vtables and pointers) stays zero: the engine
/// writes them at load (`0x001ea9c0`).
pub fn physics_model(bodies: &[Body], nodes: &[PhysNode]) -> Result<Block> {
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
        set(&mut rb, "unknown_18", u16_hi(2))?;
        set(&mut rb, "unknown_1c", u16_hi(5))?;
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
/// Chief, humans, or the Covenant. A biped of none (flood, sentinels,
/// the monitor) is not rebuilt.
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

/// A Halo 1 biped written from the `bipd` definition: the object header
/// (type 0, Halo 1's bounding sphere, the model and its predicted
/// resources), its role's words ([`biped_role`]), Halo 1's camera field
/// of view and [`BIPED_WORDS`]. References stay null and blocks other than
/// the role's plain-word ones empty: no retail template is read.
pub fn biped(
    unit: &h1::Unit,
    hlmt: Option<TagId>,
    predicted: &[Predicted],
    role: &RoleWords,
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
    for (path, at) in BIPED_WORDS {
        if let Some((_, w)) = unit.biped_words.iter().find(|x| x.0 == *at) {
            set(&mut r, path, Val::Bytes(w.to_le_bytes().to_vec()))?;
        }
    }
    built(d, &r)
}
