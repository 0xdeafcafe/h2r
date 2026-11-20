//! Halo 1's AI data, read out of a cache file through [`h1map`]: the
//! scenario's actor palette, encounters, command lists and conversations,
//! and the `actr` (actor) and `actv` (actor variant) tags.
//!
//! Offsets and strides are in [`layout`]; evidence per field is in
//! `docs/halo1/ai.md`. Strides were confirmed on every campaign map by
//! tiling (a block's elements end where the next block starts) and the
//! values by the range checks in the disc test.

use h1map::{Group, H1Map, Space, Tag, TagId, TagRef};

use crate::bytes::{f32_at, fixed_str, i16_at, u16_at, u32_at, vec3};
use crate::{Error, Result};

pub use crate::conversations::Conversation;

/// Scenario field offsets and element sizes, `(offset, stride)`.
pub mod layout {
    /// Actor palette: an `actv` reference each.
    pub const ACTOR_PALETTE: (usize, usize) = (0x420, 0x10);
    /// Encounters.
    pub const ENCOUNTERS: (usize, usize) = (0x42c, 0xb0);
    /// Command lists.
    pub const COMMAND_LISTS: (usize, usize) = (0x438, 0x60);
    /// AI animation references (counted only).
    pub const AI_ANIMATIONS: (usize, usize) = (0x444, 0x3c);
    /// AI script references (counted only).
    pub const AI_SCRIPTS: (usize, usize) = (0x450, 0x28);
    /// AI recording references (counted only).
    pub const AI_RECORDINGS: (usize, usize) = (0x45c, 0x28);
    /// AI conversations.
    pub const CONVERSATIONS: (usize, usize) = (0x468, 0x74);

    /// Encounter: squads.
    pub const ENC_SQUADS: (usize, usize) = (0x80, 0xe8);
    /// Encounter: platoons.
    pub const ENC_PLATOONS: (usize, usize) = (0x8c, 0xac);
    /// Encounter: firing positions.
    pub const ENC_FIRING_POSITIONS: (usize, usize) = (0x98, 0x18);
    /// Encounter: player starting locations (the scenario's own layout).
    pub const ENC_PLAYER_STARTS: (usize, usize) = (0xa4, 0x34);

    /// Squad: move positions.
    pub const SQUAD_MOVE_POSITIONS: (usize, usize) = (0xc4, 0x50);
    /// Squad: starting locations.
    pub const SQUAD_STARTING_LOCATIONS: (usize, usize) = (0xd0, 0x1c);

    /// Command list: atoms.
    pub const CL_ATOMS: (usize, usize) = (0x30, 0x20);
    /// Command list: points.
    pub const CL_POINTS: (usize, usize) = (0x3c, 0x14);

    /// Conversation: participants.
    pub const CONV_PARTICIPANTS: (usize, usize) = (0x50, 0x54);
    /// Conversation: lines.
    pub const CONV_LINES: (usize, usize) = (0x5c, 0x7c);

    /// `actr` tag size.
    pub const ACTOR_SIZE: usize = 0x4f8;
    /// `actv` tag size, without its change-colour block's elements.
    pub const ACTOR_VARIANT_SIZE: usize = 0x238;
}

/// Halo 1 actor type (`actr +0x14`, `u16`). Halo 2's `char` general
/// properties type uses the same numbering and extends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActorType(pub u16);

impl ActorType {
    /// A label for the type: its number. (Names for the types will come
    /// back as disc evidence ties each to its actor tags.)
    pub fn name(self) -> String {
        format!("type {}", self.0)
    }
}

/// An `actr` tag: what kind of actor, and its flags.
#[derive(Debug, Clone, PartialEq)]
pub struct Actor {
    /// Tag path.
    pub name: String,
    /// Flags (`u32 +0x00`).
    pub flags: u32,
    /// More flags (`u32 +0x04`).
    pub more_flags: u32,
    /// Type (`u16 +0x14`).
    pub kind: ActorType,
    /// Ordinary uncover requirements, in seconds and a fraction of body vitality.
    pub uncover: [f32; 2],
}

