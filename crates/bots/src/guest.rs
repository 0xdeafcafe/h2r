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

/// In the block: the profile's name, UTF-16, 32 units (the profile file's
/// `+8`: "Halo0001"). The game builds the slot's display name ([`SLOT_NAME`])
/// from it (`0x190eb3`: slot `+0x20`, 32 units, to `+0xc1c`) at sign-in and
/// again whenever the slot's Live state or account changes (`0x190074`,
/// `0x18fee9`), so a name written only to [`SLOT_NAME`] does not last: on
/// Xbox Live every bot showed the human's profile name, and the session's
/// unique-name pass (`0x7f660`, from `0x65e37`) renamed the copies from the
/// game's default names (`0x19fd00`: "Shadow", "Wilshire").
pub const BLOCK_PROFILE_NAME: usize = 8;
pub const BLOCK_PROFILE_NAME_UNITS: usize = 32;
/// In the block: the 16 single-byte characters `0x190001` writes (the 16th
/// always NUL; the profile UI's commit fills it, `0x24b9ce`). Empty in a
/// saved profile; a bot's holds its name too.
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
    [
        colours[0], colours[1], colours[2], colours[3], model, fg, bg, 0,
    ]
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

/// `name` as the block's profile name: UTF-16, at most 31 units, the rest
/// NUL.
pub fn profile_name_bytes(name: &str) -> [u8; BLOCK_PROFILE_NAME_UNITS * 2] {
    let mut out = [0u8; BLOCK_PROFILE_NAME_UNITS * 2];
    for (dst, u) in out
        .chunks_exact_mut(2)
        .zip(name.encode_utf16().take(BLOCK_PROFILE_NAME_UNITS - 1))
    {
        dst.copy_from_slice(&u.to_le_bytes());
    }
    out
}

