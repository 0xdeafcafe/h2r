//! Recorded animations: the scenario's block of named unit control streams
//! (`recorded_animation`, scenario `+0x110`), the walker that frames a
//! stream into events, and a typed read of what each event and each field
//! of the initial state holds (docs/editor/rich-editors.md, "Recordings").
//!
//! The stream layout, its evidence and the handler tables are written up
//! with the converter that rewrites Halo 1's streams
//! (`h1campaign::recordings`): the initial unit control state in the
//! layout of the control version, twelve bytes copied as they are, then
//! events, each a header byte (low two bits the delay form, upper six the
//! type) with its delay and payload. [`crate::recording_edit`] encodes a
//! stream and edits the block.

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


/// The scenario field holding the recordings block (count, address).
const BLOCK: usize = 0x110;
/// The bytes of a `recorded_animation` element
/// (`tags/src/defs/recorded_animation.txt`).
pub const ELEMENT: usize = 0x34;
/// Most recordings a block may claim. A sanity bound, not an engine limit.
pub const MAX_RECORDINGS: usize = 1024;
/// Most bytes one stream may claim. A sanity bound, not an engine limit.
pub const MAX_STREAM: usize = 4 << 20;

/// One element of the block, with its stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    pub name: String,
    pub version: u8,
    pub control_version: u8,
    pub ticks: i16,
    pub stream: Vec<u8>,
}

fn invalid(why: impl Into<String>) -> crate::Error {
    crate::Error::Invalid(why.into())
}

/// The recordings block as the source holds it: where it is and each
/// element's raw bytes, with the rows read from them. What
/// [`crate::recording_edit::changes`] diffs an edited block against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// Guest address of the scenario's `(count, address)` pair.
    pub root: u32,
    /// Guest address of the first element (0 when the block is empty).
    pub address: u32,
    /// Each element's `ELEMENT` bytes.
    pub elements: Vec<Vec<u8>>,
    pub rows: Vec<Recorded>,
}

/// Read the recordings block of the open map's scenario.
pub fn read<R: std::io::Read + std::io::Seek>(
    map: &mut map::Map<R>,
) -> crate::Result<Vec<Recorded>> {
    Ok(read_block(map)?.rows)
}

/// [`read`], with where the block is and its elements' raw bytes.
pub fn read_block<R: std::io::Read + std::io::Seek>(
    map: &mut map::Map<R>,
) -> crate::Result<Block> {
    let loc = map
        .scenario()
        .ok_or_else(|| invalid("map has no scenario"))?
        .location
        .ok_or(map::Error::NoTagData)?;
    if (loc.size as usize) < BLOCK + 8 {
        return Err(invalid("scenario header is truncated"));
    }
    let header = map.read_file_range(loc.file_offset()?, (BLOCK + 8) as u64)?;
    parse(&header, loc.address + BLOCK as u32, |address, len| {
        let at = map.file_offset(address, len)?;
        Ok(map.read_file_range(at, len.into())?)
    })
}

/// Read the recordings block of a complete scnr payload whose guest base
/// is `base`.
pub fn decode(bytes: &[u8], base: u32) -> crate::Result<Vec<Recorded>> {
    Ok(decode_block(bytes, base)?.rows)
}

/// [`decode`], with where the block is and its elements' raw bytes.
pub fn decode_block(bytes: &[u8], base: u32) -> crate::Result<Block> {
    if bytes.len() < BLOCK + 8 {
        return Err(invalid("scenario header is truncated"));
    }
    parse(bytes, base + BLOCK as u32, |address, len| {
        let at = address
            .checked_sub(base)
            .ok_or_else(|| invalid("recordings precede the scenario"))? as usize;
        bytes
            .get(at..at + len as usize)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| invalid(format!("recordings at {address:#x} exceed the scenario payload")))
    })
}

