//! Halo 1 conversation definitions. The Xbox evaluator at `0x001425a0`
//! walks the lines and selects a sound using the speaking role's voice slot.

use std::collections::BTreeMap;

use h1map::{Group, H1Map, Space, TagId, TagRef};

use crate::bytes::{f32_at, fixed_str, i16_at, u16_at, u32_at};
use crate::h1::layout;
use crate::{Error, Result};

const OBJECT_NAMES: (usize, usize) = (0x204, 0x24);
const SOUND: Group = Group::new(b"snd!");

/// A scenario AI conversation in source block order.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    pub name: String,
    pub flags: u16,
    pub trigger_distance: f32,
    pub run_to_player_distance: f32,
    pub participants: Vec<Participant>,
    pub lines: Vec<Line>,
}

impl Conversation {
    /// The source engine retries a pending cast or proximity check.
    pub fn delays_start(&self) -> bool {
        self.flags & 0x40 != 0
    }

    pub fn runs_to_player(&self) -> bool {
        self.flags & 0x20 != 0 && self.run_to_player_distance > 0.0
    }

    pub fn requires_player_attention(&self) -> bool {
        self.flags & 0x80 != 0
    }
}

/// One role in the cast. Voice slots index the line's parallel sound array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Participant {
    pub flags: u16,
    pub selection: u16,
    pub actor_type: u16,
    pub existing_name: Option<String>,
    pub assigned_name: Option<String>,
    pub voices: [i16; 6],
    pub encounter: Option<String>,
}

impl Participant {
    /// Selection 1 is a recorded voice without an actor (`0x00143c4e`).
    pub fn radio(&self) -> bool {
        self.selection == 1
    }

    /// Selections 6 and 7 bind an actor but play the line on the radio
    /// channel (`0x001426a3` and `0x00144992`).
    pub fn nonspatial(&self) -> bool {
        matches!(self.selection, 1 | 6 | 7)
    }

    pub fn optional(&self) -> bool {
        self.flags & 1 != 0
    }
}

/// A line and its source sound paths in voice-slot order.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub flags: u16,
    pub speaker: i16,
    pub addressee_kind: i16,
    pub addressee: i16,
    pub pause_after: f32,
    pub sounds: [Option<String>; 6],
}

/// The object one cast member should turn to during a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addressee {
    Player,
    Role(usize),
}

impl Line {
    pub fn faces(&self, role: usize) -> Option<Addressee> {
        let speaker = usize::try_from(self.speaker).ok()?;
        let target = match self.addressee_kind {
            1 => Some(Addressee::Player),
            2 => usize::try_from(self.addressee)
                .ok()
                .filter(|&i| i != speaker)
                .map(Addressee::Role),
            _ => None,
        };
        if role == speaker {
            return target;
        }
        if target == Some(Addressee::Role(role)) {
            return (self.flags & 1 != 0).then_some(Addressee::Role(speaker));
        }
        if self.flags & 2 != 0 {
            Some(Addressee::Role(speaker))
        } else if self.flags & 4 != 0 {
            target
        } else {
            None
        }
    }

    pub fn waits_for_speaker(&self) -> bool {
        self.flags & 0x10 != 0
    }

    pub fn waits_for_cast(&self) -> bool {
        self.flags & 0x20 != 0
    }

    /// Hold at the end of the line until `ai_conversation_advance`.
    pub fn waits_for_advance(&self) -> bool {
        self.flags & 8 != 0
    }
}

/// Converted AI sources for a role with no authored encounter. Whole
/// encounter groups can be used when every populated squad matches.
#[derive(Debug, Default)]
pub struct SpeakerScopes {
    by_type: BTreeMap<u16, Vec<String>>,
    pub leaders: Vec<LeaderSquad>,
}

/// The original unique-leader rule for a converted marine squad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaderSquad {
    pub squad: u16,
    pub encounter: u16,
    pub selection: u16,
}

