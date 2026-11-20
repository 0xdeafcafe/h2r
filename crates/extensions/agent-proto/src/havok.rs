//! `havok.*`: the Havok physics world, read (docs/runtime/agent-api.md,
//! "havok"; layout in docs/engine/havok-runtime.md). Wire types only.

use serde::{Deserialize, Serialize};

/// Method names. They are game methods: `session.methods` lists them where
/// the game registered them (`agent::havok::install`).
pub mod method {
    /// Rigid bodies ([`super::BodiesParams`] → [`super::Bodies`]).
    pub const BODIES: &str = "havok.bodies";
    /// Contact points ([`super::ContactsParams`] → [`super::Contacts`]).
    pub const CONTACTS: &str = "havok.contacts";

    /// Per-pair cached collision state ([`super::AgentsParams`] → [`super::Agents`]).
    pub const AGENTS: &str = "havok.agents";

    pub const ALL: &[&str] = &[BODIES, CONTACTS, AGENTS];
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodiesParams {
    /// Only bodies within `radius` of this object's body (or its position
    /// when it has none), nearest first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub near: Option<String>,
    /// World units; default [`BodiesParams::RADIUS`]. Needs `near`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    /// At most this many bodies; default [`BodiesParams::LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

impl BodiesParams {
    pub const RADIUS: f64 = 5.0;
    pub const LIMIT: usize = 64;
    pub const MAX_LIMIT: usize = 1024;
}

/// One rigid body.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Body {
    /// The Havok entity's guest address, 8 hex digits: the key
    /// `havok.contacts` names bodies by.
    pub entity: String,
    /// `active`, `inactive` (asleep) or `fixed` (the level, static bodies).
    pub state: String,
    /// The object's agent id, where it is one the API lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    /// The object datum handle, 8 hex digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datum: Option<String>,
    pub pos: [f64; 3],
    pub velocity: [f64; 3],
    pub angular_velocity: [f64; 3],
    /// 0 for a body that does not move.
    pub inv_mass: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mass: Option<f64>,
    pub friction: f64,
    pub restitution: f64,
    /// From `near`'s body, with `near`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<f64>,
    /// The collidable's shape, where it reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<Shape>,
}

/// A Havok shape (docs/engine/havok-runtime.md, "Shapes"). Positions are in
/// the body's frame.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    /// Guest address, 8 hex digits.
    pub addr: String,
    /// Its vtable, 8 hex digits: Halo's tag shapes and Havok's own differ
    /// for the same type.
    pub vtable: String,
    /// `capsule`, `sphere`, `transform` or `other`.
    pub kind: String,
    /// A capsule's or sphere's radius.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    /// A capsule's end points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b: Option<[f64; 3]>,
    /// A capsule's whole height: |b − a| + 2 × radius.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// A transform's translation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<[f64; 3]>,
    /// A transform's child shape: its address, decoded where it reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<Box<Shape>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bodies {
    /// Bodies in the world, before `near` and `limit`.
    pub total: usize,
    pub bodies: Vec<Body>,
    /// More matched than `limit` allowed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub more: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContactsParams {
    /// Only contacts this object's bodies are in, that object as `a`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
}

/// One side of a contact.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Side {
    pub entity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datum: Option<String>,
    /// In the fixed island: the level (no datum) or a static body.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fixed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub pos: [f64; 3],
    /// Havok's separating normal, pointing from `b` toward `a` (inferred);
    /// flipped with the sides when `object` put that object first.
    pub normal: [f64; 3],
    /// Separation along the normal; negative is penetration.
    pub distance: f64,
    /// The impulse the solver applied along the normal on its last step
    /// (mass × velocity change).
    pub impulse: f64,
    pub friction: f64,
    pub restitution: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Contact {
    pub a: Side,
    pub b: Side,
    pub points: Vec<Point>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Contacts {
    pub contacts: Vec<Contact>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentsParams {
    pub object: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Agents {
    pub object: String,
    pub agents: Vec<CollisionAgent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollisionAgent {
    pub entry: String,
    pub island: String,
    pub collidables: [String; 2],
    /// `a` or `b` in dispatch order. `entity` belongs to the requested object.
    pub object_side: String,
    pub entity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other: Option<Side>,
    pub pair_word: String,
    pub cache: AgentCache,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tree: Option<TreeCache>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentCache {
    pub base: String,
    pub manager: String,
    pub vtable: String,
    pub kind: String,
    /// The complete inline allocation, including its header, in guest byte order.
    pub hex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub convex: Option<ConvexCache>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConvexCache {
    pub vertices_a: Vec<u16>,
    pub vertices_b: Vec<u16>,
    pub contact_ids: Vec<u16>,
    pub separating_axis: Option<[f32; 3]>,
    pub separating_distance: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swept_sides: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreeCache {
    pub min: Option<[f32; 3]>,
    pub max: Option<[f32; 3]>,
    pub children: Vec<ChildCache>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChildCache {
    pub entry: String,
    pub key: String,
    pub word_04: String,
    /// BSP collision surface index for a type-1 key of the structure MOPP.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<u16>,
    pub cache: AgentCache,
}