fn parse(
    header: &[u8],
    root: u32,
    mut fetch: impl FnMut(u32, u32) -> crate::Result<Vec<u8>>,
) -> crate::Result<Block> {
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let count = word(header, BLOCK) as usize;
    if count == 0 {
        return Ok(Block {
            root,
            address: 0,
            elements: Vec::new(),
            rows: Vec::new(),
        });
    }
    if count > MAX_RECORDINGS {
        return Err(invalid(format!(
            "recordings: count {count} exceeds {MAX_RECORDINGS}"
        )));
    }
    let address = word(header, BLOCK + 4);
    if address & 3 != 0 {
        return Err(invalid("recordings block is unaligned"));
    }
    let rows = fetch(address, (count * ELEMENT) as u32)?;
    let mut out = Vec::with_capacity(count);
    let elements: Vec<Vec<u8>> = rows.chunks_exact(ELEMENT).map(<[u8]>::to_vec).collect();
    for (i, row) in rows.chunks_exact(ELEMENT).enumerate() {
        let end = row[..32].iter().position(|&c| c == 0).unwrap_or(32);
        let name = std::str::from_utf8(&row[..end])
            .map_err(|_| invalid(format!("recording {i}: the name is not text")))?
            .to_owned();
        let size = word(row, 0x2c) as usize;
        if size > MAX_STREAM {
            return Err(invalid(format!(
                "recording {name}: stream of {size} bytes exceeds {MAX_STREAM}"
            )));
        }
        let stream = if size == 0 {
            Vec::new()
        } else {
            fetch(word(row, 0x30), size as u32)?
        };
        out.push(Recorded {
            name,
            version: row[0x20],
            control_version: row[0x22],
            ticks: i16::from_le_bytes([row[0x24], row[0x25]]),
            stream,
        });
    }
    Ok(Block {
        root,
        address,
        elements,
        rows: out,
    })
}

/// What a field or a payload holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Unsigned(u32),
    Signed(i32),
    /// Little-endian floats, by the field's width.
    Reals(Vec<f32>),
}

/// A named value: the name is the engine's field where the converter's
/// notes name it, else a description of the width.
pub type Named = (&'static str, Value);

fn unsigned(b: &[u8]) -> Value {
    Value::Unsigned(b.iter().rev().fold(0u32, |a, &x| a << 8 | u32::from(x)))
}

fn signed(b: &[u8]) -> Value {
    let u = b.iter().rev().fold(0u32, |a, &x| a << 8 | u32::from(x));
    let bits = 8 * b.len() as u32;
    Value::Signed(((u << (32 - bits)) as i32) >> (32 - bits))
}

fn reals(b: &[u8]) -> Value {
    Value::Reals(
        b.chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect(),
    )
}

/// The value of one initial-state field, `None` for padding.
pub(crate) fn initial_value(field: Field, b: &[u8]) -> Option<Named> {
    Some(match field {
        Field::AnimationState => ("animation_state", unsigned(b)),
        Field::AimingSpeed => ("aiming_speed", signed(b)),
        Field::ControlFlags => ("control_flags", unsigned(b)),
        Field::WeaponSet => ("weapon_set", unsigned(b)),
        Field::WeaponIndex => ("weapon_index", signed(b)),
        Field::SecondWeaponIndex => ("second_weapon_index", signed(b)),
        Field::Pad => return None,
        Field::Throttle => ("throttle", reals(b)),
        Field::Facing => ("facing", reals(b)),
        Field::Aiming => ("aiming", reals(b)),
        Field::Looking => ("looking", reals(b)),
        Field::PrimaryTrigger => ("primary_trigger", unsigned(b)),
        Field::GrenadeIndex => ("grenade_index", signed(b)),
        Field::ZoomLevel => ("zoom_level", signed(b)),
    })
}

/// The event type's name. Types 7 to 14 and 15 to 22 are the byte and
/// short deltas: which control each type moves is not read.
pub fn event_name(kind: u8) -> &'static str {
    match kind {
        DELAY => "delay",
        END => "end",
        2 => "animation_state",
        3 => "aiming_speed",
        4 => "control_flags",
        5 => "weapon_index",
        6 => "throttle",
        7..=14 => "delta_byte",
        15..=22 => "delta_short",
        23 => "second_weapon_index",
        _ => "unknown",
    }
}

