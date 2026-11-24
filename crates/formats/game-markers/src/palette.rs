//! Marker colours: a team's, or a kind's own.

/// How a kind is coloured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tint {
    /// The placement's team colour; the neutral colour without a team.
    Team,
    /// The team colour when the placement has a team, else this one.
    TeamOr([f32; 3]),
    /// Always this colour.
    Fixed([f32; 3]),
}

/// The colours a drawer gives the markers. The game fills `teams` with
/// the loaded map's own team colours; a tool with no map draws the
/// stand-ins of [`Palette::default`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Teams 0 to 7: red, blue, yellow, green, purple, orange, brown,
    /// pink.
    pub teams: [[f32; 3]; 8],
    pub neutral: [f32; 3],
}

/// Stand-ins of ours for the eight teams, apart enough to tell.
pub const TEAMS: [[f32; 3]; 8] = [
    [0.90, 0.22, 0.20],
    [0.22, 0.45, 0.95],
    [0.95, 0.85, 0.20],
    [0.25, 0.80, 0.30],
    [0.62, 0.35, 0.90],
    [0.98, 0.55, 0.12],
    [0.55, 0.36, 0.22],
    [0.98, 0.55, 0.75],
];
/// No team: a pale blue-white.
pub const NEUTRAL: [f32; 3] = [0.88, 0.92, 0.96];

/// A teleporter's sending end.
pub const SENDER: [f32; 3] = [1.0, 0.62, 0.15];
/// A teleporter's receiving end.
pub const RECEIVER: [f32; 3] = [0.25, 0.85, 1.0];
pub const WEAPON: [f32; 3] = [1.0, 0.6, 0.1];
pub const VEHICLE: [f32; 3] = [0.2, 0.5, 1.0];
pub const EQUIPMENT: [f32; 3] = [0.2, 0.9, 0.9];
pub const TRIGGER: [f32; 3] = [0.95, 0.85, 0.2];
pub const KILL: [f32; 3] = [1.0, 0.2, 0.2];
/// A soft boundary: a return-to-battlefield edge.
pub const SOFT: [f32; 3] = [1.0, 0.55, 0.15];
pub const BSP_SWITCH: [f32; 3] = [0.4, 0.6, 1.0];
/// Where a wave arrives.
pub const DROP: [f32; 3] = [0.24, 0.86, 0.35];
pub const COVER: [f32; 3] = [0.92, 0.8, 0.24];
/// The siege labels.
pub const SIEGE: [f32; 3] = [0.75, 0.47, 0.96];

impl Default for Palette {
    fn default() -> Self {
        Self {
            teams: TEAMS,
            neutral: NEUTRAL,
        }
    }
}

impl Palette {
    /// The colour of a marker tinted `tint` for `team`. A team past 7 is
    /// no team.
    pub fn colour(&self, tint: Tint, team: Option<u8>) -> [f32; 3] {
        let team = team.and_then(|t| self.teams.get(usize::from(t)).copied());
        match tint {
            Tint::Team => team.unwrap_or(self.neutral),
            Tint::TeamOr(own) => team.unwrap_or(own),
            Tint::Fixed(own) => own,
        }
    }
}
