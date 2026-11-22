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

use crate::caps;
use crate::ty::{MAX_PLAYERS, MAX_TEAMS};

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

/// A one-shot show event. Sounds and medal lines join in their slices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cue {
    Message(Message),
}

/// The kind byte of a message cue in the `Cue` record's payload.
pub const MESSAGE: u8 = 0;

impl Cue {
    /// The kind byte of the `Cue` record's payload.
    pub fn kind(&self) -> u8 {
        match self {
            Cue::Message(_) => MESSAGE,
        }
    }

    /// The cue's payload, at most [`MAX_CUE_BYTES`] bytes.
    pub fn encode(&self) -> Result<Vec<u8>, CueError> {
        match self {
            Cue::Message(message) => message.encode(),
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
            kind => Err(CueError::UnknownKind(kind)),
        }
    }
}

/// Why a cue does not encode or decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CueError {
    /// The kind byte is no cue's.
    UnknownKind(u8),
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
            Cue::decode(MESSAGE, &vec![0; MAX_CUE_BYTES + 1], &strings),
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
}
