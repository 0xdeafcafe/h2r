//! Recorded-animation event streams: how each game's version-4 player reads
//! them, and the rewrite from Halo 1's encoding to Halo 2's.
//!
//! A recording's data is a unit control stream. Both games read it the same
//! way at the top level (evidence below, from each `default.xbe`):
//!
//! 1. The initial unit control state, field by field, in the layout of the
//!    recording's unit control version (element `+0x22`). Version 0 reads as
//!    version 1. Each version appends fields to the one before.
//! 2. Twelve bytes copied as they are.
//! 3. Events. Each starts with a header byte. Its low two bits select the
//!    delay before the event: 0 for none, 1 for one tick, 2 for a byte that
//!    follows, 3 for a little-endian short that follows. Its upper six bits
//!    are the event type, which selects the handler through a table of
//!    function pointers. Type 0 has no handler (a delay only), type 1 ends
//!    the stream, and every other handler reads its own payload after the
//!    delay.
//!
//! The decoder loops are the same code in both games (Halo 2 `0x29f0c0`,
//! Halo 1 `0x119910`: `mov dl,[eax]`, `and ecx,3`, `jmp [ecx*4+table]`, then
//! `shr ecx,2` and `call [ecx*4+handlers]`). Neither bounds the type before
//! the call. Halo 2's handler table at `0x4710f0` has 24 entries (types 0 to
//! 23) and Halo 1's at `0x232538` has 23 (types 0 to 22); past the end are
//! unrelated words such as `0x00000001`, so an out-of-range type calls a
//! small integer. That is the fault a30 and a50 hit headless (pc 4 and pc 1,
//! stack `0x29f13f`, `0x1fb61e`): the two games' payloads differ for some
//! types, so a Halo 1 stream read by Halo 2's handlers loses its place and
//! a payload byte is read as a header.
//!
//! Payloads, in bytes after the header and delay (handler addresses are
//! Halo 2's, then Halo 1's):
//!
//! | type | Halo 2 | Halo 1 | field |
//! |---|---|---|---|
//! | 2 | 1 (`0x29ec30`) | 1 (`0x119b10`) | animation state |
//! | 3 | 1 (`0x29ec50`) | 1 (`0x119b00`) | aiming speed |
//! | 4 | 4 (`0x29ec70`) | 2 (`0x119ae0`) | control flags |
//! | 5 | 2 (`0x29ec90`) | 2 (`0x119ac0`) | weapon index |
//! | 6 | 8 (`0x29ecd0`) | 8 (`0x119aa0`) | throttle |
//! | 7 to 14 | 2 (`0x29edc0`) | 2 (`0x119d30`) | facing, aiming, looking deltas (bytes) |
//! | 15 to 22 | 4 (`0x29ef20`) | 4 (`0x119bb0`) | the same deltas (shorts) |
//! | 23 | 2 (`0x29ecb0`) | none | Halo 2's second weapon index |
//!
//! Halo 2 reads the control flags as a word where Halo 1 reads a half-word,
//! its initial state has wider fields, and its animation state is a mode
//! string ID where Halo 1's is a small enum; everything else is the same
//! bytes. [`convert`] rewrites those.

use std::fmt;

/// Which game's player reads the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Game {
    Halo1,
    Halo2,
}

/// Event types each game's handler table covers (Halo 2 `0x4710f0`, 24
/// entries; Halo 1 `0x232538`, 23 entries). A type at or past this calls
/// whatever word follows the table.
pub const fn handler_count(game: Game) -> u8 {
    match game {
        Game::Halo2 => 24,
        Game::Halo1 => 23,
    }
}

/// Type 0: no handler, only the delay.
pub const DELAY: u8 = 0;
/// Type 1: the end of the stream (both loops stop on `header & 0xfc == 4`).
pub const END: u8 = 1;

/// Payload bytes the handler for `kind` reads, or `None` when the type has
/// no handler in that game's table (types 0 and 1 included, which the
/// loop handles itself).
pub fn payload_len(game: Game, kind: u8) -> Option<usize> {
    Some(match (game, kind) {
        (_, 2 | 3) => 1,
        (Game::Halo2, 4) => 4,
        (Game::Halo1, 4) => 2,
        (_, 5) => 2,
        (_, 6) => 8,
        (_, 7..=14) => 2,
        (_, 15..=22) => 4,
        (Game::Halo2, 23) => 2,
        _ => return None,
    })
}

