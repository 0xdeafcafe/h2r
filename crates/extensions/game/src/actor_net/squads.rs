//! The squad place, hunt and erase entry shared by Firefight's director and
//! Forge's "squad place" sink (docs/game/ai-networking.md, "Sharing with
//! Forge").
//!
//! An entry is applied at a tick boundary through the engine's own squad
//! routines, never by writing actor records. The engine half implements
//! [`SquadEntry`]; until those routines are traced, only fakes do
//! (docs/game/firefight.md, "Guest boundary").

/// Actors one placement may ask for (the definition's squad limit).
pub const MAX_COUNT: u16 = 32;

/// One squad placement, with the kit's names resolved to the map's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    /// The scenario squad the kit reserved for this slot.
    pub slot: u8,
    /// The map-local character tag slot the kit maps the class to.
    pub character: u16,
    /// Actors to place.
    pub count: u16,
    /// The kit zone the squad enters from.
    pub zone: u8,
    /// The team the actors join: one no player holds.
    pub team: u8,
    /// The kit's weapon tier, 0 to 3.
    pub weapon_tier: u8,
}

/// What the entry is asked to do with one squad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SquadOp {
    Place(Placement),
    /// Send the squad at the players: through the map's hunt order for
    /// `base`. Firefight's base is the squad's target team's (0 with one
    /// team); Forge, which marks one base, sends 0.
    Hunt {
        slot: u8,
        base: u8,
    },
    /// Remove the squad's actors and bodies.
    Erase {
        slot: u8,
    },
}

impl SquadOp {
    pub fn slot(&self) -> u8 {
        match *self {
            SquadOp::Place(Placement { slot, .. })
            | SquadOp::Hunt { slot, .. }
            | SquadOp::Erase { slot } => slot,
        }
    }

    /// Refuse a slot outside the `slots` the map reserves, and a placement
    /// of no actors or more than [`MAX_COUNT`].
    pub fn check(&self, slots: usize) -> Result<(), Refused> {
        if usize::from(self.slot()) >= slots {
            return Err(Refused::Slot {
                slot: self.slot(),
                slots,
            });
        }
        if let SquadOp::Place(place) = self
            && !(1..=MAX_COUNT).contains(&place.count)
        {
            return Err(Refused::Count(place.count));
        }
        Ok(())
    }
}

/// Why a squad operation was not passed to the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    Slot { slot: u8, slots: usize },
    Count(u16),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Slot { slot, slots } => {
                write!(f, "squad slot {slot} is outside the map's {slots} slots")
            }
            Refused::Count(count) => {
                write!(f, "a squad of {count} actors is outside 1 to {MAX_COUNT}")
            }
        }
    }
}

impl std::error::Error for Refused {}

/// The engine's squad routines. Callers check the operation first
/// ([`SquadOp::check`]).
pub trait SquadEntry {
    fn squad(&mut self, op: SquadOp) -> anyhow::Result<()>;
}

/// Count actors by squad index into `out`; indices past its end, such as a
/// squad of the base map or none (`u16::MAX`), are not counted.
pub fn count_by_squad(squads: impl IntoIterator<Item = u16>, out: &mut [u16]) {
    out.fill(0);
    for squad in squads {
        if let Some(count) = out.get_mut(usize::from(squad)) {
            *count = count.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(slot: u8, count: u16) -> SquadOp {
        SquadOp::Place(Placement {
            slot,
            character: 4,
            count,
            zone: 0,
            team: 3,
            weapon_tier: 0,
        })
    }

    #[test]
    fn a_slot_outside_the_kit_or_an_empty_squad_is_refused() {
        assert_eq!(place(15, 1).check(16), Ok(()));
        assert_eq!(place(15, 32).check(16), Ok(()));
        assert_eq!(
            place(16, 1).check(16),
            Err(Refused::Slot {
                slot: 16,
                slots: 16
            })
        );
        assert_eq!(place(0, 0).check(16), Err(Refused::Count(0)));
        assert_eq!(place(0, 33).check(16), Err(Refused::Count(33)));
        assert_eq!(SquadOp::Erase { slot: 2 }.check(2).map_err(|_| ()), Err(()));
        assert_eq!(SquadOp::Hunt { slot: 1, base: 1 }.check(2), Ok(()));
    }

    #[test]
    fn actors_count_by_squad_and_others_are_ignored() {
        let mut out = [9; 4];
        count_by_squad([0, 2, 2, 7, u16::MAX, 3], &mut out);
        assert_eq!(out, [1, 0, 2, 1]);
    }
}