impl SpeakerScopes {
    pub fn from_plan(plan: &crate::convert::Plan) -> Self {
        let mut by_group = BTreeMap::<i16, Vec<_>>::new();
        for squad in &plan.ai.squads {
            // An empty squad can receive actors through ai_attach/ai_migrate.
            by_group.entry(squad.parent).or_default().push(squad);
        }
        let mut result = BTreeMap::<u16, Vec<String>>::new();
        let actor_type =
            |character: i16| plan.characters.get(character as usize).map(|c| c.h1_type.0);
        for (group, squads) in by_group {
            let kinds: Vec<Vec<u16>> = squads
                .iter()
                .map(|squad| {
                    let mut types: Vec<_> = std::iter::once(squad.character)
                        .chain(squad.starting_locations.iter().map(|l| l.character))
                        .filter_map(actor_type)
                        .collect();
                    types.sort_unstable();
                    types.dedup();
                    types
                })
                .collect();
            let common = kinds
                .first()
                .and_then(|types| types.first())
                .filter(|&&kind| kinds.iter().all(|types| types.as_slice() == [kind]));
            if let Some((&kind, group)) = common.zip(plan.ai.squad_groups.get(group as usize)) {
                result.entry(kind).or_default().push(group.name.clone());
            } else {
                for (squad, types) in squads.into_iter().zip(kinds) {
                    for kind in types {
                        result.entry(kind).or_default().push(squad.name.clone());
                    }
                }
            }
        }
        Self {
            by_type: result,
            leaders: plan.conversation_leaders.clone(),
        }
    }

    pub fn for_actor_type(&self, actor_type: u16) -> &[String] {
        self.by_type.get(&actor_type).map_or(&[], Vec::as_slice)
    }
}

/// Loads all conversation voices from the map's own tag index.
pub fn read(m: &H1Map) -> Result<Vec<Conversation>> {
    decode(m.tag_data(m.scenario())?, &m.tag_space(), |id| {
        let tag = m.tag(id)?;
        if tag.group != SOUND {
            return Err(Error::Invalid(format!(
                "conversation sound {id}: {}",
                tag.group
            )));
        }
        Ok(tag.name.clone())
    })
}

