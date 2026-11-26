//! The ground file of a `map` item (docs/editor/studio-redesign.md,
//! "Contents": `map`; docs/game/game-types-without-carriers.md, "In
//! Studio"): the AI ground labels a map is made with, as YAML.
//!
//! ```yaml
//! home: red
//! labels:
//!   - {label: ff_drop, position: [1.0, 2.0, 0.5], facing: 1.5}
//!   - {label: ff_base, position: [0.0, 0.0, 0.0], team: red}
//!   - {label: siege_line, position: [4.0, 4.0, 0.0], team: blue, phase: 1}
//! ```
//!
//! The file holds what an author placed, never what the build makes. This
//! crate only reads and writes it ([`GroundFile`]); the generator is above
//! it, so the build is the caller's ([`crate::Fresh::ground`]).
use serde::{Deserialize, Serialize};

/// A team of the ground: red or blue.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    #[default]
    Red,
    Blue,
}

/// The label names the ground reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelName {
    FfDrop,
    FfBase,
    FfCover,
    SiegeSite,
    SiegeGoal,
    SiegeLine,
}

impl LabelName {
    pub const fn text(self) -> &'static str {
        match self {
            Self::FfDrop => "ff_drop",
            Self::FfBase => "ff_base",
            Self::FfCover => "ff_cover",
            Self::SiegeSite => "siege_site",
            Self::SiegeGoal => "siege_goal",
            Self::SiegeLine => "siege_line",
        }
    }

    /// Whether the label has a facing (a yaw in radians).
    pub const fn has_facing(self) -> bool {
        matches!(self, Self::FfDrop | Self::FfCover)
    }

    /// Whether the label belongs to a team.
    pub const fn has_team(self) -> bool {
        matches!(self, Self::FfBase | Self::SiegeLine)
    }

    /// Whether the label has a phase.
    pub const fn has_phase(self) -> bool {
        matches!(self, Self::SiegeLine)
    }
}

/// One label: a place, and what its kind reads of it.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroundLabel {
    pub label: LabelName,
    /// In map units.
    pub position: [f32; 3],
    /// A drop's and a cover's yaw, in radians.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facing: Option<f32>,
    /// A base's and a siege line's team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<Side>,
    /// A siege line's phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<u16>,
}

/// A `map` item's ground file.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroundFile {
    /// The team whose base is home.
    #[serde(default)]
    pub home: Side,
    #[serde(default)]
    pub labels: Vec<GroundLabel>,
}

impl GroundFile {
    /// Read and check a ground file.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes).map_err(|_| "the ground file is not UTF-8")?;
        let file: Self =
            serde_yaml_ng::from_str(text).map_err(|e| format!("the ground file: {e}"))?;
        file.validate()?;
        Ok(file)
    }

    /// The file as YAML: what [`GroundFile::parse`] reads back.
    pub fn to_yaml(&self) -> Result<String, String> {
        serde_yaml_ng::to_string(self).map_err(|e| format!("the ground file: {e}"))
    }

    /// Each label has a finite place and exactly the parts its kind reads.
    pub fn validate(&self) -> Result<(), String> {
        for (n, l) in self.labels.iter().enumerate() {
            let at = |what: &str| format!("the ground file: label {n} ({}) {what}", l.label.text());
            if !l.position.iter().all(|x| x.is_finite()) {
                return Err(at("has a position that is not a number"));
            }
            if l.facing.is_some_and(|f| !f.is_finite()) {
                return Err(at("has a facing that is not a number"));
            }
            let parts = [
                ("facing", l.facing.is_some(), l.label.has_facing(), false),
                ("team", l.team.is_some(), l.label.has_team(), true),
                ("phase", l.phase.is_some(), l.label.has_phase(), true),
            ];
            for (what, given, takes, required) in parts {
                if given && !takes {
                    return Err(at(&format!("has a {what}, which its kind does not read")));
                }
                if !given && takes && required {
                    return Err(at(&format!("needs a {what}")));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(kind: LabelName) -> GroundLabel {
        GroundLabel {
            label: kind,
            position: [1.0, 2.0, 0.5],
            facing: kind.has_facing().then_some(1.5),
            team: kind.has_team().then_some(Side::Blue),
            phase: kind.has_phase().then_some(2),
        }
    }

    #[test]
    fn a_file_reads_back_as_written() {
        let file = GroundFile {
            home: Side::Blue,
            labels: [
                LabelName::FfDrop,
                LabelName::FfBase,
                LabelName::FfCover,
                LabelName::SiegeSite,
                LabelName::SiegeGoal,
                LabelName::SiegeLine,
            ]
            .map(label)
            .to_vec(),
        };
        let yaml = file.to_yaml().unwrap();
        assert_eq!(GroundFile::parse(yaml.as_bytes()).unwrap(), file);
        // The label names are the ones the game and Forge use.
        for kind in ["ff_drop", "ff_base", "ff_cover", "siege_site", "siege_goal", "siege_line"] {
            assert!(yaml.contains(kind), "{kind} in {yaml}");
        }
    }

    #[test]
    fn an_empty_file_is_no_labels_and_red_home() {
        assert_eq!(GroundFile::parse(b"{}").unwrap(), GroundFile::default());
        assert_eq!(GroundFile::default().home, Side::Red);
    }

    #[test]
    fn a_label_holds_exactly_the_parts_its_kind_reads() {
        let with = |f: fn(&mut GroundLabel)| {
            let mut l = label(LabelName::SiegeLine);
            f(&mut l);
            GroundFile {
                labels: vec![l],
                ..GroundFile::default()
            }
            .validate()
        };
        assert!(with(|_| {}).is_ok());
        assert!(with(|l| l.team = None).unwrap_err().contains("needs a team"));
        assert!(with(|l| l.phase = None).unwrap_err().contains("needs a phase"));
        assert!(with(|l| l.facing = Some(0.0)).unwrap_err().contains("facing"));
        assert!(with(|l| l.position[1] = f32::NAN).unwrap_err().contains("not a number"));
        let mut drop = label(LabelName::FfDrop);
        drop.team = Some(Side::Red);
        let file = GroundFile {
            labels: vec![drop],
            ..GroundFile::default()
        };
        assert!(file.validate().unwrap_err().contains("team"));
    }

    #[test]
    fn a_file_with_an_unknown_part_is_refused() {
        assert!(GroundFile::parse(b"colour: red").is_err());
        assert!(GroundFile::parse(b"labels: [{label: ff_drop, position: [0, 0]}]").is_err());
    }
}
