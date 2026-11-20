//! `sim.objects`: every live object as the simulation's typed views read
//! it (`sim-state`, docs/engine/sim-rewrite-objects.md section 8): kind,
//! header flags, links, transform, bounds, vitality, node count and, for
//! units, the unit fields. Read at a tick boundary.

use serde::{Deserialize, Serialize};

/// `sim.objects`' params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectsParams {
    /// Only these kinds, by tag group (`bipd`, `vehi`, `weap`, `eqip`,
    /// `garb`, `proj`, `scen`, `mach`, `ctrl`, `lifi`, `ssce`, `bloc`,
    /// `crea`). Empty: every kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
    /// At most this many (default and cap [`ObjectsReply::MAX`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<usize>,
}

/// `sim.objects`' answer: live objects in slot order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectsReply {
    pub objects: Vec<SimObject>,
    /// More objects matched than were returned.
    pub truncated: bool,
}

impl ObjectsReply {
    /// The objects pool's capacity.
    pub const MAX: usize = 2048;
}

/// One live object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimObject {
    /// Its salted handle ([`handle`]).
    pub id: String,
    /// Its tag group; `null` for a type outside the table.
    pub kind: Option<String>,
    /// Its definition tag's datum, `null` when none.
    pub definition: Option<String>,
    /// Header flags set: `active`, `awake`, `post`, `new`, `deleting`,
    /// `defer`, `attach`, `child`.
    pub flags: Vec<String>,
    /// The object it is attached to.
    pub parent: Option<String>,
    /// Parent-relative when it has a parent.
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
    pub scale: f32,
    /// World units a second.
    pub velocity: [f32; 3],
    /// The bounding sphere, world space.
    pub sphere: SimSphere,
    pub vitality: SimVitality,
    /// How many node matrices it has.
    pub nodes: usize,
    /// Bipeds and vehicles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<SimUnit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimSphere {
    pub center: [f32; 3],
    pub radius: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimVitality {
    /// 1.0 full.
    pub body: f32,
    /// 1.0 full; an overshield goes past it.
    pub shield: f32,
    pub depleted: bool,
}

/// A unit's fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimUnit {
    /// The controlling player's datum.
    pub player: Option<String>,
    /// The campaign actor driving it.
    pub actor: Option<String>,
    pub team: i16,
    /// The seat it rides in.
    pub seat: Option<i16>,
    /// The four weapon slots.
    pub weapons: Vec<Option<String>>,
    /// The weapon in its hands.
    pub held: Option<String>,
}

/// A datum as the answer writes it: `0x` and eight hex digits.
pub fn handle(raw: u32) -> String {
    format!("0x{raw:08x}")
}