/// An `actv` tag: an actor with a unit, a weapon and tuning.
#[derive(Debug, Clone, PartialEq)]
pub struct ActorVariant {
    /// Tag path.
    pub name: String,
    /// Flags (`u32 +0x00`).
    pub flags: u32,
    /// The `actr` (`+0x04`).
    pub actor: TagRef,
    /// The unit, a `bipd` or `vehi` (`+0x14`).
    pub unit: TagRef,
    /// The variant this one upgrades to (`+0x24`, an `actv`).
    pub major_variant: TagRef,
    /// The weapon (`+0x64`, a `weap`).
    pub weapon: TagRef,
}

/// A squad's starting location (`0x1c` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct StartingLocation {
    /// Position.
    pub position: [f32; 3],
    /// Facing, radians.
    pub facing: f32,
    /// Sequence ID (`i8 +0x12`).
    pub sequence_id: i8,
    /// Flags (`u8 +0x13`; bit 0 "required").
    pub flags: u8,
    /// Return state (`u16 +0x14`).
    pub return_state: u16,
    /// Initial state (`u16 +0x16`).
    pub initial_state: u16,
    /// Actor type override: an actor palette index, `-1` the squad's.
    pub actor: i16,
    /// Command list run at spawn, `-1` none.
    pub command_list: i16,
}

/// A squad's move position (`0x50` bytes): where the squad walks when it
/// has nothing to do.
#[derive(Debug, Clone, PartialEq)]
pub struct MovePosition {
    /// Position.
    pub position: [f32; 3],
    /// Facing, radians.
    pub facing: f32,
    /// Relative weight.
    pub weight: f32,
    /// Seconds spent there, (min, max).
    pub time: [f32; 2],
    /// AI animation reference index, `-1` none.
    pub animation: i16,
}

/// A squad (`0xe8` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct Squad {
    /// Name.
    pub name: String,
    /// Actor palette index (`+0x20`).
    pub actor: i16,
    /// Platoon index in the encounter, `-1` none (`+0x22`).
    pub platoon: i16,
    /// Initial state (`+0x24`).
    pub initial_state: u16,
    /// Return state (`+0x26`).
    pub return_state: u16,
    /// Flags (`+0x28`).
    pub flags: u32,
    /// Unique leader type (`+0x2c`).
    pub unique_leader: u16,
    /// Squad index this one maneuvers to, `-1` none (`+0x4e`).
    pub maneuver_to: i16,
    /// Seconds before the squad spawns (`+0x50`).
    pub delay: f32,
    /// Firing position groups (bit 0 = A) while attacking, attacking and
    /// searching, attacking and guarding, defending, defending and
    /// searching, defending and guarding, pursuing (`+0x54`..`+0x6c`).
    pub groups: [u32; 7],
    /// Actors on normal (`+0x7c`).
    pub normal_count: i16,
    /// Actors on legendary (`+0x7e`).
    pub insane_count: i16,
    /// Major upgrade (`+0x80`).
    pub major_upgrade: u16,
    /// Respawn: minimum actors, maximum actors, total (`+0x84`..`+0x88`).
    pub respawn: [i16; 3],
    /// Respawn delay, seconds (min, max) (`+0x8c`).
    pub respawn_delay: [f32; 2],
    /// Move positions.
    pub move_positions: Vec<MovePosition>,
    /// Starting locations.
    pub starting_locations: Vec<StartingLocation>,
}

impl Squad {
    /// Firing position groups while attacking (the first mask).
    pub fn attacking(&self) -> u32 {
        self.groups[0]
    }
}

/// A platoon (`0xac` bytes): squads that change state together.
#[derive(Debug, Clone, PartialEq)]
pub struct Platoon {
    /// Name.
    pub name: String,
    /// Flags (`+0x20`).
    pub flags: u32,
    /// Condition for reversing the initial attack/defend state (`+0x30`).
    pub change: PlatoonTrigger,
    /// Condition for arming the maneuver order (`+0x3c`).
    pub maneuver: PlatoonTrigger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatoonTrigger {
    /// The retail condition enum. Zero disables the trigger.
    pub condition: u16,
    /// Platoon whose status is tested, or -1 for the whole encounter.
    pub subject: i16,
}

impl PlatoonTrigger {
    pub const DISABLED: Self = Self {
        condition: 0,
        subject: -1,
    };

