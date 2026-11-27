//! The walks' working set (docs/engine/sim-rewrite-objects.md section 4,
//! "Transient SoA sets"): one row per live object, gathered at a walk's
//! entry, in slot order, as columns.
//!
//! A [`WalkSet`] is a prediction, never the state: every type callback
//! the walk makes can create, delete, wake and relink objects, so the
//! walk itself reads the pool again at each slot. What the set is good
//! for is what the walk is about to do: which slots its slot pass will
//! visit and of which types ([`WalkSet::visits`]), read once, without a
//! guest call in between. Capacity is kept between gathers; contents
//! never are.

use crate::view::Objects;
use guest_layout::object::{header, physics_flags};
use guest_layout::{Fault, Mem, ObjectType};

/// The two walks with a slot pass over awake objects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Walk {
    /// `b6f10`: `flags & (ACTIVE | AWAKE | NEW) == ACTIVE | AWAKE`.
    Pre,
    /// `b7060`: `flags & (POST | NEW) == POST`.
    Post,
}

/// The type column's value for a type outside the table.
pub const NO_KIND: u8 = 0xff;

/// One row per live object, in slot order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WalkSet {
    /// The salted handle.
    pub handle: Vec<u32>,
    /// Header flags ([`header`]).
    pub flags: Vec<u8>,
    /// Header type ([`ObjectType`] as a byte, [`NO_KIND`] outside it).
    pub kind: Vec<u8>,
    /// Body `+c0` ([`physics_flags`]).
    pub physics: Vec<u8>,
    /// The parent's handle (all ones for none).
    pub parent: Vec<u32>,
    pub position: Vec<[f32; 3]>,
    pub forward: Vec<[f32; 3]>,
    pub up: Vec<[f32; 3]>,
    /// The animation manager's offset from the body, -1 for none.
    pub animation: Vec<i16>,
}

impl WalkSet {
    /// Gather every live object of `pool` now. On a fault the set holds
    /// the rows before it.
    pub fn gather<M: Mem + ?Sized>(&mut self, m: &M, pool: &Objects) -> Result<(), Fault> {
        self.clear();
        for object in pool.iter(m) {
            let o = object?;
            let b = o.body;
            let row = (
                o.header.flags(m)?,
                o.header.kind(m)?,
                b.physics_flags(m)?,
                b.parent(m)?.0,
                b.position(m)?,
                b.forward(m)?,
                b.up(m)?,
                b.animation_manager(m)?,
            );
            self.handle.push(o.handle.0);
            self.flags.push(row.0);
            self.kind.push(if ObjectType::from_u8(row.1).is_some() { row.1 } else { NO_KIND });
            self.physics.push(row.2);
            self.parent.push(row.3);
            self.position.push(row.4);
            self.forward.push(row.5);
            self.up.push(row.6);
            self.animation.push(row.7);
        }
        Ok(())
    }

    pub fn clear(&mut self) {
        self.handle.clear();
        self.flags.clear();
        self.kind.clear();
        self.physics.clear();
        self.parent.clear();
        self.position.clear();
        self.forward.clear();
        self.up.clear();
        self.animation.clear();
    }

    pub fn len(&self) -> usize {
        self.handle.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handle.is_empty()
    }

    /// The rows `walk`'s slot pass would visit if nothing changed: its
    /// flag test, and not on the early-mover list (that pass visits those
    /// first). Children are visited by their parent's update, inside the
    /// visit, so they are rows of their own here only when they pass the
    /// test themselves, as in retail.
    pub fn visits(&self, walk: Walk) -> impl Iterator<Item = usize> + '_ {
        (0..self.len()).filter(move |&i| {
            let f = self.flags[i];
            let selected = match walk {
                Walk::Pre => {
                    f & (header::ACTIVE | header::AWAKE | header::NEW) == header::ACTIVE | header::AWAKE
                }
                Walk::Post => f & (header::POST | header::NEW) == header::POST,
            };
            selected && self.physics[i] & physics_flags::EARLY_MOVER == 0
        })
    }

    /// [`visits`](Self::visits) counted by type, in the type table's
    /// order; a type outside it is not counted.
    pub fn visits_by_kind(&self, walk: Walk) -> [u32; ObjectType::ALL.len()] {
        let mut out = [0; ObjectType::ALL.len()];
        for i in self.visits(walk) {
            if let Some(n) = out.get_mut(usize::from(self.kind[i])) {
                *n += 1;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Pool;
    use guest_layout::Object;

    fn pool() -> (Pool, Objects) {
        let mut p = Pool::new();
        let awake = header::ACTIVE | header::AWAKE;
        p.add(0, 1, ObjectType::Biped, awake);
        p.add(1, 2, ObjectType::Weapon, awake | header::POST);
        p.add(3, 3, ObjectType::Vehicle, awake | header::NEW);
        p.add(4, 4, ObjectType::Scenery, header::ACTIVE | header::POST);
        p.add(40, 5, ObjectType::Weapon, awake);
        let objects = Objects::read(&p.ram).unwrap().unwrap();
        (p, objects)
    }

    #[test]
    fn a_gather_has_one_row_per_live_slot_in_slot_order() {
        let (mut p, objects) = pool();
        let child = guest_layout::Datum(2 << 16 | 1);
        p.attach(child, guest_layout::Datum(1 << 16));
        p.body(0).set(&mut p.ram, Object::POSITION, [1.0f32, 2.0, 3.0]).unwrap();
        let mut set = WalkSet::default();
        set.gather(&p.ram, &objects).unwrap();
        assert_eq!(set.handle, [1 << 16, 2 << 16 | 1, 3 << 16 | 3, 4 << 16 | 4, 5 << 16 | 40]);
        assert_eq!(set.kind, [0, 2, 1, 6, 2]);
        assert_eq!(set.parent[1], 1 << 16);
        assert_eq!(set.parent[0], u32::MAX);
        assert_eq!(set.position[0], [1.0, 2.0, 3.0]);
        // The synthetic bodies leave the manager offset zeroed.
        assert_eq!(set.animation, [0; 5]);
        assert_eq!(set.flags[1] & header::CHILD, header::CHILD);
    }

    #[test]
    fn visits_follow_each_walks_flag_test_and_skip_early_movers() {
        let (mut p, objects) = pool();
        let mut set = WalkSet::default();
        set.gather(&p.ram, &objects).unwrap();
        assert_eq!(set.visits(Walk::Pre).collect::<Vec<_>>(), [0, 1, 4]);
        assert_eq!(set.visits(Walk::Post).collect::<Vec<_>>(), [1, 3]);
        let by_kind = set.visits_by_kind(Walk::Pre);
        assert_eq!((by_kind[0], by_kind[2], by_kind[1]), (1, 2, 0));
        // The early-mover pass owns slot 40.
        p.body(40).set(&mut p.ram, Object::PHYSICS_FLAGS, physics_flags::EARLY_MOVER).unwrap();
        set.gather(&p.ram, &objects).unwrap();
        assert_eq!(set.visits(Walk::Pre).collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn a_new_gather_sees_the_pool_as_it_is_now() {
        let (mut p, objects) = pool();
        let mut set = WalkSet::default();
        set.gather(&p.ram, &objects).unwrap();
        p.free(1);
        set.gather(&p.ram, &objects).unwrap();
        assert_eq!(set.len(), 4);
        assert!(!set.handle.contains(&(2 << 16 | 1)));
        set.clear();
        assert!(set.is_empty());
    }
}
