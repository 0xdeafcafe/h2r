//! Prefabs: a small file of tag keys and frames relative to a pivot.
//!
//! A prefab is the pieces of a build a player saved to place again
//! (docs/game/forge-editing.md, "Selection, groups and prefabs"). It names
//! tags by key and holds nothing a [`ForgeDoc`](crate::ForgeDoc) could not:
//! no game bytes, no index into a map. Placing one writes ordinary
//! placement, label and group ops, so a document that holds a placed
//! prefab has no trace of the file and the file has its own number,
//! [`PREFAB_FORMAT`].
//!
//! Each piece's frame is relative to the pivot, the first piece as saved:
//! its position is turned back by the pivot's yaw, and its yaw is the
//! difference of the two yaws. Placing turns the offsets by the yaw the
//! prefab is put down at, so a prefab is the same shape facing any way.
//! Pitch and roll are the piece's own.
use serde::{Deserialize, Serialize};

use crate::{
    BlockKind, F32, Label, MAX_GROUP_MEMBERS, Refusal, Result, TagKey, groups::group_name, wire,
};

/// The number a prefab file carries.
pub const PREFAB_FORMAT: u32 = 1;
/// The most bytes a prefab file takes.
pub const MAX_PREFAB_BYTES: usize = 128 * 1024;
/// The most pieces one prefab holds: as many as a group.
pub const MAX_PREFAB_PIECES: usize = MAX_GROUP_MEMBERS;
/// The farthest a piece stands from the pivot, in world units.
pub const MAX_PREFAB_REACH: f32 = 4096.;
/// The most bytes a tag key's group or path takes.
pub const MAX_PREFAB_TAG: usize = 256;

/// One piece of a prefab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Piece {
    /// The placement block the object is added to.
    #[serde(with = "wire::block")]
    pub block: BlockKind,
    pub tag: TagKey,
    /// Where it stands from the pivot, as if the pivot faced yaw 0.
    pub position: [F32; 3],
    /// Yaw from the pivot's yaw, then pitch and roll, in radians.
    pub rotation: [F32; 3],
    /// The piece's label, which placing sets on the object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<Label>,
}

/// A prefab file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prefab {
    pub format: u32,
    /// The file's stem: 1 to 64 letters, digits, dashes and underscores.
    pub name: String,
    /// Placing makes one group of the pieces, named as the prefab is.
    #[serde(default, skip_serializing_if = "is_false")]
    pub grouped: bool,
    #[serde(deserialize_with = "wire::bounded::<_, _, MAX_PREFAB_PIECES>")]
    pub pieces: Vec<Piece>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A frame as the session holds one: a position, then yaw, pitch and roll.
pub type Pose = ([f32; 3], [f32; 3]);

/// An object in the world, as a prefab is captured from it.
#[derive(Clone, Debug, PartialEq)]
pub struct Captured {
    pub block: BlockKind,
    pub tag: TagKey,
    pub pose: Pose,
    pub label: Option<Label>,
}

fn turn(d: [f32; 3], yaw: f32) -> [f32; 3] {
    let (s, c) = yaw.sin_cos();
    [c * d[0] - s * d[1], s * d[0] + c * d[1], d[2]]
}

fn floats(v: [f32; 3]) -> Result<[F32; 3]> {
    Ok([F32::new(v[0])?, F32::new(v[1])?, F32::new(v[2])?])
}

fn get(v: [F32; 3]) -> [f32; 3] {
    v.map(F32::get)
}

impl Piece {
    /// Where the piece stands when the prefab is put down at `at` (a
    /// position and a pose whose first angle is the yaw it faces).
    pub fn pose(&self, at: Pose) -> Pose {
        let turned = turn(get(self.position), at.1[0]);
        let rotation = get(self.rotation);
        (
            std::array::from_fn(|i| at.0[i] + turned[i]),
            [
                (at.1[0] + rotation[0]).rem_euclid(std::f32::consts::TAU),
                rotation[1],
                rotation[2],
            ],
        )
    }
}