    fn read(bytes: &[u8], at: usize) -> Self {
        Self {
            condition: u16_at(bytes, at),
            subject: i16_at(bytes, at + 2),
        }
    }
}

/// A firing position (`0x18` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct FiringPosition {
    /// Position.
    pub position: [f32; 3],
    /// Group, 0 = A .. 25 = Z (`u16 +0x0c`).
    pub group: u16,
    /// Cluster (`i16 +0x0e`, filled by the tools).
    pub cluster: i16,
    /// Collision surface (`u32 +0x14`, filled by the tools).
    pub surface: u32,
}

/// An encounter (`0xb0` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct Encounter {
    /// Name.
    pub name: String,
    /// Flags (`u32 +0x20`).
    pub flags: u32,
    /// Team (`u16 +0x24`).
    pub team: u16,
    /// Search behavior (`u16 +0x28`).
    pub search_behavior: u16,
    /// Manual BSP index (`i16 +0x2a`).
    pub manual_bsp: i16,
    /// BSP assigned by the Halo 1 cache compiler (+0x7e), -1 when unassigned.
    pub bsp: i16,
    /// Respawn delay, seconds (min, max) (`+0x2c`).
    pub respawn_delay: [f32; 2],
    /// Squads.
    pub squads: Vec<Squad>,
    /// Platoons.
    pub platoons: Vec<Platoon>,
    /// Firing positions.
    pub firing_positions: Vec<FiringPosition>,
    /// Player starting locations: positions only.
    pub player_starts: Vec<[f32; 3]>,
}

/// A command list atom (`0x20` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct Atom {
    /// Atom type (`u16 +0x00`), see [`atom_name`].
    pub kind: u16,
    /// Modifier (`i16 +0x02`).
    pub modifier: i16,
    /// Two parameters (`f32 +0x04`, `+0x08`).
    pub parameters: [f32; 2],
    /// Two point indices into the list's points, `-1` none (`+0x0c`, `+0x0e`).
    pub points: [i16; 2],
    /// AI animation reference index (`+0x10`).
    pub animation: i16,
    /// AI script reference index (`+0x12`).
    pub script: i16,
    /// AI recording reference index (`+0x14`).
    pub recording: i16,
    /// Branch target within this list (`+0x16`).
    pub command: i16,
    /// Scenario object name index (`+0x18`).
    pub object_name: i16,
}

/// A label for a command list atom type: its number.
pub fn atom_name(kind: u16) -> String {
    format!("atom {kind}")
}

/// Only these dispatch cases dereference point indices. Timed movement (22) uses facing.
pub fn atom_points(a: &Atom) -> Vec<i16> {
    match a.kind {
        1 | 3 | 4 | 7 | 8 => vec![a.points[0]],
        2 | 23 | 27 => a.points.to_vec(),
        _ => Vec::new(),
    }
}

/// A command list point (`0x14` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    /// Position.
    pub position: [f32; 3],
    /// Collision surface (`u32 +0x0c`).
    pub surface: u32,
}

/// A command list (`0x60` bytes): a scripted sequence of atoms over points.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandList {
    /// Name.
    pub name: String,
    /// Flags (`u32 +0x20`).
    pub flags: u32,
    /// Manual BSP index (`i16 +0x2c`).
    pub manual_bsp: i16,
    /// BSP resolved by the H1 cache compiler (+0x2e), including automatically assigned lists.
    pub bsp: i16,
    /// Atoms.
    pub atoms: Vec<Atom>,
    /// Points.
    pub points: Vec<Point>,
}

/// All of a Halo 1 scenario's AI data.
#[derive(Debug, Clone, PartialEq)]
pub struct H1Ai {
    /// Actor palette: the `actv` each entry names, resolved.
    pub actor_palette: Vec<Option<ActorVariant>>,
    /// The `actr` tags those variants name, by tag ID.
    pub actors: Vec<(TagId, Actor)>,
    /// Encounters.
    pub encounters: Vec<Encounter>,
    /// Command lists.
    pub command_lists: Vec<CommandList>,
    /// Conversations.
    pub conversations: Vec<Conversation>,
    pub voice_definitions: Vec<crate::voices::Definition>,
    /// AI animation, script and recording reference counts.
    pub reference_counts: [usize; 3],
}

