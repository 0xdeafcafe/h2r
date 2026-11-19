//! Boundary with the film library and the playback agent. The library DTO
//! keeps UI-only fields optional when the recording does not supply them.

use std::path::{Path, PathBuf};

use debug_menu::{Call, Rpc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Host,
    Local,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Player {
    pub name: String,
    #[serde(default)]
    pub team: Option<String>,
    #[serde(default)]
    pub score: Option<i32>,
    #[serde(default)]
    pub kills: Option<u32>,
    #[serde(default)]
    pub deaths: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct FilmMeta {
    pub id: String,
    pub map: String,
    pub variant: String,
    pub started_ms: u64,
    pub length_ticks: u64,
    pub players: Vec<Player>,
    pub source: Source,
    pub image_stamp: String,
    pub playable_here: bool,
    pub saved: bool,
    pub result: Option<String>,
    pub host: Option<String>,
}

pub trait Library: Send + Sync {
    fn list(&self, stamp: &str) -> Result<Vec<FilmMeta>, String>;
    fn save(&self, id: &str) -> Result<(), String>;
    fn unsave(&self, id: &str) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<(), String>;
}

/// An opener for `film::Library::open(dir)`. Kept here so the UI and
/// playback can be exercised against independent in-memory adapters.
pub type OpenLibrary = fn(&Path) -> Result<Box<dyn Library>, String>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clip {
    pub film: String,
    pub from_tick: u64,
    pub to_tick: u64,
}

pub(super) fn load_clips(path: &Path) -> Result<Vec<Clip>, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let clips = serde_json::from_slice::<Vec<Clip>>(&bytes).map_err(|e| e.to_string())?;
            Ok(clips
                .into_iter()
                .filter(|c| c.from_tick < c.to_tick)
                .collect())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

pub(super) fn store_clips(path: &Path, clips: &[Clip]) -> Result<(), String> {
    fn write(path: &Path, clips: &[Clip]) -> anyhow::Result<()> {
        let parent = path.parent().expect("clip path contains the data root");
        std::fs::create_dir_all(parent)?;
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, serde_json::to_vec(clips)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
    write(path, clips).map_err(|e| e.to_string())
}

pub(super) fn clips_path(dir: &Path) -> PathBuf {
    dir.join("theater-clips.json")
}

/// Use the same local JSON-RPC connection as the debug menu.
pub(super) struct Playback {
    rpc: agent::menu::MenuRpc,
    next: u64,
}

impl Playback {
    pub(super) fn new(agent: &agent::Agent) -> Self {
        Self {
            rpc: agent::menu::MenuRpc::new(agent),
            next: 1,
        }
    }

    pub(super) fn send(&mut self, method: &'static str, params: Value) -> u64 {
        let id = self.next;
        self.next += 1;
        self.rpc.send(id, &Call::new(method, params));
        id
    }

    pub(super) fn answers(&mut self) -> Vec<(u64, Result<Value, String>)> {
        self.rpc.recv()
    }

    pub(super) fn play(&mut self, id: &str) -> u64 {
        self.send("theater.play", json!({ "id": id }))
    }
}