/// A bot's profile block: the human's, with the bot's name (the profile
/// name the game shows, [`BLOCK_PROFILE_NAME`], and [`BLOCK_NAME`]), its own look
/// ([`bot_look`]: colours and emblem by `port`, the human's model) and
/// controls that mean the same on every machine (inversion and the other
/// flag off, the Default layouts, a fixed sensitivity). Everything else is
/// the human's.
pub fn bot_block(human: &[u8; BLOCK_LEN], name: &str, port: u32) -> [u8; BLOCK_LEN] {
    let mut b = *human;
    let look = bot_look(port, &human[BLOCK_LOOK..BLOCK_LOOK + 16]);
    b[BLOCK_LOOK..BLOCK_LOOK + 8].copy_from_slice(&look);
    b[BLOCK_PROFILE_NAME..BLOCK_PROFILE_NAME + BLOCK_PROFILE_NAME_UNITS * 2]
        .copy_from_slice(&profile_name_bytes(name));
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

/// Signs the bot on `port` in with the game's own `PROFILE_SIGN_IN` (no
/// file, so nothing is saved): its block made from the human's
/// ([`bot_block`]), its slot named, and, given the players screen at
/// `screen`, its port joined on that screen as the screen's own join does
/// once a profile is picked. `buffer` is `BLOCK_LEN` bytes of guest memory of
/// ours. False, with nothing done, when no human has a profile to copy.
async fn sign_in_one(
    cx: Cx,
    port: u32,
    bots: u32,
    name: &str,
    buffer: u32,
    screen: Option<u32>,
) -> Result<bool, EntryError> {
    let made = cx.write(|m| -> R<bool> {
        let Some(human) = human_port(m, bots)? else {
            return Ok(false);
        };
        let mut block = [0u8; BLOCK_LEN];
        m.read(slot(human) + SLOT_BLOCK, &mut block)?;
        m.write(buffer, &bot_block(&block, name, port))?;
        Ok(true)
    })?;
    if !made {
        return Ok(false);
    }
    cx.call(GuestCall::new(points::PROFILE_SIGN_IN.base).args([port, buffer, u32::MAX]))
        .await?;
    cx.write(|m| -> R<()> {
        set_slot_name(m, port, name)?;
        if let Some(screen) = screen {
            let mask = m.u16(screen + SCREEN_MASK)?;
            m.set_u16(screen + SCREEN_MASK, mask | 1 << port)?;
        }
        Ok(())
    })?;
    Ok(true)
}

/// In place of the players screen's unjoined handler for a bot's A (a
/// script's, or a stray press): signs the bot in ([`sign_in_one`]) on the
/// screen the handler was given. Returns as the handler does: AL 1, `ret 4`.
pub async fn sign_in(
    cx: Cx,
    port: u32,
    bots: u32,
    name: String,
    buffer: u32,
) -> Result<Ret, EntryError> {
    let screen = cx.arg(0)?;
    sign_in_one(cx, port, bots, &name, buffer, Some(screen)).await?;
    Ok(Ret::new(1, 4))
}

/// Signs the bots in `joins` (port, name) in ([`sign_in_one`], joining
/// each on the players screen at `screen`), then calls `then` (a `fn() ret
/// 0` the hook is answering for, so its frame still runs) and returns its
/// EAX. A bot never presses a button to join: any press from a controller
/// with no profile, or with a profile not on Xbox Live, opens a profile
/// chooser somewhere (the players screen `0x252acc`, the pregame lobby
/// `0x250cc3` and `0x2511b6`), and a chooser a bot's A reaches picks the
/// human's profile.
pub async fn sign_in_then(
    cx: Cx,
    joins: Vec<(u32, String)>,
    bots: u32,
    buffer: u32,
    screen: Option<u32>,
    then: u32,
) -> Result<Ret, EntryError> {
    for (port, name) in &joins {
        sign_in_one(cx.clone(), *port, bots, name, buffer, screen).await?;
    }
    let r = cx.call(GuestCall::new(then)).await?;
    Ok(Ret::new(r.eax, 0))
}

/// Signs the slots on `ports` out altogether with the game's own
/// `SLOT_SIGN_OUT` (slot in EAX: off Xbox Live, its local user freed, the
/// profile signed out), then calls `then` (a `fn() ret 0` the hook
/// answering for, so its frame still runs) and returns its EAX. The
/// leaving bots' slots are empty after ([`slot_held`] false); their pads
/// go a frame later ([`crate::local::leave_step`]).
///
/// Not `PROFILE_SIGN_OUT` alone: that clears the profile and nothing
/// else, so the slot kept its local user with its Live identity, and the
/// squad session (which drops a player only when its local user goes,
/// `0x65770` status 0) kept the bot in the lobby (the user's run,
/// 2026-11-19: "i turned them off and they stay in the lobby").
pub async fn sign_out_then(cx: Cx, ports: Vec<u8>, then: u32) -> Result<Ret, EntryError> {
    for p in ports {
        cx.call(GuestCall::new(points::SLOT_SIGN_OUT.base).eax(u32::from(p)))
            .await?;
    }
    let r = cx.call(GuestCall::new(then)).await?;
    Ok(Ret::new(r.eax, 0))
}

// ---- Xbox Live guests (section 4.2, "Xbox Live guests") ----

/// Slot `+0` bit: the slot is signed in to Xbox Live (`0x190074` sets and
/// clears it; the profile UI's Live commit sets it at `0x24b99a`).
pub const SLOT_LIVE: u32 = 0x20;
/// Slot `+4`: the slot's local user, an index into [`LOCAL_USERS`] (-1
/// none). The profile sign-in takes one (`0x64610`) for a slot that had no
/// profile.
pub const SLOT_LOCAL_USER: u32 = 4;
/// Slot `+8`: the slot's player id, 12 bytes: an XUID (the account's
/// 8-byte id, then its user flags). The account commit copies it from the
/// account record (`0x18fee9`, `0x18ff39`).
pub const SLOT_ID: u32 = 8;
pub const ID_LEN: usize = 12;
/// Slot `+0x470`: the Xbox Live account record (`XONLINE_USER`, `0x70`
/// bytes): `+0` the user id, `+8` the user flags, `+0xc` the gamertag (16
/// bytes, `0x190eb3`), `+0x6c` the logon result (`0x6c6c0`).
pub const SLOT_ACCOUNT: u32 = 0x470;
pub const ACCOUNT_LEN: usize = 0x70;
pub const ACCOUNT_FLAGS: usize = 8;
/// The user flags' guest number: 0 for the account's own player, 1 to 3
/// for its guests (`0x1900a5` tests `flags & 3`; `0x190eb3` names a guest
/// "gamertag(n)" from it).
pub const GUEST_MASK: u32 = 3;
/// The local users (`0x4cd904`, 4 of `0xd0` bytes): `+0` in use, `+1` the
/// player id the session knows the user by (`0x64820` copies a slot's
/// [`SLOT_ID`] there).
pub const LOCAL_USERS: u32 = 0x004c_d904;
pub const LOCAL_USER_STRIDE: u32 = 0xd0;
pub const LOCAL_USER_COUNT: u32 = 4;
pub const LOCAL_USER_ID: u32 = 1;

/// The slot's flags (`+0`).
pub fn slot_flags(m: &dyn GuestMemory, port: u32) -> R<u32> {
    m.u32(slot(port))
}

/// Whether `port`'s slot still holds anything a leaving bot must give up
/// before its pad goes: a profile or Xbox Live ([`SLOT_PROFILE`],
/// [`SLOT_LIVE`]). `SLOT_SIGN_OUT` clears both, with the slot's local
/// user; a slot left on Live with no profile is signed out again.
pub fn slot_held(m: &dyn GuestMemory, port: u32) -> R<bool> {
    Ok(slot_flags(m, port)? & (SLOT_PROFILE | SLOT_LIVE) != 0)
}

/// Whether `port`'s slot is signed in to Xbox Live.
pub fn on_live(m: &dyn GuestMemory, port: u32) -> R<bool> {
    Ok(slot_flags(m, port)? & SLOT_LIVE != 0)
}

/// `port`'s Xbox Live account record.
pub fn account(m: &dyn GuestMemory, port: u32) -> R<[u8; ACCOUNT_LEN]> {
    let mut a = [0u8; ACCOUNT_LEN];
    m.read(slot(port) + SLOT_ACCOUNT, &mut a)?;
    Ok(a)
}

/// An account record's user id.
pub fn account_xuid(a: &[u8; ACCOUNT_LEN]) -> u64 {
    u64::from_le_bytes(a[..8].try_into().unwrap())
}

/// An account record's guest number (0: the account's own player).
pub fn guest_number(a: &[u8; ACCOUNT_LEN]) -> u32 {
    u32::from_le_bytes(a[ACCOUNT_FLAGS..ACCOUNT_FLAGS + 4].try_into().unwrap()) & GUEST_MASK
}

/// `host`'s record as its guest number `n`'s: the same account, with the
/// guest number in the user flags. Everything else (the gamertag, the
/// logon result) is the host's, as a guest's is on an Xbox.
pub fn guest_account(host: &[u8; ACCOUNT_LEN], n: u32) -> [u8; ACCOUNT_LEN] {
    let mut a = *host;
    let f = ACCOUNT_FLAGS;
    let flags = u32::from_le_bytes(a[f..f + 4].try_into().unwrap()) & !GUEST_MASK | n & GUEST_MASK;
    a[f..f + 4].copy_from_slice(&flags.to_le_bytes());
    a
}

/// The human the bots are guests of: the first port that is not a bot's
/// (`bots`, a mask) with a profile signed in to Xbox Live as an account's
/// own player (a human guest has no guests of its own).
pub fn live_host(m: &dyn GuestMemory, bots: u32) -> R<Option<u32>> {
    for port in 0..4 {
        if bots & 1 << port != 0 {
            continue;
        }
        let flags = slot_flags(m, port)?;
        if flags & SLOT_PROFILE != 0
            && flags & SLOT_LIVE != 0
            && guest_number(&account(m, port)?) == 0
        {
            return Ok(Some(port));
        }
    }
    Ok(None)
}

/// The lowest guest number (1 to 3) no Live slot of `host`'s account
/// holds, or `None` when all three are taken.
pub fn free_guest_number(m: &dyn GuestMemory, host: u32) -> R<Option<u32>> {
    let id = account_xuid(&account(m, host)?);
    let mut used = 0u32;
    for port in 0..4 {
        if port == host || !on_live(m, port)? {
            continue;
        }
        let a = account(m, port)?;
        if account_xuid(&a) == id {
            used |= 1 << guest_number(&a);
        }
    }
    Ok((1..=GUEST_MASK).find(|n| used & 1 << n == 0))
}

/// What [`go_live`] or [`go_live_as`] did for one bot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoLive {
    /// Signed in as guest `number` of the human on port `host`.
    Guest { host: u32, number: u32 },
    /// Signed in as its own bot account, beside the human on port `host`.
    Account { host: u32 },
    /// No human is on Xbox Live: nothing to be a guest of.
    NoHost,
    /// The bot's slot has no profile yet.
    NoProfile,
    /// The bot's slot is on Xbox Live already.
    Already,
    /// The host's account has three guests already.
    Full,
    /// The bot's slot has no local user (slot `+4`) to give its id to.
    NoLocalUser,
}

