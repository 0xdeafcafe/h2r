//! This machine's side: the U: drive's files on the host, and our
//! per-profile settings.
//!
//! U: is `<data>/disk/Partition1/UDATA/4D530064` (docs/runtime/storage.md):
//! a directory per XAPI save container (`<12 hex digits>/SaveMeta.xbx` and
//! the game's payload: profiles, saved variants, campaign saves), plus the
//! title's files at the top. Every file is a `save` item named by its path
//! below U: with `/` between segments. The VFS keeps its logical metadata
//! in a separate tree (`<data>/metadata`), so only the game's bytes are
//! here.
//!
//! Items are written here only before the game starts (the merge at
//! sign-in), never under a running game: a file replaced behind the VFS's
//! back would surprise it.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use live_proto::storage::{MAX_SEGMENTS, valid_name};
use serde_json::Value as Json;

use crate::{KIND_SAVE, KIND_SETTINGS, Key, LocalItem, Sha256, meta_damaged};

/// Halo 2's U: under a data root.
pub fn udata_dir(data: &Path) -> PathBuf {
    data.join("disk")
        .join("Partition1")
        .join("UDATA")
        .join(format!("{:08X}", live_proto::HALO_2_TITLE_ID.0))
}

/// Our per-profile settings, as cloud saves see them: each profile's
/// values by key. [`settings::Store`] is the real one.
pub trait ProfileSettings: Send + Sync {
    fn snapshot(&self) -> BTreeMap<String, BTreeMap<String, Json>>;
    /// Sets `profile`'s values (a key it does not know is skipped).
    fn apply(&self, profile: &str, values: &BTreeMap<String, Json>) -> Result<(), String>;
    /// When the settings last changed, Unix milliseconds.
    fn modified(&self) -> i64;
}

impl ProfileSettings for settings::Store {
    fn snapshot(&self) -> BTreeMap<String, BTreeMap<String, Json>> {
        self.profiles()
            .into_iter()
            .map(|p| {
                let values = self
                    .profile_values(&p)
                    .into_iter()
                    .map(|(k, v)| {
                        let j = match v {
                            settings::Value::Bool(b) => Json::Bool(b),
                            settings::Value::Int(i) => Json::from(i),
                        };
                        (k.to_string(), j)
                    })
                    .collect();
                (p, values)
            })
            .collect()
    }