fn decode(
    scenario: &[u8],
    space: &Space<'_>,
    sound_name: impl Fn(TagId) -> Result<String>,
) -> Result<Vec<Conversation>> {
    let (offset, stride) = layout::CONVERSATIONS;
    let conversations = space.block_at(scenario, offset, stride)?;
    if conversations.is_empty() {
        return Ok(Vec::new());
    }
    let object_names = space.block_at(scenario, OBJECT_NAMES.0, OBJECT_NAMES.1)?;
    let object_name = |p: &[u8], at| -> Result<Option<String>> {
        let index = i16_at(p, at);
        if index == -1 {
            return Ok(None);
        }
        let entry = object_names
            .get(index as usize)
            .ok_or_else(|| Error::Invalid(format!("conversation object name index {index}")))?;
        Ok(Some(fixed_str(&entry[..32])))
    };
    let parse_participant = |p: &[u8]| -> Result<Participant> {
        let encounter = fixed_str(&p[0x24..0x44]);
        Ok(Participant {
            flags: u16_at(p, 2),
            selection: u16_at(p, 4),
            actor_type: u16_at(p, 6),
            existing_name: object_name(p, 8)?,
            assigned_name: object_name(p, 0xa)?,
            voices: std::array::from_fn(|i| i16_at(p, 0x18 + 2 * i)),
            encounter: (u32_at(p, 0x44) != u32::MAX && !encounter.is_empty()).then_some(encounter),
        })
    };
    let parse_line = |l: &[u8]| -> Result<Line> {
        let mut sounds = std::array::from_fn(|_| None);
        for (i, sound) in sounds.iter_mut().enumerate() {
            let r = TagRef::read(l, 0x1c + 0x10 * i).expect("conversation line stride");
            if !r.id.is_none() {
                if r.group != SOUND {
                    return Err(Error::Invalid(format!(
                        "conversation sound group {}",
                        r.group
                    )));
                }
                *sound = Some(sound_name(r.id)?);
            }
        }
        Ok(Line {
            flags: u16_at(l, 0),
            speaker: i16_at(l, 2),
            addressee_kind: i16_at(l, 4),
            addressee: i16_at(l, 6),
            pause_after: f32_at(l, 0xc),
            sounds,
        })
    };

    conversations
        .into_iter()
        .map(|c| {
            let (participant_at, participant_stride) = layout::CONV_PARTICIPANTS;
            let (line_at, line_stride) = layout::CONV_LINES;
            Ok(Conversation {
                name: fixed_str(&c[..32]),
                flags: u16_at(c, 0x20),
                trigger_distance: f32_at(c, 0x24),
                run_to_player_distance: f32_at(c, 0x28),
                participants: space
                    .block_at(c, participant_at, participant_stride)?
                    .into_iter()
                    .map(parse_participant)
                    .collect::<Result<_>>()?,
                lines: space
                    .block_at(c, line_at, line_stride)?
                    .into_iter()
                    .map(parse_line)
                    .collect::<Result<_>>()?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_line_faces_the_player_or_the_addressed_role() {
        let mut line = Line {
            flags: 1 | 4 | 0x20,
            speaker: 0,
            addressee_kind: 2,
            addressee: 1,
            pause_after: 0.0,
            sounds: Default::default(),
        };
        assert_eq!(line.faces(0), Some(Addressee::Role(1)));
        assert_eq!(line.faces(1), Some(Addressee::Role(0)));
        assert_eq!(line.faces(2), Some(Addressee::Role(1)));
        assert!(line.waits_for_cast());
        line.flags = 2 | 0x10;
        assert_eq!(line.faces(1), None);
        assert_eq!(line.faces(2), Some(Addressee::Role(0)));
        assert!(line.waits_for_speaker());
        line.addressee_kind = 1;
        line.flags = 4;
        assert_eq!(line.faces(0), Some(Addressee::Player));
        assert_eq!(line.faces(2), Some(Addressee::Player));
    }

    #[test]
    fn conversation_reads_role_names_and_parallel_voice_slots() {
        let base = 0x8000;
        let (names, conversation, role, line) = (0x480, 0x4a4, 0x518, 0x56c);
        let mut bytes = vec![0; 0x5e8];
        for (field, start) in [
            (OBJECT_NAMES.0, names),
            (layout::CONVERSATIONS.0, conversation),
            (conversation + layout::CONV_PARTICIPANTS.0, role),
            (conversation + layout::CONV_LINES.0, line),
        ] {
            bytes[field..field + 4].copy_from_slice(&1u32.to_le_bytes());
            bytes[field + 4..field + 8].copy_from_slice(&(base + start as u32).to_le_bytes());
        }
        bytes[names..names + 4].copy_from_slice(b"lead");
        bytes[conversation..conversation + 4].copy_from_slice(b"test");
        bytes[role + 0xa..role + 0xc].copy_from_slice(&(-1i16).to_le_bytes());
        bytes[role + 0x18..role + 0x24].fill(0xff);
        bytes[role + 0x1c..role + 0x1e].copy_from_slice(&7i16.to_le_bytes());
        bytes[role + 0x24..role + 0x29].copy_from_slice(b"guard");
        bytes[line..line + 2].copy_from_slice(&8u16.to_le_bytes());
        bytes[line + 0xc..line + 0x10].copy_from_slice(&0.5f32.to_le_bytes());
        for i in 0..6 {
            bytes[line + 0x28 + i * 16..line + 0x2c + i * 16].fill(0xff);
        }
        bytes[line + 0x3c..line + 0x40].copy_from_slice(&SOUND.0.to_le_bytes());
        bytes[line + 0x48..line + 0x4c].copy_from_slice(&0x1234_0001u32.to_le_bytes());

        let scenes = decode(&bytes[..names], &Space::new(&bytes, base), |id| {
            assert_eq!(id, TagId(0x1234_0001));
            Ok("sound\\alternate".into())
        })
        .unwrap();
        let c = &scenes[0];
        assert_eq!(c.name, "test");
        assert_eq!(c.participants[0].existing_name.as_deref(), Some("lead"));
        assert_eq!(c.participants[0].assigned_name, None);
        assert_eq!(c.participants[0].encounter.as_deref(), Some("guard"));
        assert_eq!(c.participants[0].voices, [-1, -1, 7, -1, -1, -1]);
        assert!(c.lines[0].waits_for_advance());
        assert_eq!(c.lines[0].pause_after, 0.5);
        assert_eq!(c.lines[0].sounds[2].as_deref(), Some("sound\\alternate"));
        assert_eq!(c.lines[0].sounds.iter().flatten().count(), 1);
    }
}
