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
    ("unit.unknown_cc", 0x3f9c_61aa),
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
fn predicted_resources(
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
) -> Result<Block> {
    let d = def(b"vehi")?;
    let mut r = defs::default_record(d);
    role(&mut r, TURRET_ROLE)?;
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
/// `object_new` deletes it and its name stays `none`; a rebuilt vehicle
/// gets this one. It is a stand-in until Halo 1's collision and physics
/// become a Halo 2 shape: a sphere this small barely touches anything.
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
