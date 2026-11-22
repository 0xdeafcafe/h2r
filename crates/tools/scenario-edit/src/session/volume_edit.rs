//! Editing trigger volumes and kill triggers (docs/editor/polish-plan.md,
//! slice P9): move, turn and resize a box through the gizmo, set its frame
//! exactly, add one, make a kill volume, and remove one. Each is one undo
//! step; a gizmo gesture is one step however many frames it runs.
//!
//! The volume blocks are the editor's own list
//! ([`crate::volumes::VolumeBlock`]), not `map::scenario_layout` blocks:
//! Forge documents share that layout.
use super::*;
use crate::edit::Op;
use crate::volumes::{self, Frame};

/// The size of a volume added without one: a two-unit cube.
pub const NEW_VOLUME_SIZE: Vec3 = [2.0, 2.0, 2.0];

impl Session {
    fn scenario_or_closed(&self) -> Result<&Scenario> {
        self.scenario().ok_or(Error::Save("no map is open"))
    }

    /// The selected trigger volume's frame, when it is in world space (a
    /// volume with a parent object is placed by the running game only).
    pub fn selected_volume_frame(&self) -> Option<Frame> {
        let v = self.volumes()?.triggers.get(self.selected_volume()?)?;
        v.in_world().then_some(v.frame)
    }

    /// Why entries of a volume block may not be added, if they may not:
    /// the scenario's rule, then growth past the source size while
    /// growth is off ([`GROWTH_REFUSAL`]).
    pub fn volume_add_refusal(&self, kind: VolumeKind) -> Option<String> {
        let s = self.scenario()?;
        if let Some(why) = s.volume_refusal(kind) {
            return Some(why);
        }
        let b = s.volume_block(kind);
        (!self.allow_growth && b.len() >= b.capacity()).then(|| GROWTH_REFUSAL.to_owned())
    }

    /// Why a volume entry may not be removed, if it may not
    /// ([`Scenario::volume_remove_refusal`]).
    pub fn volume_remove_refusal(&self, kind: VolumeKind, index: usize) -> Option<String> {
        self.scenario()?.volume_remove_refusal(kind, index)
    }

    /// The command that gives trigger volume `index` `frame`, labelled by
    /// what changed: "move", "turn" or "resize".
    fn frame_command(&self, index: usize, frame: &Frame) -> Result<Option<Command>> {
        let s = self.scenario_or_closed()?;
        let block = s.volume_block(VolumeKind::Trigger);
        let like = block
            .entry(index)
            .ok_or_else(|| Error::Declined(format!("no trigger volume {index}")))?;
        let before = volumes::entry_frame(like);
        let frame = frame.squared().ok_or(Error::Value(
            "frame",
            "needs a finite origin, a positive size and two directions that are not parallel",
        ))?;
        let entry = volumes::trigger_entry(Some(like), &frame);
        if entry == like {
            return Ok(None);
        }
        let near = |a: Vec3, b: Vec3| (0..3).all(|k| (a[k] - b[k]).abs() <= 1e-5);
        let what = if !near(before.size, frame.size) {
            "resize"
        } else if !near(before.forward, frame.forward) || !near(before.up, frame.up) {
            "turn"
        } else {
            "move"
        };
        Ok(Some(Command {
            label: format!("{what} {}", s.volumes.label(index)),
            ops: vec![Op::VolumeSet {
                kind: VolumeKind::Trigger,
                index,
                entry,
            }],
            merge: None,
        }))
    }

    /// Give trigger volume `index` `frame` (forward and up are squared) as
    /// one undo step.
    pub fn set_volume_frame(&mut self, index: usize, frame: &Frame) -> Result<()> {
        self.edit_volume_frame(index, frame, None)
    }

    /// As [`Self::set_volume_frame`]; edits with the same merge key in a
    /// row are one undo step (an inspector drag).
    pub fn edit_volume_frame(
        &mut self,
        index: usize,
        frame: &Frame,
        merge: Option<u64>,
    ) -> Result<()> {
        match self.frame_command(index, frame)? {
            Some(cmd) => self.apply(match merge {
                Some(key) => cmd.merging(key),
                None => cmd,
            }),
            None => Ok(()),
        }
    }

    /// Set the selected volume's frame within the current gizmo gesture
    /// (one is begun if needed): the gesture is one undo step.
    pub fn volume_gesture(&mut self, frame: &Frame) -> Result<()> {
        let index = self
            .selected_volume()
            .filter(|_| self.selected_volume_frame().is_some())
            .ok_or(Error::Save("no world-space trigger volume is selected"))?;
        self.begin_gesture();
        let key = self.gesture.expect("just begun");
        match self.frame_command(index, frame)? {
            Some(cmd) => self.apply(cmd.merging(key)),
            None => Ok(()),
        }
    }