/// Signs the bot on `port` (its profile signed in) in to Xbox Live as a
/// guest of the human's account ([`live_host`]), as the game's own account
/// commit does for a guest (`0x24b99a`: `0x190074(port, 1)`, then
/// `0x18fee9`): the slot's Live flag, the account record (the host's, with
/// the guest number, [`guest_account`]) at `+0x470`, its player id at
/// [`SLOT_ID`] and in the slot's local user (`0x64820`), so the session
/// knows the bot as the host's guest. It leaves out the commit's display
/// name (`0x190eb3` would make it "gamertag(n)"): the slot keeps the bot's
/// `name`. No service logon is made for a guest: the Xbox Live service
/// (and our server) only ever sees the host's account.
pub fn go_live(m: &mut dyn GuestMemory, port: u32, bots: u32, name: &str) -> R<GoLive> {
    let flags = slot_flags(m, port)?;
    if flags & SLOT_PROFILE == 0 {
        return Ok(GoLive::NoProfile);
    }
    if flags & SLOT_LIVE != 0 {
        return Ok(GoLive::Already);
    }
    let Some(host) = live_host(m, bots)? else {
        return Ok(GoLive::NoHost);
    };
    let Some(number) = free_guest_number(m, host)? else {
        return Ok(GoLive::Full);
    };
    let user = m.i32(slot(port) + SLOT_LOCAL_USER)?;
    if !(0..LOCAL_USER_COUNT as i32).contains(&user) {
        return Ok(GoLive::NoLocalUser);
    }
    let entry = LOCAL_USERS + user as u32 * LOCAL_USER_STRIDE;
    if m.u8(entry)? == 0 {
        return Ok(GoLive::NoLocalUser);
    }
    let a = guest_account(&account(m, host)?, number);
    m.write(slot(port) + SLOT_ACCOUNT, &a)?;
    m.write(slot(port) + SLOT_ID, &a[..ID_LEN])?;
    m.write(entry + LOCAL_USER_ID, &a[..ID_LEN])?;
    m.set_u32(slot(port), flags | SLOT_LIVE)?;
    set_slot_name(m, port, name)?;
    Ok(GoLive::Guest { host, number })
}

