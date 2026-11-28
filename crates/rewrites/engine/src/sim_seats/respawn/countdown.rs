//! The respawn countdown's callees (docs/engine/sim-rewrite-sketches.md,
//! "Seats and rules (phase 7): kernels"): the entity dirty mark `a7840` with its network
//! entity queue `b58c0`, and the death event `19eb90` with its post
//! `a7c50` (`b5a70`) and its announcement walk `19eb30` -> `19de80` ->
//! `19df10`, `19e890`.
//!
//! Each runs on [`Mem`], the guest memory as the call has left it so far:
//! the gate's own writes (the timer, the fade, the queue's flags) are seen
//! by its later reads and reach the guest only when the prediction is
//! applied. The event the gate builds lives on its dead stack; it is
//! written to `Mem` as scratch and never applied.
//!
//! Every function returns `eax`, `ecx` and `edx` as retail leaves them; the
//! callee-saved registers are restored by each, and each pops what its
//! `ret` pops. A read that faults is the guest's fault, returned as it.
//! Three paths reach further guest systems and are not answered here:
//! each is recorded in [`Mem`] ([`Call`]) and the gate makes the call from
//! the retail frame instead.
//!
//! | Path | Where | The gate calls |
//! |---|---|---|
//! | a local player's announcement | `19e890` past its early exits | the seat `19e890` ([`Target::Announce`]) |
//! | a host's event message | `b5a70` with the network state 4 or 5 | `19eb90` ([`Target::Event`]): the seats `19eb90`, `a7c50`, `19eb30`, `19de80`, `19df10`, `19e890`, with `b5a70` a guest call |
//! | an event naming no player | `19eb30` with the event's player `-1` | `19eb90` ([`Target::Event`]): `19eb30` walks the players through the guest iterator `19f240` |

use super::{Regs, GAME, MODE, PLAYERS, STRIDE, USERS};
use runtime::{Guest, GuestFault};

/// The network globals: `+4` the session, `+8` its state.
const NETWORK: u32 = 0x004c_f77c;
/// The word `a7c50` passes `b5a70` last.
const POST_TAG: u32 = 0x004c_edf0;
/// The game settings: `+0x16c` the globals tag.
const SETTINGS: u32 = 0x004e_034c;
/// The tag instance table: 16-byte rows, `+8` the definition.
const TAGS: u32 = 0x004e_3b44;

/// `19df10`'s first jump table, by the event's type: the offset of its
/// announcement block in the globals' definition, or none (`ecx` stays 0).
const BLOCKS: [Option<u32>; 11] = [
    Some(0x98),
    Some(0xa0),
    Some(0xa8),
    Some(0xb0),
    Some(0xb8),
    None,
    Some(0xc8),
    None,
    Some(0xd8),
    Some(0xe0),
    Some(0xe8),
];
/// One announcement row's stride.
const ROW: u32 = 0xa8;

const SLOTS: usize = 16;
/// The most live words one gate call writes.
pub const WRITES: usize = 4;

#[derive(Clone, Copy, Debug, Default)]
struct Write {
    addr: u32,
    value: u32,
    size: u8,
}

/// The live words a call wrote, last value per address, in first-write
/// order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Writes {
    at: [(u32, u32); WRITES],
    len: u8,
}

impl Writes {
    pub fn iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.at[..usize::from(self.len)].iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn put(&mut self, addr: u32, value: u32) -> Result<(), GuestFault> {
        let len = usize::from(self.len);
        if let Some(w) = self.at[..len].iter_mut().find(|w| w.0 == addr) {
            w.1 = value;
            return Ok(());
        }
        assert!(len < WRITES, "respawn: the gate writes at most {WRITES} live words");
        self.at[len] = (addr, value);
        self.len += 1;
        Ok(())
    }
}

/// What the gate calls from the retail frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// `19e890 (player: eax, row: ecx, event)`: a local player's
    /// announcement.
    Announce { player: u32, row: u32 },
    /// `19eb90 (event: eax)` at the gate's own `call`: the death event
    /// whole, for a host's message or an event naming no player.
    Event,
}

/// A call the death event needs from the retail frame, with the stores
/// the gate had made by then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    pub target: Target,
    /// The event, on the gate's frame.
    pub event: u32,
    /// The stores before the call (scratch and live), in order: the
    /// event's words.
    stores: [(u32, u32, u8); SLOTS],
    len: u8,
}

impl Call {
    pub fn stores(&self) -> impl Iterator<Item = (u32, u32, u8)> + '_ {
        self.stores[..usize::from(self.len)].iter().copied()
    }
}