    /// A new trigger volume's frame: a [`NEW_VOLUME_SIZE`] box, axis
    /// aligned, centred on the camera focus.
    pub fn new_volume_frame(&self) -> Frame {
        Frame::around(
            self.camera.target,
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            NEW_VOLUME_SIZE,
        )
        .expect("an axis-aligned cube is a box")
    }

    fn add_volumes(&mut self, label: String, ops: Vec<Op>) -> Result<usize> {
        for op in &ops {
            if let Op::VolumeAdd { kind, .. } = op
                && let Some(why) = self.volume_add_refusal(*kind)
            {
                return Err(Error::Declined(why));
            }
        }
        let index = self
            .scenario_or_closed()?
            .volume_block(VolumeKind::Trigger)
            .len();
        self.apply(Command {
            label,
            ops,
            merge: None,
        })?;
        self.select_volume(Some(index));
        Ok(index)
    }

    /// Add a trigger volume (at `frame`, or [`Self::new_volume_frame`]) as
    /// one undo step. Selects and returns it.
    pub fn add_volume(&mut self, frame: Option<Frame>) -> Result<usize> {
        let frame = frame.unwrap_or_else(|| self.new_volume_frame());
        let frame = frame.squared().ok_or(Error::Value(
            "frame",
            "needs a finite origin, a positive size and two directions that are not parallel",
        ))?;
        let entry = volumes::trigger_entry(None, &frame);
        self.add_volumes(
            "add trigger volume".into(),
            vec![Op::VolumeAdd {
                kind: VolumeKind::Trigger,
                entry,
            }],
        )
    }

    /// Make a kill volume: a trigger volume (at `frame`, or
    /// [`Self::new_volume_frame`]) and a kill trigger naming it, as one
    /// undo step. Selects and returns the volume.
    pub fn make_kill_volume(&mut self, frame: Option<Frame>) -> Result<usize> {
        let frame = frame.unwrap_or_else(|| self.new_volume_frame());
        let frame = frame.squared().ok_or(Error::Value(
            "frame",
            "needs a finite origin, a positive size and two directions that are not parallel",
        ))?;
        let index = self
            .scenario_or_closed()?
            .volume_block(VolumeKind::Trigger)
            .len();
        let volume = i16::try_from(index)
            .map_err(|_| Error::Declined("the trigger volume block is full".into()))?;
        self.add_volumes(
            "make kill volume".into(),
            vec![
                Op::VolumeAdd {
                    kind: VolumeKind::Trigger,
                    entry: volumes::trigger_entry(None, &frame),
                },
                Op::VolumeAdd {
                    kind: VolumeKind::Kill,
                    entry: volumes::kill_entry(volume),
                },
            ],
        )
    }

