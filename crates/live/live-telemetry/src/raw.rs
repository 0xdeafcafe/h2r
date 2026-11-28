//! An object's state as raw guest bytes, for diffing one moment against
//! another (`world.raw`, docs/runtime/agent-api.md): the object's body, its
//! physics component, the Havok rigid body (entity) behind it, the body's
//! motion and the collidable's shape (docs/engine/havok-runtime.md).
//!
//! Each part is found through the one before it. A part whose pointer is
//! unset or does not read is left out with the reason, not an error.

use guest_layout::view::objects;
use guest_layout::{DatumHandle, GuestMemory, MemFault};

use crate::layout;

/// The parts, in the order they are found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Part {
    /// The object's body (its full state), as long as its header says.
    Object,
    /// Its physics component record (`layout::PHYSICS_COMPONENT_SIZE`).
    PhysicsComponent,
    /// The component's first rigid body: the Havok entity.
    Entity,
    /// The entity's motion (velocities, transform).
    Motion,
    /// The entity's collidable shape.
    Shape,
}

impl Part {
    pub const ALL: [Part; 5] = [
        Part::Object,
        Part::PhysicsComponent,
        Part::Entity,
        Part::Motion,
        Part::Shape,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Part::Object => "object",
            Part::PhysicsComponent => "physics_component",
            Part::Entity => "entity",
            Part::Motion => "motion",
            Part::Shape => "shape",
        }
    }

    pub fn parse(s: &str) -> Option<Part> {
        Part::ALL.into_iter().find(|p| p.as_str() == s)
    }

    /// Bytes read for this part. The object's body is as long as its
    /// header says, when it says.
    pub fn size(self) -> u32 {
        match self {
            Part::Object => OBJECT_LEN,
            Part::PhysicsComponent => layout::PHYSICS_COMPONENT_SIZE,
            Part::Entity => 0x200,
            Part::Motion => 0x140,
            Part::Shape => 0x80,
        }
    }
}

/// The object's length when its header has none and none is asked for.
pub const OBJECT_LEN: u32 = 0x400;
/// At most this many bytes of the object.
pub const OBJECT_MAX: u32 = 0x2000;

/// One part's bytes, or why there are none.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub part: Part,
    pub read: Result<(u32, Vec<u8>), String>,
}

/// Where each part is, or why it is not known.
struct Chain {
    object: Result<(u32, u32), String>,
    component: Result<u32, String>,
    entity: Result<u32, String>,
    motion: Result<u32, String>,
    shape: Result<u32, String>,
}

fn fault(what: &str, e: MemFault) -> String {
    format!("reading {what} at {:08x} faulted", e.addr)
}

/// A pointer at `at`: an error naming `what` when it faults or is null.
fn pointer(m: &dyn GuestMemory, at: u32, what: &str) -> Result<u32, String> {
    match m.u32(at) {
        Ok(0) => Err(format!("no {what} (null at {at:08x})")),
        Ok(p) => Ok(p),
        Err(e) => Err(fault(what, e)),
    }
}

/// The object's body and its length: the header's size, else `len`, else
/// [`OBJECT_LEN`]; at most [`OBJECT_MAX`].
fn object(m: &dyn GuestMemory, header: objects::Header, len: Option<u32>) -> (u32, u32) {
    let body = header.body(m).unwrap_or(0);
    let size = m
        .u16(header.addr + layout::OBJECT_BODY_SIZE)
        .map(u32::from)
        .unwrap_or(0);
    let n = if size != 0 {
        size
    } else {
        len.unwrap_or(OBJECT_LEN)
    };
    (body, n.min(OBJECT_MAX))
}

/// The physics component of the object whose body is at `body`
/// (`layout::BODY_PHYSICS`, as `0x001d08e0` finds it).
fn component(m: &dyn GuestMemory, body: u32) -> Result<u32, String> {
    let index = m
        .u32(body + layout::BODY_PHYSICS)
        .map_err(|e| fault("the physics component index", e))?;
    if index == u32::MAX {
        return Err("no physics component (index -1)".into());
    }
    let array = pointer(m, layout::PHYSICS_COMPONENTS_PTR, "physics component array")?;
    let data = pointer(m, array + layout::DATUM_DATA, "physics component data")?;
    Ok(data.wrapping_add((index & 0xffff) * layout::PHYSICS_COMPONENT_SIZE))
}

/// The component's first rigid body.
fn entity(m: &dyn GuestMemory, component: u32) -> Result<u32, String> {
    let count = m
        .u32(component + layout::COMPONENT_BODY_COUNT)
        .map_err(|e| fault("the rigid body count", e))?;
    if count == 0 || count > 0x100 {
        return Err(format!("rigid body count {count}"));
    }
    let bodies = pointer(m, component + layout::COMPONENT_BODIES, "rigid body array")?;
    pointer(m, bodies + layout::COMPONENT_BODY_RIGID, "rigid body")
}

