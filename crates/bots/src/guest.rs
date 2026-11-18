//! The game's memory as the bot driver uses it (docs/game/bots.md,
//! sections 2 and 4): local player slots and signing a bot in, the local
//! users the views and listeners are made for, and what a bot sees.
//!
//! Everything here works on [`GuestMemory`], so the tests run on synthetic
//! guest memory. Addresses are research notes; no game data is here.

use engine::entry::{Cx, EntryError, GuestCall, Ret};
use engine::handle::DatumHandle;
use engine::memory::{GuestMemory, MemFault};
use engine::points;
use engine::view::{objects, players};

use crate::brain::{Me, Perception, Seen};

pub type R<T> = Result<T, MemFault>;

// ---- local player slots (section 4.2) ----

/// The four local player slots, one per controller port.
pub const SLOTS: u32 = 0x0054_e8e0;
pub const SLOT_STRIDE: u32 = 0xc70;
/// Slot `+0` bit: a profile is signed in (`0x18fb34` sets it).
pub const SLOT_PROFILE: u32 = 0x10;
/// The profile block: `0x1e0` bytes at slot `+0x18`.
pub const SLOT_BLOCK: u32 = 0x18;
pub const BLOCK_LEN: usize = 0x1e0;
/// The name the game shows for the slot: UTF-16, 32 units, at slot `+0xc1c`
/// (`0x1902c1`).
pub const SLOT_NAME: u32 = 0xc1c;
pub const SLOT_NAME_UNITS: usize = 32;

/// In the block: the name `0x190001` writes, 16 single-byte characters,
/// the 16th always NUL.
pub const BLOCK_NAME: usize = 0x48;
pub const BLOCK_NAME_LEN: usize = 16;
/// In the block: the controls record `0x190c34` reads (slot `+0x114`): a
/// flags dword (bit 0 and bit 2 are read), then the button layout, the
/// thumbstick layout and the look sensitivity (1 to 10).
pub const BLOCK_CONTROLS: usize = 0xfc;
/// The bot's look sensitivity, which the brain's turning is tuned to.
pub const BOT_SENSITIVITY: u8 = 3;
/// In the block: the player's appearance, 16 bytes (slot `+0x130`, which
/// `0x18fd94` copies to the player properties' `+0x40`). The block check
/// `0x1a0540` zeroes all 16 when `0x153750` finds them bad, which reads
/// the first 8: four colours (`+0..+3`, -1 or below 18: armour primary and
/// secondary, emblem primary and secondary), the model (`+4`, below 4),
/// the emblem's foreground (`+5`, below 0x40) and background (`+6`, below
/// 0x20), and emblem flags (`+7`, high nibble clear). `+8..+0x10` are not
/// checked; a bot keeps the human's.
pub const BLOCK_LOOK: usize = 0x118;
/// How many player colours there are (`0x153750`'s bound).
pub const COLOURS: u8 = 18;
/// The emblem bounds `0x153750` checks.
pub const EMBLEM_FOREGROUNDS: u8 = 0x40;
pub const EMBLEM_BACKGROUNDS: u8 = 0x20;

/// A bot's own colours and emblem, by its port (1 to 3; other ports take
/// port 1's): four colours and the emblem's foreground and background.
/// Indices only; which colour each is, is the game's palette.
pub const BOT_LOOKS: [([u8; 4], u8, u8); 3] = [
    ([3, 1, 3, 1], 5, 2),
    ([6, 9, 6, 9], 12, 7),
    ([12, 4, 12, 4], 20, 11),
];

/// The look `bot_block` writes for the bot on `port`: [`BOT_LOOKS`], with
/// each armour colour that is the human's (`human`, their 16 look bytes)
/// moved one on, so the bot never wears the human's colours.
pub fn bot_look(port: u32, human: &[u8]) -> [u8; 8] {
    let i = (port as usize).clamp(1, 3) - 1;
    let (mut colours, fg, bg) = BOT_LOOKS[i];
    for (k, c) in colours.iter_mut().enumerate().take(2) {
        if human.get(k) == Some(&*c) {
            *c = (*c + 1) % COLOURS;
        }
    }
    let model = human.get(4).copied().filter(|&m| m < 4).unwrap_or(0);
    [colours[0], colours[1], colours[2], colours[3], model, fg, bg, 0]
}

pub fn slot(port: u32) -> u32 {
    SLOTS + port * SLOT_STRIDE
}

