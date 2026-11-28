//! The Havok world, read: its rigid bodies and its contact points
//! (docs/engine/havok-runtime.md). The world at `layout::HAVOK_WORLD_PTR`
//! keeps its entities in simulation islands (active, inactive, and one
//! fixed island for the level); contacts are contact constraints in the
//! islands' constraint arrays, one per touching pair, each with its points.
//!
//! Every count is checked against a cap, so a torn or stale read gives an
//! error, not a runaway walk.

use guest_layout::{GuestMemory, MemFault};

use crate::layout;

/// Which island an entity is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IslandState {
    Active,
    Inactive,
    /// The fixed island: the level and other bodies that never move.
    Fixed,
}

impl IslandState {
    pub fn as_str(self) -> &'static str {
        match self {
            IslandState::Active => "active",
            IslandState::Inactive => "inactive",
            IslandState::Fixed => "fixed",
        }
    }
}

/// One rigid body (Havok entity).
#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    /// The entity's guest address.
    pub entity: u32,
    pub state: IslandState,
    /// The object datum handle, through the entity's physics component
    /// property; `None` for the level and other bodies without one.
    pub object: Option<u32>,
    pub pos: [f64; 3],
    pub velocity: [f64; 3],
    pub angular_velocity: [f64; 3],
    /// 0 for a body that does not move (infinite mass).
    pub inv_mass: f64,
    pub friction: f64,
    pub restitution: f64,
    /// The collidable's shape (`layout::ENTITY_SHAPE`); `None` without one
    /// or where it does not read.
    pub shape: Option<Shape>,
}

/// What kind of shape, by its type getter (`layout::SHAPE_TYPE_*_FN`).
#[derive(Debug, Clone, PartialEq)]
pub enum ShapeKind {
    /// End points `a` and `b` in the body's frame, and the radius.
    Capsule {
        a: [f64; 3],
        b: [f64; 3],
        radius: f64,
    },
    Sphere {
        radius: f64,
    },
    /// A child shape moved by a transform. The child is decoded, but not
    /// its own children; `None` where it does not read.
    Transform {
        translation: [f64; 3],
        child: u32,
        child_shape: Option<Box<Shape>>,
    },
    /// Another type: only the pointer and vtable.
    Other,
}

/// A Havok shape (docs/engine/havok-runtime.md, "Shapes").
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub addr: u32,
    pub vtable: u32,
    pub kind: ShapeKind,
}

/// One contact point of a contact constraint.
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub pos: [f64; 3],
    /// Havok's separating normal (from B toward A, inferred).
    pub normal: [f64; 3],
    /// Separation along the normal: negative is penetration.
    pub distance: f64,
    /// The impulse the solver applied along the normal on the last step.
    pub impulse: f64,
    /// The pair's combined friction and restitution.
    pub friction: f64,
    pub restitution: f64,
}

/// A contact constraint: two entities touching, and the points.
#[derive(Debug, Clone, PartialEq)]
pub struct Contact {
    /// The constraint's guest address.
    pub constraint: u32,
    pub a: u32,
    pub b: u32,
    pub points: Vec<Point>,
}

/// Caps on what a walk accepts.
const MAX_ISLANDS: u32 = 4096;
const MAX_ENTITIES: u32 = 8192;
const MAX_CONSTRAINTS: u32 = 16384;
const MAX_PROPERTIES: u32 = 64;
/// `0x00318c22` refuses a point past 250.
const MAX_POINTS: u32 = 256;

pub(crate) fn bad(addr: u32) -> MemFault {
    MemFault {
        addr,
        len: 4,
        write: false,
    }
}

/// Check the entire range before computing individual field/element addresses.
pub(crate) fn span(base: u32, size: u32) -> Result<(), MemFault> {
    if size != 0 && (base == 0 || base.checked_add(size - 1).is_none()) {
        return Err(bad(base));
    }
    Ok(())
}

/// A Havok array's pointer and count at `at`, the count checked.
pub(crate) fn array(m: &dyn GuestMemory, at: u32, cap: u32) -> Result<(u32, u32), MemFault> {
    span(at, 8)?;
    let ptr = m.u32(at)?;
    let count = m.u32(at + 4)?;
    if count > cap || (count > 0 && ptr == 0) {
        return Err(bad(at));
    }
    Ok((ptr, count))
}