fn tag_of<'a>(m: &'a H1Map, r: &TagRef, group: &[u8; 4]) -> Result<Option<&'a Tag>> {
    if r.id.is_none() {
        return Ok(None);
    }
    let t = m.tag(r.id)?;
    if t.group != Group::new(group) {
        return Err(Error::Invalid(format!(
            "{} is a {}, want {}",
            t.name,
            t.group,
            String::from_utf8_lossy(group)
        )));
    }
    Ok(Some(t))
}

fn tag_ref(b: &[u8], at: usize) -> TagRef {
    TagRef::read(b, at).expect("caller checked the length")
}

/// Reads an `actr` tag.
pub fn actor(m: &H1Map, t: &Tag) -> Result<Actor> {
    let b = m.tag_data(t)?;
    if b.len() < layout::ACTOR_SIZE {
        return Err(Error::Invalid(format!(
            "actr {} is {} bytes",
            t.name,
            b.len()
        )));
    }
    Ok(Actor {
        name: t.name.clone(),
        flags: u32_at(b, 0),
        more_flags: u32_at(b, 4),
        kind: ActorType(u16_at(b, 0x14)),
        uncover: [f32_at(b, 0x2d8), f32_at(b, 0x324)],
    })
}

/// A `weap` tag's report loudness (`+0x270`). Halo 1 multiplies it by
/// three and adds the hearing unit's own radius (`0x0014e379`) for the
/// reach of a shot's alert report to the AI.
pub fn weapon_loudness(m: &H1Map, t: &Tag) -> Result<f32> {
    let b = m.tag_data(t)?;
    if b.len() < 0x274 {
        return Err(Error::Invalid(format!(
            "weap {} is {} bytes",
            t.name,
            b.len()
        )));
    }
    Ok(f32_at(b, 0x270))
}

/// Reads an `actv` tag.
pub fn actor_variant(m: &H1Map, t: &Tag) -> Result<ActorVariant> {
    let b = m.tag_data(t)?;
    if b.len() < layout::ACTOR_VARIANT_SIZE {
        return Err(Error::Invalid(format!(
            "actv {} is {} bytes",
            t.name,
            b.len()
        )));
    }
    Ok(ActorVariant {
        name: t.name.clone(),
        flags: u32_at(b, 0),
        actor: tag_ref(b, 0x04),
        unit: tag_ref(b, 0x14),
        major_variant: tag_ref(b, 0x24),
        weapon: tag_ref(b, 0x64),
    })
}

fn squad(sp: &Space, e: &[u8]) -> Result<Squad> {
    let b = |l: (usize, usize)| sp.block_at(e, l.0, l.1);
    Ok(Squad {
        name: fixed_str(&e[..0x20]),
        actor: i16_at(e, 0x20),
        platoon: i16_at(e, 0x22),
        initial_state: u16_at(e, 0x24),
        return_state: u16_at(e, 0x26),
        flags: u32_at(e, 0x28),
        unique_leader: u16_at(e, 0x2c),
        maneuver_to: i16_at(e, 0x4e),
        delay: f32_at(e, 0x50),
        groups: std::array::from_fn(|i| u32_at(e, 0x54 + 4 * i)),
        normal_count: i16_at(e, 0x7c),
        insane_count: i16_at(e, 0x7e),
        major_upgrade: u16_at(e, 0x80),
        respawn: std::array::from_fn(|i| i16_at(e, 0x84 + 2 * i)),
        respawn_delay: [f32_at(e, 0x8c), f32_at(e, 0x90)],
        move_positions: b(layout::SQUAD_MOVE_POSITIONS)?
            .into_iter()
            .map(|p| MovePosition {
                position: vec3(p, 0),
                facing: f32_at(p, 0xc),
                weight: f32_at(p, 0x10),
                time: [f32_at(p, 0x14), f32_at(p, 0x18)],
                animation: i16_at(p, 0x1c),
            })
            .collect(),
        starting_locations: b(layout::SQUAD_STARTING_LOCATIONS)?
            .into_iter()
            .map(|s| StartingLocation {
                position: vec3(s, 0),
                facing: f32_at(s, 0xc),
                sequence_id: s[0x12] as i8,
                flags: s[0x13],
                return_state: u16_at(s, 0x14),
                initial_state: u16_at(s, 0x16),
                actor: i16_at(s, 0x18),
                command_list: i16_at(s, 0x1a),
            })
            .collect(),
    })
}

