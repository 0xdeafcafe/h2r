//! Source geometry for synthesising transient fire and hit effects. Coordinates
//! use the engine's world units, velocities world units per second. Datum fields
//! include the salt, so a browser can associate them with the pose dictionary.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Barrel {
    pub weapon: u32,
    pub definition: Option<u32>,
    pub index: u32,
    pub unit: Option<u32>,
    /// Shooter's body position at fire time. A mounted body uses its parent's
    /// space. This is the unit origin, not a model-specific muzzle marker.
    pub parent: Option<u32>,
    pub origin: Option<[f32; 3]>,
    /// Unit's world-space aim before the engine applies spread and assistance.
    pub aim: Option<[f32; 3]>,
}

/// The geometry of an applied damage effect. For a radial blast many targets
/// can name the same origin and effect. A viewer can group those sparks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Impact {
    pub target: u32,
    pub source: Option<u32>,
    pub effect: Option<u32>,
    pub code: u8,
    /// The damage report's world-space origin. May be a blast centre.
    pub origin: [f32; 3],
    pub direction: Option<[f32; 3]>,
    pub distance: Option<f32>,
}

/// Last position still held by a projectile before the engine frees its
/// object. Includes first-tick bullets invisible to the lower-rate pose scan.
/// Deletion also happens at maximum range and during clean-up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Endpoint {
    pub projectile: u32,
    pub definition: Option<u32>,
    pub parent: Option<u32>,
    /// Object position. Parent-relative when the projectile is stuck.
    pub position: [f32; 3],
    pub velocity: Option<[f32; 3]>,
}