fn chain(m: &dyn GuestMemory, header: objects::Header, len: Option<u32>) -> Chain {
    let (body, n) = object(m, header, len);
    let object = if body == 0 {
        Err("no body".to_owned())
    } else {
        Ok((body, n))
    };
    let component = object.clone().and_then(|(b, _)| component(m, b));
    let entity = component.clone().and_then(|c| entity(m, c));
    let motion = entity
        .clone()
        .and_then(|e| pointer(m, e + layout::RIGID_MOTION, "motion"));
    let shape = entity
        .clone()
        .and_then(|e| pointer(m, e + layout::ENTITY_SHAPE, "shape"));
    Chain {
        object,
        component,
        entity,
        motion,
        shape,
    }
}

fn bytes(m: &dyn GuestMemory, part: Part, at: Result<u32, String>, n: u32) -> Region {
    let read = at.and_then(|a| {
        m.bytes(a, n as usize)
            .map(|b| (a, b))
            .map_err(|e| fault(part.as_str(), e))
    });
    Region { part, read }
}

/// The `parts` (in [`Part::ALL`]'s order, each once) of the object
/// `datum`; `None` when it is not a live object with a body. `len` is the
/// object's length when its header has none.
pub fn object_parts(
    m: &dyn GuestMemory,
    datum: u32,
    parts: &[Part],
    len: Option<u32>,
) -> Result<Option<Vec<Region>>, MemFault> {
    let Some(handle) = DatumHandle::from_raw(datum) else {
        return Ok(None);
    };
    let Some(table) = objects::array(m)? else {
        return Ok(None);
    };
    let Some(header) = table.get(m, handle)? else {
        return Ok(None);
    };
    if header.body(m)? == 0 {
        return Ok(None);
    }
    let c = chain(m, header, len);
    let out = Part::ALL
        .into_iter()
        .filter(|p| parts.contains(p))
        .map(|p| match p {
            Part::Object => match &c.object {
                Ok((a, n)) => bytes(m, p, Ok(*a), *n),
                Err(e) => bytes(m, p, Err(e.clone()), 0),
            },
            Part::PhysicsComponent => bytes(m, p, c.component.clone(), p.size()),
            Part::Entity => bytes(m, p, c.entity.clone(), p.size()),
            Part::Motion => bytes(m, p, c.motion.clone(), p.size()),
            Part::Shape => bytes(m, p, c.shape.clone(), p.size()),
        })
        .collect();
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;
    use guest_layout::view::fake;

    const OBJECTS_HDR: u32 = 0x8010_0000;
    const BODY: u32 = 0x8020_0000;
    const COMPONENTS_HDR: u32 = 0x8030_0000;
    const COMPONENT_DATA: u32 = 0x8031_0000;
    const RIGIDS: u32 = 0x8040_0000;
    const ENTITY: u32 = 0x8041_0000;
    const MOTION: u32 = 0x8042_0000;
    const SHAPE: u32 = 0x8043_0000;

    /// An object with a body of `size` bytes (0: the header says none),
    /// physics component 2, one rigid body with a motion and a shape. Each
    /// region starts with a marker word.
    fn game(size: u16) -> (FakeGuest, u32) {
        let mut g = FakeGuest::new();
        g.map(guest_layout::points::OBJECTS.base, 4);
        g.put_u32(guest_layout::points::OBJECTS.base, OBJECTS_HDR);
        let table = fake::datums::<objects::Header>(&mut g, OBJECTS_HDR, 16);
        let (h, hdr) = fake::push(&mut g, &table);
        g.map(BODY, 0x3000);
        hdr.set_body(&mut g, BODY).unwrap();
        g.put_u16(hdr.addr + layout::OBJECT_BODY_SIZE, size);
        g.put_u32(BODY, 0x0b0d_0000);
        g.put_u32(BODY + layout::BODY_PHYSICS, 0xab00_0002);

        g.put_u32(layout::PHYSICS_COMPONENTS_PTR, COMPONENTS_HDR);
        g.put_u32(COMPONENTS_HDR + layout::DATUM_DATA, COMPONENT_DATA);
        let comp = COMPONENT_DATA + 2 * layout::PHYSICS_COMPONENT_SIZE;
        g.map(COMPONENT_DATA, 4 * layout::PHYSICS_COMPONENT_SIZE);
        g.put_u32(comp, 0xc0c0_0000);
        g.put_u32(comp + layout::COMPONENT_BODIES, RIGIDS);
        g.put_u32(comp + layout::COMPONENT_BODY_COUNT, 1);
        g.put_u32(RIGIDS + layout::COMPONENT_BODY_RIGID, ENTITY);
        g.map(ENTITY, 0x200);
        g.put_u32(ENTITY, 0x0e0e_0000);
        g.put_u32(ENTITY + layout::RIGID_MOTION, MOTION);
        g.put_u32(ENTITY + layout::ENTITY_SHAPE, SHAPE);
        g.map(MOTION, 0x140);
        g.put_f32(MOTION + layout::MOTION_POSITION, -10.5);
        g.map(SHAPE, 0x80);
        g.put_f32(SHAPE + layout::SHAPE_RADIUS, 0.175);
        (g, h.raw())
    }

    fn read(r: &[Region], p: Part) -> &(u32, Vec<u8>) {
        let region = r.iter().find(|x| x.part == p).expect("asked for");
        region.read.as_ref().expect("read")
    }

    fn word(b: &[u8], at: u32) -> u32 {
        let i = at as usize;
        u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
    }

    #[test]
    fn reads_every_part_through_the_chain() {
        let (g, datum) = game(0x464);
        let r = object_parts(&g, datum, &Part::ALL, None).unwrap().unwrap();
        let got: Vec<Part> = r.iter().map(|x| x.part).collect();
        assert_eq!(got, Part::ALL);

        let (at, b) = read(&r, Part::Object);
        assert_eq!((*at, b.len()), (BODY, 0x464));
        assert_eq!(word(b, 0), 0x0b0d_0000);
        let (at, b) = read(&r, Part::PhysicsComponent);
        assert_eq!(*at, COMPONENT_DATA + 2 * layout::PHYSICS_COMPONENT_SIZE);
        assert_eq!((b.len(), word(b, 0)), (0xa0, 0xc0c0_0000));
        let (at, b) = read(&r, Part::Entity);
        assert_eq!((*at, b.len(), word(b, 0)), (ENTITY, 0x200, 0x0e0e_0000));
        let (at, b) = read(&r, Part::Motion);
        assert_eq!((*at, b.len()), (MOTION, 0x140));
        assert_eq!(f32::from_bits(word(b, layout::MOTION_POSITION)), -10.5);
        let (at, b) = read(&r, Part::Shape);
        assert_eq!((*at, b.len()), (SHAPE, 0x80));
        assert_eq!(f32::from_bits(word(b, layout::SHAPE_RADIUS)), 0.175);
    }

    #[test]
    fn the_object_length_is_the_headers_else_len_and_capped() {
        let (g, datum) = game(0);
        let only = [Part::Object];
        let r = object_parts(&g, datum, &only, None).unwrap().unwrap();
        assert_eq!(read(&r, Part::Object).1.len(), OBJECT_LEN as usize);
        let r = object_parts(&g, datum, &only, Some(0x20)).unwrap().unwrap();
        assert_eq!(read(&r, Part::Object).1.len(), 0x20);
        let r = object_parts(&g, datum, &only, Some(0x10_0000))
            .unwrap()
            .unwrap();
        assert_eq!(read(&r, Part::Object).1.len(), OBJECT_MAX as usize);
    }

    #[test]
    fn a_missing_link_drops_the_parts_after_it_with_a_reason() {
        let (mut g, datum) = game(0x464);
        // No shape: the rest still reads.
        g.put_u32(ENTITY + layout::ENTITY_SHAPE, 0);
        let r = object_parts(&g, datum, &Part::ALL, None).unwrap().unwrap();
        let shape = r.iter().find(|x| x.part == Part::Shape).unwrap();
        assert!(shape.read.as_ref().unwrap_err().contains("no shape"));
        assert!(r[3].read.is_ok());

        // No physics component: only the object.
        g.put_u32(BODY + layout::BODY_PHYSICS, u32::MAX);
        let r = object_parts(&g, datum, &Part::ALL, None).unwrap().unwrap();
        assert!(r[0].read.is_ok());
        for x in &r[1..] {
            assert!(x.read.is_err(), "{:?}", x.part);
        }
        assert!(r[1].read.as_ref().unwrap_err().contains("index -1"));

        // A motion pointer into nothing: that part faults, by name.
        let (mut g, datum) = game(0x464);
        g.put_u32(ENTITY + layout::RIGID_MOTION, 0x0900_0000);
        let only = [Part::Motion, Part::Entity];
        let r = object_parts(&g, datum, &only, None).unwrap().unwrap();
        assert_eq!(r[0].part, Part::Entity);
        let e = r[1].read.as_ref().unwrap_err();
        assert!(e.contains("motion") && e.contains("09000000"), "{e}");
    }

    #[test]
    fn a_stale_or_unknown_datum_is_none() {
        let (g, datum) = game(0x464);
        let stale = datum ^ 0x0001_0000;
        assert_eq!(object_parts(&g, stale, &Part::ALL, None).unwrap(), None);
        assert_eq!(object_parts(&g, u32::MAX, &Part::ALL, None).unwrap(), None);
        // No objects array at all.
        let g = FakeGuest::new();
        assert!(object_parts(&g, datum, &Part::ALL, None).is_err());
    }

    #[test]
    fn parts_parse_by_name() {
        for p in Part::ALL {
            assert_eq!(Part::parse(p.as_str()), Some(p));
        }
        assert_eq!(Part::parse("havok"), None);
    }
}
