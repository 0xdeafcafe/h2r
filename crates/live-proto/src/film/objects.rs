//! Presentation observations at one simulation boundary, before quantisation.

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Scene {
    pub objects: Vec<ObjectPose>,
    pub hill: Option<HillPose>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HillPose {
    pub index: u8,
    pub corners: Vec<[f64; 3]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjectiveKind {
    Flag = 1,
    Ball = 3,
    Bomb = 9,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimPose {
    pub graph: u32,
    pub inherited: i16,
    pub index: i16,
    pub frame: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectPose {
    /// Salted handle. A reused slot belongs to a new object.
    pub datum: u32,
    /// The type byte from the engine object header.
    pub kind: u8,
    pub definition: Option<u32>,
    pub name: Option<String>,
    pub placement: Option<u16>,
    pub spawn: Option<u8>,
    pub parent: Option<u32>,
    /// Guest world units. Attached objects retain the engine's parent space.
    pub pos: [f64; 3],
    pub forward: [f64; 3],
    pub up: [f64; 3],
    pub seat: Option<u16>,
    pub aim: Option<[f64; 3]>,
    pub animation: Option<AnimPose>,
    /// Normalised device travel.
    pub travel: Option<f64>,
    pub depleted: bool,
    /// Salted controlling player handle, when present.
    pub player: Option<u32>,
    pub objective: Option<ObjectiveKind>,
}
