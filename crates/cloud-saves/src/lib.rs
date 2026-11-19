//! Cloud saves (docs/live/live-cloud-saves.md): a signed-in account's save
//! storage follows it to any machine.
//!
//! - [`local`]: what this machine has. U: (`<data>/disk/Partition1/UDATA/
//!   4D530064`, every XAPI save container: profiles, saved variants,
//!   campaign saves) as `save` items, and our per-profile settings as
//!   `settings` items.
//! - [`merge`]: the pure three-way merge of the last agreed state, the
//!   local items and the server's, with the conflict rules.
//! - [`state`]: the last agreed state and the upload queue, kept on disk.
//! - [`remote`]: the server's storage endpoints ([`remote::HttpRemote`]).
//! - [`watch`]: VFS change reports to item names, debounced.
//! - [`engine`]: sync at sign-in, then pushes while the game runs.
//! - [`session`]: both for one signed-in account, as every host starts them.

pub mod engine;
pub mod local;
pub mod merge;
pub mod remote;
pub mod session;
pub mod state;
pub mod watch;

use serde::{Deserialize, Serialize};

pub use live_proto::maps::Sha256;
pub use live_proto::storage::{KIND_SAVE, KIND_SETTINGS, StorageItem};

/// An item: its kind and its name within the kind.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Key {
    pub kind: String,
    pub name: String,
}

impl Key {
    pub fn new(kind: &str, name: &str) -> Self {
        Key {
            kind: kind.into(),
            name: name.into(),
        }
    }

    pub fn save(name: &str) -> Self {
        Key::new(KIND_SAVE, name)
    }

    /// What a conflict is decided for as a whole: one save container
    /// (`<directory>/...`), or the item itself.
    pub fn group(&self) -> (&str, &str) {
        let group = match self.kind.as_str() {
            KIND_SAVE => self.name.split('/').next().unwrap_or(&self.name),
            _ => &self.name,
        };
        (&self.kind, group)
    }

    /// Whether this item is `name` or inside it (`name/...`).
    pub fn within(&self, kind: &str, name: &str) -> bool {
        self.kind == kind
            && (self.name == name
                || self
                    .name
                    .strip_prefix(name)
                    .is_some_and(|rest| rest.starts_with('/')))
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.kind, self.name)
    }
}

/// An item as this machine has it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalItem {
    pub sha256: Sha256,
    pub size: u64,
    /// When it last changed, Unix milliseconds (the file's modification
    /// time).
    pub modified: i64,
    /// A `SaveMeta.xbx` or `TitleMeta.xbx` whose bytes are damaged (empty
    /// or torn: [`meta_damaged`]). The merge treats it as unchanged since
    /// the last sync, so the server's copy replaces it; it is never
    /// uploaded, and never deleted on the server.
    pub damaged: bool,
}

/// Whether the save item `name` is a meta file and `bytes` are damaged:
/// not UTF-16LE with a byte-order mark and a `Name=` line. The game never
/// writes one so; a process killed between XCreateSaveGame's truncate and
/// its write leaves it.
pub fn meta_damaged(name: &str, bytes: &[u8]) -> bool {
    vfs::save_meta::is_meta_file(name) && vfs::save_meta::meta_damaged(bytes)
}

/// Unix milliseconds now.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
