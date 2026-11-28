//! The Pogo Stick's riders in a game (docs/game/pogo-stick.md, "Phases
//! and tricks"): every player's [`Rider`](crate::pogo::Rider), stepped
//! once a tick from the superbounce mode's own reads.
//!
//! [`on_tick`] is called by `superbounce_mode` after the medal chain has
//! stepped, with the players it read ([`Seen`]) and the awards the chain
//! filed on the tick, so a bounce's verdict reaches the rider on the tick
//! it is made. It reads one thing more of each player: the weapon held,
//! and which of the map's sticks it is ([`crate::pogo_loadout::held_style`],
//! against `pogo::placed`). A player holding a stick is published as
//! a [`Ridden`]; the pose ([`crate::pogo_pose_glue`]), the camera
//! ([`crate::pogo_camera`]) and `pogo.state` read that list.
//!
//! All of it is presentation: nothing here writes the guest, and it is
//! not in a snapshot (after a load the riders start from the next tick).
//! It runs for every player on every machine, from reads that are the
//! same on each, so everyone sees everyone's tricks.

use std::sync::{Mutex, MutexGuard};

use guest_layout::GuestMemory;
use guest_layout::handle::DatumHandle;
use hud_kit_core::views::superbounce::Medal;
use pogo::Style;
use pogo::placed::Placed;

use crate::pogo::{Bounce, Cue, RiderTick, Riders, Sample, Trick};
use crate::superbounce_chain::{Award, Seen};

/// One player holding a stick, as of the last tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ridden {
    pub slot: u8,
    /// A player of this machine.
    pub local: bool,
    /// The biped's object datum.
    pub unit: u32,
    /// The held stick's object datum.
    pub weapon: u32,
    pub sample: Sample,
}

/// What one tick reads of a player for its rider.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Read {
    pub slot: u8,
    /// The local user; `None` for a remote player or a bot.
    pub user: Option<u8>,
    /// The stick held; `None` with another weapon, or no unit.
    pub held: Option<Held>,
}

/// A held stick and the body holding it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Held {
    pub unit: u32,
    pub weapon: u32,
    pub style: Style,
    pub airborne: bool,
    pub vz: f32,
}

static RIDERS: Mutex<Riders> = Mutex::new(Riders::new());
static RIDDEN: Mutex<Vec<Ridden>> = Mutex::new(Vec::new());

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Every player holding a stick, as of the last tick; empty while the
/// superbounce mode is off.
pub fn ridden() -> Vec<Ridden> {
    lock(&RIDDEN).clone()
}

/// The first local player's stick (the lowest local user's), if that
/// player holds one: the one the chase camera follows.
pub fn local() -> Option<Ridden> {
    lock(&RIDDEN).iter().find(|r| r.local).copied()
}

/// Starts every rider over: a new game, the game's end, the mode off.
pub fn reset() {
    *lock(&RIDERS) = Riders::new();
    lock(&RIDDEN).clear();
    crate::pogo_camera::reset();
}

/// The bounce the chain filed for `slot` on this tick: a Bounced (a fall
/// out of the map) is no bounce.
pub fn bounce_of(slot: u8, awards: &[Award]) -> Option<Bounce> {
    awards
        .iter()
        .find(|a| a.slot == slot && a.chain > 0 && !a.medals.contains(&Medal::Bounced))
        .map(|a| Bounce {
            chain: a.chain,
            monster: a.monster(),
        })
}