    /// Remove entry `index` of a volume block as one undo step (the last
    /// entry takes its slot). Removing a trigger volume selects the one
    /// now in its slot, if any.
    pub fn remove_volume(&mut self, kind: VolumeKind, index: usize) -> Result<()> {
        if let Some(why) = self.volume_remove_refusal(kind, index) {
            return Err(Error::Declined(why));
        }
        self.apply(Command {
            label: format!("remove {} {index}", kind.noun()),
            ops: vec![Op::VolumeRemove { kind, index }],
            merge: None,
        })?;
        if kind == VolumeKind::Trigger {
            let len = self.volumes().map_or(0, |v| v.triggers.len());
            self.select_volume((index < len).then_some(index));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{self, VOLUMES};

    fn open(bytes: Vec<u8>, name: &str) -> Session {
        let dir = std::env::temp_dir().join(format!(
            "scenario-edit-volume-edit-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("volumes.map");
        std::fs::write(&path, &bytes).unwrap();
        let mut ed = Session::new();
        ed.build_scenes = false;
        ed.open(MapSource::file(&path)).unwrap();
        // The source stays on disk: saving reads it again.
        ed
    }

    fn frame(ed: &Session, i: usize) -> Frame {
        ed.volumes().unwrap().triggers[i].frame
    }

    /// Every frame of a gizmo move, turn or resize lands in one undo step,
    /// and undo restores the source bytes.
    #[test]
    fn a_gizmo_move_turn_and_resize_are_one_undo_step_each() {
        let mut ed = open(fixture::fixture_with_volumes(), "gesture");
        ed.select_volume(Some(0));
        let start = ed.selected_volume_frame().unwrap();
        let source = ed.scenario().unwrap().changes();

        // Move: three frames of one drag.
        for step in 1..=3 {
            let mut f = start;
            f.origin[0] += step as f32;
            ed.volume_gesture(&f).unwrap();
        }
        ed.end_gesture();
        assert_eq!(frame(&ed, 0).origin, [103.0, 100.0, 0.0]);

        // Turn a quarter about the centre, through the gizmo's matrix.
        let moved = frame(&ed, 0);
        let mut m = moved.matrix().unwrap();
        for step in 1..=2 {
            let a = step as f32 * std::f32::consts::FRAC_PI_4;
            let (s, c) = a.sin_cos();
            let size = moved.size;
            m[0..3].copy_from_slice(&[c * size[0], s * size[0], 0.0]);
            m[4..7].copy_from_slice(&[-s * size[1], c * size[1], 0.0]);
            ed.volume_gesture(&Frame::from_matrix(&m).unwrap()).unwrap();
        }
        ed.end_gesture();
        let turned = frame(&ed, 0);
        assert!((turned.forward[1] - 1.0).abs() < 1e-5, "{turned:?}");
        let (c0, c1) = (moved.center().unwrap(), turned.center().unwrap());
        assert!(
            (0..3).all(|k| (c0[k] - c1[k]).abs() < 1e-4),
            "{c0:?} {c1:?}"
        );

        // Resize: stretch forward to six.
        let mut m = turned.matrix().unwrap();
        for v in &mut m[0..3] {
            *v *= 1.5;
        }
        ed.volume_gesture(&Frame::from_matrix(&m).unwrap()).unwrap();
        ed.end_gesture();
        assert!((frame(&ed, 0).size[0] - 6.0).abs() < 1e-4);

        assert!(ed.undo());
        assert!(ed.status().contains("resize"), "{}", ed.status());
        assert!(ed.undo());
        assert!(ed.status().contains("turn"), "{}", ed.status());
        assert!(ed.undo());
        assert!(ed.status().contains("move"), "{}", ed.status());
        assert_eq!(frame(&ed, 0), start);
        assert_eq!(ed.scenario().unwrap().changes(), source);
        assert!(!ed.can_undo());
    }

    /// A saved frame edit patches only the volume's bytes (plus the
    /// header checksum).
    #[test]
    fn a_saved_volume_edit_differs_only_in_the_volume_bytes_and_the_checksum() {
        let bytes = fixture::fixture_with_volumes();
        let mut ed = open(bytes.clone(), "save");
        let mut f = frame(&ed, 1);
        f.origin = [-30.0, -35.0, -5.0];
        f.size = [8.0, 3.0, 2.0];
        ed.set_volume_frame(1, &f).unwrap();
        let changes = ed.scenario().unwrap().changes();
        let entry = fixture::sfile(fixture::VOLUMES_AT + 0x44) as u64;
        let dir =
            std::env::temp_dir().join(format!("scenario-edit-volume-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("saved.map");
        ed.save_as(&out).unwrap();
        let saved = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(saved.len(), bytes.len());
        assert!(changes.grown.is_empty() && changes.tag_grown.is_empty());
        // The header checksum.
        let checksum = 0x2d0;
        for (at, (a, b)) in bytes.iter().zip(&saved).enumerate() {
            if a == b || (checksum..checksum + 4).contains(&at) {
                continue;
            }
            let at = at as u64;
            assert!(
                (entry..entry + 0x44).contains(&at),
                "byte {at:#x} changed outside volume 1"
            );
        }
        let mut map = map::Map::from_reader(std::io::Cursor::new(saved)).unwrap();
        let reread = Scenario::read(&mut map).unwrap().volumes;
        assert_eq!(reread.triggers[1].frame.origin, f.origin);
        assert_eq!(reread.triggers[1].frame.size, f.size);
        assert_eq!(reread.triggers[0].frame.origin, VOLUMES[0].0);
    }

    /// A volume a kill trigger or BSP switch names is not removed, and the
    /// refusal names the holder; the kill trigger itself can go first.
    #[test]
    fn removing_a_named_volume_is_refused_by_its_holder() {
        let mut ed = open(fixture::fixture_with_volumes(), "remove");
        let why = ed.volume_remove_refusal(VolumeKind::Trigger, 1).unwrap();
        assert!(why.contains("kill trigger 0"), "{why}");
        let err = ed.remove_volume(VolumeKind::Trigger, 1).unwrap_err();
        assert!(err.to_string().contains("kill trigger 0"), "{err}");
        let why = ed.volume_remove_refusal(VolumeKind::Trigger, 0).unwrap();
        assert!(why.contains("BSP switch 0 (BSP 0 to 1)"), "{why}");
        assert!(ed.volume_remove_refusal(VolumeKind::BspSwitch, 0).is_some());

        ed.remove_volume(VolumeKind::Kill, 0).unwrap();
        assert!(ed.volumes().unwrap().kills.is_empty());
        ed.remove_volume(VolumeKind::Trigger, 1).unwrap();
        assert_eq!(ed.volumes().unwrap().triggers.len(), 1);
        assert!(ed.undo());
        assert!(ed.undo());
        assert_eq!(ed.volumes().unwrap().kills, [1]);
        assert_eq!(ed.volumes().unwrap().triggers.len(), 2);
        assert!(!ed.is_dirty());
    }

    /// Make a kill volume adds a volume and a kill trigger naming it, and
    /// one undo takes both away. Removing a volume before the last
    /// repoints the holders of the one that moves into its slot.
    #[test]
    fn make_a_kill_volume_adds_both_entries_and_undoes_as_one_step() {
        let mut ed = open(fixture::fixture_with_volumes(), "make-kill");
        assert_eq!(
            ed.make_kill_volume(None).unwrap_err().to_string(),
            format!("refused: {GROWTH_REFUSAL}")
        );
        let mut ed = open(fixture::fixture_with_volumes_at_end(), "make-kill-end");
        ed.set_allow_growth(true);
        ed.camera_mut().target = [5.0, 6.0, 7.0];
        let index = ed.make_kill_volume(None).unwrap();
        assert_eq!(index, 2);
        assert_eq!(ed.selected_volume(), Some(2));
        let v = ed.volumes().unwrap();
        assert_eq!(v.kills, [1, 2]);
        assert_eq!(v.triggers[2].frame.center(), Some([5.0, 6.0, 7.0]));
        assert_eq!(v.triggers[2].parent, None);
        let changes = ed.scenario().unwrap().changes();
        assert_eq!(
            changes.tag_grown.len(),
            2,
            "both blocks outgrow their source"
        );

        assert!(ed.undo());
        assert!(ed.status().contains("make kill volume"), "{}", ed.status());
        assert_eq!(ed.volumes().unwrap().kills, [1]);
        assert_eq!(ed.volumes().unwrap().triggers.len(), 2);
        assert!(!ed.is_dirty());
        assert!(ed.redo());
        assert_eq!(ed.volumes().unwrap().kills, [1, 2]);

        // Saved, both grown blocks read back from their new copies.
        let dir =
            std::env::temp_dir().join(format!("scenario-edit-volume-grow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("grown.map");
        ed.save_as(&out).unwrap();
        let saved = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let mut map = map::Map::from_reader(std::io::Cursor::new(saved)).unwrap();
        let reread = Scenario::read(&mut map).unwrap();
        assert_eq!(reread.volumes.kills, [1, 2]);
        assert_eq!(reread.volumes.triggers.len(), 3);
        assert_eq!(
            reread.volumes.triggers[2].frame.center(),
            Some([5.0, 6.0, 7.0])
        );
        assert_eq!(reread.volume_block(VolumeKind::Trigger).capacity(), 3);
    }

    #[test]
    fn a_removal_repoints_the_kill_trigger_of_the_moved_volume() {
        let mut ed = open(fixture::fixture_with_volumes_at_end(), "repoint");
        ed.set_allow_growth(true);
        ed.add_volume(None).unwrap();
        ed.make_kill_volume(None).unwrap();
        assert_eq!(ed.volumes().unwrap().kills, [1, 3]);
        ed.remove_volume(VolumeKind::Trigger, 2).unwrap();
        assert_eq!(
            ed.volumes().unwrap().kills,
            [1, 2],
            "the kill trigger follows"
        );
        assert!(ed.undo());
        assert_eq!(ed.volumes().unwrap().kills, [1, 3]);
    }

    /// A scripted map keeps its source volumes; bad frames are refused.
    #[test]
    fn a_scripted_map_keeps_its_source_volumes_and_bad_frames_are_refused() {
        let mut bytes = fixture::fixture_with_volumes();
        fixture::put(&mut bytes, fixture::sfile(0x238), 1);
        let mut ed = open(bytes, "scripted");
        assert!(ed.scenario().unwrap().is_scripted());
        ed.remove_volume(VolumeKind::Kill, 0).unwrap();
        let why = ed.volume_remove_refusal(VolumeKind::Trigger, 1).unwrap();
        assert!(why.contains("scripts"), "{why}");

        let mut f = frame(&ed, 0);
        f.size = [0.0, 1.0, 1.0];
        assert!(ed.set_volume_frame(0, &f).is_err());
        f.size = [1.0; 3];
        f.up = f.forward;
        assert!(ed.set_volume_frame(0, &f).is_err());
        assert_eq!(frame(&ed, 0).origin, VOLUMES[0].0);
    }
}