/// Guest memory as the call has left it so far.
pub struct Mem<'g> {
    g: &'g Guest,
    writes: [Write; SLOTS],
    len: usize,
    live: Writes,
    call: std::cell::Cell<Option<Call>>,
}

impl<'g> Mem<'g> {
    pub fn new(g: &'g Guest) -> Self {
        Self {
            g,
            writes: [Write::default(); SLOTS],
            len: 0,
            live: Writes::default(),
            call: std::cell::Cell::new(None),
        }
    }

    /// The call the death event needs, if any.
    pub fn call(&self) -> Option<Call> {
        self.call.get()
    }

    /// Record a call for the gate to make. The first one stands: the
    /// event's walk stops at it.
    fn record(&self, target: Target, event: u32) {
        if self.call.get().is_some() {
            return;
        }
        let mut stores = [(0, 0, 0); SLOTS];
        for (slot, w) in stores.iter_mut().zip(&self.writes[..self.len]) {
            *slot = (w.addr, w.value, w.size);
        }
        self.call.set(Some(Call { target, event, stores, len: self.len as u8 }));
    }

    /// The guest as it was before the call.
    pub fn guest(&self) -> &'g Guest {
        self.g
    }

    /// The live words written so far.
    pub fn writes(&self) -> Writes {
        self.live
    }

    pub fn u8(&self, a: u32) -> Result<u8, GuestFault> {
        for w in self.writes[..self.len].iter().rev() {
            let off = a.wrapping_sub(w.addr);
            if off < u32::from(w.size) {
                return Ok((w.value >> (8 * off)) as u8);
            }
        }
        self.g.u8(a)
    }

    pub fn u16(&self, a: u32) -> Result<u16, GuestFault> {
        if self.len == 0 {
            return self.g.u16(a);
        }
        Ok(u16::from(self.u8(a)?) | u16::from(self.u8(a.wrapping_add(1))?) << 8)
    }

    pub fn u32(&self, a: u32) -> Result<u32, GuestFault> {
        if self.len == 0 {
            return self.g.u32(a);
        }
        let mut v = 0;
        for i in 0..4 {
            v |= u32::from(self.u8(a.wrapping_add(i))?) << (8 * i);
        }
        Ok(v)
    }

    /// `movsx r32, word [a]`.
    fn i16(&self, a: u32) -> Result<u32, GuestFault> {
        Ok(self.u16(a)? as i16 as i32 as u32)
    }

    /// `movsx r32, byte [a]`.
    fn i8(&self, a: u32) -> Result<u32, GuestFault> {
        Ok(self.u8(a)? as i8 as i32 as u32)
    }

    fn put(&mut self, addr: u32, value: u32, size: u8, live: bool) -> Result<(), GuestFault> {
        assert!(self.len < SLOTS, "respawn: the gate makes at most {SLOTS} stores");
        if live {
            self.live.put(addr, value)?;
        }
        self.writes[self.len] = Write { addr, value, size };
        self.len += 1;
        Ok(())
    }

    /// A word write that reaches the guest when the prediction is applied.
    pub fn set_u32(&mut self, addr: u32, value: u32) -> Result<(), GuestFault> {
        self.put(addr, value, 4, true)
    }

    /// A write to the call's own stack, below the caller's `esp`: seen by
    /// the call's reads, never applied.
    pub fn scratch_u32(&mut self, addr: u32, value: u32) -> Result<(), GuestFault> {
        self.put(addr, value, 4, false)
    }

    pub fn scratch_u16(&mut self, addr: u32, value: u16) -> Result<(), GuestFault> {
        self.put(addr, u32::from(value), 2, false)
    }
}

/// `b58c0 (entity, flags)`, `ret 8`: on a host in network state 4, ORs
/// `flags` into the entity's queued update word when the entity is
/// replicated (bit 2 of its row's `+0x44`).
pub fn entity_queue(m: &mut Mem, mut r: Regs, entity: u32, flags: u32) -> Result<Regs, GuestFault> {
    r.ecx = m.u32(NETWORK)?;
    r.eax = m.u32(r.ecx.wrapping_add(8))?;
    // `cmp 4; je`, `cmp 5; jne out`, then `cmp 3; je out`, `cmp 5; je out`.
    if r.eax != 4 {
        return Ok(r);
    }
    r.ecx = m.u32(r.ecx.wrapping_add(4))?;
    r.eax = entity;
    r.edx = m.u32(r.ecx.wrapping_add(0x20a4))?;
    r.ecx = r.ecx.wrapping_add(0x2098);
    r.eax &= 0x3ff;
    if m.u8(r.edx.wrapping_add(r.eax * 8).wrapping_add(0x44))? & 4 == 0 {
        return Ok(r);
    }
    r.eax <<= 5;
    r.edx = m.u32(r.eax.wrapping_add(r.ecx).wrapping_add(0x20))?;
    r.eax = r.eax.wrapping_add(r.ecx).wrapping_add(0x14);
    r.ecx = flags;
    r.edx |= r.ecx;
    m.set_u32(r.eax.wrapping_add(0xc), r.edx)?;
    Ok(r)
}