/// An event's payload as named values.
fn payload_values(kind: u8, p: &[u8]) -> Vec<Named> {
    match kind {
        2 => vec![("value", unsigned(p))],
        3 => vec![("value", signed(p))],
        4 => vec![("value", unsigned(p))],
        5 | 23 => vec![("value", signed(p))],
        6 => vec![("value", reals(p))],
        7..=14 => vec![
            ("channel", Value::Unsigned(u32::from(kind - 7))),
            ("a", signed(&p[..1])),
            ("b", signed(&p[1..2])),
        ],
        15..=22 => vec![
            ("channel", Value::Unsigned(u32::from(kind - 15))),
            ("a", signed(&p[..2])),
            ("b", signed(&p[2..4])),
        ],
        _ => Vec::new(),
    }
}

/// The unit control state a stream starts from.
#[derive(Clone, Debug, PartialEq)]
pub struct Initial {
    pub fields: Vec<Named>,
    /// The twelve bytes both games copy as they are.
    pub trailer: [u8; TRAILER],
}

/// One event, framed and typed.
#[derive(Clone, Debug, PartialEq)]
pub struct Typed {
    /// Offset of the header byte in the stream.
    pub at: usize,
    /// Ticks since the start, with this event's delay.
    pub tick: u32,
    pub delay: u16,
    pub kind: u8,
    pub name: &'static str,
    pub fields: Vec<Named>,
    /// The payload bytes as they are in the stream.
    pub payload: Vec<u8>,
}

/// A stream read through to its end event.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub initial: Initial,
    pub events: Vec<Typed>,
    pub ticks: u32,
    pub trailing: usize,
}

