//! Boundary with the film library and the playback agent. The library DTO
//! keeps UI-only fields optional when the recording does not supply them.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use agent_proto::{CallError, Client};
use debug_menu::{Call, Rpc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    Host,
    #[default]
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

#[derive(Clone, Debug, Default)]
pub struct FilmMeta {
    pub id: String,
    pub map: String,
    pub game_type: Option<String>,
    pub mode: Option<String>,
    pub variant_name: String,
    pub started_ms: u64,
    /// Absolute end tick for an exact recording.
    pub length_ticks: u64,
    /// Match duration when the archive knows the match boundaries.
    pub duration_ticks: Option<u64>,
    pub scoreboard: Vec<Player>,
    pub source: Source,
    pub image_stamp: String,
    pub playable_here: bool,
    pub saved: bool,
    pub result: Option<String>,
    pub host: Option<String>,
    pub live_game_id: Option<String>,
    pub kill_ticks: Vec<u64>,
}

/// The selector consumes summaries without depending on the exact film schema.
pub trait FilmSummary {
    fn map(&self) -> &str;
    fn game_type(&self) -> Option<&str>;
    fn mode(&self) -> Option<&str>;
    fn variant_name(&self) -> &str;
    fn duration(&self) -> Option<u64>;
    fn scoreboard(&self) -> &[Player];
}

impl FilmSummary for FilmMeta {
    fn map(&self) -> &str {
        &self.map
    }

    fn game_type(&self) -> Option<&str> {
        self.game_type.as_deref()
    }

    fn mode(&self) -> Option<&str> {
        self.mode.as_deref()
    }

    fn variant_name(&self) -> &str {
        &self.variant_name
    }

    fn duration(&self) -> Option<u64> {
        self.duration_ticks
            .or((self.length_ticks > 0).then_some(self.length_ticks))
    }

    fn scoreboard(&self) -> &[Player] {
        &self.scoreboard
    }
}

impl FilmMeta {
    pub(super) fn archive_id(&self) -> Option<&str> {
        self.id.strip_prefix("live:")
    }

    pub(super) fn source_badge(&self) -> &'static str {
        match self.source {
            Source::Host => "Host film",
            Source::Local => "Local film",
            Source::Archive => "Live summary",
        }
    }

    pub(super) fn watch_unavailable(&self) -> Option<&'static str> {
        match self.source {
            Source::Archive => Some("No host recording"),
            Source::Local => Some("Local recording cannot be replayed"),
            Source::Host if !self.playable_here => Some("Recording needs a different build"),
            Source::Host => None,
        }
    }
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
        let campaign = film.mode == "campaign" || film.kind.as_deref() == Some("campaign");
        let mode = match film.kind.as_deref() {
            _ if campaign => {
                let name = if film.online == Some(true) || film.players.len() > 1 {
                    "Co-op"
                } else {
                    "Campaign"
                };
                film.difficulty.as_ref().map_or_else(
                    || name.into(),
                    |difficulty| format!("{name} / {}", title_words(difficulty)),
                )
            }
            Some("matchmaking" | "clankermatch") => film.playlist.as_ref().map_or_else(
                || "Matchmaking".into(),
                |playlist| format!("Matchmaking / {playlist}"),
            ),
            _ if matches!(film.mode.as_str(), "system_link" | "system-link") => {
                "System Link".into()
            }
            Some("custom") => "Custom Game".into(),
            _ => title_words(&film.mode),
        };
        let game_type = if campaign {
            Some("Campaign".into())
        } else {
            film.variant.as_deref().and_then(game_type)
        };
        let scoreboard = film
            .players
            .into_iter()
            .map(|player| {
                let score = film
                    .scoreboard
                    .iter()
                    .find(|score| score.index == player.index);
                Player {
                    name: player.name,
                    team: (player.team >= 0).then(|| team_name(player.team)),
                    score: score.and_then(|s| i32::try_from(s.score).ok()),
                    kills: score.map(|s| s.kills),
                    deaths: score.map(|s| s.deaths),
                }
            })
            .collect();
        Ok(Self {
            id: format!("live:{}", film.game_id),
            map: film.map,
            game_type,
            mode: Some(mode),
            variant_name: film.variant.unwrap_or_default(),
            started_ms,
            duration_ticks: film.duration_s.map(|s| s.saturating_mul(30)),
            scoreboard,
            source: if film.has_host_film {
                Source::Host
            } else {
                Source::Archive
            },
            playable_here: film.has_host_film,
            result: film.result.map(|result| title_words(&result)),
            live_game_id: Some(film.game_id),
            ..Self::default()
        })
    }
}

fn team_name(team: i32) -> String {
    [
        "Red", "Blue", "Green", "Orange", "Purple", "Gold", "Brown", "Pink",
    ]
    .get(team as usize)
    .map_or_else(|| team.to_string(), |name| (*name).to_owned())
}

pub fn game_type(variant: &str) -> Option<String> {
    let variant = variant.to_ascii_lowercase();
    let words: Vec<_> = variant
        .split(|c: char| !c.is_ascii_alphanumeric())
        .collect();
    for (engine, title) in [
        ("slayer", "Slayer"),
        ("ctf", "CTF"),
        ("oddball", "Oddball"),
        ("king", "King of the Hill"),
        ("race", "Race"),
        ("headhunter", "Headhunter"),
        ("juggernaut", "Juggernaut"),
        ("territories", "Territories"),
        ("assault", "Assault"),
    ] {
        if words.contains(&engine) {
            return Some(title.into());
        }
    }
    None
}

pub(super) fn title_words(text: &str) -> String {
    text.split(['_', ' ', '-'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            let first = chars.next().expect("nonempty word");
            format!("{}{}", first.to_uppercase(), chars.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn archive_path(dir: &Path, game: &str) -> Result<PathBuf, String> {
    if !live_proto::game::valid_id(game) {
        return Err("Invalid Live film ID".into());
    }
    Ok(dir.join("live").join(format!("{game}.h2film")))
}

pub(super) fn download_film(
    archive: &live_hle::films::Archive,
    dir: &Path,
    game: &str,
) -> Result<(), String> {
    let path = archive_path(dir, game)?;
    std::fs::create_dir_all(path.parent().expect("archive directory"))
        .map_err(|e| e.to_string())?;
    let part = path.with_extension(format!("{}.part", live_proto::sign::random_id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&part).map_err(|e| e.to_string())?;
    let result = (|| {
        archive.download(game, file)?;
        let reader = film::Reader::open(&part).map_err(|e| format!("Invalid host film: {e}"))?;
        if reader.header.meta.source != film::Source::Host {
            return Err("Live did not return a host recording".into());
        }
        std::fs::rename(&part, &path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(part);
    }
    result
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
    fn write(path: &Path, edit: impl FnOnce(&mut Vec<Clip>)) -> anyhow::Result<Vec<Clip>> {
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
                    if replies
                        .send((id, answer.map_err(|e| e.to_string())))
                        .is_err()
                    {
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
