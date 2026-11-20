//! Snapshot disk state relative to DATA. Restore materialises the selected
//! state directly into a new replay directory, even for deletions or renames.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default, Serialize, Deserialize)]
struct Delta {
    removed: Vec<PathBuf>,
    changed: Vec<Item>,
}

impl Manifest {
    pub fn keyframe_delta(&self, root: &Path) -> Result<Vec<u8>> {
        let mut previous: BTreeMap<_, _> = self
            .entries
            .iter()
            .map(|e| (e.path.as_path(), e.sha256.as_str()))
            .collect();
        let mut delta = Delta::default();
        let mut left = LIMIT;
        for mut e in Self::capture(root)?.entries {
            if previous.remove(e.path.as_path()) == Some(e.sha256.as_str()) {
                continue;
            }
            if !e.sha256.is_empty() {
                // Changed large files have only a digest in the normal start
                // manifest. A keyframe must carry their actual new contents.
                if e.data.is_none() {
                    let mut data = Vec::new();
                    fs::File::open(root.join(&e.path))?
                        .take(left as u64 + 1)
                        .read_to_end(&mut data)?;
                    ensure!(
                        data.len() <= left && crate::xbe_digest(&data) == e.sha256,
                        "changed guest file too large or unstable: {}",
                        e.path.display()
                    );
                    e.data = Some(data);
                }
                left = left
                    .checked_sub(e.data.as_ref().unwrap().len())
                    .context("changed guest files exceed keyframe limit")?;
            }
            delta.changed.push(e);
        }
        delta.removed = previous.into_keys().map(Path::to_path_buf).collect();
        if delta.removed.is_empty() && delta.changed.is_empty() {
            return Ok(Vec::new());
        }
        Ok(serde_json::to_vec(&delta)?)
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