/// Record offset of the gamertag (16 bytes, NUL-terminated).
pub const ACCOUNT_GAMERTAG: usize = 0xc;
pub const ACCOUNT_GAMERTAG_LEN: usize = 16;

/// A bot account the server leased to this console (docs/game/bots.md
/// §9.5): a roster bot's xuid (the server's `0x000B` range) and gamertag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotAccount {
    pub xuid: u64,
    pub gamertag: String,
}

/// `account`'s record, made from the host's: the bot's xuid, user flags
/// with no guest number (an account's own player), and its gamertag (at
/// most 15 bytes and a NUL). The rest (the logon result) is the host's, so
/// the game reads it as signed in as the host is.
pub fn bot_account_record(host: &[u8; ACCOUNT_LEN], account: &BotAccount) -> [u8; ACCOUNT_LEN] {
    let mut a = *host;
    a[..8].copy_from_slice(&account.xuid.to_le_bytes());
    let f = ACCOUNT_FLAGS;
    let flags = u32::from_le_bytes(a[f..f + 4].try_into().unwrap()) & !GUEST_MASK;
    a[f..f + 4].copy_from_slice(&flags.to_le_bytes());
    let tag = &mut a[ACCOUNT_GAMERTAG..ACCOUNT_GAMERTAG + ACCOUNT_GAMERTAG_LEN];
    tag.fill(0);
    let bytes = account.gamertag.as_bytes();
    let n = bytes.len().min(ACCOUNT_GAMERTAG_LEN - 1);
    tag[..n].copy_from_slice(&bytes[..n]);
    a
}

/// The slot's local-user entry (in use), or `None`.
fn local_user_entry(m: &dyn GuestMemory, port: u32) -> R<Option<u32>> {
    let user = m.i32(slot(port) + SLOT_LOCAL_USER)?;
    if !(0..LOCAL_USER_COUNT as i32).contains(&user) {
        return Ok(None);
    }
    let entry = LOCAL_USERS + user as u32 * LOCAL_USER_STRIDE;
    Ok((m.u8(entry)? != 0).then_some(entry))
}