fn encounter(sp: &Space, e: &[u8]) -> Result<Encounter> {
    let b = |l: (usize, usize)| sp.block_at(e, l.0, l.1);
    Ok(Encounter {
        name: fixed_str(&e[..0x20]),
        flags: u32_at(e, 0x20),
        team: u16_at(e, 0x24),
        search_behavior: u16_at(e, 0x28),
        manual_bsp: i16_at(e, 0x2a),
        bsp: i16_at(e, 0x7e),
        respawn_delay: [f32_at(e, 0x2c), f32_at(e, 0x30)],
        squads: b(layout::ENC_SQUADS)?
            .into_iter()
            .map(|s| squad(sp, s))
            .collect::<Result<_>>()?,
        platoons: b(layout::ENC_PLATOONS)?
            .into_iter()
            .map(|p| Platoon {
                name: fixed_str(&p[..0x20]),
                flags: u32_at(p, 0x20),
                change: PlatoonTrigger::read(p, 0x30),
                maneuver: PlatoonTrigger::read(p, 0x3c),
            })
            .collect(),
        firing_positions: b(layout::ENC_FIRING_POSITIONS)?
            .into_iter()
            .map(|f| FiringPosition {
                position: vec3(f, 0),
                group: u16_at(f, 0xc),
                cluster: i16_at(f, 0xe),
                surface: u32_at(f, 0x14),
            })
            .collect(),
        player_starts: b(layout::ENC_PLAYER_STARTS)?
            .into_iter()
            .map(|p| vec3(p, 0))
            .collect(),
    })
}

/// Read the scenario's command lists without loading encounter or actor tags.
pub fn command_lists(scnr: &[u8], sp: &Space) -> Result<Vec<CommandList>> {
    sp.block_at(scnr, layout::COMMAND_LISTS.0, layout::COMMAND_LISTS.1)?
        .into_iter()
        .map(|e| command_list(sp, e))
        .collect()
}

/// Keep the authored list index stable even when list names repeat or collide
/// with campaign scripts. Both the placement record and HS use this name.
pub fn command_script_name(index: usize) -> String {
    format!("h1_cmd_{index}")
}

fn command_list(sp: &Space, e: &[u8]) -> Result<CommandList> {
    let b = |l: (usize, usize)| sp.block_at(e, l.0, l.1);
    Ok(CommandList {
        name: fixed_str(&e[..0x20]),
        flags: u32_at(e, 0x20),
        manual_bsp: i16_at(e, 0x2c),
        bsp: i16_at(e, 0x2e),
        atoms: b(layout::CL_ATOMS)?
            .into_iter()
            .map(|a| Atom {
                kind: u16_at(a, 0),
                modifier: i16_at(a, 2),
                parameters: [f32_at(a, 4), f32_at(a, 8)],
                points: [i16_at(a, 0xc), i16_at(a, 0xe)],
                animation: i16_at(a, 0x10),
                script: i16_at(a, 0x12),
                recording: i16_at(a, 0x14),
                command: i16_at(a, 0x16),
                object_name: i16_at(a, 0x18),
            })
            .collect(),
        points: b(layout::CL_POINTS)?
            .into_iter()
            .map(|p| Point {
                position: vec3(p, 0),
                surface: u32_at(p, 0xc),
            })
            .collect(),
    })
}

