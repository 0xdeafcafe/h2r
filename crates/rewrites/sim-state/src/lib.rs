//! Typed, read-only views of the simulation for consumers that are not
//! the simulation: theater, stats, Live, the agent API
//! (docs/engine/sim-rewrite-objects.md section 8, docs/engine/sim-rewrite.md
//! section 7).
//!
//! Read on the game thread at tick end, never from a worker and never
//! mid-tick. Views borrow guest memory for the call and cache nothing, so
//! a checkpoint revert or a keyframe load needs no invalidation here.
//! Offsets come from `guest_layout`; this crate adds typed ids and kinds,
//! not offsets.

mod actors;
pub use actors::{ActorControl, ActorView, visit_actors, visit_ai_units};

use guest_layout::object::{NODE_MATRIX_SIZE, header};
use guest_layout::{Datum, Fault, Mem, Off, Unit};
use sim_objects::view::{Children as ChildLinks, Slots};
use sim_objects::{ObjRef, Objects};

/// An object's kind: the type table's order.
pub use guest_layout::ObjectType as ObjectKind;

/// A live object's salted handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(pub u32);

impl ObjectId {
    pub const fn datum(self) -> Datum {
        Datum(self.0)
    }
    /// The slot in the objects pool.
    pub const fn index(self) -> u16 {
        self.0 as u16
    }
}

impl From<Datum> for ObjectId {
    fn from(d: Datum) -> Self {
        ObjectId(d.raw())
    }
}

/// The header flags the walks select objects by
/// (`guest_layout::object::header`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObjectFlags {
    pub active: bool,
    pub awake: bool,
    /// Due a post-physics update.
    pub post: bool,
    /// Made or woken this tick.
    pub new: bool,
    /// Deleted at the end of this tick.
    pub deleting: bool,
    pub defer: bool,
    pub attach: bool,
    /// Attached to a parent.
    pub child: bool,
}

impl ObjectFlags {
    pub const fn from_bits(f: u8) -> ObjectFlags {
        ObjectFlags {
            active: f & header::ACTIVE != 0,
            awake: f & header::AWAKE != 0,
            post: f & header::POST != 0,
            new: f & header::NEW != 0,
            deleting: f & header::DELETE != 0,
            defer: f & header::DEFER != 0,
            attach: f & header::ATTACH != 0,
            child: f & header::CHILD != 0,
        }
    }
}

/// Position, basis and scale. A parented object's are relative to its
/// parent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
    pub scale: f32,
}

/// Body and shield vitality, 1.0 full, and the damage flags.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vitality {
    pub body: f32,
    pub shield: f32,
    pub flags: u16,
}

impl Vitality {
    /// The body is depleted (`guest_layout::object::DEPLETED`).
    pub const fn depleted(&self) -> bool {
        self.flags & guest_layout::object::DEPLETED != 0
    }
}

/// A bounding sphere, world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sphere {
    pub center: [f32; 3],
    pub radius: f32,
}

/// One node matrix as the guest stores it: scale, three axes, position
/// (13 floats, world space).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NodeMatrix(pub [f32; 13]);

/// Every live object, in slot order. Empty while no map is loaded.
pub fn objects<M: Mem + ?Sized>(m: &M) -> Result<ObjectIter<'_, M>, Fault> {
    let pool = Objects::read(m)?;
    Ok(ObjectIter {
        m,
        inner: pool.map(|p| (p, p.iter(m))),
    })
}

/// The live object `id` names, `None` for a stale or absent id.
pub fn object<M: Mem + ?Sized>(m: &M, id: ObjectId) -> Result<Option<ObjectView<'_, M>>, Fault> {
    let Some(pool) = Objects::read(m)? else {
        return Ok(None);
    };
    Ok(pool.resolve(m, id.datum())?.map(|obj| ObjectView { m, pool, obj }))
}

/// Visit the listed players in slot order, including players between lives.
pub fn visit_players(
    m: &dyn guest_layout::GuestMemory,
    mut visit: impl FnMut(u16, guest_layout::Player) -> Result<(), Fault>,
) -> Result<(), Fault> {
    use guest_layout::view::players;

    if let Some(array) = players::array(m)? {
        for row in array.live_iter(m, 16)? {
            let (handle, slot) = row?;
            let player = guest_layout::Player::at(slot.addr);
            // live_iter already excludes free slots.
            if player.flags(m)? & guest_layout::player::LEAVING == 0 {
                visit(handle.index(), player)?;
            }
        }
    }
    Ok(())
}

