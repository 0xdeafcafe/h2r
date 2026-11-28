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
    /// The ordered heap walk ([`super::DigestParams`] → [`super::Digest`]): the whole Havok
    /// world's persistent state as one hash, for parity checks.
    pub const DIGEST: &str = "havok.digest";
    /// Overwrite one collision pair's word ([`super::PairSetParams`] →
    /// [`super::PairSet`]). Needs the claim and a session that allows host writes.
    pub const PAIR_SET: &str = "havok.pair.set";
    /// The island solve's and the island integrate's counters
    /// ([`super::IslandsParams`] → [`super::Islands`]): islands answered
    /// natively and islands sent to the guest, by reason.
    pub const ISLANDS: &str = "havok.islands";

    pub const ALL: &[&str] = &[BODIES, CONTACTS, AGENTS, DIGEST, PAIR_SET, ISLANDS];
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

/// `entry` is a pair's `entry` from `havok.agents` for the same object;
/// without it, the object's one pair against a fixed body (the level).
/// `word` is eight hex digits, as `pair_word` prints them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairSetParams {
    pub object: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    pub word: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairSet {
    pub object: String,
    pub entry: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Agents {
    pub object: String,
    pub agents: Vec<CollisionAgent>,
}

/// `havok.digest` takes no parameters; any field is refused.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigestParams {}

/// `havok.digest`: the canonical walk over the live Havok heap
/// (docs/engine/havok-step.md, "The digest"). The arena hash does not cover
/// the Havok heap, so this digest is what proves a physics rewrite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Digest {
    /// The walk's FNV-1a hash over every live byte and address, 16 hex
    /// digits.
    pub digest: String,
    pub bodies: u32,
    pub constraints: u32,
    pub pairs: u32,
    /// Bytes in occupied pool slots.
    pub heap_used: u32,
}

/// `havok.islands` takes no parameters; any field is refused.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IslandsParams {}

/// `havok.islands`: the island solve's counters since the process started
/// (docs/engine/havok-step.md, "The island solve"), and the island
/// integrate arm's ("The island integrate").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Islands {
    /// Islands whose solve ran in Rust: at the solve's entry, or inside an
    /// integrate the arm answered.
    pub solved: u64,
    /// Islands sent to the lifted solve: the sum of `fallbacks`.
    pub declined: u64,
    /// Declines by reason, every reason listed (`memory`, `overflow`,
    /// `motion`, `constraint`, `tables`, `solver`, `float`), in that order.
    pub fallbacks: Vec<IslandFallback>,
    /// The island integrate arm (`physics.island.integrate`).
    pub integrate: TakeoverArm,
    /// The deactivation test arm (`physics.island.deactivation`).
    pub deactivation: TakeoverArm,
    /// The integrate phase arm (`physics.integrate`).
    pub phase: TakeoverArm,
    /// The structure polygon builder arm (`physics.structure.polygon`).
    pub polygon: TakeoverArm,
    /// The convex agent's process call arm (`physics.agent.convex`).
    pub convex: TakeoverArm,
    /// The closest-feature query arm (`physics.agent.query`).
    pub query: TakeoverArm,
    /// The add-point call arm (`physics.agent.point`).
    pub point: TakeoverArm,
    /// The contact commit arm (`physics.contact.commit`).
    pub commit: TakeoverArm,
}

/// An answering arm's calls since the process started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakeoverArm {
    /// Calls the Rust kernels answered.
    pub answered: u64,
    /// Calls the model refused, which ran the lifted entry: the sum of
    /// `fallbacks`.
    pub declined: u64,
    /// Declines by reason, every reason of the arm's model listed in its
    /// order (docs/runtime/agent-api.md, `havok.islands`).
    pub fallbacks: Vec<IslandFallback>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IslandFallback {
    pub reason: String,
    pub count: u64,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn digest_wire_round_trips_and_params_take_no_fields() {
        let wire = json!({
            "digest": "00000000deadbeef",
            "bodies": 3,
            "constraints": 2,
            "pairs": 1,
            "heap_used": 4096,
        });
        let d: Digest = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(d.digest, "00000000deadbeef");
        assert_eq!((d.bodies, d.constraints, d.pairs, d.heap_used), (3, 2, 1, 4096));
        assert_eq!(serde_json::to_value(&d).unwrap(), wire);
        let p: DigestParams = serde_json::from_value(json!({})).unwrap();
        assert_eq!(serde_json::to_value(p).unwrap(), json!({}));
        assert!(serde_json::from_value::<DigestParams>(json!({"strict": true})).is_err());
    }

    #[test]
    fn islands_wire_round_trips_and_params_take_no_fields() {
        let wire = json!({
            "solved": 40,
            "declined": 3,
            "fallbacks": [{"reason": "overflow", "count": 1}, {"reason": "float", "count": 2}],
            "integrate": {
                "answered": 900, "declined": 2,
                "fallbacks": [{"reason": "solve", "count": 2}],
            },
            "deactivation": {
                "answered": 0, "declined": 1,
                "fallbacks": [{"reason": "object", "count": 1}],
            },
            "phase": {
                "answered": 0, "declined": 5,
                "fallbacks": [{"reason": "listener", "count": 5}],
            },
            "polygon": {
                "answered": 70, "declined": 1,
                "fallbacks": [{"reason": "ring", "count": 1}],
            },
            "convex": {
                "answered": 9, "declined": 30,
                "fallbacks": [{"reason": "closest", "count": 30}],
            },
            "query": {
                "answered": 0, "declined": 2,
                "fallbacks": [{"reason": "shape", "count": 2}],
            },
            "point": {
                "answered": 0, "declined": 4,
                "fallbacks": [{"reason": "commit", "count": 4}],
            },
            "commit": {
                "answered": 3, "declined": 1,
                "fallbacks": [{"reason": "walk", "count": 1}],
            },
        });
        let i: Islands = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!((i.solved, i.declined, i.fallbacks.len()), (40, 3, 2));
        assert_eq!((i.integrate.answered, i.integrate.declined), (900, 2));
        assert_eq!((i.deactivation.declined, i.phase.declined, i.polygon.answered, i.convex.declined, i.query.declined, i.point.declined), (1, 5, 70, 30, 2, 4));
        assert_eq!(serde_json::to_value(&i).unwrap(), wire);
        assert!(serde_json::from_value::<IslandsParams>(json!({})).is_ok());
        assert!(serde_json::from_value::<IslandsParams>(json!({"reset": true})).is_err());
    }
}