/// `a7840 (player, flags)`, `ret 8`: with a local user mapped, the
/// player's entity `[game + 0x2c + 4 * (i16)player]`, unless `-1`, gets
/// `flags` through the tail call `b58c0`.
pub fn entity_mark(m: &mut Mem, mut r: Regs, player: u32, flags: u32) -> Result<Regs, GuestFault> {
    r.eax = m.u32(GAME)?;
    r.ecx = m.u32(r.eax.wrapping_add(0xc14))?;
    r.edx = m.u32(r.ecx.wrapping_mul(4).wrapping_add(USERS))?;
    if r.edx == 0 {
        return Ok(r);
    }
    r.edx = player as u16 as i16 as i32 as u32;
    r.eax = m.u32(r.eax.wrapping_add(r.edx.wrapping_mul(4)).wrapping_add(0x2c))?;
    if r.eax == u32::MAX {
        return Ok(r);
    }
    entity_queue(m, r, r.eax, flags)
}

/// `a7c50 (event: edx)`: on a mode-5 machine, the event's copy goes to
/// `b5a70`, which returns at once unless the network state is 4 or 5: then
/// the gate calls the whole event ([`Target::Event`], recorded in `m`).
/// `esp` is the call's entry `esp`.
pub fn event_post(m: &Mem, mut r: Regs, esp: u32) -> Result<Regs, GuestFault> {
    let mode = m.i8(m.u32(MODE)?.wrapping_add(0xc))?;
    r.eax = mode;
    let mode = mode as i32;
    if !(4..=5).contains(&mode) {
        return Ok(r);
    }
    if mode == 4 {
        r.eax = 0;
        return Ok(r);
    }
    // `rep movsd` of the event's nine words to the frame.
    let event = r.edx;
    for i in 0..9 {
        m.u32(event.wrapping_add(4 * i))?;
    }
    // `b5a70` with `eax` -1 and no entities to resolve.
    let network = m.u32(NETWORK)?;
    let state = m.u32(network.wrapping_add(8))?;
    if state == 4 || state == 5 {
        m.record(Target::Event, event);
    }
    Ok(Regs { eax: network, ecx: m.u32(POST_TAG)?, edx: esp.wrapping_sub(0x24) })
}

/// `19df10 (player: eax, event: edi, kind)`, `ret 4`: the first
/// announcement row in the event type's block whose event and kind match
/// and whose subject tests pass, in `eax`, or 0.
pub fn announcement(m: &Mem, player: u32, event: u32, kind: u32) -> Result<Regs, GuestFault> {
    let settings = m.u32(SETTINGS)?;
    let tag = m.u32(settings.wrapping_add(0x16c))? & 0xffff;
    let tags = m.u32(TAGS)?;
    let def = m.u32((tag << 4).wrapping_add(tags).wrapping_add(8))?;
    let def = m.u32(def.wrapping_add(0xc))?;
    let ty = m.u32(event)?;
    let block = match BLOCKS.get(ty as usize).copied().flatten() {
        Some(off) => def.wrapping_add(off),
        None => 0,
    };
    let count = m.u32(block)?;
    if count as i32 <= 0 {
        return Ok(Regs { eax: 0, ecx: block, edx: count });
    }
    let mut row = m.u32(block.wrapping_add(4))?;
    let players = m.u32(PLAYERS)?;
    let mut i: u32 = 0;
    loop {
        if let Some(r) = row_matches(m, row, player, event, kind, players)? {
            return Ok(r);
        }
        i = i.wrapping_add(1);
        row = row.wrapping_add(ROW);
        if (i as i32) >= (count as i32) {
            return Ok(Regs { eax: 0, ecx: i, edx: count });
        }
    }
}