/// One event of the tick: `ObjectDeleted` from finalize's delete pass,
/// [`Damage`] from the damage report, [`Kill`] from the kill credit.
pub use sim_objects::{Damage, Kill, SimEvent};

/// This tick's events so far, in the order they happened. Read at tick end
/// on the game thread: the buffer belongs to the thread that runs the
/// simulation, and it is cleared when a tick starts (`GAME_TICK`'s entry).
/// `ObjectDeleted` from finalize, `Damage`, `Kill` and `ActorKilled` from
/// the damage report and kill credit; all always recorded.
pub fn events<R>(f: impl FnOnce(&[SimEvent]) -> R) -> R {
    sim_objects::events::with(f)
}

/// [`objects`].
pub struct ObjectIter<'a, M: ?Sized> {
    m: &'a M,
    inner: Option<(Objects, Slots<'a, M>)>,
}

impl<'a, M: Mem + ?Sized> Iterator for ObjectIter<'a, M> {
    type Item = Result<ObjectView<'a, M>, Fault>;

    fn next(&mut self) -> Option<Self::Item> {
        let (pool, slots) = self.inner.as_mut()?;
        let pool = *pool;
        let m = self.m;
        slots.next().map(|r| r.map(|obj| ObjectView { m, pool, obj }))
    }
}

/// One live object, read through guest memory when asked.
pub struct ObjectView<'a, M: ?Sized> {
    m: &'a M,
    pool: Objects,
    obj: ObjRef,
}

impl<M: ?Sized> Clone for ObjectView<'_, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: ?Sized> Copy for ObjectView<'_, M> {}

impl<'a, M: Mem + ?Sized> ObjectView<'a, M> {
    pub fn id(&self) -> ObjectId {
        self.obj.handle.into()
    }

    /// The header type byte, including values outside the known type table.
    pub fn kind_byte(&self) -> Result<u8, Fault> {
        self.obj.header.kind(self.m)
    }

    /// The kind, `None` for a value outside the type table.
    pub fn kind(&self) -> Result<Option<ObjectKind>, Fault> {
        self.obj.kind(self.m)
    }

    pub fn flag_bits(&self) -> Result<u8, Fault> {
        self.obj.flags(self.m)
    }

    pub fn flags(&self) -> Result<ObjectFlags, Fault> {
        self.flag_bits().map(ObjectFlags::from_bits)
    }

    /// The index in this kind's scenario placement block.
    pub fn placement(&self) -> Result<Option<u16>, Fault> {
        Ok(u16::try_from(self.obj.body.placement_index(self.m)?).ok())
    }

    /// The definition tag.
    pub fn definition(&self) -> Result<Datum, Fault> {
        self.obj.body.definition(self.m)
    }

    /// The object it is attached to.
    pub fn parent(&self) -> Result<Option<ObjectId>, Fault> {
        Ok(self.obj.body.parent(self.m)?.get().map(ObjectId))
    }