/// Signs the bot on `port` (its profile signed in) in to Xbox Live as its
/// own bot `account`, beside the human on Live ([`live_host`]): the same
/// writes as [`go_live`] (the Live flag, the record at `+0x470`, its player
/// id at [`SLOT_ID`] and in the slot's local user), with
/// [`bot_account_record`] in place of a guest's. A slot on Live as a guest
/// or as another account is moved to this one; one on Live as this
/// account already is left alone. The slot's name becomes the gamertag.
///
/// The guest numbers stay free for real guests: the duplicate-account
/// check (`0x1900ff`) and the guest numbering compare user ids, and a bot
/// account's is its own. No logon is made: the Live HLE takes the xuid in
/// a user change only because the server leased it to this console.
pub fn go_live_as(
    m: &mut dyn GuestMemory,
    port: u32,
    bots: u32,
    account: &BotAccount,
) -> R<GoLive> {
    let flags = slot_flags(m, port)?;
    if flags & SLOT_PROFILE == 0 {
        return Ok(GoLive::NoProfile);
    }
    if flags & SLOT_LIVE != 0 && account_xuid(&self::account(m, port)?) == account.xuid {
        return Ok(GoLive::Already);
    }
    let Some(host) = live_host(m, bots)? else {
        return Ok(GoLive::NoHost);
    };
    let Some(entry) = local_user_entry(m, port)? else {
        return Ok(GoLive::NoLocalUser);
    };
    let a = bot_account_record(&self::account(m, host)?, account);
    m.write(slot(port) + SLOT_ACCOUNT, &a)?;
    m.write(slot(port) + SLOT_ID, &a[..ID_LEN])?;
    m.write(entry + LOCAL_USER_ID, &a[..ID_LEN])?;
    m.set_u32(slot(port), flags | SLOT_LIVE)?;
    set_slot_name(m, port, &account.gamertag)?;
    Ok(GoLive::Account { host })
}

/// The bot on `port` leaves Xbox Live (no human is on it any more): the
/// Live flag cleared, the record zeroed, and `offline_id` (the player id
/// its profile sign-in gave it, kept by the glue before it went on Live)
/// back at [`SLOT_ID`] and in its local user; its name back to `name`.
/// The game's own sign-out of a failed host logon (`0x190186`) only finds
/// slots with the host's user id, so a bot account is left for this.
pub fn leave_live(
    m: &mut dyn GuestMemory,
    port: u32,
    offline_id: &[u8; ID_LEN],
    name: &str,
) -> R<bool> {
    let flags = slot_flags(m, port)?;
    if flags & SLOT_LIVE == 0 {
        return Ok(false);
    }
    m.write(slot(port) + SLOT_ACCOUNT, &[0u8; ACCOUNT_LEN])?;
    m.write(slot(port) + SLOT_ID, offline_id)?;
    if let Some(entry) = local_user_entry(m, port)? {
        m.write(entry + LOCAL_USER_ID, offline_id)?;
    }
    m.set_u32(slot(port), flags & !SLOT_LIVE)?;
    set_slot_name(m, port, name)?;
    Ok(true)
}

/// The slot's player id (offline: the one its profile sign-in made).
pub fn slot_id(m: &dyn GuestMemory, port: u32) -> R<[u8; ID_LEN]> {
    let mut id = [0u8; ID_LEN];
    m.read(slot(port) + SLOT_ID, &mut id)?;
    Ok(id)
}

/// One port's slot as the start log says it: its flags and, on Xbox Live,
/// its guest number.
pub fn slot_summary(m: &dyn GuestMemory, port: u32) -> String {
    match slot_flags(m, port) {
        Err(_) => format!("port {port} ?"),
        Ok(f) if f & SLOT_LIVE != 0 => {
            let n = account(m, port).map(|a| guest_number(&a)).unwrap_or(0);
            if n == 0 {
                format!("port {port} {f:#x} Live")
            } else {
                format!("port {port} {f:#x} Live guest {n}")
            }
        }
        Ok(f) if f & SLOT_PROFILE != 0 => format!("port {port} {f:#x} offline"),
        Ok(f) => format!("port {port} {f:#x} empty"),
    }
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
    /// The users taken out, as a mask (bit `u` for user `u`).
    users: u32,
}