/// Whether `port`'s slot has a profile signed in.
pub fn signed_in(m: &dyn GuestMemory, port: u32) -> R<bool> {
    Ok(m.u32(slot(port))? & SLOT_PROFILE != 0)
}

/// The first port with a profile that is not a bot's (`bots` is a port
/// mask): the human whose block a bot's is made from.
pub fn human_port(m: &dyn GuestMemory, bots: u32) -> R<Option<u32>> {
    for port in 0..4 {
        if bots & 1 << port == 0 && signed_in(m, port)? {
            return Ok(Some(port));
        }
    }
    Ok(None)
}

/// A bot's profile block: the human's, with the bot's name, its own look
/// ([`bot_look`]: colours and emblem by `port`, the human's model) and
/// controls that mean the same on every machine (inversion and the other
/// flag off, the Default layouts, a fixed sensitivity). Everything else is
/// the human's.
pub fn bot_block(human: &[u8; BLOCK_LEN], name: &str, port: u32) -> [u8; BLOCK_LEN] {
    let mut b = *human;
    let look = bot_look(port, &human[BLOCK_LOOK..BLOCK_LOOK + 16]);
    b[BLOCK_LOOK..BLOCK_LOOK + 8].copy_from_slice(&look);
    let field = &mut b[BLOCK_NAME..BLOCK_NAME + BLOCK_NAME_LEN];
    field.fill(0);
    for (dst, c) in field[..BLOCK_NAME_LEN - 1]
        .iter_mut()
        .zip(name.bytes().filter(u8::is_ascii))
    {
        *dst = c;
    }
    let c = BLOCK_CONTROLS;
    let flags = u32::from_le_bytes(b[c..c + 4].try_into().unwrap()) & !0b101;
    b[c..c + 4].copy_from_slice(&flags.to_le_bytes());
    b[c + 4] = 0;
    b[c + 5] = 0;
    b[c + 6] = BOT_SENSITIVITY;
    b
}

/// The name the game shows for `port`'s slot.
pub fn set_slot_name(m: &mut dyn GuestMemory, port: u32, name: &str) -> R<()> {
    let at = slot(port) + SLOT_NAME;
    let mut units: Vec<u16> = name.encode_utf16().take(SLOT_NAME_UNITS - 1).collect();
    units.resize(SLOT_NAME_UNITS, 0);
    let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
    m.write(at, &bytes)
}

/// The players screen's event record: `+4` the controller, `+8` the button.
pub const EVENT_CONTROLLER: u32 = 4;
pub const EVENT_BUTTON: u32 = 8;
pub const BUTTON_A: u32 = 0;
pub const BUTTON_START: u32 = 0xc;
/// The screen's mask of joined controllers (`u16`, `or [screen+8], ax`).
pub const SCREEN_MASK: u32 = 8;

/// Whether the players screen's unjoined event at `event` is a bot asking
/// to join: A or Start from a bot's port (`bots`, a mask) whose slot has no
/// profile yet.
pub fn bot_join(m: &dyn GuestMemory, event: u32, bots: u32) -> R<Option<u32>> {
    let port = m.u32(event + EVENT_CONTROLLER)?;
    let button = m.u32(event + EVENT_BUTTON)?;
    if port >= 4 || bots & 1 << port == 0 || !matches!(button, BUTTON_A | BUTTON_START) {
        return Ok(None);
    }
    Ok((!signed_in(m, port)?).then_some(port))
}

/// In place of the players screen's unjoined handler for a bot's A: signs
/// the bot in with the game's own `PROFILE_SIGN_IN` (no file, so nothing is
/// saved), names its slot, and joins its port on the screen, as the handler
/// does for a human once a profile is picked. `buffer` is `BLOCK_LEN` bytes
/// of guest memory of ours. Returns as the handler does: AL 1, `ret 4`.
pub async fn sign_in(
    cx: Cx,
    port: u32,
    bots: u32,
    name: String,
    buffer: u32,
) -> Result<Ret, EntryError> {
    let screen = cx.arg(0)?;
    let made = cx.write(|m| -> R<bool> {
        let Some(human) = human_port(m, bots)? else {
            return Ok(false);
        };
        let mut block = [0u8; BLOCK_LEN];
        m.read(slot(human) + SLOT_BLOCK, &mut block)?;
        m.write(buffer, &bot_block(&block, &name, port))?;
        Ok(true)
    })?;
    if !made {
        return Ok(Ret::new(1, 4));
    }
    cx.call(GuestCall::new(points::PROFILE_SIGN_IN.base).args([port, buffer, u32::MAX]))
        .await?;
    cx.write(|m| -> R<()> {
        set_slot_name(m, port, &name)?;
        let mask = m.u16(screen + SCREEN_MASK)?;
        m.set_u16(screen + SCREEN_MASK, mask | 1 << port)
    })?;
    Ok(Ret::new(1, 4))
}

