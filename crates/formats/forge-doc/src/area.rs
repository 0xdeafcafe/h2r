//! Areas on netgame flags (docs/game/gametype-markers.md, "In the document").
use game_markers::Shape;
use serde::{Deserialize, Serialize};

pub use game_markers::Area as MarkerArea;

use crate::{BlockKind, F32, Refusal, Result};

/// The document spelling of a marker's area. All distances and offsets use
/// the same bit-exact floats as placement fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Area {
    Radius {
        radius: F32,
        below: F32,
        above: F32,
    },
    Path {
        points: Vec<[F32; 2]>,
        below: F32,
        above: F32,
    },
}

impl Area {
    /// Decode the shape and heights that the marker and the zone use.
    pub fn marker_area(&self) -> MarkerArea {
        let (shape, below, above) = match self {
            Self::Radius { radius, below, above } => (Shape::Radius(radius.get()), below, above),
            Self::Path { points, below, above } => (
                Shape::Path(points.iter().map(|point| point.map(F32::get)).collect()),
                below,
                above,
            ),
        };
        MarkerArea {
            shape,
            below: below.get(),
            above: above.get(),
        }
    }

    pub(crate) fn check(&self) -> Result<()> {
        self.marker_area().validate().map_err(Refusal::area)
    }
}

impl TryFrom<&MarkerArea> for Area {
    type Error = Refusal;

    fn try_from(area: &MarkerArea) -> Result<Self> {
        area.validate().map_err(Refusal::area)?;
        let below = F32::new(area.below)?;
        let above = F32::new(area.above)?;
        Ok(match &area.shape {
            Shape::Radius(radius) => Self::Radius {
                radius: F32::new(*radius)?,
                below,
                above,
            },
            Shape::Path(points) => Self::Path {
                points: points
                    .iter()
                    .map(|&[x, y]| Ok([F32::new(x)?, F32::new(y)?]))
                    .collect::<Result<_>>()?,
                below,
                above,
            },
        })
    }
}

pub(crate) fn check_block(block: BlockKind) -> Result<()> {
    if block == BlockKind::NetgameFlags {
        Ok(())
    } else {
        Err(Refusal::invalid("entry area", "an area belongs to a netgame flag"))
    }
}
