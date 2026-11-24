//! The show channel's cues: one-shot events with a tick that the engine
//! does not replicate (docs/game/rules-world-api.md, "The show channel").
//! A cue is made by the `Authority` (and identically by every `Replica`),
//! rides the actor channel's `Cue` record in three consecutive packets and
//! is shown by a `Viewer` to its own players in the audience. A cue is not
//! state: a machine that joins later does not get it.
//!
//! The codec is self-contained: encode needs no table, and decode checks
//! every index against the blob's string table, so a record names only
//! what the verified blob holds. No field is an offset, a length the
//! receiver allocates by, or text.

mod state;

pub use state::{
    MAX_BYTES as MAX_STATE_BYTES, MAX_WAYPOINTS, MAX_WIDGETS, State, StateError, View, Viewer, WaypointView,
    Waypoint, Widget, WidgetView, fill_byte, view,
};

use crate::caps;
use crate::ty::{MAX_PLAYERS, MAX_TEAMS};
use crate::types::{StartLine, Type};

/// The most bytes one cue's payload encodes to: the `Cue` record's bound
/// (the actor channel's `MAX_CUE_BYTES`).
pub const MAX_CUE_BYTES: usize = 24;
/// Pending channel cues kept on a machine with no consumer (a solo game or
/// a synchronous peer): the oldest are dropped past this.
pub const MAX_PENDING: usize = 64;

/// Who a cue is for: a mask of the 16 player slots.
pub type Audience = u16;
/// Every slot, as `message(all, …)` resolves.
pub const EVERYONE: Audience = u16::MAX;

/// The slot's bit of an audience; none for a slot outside the roster.
pub fn player(slot: u8) -> Audience {
    if (slot as usize) < MAX_PLAYERS {
        1 << slot
    } else {
        0
    }
}

/// Who a `message(to, text)` is for, before a roster resolves it to slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum To {
    Player(u8),
    Team(u8),
    Everyone,
}

/// The value of one message hole, typed as the template's hole is
/// (`caps::Hole`): a player or team by slot (`None` for no player or
/// team), an int, a time in ticks, or an enum variant's name as an index
/// into the blob's string table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoleValue {
    Player(Option<u8>),
    Team(Option<u8>),
    Int(i32),
    Time(i32),
    /// The variant's name, an index into the blob's string table.
    Enum(u16),
}

/// A message cue: a text index into the blob's string table, up to four
/// typed hole values and an audience.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub audience: Audience,
    /// The template, an index into the blob's string table.
    pub text: u16,
    /// The holes' values, in hole order, at most [`caps::MAX_HOLES`].
    pub values: Vec<HoleValue>,
}

/// A sound cue: a sound of the blob's tag table, played at each unit of the
/// audience's players (`sound_to`, `sound_to_team`, `sound_to_all`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundCue {
    pub audience: Audience,
    /// The sound, an index into the blob's tag table; its entry must be of
    /// the sound group ([`SoundCue::check`]).
    pub sound: u16,
}

/// The bytes a sound cue encodes to.
pub const SOUND_BYTES: usize = 4;

impl SoundCue {
    /// The payload: the sound, then the audience.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SOUND_BYTES);
        out.extend(self.sound.to_le_bytes());
        out.extend(self.audience.to_le_bytes());
        out
    }

    /// Reads a payload: exactly the sound and the audience. The sound is
    /// checked against the blob's tag table by [`SoundCue::check`], which
    /// the receiver makes with the table in hand.
    pub fn decode(bytes: &[u8]) -> Result<SoundCue, CueError> {
        let mut r = Reader(bytes);
        let sound = r.u16()?;
        let audience = r.u16()?;
        if !r.0.is_empty() {
            return Err(CueError::TrailingBytes);
        }
        Ok(SoundCue { audience, sound })
    }

    /// Whether the sound is an entry of `tags` of the sound group: a cue
    /// names only a sound the verified blob holds.
    pub fn check(&self, tags: &[crate::ir::TagDef]) -> Result<(), CueError> {
        match tags.get(usize::from(self.sound)) {
            Some(t) if t.group == crate::ir::TagGroup::Sound => Ok(()),
            _ => Err(CueError::BadSound(self.sound)),
        }
    }
}

