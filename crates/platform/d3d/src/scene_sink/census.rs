//! The side targets and readbacks a game view used over a whole run. Aux
//! numbers are local to a frame, so group by surface description, slot and
//! use (including the copy direction and how the GPU sampled it).
use std::collections::BTreeMap;
use std::sync::Mutex;

use frame_scene::{Carried, GuestOp, Slot, TargetRef, TargetSample};

/// A possibly transient use in the sink's recorded frames, numbered from one.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub struct Uses {
    pub count: u64,
    pub first_frame: u64,
    pub last_frame: u64,
}

impl Uses {
    fn add(&mut self, frame: u64) {
        if self.count == 0 {
            self.first_frame = frame;
        }
        self.last_frame = frame;
        self.count += 1;
    }
}

/// A class of aux surface and the operation which named it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TargetUse {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub depth: bool,
    pub slot: Slot,
    pub operation: &'static str,
}

#[derive(Debug, Default, Clone)]
pub struct Census {
    /// Frames recorded by the enabled sink.
    pub frames: u64,
    /// Frames with at least one D3D op in a game view.
    pub game_frames: u64,
    pub targets: BTreeMap<TargetUse, Uses>,
    /// All reads, including those in the outside and overlay slots.
    pub reads: BTreeMap<(Slot, &'static str), Uses>,
}

impl Census {
    fn record<D>(&mut self, f: &Carried<D>) {
        self.frames += 1;
        let mut game = false;
        for o in &f.ops {
            if !o.at.slot.in_game_view() {
                continue;
            }
            game = true;
            let mut add = |t, operation| {
                let TargetRef::Aux(n) = t else { return };
                let desc = f.aux.get(usize::from(n)).copied().unwrap_or_default();
                let key = TargetUse {
                    width: desc.width,
                    height: desc.height,
                    depth: desc.depth,
                    format: desc.format,
                    slot: o.at.slot,
                    operation,
                };
                self.targets.entry(key).or_default().add(self.frames);
            };
            match o.op {
                GuestOp::Draw(_) => {}
                GuestOp::Bind { colour, depth } => {
                    add(colour, "bind_colour");
                    add(depth, "bind_depth");
                }
                GuestOp::Clear { colour, depth, .. } => {
                    add(colour, "clear_colour_attachment");
                    add(depth, "clear_depth_attachment");
                }
                GuestOp::Copy { from, to } => {
                    add(from, "copy_from");
                    add(to, "copy_to");
                }
                GuestOp::Upload { to } => add(to, "upload"),
                GuestOp::Sample { of, how } => add(
                    of,
                    match how {
                        TargetSample::Snapshot => "snapshot",
                        TargetSample::InPlace => "sample_in_place",
                        TargetSample::Depth => "sample_depth",
                    },
                ),
            }
        }
        self.game_frames += u64::from(game);
        for &(at, kind) in &f.readbacks {
            self.reads
                .entry((at.slot, kind.name()))
                .or_default()
                .add(self.frames);
        }
    }
}

static CENSUS: Mutex<Option<Census>> = Mutex::new(None);

pub(super) fn record<D>(f: &Carried<D>) {
    CENSUS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Census::default)
        .record(f);
}

/// Returns no census until a sink has recorded at least one frame.
pub fn census() -> Option<Census> {
    CENSUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::{AuxTarget, Readback, SlotAt};

    #[test]
    fn transient_targets_and_reads_survive_their_frame_and_aux_ids_can_be_reused() {
        let mut census = Census::default();
        let at = SlotAt {
            slot: Slot::AfterLights,
            view: 0,
        };
        let overlay = SlotAt {
            slot: Slot::Overlay,
            view: 0,
        };
        let aux = TargetRef::Aux(0);
        let mut f: Carried<()> = Carried::default();
        f.aux.push(AuxTarget {
            width: 5,
            height: 7,
            format: 6,
            depth: false,
        });
        f.push(
            at,
            GuestOp::Copy {
                from: TargetRef::SceneColour,
                to: aux,
            },
        );
        f.push(
            at,
            GuestOp::Sample {
                of: aux,
                how: TargetSample::Snapshot,
            },
        );
        f.readback(at, Readback::Surface);
        f.readback(overlay, Readback::Mirror);
        census.record(&f);

        f.clear();
        f.push(overlay, GuestOp::Draw(()));
        census.record(&f);
        assert_eq!(census.targets.len(), 2);

        f.clear();
        f.aux.push(AuxTarget {
            width: 3,
            height: 3,
            format: 9,
            depth: true,
        });
        f.push(
            at,
            GuestOp::Sample {
                of: aux,
                how: TargetSample::Depth,
            },
        );
        f.push(
            at,
            GuestOp::Sample {
                of: aux,
                how: TargetSample::Depth,
            },
        );
        f.push(
            overlay,
            GuestOp::Copy {
                from: aux,
                to: TargetRef::SceneColour,
            },
        );
        census.record(&f);
        assert_eq!((census.frames, census.game_frames), (3, 2));
        let rows: Vec<_> = census
            .targets
            .iter()
            .map(|(k, v)| {
                (
                    k.width,
                    k.height,
                    k.operation,
                    v.count,
                    v.first_frame,
                    v.last_frame,
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (3, 3, "sample_depth", 2, 3, 3),
                (5, 7, "copy_to", 1, 1, 1),
                (5, 7, "snapshot", 1, 1, 1)
            ]
        );
        assert_eq!(census.reads[&(at.slot, "surface")].count, 1);
        assert_eq!(census.reads[&(overlay.slot, "mirror")].last_frame, 1);
    }
}