/// One field of the initial unit control state as it sits in the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    AnimationState,
    AimingSpeed,
    ControlFlags,
    /// Halo 2 only: compared with the unit's before the weapon indices
    /// apply.
    WeaponSet,
    WeaponIndex,
    /// Halo 2 only: the second weapon.
    SecondWeaponIndex,
    /// Read and dropped by both games.
    Pad,
    Throttle,
    Facing,
    Aiming,
    Looking,
    PrimaryTrigger,
    GrenadeIndex,
    ZoomLevel,
}

/// The initial state's fields and their stream sizes, per unit control
/// version: Halo 2's tables at `0x475c80`, Halo 1's at `0x232820` (each
/// entry a size and a state offset, `-1` to skip). Version 0 reads as 1.
pub fn initial_fields(game: Game, control_version: u8) -> Vec<(Field, usize)> {
    use Field::*;
    let mut out = match game {
        Game::Halo2 => vec![
            (AnimationState, 4),
            (AimingSpeed, 2),
            (ControlFlags, 4),
            (WeaponSet, 2),
            (WeaponIndex, 1),
            (SecondWeaponIndex, 1),
            (Pad, 2),
            (Throttle, 8),
            (Facing, 12),
            (Aiming, 12),
            (Looking, 12),
        ],
        Game::Halo1 => vec![
            (AnimationState, 1),
            (AimingSpeed, 1),
            (ControlFlags, 2),
            (WeaponIndex, 2),
            (Pad, 2),
            (Throttle, 8),
            (Facing, 12),
            (Aiming, 12),
            (Looking, 12),
        ],
    };
    if control_version >= 2 {
        out.push((PrimaryTrigger, 4));
    }
    if control_version >= 3 {
        out.push((GrenadeIndex, 2));
    }
    if control_version >= 4 {
        out.push((ZoomLevel, 2));
    }
    out
}

/// Bytes after the initial state that both init routines copy as they are
/// (Halo 2 `0x29f095`, Halo 1 `0x119b36`).
pub const TRAILER: usize = 12;

/// Length of everything before the first event.
pub fn prologue_len(game: Game, control_version: u8) -> usize {
    initial_fields(game, control_version)
        .iter()
        .map(|f| f.1)
        .sum::<usize>()
        + TRAILER
}

/// One event as the player reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    /// Offset of the header byte in the stream.
    pub at: usize,
    pub kind: u8,
    /// Ticks before the event.
    pub delay: u16,
    /// Header and delay bytes.
    pub head_len: usize,
    pub payload_len: usize,
}

impl Event {
    pub fn size(&self) -> usize {
        self.head_len + self.payload_len
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalkError {
    /// The stream ends before the initial state does.
    ShortPrologue { need: usize, have: usize },
    /// The stream ends inside an event, or before an end event.
    Truncated { at: usize },
    /// A type with no handler in the game's table: past the end, the
    /// player calls whatever word follows the table.
    NoHandler { at: usize, kind: u8 },
}

impl fmt::Display for WalkError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WalkError::ShortPrologue { need, have } => {
                write!(f, "initial state needs {need} bytes, stream has {have}")
            }
            WalkError::Truncated { at } => write!(f, "stream ends inside the event at {at:#x}"),
            WalkError::NoHandler { at, kind } => {
                write!(f, "event type {kind} at {at:#x} has no handler")
            }
        }
    }
}

impl std::error::Error for WalkError {}

/// A stream read through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Walk {
    pub events: Vec<Event>,
    /// Sum of every event's delay, the end included.
    pub ticks: u32,
    /// Bytes after the end event.
    pub trailing: usize,
}