/// A one-shot show event. The host names custom round-start audio by script type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cue {
    Message(Message),
    StartClip(Type),
    /// The areas the script shows in play (`show_area`;
    /// `vm::RulesExt::shown`), whole: each zone object and the team of
    /// its wall's colour (`None` for neutral). The authority sends it when
    /// the list changes and every [`WALLS_REPEAT`] ticks while it is not
    /// empty, so a machine that joins late has it within a few seconds; an
    /// empty list takes every wall off.
    Walls(Vec<(u32, Option<u8>)>),
    Sound(SoundCue),
}

/// The kind byte of a message cue in the `Cue` record's payload.
pub const MESSAGE: u8 = 0;
pub const START_CLIP: u8 = 1;
pub const WALLS: u8 = 2;

/// A zone object is a netgame flag's labelled thing: its handle is this
/// base (the handle bit and the flags' block) with the flag's index
/// (`game::rules::census::Thing::handle`). The index is what a walls cue
/// carries.
pub const ZONE_BASE: u32 = 0x4008_0000;
/// The most walls a cue holds: the VM's shown areas.
pub const MAX_WALLS: usize = crate::vm::MAX_SHOWN_AREAS;
/// How often the authority repeats a non-empty walls cue, in ticks.
pub const WALLS_REPEAT: u32 = 90;
/// A wall's team on the wire when it is neutral.
const NEUTRAL: u16 = 0xf;
pub const SOUND: u8 = 3;

impl Cue {
    /// The kind byte of the `Cue` record's payload.
    pub fn kind(&self) -> u8 {
        match self {
            Cue::Message(_) => MESSAGE,
            Cue::StartClip(_) => START_CLIP,
            Cue::Walls(_) => WALLS,
            Cue::Sound(_) => SOUND,
        }
    }

    /// The cue's payload, at most [`MAX_CUE_BYTES`] bytes.
    pub fn encode(&self) -> Result<Vec<u8>, CueError> {
        match self {
            Cue::Message(message) => message.encode(),
            Cue::Sound(sound) => Ok(sound.encode()),
            Cue::StartClip(ty) if ty.row().start == StartLine::Ours => Ok(vec![ty.code()]),
            Cue::StartClip(ty) => Err(CueError::NoStartClip(ty.code())),
            Cue::Walls(walls) => encode_walls(walls),
        }
    }

    /// Reads a cue's payload, checked against the blob's string table: the
    /// text is in the table, the hole count and each value's type match
    /// the template's holes, and each enum value is in the table.
    pub fn decode(kind: u8, bytes: &[u8], strings: &[String]) -> Result<Cue, CueError> {
        if bytes.len() > MAX_CUE_BYTES {
            return Err(CueError::TooLong(bytes.len()));
        }
        match kind {
            MESSAGE => Message::decode(bytes, strings).map(Cue::Message),
            SOUND => SoundCue::decode(bytes).map(Cue::Sound),
            START_CLIP => match bytes {
                [code] => Type::from_code(*code)
                    .filter(|ty| ty.row().start == StartLine::Ours)
                    .map(Cue::StartClip)
                    .ok_or(CueError::NoStartClip(*code)),
                [] => Err(CueError::Truncated),
                _ => Err(CueError::TrailingBytes),
            },
            WALLS => decode_walls(bytes).map(Cue::Walls),
            kind => Err(CueError::UnknownKind(kind)),
        }
    }
}

/// A walls cue's payload: the count, then a `u16` for each wall, the flag's
/// index in the low 12 bits and the team (`0xf` for neutral) above.
fn encode_walls(walls: &[(u32, Option<u8>)]) -> Result<Vec<u8>, CueError> {
    if walls.len() > MAX_WALLS {
        return Err(CueError::TooManyWalls(walls.len()));
    }
    let mut out = vec![walls.len() as u8];
    for (n, &(zone, team)) in walls.iter().enumerate() {
        let index = zone.wrapping_sub(ZONE_BASE);
        if zone & !0xffff != ZONE_BASE || index >= 0x1000 {
            return Err(CueError::BadZone(zone));
        }
        if walls[..n].iter().any(|(z, _)| *z == zone) {
            return Err(CueError::DuplicateWall(zone));
        }
        let team = match team {
            None => NEUTRAL,
            Some(t) if (t as usize) < MAX_TEAMS => u16::from(t),
            Some(t) => return Err(CueError::BadTeam(t)),
        };
        out.extend((index as u16 | team << 12).to_le_bytes());
    }
    Ok(out)
}

