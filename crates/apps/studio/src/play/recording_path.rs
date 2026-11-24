//! The path a played recording's unit takes (docs/editor/rich-editors.md,
//! "Recordings", slice K2). A recording holds control input, not
//! positions, so the editor draws no path of its own: after
//! `recording.play` it keeps the position of that player's unit from each
//! sample the live mirror streams, and the Scene viewport draws them as a
//! line ([`crate::viewport::mirror::Scene`]).

use live_mirror::Sample;

use crate::viewport::mirror::WorldPoses;

/// A path longer than this stops growing (about 11 minutes of ticks at
/// the stream's default rate of one in two).
pub const MAX_POINTS: usize = 20_000;

/// The unit of one player, followed from a start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecordingPath {
    /// The recording being followed; `None` with no path started.
    pub recording: Option<String>,
    /// The player whose unit it is (the stream's player index).
    pub player: u16,
    /// World positions, one per streamed tick.
    pub points: Vec<[f32; 3]>,
    /// The tick of the last point, so a repeated sample adds nothing.
    last_tick: Option<u64>,
    /// The epoch the path began in: a new one (a reload, a map change)
    /// ends it.
    epoch: Option<u64>,
}

impl RecordingPath {
    /// Begin a path for `recording` on `player`'s unit, from the next
    /// sample. A path already there is dropped.
    pub fn start(&mut self, recording: &str, player: u16) {
        *self = Self {
            recording: Some(recording.to_owned()),
            player,
            ..Self::default()
        };
    }

    /// Drop the path.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Add the followed unit's position in `sample`, when it is a new
    /// tick of the same epoch. A path whose epoch changes is dropped.
    pub fn record(&mut self, sample: &Sample) {
        if self.recording.is_none() {
            return;
        }
        match self.epoch {
            None => self.epoch = Some(sample.epoch),
            Some(e) if e != sample.epoch => {
                let (recording, player) = (self.recording.take(), self.player);
                self.clear();
                self.recording = recording;
                self.player = player;
                self.epoch = Some(sample.epoch);
            }
            Some(_) => {}
        }
        if self.points.len() >= MAX_POINTS || self.last_tick.is_some_and(|t| sample.tick <= t) {
            return;
        }
        let Some(unit) = sample
            .players
            .iter()
            .find(|p| p.index == self.player)
            .and_then(|p| p.unit)
            .and_then(|id| sample.objects.iter().find(|o| o.id == id))
        else {
            return;
        };
        if let Some(at) = WorldPoses::unit_position(sample, unit) {
            self.points.push(at);
            self.last_tick = Some(sample.tick);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::live::{Object, Player};

    fn sample(tick: u64, epoch: u64, at: [f32; 3]) -> Sample {
        Sample {
            tick,
            epoch,
            stale: false,
            objects: vec![Object {
                id: 7,
                position: at,
                ..Default::default()
            }],
            players: vec![Player {
                index: 1,
                unit: Some(7),
                ..Default::default()
            }],
            cameras: Vec::new(),
        }
    }

    #[test]
    fn a_followed_unit_leaves_one_point_per_new_tick_and_nothing_before_it_starts() {
        let mut p = RecordingPath::default();
        p.record(&sample(1, 1, [0.0; 3]));
        assert!(p.points.is_empty());
        p.start("arrive", 1);
        p.record(&sample(2, 1, [1.0, 0.0, 0.0]));
        p.record(&sample(2, 1, [9.0, 0.0, 0.0]));
        p.record(&sample(3, 1, [2.0, 0.0, 0.0]));
        assert_eq!(p.points, [[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]);
    }

    #[test]
    fn another_players_unit_and_a_new_epoch_do_not_extend_the_path() {
        let mut p = RecordingPath::default();
        p.start("arrive", 0);
        p.record(&sample(2, 1, [1.0, 0.0, 0.0]));
        assert!(p.points.is_empty(), "player 0 has no unit in the sample");
        p.start("arrive", 1);
        p.record(&sample(2, 1, [1.0, 0.0, 0.0]));
        p.record(&sample(3, 2, [5.0, 0.0, 0.0]));
        assert_eq!(p.points, [[5.0, 0.0, 0.0]]);
        assert_eq!(p.recording.as_deref(), Some("arrive"));
    }
}