/// Read `stream` as `game`'s version-4 player does, through the end event.
pub fn walk(game: Game, control_version: u8, stream: &[u8]) -> Result<Walk, WalkError> {
    let mut at = prologue_len(game, control_version);
    if stream.len() < at {
        return Err(WalkError::ShortPrologue {
            need: at,
            have: stream.len(),
        });
    }
    let mut events = Vec::new();
    let mut ticks = 0u32;
    loop {
        let truncated = WalkError::Truncated { at };
        let header = *stream.get(at).ok_or(truncated.clone())?;
        let (delay, head_len) = match header & 3 {
            0 => (0, 1),
            1 => (1, 1),
            2 => (*stream.get(at + 1).ok_or(truncated.clone())? as u16, 2),
            _ => {
                let b = stream.get(at + 1..at + 3).ok_or(truncated.clone())?;
                (u16::from_le_bytes([b[0], b[1]]), 3)
            }
        };
        let kind = header >> 2;
        let payload_len = match kind {
            DELAY | END => 0,
            _ => payload_len(game, kind).ok_or(WalkError::NoHandler { at, kind })?,
        };
        let e = Event {
            at,
            kind,
            delay,
            head_len,
            payload_len,
        };
        if at + e.size() > stream.len() {
            return Err(truncated);
        }
        ticks += delay as u32;
        events.push(e);
        at += e.size();
        if kind == END {
            return Ok(Walk {
                events,
                ticks,
                trailing: stream.len() - at,
            });
        }
    }
}

/// Halo 2's version-4 handler table (`0x4710f0`), by event type. Zero is no
/// handler. The word after it is `0x00000001`.
pub const HALO2_HANDLER_TABLE: u32 = 0x4710f0;
pub const HALO2_HANDLERS: [u32; 24] = [
    0, 0, 0x29ec30, 0x29ec50, 0x29ec70, 0x29ec90, 0x29ecd0, 0x29edc0, 0x29edc0, 0x29edc0, 0x29edc0,
    0x29edc0, 0x29edc0, 0x29edc0, 0x29edc0, 0x29ef20, 0x29ef20, 0x29ef20, 0x29ef20, 0x29ef20,
    0x29ef20, 0x29ef20, 0x29ef20, 0x29ecb0,
];
/// Halo 2's version-4 decoder pair (init, event loop) and the version
/// table that selects it (`0x46fd5c`, indexed by version - 1).
pub const HALO2_DECODER: (u32, [u32; 2]) = (0x46fd4c, [0x29f080, 0x29f0c0]);
pub const HALO2_VERSIONS: u32 = 0x46fd5c;

/// Halo 1's version-4 handler table (`0x232538`), by event type.
pub const HALO1_HANDLER_TABLE: u32 = 0x232538;
pub const HALO1_HANDLERS: [u32; 23] = [
    0, 0, 0x119b10, 0x119b00, 0x119ae0, 0x119ac0, 0x119aa0, 0x119d30, 0x119d30, 0x119d30, 0x119d30,
    0x119d30, 0x119d30, 0x119d30, 0x119d30, 0x119bb0, 0x119bb0, 0x119bb0, 0x119bb0, 0x119bb0,
    0x119bb0, 0x119bb0, 0x119bb0,
];
pub const HALO1_DECODER: (u32, [u32; 2]) = (0x232294, [0x119b20, 0x119910]);
pub const HALO1_VERSIONS: u32 = 0x2322a4;

/// Halo 2 animation states are string IDs (the unit's mode; Halo 2's own
/// default control, `0xc6f07`, starts at "combat"). Engine string IDs are
/// fixed, so these hold on every map.
pub const MODE_ASLEEP: u32 = 0x0600_0084;
pub const MODE_COMBAT: u32 = 0x0600_0086;
pub const MODE_FLEE: u32 = 0x0400_0089;
pub const MODE_FLAMING: u32 = 0x0700_0039;