fn decode_walls(bytes: &[u8]) -> Result<Vec<(u32, Option<u8>)>, CueError> {
    let mut r = Reader(bytes);
    let count = usize::from(r.u8()?);
    if count > MAX_WALLS {
        return Err(CueError::TooManyWalls(count));
    }
    let mut walls: Vec<(u32, Option<u8>)> = Vec::with_capacity(count);
    for _ in 0..count {
        let word = r.u16()?;
        let zone = ZONE_BASE | u32::from(word & 0x0fff);
        let team = match word >> 12 {
            NEUTRAL => None,
            t if (t as usize) < MAX_TEAMS => Some(t as u8),
            t => return Err(CueError::BadTeam(t as u8)),
        };
        if walls.iter().any(|(z, _)| *z == zone) {
            return Err(CueError::DuplicateWall(zone));
        }
        walls.push((zone, team));
    }
    if !r.0.is_empty() {
        return Err(CueError::TrailingBytes);
    }
    Ok(walls)
}

/// Why a cue does not encode or decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CueError {
    /// The kind byte is no cue's.
    UnknownKind(u8),
    /// The type byte names no custom round-start clip.
    NoStartClip(u8),
    /// The payload is over [`MAX_CUE_BYTES`].
    TooLong(usize),
    /// The payload ends inside a field.
    Truncated,
    /// Bytes past the last hole value.
    TrailingBytes,
    /// More than [`caps::MAX_HOLES`] values at encode.
    TooManyHoles(usize),
    /// The text index is outside the blob's string table.
    BadText(u16),
    /// The text is no valid template (an unpaired brace, an unknown hole
    /// word, or more than [`caps::MAX_HOLES`] holes).
    BadTemplate,
    /// The payload's hole count is not the template's.
    Holes { got: usize, want: usize },
    /// A player slot outside the roster (`0xff` is no player).
    BadPlayer(u8),
    /// A team outside the eight (`0xff` is no team).
    BadTeam(u8),
    /// An enum value outside the blob's string table.
    BadEnum(u16),
    /// A wall's zone is no netgame flag's object, or its index is past
    /// the 12 bits the wire has.
    BadZone(u32),
    /// More than [`MAX_WALLS`] walls.
    TooManyWalls(usize),
    /// A zone twice in one walls cue.
    DuplicateWall(u32),
    /// A sound that is no sound-group entry of the blob's tag table.
    BadSound(u16),
}

impl std::fmt::Display for CueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "show cue error: {self:?}")
    }
}

impl std::error::Error for CueError {}

/// No player or team on the wire.
const NONE: u8 = 0xff;

impl Message {
    /// The payload: text, audience, hole count and each value by its type.
    pub fn encode(&self) -> Result<Vec<u8>, CueError> {
        if self.values.len() > caps::MAX_HOLES {
            return Err(CueError::TooManyHoles(self.values.len()));
        }
        let mut out = Vec::with_capacity(MAX_CUE_BYTES);
        out.extend(self.text.to_le_bytes());
        out.extend(self.audience.to_le_bytes());
        out.push(self.values.len() as u8);
        for value in &self.values {
            match *value {
                HoleValue::Player(slot) => out.push(match slot {
                    None => NONE,
                    Some(slot) if (slot as usize) < MAX_PLAYERS => slot,
                    Some(slot) => return Err(CueError::BadPlayer(slot)),
                }),
                HoleValue::Team(team) => out.push(match team {
                    None => NONE,
                    Some(team) if (team as usize) < MAX_TEAMS => team,
                    Some(team) => return Err(CueError::BadTeam(team)),
                }),
                HoleValue::Int(n) | HoleValue::Time(n) => out.extend(n.to_le_bytes()),
                HoleValue::Enum(index) => out.extend(index.to_le_bytes()),
            }
        }
        if out.len() > MAX_CUE_BYTES {
            return Err(CueError::TooLong(out.len()));
        }
        Ok(out)
    }

