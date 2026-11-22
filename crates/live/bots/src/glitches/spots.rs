//! Super bounce spots: where on which map a landing can come down on a
//! seam that throws the player up (docs/game/superbounce.md).
//!
//! Only measured spots go here: a place a game run bounced from, with the
//! trace that shows it. Community guides describe spots by landmarks
//! ("crouch under the hatch door near the sniper tower"), not
//! coordinates; turning one into an entry needs a game run that finds
//! the patch and traces bounces on it (the agent API's `world.object
//! player.N physics`, `havok.contacts` and `trace.start`). Coordinates are
//! the game's world units, a unit's origin, Z up.

use crate::brain::Vec3;

/// One spot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bounce {
    /// The map's short name, as its `.map` file is named (`ascension`).
    pub map: &'static str,
    /// What it is, for the log and the docs.
    pub name: &'static str,
    /// Where to stand and jump: the unit's origin standing on the patch.
    pub stand: Vec3,
    /// Between tries the bot stands up to this far off `stand` in X and Y
    /// (world units): not every landing bounces, and the patch is small.
    pub spread: f32,
    /// A world direction (unit length, or zero for none) to come in
    /// along and to drift along while in the air: the bot walks to a
    /// point [`APPROACH`] behind `stand` and steps onto it this way.
    pub drift: [f32; 2],
    /// Where the spot comes from.
    pub source: &'static str,
    /// Where a running jump onto the spot is taken from, when the spot
    /// needs a fall a standing jump doesn't give: the recipe's takeoff,
    /// used as the drop's launch point (`bounce_route::launch_from`).
    /// `None`: a standing jump, or a launch floor found on the nav mesh.
    pub takeoff: Option<Vec3>,
    /// Where the measured landings that bounced came down, when they were
    /// traced: the point a route aims at ([`crate::spots::Site::target`]).
    /// `None`: `stand`.
    pub landing: Option<Vec3>,
}

/// Every measured spot.
pub static SPOTS: &[Bounce] = &[Bounce {
    map: "ascension",
    name: "the crease where the ramp meets the floor",
    // The patch is x about -10.62, y 3.9 to 4.1, z -3.19 standing; the
    // launch is a pinch between a face at y 4.12 looking -y and one at
    // y 3.94 looking +y. It stands just short of the crease (lower y): a
    // landing drifting slowly toward +y comes down in it.
    stand: [-10.62, 3.88, -3.19],
    spread: 0.04,
    // Every traced bounce came in moving slowly toward +y (0.27 to 1.29
    // units a second); the "caught" landings rest on the ramp side (-y).
    drift: [0.0, 1.0],
    source: "docs/game/superbounce.md, Traces and \"The forced crouch leaves state\": every \
             traced bounce fell onto it at -5.26 to -5.40 units/s, from the recipe's running \
             jump off the upper level; a standing jump lands at -3 and never bounces",
    // The recipe's step 3: a running jump from about (-8.4, 2.5, -0.76),
    // heading (-0.8, 0.6) at about 1.6 units/s, a 2.43 drop onto the patch.
    takeoff: Some([-8.4, 2.5, -0.76]),
    // The recipe's step 4: the bounces landed on x -10.2 to -10.7, y 3.93
    // to 4.04. The middle, which the takeoff heads at along the recipe's
    // (-0.8, 0.6); the stand is 0.1 short of the crease in y.
    landing: Some([-10.45, 3.985, -3.19]),
}];

/// How far behind a spot (against its drift) the walk there ends.
pub const APPROACH: f32 = 0.8;

impl Bounce {
    /// Where the walk to the spot ends: [`APPROACH`] behind it, or the
    /// thinking brain's bounce tuning's `approach`
    /// ([`crate::tuning::bounce`]).
    pub fn from(&self) -> Vec3 {
        let back = crate::tuning::bounce().approach;
        [
            self.stand[0] - self.drift[0] * back,
            self.stand[1] - self.drift[1] * back,
            self.stand[2],
        ]
    }
}

/// The spots on `map` (its short name, any case).
pub fn bounces(map: &str) -> Vec<Bounce> {
    SPOTS
        .iter()
        .filter(|s| !map.is_empty() && s.map.eq_ignore_ascii_case(map))
        .copied()
        .collect()
}