impl H1Ai {
    /// Reads the AI data of `m`'s scenario, and every actor variant and
    /// actor its palette names.
    pub fn read(m: &H1Map) -> Result<Self> {
        let scnr = m.tag_data(m.scenario())?;
        if scnr.len() < layout::CONVERSATIONS.0 + 12 {
            return Err(Error::Invalid("scenario tag too short".into()));
        }
        let sp = m.tag_space();
        let b = |l: (usize, usize)| sp.block_at(scnr, l.0, l.1);
        let mut actors: Vec<(TagId, Actor)> = Vec::new();
        let mut actor_palette = Vec::new();
        for e in b(layout::ACTOR_PALETTE)? {
            let Some(t) = tag_of(m, &tag_ref(e, 0), b"actv")? else {
                actor_palette.push(None);
                continue;
            };
            let v = actor_variant(m, t)?;
            if let Some(a) = tag_of(m, &v.actor, b"actr")?
                && !actors.iter().any(|(id, _)| *id == a.id)
            {
                actors.push((a.id, actor(m, a)?));
            }
            actor_palette.push(Some(v));
        }
        Ok(Self {
            actor_palette,
            actors,
            encounters: b(layout::ENCOUNTERS)?
                .into_iter()
                .map(|e| encounter(&sp, e))
                .collect::<Result<_>>()?,
            command_lists: command_lists(scnr, &sp)?,
            conversations: crate::conversations::read(m)?,
            voice_definitions: crate::voices::read(m)?,
            reference_counts: [
                b(layout::AI_ANIMATIONS)?.len(),
                b(layout::AI_SCRIPTS)?.len(),
                b(layout::AI_RECORDINGS)?.len(),
            ],
        })
    }

    /// The actor (`actr`) behind actor palette entry `i`.
    pub fn actor_of(&self, i: usize) -> Option<&Actor> {
        let v = self.actor_palette.get(i)?.as_ref()?;
        self.actors
            .iter()
            .find(|(id, _)| *id == v.actor.id)
            .map(|(_, a)| a)
    }

    /// Every index this data holds that must be in range, checked: actor
    /// palette entries of squads and starting locations, platoons, maneuver
    /// squads, starting locations' command lists, atoms' points and command
    /// lists, firing position groups. Returns one message per problem.
    pub fn check_indices(&self) -> Vec<String> {
        let mut bad = Vec::new();
        let opt = |i: i16, n: usize| i == -1 || (i >= 0 && (i as usize) < n);
        let actors = self.actor_palette.len();
        let lists = self.command_lists.len();
        for e in &self.encounters {
            for s in &e.squads {
                if !opt(s.actor, actors) {
                    bad.push(format!("{}/{}: actor {}", e.name, s.name, s.actor));
                }
                if !opt(s.platoon, e.platoons.len()) {
                    bad.push(format!("{}/{}: platoon {}", e.name, s.name, s.platoon));
                }
                if !opt(s.maneuver_to, e.squads.len()) {
                    bad.push(format!("{}/{}: maneuver {}", e.name, s.name, s.maneuver_to));
                }
                for l in &s.starting_locations {
                    if !opt(l.actor, actors) || !opt(l.command_list, lists) {
                        bad.push(format!(
                            "{}/{}: start actor {} list {}",
                            e.name, s.name, l.actor, l.command_list
                        ));
                    }
                }
            }
            for f in &e.firing_positions {
                if f.group >= 26 {
                    bad.push(format!("{}: firing position group {}", e.name, f.group));
                }
            }
        }
        for c in &self.command_lists {
            for a in &c.atoms {
                if a.kind >= 28 {
                    bad.push(format!("{}: atom type {}", c.name, a.kind));
                }
                // Fields an atom type doesn't use keep stale values, so
                // only the ones it reads are checked.
                for p in atom_points(a) {
                    if !opt(p, c.points.len()) {
                        bad.push(format!("{}: {} point {p}", c.name, atom_name(a.kind)));
                    }
                }
                if a.kind == 20 && !opt(a.command, c.atoms.len()) {
                    bad.push(format!("{}: loop to atom {}", c.name, a.command));
                }
                if a.kind == 13 && !opt(a.animation, self.reference_counts[0]) {
                    bad.push(format!("{}: animation {}", c.name, a.animation));
                }
            }
        }
        for c in &self.conversations {
            for line in &c.lines {
                if !opt(line.speaker, c.participants.len()) {
                    bad.push(format!("{}: line participant {}", c.name, line.speaker));
                }
            }
        }
        bad
    }
}