/// Read `stream` as `game`'s player does and type every field: what
/// [`walk`] frames, with the initial state and each payload decoded. An
/// error is [`walk`]'s, with the offset of the event that broke it.
pub fn decode_stream(
    game: Game,
    control_version: u8,
    stream: &[u8],
) -> Result<Decoded, WalkError> {
    let w = walk(game, control_version, stream)?;
    let mut at = 0;
    let mut fields = Vec::new();
    for (field, len) in initial_fields(game, control_version) {
        fields.extend(initial_value(field, &stream[at..at + len]));
        at += len;
    }
    let trailer: [u8; TRAILER] = stream[at..at + TRAILER].try_into().unwrap();
    let mut tick = 0u32;
    let events = w
        .events
        .iter()
        .map(|e| {
            tick += u32::from(e.delay);
            let payload = stream[e.at + e.head_len..e.at + e.size()].to_vec();
            Typed {
                at: e.at,
                tick,
                delay: e.delay,
                kind: e.kind,
                name: event_name(e.kind),
                fields: payload_values(e.kind, &payload),
                payload,
            }
        })
        .collect();
    Ok(Decoded {
        initial: Initial { fields, trailer },
        events,
        ticks: w.ticks,
        trailing: w.trailing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prologue(game: Game, cv: u8) -> Vec<u8> {
        vec![0; prologue_len(game, cv)]
    }

    #[test]
    fn a_stream_types_its_initial_state_and_events() {
        let mut s = prologue(Game::Halo2, 1);
        s[0..4].copy_from_slice(&0x0600_0086u32.to_le_bytes());
        s[4..6].copy_from_slice(&(-3i16).to_le_bytes());
        s[6..10].copy_from_slice(&5u32.to_le_bytes());
        let throttle = 4 + 2 + 4 + 2 + 1 + 1 + 2;
        s[throttle..throttle + 4].copy_from_slice(&1.5f32.to_le_bytes());
        let n = s.len();
        s[n - 12..].copy_from_slice(&[9; 12]);
        s.extend_from_slice(&[(6 << 2) | 2, 4]);
        s.extend_from_slice(&0.25f32.to_le_bytes());
        s.extend_from_slice(&(-1.0f32).to_le_bytes());
        s.extend_from_slice(&[(9 << 2) | 1, 0xff, 0x02]);
        s.extend_from_slice(&[(16 << 2) | 3, 0x00, 0x01, 0x01, 0x00, 0x01, 0x00]);
        s.push((END << 2) | 1);
        let d = decode_stream(Game::Halo2, 1, &s).unwrap();
        let get = |n: &str| d.initial.fields.iter().find(|f| f.0 == n).map(|f| f.1.clone());
        assert_eq!(get("animation_state"), Some(Value::Unsigned(0x0600_0086)));
        assert_eq!(get("aiming_speed"), Some(Value::Signed(-3)));
        assert_eq!(get("control_flags"), Some(Value::Unsigned(5)));
        assert_eq!(get("throttle"), Some(Value::Reals(vec![1.5, 0.0])));
        assert_eq!(get("primary_trigger"), None, "version 1 stops before it");
        assert_eq!(d.initial.trailer, [9; 12]);
        let kinds: Vec<_> = d.events.iter().map(|e| (e.tick, e.name)).collect();
        assert_eq!(
            kinds,
            [(4, "throttle"), (5, "delta_byte"), (261, "delta_short"), (262, "end")]
        );
        assert_eq!(d.events[0].fields, [("value", Value::Reals(vec![0.25, -1.0]))]);
        assert_eq!(
            d.events[1].fields,
            [
                ("channel", Value::Unsigned(2)),
                ("a", Value::Signed(-1)),
                ("b", Value::Signed(2))
            ]
        );
        assert_eq!(
            d.events[2].fields,
            [
                ("channel", Value::Unsigned(1)),
                ("a", Value::Signed(1)),
                ("b", Value::Signed(1))
            ]
        );
        assert_eq!((d.ticks, d.trailing), (262, 0));
        assert_eq!(d.events[2].payload, [0x01, 0x00, 0x01, 0x00]);
    }

    #[test]
    fn signed_widths_extend_their_sign() {
        assert_eq!(signed(&[0xff]), Value::Signed(-1));
        assert_eq!(signed(&[0xfe, 0xff]), Value::Signed(-2));
        assert_eq!(signed(&[0x7f]), Value::Signed(127));
        assert_eq!(unsigned(&[1, 2, 3, 4]), Value::Unsigned(0x0403_0201));
    }

    #[test]
    fn halo_one_streams_decode_with_halo_one_widths() {
        let mut s = prologue(Game::Halo1, 0);
        s.extend_from_slice(&[(4 << 2), 0x01, 0x80]);
        s.push(END << 2);
        let d = decode_stream(Game::Halo1, 0, &s).unwrap();
        assert_eq!(d.events[0].fields, [("value", Value::Unsigned(0x8001))]);
    }

    #[test]
    fn a_broken_stream_is_an_error_with_its_offset() {
        let mut s = prologue(Game::Halo2, 1);
        let at = s.len();
        s.push(30 << 2);
        assert_eq!(
            decode_stream(Game::Halo2, 1, &s),
            Err(WalkError::NoHandler { at, kind: 30 })
        );
        let mut s = prologue(Game::Halo2, 1);
        s.push(6 << 2);
        s.push(0);
        assert_eq!(
            decode_stream(Game::Halo2, 1, &s),
            Err(WalkError::Truncated { at })
        );
        assert!(matches!(
            decode_stream(Game::Halo2, 1, &[0; 3]),
            Err(WalkError::ShortPrologue { .. })
        ));
    }

    fn block(rows: &[Recorded], base: u32, at: usize) -> Vec<u8> {
        let mut p = vec![0u8; 0x400];
        crate::fixture::write_recordings(&mut p, base, at, rows);
        p
    }

    #[test]
    fn the_block_reads_back_from_a_payload() {
        let mut stream = prologue(Game::Halo2, 1);
        stream.push((END << 2) | 1);
        let rows = vec![
            Recorded {
                name: "arrive".into(),
                version: 4,
                control_version: 1,
                ticks: 1,
                stream,
            },
            Recorded {
                name: "empty".into(),
                version: 4,
                control_version: 0,
                ticks: 0,
                stream: Vec::new(),
            },
        ];
        let p = block(&rows, 0x9000_0000, 0x200);
        assert_eq!(decode(&p, 0x9000_0000).unwrap(), rows);
        let none = block(&[], 0x9000_0000, 0x200);
        assert_eq!(decode(&none, 0x9000_0000).unwrap(), []);
        // A block past the payload and a header cut short are refused.
        assert!(decode(&p[..0x210], 0x9000_0000).is_err());
        assert!(decode(&p[..0x100], 0x9000_0000).is_err());
        let mut bad = p.clone();
        bad[0x110..0x114].copy_from_slice(&(MAX_RECORDINGS as u32 + 1).to_le_bytes());
        assert!(decode(&bad, 0x9000_0000).unwrap_err().to_string().contains("exceeds"));
    }
}
