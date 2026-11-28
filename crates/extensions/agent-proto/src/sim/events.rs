//! `sim.events`: the tick's simulation events (`sim_state::events`,
//! docs/engine/sim-rewrite-objects.md section 8): objects deleted, damage
//! applied and kills credited, in the order they happened. Read at a tick
//! boundary, so they are the last tick's. Deletions, damage and kills are
//! always recorded.

use serde::{Deserialize, Serialize};

/// `sim.events`' params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventsParams {
    /// Only these event types ([`EventKind::NAMES`]). Empty: every type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
}

/// An event's type, as `kinds` and [`SimEvent`]'s `type` name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Deleted,
    Damage,
    Kill,
    ActorKilled,
}

impl EventKind {
    pub const ALL: [EventKind; 4] = [
        EventKind::Deleted, EventKind::Damage, EventKind::Kill, EventKind::ActorKilled,
    ];
    pub const NAMES: [&'static str; 4] = ["deleted", "damage", "kill", "actor_killed"];

    pub const fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    pub fn parse(name: &str) -> Option<EventKind> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// `sim.events`' answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventsReply {
    pub events: Vec<SimEvent>,
}

/// One event. Datums are `0x` and eight hex digits
/// ([`super::objects::handle`]), `null` for none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SimEvent {
    /// Finalize deleted an object.
    Deleted {
        id: String,
        /// Its tag group; `null` outside the type table.
        kind: Option<String>,
        definition: Option<String>,
    },
    Damage(DamageEvent),
    Kill(KillEvent),
    ActorKilled {
        unit: String,
        character: Option<String>,
        killer: Option<String>,
        /// Packed damage code of the killing blow.
        weapon: u8,
    },
}

/// A damage instance, as the damage report applied it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DamageEvent {
    /// The game tick; `null` without game time.
    pub tick: Option<u32>,
    pub victim: String,
    /// The victim's tag group; `null` when its handle did not resolve.
    pub victim_kind: Option<String>,
    /// The player controlling the victim, for a unit with one.
    pub victim_player: Option<String>,
    /// Who the damage is credited to: player, object and team.
    pub owner_player: Option<String>,
    pub owner_object: Option<String>,
    pub owner_team: i16,
    /// The packed damage code, and its two halves: the damage reporting
    /// type (low 6 bits) and the modifier (high 2).
    pub code: u8,
    pub damage_type: u8,
    pub modifier: u8,
    /// The damage effect (`jpt!`) tag.
    pub effect: Option<String>,
    /// The report flags, and whether this damage depleted the body.
    pub flags: u32,
    pub depleting: bool,
    /// Body and shield damage, fractions of full vitality.
    pub body: f32,
    pub shield: f32,
    pub direction: [f32; 3],
    pub origin: [f32; 3],
    /// The victim's position; `null` when its handle did not resolve.
    pub position: Option<[f32; 3]>,
}

/// A kill, as the game credited it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KillEvent {
    pub tick: Option<u32>,
    /// The killer's player; `null` for none.
    pub attacker: Option<String>,
    /// The killed player.
    pub victim: Option<String>,
    pub code: u8,
    pub damage_type: u8,
    pub modifier: u8,
    /// Each player's unit, and its position.
    pub victim_unit: Option<String>,
    pub attacker_unit: Option<String>,
    pub position: Option<[f32; 3]>,
    pub attacker_position: Option<[f32; 3]>,
}