/// One row of `19df10`'s walk: `Some` with the registers its `ret` leaves
/// when the row is the answer.
fn row_matches(
    m: &Mem,
    row: u32,
    player: u32,
    event: u32,
    kind: u32,
    players: u32,
) -> Result<Option<Regs>, GuestFault> {
    let mut ecx = m.i16(row.wrapping_add(4))?;
    if ecx != m.u32(event.wrapping_add(4))? || m.i16(row.wrapping_add(6))? != kind {
        return Ok(None);
    }
    // The row's first subject: `cl` says the event names it.
    let first = m.i16(row.wrapping_add(0x10))?.wrapping_sub(1);
    if first <= 3 {
        let v = m.u32(event.wrapping_add(0xc + 4 * first))?;
        ecx = (v & !0xff) | u32::from(v != u32::MAX);
    } else {
        ecx = (ecx & !0xff) | 1;
    }
    let set = ecx & 0xff != 0;
    // The second subject: the row answers when the event's differs.
    let mut edx = m.i16(row.wrapping_add(0x12))?.wrapping_sub(1);
    let found = match edx {
        0..=3 if !set => false,
        0 => m.u32(event.wrapping_add(0xc))? != player,
        1 => {
            edx = m.u32(players.wrapping_add(0x44))?;
            ecx = team(m, players, player)?;
            m.u32(event.wrapping_add(0x10))? != ecx
        }
        2 => m.u32(event.wrapping_add(0x14))? != player,
        3 => {
            ecx = m.u32(players.wrapping_add(0x44))?;
            edx = team(m, players, player)?;
            m.u32(event.wrapping_add(0x18))? != edx
        }
        _ => set,
    };
    Ok(found.then_some(Regs { eax: row, ecx, edx }))
}

/// The player's team, `movsx [player + 0xc0]`, read as `19df10` reads it.
fn team(m: &Mem, players: u32, player: u32) -> Result<u32, GuestFault> {
    let data = m.u32(players.wrapping_add(0x44))?;
    m.i8((player & 0xffff).wrapping_mul(STRIDE).wrapping_add(data).wrapping_add(0xc0))
}

/// `19e890 (player: eax, row: ecx, event)`, `ret 4`: the announcement for
/// a local player. A player with no local user (`+0x28` is `-1`) returns
/// before anything is drawn, as does every event but type 0 kind `0x13`
/// once `[[4e6948] + 0x1128]` is set. The rest draws and plays: it is
/// recorded in `m` ([`Mem::call`]) for the gate to call, and the
/// registers it returns are the early exit's (the gate's `15de0a` reloads
/// all three).
pub fn announce(m: &Mem, player: u32, row: u32, event: u32) -> Result<Regs, GuestFault> {
    let settings = m.u32(SETTINGS)?;
    let mut eax = (player & 0xffff).wrapping_mul(STRIDE);
    let data = m.u32(m.u32(PLAYERS)?.wrapping_add(0x44))?;
    let mut ecx = m.u32(settings.wrapping_add(0x16c))?;
    let mut edx = m.u32(TAGS)?;
    ecx &= 0xffff;
    eax = eax.wrapping_add(data);
    ecx <<= 4;
    let local = m.u16(eax.wrapping_add(0x28))?;
    ecx = m.u32(ecx.wrapping_add(edx).wrapping_add(8))?;
    m.u32(ecx.wrapping_add(0xc))?;
    if local == 0xffff {
        return Ok(Regs { eax, ecx, edx });
    }
    edx = m.u32(MODE)?;
    let quiet = m.u8(edx.wrapping_add(0x1128))?;
    eax = (eax & !0xff) | u32::from(quiet);
    if quiet != 0 && (m.u32(event)? != 0 || m.u32(event.wrapping_add(4))? != 0x13) {
        return Ok(Regs { eax, ecx, edx });
    }
    m.record(Target::Announce { player, row }, event);
    Ok(Regs { eax, ecx, edx })
}

