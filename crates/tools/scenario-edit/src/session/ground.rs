//! Ground labels in the session (docs/game/game-types-without-carriers.md,
//! "In Studio"): placing one, and showing the pathfinding of AI ground in
//! the Navigation layers.
use super::*;
use crate::nav_view::{LevelPathfinding, NavPick};
use crate::scenario::ground::{FlagLabel, LABEL_FLAG_TYPE, LabelKind};

impl Session {
    /// Place a ground label as one undo step: a netgame flag at `at` of
    /// the kind's own type, facing `facing` (radians), for `team` (0 red,
    /// 1 blue; only a base and a siege line read it) and `phase` (a siege
    /// line's), with its label. The window's **Place new with the mouse**
    /// and `scenario.add {block: "label"}` both come here. Selects the flag
    /// and returns its index.
    pub fn add_label_at(
        &mut self,
        kind: LabelKind,
        at: Vec3,
        facing: f32,
        team: u8,
        phase: u16,
    ) -> Result<usize> {
        let flags = BlockKind::NetgameFlags;
        if team >= 2 {
            return Err(Error::Declined(
                "a label's team is 0 (red) or 1 (blue)".into(),
            ));
        }
        if !facing.is_finite() || !at.iter().all(|x| x.is_finite()) {
            return Err(Error::Declined(
                "a label's place and facing are numbers".into(),
            ));
        }
        if let Some(why) = self.add_refusal(flags) {
            return Err(Error::Refused(why));
        }
        let s = self.scenario().ok_or(Error::Save("no map is open"))?;
        let entry = s.new_entry(flags, None, Some(at))?;
        let index = s.block(flags).len();
        let team = if kind.has_team() { team } else { 0 };
        let phase = if kind.has_phase() { phase } else { 0 };
        let command = Command {
            label: format!("place {}", kind.text()),
            ops: vec![
                crate::edit::Op::Add {
                    block: flags,
                    entry,
                },
                set_op(flags, index, "type", Value::Int(LABEL_FLAG_TYPE)),
                set_op(flags, index, "facing", Value::Float(facing)),
                set_op(flags, index, "team", Value::Int(i64::from(team))),
                crate::edit::Op::Label {
                    index,
                    label: Some(FlagLabel { kind, phase }),
                },
            ],
            merge: None,
        };
        self.apply(command)?;
        self.select(Some((flags, index)));
        Ok(index)
    }

    /// Show `ground`'s pathfinding in the Navigation layers in place of
    /// the map's own, or the map's again with `None`. A selected sector is
    /// cleared.
    pub fn set_ground_pathfinding(&mut self, ground: Option<LevelPathfinding>) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        if doc.ground_pathfinding == ground {
            return;
        }
        doc.ground_pathfinding = ground;
        if matches!(self.nav, Some(NavPick::Sector(_))) {
            self.nav = None;
        }
        self.markers_rev += 1;
    }

    /// Whether the Navigation layers show AI ground's pathfinding.
    pub fn shows_ground_pathfinding(&self) -> bool {
        self.doc
            .as_ref()
            .is_some_and(|d| d.ground_pathfinding.is_some())
    }
}