/// One tick of every rider: the players holding a stick, the first local
/// player first, and the sounds' cues of the local players.
pub fn step(riders: &mut Riders, reads: &[Read], awards: &[Award]) -> (Vec<Ridden>, Vec<Cue>) {
    let mut out = Vec::new();
    let mut cues = Vec::new();
    for r in reads {
        let tick = r.held.map(|h| RiderTick {
            style: h.style,
            airborne: h.airborne,
            vz: h.vz,
            bounce: bounce_of(r.slot, awards),
        });
        let s = riders.step(r.slot, tick.as_ref());
        let (Some(h), Some(sample)) = (r.held, s.sample) else {
            continue;
        };
        if r.user.is_some() {
            cues.extend(s.cue);
        }
        out.push((
            r.user,
            Ridden {
                slot: r.slot,
                local: r.user.is_some(),
                unit: h.unit,
                weapon: h.weapon,
                sample,
            },
        ));
    }
    let seen: Vec<u8> = reads.iter().map(|r| r.slot).collect();
    riders.keep(&seen);
    // Local players first, by user; then the rest by slot.
    out.sort_by_key(|(user, r)| (user.is_none(), user.unwrap_or(0), r.slot));
    (out.into_iter().map(|(_, r)| r).collect(), cues)
}

/// What a tick reads of `seen` for its rider.
fn read(m: &dyn GuestMemory, placed: Option<&Placed>, seen: &Seen) -> Read {
    let held = (|| {
        let t = seen.tick.as_ref()?;
        let placed = placed?;
        let (_, body) = guest_layout::view::objects::get(m, DatumHandle(t.unit)).ok()??;
        let style = crate::pogo_loadout::held_style(m, body.addr, placed)?;
        let weapon = guest_layout::Unit::at(body.addr).held_weapon(m).ok()??;
        Some(Held {
            unit: t.unit,
            weapon: weapon.raw(),
            style,
            airborne: t.airborne,
            vz: t.velocity[2],
        })
    })();
    Read {
        slot: seen.slot,
        user: seen.user,
        held,
    }
}

/// One tick, from the superbounce mode: `players` and `awards` are the
/// medal chain's of this tick, `map` the loaded map's name. Gives the
/// local players' cues.
pub(crate) fn on_tick(
    m: &dyn GuestMemory,
    map: Option<&str>,
    players: &[Seen],
    awards: &[Award],
) -> Vec<Cue> {
    let placed = map.and_then(pogo::placed::get);
    if placed.is_none() {
        // No kit in this map (the feature off, or the map refused): no
        // stick can be held.
        reset();
        return Vec::new();
    }
    let reads: Vec<Read> = players
        .iter()
        .map(|p| read(m, placed.as_ref(), p))
        .collect();
    let (ridden, cues) = step(&mut lock(&RIDERS), &reads, awards);
    *lock(&RIDDEN) = ridden;
    cues
}

