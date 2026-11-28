//! A phantom's weapon accuracy, from the game's own multiplayer shot credits.
//! The shooter, weapon and range at fire stay attached to the shot ID until
//! the game credits its first hostile hit. See docs/runtime/sim.md §6.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use bots::phantom::{PhantomHandle, Phantoms};
use sim_bench::metrics::WeaponAccuracy;

// Return addresses at the two calls to WEAPON_STAT_BUCKET in the shooter's
// shot register and hostile-hit credit paths. Other callers also look up
// weapon stat buckets (e.g. kills) and are irrelevant.
const REGISTERED_SHOT: u32 = 0x0015_cd6a;
const CREDITED_HIT: u32 = 0x0015_ce45;
const SHOT_SLOTS: usize = 8;
const PLAYERS: usize = 16;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Range {
    Close,
    Mid,
    Long,
    Unknown,
}

impl Range {
    fn of(distance: Option<f32>) -> Self {
        match distance.filter(|d| d.is_finite() && *d >= 0.0) {
            Some(d) if d < 3.0 => Self::Close,
            Some(d) if d < 12.0 => Self::Mid,
            Some(_) => Self::Long,
            None => Self::Unknown,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Close => "close",
            Self::Mid => "mid",
            Self::Long => "long",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy)]
struct Shot {
    id: u16,
    handle: PhantomHandle,
    code: u8,
    range: Range,
}

#[derive(Default)]
struct Counts {
    shots: u64,
    hits: u64,
}

#[derive(Default)]
struct Accuracy {
    slots: [[Option<Shot>; SHOT_SLOTS]; PLAYERS],
    totals: BTreeMap<(PhantomHandle, u8, Range), Counts>,
}

impl Accuracy {
    fn fire(&mut self, player: usize, slot: usize, shot: Option<Shot>) {
        let Some(entry) = self.slots.get_mut(player).and_then(|slots| slots.get_mut(slot)) else {
            return;
        };
        // Retail can replace an outstanding shot, or reuse a player's
        // slot for a different machine before the old projectile expires.
        *entry = shot;
        if let Some(shot) = shot {
            self.totals.entry((shot.handle, shot.code, shot.range)).or_default().shots += 1;
        }
    }

    fn hit(&mut self, player: usize, slot: usize, id: u16) {
        let Some(entry) = self.slots.get_mut(player).and_then(|slots| slots.get_mut(slot)) else {
            return;
        };
        if let Some(shot) = entry.take_if(|shot| shot.id == id)
            && let Some(counts) = self.totals.get_mut(&(shot.handle, shot.code, shot.range))
        {
            counts.hits += 1;
        }
    }

    fn rows(&self, handle: PhantomHandle) -> Vec<WeaponAccuracy> {
        self.totals.iter().filter(|((owner, _, _), _)| *owner == handle)
            .map(|(&(_, code, range), c)| {
                let (kind, name) = crate::telemetry::layout::damage_reporting_type(code);
                WeaponAccuracy {
                    damage_code: code,
                    weapon: name.unwrap_or(kind).to_owned(),
                    range: range.name().to_owned(),
                    shots: c.shots,
                    hits: c.hits,
                    accuracy: c.hits as f64 / c.shots as f64,
                }
            }).collect()
    }
}

fn book() -> MutexGuard<'static, Accuracy> {
    static BOOK: OnceLock<Mutex<Accuracy>> = OnceLock::new();
    BOOK.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn rows(handle: PhantomHandle) -> Vec<WeaponAccuracy> {
    book().rows(handle)
}

pub(crate) fn reset() {
    *book() = Accuracy::default();
}

pub(crate) fn install(b: &mut hooks::Builder, phantoms: Phantoms) {
    b.game().on_start("start phantom shot counts", |_| reset());
    b.raw().observe_before(
        &guest_layout::points::WEAPON_STAT_BUCKET,
        "credit phantom shots by weapon and range",
        move |o| {
            let player = (o.cpu.ebp & 0xffff) as usize;
            if player >= PLAYERS || phantoms.is_empty() {
                return;
            }
            let g = o.guest();
            match g.u32(o.cpu.esp).ok() {
                Some(REGISTERED_SHOT) => {
                    drop(g);
                    // ESI selects one of the player's eight retail entries.
                    // A human overwriting the same entry forgets its old ID.
                    let shot = phantoms.shot_context(player as u32).map(|(handle, range)| Shot {
                        id: o.cpu.edi as u16,
                        handle,
                        code: o.cpu.ecx as u8 & 0x3f,
                        range: Range::of(range),
                    });
                    book().fire(player, o.cpu.esi as usize, shot);
                }
                Some(CREDITED_HIT) => {
                    // ESI is the matched entry. EDI counts slots remaining.
                    if let Some(slot) = SHOT_SLOTS.checked_sub(o.cpu.edi as usize)
                        && let Ok(id) = g.u16(o.cpu.esi)
                    {
                        book().hit(player, slot, id);
                    }
                }
                _ => {}
            }
        },
    ).optional();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_follow_the_fired_shot_through_a_switch_and_multiple_impacts() {
        let owner = PhantomHandle(40);
        let other = PhantomHandle(41);
        let rifle = Shot { id: 99, handle: owner, code: 10, range: Range::of(Some(12.0)) };
        let smg = Shot { id: 100, code: 8, range: Range::of(Some(2.0)), ..rifle };
        let mut tally = Accuracy::default();
        tally.fire(3, 5, Some(rifle));
        tally.fire(3, 2, Some(smg));
        tally.hit(3, 5, 99);
        tally.hit(3, 5, 99);
        tally.hit(3, 2, 99);
        tally.fire(3, 2, Some(Shot { id: 101, handle: other, ..smg }));
        tally.hit(3, 2, 100);
        tally.hit(3, 2, 101);
        let rows = tally.rows(owner);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].range.as_str(), rows[0].hits, rows[0].shots), ("close", 0, 1));
        assert_eq!((rows[1].weapon.as_str(), rows[1].accuracy), ("battle_rifle", 1.0));
        assert_eq!(tally.rows(other)[0].hits, 1);
        assert_eq!(Range::of(Some(3.0)), Range::Mid);
        assert_eq!(Range::of(Some(f32::NAN)), Range::Unknown);
        assert_eq!(Range::of(None), Range::Unknown);
    }
}