impl Hidden {
    /// The local users taken out, as a mask.
    pub fn users(&self) -> u32 {
        self.users
    }
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
        users: users.iter().fold(0, |acc, u| acc | 1 << u),
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

// ---- the bots' ears (section 4.3, "Listeners") ----

/// The sound globals (`[0x4e6380]`): one listener record per local user,
/// `0x48` bytes apart, user `u`'s "active" byte at `+0x8e + 0x48 * u`
/// (`0x127400` sets it for each mapped user and, for an unmapped one,
/// leaves it as it was; `0x127d00`, which picks the listener a starting
/// sound is for, walks the four and skips inactive ones).
pub const SOUND_GLOBALS: u32 = 0x004e_6380;
pub const LISTENER_ACTIVE: u32 = 0x8e;
pub const LISTENER_STRIDE: u32 = 0x48;

/// Marks the listeners of the users [`hide`] took out (`h`) inactive, so
/// no sound that starts is taken as heard by a bot. The listener update
/// (`0x127400`) never clears the byte for a user it skips, so a bot's
/// listener made before its user was hidden would otherwise stay active
/// where the bot last stood. Returns the users whose listener was active.
pub fn quiet_listeners(m: &mut dyn GuestMemory, h: &Hidden) -> R<u32> {
    let sound = m.u32(SOUND_GLOBALS)?;
    if sound == 0 {
        return Ok(0);
    }
    let mut quieted = 0;
    for u in 0..4u32 {
        if h.users & 1 << u == 0 {
            continue;
        }
        let at = sound + LISTENER_ACTIVE + LISTENER_STRIDE * u;
        if m.u8(at)? != 0 {
            m.set_u8(at, 0)?;
            quieted |= 1 << u;
        }
    }
    Ok(quieted)
}

// ---- the players screen's panes (section 4.3, "The players screen") ----

/// The game's controller handles, one dword per port (what `XInputOpen`
/// gave it; 0 is no controller). The players screen's update
/// (`PLAYERS_SCREEN_UPDATE`) reads them through `0x18f8b8` and `0x1249d0`.
pub const PAD_HANDLES: u32 = 0x004e_61cc;

/// What [`hide_pads`] changed, for [`restore_pads`]: each port's handle,
/// 0 where nothing was hidden.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HiddenPads {
    handles: [u32; 4],
}

impl HiddenPads {
    /// The ports hidden, as a mask.
    pub fn ports(&self) -> u32 {
        (0..4u32)
            .filter(|&p| self.handles[p as usize] != 0)
            .fold(0, |m, p| m | 1 << p)
    }
}

/// Makes the bots' controllers (`bots`, a port mask) look unplugged to
/// what runs next: their handles are zeroed. Around the players screen's
/// update, a bot's pane then reads as an empty port ("CONNECT
/// CONTROLLER"), whether or not the bot is signed in, and the human's
/// pane is untouched. `None` when no bot port has a controller.
pub fn hide_pads(m: &mut dyn GuestMemory, bots: u32) -> R<Option<HiddenPads>> {
    let mut h = HiddenPads::default();
    for p in 0..4u32 {
        if bots & 1 << p == 0 {
            continue;
        }
        let at = PAD_HANDLES + 4 * p;
        let handle = m.u32(at)?;
        if handle != 0 {
            h.handles[p as usize] = handle;
            m.set_u32(at, 0)?;
        }
    }
    Ok((h.ports() != 0).then_some(h))
}

