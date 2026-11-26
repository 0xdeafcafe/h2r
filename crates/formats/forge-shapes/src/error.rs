use crate::def::Shape;
use crate::units::{MAX, MIN};

/// Why a definition is refused. Each message names what is wrong.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("size {axis} of {value} units is outside {} to {}", MIN, MAX)]
    SizeRange { axis: char, value: u16 },
    #[error("{shape} takes a param of {allowed}, not {param}")]
    ParamRange {
        shape: Shape,
        param: u16,
        allowed: &'static str,
    },
    #[error("a map material must name a shad tag, not group {group:?}")]
    MapNotShader { group: String },
    #[error("a map material names no tag path")]
    EmptyPath,
    #[error("an arch {size_x} units wide must be at least {min_z} units tall")]
    ArchTooLow { size_x: u16, min_z: u16 },
    #[error("{shape} made no rock: {why}")]
    NoRock { shape: Shape, why: &'static str },
    #[error("piece name {name:?} is not a name: {why}")]
    BadName { name: String, why: &'static str },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