fn vector(m: &dyn GuestMemory, at: u32) -> Result<[f64; 3], MemFault> {
    Ok([
        f64::from(m.f32(at)?),
        f64::from(m.f32(at + 4)?),
        f64::from(m.f32(at + 8)?),
    ])
}

fn world(m: &dyn GuestMemory) -> Result<Option<u32>, MemFault> {
    let w = m.u32(layout::HAVOK_WORLD_PTR)?;
    Ok((w != 0).then_some(w))
}

/// The islands, each with its state: active, inactive, then fixed.
pub(crate) fn islands(m: &dyn GuestMemory, w: u32) -> Result<Vec<(u32, IslandState)>, MemFault> {
    span(w, layout::WORLD_FIXED_ISLAND + 4)?;
    let mut out = Vec::new();
    for (at, state) in [
        (layout::WORLD_ACTIVE_ISLANDS, IslandState::Active),
        (layout::WORLD_INACTIVE_ISLANDS, IslandState::Inactive),
    ] {
        let (ptr, count) = array(m, w + at, MAX_ISLANDS)?;
        span(ptr, 4 * count)?;
        for i in 0..count {
            let island = m.u32(ptr + 4 * i)?;
            if island != 0 {
                out.push((island, state));
            }
        }
    }
    let fixed = m.u32(w + layout::WORLD_FIXED_ISLAND)?;
    if fixed != 0 {
        out.push((fixed, IslandState::Fixed));
    }
    Ok(out)
}

/// The object an entity belongs to: its physics component property
/// (`0x001c3f30`), the component's object handle (`0x001c3fa4`).
pub fn entity_object(m: &dyn GuestMemory, entity: u32) -> Result<Option<u32>, MemFault> {
    span(entity, layout::ENTITY_PROPERTIES + 8)?;
    let (ptr, count) = array(m, entity + layout::ENTITY_PROPERTIES, MAX_PROPERTIES)?;
    span(ptr, 8 * count)?;
    let mut component = None;
    for i in 0..count {
        if m.u32(ptr + 8 * i)? == layout::PROPERTY_PHYSICS_COMPONENT {
            component = Some(m.u32(ptr + 8 * i + 4)?);
            break;
        }
    }
    let Some(index) = component.filter(|c| *c != u32::MAX) else {
        return Ok(None);
    };
    let components = m.u32(layout::PHYSICS_COMPONENTS_PTR)?;
    if components == 0 {
        return Ok(None);
    }
    span(components, layout::DATUM_DATA + 4)?;
    let data = m.u32(components + layout::DATUM_DATA)?;
    let offset = (index & 0xffff) * layout::PHYSICS_COMPONENT_SIZE + layout::COMPONENT_OBJECT;
    span(data, offset + 4)?;
    let object = m.u32(data + offset)?;
    Ok((object != u32::MAX).then_some(object))
}

/// The shape at `addr`, its children followed `depth` levels.
fn shape_at(m: &dyn GuestMemory, addr: u32, depth: u32) -> Result<Shape, MemFault> {
    let vtable = m.u32(addr)?;
    let kind = match m.u32(vtable.wrapping_add(layout::SHAPE_TYPE_SLOT))? {
        layout::SHAPE_TYPE_CAPSULE_FN => ShapeKind::Capsule {
            a: vector(m, addr + layout::CAPSULE_A)?,
            b: vector(m, addr + layout::CAPSULE_B)?,
            radius: f64::from(m.f32(addr + layout::SHAPE_RADIUS)?),
        },
        layout::SHAPE_TYPE_SPHERE_FN => ShapeKind::Sphere {
            radius: f64::from(m.f32(addr + layout::SHAPE_RADIUS)?),
        },
        layout::SHAPE_TYPE_TRANSFORM_FN => {
            let child = m.u32(addr + layout::TRANSFORM_CHILD)?;
            let child_shape = if child != 0 && depth > 0 {
                shape_at(m, child, depth - 1).ok().map(Box::new)
            } else {
                None
            };
            ShapeKind::Transform {
                translation: vector(m, addr + layout::TRANSFORM_TRANSLATION)?,
                child,
                child_shape,
            }
        }
        _ => ShapeKind::Other,
    };
    Ok(Shape { addr, vtable, kind })
}

/// An entity's shape, decoded; `None` without one. A shape that does not
/// read is `None` too: it only adds detail to the body.
pub fn entity_shape(m: &dyn GuestMemory, entity: u32) -> Result<Option<Shape>, MemFault> {
    let addr = m.u32(entity + layout::ENTITY_SHAPE)?;
    if addr == 0 {
        return Ok(None);
    }
    Ok(shape_at(m, addr, 1).ok())
}

