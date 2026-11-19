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
    DurationOverflow,
    ControlVersion(u8),
    WeaponSlot(i16),
    UnsupportedFlags(u16),
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
            WalkError::DurationOverflow => write!(f, "recording duration exceeds u32"),
            WalkError::ControlVersion(v) => write!(f, "unsupported unit control version {v}"),
            WalkError::WeaponSlot(w) => write!(f, "invalid weapon slot {w}"),
            WalkError::UnsupportedFlags(w) => write!(f, "unsupported control flags {w:#06x}"),
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
        ticks = ticks.checked_add(u32::from(delay)).ok_or(WalkError::DurationOverflow)?;
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
pub use map::recording::{
    ASLEEP as MODE_ASLEEP, COMBAT as MODE_COMBAT, FLAMING as MODE_FLAMING, FLEE as MODE_FLEE,
};

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
    /// Timed mode changes encoded in the stream.
    pub animation_events: usize,
    /// Control flag words carrying weapon actions.
    pub weapon_flags: usize,
}

/// Translate the flags present in the campaign's recordings. Both unit
/// weapon readers produce trigger bits 1 and 2 (H1 0x456dd..0x456ea,
/// H2 0xc5d38..0xc5d53). H2 reload is bit 28 (0xc5a9a) and the
/// grenade request is bit 26 (0x14fe58). H1's seat exit, turn inhibition
/// and alert override are consumed by the recording playback adapter.
fn flags(h1: u16) -> Result<u32, WalkError> {
    use map::recording::{EXIT_SEAT, FORCE_ALERT, HOLD_FACING};
    let mut out = u32::from(h1 & 3); // crouch and jump
    for (source, target) in [
        (4, 1 << 2),
        (5, 1 << 3),
        (6, EXIT_SEAT),
        (7, 1 << 4),
        (8, HOLD_FACING),
        (9, FORCE_ALERT),
        (10, 1 << 28),
        (11, 1 << 16),
        (12, 1 << 17),
        (13, 1 << 26),
    ] {
        if h1 & (1 << source) != 0 {
            out |= target;
        }
    }
    // No recording on the campaign disc uses these bits.
    if h1 & 0xc00c != 0 {
        return Err(WalkError::UnsupportedFlags(h1));
    }
    Ok(out)
}

