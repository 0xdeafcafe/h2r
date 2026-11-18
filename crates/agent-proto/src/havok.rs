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

    pub const ALL: &[&str] = &[BODIES, CONTACTS];
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