/// `pogo.test`: shows `trick` from its start on the first local player
/// holding a stick, whatever that player is doing. Presentation only:
/// nothing is filed, sent or recorded. Gives the player's slot.
pub fn test(trick: Trick) -> Result<u8, String> {
    if !crate::superbounce_mode::active() {
        return Err("the superbounce mode is off: start a Superbounce game".into());
    }
    let Some(r) = local() else {
        return Err("no local player holds a pogo stick (swap to it with Y)".into());
    };
    lock(&RIDERS).force(r.slot, trick);
    Ok(r.slot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pogo::{AIR_TICKS, MOUNT_TICKS, Phase};

    fn held(unit: u32, airborne: bool, vz: f32) -> Option<Held> {
        Some(Held {
            unit,
            weapon: unit + 1,
            style: Style::Covenant,
            airborne,
            vz,
        })
    }

    fn award(slot: u8, chain: u32, medals: &[Medal]) -> Award {
        Award {
            seq: 1,
            slot,
            user: None,
            tick: 0,
            chain,
            medals: medals.to_vec(),
            spot: None,
            strips: None,
        }
    }

    #[test]
    fn a_bounce_is_the_slots_own_and_a_bounced_is_none() {
        let awards = [
            award(1, 3, &[Medal::Superbounce, Medal::TripleBounce]),
            award(2, 2, &[Medal::Bounced]),
            award(4, 1, &[Medal::Superbounce, Medal::Monster]),
        ];
        assert_eq!(
            bounce_of(1, &awards),
            Some(Bounce {
                chain: 3,
                monster: false
            })
        );
        assert_eq!(bounce_of(2, &awards), None);
        assert_eq!(
            bounce_of(4, &awards),
            Some(Bounce {
                chain: 1,
                monster: true
            })
        );
        assert_eq!(bounce_of(0, &awards), None);
        // A Bounced with no chain.
        assert_eq!(bounce_of(3, &[award(3, 0, &[Medal::Bounced])]), None);
    }

    #[test]
    fn only_players_holding_a_stick_are_ridden_and_locals_come_first() {
        let mut riders = Riders::new();
        let reads = [
            Read {
                slot: 0,
                user: None,
                held: held(0x10, false, 0.0),
            },
            Read {
                slot: 3,
                user: Some(1),
                held: held(0x30, false, 0.0),
            },
            Read {
                slot: 5,
                user: Some(0),
                held: held(0x50, false, 0.0),
            },
            Read {
                slot: 7,
                user: Some(2),
                held: None,
            },
        ];
        let (ridden, cues) = step(&mut riders, &reads, &[]);
        let slots: Vec<_> = ridden.iter().map(|r| (r.slot, r.local)).collect();
        assert_eq!(slots, [(5, true), (3, true), (0, false)]);
        assert!(cues.is_empty());
        assert_eq!(ridden[0].unit, 0x50);
        assert_eq!(ridden[0].weapon, 0x51);
        assert_eq!(ridden[0].sample.style, Style::Covenant);
        assert_eq!(ridden[0].sample.phase, Phase::Carried);
    }

    #[test]
    fn a_bounce_reaches_its_rider_and_only_a_locals_cue_is_given() {
        let mut riders = Riders::new();
        let reads = |airborne, vz| {
            [
                Read {
                    slot: 0,
                    user: Some(0),
                    held: held(0x10, airborne, vz),
                },
                Read {
                    slot: 1,
                    user: None,
                    held: held(0x20, airborne, vz),
                },
            ]
        };
        for _ in 0..(AIR_TICKS + MOUNT_TICKS + 2) {
            step(&mut riders, &reads(true, -6.0), &[]);
        }
        // Both touch down; only the local player's piston is heard.
        let (ridden, cues) = step(&mut riders, &reads(false, 12.0), &[]);
        assert!(ridden.iter().all(|r| r.sample.phase == Phase::Compress));
        assert_eq!(cues, [Cue::Compress]);
        // The remote player's bounce: its rider springs, with no cue.
        let awards = [award(1, 2, &[Medal::Superbounce, Medal::DoubleBounce])];
        let (ridden, cues) = step(&mut riders, &reads(false, 12.0), &awards);
        assert_eq!(ridden[0].sample.phase, Phase::Compress);
        assert_eq!(ridden[1].sample.phase, Phase::Spring);
        assert!(cues.is_empty());
        // The local player's.
        let awards = [award(0, 1, &[Medal::Superbounce])];
        let (ridden, cues) = step(&mut riders, &reads(false, 12.0), &awards);
        assert_eq!(ridden[0].sample.phase, Phase::Spring);
        assert_eq!(cues, [Cue::Spring]);
    }

    #[test]
    fn swapping_away_and_leaving_start_the_rider_over() {
        let mut riders = Riders::new();
        let with = |h| {
            [Read {
                slot: 2,
                user: Some(0),
                held: h,
            }]
        };
        for _ in 0..(AIR_TICKS + 1) {
            step(&mut riders, &with(held(0x10, true, 2.0)), &[]);
        }
        assert_eq!(riders.rider(2).unwrap().phase(), Phase::Mount);
        // Another weapon in the hands.
        let (ridden, _) = step(&mut riders, &with(None), &[]);
        assert!(ridden.is_empty());
        assert_eq!(riders.rider(2).unwrap().phase(), Phase::Carried);
        // Back on it, then gone from the game.
        for _ in 0..(AIR_TICKS + 1) {
            step(&mut riders, &with(held(0x10, true, 2.0)), &[]);
        }
        step(&mut riders, &[], &[]);
        assert_eq!(riders, Riders::new());
    }
}
