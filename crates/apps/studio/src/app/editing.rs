//! The scenario tool's edits of many placements at once and the camera
//! commands: delete and duplicate a multi-selection, place a new entry
//! where the cursor met the level, frame the selection.
use scenario_edit::BlockKind;

use super::App;

impl App {
    /// Delete every selected placement as one undo step.
    pub(super) fn delete_selection(&mut self) {
        let r = self.session.remove_selection();
        if let Some(n) = self.ok_or_log(r) {
            self.info(format!("deleted {n} placements"));
        }
    }

    /// Copy every selected placement in place as one undo step; the copies
    /// are selected.
    pub(super) fn duplicate_selection(&mut self) {
        let r = self.session.duplicate_selection();
        if let Some(copies) = self.ok_or_log(r) {
            self.info(format!("duplicated {} placements", copies.len()));
        }
    }

    /// Add a placement at `at` (snapped by the viewport when snapping is on).
    pub(super) fn place_at(&mut self, kind: BlockKind, like: Option<usize>, at: [f32; 3]) {
        let r = self.session.add_entry_at(kind, like, at);
        if let Some(n) = self.ok_or_log(r) {
            self.info(format!(
                "placed {kind:?} {n} at {:.2}, {:.2}, {:.2}",
                at[0], at[1], at[2]
            ));
        }
    }

    /// Bring the camera round to what is selected, framed to its size (the
    /// viewport glides there).
    pub(super) fn frame_selection(&mut self) {
        match self.session.selection_sphere() {
            Some((center, radius)) => {
                let to = crate::camera::frame_sphere(self.session.camera(), center, radius);
                *self.session.camera_mut() = to;
            }
            None => self.session.focus_selected(),
        }
    }
}