/// Rebuild a v4 stream. Each type-2 handler consumes one byte and sets the
/// mode at that event's tick. Type 4 carries a full Halo 2 control word.
pub fn convert(control_version: u8, h1: &[u8]) -> Result<Converted, WalkError> {
    if control_version > 4 {
        return Err(WalkError::ControlVersion(control_version));
    }
    let walk = walk(Game::Halo1, control_version, h1)?;
    let mut out = Vec::with_capacity(h1.len() + 32);
    let mut at = 0;
    let mut weapon_flags = 0;
    for (field, len) in initial_fields(Game::Halo1, control_version) {
        let b = &h1[at..at + len];
        at += len;
        match field {
            Field::AnimationState => out.extend_from_slice(&mode(b[0]).to_le_bytes()),
            Field::AimingSpeed => out.extend_from_slice(&(b[0] as i8 as i16).to_le_bytes()),
            Field::ControlFlags => {
                let f = u16::from_le_bytes([b[0], b[1]]);
                weapon_flags += usize::from(f & 0x7c00 != 0);
                out.extend_from_slice(&flags(f)?.to_le_bytes())
            }
            Field::WeaponIndex => {
                let w = i16::from_le_bytes([b[0], b[1]]);
                let slot = weapon_slot(w)?;
                out.extend_from_slice(&map::recording::WEAPON_SET.to_le_bytes());
                out.extend_from_slice(&[slot, map::recording::SECOND_WEAPON]);
            }
            Field::Pad => out.extend_from_slice(&[0, 0]),
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
                out.extend_from_slice(head);
                out.push(mode(payload[0]) as u8);
            }
            4 => {
                let f = u16::from_le_bytes([payload[0], payload[1]]);
                weapon_flags += usize::from(f & 0x7c00 != 0);
                out.extend_from_slice(head);
                out.extend_from_slice(&flags(f)?.to_le_bytes());
            }
            5 => {
                let slot = i16::from_le_bytes([payload[0], payload[1]]);
                weapon_slot(slot)?;
                out.extend_from_slice(&h1[e.at..e.at + e.size()]);
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

fn weapon_slot(slot: i16) -> Result<u8, WalkError> {
    match slot {
        -1..=3 => Ok(slot as u8),
        _ => Err(WalkError::WeaponSlot(slot)),
    }
}

/// What [`for_halo2`] converted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub converted: usize,
    pub animation_events: usize,
    pub weapon_flags: usize,
}

/// Convert the whole block before publishing any replacement stream.
pub fn for_halo2(recordings: &mut [crate::cutscenes::Recording]) -> crate::Result<Report> {
    let converted = recordings
        .iter()
        .map(|r| {
            if r.version != 4 {
                return Err(crate::invalid(format!(
                    "recording {}: stream version {}",
                    r.name, r.version
                )));
            }
            convert(r.control_version, &r.stream)
                .map_err(|e| crate::invalid(format!("recording {}: {e}", r.name)))
        })
        .collect::<crate::Result<Vec<_>>>()?;
    let mut report = Report::default();
    for (r, c) in recordings.iter_mut().zip(converted) {
        r.stream = c.stream;
        report.converted += 1;
        report.animation_events += c.animation_events;
        report.weapon_flags += c.weapon_flags;
    }
    Ok(report)
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
        s[pad] = 0xaa; // dropped by both games
        let n = s.len();
        s[n - 12..].copy_from_slice(&[7; 12]);
        s.extend_from_slice(&[(2 << 2) | 1, 3]); // animation state 3 after a tick
        s.extend_from_slice(&[(4 << 2) | 2, 5, 0x00, 0x09]); // flags 0x900 after 5
        s.extend_from_slice(&[(5 << 2), 0xff, 0xff]); // weapon -1
        s.push((END << 2) | 1);
        let c = convert(4, &s).unwrap();
        assert_eq!((c.animation_events, c.weapon_flags), (1, 2));
        let h2 = &c.stream;
        assert_eq!(&h2[0..4], &MODE_ASLEEP.to_le_bytes());
        assert_eq!(&h2[4..6], &(-2i16).to_le_bytes());
        assert_eq!(&h2[6..10], &0x0001_0001u32.to_le_bytes());
        assert_eq!(&h2[10..14], &[0xfe, 0xff, 1, 0xfe]);
        assert_eq!(&h2[14..16], &[0, 0]);
        let p = prologue_len(Game::Halo2, 4);
        assert_eq!(&h2[p - 12..p], &[7; 12]);
        assert_eq!(
            &h2[p..],
            &[
                (2 << 2) | 1, MODE_COMBAT as u8,
                (4 << 2) | 2, 5, 0x00, 0x00, 0x21, 0x00,
                5 << 2, 0xff, 0xff,
                (END << 2) | 1,
            ]
        );
        let w = walk(Game::Halo2, 4, h2).unwrap();
        assert_eq!(w.trailing, 0);
        assert_eq!(w.ticks, walk(Game::Halo1, 4, &s).unwrap().ticks);
    }

    #[test]
    fn recording_controls_keep_their_actions() {
        for (source, expected) in [
            (1, 1), (2, 2), (0x40, map::recording::EXIT_SEAT),
            (0x100, map::recording::HOLD_FACING), (0x200, map::recording::FORCE_ALERT),
            (0x400, 1 << 28), (0x800, 1 << 16), (0x1000, 1 << 17), (0x2000, 1 << 26),
        ] {
            assert_eq!(flags(source), Ok(expected));
        }
        assert_eq!(flags(0x8000), Err(WalkError::UnsupportedFlags(0x8000)));
    }

    #[test]
    fn invalid_recording_does_not_publish_a_partial_block() {
        let mut stream = prologue(Game::Halo1, 0, &[]);
        stream.push(END << 2);
        let first = crate::cutscenes::Recording {
            name: "arrive".into(), version: 4,
            control_version: 0, ticks: 1, stream,
        };
        let mut second = first.clone();
        second.name = "depart".into();
        second.stream.pop();
        let mut block = vec![first, second];
        let before = block.clone();
        assert!(for_halo2(&mut block).unwrap_err().to_string().contains("depart"));
        assert_eq!(block, before);
        assert_eq!(convert(5, &[]), Err(WalkError::ControlVersion(5)));
    }

    #[test]
    fn modes() {
        assert_eq!(mode(0), MODE_ASLEEP);
        assert_eq!(mode(3), MODE_COMBAT);
        assert_eq!(mode(5), MODE_FLEE);
        assert_eq!(mode(6), MODE_FLAMING);
    }
}
