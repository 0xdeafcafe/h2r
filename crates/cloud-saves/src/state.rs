//! What this machine remembers between runs, per server and account:
//! `<data>/cloud-saves/<server id>/<xuid>/state.json` holds the base (each
//! item's version and hash when both sides last agreed) and the upload
//! queue (items changed here that the server has not taken yet). Conflict
//! copies go beside it, in `conflicts/`.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Key, Sha256};

/// One item when both sides last agreed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Base {
    pub version: u64,
    /// `None`: deleted at that version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<Sha256>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default, with = "entries")]
    pub base: BTreeMap<Key, Base>,
    /// Items to push when the server can be reached.
    #[serde(default)]
    pub queue: BTreeSet<Key>,
}

/// The map as a list: JSON keys must be strings.
mod entries {
    use super::*;
    use serde::{Deserializer, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Entry {
        #[serde(flatten)]
        key: Key,
        #[serde(flatten)]
        base: Base,
    }

    pub fn serialize<S: Serializer>(map: &BTreeMap<Key, Base>, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(map.iter().map(|(key, base)| Entry {
            key: key.clone(),
            base: base.clone(),
        }))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<Key, Base>, D::Error> {
        Ok(Vec::<Entry>::deserialize(d)?
            .into_iter()
            .map(|e| (e.key, e.base))
            .collect())
    }
}

/// Where one account's state lives.
#[derive(Debug, Clone)]
pub struct Places {
    pub dir: PathBuf,
}

impl Places {
    /// `<data>/cloud-saves/<server id>/<xuid>`. A server id is a host name
    /// (and port); anything else in it becomes `_`.
    pub fn new(data: &Path, server_id: &str, xuid: &str) -> Self {
        let safe = |s: &str| -> String {
            s.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect()
        };
        Places {
            dir: data.join("cloud-saves").join(safe(server_id)).join(safe(xuid)),
        }
    }

    pub fn state_file(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    /// `conflicts/<stamp>-<side>/<kind>/<name>`.
    pub fn conflict_copy(&self, stamp: &str, side: &str, key: &Key) -> PathBuf {
        let mut p = self
            .dir
            .join("conflicts")
            .join(format!("{stamp}-{side}"))
            .join(&key.kind);
        for segment in key.name.split('/') {
            p.push(segment);
        }
        p
    }
}

impl State {
    /// The state in `places`; empty when there is none. A damaged file is
    /// set aside (`state.json.damaged`) and treated as none: the merge then
    /// sees every item as new on both sides and keeps conflict copies
    /// rather than guess.
    pub fn load(places: &Places) -> io::Result<State> {
        let path = places.state_file();
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(s) => Ok(s),
                Err(e) => {
                    eprintln!(
                        "cloud saves: {} is damaged ({e}); starting over, keeping copies of anything that differs",
                        path.display()
                    );
                    let _ = std::fs::rename(&path, path.with_extension("json.damaged"));
                    Ok(State::default())
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(e),
        }
    }

    /// Writes it whole: a temporary file, then a rename.
    pub fn save(&self, places: &Places) -> io::Result<()> {
        std::fs::create_dir_all(&places.dir)?;
        let path = places.state_file();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(io::Error::other)?)?;
        std::fs::rename(&tmp, &path)
    }

    /// Records what both sides now agree on; `None` with version 0
    /// forgets the item.
    pub fn agree(&mut self, key: &Key, version: u64, sha256: Option<Sha256>) {
        if version == 0 && sha256.is_none() {
            self.base.remove(key);
        } else {
            self.base.insert(key.clone(), Base { version, sha256 });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_survives_damage() {
        let dir = std::env::temp_dir().join(format!("h2-cloud-state-{}", std::process::id()));
        let places = Places::new(&dir, "live.example:8074", "0009000000000001");
        assert!(places.dir.ends_with("cloud-saves/live.example_8074/0009000000000001"));
        let mut s = State::default();
        s.agree(&Key::save("A/b"), 3, Some(Sha256::of(b"x")));
        s.agree(&Key::save("A/c"), 4, None);
        s.queue.insert(Key::new("settings", "6368"));
        s.save(&places).unwrap();
        assert_eq!(State::load(&places).unwrap(), s);
        s.agree(&Key::save("A/c"), 0, None);
        assert_eq!(s.base.len(), 1);
        std::fs::write(places.state_file(), b"{not json").unwrap();
        assert_eq!(State::load(&places).unwrap(), State::default());
        assert!(places.state_file().with_extension("json.damaged").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
