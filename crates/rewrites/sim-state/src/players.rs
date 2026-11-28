//! Players, local users' first-person state and the animation channels, as
//! typed views over any [`Mem`]: the arena, or [`PlayersMem`] and
//! [`sim_objects::PoolMem`] over the World's pools (docs/engine/sim-world.md,
//! "The players store"). Offsets are `guest_layout`'s; the first-person block
//! and the animation manager are the two layouts named here.

use guest_layout::object::Object;
use guest_layout::player::{LEAVING, Player};
use guest_layout::{DatumArray, Datum, Fault, Mem, Off};

use crate::ObjectId;

pub use sim_players::{PlayersMem, PlayersState};

/// A live player's salted handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlayerId(pub u32);

impl PlayerId {
    /// The slot in the players pool.
    pub const fn index(self) -> u16 {
        self.0 as u16
    }
}

/// One listed player.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerView {
    pub id: PlayerId,
    pub name: String,
    pub team: i8,
    /// The machine id (`Player::MACHINE`).
    pub machine: [u8; 6],
    /// The local user index, `None` for a remote player.
    pub local_user: Option<u16>,
    /// The unit the player controls, `None` between lives.
    pub unit: Option<ObjectId>,
    /// Spawn protection ticks left.
    pub spawn_grace: i16,
    /// Active camouflage ticks left.
    pub camo_timer: i16,
}

/// The listed players in slot order: live and not leaving (`Player::listed`).
/// Empty while no game has initialised the players.
pub fn players<M: Mem + ?Sized>(m: &M) -> Result<Vec<PlayerView>, Fault> {
    let Some(addr) = guest_layout::global::PLAYERS.pointer(m)? else {
        return Ok(Vec::new());
    };
    let array = DatumArray::at(addr);
    let (high, data) = (array.high_water(m)?.max(0) as u32, array.data(m)?);
    let stride = array.element_size(m)? as u32;
    let mut out = Vec::new();
    for slot in 0..high {
        let p = Player::at(data.wrapping_add(slot * stride));
        let salt = p.salt(m)?;
        if salt == 0 || p.flags(m)? & LEAVING != 0 {
            continue;
        }
        let local = p.local_user(m)?;
        out.push(PlayerView {
            id: PlayerId(u32::from(salt) << 16 | slot),
            name: p.name(m)?,
            team: p.team(m)?,
            machine: p.machine(m)?,
            local_user: u16::try_from(local).ok(),
            unit: p.unit(m)?.get().map(ObjectId),
            spawn_grace: p.spawn_grace(m)?,
            camo_timer: p.camo_timer(m)?,
        });
    }
    Ok(out)
}

/// The player index each of the four local users maps to (`+0xc` of the
/// players' globals), `None` for an unmapped user.
pub fn local_users<M: Mem + ?Sized>(m: &M) -> Result<[Option<u16>; sim_players::FIRST_PERSON_USERS], Fault> {
    let mut out = [None; sim_players::FIRST_PERSON_USERS];
    let Some(globals) = guest_layout::global::PLAYER_GLOBALS.pointer(m)? else {
        return Ok(out);
    };
    for (user, slot) in out.iter_mut().enumerate() {
        let index = Off::<u32>::new(0xc + 4 * user as u32).read(m, globals)?;
        *slot = (index != u32::MAX).then_some(index as u16);
    }
    Ok(out)
}

/// One local user's first-person state: the block the `first_person` phase
/// (`0x00165dc1`) keeps per user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirstPersonView {
    pub user: u8,
    /// The unit shown (`+4`), `None` while none is.
    pub unit: Option<ObjectId>,
    /// `+8`: what the phase compares with the player's `+0x88` byte mapped
    /// through `0x0016896f`; a change rebuilds the user's slots.
    pub key: u32,
}

/// The first-person state of local user `user` (0 to 3), `None` while the
/// block does not exist or `user` is out of range.
pub fn first_person<M: Mem + ?Sized>(m: &M, user: u8) -> Result<Option<FirstPersonView>, Fault> {
    if usize::from(user) >= sim_players::FIRST_PERSON_USERS {
        return Ok(None);
    }
    let Some(base) = guest_layout::global::FIRST_PERSON.pointer(m)? else {
        return Ok(None);
    };
    let at = base.wrapping_add(u32::from(user) * sim_players::FIRST_PERSON_USER_SIZE as u32);
    Ok(Some(FirstPersonView {
        user,
        unit: Datum(Off::<u32>::new(4).read(m, at)?).get().map(ObjectId),
        key: Off::<u32>::new(8).read(m, at)?,
    }))
}

/// One animation channel (`0x20` bytes in the manager).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Channel {
    /// The channel's graph (`jmad` tag datum), `None` when unset.
    pub graph: Option<u32>,
    /// `-1` the channel's graph, else a position in its inheritance list.
    pub inherited: i16,
    /// The animation's index in that graph; `-1` none.
    pub index: i16,
    /// Frame, 0 to the animation's frame count minus one.
    pub frame: f32,
}

impl Channel {
    /// The channel plays an animation: a graph, a non-negative index and a
    /// sane inheritance slot.
    pub fn active(&self) -> bool {
        self.graph.is_some() && self.index >= 0 && self.inherited >= -1
    }
}

/// An object's animation state: the manager's graph and its three channels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimState {
    pub graph: Option<u32>,
    pub channels: [Channel; 3],
}

/// Offsets a manager can be at (`live-telemetry`'s `MANAGER_OFFSETS`).
const MANAGER_OFFSETS: std::ops::Range<i16> = 0x12c..0x2000;
const MANAGER_GRAPH: u32 = 0x68;
const CHANNEL_SIZE: u32 = 0x20;

/// The animation state of `object`'s body, `None` for an object without a
/// manager (`Object::ANIMATION_MANAGER` outside the manager offsets).
pub fn anim<M: Mem + ?Sized>(m: &M, body: Object) -> Result<Option<AnimState>, Fault> {
    let offset = body.animation_manager(m)?;
    if !MANAGER_OFFSETS.contains(&offset) {
        return Ok(None);
    }
    let manager = body.addr.wrapping_add(offset as u32);
    let mut channels = [Channel { graph: None, inherited: -1, index: -1, frame: 0.0 }; 3];
    for (i, c) in channels.iter_mut().enumerate() {
        let at = manager.wrapping_add(i as u32 * CHANNEL_SIZE);
        *c = Channel {
            graph: Datum(Off::<u32>::new(0).read(m, at)?).get(),
            inherited: Off::<i16>::new(4).read(m, at)?,
            index: Off::<i16>::new(6).read(m, at)?,
            frame: Off::<f32>::new(0x1c).read(m, at)?,
        };
    }
    Ok(Some(AnimState {
        graph: Datum(Off::<u32>::new(MANAGER_GRAPH).read(m, manager)?).get(),
        channels,
    }))
}

#[cfg(test)]
mod tests;