impl Prefab {
    /// A prefab of `pieces` given as they stand in the world, the first
    /// being the pivot.
    pub fn capture(name: &str, grouped: bool, pieces: &[Captured]) -> Result<Self> {
        let Some(Captured { pose: pivot, .. }) = pieces.first() else {
            return Err(Refusal::invalid("prefab", "a prefab has a piece"));
        };
        let out = pieces
            .iter()
            .map(
                |Captured {
                     block,
                     tag,
                     pose,
                     label,
                 }| {
                    let d: [f32; 3] = std::array::from_fn(|i| pose.0[i] - pivot.0[i]);
                    let d = turn(d, -pivot.1[0]);
                    Ok(Piece {
                        block: *block,
                        tag: tag.clone(),
                        position: floats(d)?,
                        rotation: floats([
                            (pose.1[0] - pivot.1[0]).rem_euclid(std::f32::consts::TAU),
                            pose.1[1],
                            pose.1[2],
                        ])?,
                        label: label.clone(),
                    })
                },
            )
            .collect::<Result<Vec<_>>>()?;
        let prefab = Self {
            format: PREFAB_FORMAT,
            name: name.into(),
            grouped,
            pieces: out,
        };
        prefab.check()?;
        Ok(prefab)
    }

    /// Where each piece stands when the prefab is put down at `at`.
    pub fn poses(&self, at: Pose) -> Vec<Pose> {
        self.pieces.iter().map(|p| p.pose(at)).collect()
    }

    /// The first piece's tag that `has` does not know, by key: the map a
    /// prefab is placed on must hold every tag.
    pub fn missing(&self, has: impl Fn(BlockKind, &TagKey) -> bool) -> Option<&TagKey> {
        self.pieces
            .iter()
            .find(|p| !has(p.block, &p.tag))
            .map(|p| &p.tag)
    }

    /// Refuse a prefab that is not one a build could write.
    pub fn check(&self) -> Result<()> {
        if self.format != PREFAB_FORMAT {
            return Err(Refusal::invalid(
                "prefab",
                format!(
                    "format {} is unsupported (this build reads {PREFAB_FORMAT})",
                    self.format
                ),
            ));
        }
        if !content::manifest::is_map_name(&self.name) {
            return Err(Refusal::invalid(
                "prefab",
                "name wants 1 to 64 letters, digits, dashes or underscores",
            ));
        }
        if self.pieces.is_empty() || self.pieces.len() > MAX_PREFAB_PIECES {
            return Err(Refusal::invalid(
                "prefab",
                format!("want 1 to {MAX_PREFAB_PIECES} pieces"),
            ));
        }
        if self.grouped {
            group_name(&self.name)?;
            if self.pieces.len() < crate::MIN_GROUP_MEMBERS {
                return Err(Refusal::invalid("prefab", "a group has two or more pieces"));
            }
        }
        for piece in &self.pieces {
            for part in [&piece.tag.group, &piece.tag.path] {
                if part.is_empty()
                    || part.len() > MAX_PREFAB_TAG
                    || part.chars().any(char::is_control)
                {
                    return Err(Refusal::invalid("prefab", "a piece has a bad tag key"));
                }
            }
            if get(piece.position)
                .iter()
                .any(|v| v.abs() > MAX_PREFAB_REACH)
            {
                return Err(Refusal::invalid(
                    "prefab",
                    format!("a piece stands more than {MAX_PREFAB_REACH} units from the pivot"),
                ));
            }
            if let Some(label) = &piece.label {
                label.check()?;
            }
        }
        Ok(())
    }

    /// Read a prefab file: bounded, then checked.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        wire::check_size(bytes.len(), "prefab bytes", MAX_PREFAB_BYTES)?;
        let prefab: Self = serde_json::from_slice(bytes).map_err(wire::json_error)?;
        prefab.check()?;
        Ok(prefab)
    }

    /// The canonical file form.
    pub fn canonical_json(&self) -> Result<String> {
        self.check()?;
        wire::canonical(self, "prefab bytes", MAX_PREFAB_BYTES)
    }
}
