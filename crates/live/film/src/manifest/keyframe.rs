//! Snapshot disk state relative to DATA. Restore materialises the selected
//! state directly into a new replay directory, even for deletions or renames.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize, Deserialize)]
pub(super) struct Delta<P = PathBuf, E = Item> {
    pub(super) removed: Vec<P>,
    pub(super) changed: Vec<E>,
}

impl Manifest {
    /// Track changes relative to the sealed DATA manifest, even when the
    /// current disk has already been restored from a later keyframe.
    pub fn disk_tracker(&self, root: &Path) -> DiskTracker {
        let mut tracker = DiskTracker::new(root);
        tracker.set_initial(&self.entries);
        tracker
    }

    pub fn keyframe_delta(&self, root: &Path) -> Result<Vec<u8>> {
        self.disk_tracker(root).capture()
    }

    pub fn restore_keyframe(
        mut self,
        root: &Path,
        initial: Option<&Path>,
        delta: &[u8],
    ) -> Result<()> {
        if delta.is_empty() {
            return self.restore_with(root, initial);
        }
        let delta: Delta = serde_json::from_slice(delta)?;
        ensure!(
            delta.changed.len() <= 65536 && delta.removed.len() <= 65536,
            "film disk delta too large"
        );
        let mut entries: BTreeMap<_, _> = self
            .entries
            .into_iter()
            .map(|e| (e.path.clone(), e))
            .collect();
        let mut left = LIMIT;
        let mut touched = BTreeSet::new();
        for path in delta.removed {
            ensure!(
                entries.remove(&path).is_some() && touched.insert(path),
                "invalid keyframe removal"
            );
        }
        for e in delta.changed {
            ensure!(touched.insert(e.path.clone()), "duplicate keyframe change");
            if let Some(data) = &e.data {
                left = left
                    .checked_sub(data.len())
                    .context("changed guest files exceed keyframe limit")?;
            } else {
                ensure!(e.sha256.is_empty(), "missing changed file contents");
            }
            entries.insert(e.path.clone(), e);
        }
        self.entries = entries.into_values().collect();
        self.restore_limited(root, initial, LIMIT * 2)
    }
}