/// Signs the profiles on `ports` out with the game's own
/// `PROFILE_SIGN_OUT` (slot in EAX), then calls `then` (a `fn() ret 0`
/// the hook answering for, so its frame still runs) and returns its EAX.
/// The leaving bots' slots are empty after; their pads go a frame later
/// ([`crate::local::leave_step`]).
pub async fn sign_out_then(cx: Cx, ports: Vec<u8>, then: u32) -> Result<Ret, EntryError> {
    for p in ports {
        cx.call(GuestCall::new(points::PROFILE_SIGN_OUT.base).eax(u32::from(p)))
            .await?;
    }
    let r = cx.call(GuestCall::new(then)).await?;
    Ok(Ret::new(r.eax, 0))
}

// ---- the local users (section 4.3) ----

/// Players globals offsets.
pub const LOCAL_COUNT: u32 = 8;
pub const LOCAL_MAP: u32 = 0xc;
/// Player `+0x24`: the local controller (`0x186b0c` hands it to the
/// control update as EDX).
pub const PLAYER_CONTROLLER: u32 = 0x24;

fn player_at(m: &dyn GuestMemory, handle: u32) -> R<Option<players::Player>> {
    let Some(array) = players::array(m)? else {
        return Ok(None);
    };
    let Some(h) = DatumHandle::from_raw(handle) else {
        return Ok(None);
    };
    array.get(m, h)
}

/// The local users (0 to 3) whose player is on a bot's port.
pub fn bot_users(m: &dyn GuestMemory, bots: u32) -> R<Vec<u32>> {
    let globals = m.u32(points::PLAYER_GLOBALS.base)?;
    let mut out = Vec::new();
    if globals == 0 {
        return Ok(out);
    }
    for u in 0..4 {
        let handle = m.u32(globals + LOCAL_MAP + 4 * u)?;
        if handle == u32::MAX {
            continue;
        }
        if let Some(p) = player_at(m, handle)? {
            let controller = m.i32(p.addr + PLAYER_CONTROLLER)?;
            if (0..4).contains(&controller) && bots & 1 << controller != 0 {
                out.push(u);
            }
        }
    }
    Ok(out)
}

/// What [`hide`] changed, for [`restore`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hidden {
    globals: u32,
    count: u16,
    map: [u32; 4],
}

/// Takes the bots' local users out of the local player count and map, so
/// that what runs next (the render's views and HUDs, the listeners) is
/// made for the humans only. The humans keep their order. Does nothing,
/// and returns `None`, when there is no bot user, or when every user is a
/// bot (a view is still needed).
pub fn hide(m: &mut dyn GuestMemory, bots: u32) -> R<Option<Hidden>> {
    if bots == 0 {
        return Ok(None);
    }
    let users = bot_users(m, bots)?;
    let globals = m.u32(points::PLAYER_GLOBALS.base)?;
    if users.is_empty() || globals == 0 {
        return Ok(None);
    }
    let count = m.u16(globals + LOCAL_COUNT)?;
    let mut map = [0; 4];
    for (u, v) in (0u32..).zip(map.iter_mut()) {
        *v = m.u32(globals + LOCAL_MAP + 4 * u)?;
    }
    let mapped = map.iter().filter(|&&h| h != u32::MAX).count();
    if users.len() >= mapped {
        return Ok(None);
    }
    for &u in &users {
        m.set_u32(globals + LOCAL_MAP + 4 * u, u32::MAX)?;
    }
    let left = (count as i16 - users.len() as i16).max(1) as u16;
    m.set_u16(globals + LOCAL_COUNT, left)?;
    Ok(Some(Hidden {
        globals,
        count,
        map,
    }))
}