    /// Reads a payload against the blob's string table. The template's
    /// holes say how many values follow and of what type; anything that
    /// does not line up is refused.
    pub fn decode(bytes: &[u8], strings: &[String]) -> Result<Message, CueError> {
        let mut r = Reader(bytes);
        let text = r.u16()?;
        let audience = r.u16()?;
        let count = usize::from(r.u8()?);
        let template = strings
            .get(usize::from(text))
            .ok_or(CueError::BadText(text))?;
        let holes = caps::holes(template).ok_or(CueError::BadTemplate)?;
        if count != holes.len() {
            return Err(CueError::Holes {
                got: count,
                want: holes.len(),
            });
        }
        let mut values = Vec::with_capacity(count);
        for hole in holes {
            values.push(match hole {
                caps::Hole::Player => {
                    HoleValue::Player(r.slot::<MAX_PLAYERS>(CueError::BadPlayer)?)
                }
                caps::Hole::Team => HoleValue::Team(r.slot::<MAX_TEAMS>(CueError::BadTeam)?),
                caps::Hole::Int => HoleValue::Int(r.i32()?),
                caps::Hole::Time => HoleValue::Time(r.i32()?),
                caps::Hole::Enum => {
                    let index = r.u16()?;
                    if strings.get(usize::from(index)).is_none() {
                        return Err(CueError::BadEnum(index));
                    }
                    HoleValue::Enum(index)
                }
            });
        }
        if !r.0.is_empty() {
            return Err(CueError::TrailingBytes);
        }
        Ok(Message {
            audience,
            text,
            values,
        })
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn u8(&mut self) -> Result<u8, CueError> {
        let (byte, rest) = self.0.split_first().ok_or(CueError::Truncated)?;
        self.0 = rest;
        Ok(*byte)
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], CueError> {
        let (bytes, rest) = self.0.split_first_chunk().ok_or(CueError::Truncated)?;
        self.0 = rest;
        Ok(*bytes)
    }

    fn u16(&mut self) -> Result<u16, CueError> {
        Ok(u16::from_le_bytes(self.take()?))
    }

    fn i32(&mut self) -> Result<i32, CueError> {
        Ok(i32::from_le_bytes(self.take()?))
    }

    /// A slot or team index, `0xff` for none.
    fn slot<const MAX: usize>(
        &mut self,
        bad: impl Fn(u8) -> CueError,
    ) -> Result<Option<u8>, CueError> {
        match self.u8()? {
            NONE => Ok(None),
            slot if (slot as usize) < MAX => Ok(Some(slot)),
            slot => Err(bad(slot)),
        }
    }
}

/// A message's line: its template filled with its values' words. A player
/// hole shows `player(slot)`, a team's `team N`, a time as `m:ss`, an enum
/// its variant's name from `strings`.
pub fn line(
    strings: &[String],
    template: &str,
    values: &[HoleValue],
    player: &dyn Fn(u8) -> String,
) -> String {
    let holes = caps::holes(template).unwrap_or_default();
    let shown: Vec<String> = values
        .iter()
        .zip(
            holes
                .iter()
                .copied()
                .map(Some)
                .chain(std::iter::repeat(None)),
        )
        .map(|(v, h)| word(strings, v, h, player))
        .collect();
    caps::fill(template, &shown)
}

/// A hole value's word in a shown line.
fn word(
    strings: &[String],
    value: &HoleValue,
    hole: Option<caps::Hole>,
    player: &dyn Fn(u8) -> String,
) -> String {
    match (value, hole) {
        (HoleValue::Int(n), Some(caps::Hole::Time)) | (HoleValue::Time(n), _) => {
            caps::show_time(*n)
        }
        (HoleValue::Int(n), _) => n.to_string(),
        (HoleValue::Player(Some(slot)), _) => player(*slot),
        (HoleValue::Player(None), _) => "none".into(),
        (HoleValue::Team(Some(team)), _) => format!("team {team}"),
        (HoleValue::Team(None), _) => "none".into(),
        (HoleValue::Enum(index), _) => strings
            .get(usize::from(*index))
            .cloned()
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A string table the tests' messages name, no disc data.
    fn strings() -> Vec<String> {
        vec![
            "Wave {int} in {time}".into(),
            "{player} scored for {team}".into(),
            "The core is live".into(),
            "{enum}".into(),
            "attack".into(),
        ]
    }

    fn full() -> Message {
        Message {
            audience: player(2) | player(5),
            text: 0,
            values: vec![
                HoleValue::Int(3),
                HoleValue::Time(95 * caps::TICKS_PER_SECOND),
            ],
        }
    }

    #[test]
    fn the_codec_round_trips() {
        let strings = strings();
        let cases = vec![
            full(),
            Message {
                audience: EVERYONE,
                text: 1,
                values: vec![HoleValue::Player(Some(15)), HoleValue::Team(Some(7))],
            },
            Message {
                audience: 0,
                text: 2,
                values: vec![],
            },
            Message {
                audience: player(0),
                text: 3,
                values: vec![HoleValue::Enum(4)],
            },
            Message {
                audience: player(1),
                text: 1,
                values: vec![HoleValue::Player(None), HoleValue::Team(None)],
            },
            Message {
                audience: EVERYONE,
                text: 0,
                values: vec![HoleValue::Int(-2), HoleValue::Time(0)],
            },
        ];
        for message in cases {
            let cue = Cue::Message(message);
            let bytes = cue.encode().unwrap();
            assert!(bytes.len() <= MAX_CUE_BYTES);
            let back = Cue::decode(cue.kind(), &bytes, &strings).unwrap();
            assert_eq!(back, cue);
            // What decodes encodes to the same bytes.
            assert_eq!(back.encode().unwrap(), bytes);
        }
    }

    #[test]
    fn round_start_cues_name_only_types_with_our_audio() {
        for row in crate::types::TABLE {
            let cue = Cue::StartClip(row.ty);
            let code = row.ty.code();
            if row.start == StartLine::Ours {
                let bytes = cue.encode().unwrap();
                assert_eq!(Cue::decode(cue.kind(), &bytes, &[]), Ok(cue));
            } else {
                assert_eq!(cue.encode(), Err(CueError::NoStartClip(code)));
                assert_eq!(Cue::decode(START_CLIP, &[code], &[]), Err(CueError::NoStartClip(code)));
            }
        }
        assert_eq!(Cue::decode(START_CLIP, &[255], &[]), Err(CueError::NoStartClip(255)));
        assert_eq!(Cue::decode(START_CLIP, &[], &[]), Err(CueError::Truncated));
        assert_eq!(Cue::decode(START_CLIP, &[1, 2], &[]), Err(CueError::TrailingBytes));
    }

    /// The largest message, four int holes, fits the record's bound.
    #[test]
    fn the_largest_message_fits_the_bound() {
        let strings = vec!["{int}{int}{int}{int}".into()];
        let message = Message {
            audience: EVERYONE,
            text: 0,
            values: vec![HoleValue::Int(i32::MIN); caps::MAX_HOLES],
        };
        let bytes = Cue::Message(message.clone()).encode().unwrap();
        assert!(bytes.len() <= MAX_CUE_BYTES, "{} bytes", bytes.len());
        assert_eq!(
            Cue::decode(MESSAGE, &bytes, &strings),
            Ok(Cue::Message(message))
        );
    }

    #[test]
    fn the_codec_refuses_each_bad_bound() {
        let strings = strings();
        let bytes = full().encode().unwrap();
        // An unknown kind.
        assert_eq!(
            Cue::decode(9, &bytes, &strings),
            Err(CueError::UnknownKind(9))
        );
        // A payload over the record's bound.
        assert_eq!(
            Cue::decode(MESSAGE, &[0; MAX_CUE_BYTES + 1], &strings),
            Err(CueError::TooLong(MAX_CUE_BYTES + 1))
        );
        // Every truncation.
        for cut in 0..bytes.len() {
            assert!(
                matches!(
                    Cue::decode(MESSAGE, &bytes[..cut], &strings),
                    Err(CueError::Truncated
                        | CueError::BadText(_)
                        | CueError::BadTemplate
                        | CueError::Holes { .. })
                ),
                "cut at {cut} of {bytes:02x?}"
            );
        }
        // Bytes past the last value.
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(
            Cue::decode(MESSAGE, &longer, &strings),
            Err(CueError::TrailingBytes)
        );
        // More values than a message may show, at encode.
        let mut too_many = full();
        too_many.values = vec![HoleValue::Int(0); caps::MAX_HOLES + 1];
        assert_eq!(
            too_many.encode(),
            Err(CueError::TooManyHoles(caps::MAX_HOLES + 1))
        );
        // A text index outside the string table.
        let mut bad_text = bytes.clone();
        bad_text[..2].copy_from_slice(&(strings.len() as u16).to_le_bytes());
        assert_eq!(
            Cue::decode(MESSAGE, &bad_text, &strings),
            Err(CueError::BadText(strings.len() as u16))
        );
        // A text that is no valid template.
        let strings = vec!["{object}".into()];
        assert_eq!(
            Cue::decode(MESSAGE, &[0, 0, 1, 0, 0], &strings),
            Err(CueError::BadTemplate)
        );
        // A hole count that is not the template's.
        let strings = self::strings();
        let mut short = bytes.clone();
        short[4] = 1;
        assert_eq!(
            Cue::decode(MESSAGE, &short, &strings),
            Err(CueError::Holes { got: 1, want: 2 })
        );
        // A player slot outside the roster, in both directions.
        let mut bad_player = full();
        bad_player.values[0] = HoleValue::Player(Some(16));
        assert_eq!(bad_player.encode(), Err(CueError::BadPlayer(16)));
        let strings = vec!["{player}".into()];
        assert_eq!(
            Cue::decode(MESSAGE, &[0, 0, 1, 0, 1, 16], &strings),
            Err(CueError::BadPlayer(16))
        );
        // A team outside the eight, in both directions.
        let mut bad_team = full();
        bad_team.values[0] = HoleValue::Team(Some(8));
        assert_eq!(bad_team.encode(), Err(CueError::BadTeam(8)));
        let strings = vec!["{team}".into()];
        assert_eq!(
            Cue::decode(MESSAGE, &[0, 0, 1, 0, 1, 8], &strings),
            Err(CueError::BadTeam(8))
        );
        // An enum value outside the string table.
        let strings = vec!["{enum}".into()];
        assert_eq!(
            Cue::decode(MESSAGE, &[0, 0, 1, 0, 1, 9, 0], &strings),
            Err(CueError::BadEnum(9))
        );
    }

    #[test]
    fn a_walls_cue_round_trips_and_the_largest_fits_the_bound() {
        let walls: Vec<(u32, Option<u8>)> = (0..MAX_WALLS as u32)
            .map(|n| (ZONE_BASE | (n * 7 + 3), if n % 3 == 0 { None } else { Some(n as u8 % 8) }))
            .collect();
        let cue = Cue::Walls(walls);
        let bytes = cue.encode().unwrap();
        assert!(bytes.len() <= MAX_CUE_BYTES, "{} bytes", bytes.len());
        assert_eq!(cue.kind(), WALLS);
        assert_eq!(Cue::decode(WALLS, &bytes, &[]), Ok(cue));
        // None shown is a cue too: it takes every wall off.
        let none = Cue::Walls(Vec::new());
        assert_eq!(Cue::decode(WALLS, &none.encode().unwrap(), &[]), Ok(none));
    }

    #[test]
    fn the_codec_refuses_each_bad_walls_cue() {
        let zone = ZONE_BASE | 5;
        let enc = |w: Vec<(u32, Option<u8>)>| Cue::Walls(w).encode();
        assert_eq!(enc(vec![(7, None)]), Err(CueError::BadZone(7)));
        assert_eq!(enc(vec![(ZONE_BASE | 0x1000, None)]), Err(CueError::BadZone(ZONE_BASE | 0x1000)));
        assert_eq!(enc(vec![(zone, Some(8))]), Err(CueError::BadTeam(8)));
        assert_eq!(enc(vec![(zone, None), (zone, Some(1))]), Err(CueError::DuplicateWall(zone)));
        assert_eq!(enc(vec![(zone, None); 9]), Err(CueError::TooManyWalls(9)));
        let good = enc(vec![(zone, Some(1)), (zone + 1, None)]).unwrap();
        // Cut short, with a byte too many, a count over the bound, a team
        // past the eight, and the same zone twice.
        assert_eq!(Cue::decode(WALLS, &good[..good.len() - 1], &[]), Err(CueError::Truncated));
        let mut more = good.clone();
        more.push(0);
        assert_eq!(Cue::decode(WALLS, &more, &[]), Err(CueError::TrailingBytes));
        assert_eq!(Cue::decode(WALLS, &[9], &[]), Err(CueError::TooManyWalls(9)));
        let mut bad = good.clone();
        bad[2] = 0x8f;
        assert_eq!(Cue::decode(WALLS, &bad, &[]), Err(CueError::BadTeam(8)));
        let mut twice = good.clone();
        twice[3..5].copy_from_slice(&good[1..3]);
        assert_eq!(Cue::decode(WALLS, &twice, &[]), Err(CueError::DuplicateWall(zone)));
    }

    #[test]
    fn a_line_fills_its_holes_words() {
        let strings = strings();
        let name = |slot: u8| format!("player {slot}");
        let message = full();
        let template = &strings[usize::from(message.text)];
        assert_eq!(
            line(&strings, template, &message.values, &name),
            "Wave 3 in 1:35"
        );
        let message = Message {
            audience: EVERYONE,
            text: 1,
            values: vec![HoleValue::Player(Some(2)), HoleValue::Team(Some(1))],
        };
        assert_eq!(
            line(&strings, &strings[1], &message.values, &name),
            "player 2 scored for team 1"
        );
        let message = Message {
            audience: EVERYONE,
            text: 3,
            values: vec![HoleValue::Enum(4)],
        };
        assert_eq!(
            line(&strings, &strings[3], &message.values, &name),
            "attack"
        );
    }

    fn tags() -> Vec<crate::ir::TagDef> {
        use crate::ir::{TagDef, TagGroup};
        vec![
            TagDef::any("objects/x"),
            TagDef {
                path: "sound/stinger".into(),
                group: TagGroup::Sound,
            },
        ]
    }

    #[test]
    fn a_sound_cue_round_trips_and_names_only_a_sound_of_the_blob() {
        let cue = Cue::Sound(SoundCue {
            audience: player(1) | player(14),
            sound: 1,
        });
        let bytes = cue.encode().unwrap();
        assert_eq!(bytes.len(), SOUND_BYTES);
        assert_eq!(Cue::decode(cue.kind(), &bytes, &[]), Ok(cue.clone()));
        let Cue::Sound(sound) = cue else { unreachable!() };
        assert_eq!(sound.check(&tags()), Ok(()));
        // An index outside the table, and an entry of another group.
        assert_eq!(SoundCue { sound: 2, ..sound }.check(&tags()), Err(CueError::BadSound(2)));
        assert_eq!(SoundCue { sound: 0, ..sound }.check(&tags()), Err(CueError::BadSound(0)));
        // Short and long payloads.
        assert_eq!(Cue::decode(SOUND, &bytes[..3], &[]), Err(CueError::Truncated));
        let mut long = bytes.clone();
        long.push(0);
        assert_eq!(Cue::decode(SOUND, &long, &[]), Err(CueError::TrailingBytes));
    }

    /// The codec on arbitrary bytes: decoding never panics, and what
    /// decodes encodes back to the bytes it came from. A fixed seed makes
    /// the run the same each time.
    #[test]
    fn the_codec_survives_arbitrary_bytes() {
        let strings = strings();
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for _ in 0..4000 {
            let len = (next() % (MAX_CUE_BYTES as u64 + 4)) as usize;
            let bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            for kind in [MESSAGE, START_CLIP, SOUND, (next() % 256) as u8] {
                if let Ok(cue) = Cue::decode(kind, &bytes, &strings) {
                    assert_eq!(cue.kind(), kind);
                    assert_eq!(cue.encode().as_deref(), Ok(bytes.as_slice()));
                }
            }
        }
    }
}