/// Halo 1's animation state (a byte; Halo 1 `0x41bb5` turns it into a
/// mode through `0x1f7684`: asleep, alert, stand or crouch, flee, flaming)
/// as Halo 2's mode string ID. Halo 2 has no "stand"; its standing mode is
/// "combat", and it crouches by the control flag, so states 1 to 4 (alert,
/// stand, crouch) become "combat".
pub fn mode(h1_state: u8) -> u32 {
    match h1_state {
        0 => MODE_ASLEEP,
        5 => MODE_FLEE,
        6 => MODE_FLAMING,
        _ => MODE_COMBAT,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Converted {
    pub stream: Vec<u8>,
    /// Halo 1 animation-state events dropped to delays (see [`convert`]).
    pub animation_events: usize,
    /// Control flag values (initial or events) that had weapon-action bits
    /// [`flags`] drops.
    pub weapon_flags: usize,
}

/// Halo 1's movement and stance control flags, bits 0 to 9, which Halo 2
/// keeps where they were.
///
/// Both games group the flags the same way when one unit takes another's
/// controls (a vehicle from its driver and gunner: Halo 1 `0x44d59`,
/// `0x44e34`; Halo 2 `0xc4d22`, `0xc4eba`). Halo 1 takes its movement bits
/// from the driver (`& 0x3f`) and the weapon actions, bits 10 to 14, from
/// the gunner (`& 0x7c00`, the bits it also tests together at `0x44e58`).
/// Halo 2 takes the whole low half from the driver and the high half from
/// the gunner (`& 0xffff`, `& 0xffff0000`, tested together at `0xc4ef0`):
/// its weapon actions moved above bit 15, and the low half is the movement
/// and stance set. Within that half the converter keeps Halo 1's positions
/// (crouch 0, jump 1, through force alert 9, the bit Halo 1 `0x41c09`
/// turns into the alert mode); that part is assumed, not traced bit by bit
/// through Halo 2.
pub const MOVEMENT_FLAGS: u16 = 0x03ff;

/// A Halo 1 control flag word for Halo 2: the movement and stance bits
/// where they were; the weapon actions (reload, triggers, grenade,
/// exchange) dropped, as where Halo 2 keeps each one above bit 15 is not
/// established. A recorded unit moves and looks as recorded and does not
/// fire.
pub fn flags(h1: u16) -> u32 {
    u32::from(h1 & MOVEMENT_FLAGS)
}

/// Rewrite a Halo 1 version-4 stream for Halo 2's player.
///
/// - Initial state: each field into Halo 2's layout for the same unit
///   control version. Animation state becomes a mode string ID ([`mode`]);
///   aiming speed widens sign-extended, as Halo 2's type 3 handler does;
///   control flags go through [`flags`]; the weapon index goes to Halo 2's first
///   weapon byte with the weapon set and second weapon left at `-1`, as in
///   Halo 2's default control (`0xc6ef0`). The rest is the same bytes.
/// - Type 4 (control flags) payloads widen from two bytes to four, through
///   [`flags`].
/// - Type 2 (animation state) events carry a byte, and Halo 2 writes it
///   sign-extended into the mode string ID, which no byte can name. Their
///   value goes into the initial state instead (the last one, the state the
///   recording settles in), and the events become delay-only type 0 with
///   the same delay. On the discs every such event sets stand-or-crouch
///   one tick in.
/// - Everything else is copied as it is, the delays included.
pub fn convert(control_version: u8, h1: &[u8]) -> Result<Converted, WalkError> {
    let walk = walk(Game::Halo1, control_version, h1)?;
    let mut state = h1[0];
    for e in walk.events.iter().filter(|e| e.kind == 2) {
        state = h1[e.at + e.head_len];
    }
    let mut out = Vec::with_capacity(h1.len() + 32);
    let mut at = 0;
    let mut weapon_flags = 0;
    for (field, len) in initial_fields(Game::Halo1, control_version) {
        let b = &h1[at..at + len];
        at += len;
        match field {
            Field::AnimationState => out.extend_from_slice(&mode(state).to_le_bytes()),
            Field::AimingSpeed => out.extend_from_slice(&(b[0] as i8 as i16).to_le_bytes()),
            Field::ControlFlags => {
                let f = u16::from_le_bytes([b[0], b[1]]);
                weapon_flags += usize::from(f & !MOVEMENT_FLAGS != 0);
                out.extend_from_slice(&flags(f).to_le_bytes())
            }
            Field::WeaponIndex => {
                let w = i16::from_le_bytes([b[0], b[1]]);
                let w = if (-1..=127).contains(&w) {
                    w as i8 as u8
                } else {
                    0xff
                };
                out.extend_from_slice(&[0xff, 0xff, w, 0xff]);
            }
            _ => out.extend_from_slice(b),
        }
    }
    debug_assert_eq!(
        out.len(),
        prologue_len(Game::Halo2, control_version) - TRAILER
    );
    out.extend_from_slice(&h1[at..at + TRAILER]);
    let mut animation_events = 0;
    for e in &walk.events {
        let head = &h1[e.at..e.at + e.head_len];
        let payload = &h1[e.at + e.head_len..e.at + e.size()];
        match e.kind {
            2 => {
                animation_events += 1;
                out.push(head[0] & 3);
                out.extend_from_slice(&head[1..]);
            }
            4 => {
                let f = u16::from_le_bytes([payload[0], payload[1]]);
                weapon_flags += usize::from(f & !MOVEMENT_FLAGS != 0);
                out.extend_from_slice(head);
                out.extend_from_slice(&flags(f).to_le_bytes());
            }
            _ => out.extend_from_slice(&h1[e.at..e.at + e.size()]),
        }
    }
    Ok(Converted {
        stream: out,
        animation_events,
        weapon_flags,
    })
}

/// A Halo 2 stream that holds the unit still and ends: Halo 2's default
/// control state (`0xc6ef0`: "combat", indices `-1`), facing, aiming and
/// looking along +x, for `control_version`, then the end event after `ticks`.
pub fn still(control_version: u8, ticks: u16) -> Vec<u8> {
    let mut out = Vec::new();
    for (field, len) in initial_fields(Game::Halo2, control_version) {
        match field {
            Field::AnimationState => out.extend_from_slice(&MODE_COMBAT.to_le_bytes()),
            Field::WeaponSet | Field::WeaponIndex | Field::SecondWeaponIndex => {
                out.extend(std::iter::repeat_n(0xff, len))
            }
            Field::GrenadeIndex | Field::ZoomLevel => out.extend_from_slice(&[0xff, 0xff]),
            Field::Facing | Field::Aiming | Field::Looking => {
                for x in [1.0f32, 0.0, 0.0] {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
            _ => out.extend(std::iter::repeat_n(0, len)),
        }
    }
    out.extend_from_slice(&[0; TRAILER]);
    out.push((END << 2) | 3);
    out.extend_from_slice(&ticks.to_le_bytes());
    out
}

/// What [`for_halo2`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub converted: usize,
    /// Animation-state events turned into delays, over every stream.
    pub animation_events: usize,
    /// Control flag values whose weapon-action bits were dropped.
    pub weapon_flags: usize,
    /// Recordings that could not be converted, with why; each now holds
    /// [`still`] for its length.
    pub stilled: Vec<String>,
}

/// Every recording's stream rewritten for Halo 2's version-4 player. A
/// stream that cannot be (another version, or one Halo 1's own player
/// would not read through) gets [`still`] and version 4, so no element
/// reaches a handler past the table.
pub fn for_halo2(recordings: &mut [crate::cutscenes::Recording]) -> Report {
    let mut report = Report::default();
    for r in recordings {
        let cv = r.control_version();
        let why = match r.version() {
            4 => match convert(cv, &r.stream) {
                Ok(c) => {
                    r.stream = c.stream;
                    report.converted += 1;
                    report.animation_events += c.animation_events;
                    report.weapon_flags += c.weapon_flags;
                    continue;
                }
                Err(e) => e.to_string(),
            },
            v => format!("version {v}"),
        };
        report.stilled.push(format!("{}: {why}", r.name));
        r.head[0x20] = 4;
        r.stream = still(cv, r.ticks().max(1) as u16);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A version-4 prologue: zeroed fields, the given first bytes.
    fn prologue(game: Game, cv: u8, first: &[u8]) -> Vec<u8> {
        let mut v = vec![0u8; prologue_len(game, cv)];
        v[..first.len()].copy_from_slice(first);
        v
    }

    #[test]
    fn prologue_lengths_follow_the_field_tables() {
        // Halo 1: 52 bytes at version 1, then 4, 2 and 2 more; Halo 2: 60,
        // then the same. Plus the twelve copied bytes.
        assert_eq!(prologue_len(Game::Halo1, 0), 52 + 12);
        assert_eq!(prologue_len(Game::Halo1, 1), 52 + 12);
        assert_eq!(prologue_len(Game::Halo1, 4), 60 + 12);
        assert_eq!(prologue_len(Game::Halo2, 3), 66 + 12);
        assert_eq!(prologue_len(Game::Halo2, 4), 68 + 12);
    }

    #[test]
    fn walk_reads_delays_payloads_and_the_end() {
        let mut s = prologue(Game::Halo1, 4, &[]);
        s.extend_from_slice(&[(6 << 2) | 2, 9]); // throttle after 9 ticks
        s.extend_from_slice(&[0; 8]);
        s.extend_from_slice(&[(14 << 2) | 1, 1, 2]); // byte deltas after 1
        s.extend_from_slice(&[(4 << 2) | 3, 0x10, 0x01, 0x01, 0x08]); // flags after 0x110
        s.push((END << 2) | 1);
        let w = walk(Game::Halo1, 4, &s).unwrap();
        let kinds: Vec<u8> = w.events.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, [6, 14, 4, END]);
        assert_eq!(w.ticks, 9 + 1 + 0x110 + 1);
        assert_eq!(w.trailing, 0);
        // Halo 2 reads type 4 as four bytes: it eats the end marker and
        // runs off the stream.
        assert_eq!(
            walk(
                Game::Halo2,
                4,
                &[prologue(Game::Halo2, 4, &[]), s[72..].to_vec()].concat()
            ),
            Err(WalkError::Truncated { at: 80 + 10 + 3 })
        );
    }

    #[test]
    fn types_past_the_table_have_no_handler() {
        for (game, n) in [(Game::Halo1, 23u8), (Game::Halo2, 24)] {
            assert_eq!(handler_count(game), n);
            let mut s = prologue(game, 4, &[]);
            let at = s.len();
            s.push(n << 2);
            assert_eq!(walk(game, 4, &s), Err(WalkError::NoHandler { at, kind: n }));
            assert!(payload_len(game, n - 1).is_some());
        }
        assert_eq!(payload_len(Game::Halo1, 23), None);
    }

    #[test]
    fn handler_tables_agree_with_payload_lengths() {
        // Types that share a handler read the same payload.
        for (game, table) in [
            (Game::Halo1, &HALO1_HANDLERS[..]),
            (Game::Halo2, &HALO2_HANDLERS[..]),
        ] {
            assert_eq!(table.len(), handler_count(game) as usize);
            for (a, &ha) in table.iter().enumerate() {
                assert_eq!(
                    ha == 0,
                    payload_len(game, a as u8).is_none(),
                    "{game:?} {a}"
                );
                for (b, &hb) in table.iter().enumerate() {
                    if ha == hb {
                        assert_eq!(payload_len(game, a as u8), payload_len(game, b as u8));
                    }
                }
            }
        }
    }

    #[test]
    fn convert_widens_the_state_and_flag_events() {
        let mut s = prologue(Game::Halo1, 4, &[0, 0xfe, 0x01, 0x08, 0x01, 0x00]);
        let pad = 6;
        s[pad] = 0xaa; // dropped by both games, kept
        let n = s.len();
        s[n - 12..].copy_from_slice(&[7; 12]);
        s.extend_from_slice(&[(2 << 2) | 1, 3]); // animation state 3 after a tick
        s.extend_from_slice(&[(4 << 2) | 2, 5, 0x00, 0x09]); // flags 0x900 after 5
        s.extend_from_slice(&[(5 << 2), 0xff, 0xff]); // weapon -1
        s.push((END << 2) | 1);
        let c = convert(4, &s).unwrap();
        assert_eq!((c.animation_events, c.weapon_flags), (1, 2));
        let h2 = &c.stream;
        assert_eq!(&h2[0..4], &MODE_COMBAT.to_le_bytes());
        assert_eq!(&h2[4..6], &(-2i16).to_le_bytes());
        // Crouch kept, the primary trigger (bit 11) dropped.
        assert_eq!(&h2[6..10], &1u32.to_le_bytes());
        assert_eq!(&h2[10..14], &[0xff, 0xff, 1, 0xff]);
        assert_eq!(h2[14], 0xaa);
        let p = prologue_len(Game::Halo2, 4);
        assert_eq!(&h2[p - 12..p], &[7; 12]);
        assert_eq!(
            &h2[p..],
            &[
                1,
                (4 << 2) | 2,
                5,
                0x00,
                0x01,
                0,
                0,
                (5 << 2),
                0xff,
                0xff,
                (END << 2) | 1
            ]
        );
        let w = walk(Game::Halo2, 4, h2).unwrap();
        assert_eq!(w.trailing, 0);
        assert_eq!(w.ticks, walk(Game::Halo1, 4, &s).unwrap().ticks);
    }

    #[test]
    fn still_walks_for_its_length() {
        for cv in 0..=4 {
            let w = walk(Game::Halo2, cv, &still(cv, 300)).unwrap();
            assert_eq!((w.events.len(), w.ticks, w.trailing), (1, 300, 0));
        }
    }

    #[test]
    fn modes() {
        assert_eq!(mode(0), MODE_ASLEEP);
        assert_eq!(mode(3), MODE_COMBAT);
        assert_eq!(mode(5), MODE_FLEE);
        assert_eq!(mode(6), MODE_FLAMING);
    }
}