/// `19de80 (event: eax, player: esi)`: the player's announcement row, by
/// the event's subjects in order (the player as `+0xc`, as `+0x14`, its
/// team as `+0x10`, as `+0x18`, then anyone), announced through `19e890`.
pub fn announce_player(m: &Mem, event: u32, player: u32) -> Result<Regs, GuestFault> {
    let players = m.u32(PLAYERS)?;
    let base = (player & 0xffff).wrapping_mul(STRIDE).wrapping_add(m.u32(players.wrapping_add(0x44))?);
    let find = |kind: u32| announcement(m, player, event, kind);
    if player == m.u32(event.wrapping_add(0xc))? {
        let r = find(0)?;
        if r.eax != 0 {
            return announce(m, player, r.eax, event);
        }
    }
    if player == m.u32(event.wrapping_add(0x14))? {
        let r = find(2)?;
        if r.eax != 0 {
            return announce(m, player, r.eax, event);
        }
    }
    let team = m.i8(base.wrapping_add(0xc0))?;
    if team == m.u32(event.wrapping_add(0x10))? {
        let r = find(1)?;
        if r.eax != 0 {
            return announce(m, player, r.eax, event);
        }
    }
    if team == m.u32(event.wrapping_add(0x18))? {
        let r = find(3)?;
        if r.eax != 0 {
            return announce(m, player, r.eax, event);
        }
    }
    let r = find(4)?;
    if r.eax != 0 {
        return announce(m, player, r.eax, event);
    }
    Ok(r)
}

/// `19eb30 (event: edi)`: the event's player announced, or every player
/// when it names none: then the gate calls the whole event
/// ([`Target::Event`], recorded in `m`).
pub fn announce_event(m: &Mem, event: u32) -> Result<Regs, GuestFault> {
    let player = m.u32(event.wrapping_add(8))?;
    if player == u32::MAX {
        m.record(Target::Event, event);
        return Ok(Regs { eax: u32::MAX, ecx: 0, edx: 0 });
    }
    announce_player(m, event, player)
}

/// `19eb90 (event: eax)`: nothing on a client (mode 4); else the post
/// `a7c50` and the announcements `19eb30`. `esp` is the call's entry `esp`.
pub fn death_event(m: &Mem, mut r: Regs, esp: u32, event: u32) -> Result<Regs, GuestFault> {
    r.eax = m.u32(MODE)?;
    if m.u8(r.eax.wrapping_add(0xc))? == 4 {
        return Ok(r);
    }
    r.edx = event;
    // `push edi`, then the call's return address.
    let posted = event_post(m, r, esp.wrapping_sub(8))?;
    if m.call().is_some() {
        return Ok(posted);
    }
    announce_event(m, event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    const MODE_RECORD: u32 = 0x0060_0000;
    const NETWORK_RECORD: u32 = 0x0060_1000;
    const EVENT: u32 = 0x0060_2000;
    const ESP: u32 = 0x007f_0000;

    fn world(mode: u8, state: u32) -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(0x004c_0000, 0x0004_0000, Permissions::RW).unwrap();
        g.map_zeroed(MODE_RECORD, 0x0001_0000, Permissions::RW).unwrap();
        g.set_u32(MODE, MODE_RECORD).unwrap();
        g.set_u8(MODE_RECORD + 0xc, mode).unwrap();
        g.set_u32(NETWORK, NETWORK_RECORD).unwrap();
        g.set_u32(NETWORK_RECORD + 8, state).unwrap();
        g.set_u32(POST_TAG, 0x0bad_7a90).unwrap();
        g
    }

    #[test]
    fn a_host_with_a_live_network_calls_the_whole_event() {
        let g = world(5, 4);
        let m = Mem::new(&g);
        let r = event_post(&m, Regs { eax: 0, ecx: 0, edx: EVENT }, ESP).unwrap();
        assert_eq!(r, Regs { eax: NETWORK_RECORD, ecx: 0x0bad_7a90, edx: ESP - 0x24 });
        let call = m.call().expect("the gate calls 19eb90");
        assert_eq!((call.target, call.event), (Target::Event, EVENT));
        // The first call stands.
        m.record(Target::Announce { player: 1, row: 2 }, EVENT);
        assert_eq!(m.call().unwrap().target, Target::Event);
    }

    #[test]
    fn a_client_or_an_idle_network_needs_no_call() {
        for (mode, state) in [(4, 4), (5, 2), (3, 5)] {
            let g = world(mode, state);
            let m = Mem::new(&g);
            event_post(&m, Regs { eax: 0, ecx: 0, edx: EVENT }, ESP).unwrap();
            assert_eq!(m.call(), None, "mode {mode}, state {state}");
        }
    }

    #[test]
    fn an_event_naming_no_player_calls_the_whole_event() {
        let mut g = world(5, 0);
        g.set_u32(EVENT + 8, u32::MAX).unwrap();
        let m = Mem::new(&g);
        assert_eq!(announce_event(&m, EVENT).unwrap(), Regs { eax: u32::MAX, ecx: 0, edx: 0 });
        assert_eq!(m.call().map(|c| c.target), Some(Target::Event));
    }
}