    fn apply(&self, profile: &str, values: &BTreeMap<String, Json>) -> Result<(), String> {
        let mut problems = Vec::new();
        for (k, j) in values {
            let v = match j {
                Json::Bool(b) => settings::Value::Bool(*b),
                Json::Number(n) => match n.as_i64() {
                    Some(i) => settings::Value::Int(i),
                    None => continue,
                },
                _ => continue,
            };
            match self.profile_set(profile, k, v) {
                Ok(None) => {}
                Ok(Some(why)) => problems.push(why),
                Err(settings::SetError::Unknown(_)) => {}
                Err(e) => problems.push(format!("{k}: {e:?}")),
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    fn modified(&self) -> i64 {
        self.file()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok())
            .map_or(0, millis)
    }
}

fn millis(t: std::time::SystemTime) -> i64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A profile name as an item name: its UTF-8 in lowercase hex (names
/// hold spaces, quotes and anything else a profile may be called).
pub fn profile_item(profile: &str) -> String {
    profile.bytes().map(|b| format!("{b:02x}")).collect()
}

/// [`profile_item`] undone.
pub fn profile_name(item: &str) -> Option<String> {
    if !item.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..item.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(item.get(i..i + 2)?, 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

/// One profile's settings as an item's bytes: canonical JSON.
fn settings_bytes(values: &BTreeMap<String, Json>) -> Vec<u8> {
    serde_json::to_vec(values).expect("JSON values serialize")
}

/// This machine's items.
#[derive(Clone)]
pub struct Local {
    /// The host directory U: is.
    pub udata: PathBuf,
    pub settings: Option<Arc<dyn ProfileSettings>>,
}

impl Local {
    pub fn new(data: &Path, settings: Option<Arc<dyn ProfileSettings>>) -> Self {
        Local {
            udata: udata_dir(data),
            settings,
        }
    }

    fn path(&self, key: &Key) -> PathBuf {
        let mut p = self.udata.clone();
        for s in key.name.split('/') {
            p.push(s);
        }
        p
    }

    /// Every item, hashed. Files whose names cannot be items are skipped
    /// (and named on stderr).
    pub fn scan(&self) -> io::Result<BTreeMap<Key, LocalItem>> {
        let mut out = BTreeMap::new();
        self.scan_dir(&self.udata, "", 1, &mut out)?;
        out.extend(self.scan_settings());
        Ok(out)
    }

    /// Each profile's settings as an item (in memory: no file is read).
    pub fn scan_settings(&self) -> BTreeMap<Key, LocalItem> {
        let mut out = BTreeMap::new();
        let Some(s) = &self.settings else {
            return out;
        };
        let modified = s.modified();
        for (profile, values) in s.snapshot() {
            if values.is_empty() {
                continue;
            }
            let name = profile_item(&profile);
            if !valid_name(&name) {
                eprintln!("cloud saves: profile {profile:?}'s name is too long to sync");
                continue;
            }
            let bytes = settings_bytes(&values);
            out.insert(
                Key::new(KIND_SETTINGS, &name),
                LocalItem {
                    sha256: Sha256::of(&bytes),
                    size: bytes.len() as u64,
                    modified,
                    damaged: false,
                },
            );
        }
        out
    }

    /// The name U:'s top-level entry `name` has on the host (the guest may
    /// name it in another case); `name` itself when there is none.
    pub fn actual_top(&self, name: &str) -> String {
        std::fs::read_dir(&self.udata)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .find(|n| n.eq_ignore_ascii_case(name))
            .unwrap_or_else(|| name.to_string())
    }

    /// The items at or below `name` (a file, or a save directory), hashed.
    pub fn scan_under(&self, kind: &str, name: &str) -> io::Result<BTreeMap<Key, LocalItem>> {
        let mut out = BTreeMap::new();
        if kind == KIND_SAVE {
            let key = Key::save(name);
            let path = self.path(&key);
            match std::fs::metadata(&path) {
                Ok(m) if m.is_dir() => {
                    let depth = name.split('/').count() + 1;
                    self.scan_dir(&path, &format!("{name}/"), depth, &mut out)?;
                }
                Ok(_) => {
                    if let Some(item) = self.item(&key)? {
                        out.insert(key, item);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        } else {
            out.extend(
                self.scan_settings()
                    .into_iter()
                    .filter(|(k, _)| k.kind == kind && k.name == name),
            );
        }
        Ok(out)
    }

    fn scan_dir(
        &self,
        dir: &Path,
        prefix: &str,
        depth: usize,
        out: &mut BTreeMap<Key, LocalItem>,
    ) -> io::Result<()> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let entry = entry?;
            let Some(file) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            // Our temporary files, and the host's (`.DS_Store`): the game
            // makes no names starting with a dot.
            if file.starts_with('.') {
                continue;
            }
            let name = format!("{prefix}{file}");
            let kind = entry.file_type()?;
            if kind.is_dir() {
                if depth < MAX_SEGMENTS {
                    self.scan_dir(&entry.path(), &format!("{name}/"), depth + 1, out)?;
                }
            } else if kind.is_file() {
                if !valid_name(&name) {
                    eprintln!("cloud saves: U:\\{} cannot be synced (its name)", name.replace('/', "\\"));
                    continue;
                }
                let key = Key::save(&name);
                if let Some(item) = self.item(&key)? {
                    out.insert(key, item);
                }
            }
        }
        Ok(())
    }

    fn item(&self, key: &Key) -> io::Result<Option<LocalItem>> {
        let Some(bytes) = self.read(key)? else {
            return Ok(None);
        };
        let modified = match key.kind.as_str() {
            KIND_SAVE => std::fs::metadata(self.path(key))
                .and_then(|m| m.modified())
                .map_or(0, millis),
            _ => self.settings.as_ref().map_or(0, |s| s.modified()),
        };
        Ok(Some(LocalItem {
            sha256: Sha256::of(&bytes),
            size: bytes.len() as u64,
            modified,
            damaged: key.kind == KIND_SAVE && meta_damaged(&key.name, &bytes),
        }))
    }

    /// The item's bytes; `None` if this machine has no such item.
    pub fn read(&self, key: &Key) -> io::Result<Option<Vec<u8>>> {
        match key.kind.as_str() {
            KIND_SAVE => match std::fs::read(self.path(key)) {
                Ok(b) => Ok(Some(b)),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::IsADirectory
                    ) =>
                {
                    Ok(None)
                }
                Err(e) => Err(e),
            },
            KIND_SETTINGS => {
                let (Some(s), Some(profile)) = (&self.settings, profile_name(&key.name)) else {
                    return Ok(None);
                };
                Ok(s.snapshot()
                    .get(&profile)
                    .filter(|v| !v.is_empty())
                    .map(settings_bytes))
            }
            _ => Ok(None),
        }
    }

    /// Writes the item: a temporary file beside it, then a rename, with
    /// `modified` as its modification time. Settings go through the store.
    pub fn write(&self, key: &Key, bytes: &[u8], modified: i64) -> io::Result<()> {
        match key.kind.as_str() {
            KIND_SAVE => {
                let path = self.path(key);
                let dir = path.parent().expect("an item has a parent");
                std::fs::create_dir_all(dir)?;
                let tmp = dir.join(format!("{TMP_PREFIX}{}", std::process::id()));
                std::fs::write(&tmp, bytes)?;
                if modified > 0 {
                    let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(modified as u64);
                    let f = std::fs::File::options().write(true).open(&tmp)?;
                    f.set_modified(t)?;
                }
                std::fs::rename(&tmp, &path)
            }
            KIND_SETTINGS => {
                let (Some(s), Some(profile)) = (&self.settings, profile_name(&key.name)) else {
                    return Err(io::Error::other("no settings to write a profile's values into"));
                };
                let values: BTreeMap<String, Json> =
                    serde_json::from_slice(bytes).map_err(io::Error::other)?;
                s.apply(&profile, &values).map_err(io::Error::other)
            }
            k => Err(io::Error::other(format!("no such kind {k}"))),
        }
    }

    /// Deletes the item, and its save directory once that is empty.
    /// Settings values cannot be unset: a deleted profile's values stay.
    pub fn remove(&self, key: &Key) -> io::Result<()> {
        if key.kind != KIND_SAVE {
            return Ok(());
        }
        let path = self.path(key);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let mut dir = path.parent();
        while let Some(d) = dir {
            if d == self.udata || std::fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
        Ok(())
    }
}

/// Our temporary files in U: start with this (not a name the game makes).
const TMP_PREFIX: &str = ".h2cloud-";

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Profile settings in memory.
    #[derive(Default)]
    pub struct FakeSettings {
        pub values: Mutex<BTreeMap<String, BTreeMap<String, Json>>>,
        pub modified: Mutex<i64>,
    }

    impl ProfileSettings for FakeSettings {
        fn snapshot(&self) -> BTreeMap<String, BTreeMap<String, Json>> {
            self.values.lock().unwrap().clone()
        }
        fn apply(&self, profile: &str, values: &BTreeMap<String, Json>) -> Result<(), String> {
            self.values
                .lock()
                .unwrap()
                .entry(profile.into())
                .or_default()
                .extend(values.clone());
            Ok(())
        }
        fn modified(&self) -> i64 {
            *self.modified.lock().unwrap()
        }
    }

    pub fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "h2-cloud-{name}-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn profile_names_round_trip() {
        for p in ["Chief", "Arbiter \"2\"", "caf\u{e9}"] {
            assert_eq!(profile_name(&profile_item(p)).as_deref(), Some(p));
        }
        assert_eq!(profile_item("Chief"), "4368696566");
        assert_eq!(profile_name("4"), None);
        assert_eq!(profile_name("zz"), None);
    }

    #[test]
    fn scans_writes_and_removes_a_save_tree() {
        let data = temp("local");
        let settings = Arc::new(FakeSettings::default());
        settings.values.lock().unwrap().insert(
            "Chief".into(),
            BTreeMap::from([("skulls.iron".to_string(), Json::Bool(true))]),
        );
        *settings.modified.lock().unwrap() = 1234;
        let local = Local::new(&data, Some(settings.clone()));
        assert!(local.udata.ends_with("disk/Partition1/UDATA/4D530064"));
        assert!(local.scan().unwrap().keys().all(|k| k.kind == KIND_SETTINGS));
        let meta = Key::save("0123456789AB/SaveMeta.xbx");
        let payload = Key::save("0123456789AB/payload");
        local.write(&meta, b"Name=Chief", 1_600_000_000_000).unwrap();
        local.write(&payload, b"bytes", 0).unwrap();
        local.write(&Key::save("TitleMeta.xbx"), b"TitleName=", 0).unwrap();
        // An invented file the game might make with a name we cannot sync,
        // and the host's own.
        std::fs::write(local.udata.join("bad%name"), b"x").unwrap();
        std::fs::write(local.udata.join(".DS_Store"), b"x").unwrap();
        let items = local.scan().unwrap();
        let names: Vec<String> = items.keys().map(|k| k.to_string()).collect();
        assert_eq!(
            names,
            [
                "save/0123456789AB/SaveMeta.xbx",
                "save/0123456789AB/payload",
                "save/TitleMeta.xbx",
                "settings/4368696566"
            ]
        );
        assert_eq!(items[&meta].modified, 1_600_000_000_000);
        assert_eq!(items[&meta].sha256, Sha256::of(b"Name=Chief"));
        // 8-bit text with no byte-order mark is no meta file the game
        // writes: damaged. A payload file never is.
        assert!(items[&meta].damaged);
        assert!(items[&Key::save("TitleMeta.xbx")].damaged);
        assert!(!items[&payload].damaged);
        let good: Vec<u8> = std::iter::once(0xfeff_u16)
            .chain("Name=Chief\r\n".encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect();
        local.write(&meta, &good, 1_600_000_000_000).unwrap();
        assert!(!local.scan().unwrap()[&meta].damaged);
        let chief = Key::new(KIND_SETTINGS, "4368696566");
        assert_eq!(items[&chief].modified, 1234);
        assert_eq!(
            local.read(&chief).unwrap().unwrap(),
            br#"{"skulls.iron":true}"#
        );
        assert_eq!(local.scan_under(KIND_SAVE, "0123456789AB").unwrap().len(), 2);
        assert_eq!(local.scan_under(KIND_SAVE, "0123456789AB/payload").unwrap().len(), 1);
        assert!(local.scan_under(KIND_SAVE, "FFFFFFFFFFFF").unwrap().is_empty());
        // Settings arrive through the store.
        local
            .write(&Key::new(KIND_SETTINGS, &profile_item("Arbiter")), br#"{"graphics.fov":90}"#, 0)
            .unwrap();
        assert_eq!(settings.snapshot()["Arbiter"]["graphics.fov"], Json::from(90));
        // Removing the last file removes its save directory.
        local.remove(&meta).unwrap();
        assert!(local.udata.join("0123456789AB").exists());
        local.remove(&payload).unwrap();
        assert!(!local.udata.join("0123456789AB").exists());
        assert!(local.udata.exists());
        let _ = std::fs::remove_dir_all(&data);
    }
}