fn body(m: &dyn GuestMemory, entity: u32, state: IslandState) -> Result<Body, MemFault> {
    let motion = m.u32(entity + layout::RIGID_MOTION)?;
    if motion == 0 {
        return Err(bad(entity + layout::RIGID_MOTION));
    }
    Ok(Body {
        entity,
        state,
        object: entity_object(m, entity)?,
        pos: vector(m, motion + layout::MOTION_POSITION)?,
        velocity: vector(m, motion + layout::MOTION_VELOCITY)?,
        angular_velocity: vector(m, motion + layout::MOTION_ANGULAR_VELOCITY)?,
        inv_mass: f64::from(m.f32(motion + layout::MOTION_INV_MASS)?),
        friction: f64::from(m.f32(entity + layout::ENTITY_FRICTION)?),
        restitution: f64::from(m.f32(entity + layout::ENTITY_RESTITUTION)?),
        shape: entity_shape(m, entity)?,
    })
}

/// Every rigid body in the world, island by island (active, inactive,
/// fixed). Empty while there is no world.
pub fn bodies(m: &dyn GuestMemory) -> Result<Vec<Body>, MemFault> {
    let Some(w) = world(m)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (island, state) in islands(m, w)? {
        let (ptr, count) = array(m, island + layout::ISLAND_ENTITIES, MAX_ENTITIES)?;
        for i in 0..count {
            let entity = m.u32(ptr + 4 * i)?;
            if entity != 0 {
                out.push(body(m, entity, state)?);
            }
        }
    }
    Ok(out)
}

fn points(m: &dyn GuestMemory, constraint: u32) -> Result<Vec<Point>, MemFault> {
    let (ptr, count) = array(m, constraint + layout::CONTACT_POINTS, MAX_POINTS)?;
    let (props, props_count) = array(m, constraint + layout::CONTACT_PROPERTIES, MAX_POINTS)?;
    if props_count < count {
        return Err(bad(constraint + layout::CONTACT_PROPERTIES));
    }
    (0..count)
        .map(|i| {
            let p = ptr + i * layout::CONTACT_POINT_SIZE;
            let q = props + i * layout::CONTACT_PROPERTY_SIZE;
            Ok(Point {
                pos: vector(m, p)?,
                normal: vector(m, p + layout::POINT_NORMAL)?,
                distance: f64::from(m.f32(p + layout::POINT_DISTANCE)?),
                impulse: f64::from(m.f32(q + layout::PROPERTY_IMPULSE)?),
                friction: f64::from(m.f32(q + layout::PROPERTY_FRICTION)?),
                restitution: f64::from(m.f32(q + layout::PROPERTY_RESTITUTION)?),
            })
        })
        .collect()
}