    /// Its attached children: first child, then siblings.
    pub fn children(&self) -> Children<'a, M> {
        Children {
            m: self.m,
            pool: self.pool,
            links: self.pool.children(self.m, self.obj),
        }
    }

    pub fn transform(&self) -> Result<Transform, Fault> {
        let (b, m) = (self.obj.body, self.m);
        Ok(Transform {
            position: b.position(m)?,
            forward: b.forward(m)?,
            up: b.up(m)?,
            scale: b.scale(m)?,
        })
    }

    /// World units a second.
    pub fn velocity(&self) -> Result<[f32; 3], Fault> {
        self.obj.body.velocity(self.m)
    }

    pub fn bounding_sphere(&self) -> Result<Sphere, Fault> {
        Ok(Sphere {
            center: self.obj.body.bound_center(self.m)?,
            radius: self.obj.body.bound_radius(self.m)?,
        })
    }

    pub fn vitality(&self) -> Result<Vitality, Fault> {
        let (b, m) = (self.obj.body, self.m);
        Ok(Vitality {
            body: b.health(m)?,
            shield: b.shield(m)?,
            flags: b.damage_flags(m)?,
        })
    }

    /// Copy the active render permutation for each model region. The block
    /// holds ten bytes per region; the selections occupy its first column.
    pub fn region_permutations(&self, out: &mut [u8]) -> Result<usize, Fault> {
        let body = self.obj.body;
        let count = usize::from(body.regions_size(self.m)?) / 10;
        let offset = body.regions_offset(self.m)?;
        if offset <= 0 || count == 0 {
            return Ok(0);
        }
        let n = count.min(out.len());
        self.m
            .read_bytes(body.addr.wrapping_add(offset as u32), &mut out[..n])?;
        Ok(n)
    }

    /// Its node matrices: where and how many, `None` when it has none.
    fn node_block(&self) -> Result<Option<(u32, u32)>, Fault> {
        self.obj.body.nodes(self.m)
    }

    pub fn node_count(&self) -> Result<usize, Fault> {
        Ok(self.node_block()?.map_or(0, |(_, n)| n as usize))
    }

    /// Copy the node matrices into `out`, as many as fit, and return how
    /// many the object has. They are already world space and final for
    /// the tick: a pose is this copy.
    pub fn nodes(&self, out: &mut [NodeMatrix]) -> Result<usize, Fault> {
        let Some((at, n)) = self.node_block()? else {
            return Ok(0);
        };
        let matrix = Off::<[f32; 13]>::new(0);
        for (i, slot) in out.iter_mut().take(n as usize).enumerate() {
            *slot = NodeMatrix(matrix.read(self.m, at.wrapping_add(i as u32 * NODE_MATRIX_SIZE))?);
        }
        Ok(n as usize)
    }

    /// The unit fields, for bipeds and vehicles.
    pub fn as_unit(&self) -> Result<Option<UnitView<'a, M>>, Fault> {
        Ok(self
            .kind()?
            .filter(|k| k.is_unit())
            .map(|_| UnitView {
                m: self.m,
                unit: self.obj.body.as_unit(),
            }))
    }
}

/// [`ObjectView::children`].
pub struct Children<'a, M: ?Sized> {
    m: &'a M,
    pool: Objects,
    links: ChildLinks<'a, M>,
}

impl<'a, M: Mem + ?Sized> Iterator for Children<'a, M> {
    type Item = Result<ObjectView<'a, M>, Fault>;

    fn next(&mut self) -> Option<Self::Item> {
        let (m, pool) = (self.m, self.pool);
        self.links.next().map(|r| r.map(|obj| ObjectView { m, pool, obj }))
    }
}

/// A unit's fields (bipeds and vehicles; docs/live/live-gamestate.md
/// "Unit aim, held weapon").
pub struct UnitView<'a, M: ?Sized> {
    m: &'a M,
    unit: Unit,
}

fn some(d: Datum) -> Option<ObjectId> {
    d.get().map(ObjectId)
}

impl<M: Mem + ?Sized> UnitView<'_, M> {
    /// The controlling player's datum.
    pub fn player(&self) -> Result<Option<Datum>, Fault> {
        Ok(self.unit.player(self.m)?.get().map(Datum))
    }

    /// The campaign actor driving it, if AI drives it.
    pub fn actor(&self) -> Result<Option<Datum>, Fault> {
        Ok(self.unit.actor(self.m)?.get().map(Datum))
    }

    pub fn team(&self) -> Result<i16, Fault> {
        self.unit.team(self.m)
    }

    /// The seat it rides in, if any.
    pub fn seat(&self) -> Result<Option<i16>, Fault> {
        let s = self.unit.seat(self.m)?;
        Ok((s >= 0).then_some(s))
    }

    /// The four weapon slots.
    pub fn weapons(&self) -> Result<[Option<ObjectId>; 4], Fault> {
        Ok(self.unit.weapons(self.m)?.map(some))
    }

    /// The weapon in its hands.
    pub fn held_weapon(&self) -> Result<Option<ObjectId>, Fault> {
        Ok(self.unit.held_weapon(self.m)?.and_then(some))
    }

    /// The aiming vector, world space.
    pub fn aim(&self) -> Result<[f32; 3], Fault> {
        self.unit.aim(self.m)
    }
}

#[cfg(test)]
mod tests;