/// Puts back what [`hide_pads`] zeroed, where the handle is still 0 (a
/// handle the game wrote in the meantime is kept).
pub fn restore_pads(m: &mut dyn GuestMemory, h: &HiddenPads) -> R<()> {
    for p in 0..4u32 {
        let handle = h.handles[p as usize];
        let at = PAD_HANDLES + 4 * p;
        if handle != 0 && m.u32(at)? == 0 {
            m.set_u32(at, handle)?;
        }
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

/// A player's living unit: its origin, aim and body.
fn living_unit(
    m: &dyn GuestMemory,
    p: &players::Player,
) -> R<Option<([f32; 3], [f32; 3], objects::Body)>> {
    let Some(unit) = p.unit(m)? else {
        return Ok(None);
    };
    let Some((kind, body)) = objects::get(m, unit)? else {
        return Ok(None);
    };
    if kind != objects::BIPED || body.damage_flags(m)? & BODY_DEPLETED != 0 {
        return Ok(None);
    }
    Ok(Some((
        body.position(m)?,
        m.vec3(body.addr + UNIT_AIM)?,
        body,
    )))
}

/// The game's tick count (30 a second) while a game is loaded, the main
/// menu's included: `mainmenu.map` runs as a game of its own (options
/// mode [`MODE_MAIN_MENU`]) with a game-time record that ticks. `None`
/// only while no game is loaded at all. For "a game runs", see
/// [`in_game`].
pub fn game_tick(m: &dyn GuestMemory) -> Option<u32> {
    let time = m.u32(points::GAME_TIME.base).ok().filter(|&t| t != 0)?;
    m.u32(time + GAME_TIME_TICK).ok()
}

/// The game options' mode for the main menu's game (1 campaign, 2
/// multiplayer, 3 the main menu; as Live telemetry names them).
pub const MODE_MAIN_MENU: u32 = 3;

/// The loaded game's options mode (`[GAME_GLOBALS] + 8`), if readable.
pub fn game_mode(m: &dyn GuestMemory) -> Option<u32> {
    let g = m.u32(points::GAME_GLOBALS.base).ok().filter(|&g| g != 0)?;
    m.u32(g + OPTIONS).ok()
}

/// Whether a campaign or multiplayer game runs: a game tick ([`game_tick`])
/// in a game that is not the main menu's. The `h2 bots test` run of
/// 2026-11-18 read a tick on every menu screen (`mode main_menu`), so
/// "a tick" alone kept the bots from ever signing in: the players
/// screen's gate said `signs in` and nothing signed in, and the pregame
/// lobby was never the lobby. A mode that cannot be read, with a tick,
/// counts as a game (the safe side for writes).
pub fn in_game(m: &dyn GuestMemory) -> bool {
    game_tick(m).is_some() && game_mode(m) != Some(MODE_MAIN_MENU)
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
/// player with a living unit, the weapons lying about and the objective.
/// Outside one, nothing.
pub fn perceive_as(m: &dyn GuestMemory, who: Who) -> R<Perception> {
    Ok(perception(m, who, false)?.0)
}

pub(crate) fn perceive_for_brain(
    m: &dyn GuestMemory,
    who: Who,
) -> R<(Perception, Vec<crate::grenade::Grenade>)> {
    perception(m, who, true)
}

fn perception(
    m: &dyn GuestMemory,
    who: Who,
    sense_grenades: bool,
) -> R<(Perception, Vec<crate::grenade::Grenade>)> {
    let mut out = Perception::default();
    let mut grenades = Vec::new();
    let globals = m.u32(points::GAME_GLOBALS.base)?;
    if globals == 0 || m.u32(globals + OPTIONS)? != MODE_MULTIPLAYER {
        return Ok((out, grenades));
    }
    out.teams = m.u32(globals + OPTIONS + VARIANT_FLAGS)? & 1 != 0;
    out.tick = game_tick(m).unwrap_or(0);
    let Some(array) = players::array(m)? else {
        return Ok((out, grenades));
    };
    let mut me_index = None;
    for (handle, p) in array.live(m, 16)? {
        if !p.listed(m)? {
            continue;
        }
        let Some((position, aim, body)) = living_unit(m, &p)? else {
            continue;
        };
        let team = i16::from(p.team(m)?);
        if is(m, &p, who)? {
            me_index = Some(u32::from(handle.index()));
            // Ammunition is extra: a fault reading it leaves the bot
            // playing without it (section 3.6), not blind.
            out.me = Some(Me {
                position,
                aim,
                team,
                shield: body.shield(m)?,
                health: body.health(m)?,
                arms: crate::arms::read(m, body.addr).ok(),
            });
        } else {
            out.others.push(Seen {
                id: u32::from(handle.index()),
                position,
                team,
            });
        }
    }
    if let Some(me) = me_index {
        let near = out.me.filter(|_| sense_grenades).map(|me| me.position);
        (out.pickups, grenades) = crate::arms::ground_items(m, near).unwrap_or_default();
        // The objective is extra too: a fault leaves the bot playing as in
        // Slayer (section 3.7).
        out.objective = crate::objective::read(m, me).ok().flatten();
    }
    Ok((out, grenades))
}

#[cfg(test)]
mod tests;