/// Puts back what [`hide`] changed.
pub fn restore(m: &mut dyn GuestMemory, h: &Hidden) -> R<()> {
    m.set_u16(h.globals + LOCAL_COUNT, h.count)?;
    for (u, v) in (0u32..).zip(h.map) {
        m.set_u32(h.globals + LOCAL_MAP + 4 * u, v)?;
    }
    Ok(())
}

// ---- what a bot sees (section 3.1, stage 1) ----

/// Game options: `[GAME_GLOBALS] + 8`; `+0` the mode (2 multiplayer), the
/// variant at `+0x134`, its flags at `+0x48` (bit 0 a team game). As Live
/// telemetry reads them.
pub const OPTIONS: u32 = 8;
pub const MODE_MULTIPLAYER: u32 = 2;
pub const VARIANT_FLAGS: u32 = 0x134 + 0x48;
/// The game-time record's tick count (`[GAME_TIME] + 8`).
pub const GAME_TIME_TICK: u32 = 8;
/// Unit `+0x168`: the aim, a world-space unit vector.
pub const UNIT_AIM: u32 = 0x168;
/// Object body damage flags bit 2: the body is depleted (dead).
pub const BODY_DEPLETED: u16 = 1 << 2;

/// A player's living unit: its origin and aim.
fn living_unit(m: &dyn GuestMemory, p: &players::Player) -> R<Option<([f32; 3], [f32; 3])>> {
    let Some(unit) = p.unit(m)? else {
        return Ok(None);
    };
    let Some((kind, body)) = objects::get(m, unit)? else {
        return Ok(None);
    };
    if kind != objects::BIPED || body.damage_flags(m)? & BODY_DEPLETED != 0 {
        return Ok(None);
    }
    Ok(Some((body.position(m)?, m.vec3(body.addr + UNIT_AIM)?)))
}

/// The game's tick count (30 a second) while a game is loaded: `None` in
/// the menus, where the game-time record is not allocated.
pub fn game_tick(m: &dyn GuestMemory) -> Option<u32> {
    let time = m.u32(points::GAME_TIME.base).ok().filter(|&t| t != 0)?;
    m.u32(time + GAME_TIME_TICK).ok()
}

/// Which player a bot is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    /// A local bot: the local player on this controller port.
    Port(u32),
    /// A phantom: the player of this machine id (player `+0x14`).
    Machine([u8; 6]),
}

/// Player `+0x14`: the player's machine id, 6 bytes.
pub const PLAYER_MACHINE: u32 = 0x14;

/// Player `p`'s machine id.
pub fn player_machine(m: &dyn GuestMemory, p: u32) -> R<[u8; 6]> {
    let mut id = [0; 6];
    for (i, b) in (0u32..).zip(id.iter_mut()) {
        *b = m.u8(p + PLAYER_MACHINE + i)?;
    }
    Ok(id)
}

fn is(m: &dyn GuestMemory, p: &players::Player, who: Who) -> R<bool> {
    Ok(match who {
        Who::Port(port) => {
            p.local_user(m)? >= 0 && m.i32(p.addr + PLAYER_CONTROLLER)? == port as i32
        }
        Who::Machine(id) => player_machine(m, p.addr)? == id,
    })
}

/// What the bot on `port` sees (see [`perceive_as`]).
pub fn perceive(m: &dyn GuestMemory, port: u32) -> R<Perception> {
    perceive_as(m, Who::Port(port))
}

/// What the bot `who` sees: in a multiplayer game, itself and every other
/// player with a living unit. Outside one, nothing.
pub fn perceive_as(m: &dyn GuestMemory, who: Who) -> R<Perception> {
    let mut out = Perception::default();
    let globals = m.u32(points::GAME_GLOBALS.base)?;
    if globals == 0 || m.u32(globals + OPTIONS)? != MODE_MULTIPLAYER {
        return Ok(out);
    }
    out.teams = m.u32(globals + OPTIONS + VARIANT_FLAGS)? & 1 != 0;
    out.tick = game_tick(m).unwrap_or(0);
    let Some(array) = players::array(m)? else {
        return Ok(out);
    };
    for (handle, p) in array.live(m, 16)? {
        if !p.listed(m)? {
            continue;
        }
        let Some((position, aim)) = living_unit(m, &p)? else {
            continue;
        };
        let team = i16::from(p.team(m)?);
        if is(m, &p, who)? {
            out.me = Some(Me {
                position,
                aim,
                team,
            });
        } else {
            out.others.push(Seen {
                id: u32::from(handle.index()),
                position,
                team,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
