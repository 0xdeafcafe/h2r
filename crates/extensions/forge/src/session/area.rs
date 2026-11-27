//! Centre flags for authored goals (docs/game/gametype-markers.md).
use std::collections::BTreeMap;

use forge_doc::{
    Area, BlockKind, Edit, Endpoint, EntryOp, F32, FieldWrite, Forge, GroundLabel, Label,
    MarkerArea, Op, OpId, Placement, Value,
};

use super::{Session, field, reason, vector};
use crate::{Frame, Thing, World};

/// An objective marker from the Objects menu's Goals group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goal {
    Hill,
    Territory,
    Site,
}

impl Goal {
    pub const ALL: [Self; 3] = [Self::Hill, Self::Territory, Self::Site];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Hill => "Hill",
            Self::Territory => "Territory",
            Self::Site => "Siege site",
        }
    }

    pub fn in_game(self, game: Option<i32>) -> bool {
        let (Some(game), Some(kind)) = (
            game,
            match self {
                Self::Hill => Some(11),
                Self::Territory => Some(10),
                Self::Site => None,
            },
        ) else {
            return true;
        };
        forge_doc::games::flag_games(kind).is_none_or(|games| games.has(game))
    }

    pub fn of_label(text: &str) -> Option<Self> {
        match text {
            "hill_corner" => Some(Self::Hill),
            "territory" => Some(Self::Territory),
            text if text == GroundLabel::Site.text() => Some(Self::Site),
            _ => None,
        }
    }
}

/// The effective label of a flag, including the built-in goals.
pub fn flag_label(thing: &Thing, labels: &BTreeMap<Endpoint, Label>) -> Option<Label> {
    if thing.block != BlockKind::NetgameFlags {
        return None;
    }
    labels.get(&thing.id.identity()).cloned().or_else(|| {
        let Value::Int(kind) = field(thing, "type")? else {
            return None;
        };
        forge_doc::builtin_label(u16::try_from(*kind).ok()?)
    })
}

impl Session {
    pub fn labels(&self) -> Result<BTreeMap<Endpoint, Label>, String> {
        self.doc
            .label_targets(&self.limits.base_counts)
            .map_err(reason)
    }

    pub fn can_place_goals(&self) -> bool {
        self.limits
            .base_counts
            .contains_key(&BlockKind::NetgameFlags)
    }

    /// Place one centre flag and its default radius in one undo step.
    pub fn place_goal(
        &mut self,
        world: &mut impl World,
        tick: u32,
        goal: Goal,
        frame: Frame,
    ) -> Result<OpId, String> {
        self.place_goal_with(world, tick, goal, frame, None)
    }

    /// [`Session::place_goal`] with the team and order (`identifier`) of a
    /// goal it copies.
    pub(super) fn place_goal_with(
        &mut self,
        world: &mut impl World,
        tick: u32,
        goal: Goal,
        frame: Frame,
        copy: Option<(i64, i64)>,
    ) -> Result<OpId, String> {
        // In a kit session the flag is a spare row of the kit.
        if self.has_kit() {
            self.kit_check(BlockKind::NetgameFlags)?;
        }
        let kind = match goal {
            Goal::Hill => {
                let flags = self.things();
                let labels = self.labels()?;
                (11..=18)
                    .find(|&kind| {
                        let label = forge_doc::builtin_label(kind as u16);
                        !flags.iter().any(|thing| {
                            thing.block == BlockKind::NetgameFlags
                                && (field(thing, "type") == Some(&Value::Int(kind))
                                    || flag_label(thing, &labels) == label)
                        })
                    })
                    .ok_or("All eight hills are placed.")?
            }
            Goal::Territory => 10,
            // Script labels use the same carrier flag as the AI palette.
            Goal::Site => 0,
        };
        let mut fields = vec![
            vector("position", frame.position)?,
            FieldWrite {
                field: "facing".into(),
                value: Value::Float(F32::new(frame.rotation[0]).map_err(reason)?),
            },
            FieldWrite {
                field: "type".into(),
                value: Value::Int(kind),
            },
            FieldWrite {
                field: "team".into(),
                value: Value::Int(copy.map_or(8, |(team, _)| team)),
            },
        ];
        if let Some((_, order)) = copy.filter(|(_, order)| *order != 0) {
            fields.push(FieldWrite {
                field: "identifier".into(),
                value: Value::Int(order),
            });
        }
        let add = Edit::Placement(Placement::Add {
            block: BlockKind::NetgameFlags,
            like: None,
            fields,
        });

        // The add's ID is also the area and label target.
        let id = self.ops(vec![add.clone()])?[0].id;
        let target = Endpoint::Added(id);
        let mut edits = vec![add];
        if goal == Goal::Site {
            edits.push(Edit::Forge(Forge::Label {
                target,
                label: Some(Label::new(GroundLabel::Site.text(), None).map_err(reason)?),
            }));
        }
        edits.push(Edit::Entry(EntryOp::Area {
            target,
            area: Some(Area::try_from(&MarkerArea::default()).map_err(reason)?),
        }));
        let ops: Vec<Op> = self.ops(edits)?;
        self.edit(world, tick, ops, None)?;
        Ok(id)
    }
}
