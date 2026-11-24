//! The settings file of a sound, `<name>.sound.yaml`.
use serde::{Deserialize, Serialize};

use crate::edit::{Edit, MAX_GAIN_DB};

fn one() -> f64 {
    1.0
}

/// One permutation: a WAV file of the package, played with a weight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permutation {
    /// The WAV's path in the package.
    pub file: String,
    /// How often it is picked against the others (above zero).
    #[serde(default = "one")]
    pub weight: f64,
    #[serde(default)]
    pub gain_db: f64,
    /// How often it is skipped when it would repeat, 0 to 1.
    #[serde(default)]
    pub skip_fraction: f64,
    /// Edits of this permutation's file, as the sound's own `edits`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<Edit>,
}

/// What `set` changes of a permutation; a field left `None` stays.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PermutationPatch {
    pub weight: Option<f64>,
    pub gain_db: Option<f64>,
    pub skip_fraction: Option<f64>,
}

impl Permutation {
    pub fn new(file: &str) -> Self {
        Permutation {
            file: file.to_owned(),
            weight: 1.0,
            gain_db: 0.0,
            skip_fraction: 0.0,
            edits: Vec::new(),
        }
    }

    fn check(&self) -> Result<(), String> {
        let f = &self.file;
        if f.is_empty() || f.starts_with(['/', '\\']) || f.contains("..") || f.contains('\0') {
            return Err(format!("{f:?} is not a path inside the package"));
        }
        if !(self.weight.is_finite() && self.weight > 0.0) {
            return Err(format!("weight {}: above zero", self.weight));
        }
        if !(self.gain_db.is_finite() && self.gain_db.abs() <= MAX_GAIN_DB) {
            return Err(format!("gain_db {}: within {MAX_GAIN_DB} dB", self.gain_db));
        }
        if !(0.0..=1.0).contains(&self.skip_fraction) {
            return Err(format!("skip_fraction {}: 0 to 1", self.skip_fraction));
        }
        Ok(())
    }
}

/// A sound's settings: what the WAV does not say. The WAV itself is
/// never rewritten.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoundDoc {
    /// A sound class, by name or number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// The edits of the source WAV, in order; the build and `sound.render`
    /// apply them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<Edit>,
    /// The sound's other permutations, after the source WAV's own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permutations: Vec<Permutation>,
}

impl SoundDoc {
    /// The file's text; an empty file is an empty document.
    pub fn from_yaml(text: &str) -> Result<Self, String> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let doc: SoundDoc = serde_yaml_ng::from_str(text).map_err(|e| e.to_string())?;
        doc.check()?;
        Ok(doc)
    }

    pub fn to_yaml(&self) -> String {
        serde_yaml_ng::to_string(self).unwrap_or_default()
    }

    /// Every permutation's settings, and no file twice.
    pub fn check(&self) -> Result<(), String> {
        for (i, p) in self.permutations.iter().enumerate() {
            p.check().map_err(|e| format!("permutation {i}: {e}"))?;
            if self.permutations[..i].iter().any(|q| q.file == p.file) {
                return Err(format!("permutation {i}: {} is listed twice", p.file));
            }
        }
        Ok(())
    }

    /// The edit list `permutation` names (the source WAV's when `None`).
    pub fn edits_mut(&mut self, permutation: Option<usize>) -> Result<&mut Vec<Edit>, String> {
        match permutation {
            None => Ok(&mut self.edits),
            Some(i) => {
                let n = self.permutations.len();
                self.permutations
                    .get_mut(i)
                    .map(|p| &mut p.edits)
                    .ok_or_else(|| format!("permutation {i}: the sound lists {n}"))
            }
        }
    }

    /// Add a permutation file; its place.
    pub fn add_permutation(&mut self, p: Permutation) -> Result<usize, String> {
        p.check()?;
        if self.permutations.iter().any(|q| q.file == p.file) {
            return Err(format!("{} is listed already", p.file));
        }
        self.permutations.push(p);
        Ok(self.permutations.len() - 1)
    }

    pub fn remove_permutation(&mut self, index: usize) -> Result<Permutation, String> {
        if index >= self.permutations.len() {
            return Err(format!("permutation {index}: the sound lists {}", self.permutations.len()));
        }
        Ok(self.permutations.remove(index))
    }

    pub fn set_permutation(&mut self, index: usize, patch: &PermutationPatch) -> Result<(), String> {
        let n = self.permutations.len();
        let p = self
            .permutations
            .get_mut(index)
            .ok_or_else(|| format!("permutation {index}: the sound lists {n}"))?;
        let mut next = p.clone();
        next.weight = patch.weight.unwrap_or(next.weight);
        next.gain_db = patch.gain_db.unwrap_or(next.gain_db);
        next.skip_fraction = patch.skip_fraction.unwrap_or(next.skip_fraction);
        next.check()?;
        *p = next;
        Ok(())
    }

    pub fn reorder_permutations(&mut self, from: usize, to: usize) -> Result<(), String> {
        let n = self.permutations.len();
        crate::edit::reorder(&mut self.permutations, from, to)
            .map_err(|_| format!("from {from}, to {to}: the sound lists {n}"))
    }
}
