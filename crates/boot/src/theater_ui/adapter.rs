//! Boundary with the film library and the playback agent. The library DTO
//! keeps UI-only fields optional when the recording does not supply them.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use agent_proto::{CallError, Client};
use debug_menu::{Call, Rpc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Host,
    Local,
    Archive,
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
    pub web_game_id: Option<String>,
}

impl TryFrom<live_hle::films::RecentFilm> for FilmMeta {
    type Error = String;

    fn try_from(film: live_hle::films::RecentFilm) -> Result<Self, Self::Error> {
        if !live_proto::game::valid_id(&film.game_id) {
            return Err("Invalid Live film ID".into());
        }
        let date = chrono::DateTime::parse_from_rfc3339(&film.started_at)
            .map_err(|_| "Invalid Live film date".to_owned())?;
        let started_ms = u64::try_from(date.timestamp_millis())
            .map_err(|_| "Invalid Live film date".to_owned())?;
        Ok(Self {
            id: format!("live:{}", film.game_id),
            map: film.map,
            variant: film.variant.unwrap_or(film.mode),
            started_ms,
            length_ticks: film.duration_s.unwrap_or(0).saturating_mul(30),
            players: Vec::new(),
            source: Source::Archive,
            image_stamp: String::new(),
            playable_here: false,
            saved: false,
            result: film.result,
            host: None,
            web_game_id: Some(film.game_id),
        })
    }
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

pub(super) fn update_clips(
    path: &Path,
    edit: impl FnOnce(&mut Vec<Clip>),
) -> Result<Vec<Clip>, String> {
    fn write(
        path: &Path,
        edit: impl FnOnce(&mut Vec<Clip>),
    ) -> anyhow::Result<Vec<Clip>> {
        let parent = path.parent().expect("clip path contains the data root");
        std::fs::create_dir_all(parent)?;
        // The menu and the player can both edit clips. Lock a separate
        // file because publishing the new list replaces the data inode.
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))?;
        lock.lock()?;
        let mut clips = load_clips(path).map_err(anyhow::Error::msg)?;
        edit(&mut clips);
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, serde_json::to_vec(&clips)?)?;
        std::fs::rename(tmp, path)?;
        Ok(clips)
    }
    write(path, edit).map_err(|e| e.to_string())
}

pub(super) fn clips_path(dir: &Path) -> PathBuf {
    dir.join("theater-clips.json")
}

/// Watch uses the menu agent. Once the playback window opens, its
/// controls use the playback process's own agent endpoint.
pub(super) struct Playback {
    rpc: Box<dyn Rpc>,
    next: u64,
}

impl Playback {
    pub(super) fn new(agent: &agent::Agent) -> Self {
        Self {
            rpc: Box::new(agent::menu::MenuRpc::new(agent)),
            next: 1,
        }
    }

    pub(super) fn socket(path: PathBuf) -> std::io::Result<Self> {
        let (calls, requests) = mpsc::channel::<(u64, Call)>();
        let (replies, answers) = mpsc::channel();
        std::thread::Builder::new()
            .name("h2-theater-controls".into())
            .spawn(move || {
                let mut connected = None;
                for (id, call) in requests {
                    let answer = (|| -> Result<Value, CallError> {
                        if connected.is_none() {
                            let client = Client::connect(&path)?;
                            client.set_timeout(Some(Duration::from_secs(2)))?;
                            connected = Some(client);
                        }
                        connected.as_mut().unwrap().call(&call.method, call.params)
                    })();
                    if matches!(&answer, Err(CallError::Io(_) | CallError::Protocol(_))) {
                        connected = None;
                    }
                    if replies.send((id, answer.map_err(|e| e.to_string()))).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            rpc: Box::new(SocketRpc { calls, answers }),
            next: 1,
        })
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
}

struct SocketRpc {
    calls: Sender<(u64, Call)>,
    answers: Receiver<(u64, Result<Value, String>)>,
}

impl Rpc for SocketRpc {
    fn send(&mut self, id: u64, call: &Call) {
        let _ = self.calls.send((id, call.clone()));
    }

    fn recv(&mut self) -> Vec<(u64, Result<Value, String>)> {
        self.answers.try_iter().collect()
    }
}