/// Every contact constraint in the world's islands, with its points. Other
/// constraints (ragdoll joints, hinges) are skipped by their vtable.
pub fn contacts(m: &dyn GuestMemory) -> Result<Vec<Contact>, MemFault> {
    let Some(w) = world(m)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (island, _) in islands(m, w)? {
        let (ptr, count) = array(m, island + layout::ISLAND_CONSTRAINTS, MAX_CONSTRAINTS)?;
        for i in 0..count {
            let c = m.u32(ptr + 4 * i)?;
            if c == 0 || m.u32(c)? != layout::CONTACT_CONSTRAINT_VTABLE {
                continue;
            }
            out.push(Contact {
                constraint: c,
                a: m.u32(c + layout::CONSTRAINT_ENTITY_A)?,
                b: m.u32(c + layout::CONSTRAINT_ENTITY_B)?,
                points: points(m, c)?,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::{Guest, guest::Permissions};

    const WORLD: u32 = 0x10_0000;
    const ISLANDS: u32 = 0x10_1000;
    const COMPONENTS: u32 = 0x10_2000;
    const COMPONENT_DATA: u32 = 0x10_3000;
    /// Fake vtables (only their type getter slot) and shapes.
    const VTABLES: u32 = 0x14_0000;
    const SHAPES: u32 = 0x14_1000;
    /// Island `i` at `ISLAND + i × 0x1000`, entity `i` at `ENTITY + i ×
    /// 0x1000` (its motion at `+0x400`, properties at `+0x800`).
    const ISLAND: u32 = 0x11_0000;
    const ENTITY: u32 = 0x12_0000;
    const CONSTRAINT: u32 = 0x13_0000;

    fn memory() -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(0x10_0000, 0x10_0000, Permissions::RW).unwrap();
        g.map_zeroed(0x50_0000, 0x10_0000, Permissions::RW).unwrap();
        g
    }

    fn vec3(g: &mut Guest, at: u32, v: [f32; 3]) {
        for (i, c) in v.into_iter().enumerate() {
            g.set_f32(at + 4 * i as u32, c).unwrap();
        }
    }

    fn island(i: u32) -> u32 {
        ISLAND + i * 0x1000
    }

    fn entity(i: u32) -> u32 {
        ENTITY + i * 0x1000
    }

    /// Entity `i`, owned by physics component `component` (none for
    /// `u32::MAX`).
    fn put_entity(g: &mut Guest, i: u32, component: u32, pos: [f32; 3], vel: [f32; 3]) {
        let e = entity(i);
        let motion = e + 0x400;
        g.set_u32(e + layout::RIGID_MOTION, motion).unwrap();
        vec3(g, motion + layout::MOTION_POSITION, pos);
        vec3(g, motion + layout::MOTION_VELOCITY, vel);
        vec3(g, motion + layout::MOTION_ANGULAR_VELOCITY, [0.0, 0.0, 1.5]);
        g.set_f32(motion + layout::MOTION_INV_MASS, 0.0125).unwrap();
        g.set_f32(e + layout::ENTITY_FRICTION, 0.6).unwrap();
        g.set_f32(e + layout::ENTITY_RESTITUTION, 0.1).unwrap();
        // Two properties, the component one second.
        let props = e + 0x800;
        g.set_u32(e + layout::ENTITY_PROPERTIES, props).unwrap();
        g.set_u32(e + layout::ENTITY_PROPERTIES + 4, 2).unwrap();
        g.set_u32(props, 0x1234).unwrap();
        g.set_u32(props + 8, layout::PROPERTY_PHYSICS_COMPONENT)
            .unwrap();
        g.set_u32(props + 12, component).unwrap();
    }

    fn put_island(g: &mut Guest, i: u32, entities: &[u32], constraints: &[u32]) {
        let isl = island(i);
        let list = isl + 0x200;
        g.set_u32(isl + layout::ISLAND_ENTITIES, list).unwrap();
        g.set_u32(isl + layout::ISLAND_ENTITIES + 4, entities.len() as u32)
            .unwrap();
        for (n, e) in entities.iter().enumerate() {
            g.set_u32(list + 4 * n as u32, entity(*e)).unwrap();
        }
        let list = isl + 0x400;
        g.set_u32(isl + layout::ISLAND_CONSTRAINTS, list).unwrap();
        g.set_u32(
            isl + layout::ISLAND_CONSTRAINTS + 4,
            constraints.len() as u32,
        )
        .unwrap();
        for (n, c) in constraints.iter().enumerate() {
            g.set_u32(list + 4 * n as u32, *c).unwrap();
        }
    }

    /// A world: island 0 active (entity 0, a biped of object 0xe0010001,
    /// and the contact constraint), island 1 inactive (entity 1, a crate
    /// of object 0xe0020002), island 2 fixed (entity 2, the level: no
    /// component).
    fn world() -> Guest {
        let mut g = memory();
        g.set_u32(layout::HAVOK_WORLD_PTR, WORLD).unwrap();
        g.set_u32(WORLD + layout::WORLD_ACTIVE_ISLANDS, ISLANDS)
            .unwrap();
        g.set_u32(WORLD + layout::WORLD_ACTIVE_ISLANDS + 4, 1)
            .unwrap();
        g.set_u32(ISLANDS, island(0)).unwrap();
        g.set_u32(WORLD + layout::WORLD_INACTIVE_ISLANDS, ISLANDS + 0x10)
            .unwrap();
        g.set_u32(WORLD + layout::WORLD_INACTIVE_ISLANDS + 4, 1)
            .unwrap();
        g.set_u32(ISLANDS + 0x10, island(1)).unwrap();
        g.set_u32(WORLD + layout::WORLD_FIXED_ISLAND, island(2))
            .unwrap();

        g.set_u32(layout::PHYSICS_COMPONENTS_PTR, COMPONENTS)
            .unwrap();
        g.set_u32(COMPONENTS + layout::DATUM_DATA, COMPONENT_DATA)
            .unwrap();
        for (index, object) in [(3, 0xe001_0001), (5, 0xe002_0002)] {
            g.set_u32(
                COMPONENT_DATA + index * layout::PHYSICS_COMPONENT_SIZE + layout::COMPONENT_OBJECT,
                object,
            )
            .unwrap();
        }

        put_entity(&mut g, 0, 0xab00_0003, [-10.5, 3.9, -3.3], [0.0, 1.0, 11.0]);
        put_entity(&mut g, 1, 0xab00_0005, [1.0, 2.0, 3.0], [0.0; 3]);
        put_entity(&mut g, 2, u32::MAX, [0.0; 3], [0.0; 3]);

        // A ragdoll joint (another vtable) and the contact constraint.
        let joint = CONSTRAINT + 0x800;
        g.set_u32(joint, 0x0041_0000).unwrap();
        let c = CONSTRAINT;
        g.set_u32(c, layout::CONTACT_CONSTRAINT_VTABLE).unwrap();
        g.set_u32(c + layout::CONSTRAINT_ENTITY_A, entity(2))
            .unwrap();
        g.set_u32(c + layout::CONSTRAINT_ENTITY_B, entity(0))
            .unwrap();
        let pts = c + 0x200;
        let props = c + 0x400;
        g.set_u32(c + layout::CONTACT_POINTS, pts).unwrap();
        g.set_u32(c + layout::CONTACT_POINTS + 4, 2).unwrap();
        // Three properties: one more than points, as the array can hold.
        g.set_u32(c + layout::CONTACT_PROPERTIES, props).unwrap();
        g.set_u32(c + layout::CONTACT_PROPERTIES + 4, 3).unwrap();
        for i in 0..2u32 {
            let p = pts + i * layout::CONTACT_POINT_SIZE;
            vec3(&mut g, p, [-10.5, 3.8 + 0.2 * i as f32, -3.2]);
            g.set_f32(p + layout::POINT_DISTANCE, -0.07 * (i + 1) as f32)
                .unwrap();
            vec3(&mut g, p + layout::POINT_NORMAL, [0.0, 0.0, 1.0]);
            let q = props + i * layout::CONTACT_PROPERTY_SIZE;
            g.set_f32(q + layout::PROPERTY_IMPULSE, 2.5 * (i + 1) as f32)
                .unwrap();
            g.set_f32(q + layout::PROPERTY_FRICTION, 0.5).unwrap();
            g.set_f32(q + layout::PROPERTY_RESTITUTION, 0.25).unwrap();
        }
        put_island(&mut g, 0, &[0], &[joint, c]);
        put_island(&mut g, 1, &[1], &[]);
        put_island(&mut g, 2, &[2], &[]);
        g
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn reads_bodies_by_island_with_their_objects() {
        let g = world();
        let b = bodies(&g).unwrap();
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].entity, entity(0));
        assert_eq!(b[0].state, IslandState::Active);
        assert_eq!(b[0].object, Some(0xe001_0001));
        assert!(close(b[0].pos[2], -3.3));
        assert!(close(b[0].velocity[2], 11.0));
        assert!(close(b[0].angular_velocity[2], 1.5));
        assert!(close(b[0].inv_mass, 0.0125));
        assert!(close(b[0].friction, 0.6));
        assert!(close(b[0].restitution, 0.1));
        assert_eq!(b[1].state, IslandState::Inactive);
        assert_eq!(b[1].object, Some(0xe002_0002));
        assert_eq!(b[2].state, IslandState::Fixed);
        assert_eq!(b[2].object, None);
    }

    #[test]
    fn reads_contact_constraints_and_skips_other_constraints() {
        let g = world();
        let c = contacts(&g).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].constraint, CONSTRAINT);
        assert_eq!((c[0].a, c[0].b), (entity(2), entity(0)));
        let p = &c[0].points;
        assert_eq!(p.len(), 2);
        assert!(close(p[1].pos[1], 4.0));
        assert!(close(p[1].distance, -0.14));
        assert_eq!(p[1].normal, [0.0, 0.0, 1.0]);
        assert!(close(p[1].impulse, 5.0));
        assert!(close(p[0].friction, 0.5));
        assert!(close(p[0].restitution, 0.25));
    }

    #[test]
    fn no_world_is_empty_and_a_torn_count_is_an_error() {
        let mut g = memory();
        assert!(bodies(&g).unwrap().is_empty());
        assert!(contacts(&g).unwrap().is_empty());

        let mut g2 = world();
        g2.set_u32(WORLD + layout::WORLD_ACTIVE_ISLANDS + 4, 1 << 20)
            .unwrap();
        assert!(bodies(&g2).is_err());

        // Fewer properties than points: the pair is mid-update.
        g = world();
        g.set_u32(CONSTRAINT + layout::CONTACT_PROPERTIES + 4, 1)
            .unwrap();
        assert!(contacts(&g).is_err());
    }

    #[test]
    fn an_entity_without_the_component_property_has_no_object() {
        let mut g = world();
        g.set_u32(entity(0) + 0x800 + 8, 0x2002).unwrap();
        assert_eq!(entity_object(&g, entity(0)).unwrap(), None);
    }

    /// A vtable at `VTABLES + 0x100 × n` whose type getter is `getter`.
    fn vtable(g: &mut Guest, n: u32, getter: u32) -> u32 {
        let v = VTABLES + 0x100 * n;
        g.set_u32(v + layout::SHAPE_TYPE_SLOT, getter).unwrap();
        v
    }

    #[test]
    fn reads_a_capsule_and_a_transformed_sphere() {
        let mut g = world();
        let capsule = vtable(&mut g, 0, layout::SHAPE_TYPE_CAPSULE_FN);
        let sphere = vtable(&mut g, 1, layout::SHAPE_TYPE_SPHERE_FN);
        let transform = vtable(&mut g, 2, layout::SHAPE_TYPE_TRANSFORM_FN);
        let other = vtable(&mut g, 3, 0x0012_3450);

        // Entity 0: a capsule from (0, 0, 0.175) to (0, 0, 0.55).
        let pill = SHAPES;
        g.set_u32(pill, capsule).unwrap();
        g.set_f32(pill + layout::SHAPE_RADIUS, 0.175).unwrap();
        vec3(&mut g, pill + layout::CAPSULE_A, [0.0, 0.0, 0.175]);
        vec3(&mut g, pill + layout::CAPSULE_B, [0.0, 0.0, 0.55]);
        g.set_u32(entity(0) + layout::ENTITY_SHAPE, pill).unwrap();

        // Entity 1: a transform lifting a sphere of radius 0.3.
        let (t, ball) = (SHAPES + 0x100, SHAPES + 0x200);
        g.set_u32(ball, sphere).unwrap();
        g.set_f32(ball + layout::SHAPE_RADIUS, 0.3).unwrap();
        g.set_u32(t, transform).unwrap();
        g.set_u32(t + layout::TRANSFORM_CHILD, ball).unwrap();
        vec3(&mut g, t + layout::TRANSFORM_TRANSLATION, [0.0, 0.0, 0.3]);
        g.set_u32(entity(1) + layout::ENTITY_SHAPE, t).unwrap();

        // Entity 2: a type not decoded.
        g.set_u32(SHAPES + 0x300, other).unwrap();
        g.set_u32(entity(2) + layout::ENTITY_SHAPE, SHAPES + 0x300)
            .unwrap();

        let b = bodies(&g).unwrap();
        let s = b[0].shape.as_ref().unwrap();
        assert_eq!((s.addr, s.vtable), (pill, capsule));
        match &s.kind {
            ShapeKind::Capsule { a, b, radius } => {
                assert!(close(a[2], 0.175) && close(b[2], 0.55));
                assert!(close(*radius, 0.175));
            }
            k => panic!("{k:?}"),
        }
        match &b[1].shape.as_ref().unwrap().kind {
            ShapeKind::Transform {
                translation,
                child,
                child_shape,
            } => {
                assert!(close(translation[2], 0.3));
                assert_eq!(*child, ball);
                let c = child_shape.as_ref().unwrap();
                assert_eq!(
                    c.kind,
                    ShapeKind::Sphere {
                        radius: 0.3f32.into()
                    }
                );
            }
            k => panic!("{k:?}"),
        }
        assert_eq!(b[2].shape.as_ref().unwrap().kind, ShapeKind::Other);
    }

    #[test]
    fn a_shape_that_does_not_read_leaves_the_body() {
        let mut g = world();
        // No shape: none. An unmapped one: none, the body still reads.
        assert_eq!(bodies(&g).unwrap()[0].shape, None);
        g.set_u32(entity(0) + layout::ENTITY_SHAPE, 0x0300_0000)
            .unwrap();
        let b = bodies(&g).unwrap();
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].shape, None);
    }
}
